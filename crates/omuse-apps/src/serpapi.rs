//! Google Flights, Google Hotels and Google Maps results through SerpApi (serpapi.com): the "Flights & hotels"
//! connection behind the `flights_search`, `flight_offer`, `hotels_search`, `hotel_rooms`, `places_search` and
//! `place_details` tools. The key is for these tools only, never for web search. One key, one GET per search;
//! the long booking / property tokens Google hands back stay in a per-process table and the results carry short ids.

use std::collections::HashMap;
use std::fmt;
use std::sync::{LazyLock, Mutex};
use std::time::Duration;

use serde_json::{json, Map, Value};

use omuse_common::util::{get_str, truthy};

/// A SerpApi failure (HTTP, a refused key, the `error` field of a reply).
#[derive(Debug)]
pub struct SerpError(pub String);

impl fmt::Display for SerpError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for SerpError {}

/// Google's `stops` filter for the request's `max_stops` (0 / 1 / 2) or `nonstop_only`: 1 = nonstop, 2 = at most
/// one stop, 3 = at most two; `None` = any number of stops.
pub fn stops_param(a: &Value) -> Option<u8> {
    max_stops(a).map(|n| n + 1)
}

/// The request's stop limit; `None` when any number of stops is fine. The tool schema has `stops` = any | nonstop |
/// one_stop_max | two_stops_max (a model that fills every field with a default lands on "any", which is what the user
/// meant when they said nothing); direct callers may still send `max_stops` 0..=2 or `nonstop_only`.
pub fn max_stops(a: &Value) -> Option<u8> {
    if let Some(s) = a.get("stops").and_then(Value::as_str) {
        return match s.trim().to_lowercase().replace(['-', ' '], "_").as_str() {
            "nonstop" | "direct" | "0" => Some(0),
            "one_stop_max" | "1" => Some(1),
            "two_stops_max" | "2" => Some(2),
            _ => None,
        };
    }
    if let Some(n) = a.get("max_stops").and_then(Value::as_i64) {
        return Some(n.clamp(0, 2) as u8);
    }
    if truthy(&a["nonstop_only"]) {
        return Some(0);
    }
    None
}

/// A count of people from the request (`adults`, `guests`): `missing` when not given; a whole number from 1 to `max`
/// otherwise — 0, a negative number or anything that is not a number is 1, more than `max` is `max`.
pub fn people(a: &Value, key: &str, missing: i64, max: i64) -> i64 {
    match a.get(key) {
        None | Some(Value::Null) => missing,
        Some(v) => v
            .as_f64()
            .or_else(|| v.as_str().and_then(|s| s.trim().parse::<f64>().ok()))
            .filter(|x| x.is_finite() && *x >= 1.0)
            .map(|x| (x.floor() as i64).min(max))
            .unwrap_or(1),
    }
}

/// A price cap from the request: only a positive number caps anything (a model filling defaults sends 0).
pub fn price_cap(a: &Value, key: &str) -> Option<i64> {
    a.get(key)
        .and_then(Value::as_f64)
        .filter(|m| *m > 0.0)
        .map(|m| m as i64)
}

/// What was searched, in the words the answer should use — appended to every flights result so the cabin, the
/// stop limit and the date the user asked for are never mixed up with another search.
pub fn searched(a: &Value, from: &str, to: &str) -> Value {
    let cabin = match get_str(a, "cabin").to_lowercase().as_str() {
        "premium_economy" | "premium economy" => "premium economy",
        "business" => "business",
        "first" => "first",
        _ => "economy",
    };
    let stops = match max_stops(a) {
        Some(0) => "nonstop only",
        Some(1) => "at most one stop",
        Some(_) => "at most two stops",
        None => "any number of stops",
    };
    let ret = get_str(a, "return_date");
    json!({
        "origin": from, "destination": to, "date": get_str(a, "date"),
        "trip": if ret.is_empty() { "one way".to_string() } else { format!("round trip, return {ret}") },
        "adults": people(a, "adults", 1, 9),
        "cabin": cabin, "stops": stops,
        "max_price": price_cap(a, "max_price").map_or_else(|| json!("no limit (every price)"), |m| json!(m)),
        "rule": format!("every offer below is a {cabin} fare with {stops}; answer in these terms — name the cabin ({cabin}) and the stop limit in the answer — and if the user asked for a different cabin, date or stop limit, search again instead of presenting these{}{}",
            if cabin == "economy" { String::new() } else { format!("; an itinerary whose main legs are in this cabin but with a lower-cabin leg (a short economy connection, `cabin_mix`) counts as a {cabin} itinerary — keep it, and say which legs are in which cabin") },
            if pref_list(a, "airlines").is_empty() { "" } else { "; this search used the user's preferred airlines (offers marked preferred are on them) and is the only one — do not search again without them; when no preferred offer is good (preferred_found 0, or none fits the request), answer with what was found and ask whether to search all airlines" }),
    })
}

/// A list argument (`airlines`, `brands`): a JSON array of strings, or one comma-separated string; trimmed, empty
/// entries dropped.
pub fn pref_list(a: &Value, key: &str) -> Vec<String> {
    let v = match a.get(key) {
        Some(Value::Array(xs)) => xs
            .iter()
            .map(|x| {
                x.as_str()
                    .map(str::to_string)
                    .unwrap_or_else(|| x.to_string())
            })
            .collect::<Vec<_>>(),
        Some(Value::String(s)) => s.split(',').map(str::to_string).collect(),
        _ => Vec::new(),
    };
    v.into_iter()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

/// Flag the items the user prefers: `preferred: true` on every item whose `field` (an airline list such as
/// "Singapore Airlines + ANA", or a hotel name) contains one of `prefs` (case-insensitive), or — for flights — whose
/// first leg's flight number starts with a 2-letter airline code in `prefs`. The other items stay as they are, so
/// the answer can show the price difference. Returns how many were flagged.
pub fn mark_preferred(items: &mut Value, prefs: &[String], field: &str) -> usize {
    if prefs.is_empty() {
        return 0;
    }
    // an alliance or a hotel group stands for all its members
    let names = crate::loyalty::expand(prefs, if field == "name" { "hotel" } else { "airline" });
    let codes: Vec<String> = prefs
        .iter()
        .filter(|p| p.len() == 2 && p.chars().all(|c| c.is_ascii_alphanumeric()))
        .map(|p| p.to_ascii_uppercase())
        .collect();
    let mut n = 0;
    for it in items.as_array_mut().into_iter().flatten() {
        let hay = get_str(it, field).to_lowercase();
        let first_no = arr(it, "legs")
            .first()
            .map(|l| get_str(l, "flight_number"))
            .unwrap_or_default()
            .to_ascii_uppercase();
        let by_name = names
            .iter()
            .any(|p| p.len() > 2 && hay.contains(p.as_str()));
        let by_code = codes.iter().any(|c| first_no.starts_with(c.as_str()));
        if by_name || by_code {
            it["preferred"] = json!(true);
            n += 1;
        }
    }
    n
}

/// Label each offer's cabin for a premium-cabin search: `cabin` = what was searched, and on an itinerary whose legs are
/// not all in that cabin (a business long-haul with an economy connection) `cabin_mix` = "business + economy" with
/// `economy_legs` naming them. Such an itinerary counts as the searched cabin. Economy searches are left alone.
pub fn tag_cabins(offers: &mut Value, a: &Value) {
    let want = match get_str(a, "cabin").to_lowercase().as_str() {
        "premium_economy" | "premium economy" => "premium economy",
        "business" => "business",
        "first" => "first",
        _ => return,
    };
    for o in offers.as_array_mut().into_iter().flatten() {
        let legs: Vec<(String, String)> = arr(o, "legs")
            .iter()
            .map(|l| {
                let c = get_str(l, "cabin").to_lowercase().replace('_', " ");
                (
                    format!(
                        "{} {}-{}",
                        get_str(l, "flight_number"),
                        get_str(l, "from"),
                        get_str(l, "to")
                    ),
                    c,
                )
            })
            .collect();
        o["cabin"] = json!(want);
        // a leg counts as lower when its cabin is economy-like and the searched cabin is above it
        let lower: Vec<&(String, String)> = legs
            .iter()
            .filter(|(_, c)| {
                !c.is_empty()
                    && !c.contains(want)
                    && (c.contains("economy") || c.contains("coach") || c.contains("basic"))
            })
            .collect();
        if !lower.is_empty() && lower.len() < legs.len() {
            let other = if lower.iter().any(|(_, c)| c.contains("premium")) {
                "premium economy"
            } else {
                "economy"
            };
            o["cabin_mix"] = json!(format!("{want} + {other}"));
            o["economy_legs"] = json!(lower
                .iter()
                .map(|(l, _)| l.trim().to_string())
                .collect::<Vec<_>>());
        } else if !lower.is_empty() {
            // every leg is below the searched cabin: not a match, say so plainly
            o["cabin"] = json!("economy (not the cabin searched)");
        }
    }
}

/// The departure dates of a request: `date`, or every day from `date` to `date_to` (inclusive), at most `max_days`;
/// the bool says whether the range was cut. A return date moves with each departure so the stay stays as long.
pub fn date_range(a: &Value, max_days: usize) -> Result<(Vec<(String, String)>, bool), String> {
    let parse = |k: &str| chrono::NaiveDate::parse_from_str(get_str(a, k).trim(), "%Y-%m-%d");
    let first = parse("date").map_err(|_| "date must be YYYY-MM-DD".to_string())?;
    let last = match get_str(a, "date_to").trim() {
        "" => first,
        _ => parse("date_to").map_err(|_| "date_to must be YYYY-MM-DD".to_string())?,
    };
    if last < first {
        return Err("date_to is before date".into());
    }
    let stay = parse("return_date").ok().map(|r| r - first);
    let span = (last - first).num_days() as usize + 1;
    let days: Vec<(String, String)> = (0..span.min(max_days))
        .map(|i| {
            let d = first + chrono::Duration::days(i as i64);
            (
                d.format("%Y-%m-%d").to_string(),
                stay.map(|s| (d + s).format("%Y-%m-%d").to_string())
                    .unwrap_or_default(),
            )
        })
        .collect();
    Ok((days, span > max_days))
}

/// Merge one search per day into one result: every offer carries its `date`, sorted by price (at most 20), with the
/// cheapest offer of each day in `by_date` and the range in `searched.dates`.
pub fn merge_days(days: Vec<(String, Value)>, cut: bool, asked_to: &str) -> Value {
    let price = |o: &Value| o.get("price").and_then(Value::as_f64);
    let mut offers: Vec<Value> = Vec::new();
    let mut by_date = Vec::new();
    let mut base = Value::Null;
    for (day, res) in days {
        let list: Vec<Value> = arr(&res, "offers").to_vec();
        let cheapest = list
            .iter()
            .filter_map(|o| price(o).map(|p| (p, o)))
            .min_by(|x, y| x.0.partial_cmp(&y.0).unwrap_or(std::cmp::Ordering::Equal));
        by_date.push(match cheapest {
            Some((p, o)) => json!({"date": day, "cheapest": p, "offer_id": o["offer_id"], "airlines": o["airlines"], "count": list.len()}),
            None => json!({"date": day, "count": 0, "note": res.get("note").cloned().unwrap_or(Value::Null)}),
        });
        for mut o in list {
            o["date"] = json!(day);
            offers.push(o);
        }
        if base.is_null() {
            base = res;
        }
    }
    offers.sort_by(|x, y| {
        price(x)
            .unwrap_or(f64::MAX)
            .partial_cmp(&price(y).unwrap_or(f64::MAX))
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    offers.truncate(20);
    for (i, o) in offers.iter_mut().enumerate() {
        o["rank"] = json!(i + 1);
    }
    let first = by_date
        .first()
        .map(|d| get_str(d, "date"))
        .unwrap_or_default();
    let last = by_date
        .last()
        .map(|d| get_str(d, "date"))
        .unwrap_or_default();
    let mut out = base;
    out["count"] = json!(offers.len());
    out["offers"] = json!(offers);
    out["by_date"] = json!(by_date);
    // one day's price insight and Google link do not describe the whole range
    if let Some(o) = out.as_object_mut() {
        o.remove("price_insight");
        o.remove("google_url");
    }
    out["searched"]["date"] = json!(first);
    out["searched"]["dates"] = json!(format!(
        "{first} to {last} ({} days{})",
        by_date.len(),
        if cut {
            format!("; cut short of {asked_to}: search the rest in a second call")
        } else {
            String::new()
        }
    ));
    out
}

/// The SerpApi client for one key.
pub struct SerpApi {
    client: reqwest::Client,
    base: String,
    key: String,
    timeout: Duration,
}

/// Short id -> the Google token it stands for (booking tokens of itineraries, property tokens of hotels).
static TOKENS: LazyLock<Mutex<HashMap<String, String>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

fn fnv(s: &str) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    for b in s.bytes() {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}

/// A short, stable id for a long token; the token can be looked up again with [`token_of`].
pub fn short_id(prefix: &str, token: &str) -> String {
    let id = format!("{}_{:010x}", prefix, fnv(token) & 0xff_ffff_ffff);
    let mut t = TOKENS.lock().unwrap_or_else(|e| e.into_inner());
    if t.len() > 2000 {
        t.clear();
    }
    t.insert(id.clone(), token.to_string());
    id
}

/// The token behind a short id from an earlier result (`None` once it is gone, e.g. after a restart).
pub fn token_of(id: &str) -> Option<String> {
    TOKENS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(id)
        .cloned()
}

pub use crate::airports::airport_codes;

fn num(v: Option<&Value>) -> Value {
    match v {
        Some(Value::Number(n)) => Value::Number(n.clone()),
        Some(Value::String(s)) => s
            .trim()
            .parse::<f64>()
            .ok()
            .map(|f| json!(f))
            .unwrap_or(Value::Null),
        _ => Value::Null,
    }
}

fn arr<'a>(v: &'a Value, k: &str) -> &'a [Value] {
    v.get(k)
        .and_then(Value::as_array)
        .map_or(&[], Vec::as_slice)
}

/// One Google Flights itinerary, compact: airlines, the legs, local times, duration, stops, price, an offer id.
fn compact_itinerary(it: &Value, currency: &str) -> Value {
    let legs: Vec<Value> = arr(it, "flights")
        .iter()
        .map(|l| {
            json!({
                "airline": get_str(l, "airline"),
                "flight_number": get_str(l, "flight_number"),
                "from": get_str(&l["departure_airport"], "id"),
                "depart": get_str(&l["departure_airport"], "time"),
                "to": get_str(&l["arrival_airport"], "id"),
                "arrive": get_str(&l["arrival_airport"], "time"),
                "duration_min": num(l.get("duration")),
                "aircraft": get_str(l, "airplane"),
                "cabin": get_str(l, "travel_class"),
            })
        })
        .collect();
    let mut airlines: Vec<String> = Vec::new();
    for l in &legs {
        let a = get_str(l, "airline");
        if !a.is_empty() && !airlines.contains(&a) {
            airlines.push(a);
        }
    }
    let layovers: Vec<String> = arr(it, "layovers")
        .iter()
        .map(|x| {
            format!(
                "{} {} min{}",
                get_str(x, "id"),
                num(x.get("duration")),
                if truthy(&x["overnight"]) {
                    ", overnight"
                } else {
                    ""
                }
            )
        })
        .collect();
    let token = get_str(it, "booking_token");
    let mut out = Map::new();
    if !token.is_empty() {
        out.insert("offer_id".into(), json!(short_id("fl", &token)));
    }
    out.insert("airlines".into(), json!(airlines.join(" + ")));
    out.insert(
        "route".into(),
        json!(legs
            .iter()
            .map(|l| format!(
                "{} {} {} {} → {} {}",
                get_str(l, "airline"),
                get_str(l, "flight_number"),
                get_str(l, "from"),
                get_str(l, "depart"),
                get_str(l, "to"),
                get_str(l, "arrive")
            ))
            .collect::<Vec<_>>()
            .join(" | ")),
    );
    out.insert("duration_min".into(), num(it.get("total_duration")));
    out.insert("stops".into(), json!(layovers.len()));
    out.insert("layovers".into(), json!(layovers));
    out.insert("price".into(), num(it.get("price")));
    out.insert("currency".into(), json!(currency));
    out.insert("trip".into(), json!(get_str(it, "type")));
    out.insert("legs".into(), Value::Array(legs));
    let notes: Vec<String> = arr(it, "extensions")
        .iter()
        .filter_map(Value::as_str)
        .map(str::to_string)
        .collect();
    if !notes.is_empty() {
        out.insert("notes".into(), json!(notes));
    }
    Value::Object(out)
}

/// The Google Flights reply, compact (best first, at most 12), with Google's own price insight.
pub fn compact_flights(data: &Value, currency: &str) -> Value {
    let mut offers: Vec<Value> = arr(data, "best_flights")
        .iter()
        .chain(arr(data, "other_flights").iter())
        .take(12)
        .map(|it| compact_itinerary(it, currency))
        .collect();
    for (i, o) in offers.iter_mut().enumerate() {
        o["rank"] = json!(i + 1);
    }
    let mut out = json!({
        "count": offers.len(),
        "currency": currency,
        "offers": offers,
    });
    if let Some(pi) = data.get("price_insights").filter(|p| p.is_object()) {
        out["price_insight"] = json!({
            "lowest_price": num(pi.get("lowest_price")),
            "price_level": get_str(pi, "price_level"),
            "typical_range": pi.get("typical_price_range").cloned().unwrap_or(Value::Null),
        });
    }
    let url = get_str(&data["search_metadata"], "google_flights_url");
    if !url.is_empty() {
        out["google_url"] = json!(url);
    }
    if let Some(e) = data.get("error").and_then(Value::as_str) {
        out["note"] = json!(e);
    }
    Value::Object(out.as_object().cloned().unwrap_or_default())
}

/// The booking options of one itinerary (who sells it, the price, the link), compact.
pub fn compact_booking(data: &Value, currency: &str) -> Value {
    let mut options = Vec::new();
    for bo in arr(data, "booking_options") {
        for part in ["together", "departing", "returning"] {
            let Some(o) = bo.get(part).filter(|v| v.is_object()) else {
                continue;
            };
            let req = o.get("booking_request").cloned().unwrap_or(Value::Null);
            let mut item = json!({
                "part": part,
                "book_with": get_str(o, "book_with"),
                "marketed_as": arr(o, "marketed_as").iter().filter_map(Value::as_str).collect::<Vec<_>>().join(", "),
                "price": num(o.get("price")),
                "currency": currency,
                "link": get_str(&req, "url"),
                "link_method": if truthy(&req["post_data"]) { "POST (open it from Google Flights; the airline's or agency's own site is the place to book)" } else { "GET" },
                "title": get_str(o, "option_title"),
            });
            let bags: Vec<String> = arr(o, "baggage_prices")
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect();
            if !bags.is_empty() {
                item["baggage"] = json!(bags);
            }
            let ext: Vec<String> = arr(o, "extensions")
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect();
            if !ext.is_empty() {
                item["notes"] = json!(ext);
            }
            options.push(item);
        }
    }
    let selected: Vec<Value> = arr(data, "selected_flights")
        .iter()
        .map(|it| compact_itinerary(it, currency))
        .collect();
    let mut out = json!({"count": options.len(), "options": options, "itinerary": selected});
    let url = get_str(&data["search_metadata"], "google_flights_url");
    if !url.is_empty() {
        out["google_url"] = json!(url);
    }
    out
}

fn price_of(v: Option<&Value>) -> Value {
    match v {
        Some(x) if x.is_object() => num(x.get("extracted_lowest")),
        other => num(other),
    }
}

/// One Google Hotels property, compact.
fn compact_property(p: &Value, currency: &str) -> Value {
    let token = get_str(p, "property_token");
    let mut out = Map::new();
    if !token.is_empty() {
        out.insert("hotel_id".into(), json!(short_id("ht", &token)));
    }
    out.insert("name".into(), json!(get_str(p, "name")));
    out.insert("type".into(), json!(get_str(p, "type")));
    let stars = p
        .get("extracted_hotel_class")
        .filter(|v| !v.is_null())
        .cloned()
        .unwrap_or_else(|| {
            let s = get_str(p, "hotel_class");
            s.chars()
                .next()
                .and_then(|c| c.to_digit(10))
                .map(|d| json!(d))
                .unwrap_or(Value::Null)
        });
    out.insert("stars".into(), stars);
    out.insert("rating".into(), num(p.get("overall_rating")));
    out.insert("reviews".into(), num(p.get("reviews")));
    out.insert("price_per_night".into(), price_of(p.get("rate_per_night")));
    out.insert("total".into(), price_of(p.get("total_rate")));
    out.insert("currency".into(), json!(currency));
    out.insert(
        "free_cancellation".into(),
        json!(arr(p, "prices")
            .iter()
            .any(|x| truthy(&x["free_cancellation"]))),
    );
    out.insert(
        "near".into(),
        json!(arr(p, "nearby_places")
            .iter()
            .take(2)
            .map(|x| {
                let how = arr(x, "transportations")
                    .first()
                    .map(|t| format!("{} {}", get_str(t, "duration"), get_str(t, "type")))
                    .unwrap_or_default();
                format!("{} ({})", get_str(x, "name"), how.trim())
            })
            .collect::<Vec<_>>()),
    );
    out.insert(
        "amenities".into(),
        json!(arr(p, "amenities")
            .iter()
            .filter_map(Value::as_str)
            .take(8)
            .collect::<Vec<_>>()),
    );
    out.insert("check_in".into(), json!(get_str(p, "check_in_time")));
    out.insert("check_out".into(), json!(get_str(p, "check_out_time")));
    let link = get_str(p, "link");
    if !link.is_empty() {
        out.insert("link".into(), json!(link));
    }
    Value::Object(out)
}

/// The Google Hotels reply, compact (at most 15 properties, ads left out).
pub fn compact_hotels(data: &Value, currency: &str) -> Value {
    let hotels: Vec<Value> = arr(data, "properties")
        .iter()
        .take(15)
        .map(|p| compact_property(p, currency))
        .collect();
    let mut out = json!({"count": hotels.len(), "currency": currency, "hotels": hotels});
    let url = get_str(&data["search_metadata"], "google_hotels_url");
    if !url.is_empty() {
        out["google_url"] = json!(url);
    }
    if let Some(e) = data.get("error").and_then(Value::as_str) {
        out["note"] = json!(e);
    }
    out
}

/// One property's details and rates per seller, compact.
pub fn compact_rooms(data: &Value, currency: &str) -> Value {
    let mut offers = Vec::new();
    for key in ["featured_prices", "prices"] {
        for x in arr(data, key) {
            let rooms: Vec<Value> = arr(x, "rooms")
                .iter()
                .map(|r| {
                    json!({"name": get_str(r, "name"), "rate_per_night": price_of(r.get("rate_per_night")),
                           "total": price_of(r.get("total_rate")), "link": get_str(r, "link")})
                })
                .collect();
            offers.push(json!({
                "source": get_str(x, "source"),
                "rate_per_night": price_of(x.get("rate_per_night")),
                "total": price_of(x.get("total_rate")),
                "currency": currency,
                "free_cancellation": truthy(&x["free_cancellation"]),
                "cancellation": get_str(x, "free_cancellation_until_date"),
                "link": get_str(x, "link"),
                "rooms": rooms,
            }));
        }
    }
    let mut base = compact_property(data, currency);
    base["address"] = json!(get_str(data, "address"));
    base["phone"] = json!(get_str(data, "phone"));
    base["website"] = json!(get_str(data, "website"));
    base["description"] = json!(get_str(data, "description"));
    base["offers"] = json!(offers);
    base["count"] = json!(offers.len());
    base
}

/// One Google Maps place from a search, compact: what the model needs to pick and to tell the user, with the
/// `place_id` for `place_details`.
fn compact_place(p: &Value) -> Value {
    let types: Vec<String> = arr(p, "types")
        .iter()
        .filter_map(Value::as_str)
        .take(3)
        .map(str::to_string)
        .collect();
    let mut out = json!({
        "place_id": get_str(p, "place_id"),
        "name": get_str(p, "title"),
        "type": if get_str(p, "type").is_empty() { types.first().cloned().unwrap_or_default() } else { get_str(p, "type") },
        "types": types,
        "rating": num(p.get("rating")),
        "reviews": num(p.get("reviews")),
        "price": get_str(p, "price"),
        "address": get_str(p, "address"),
        "open_state": get_str(p, "open_state"),
        "phone": get_str(p, "phone"),
        "website": get_str(p, "website"),
    });
    if let Some(h) = p.get("hours").filter(|h| h.is_string()) {
        out["hours"] = h.clone();
    }
    if let Some(d) = p
        .get("description")
        .and_then(Value::as_str)
        .filter(|d| !d.is_empty())
    {
        out["description"] = json!(d);
    }
    if let Some(o) = p.get("order_online").filter(|o| truthy(o)) {
        out["order_online"] = o.clone();
    }
    if let Some(so) = p.get("service_options").and_then(Value::as_object) {
        let on: Vec<&String> = so
            .iter()
            .filter(|(_, v)| truthy(v))
            .map(|(k, _)| k)
            .collect();
        if !on.is_empty() {
            out["service_options"] = json!(on);
        }
    }
    out
}

/// The Google Maps search reply, compact (at most 15 places).
pub fn compact_places(data: &Value) -> Value {
    let places: Vec<Value> = arr(data, "local_results")
        .iter()
        .take(15)
        .map(compact_place)
        .collect();
    let mut out = json!({"count": places.len(), "places": places, "data": "Google Maps",
                         "book": "on the restaurant's own site or the booking platform the user names (the website field, when present)"});
    if let Some(e) = data.get("error").and_then(Value::as_str) {
        out["note"] = json!(e);
    }
    out
}

/// One place's details (`place_results`), compact: hours per day as lines, contact, the links Google lists.
pub fn compact_place_details(data: &Value) -> Value {
    let p = data
        .get("place_results")
        .filter(|v| v.is_object())
        .unwrap_or(data);
    let mut out = compact_place(p);
    if let Some(t) = p.get("type").and_then(Value::as_array) {
        let names: Vec<String> = t
            .iter()
            .filter_map(Value::as_str)
            .map(str::to_string)
            .collect();
        out["type"] = json!(names.first().cloned().unwrap_or_default());
        out["types"] = json!(names.into_iter().take(3).collect::<Vec<_>>());
    }
    let hours: Vec<String> = arr(p, "hours")
        .iter()
        .filter_map(|d| d.as_object())
        .flat_map(|m| {
            m.iter()
                .map(|(k, v)| format!("{k}: {}", v.as_str().unwrap_or_default()))
        })
        .collect();
    if !hours.is_empty() {
        out["hours"] = json!(hours);
    }
    for k in [
        "menu",
        "reservations",
        "reserve_a_table",
        "order_online",
        "plus_code",
        "description",
    ] {
        if let Some(v) = p.get(k).filter(|v| truthy(v)) {
            out[k] = v.clone();
        }
    }
    if let Some(rs) = p.get("rating_summary").and_then(Value::as_array) {
        out["rating_summary"] = json!(rs
            .iter()
            .map(|x| format!("{}★ {}", get_str(x, "stars"), get_str(x, "amount")))
            .collect::<Vec<_>>());
    }
    let reviews: Vec<String> = arr(p, "user_reviews")
        .iter()
        .chain(arr(&p["user_reviews"], "most_relevant").iter())
        .filter_map(|r| {
            r.get("description")
                .or_else(|| r.get("snippet"))
                .and_then(Value::as_str)
        })
        .take(3)
        .map(|t| t.chars().take(200).collect())
        .collect();
    if !reviews.is_empty() {
        out["review_snippets"] = json!(reviews);
    }
    out
}

impl SerpApi {
    pub fn new(client: reqwest::Client, base: &str, key: &str) -> Self {
        Self {
            client,
            base: base.trim_end_matches('/').to_string(),
            key: key.to_string(),
            timeout: Duration::from_secs(60),
        }
    }

    async fn get(&self, path: &str, params: &[(&str, String)]) -> Result<Value, SerpError> {
        let mut q: Vec<(&str, String)> = vec![("api_key", self.key.clone())];
        q.extend(params.iter().filter(|(_, v)| !v.is_empty()).cloned());
        let r = self
            .client
            .get(format!("{}/{}", self.base, path))
            .query(&q)
            .timeout(self.timeout)
            .send()
            .await
            .map_err(|e| SerpError(format!("SerpApi: {e}")))?;
        let status = r.status().as_u16();
        let text = r
            .text()
            .await
            .map_err(|e| SerpError(format!("SerpApi: {e}")))?;
        let body: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
        if status >= 400 {
            let msg = body
                .get("error")
                .and_then(Value::as_str)
                .map(str::to_string)
                .unwrap_or_else(|| text.chars().take(200).collect());
            return Err(SerpError(format!("SerpApi HTTP {status}: {msg}")));
        }
        if !body.is_object() {
            return Err(SerpError("SerpApi: unexpected reply".into()));
        }
        Ok(body)
    }

    /// The key's plan and remaining searches (`/account.json`).
    pub async fn account(&self) -> Result<Value, SerpError> {
        let a = self.get("account.json", &[]).await?;
        if let Some(e) = a.get("error").and_then(Value::as_str) {
            return Err(SerpError(e.to_string()));
        }
        Ok(json!({
            "plan": get_str(&a, "plan_name"),
            "searches_per_month": num(a.get("searches_per_month")),
            "this_month_usage": num(a.get("this_month_usage")),
            "searches_left": num(a.get("total_searches_left")),
        }))
    }

    /// `flights_search`: Google Flights for a route and date (a return date makes it a round trip).
    pub async fn flights(&self, a: &Value) -> Result<Value, SerpError> {
        let (days, cut) = date_range(a, 7).map_err(SerpError)?;
        if days.len() == 1 && !cut {
            return self.flights_day(a).await;
        }
        let runs = days.iter().map(|(d, r)| {
            let mut x = a.clone();
            x["date"] = json!(d);
            x["return_date"] = json!(r);
            async move { (d.clone(), self.flights_day(&x).await) }
        });
        let mut out = Vec::new();
        for (d, r) in futures_util::future::join_all(runs).await {
            out.push((d, r.unwrap_or_else(|e| json!({"offers": [], "note": e.0}))));
        }
        Ok(merge_days(out, cut, &get_str(a, "date_to")))
    }

    /// One departure date (the range is handled by [`SerpApi::flights`]).
    async fn flights_day(&self, a: &Value) -> Result<Value, SerpError> {
        let origin = get_str(a, "origin");
        let dest = get_str(a, "destination");
        let from = airport_codes(&origin)
            .ok_or_else(|| SerpError(format!("origin: give an IATA airport code for {origin:?} (e.g. SIN, or HND,NRT for several)")))?;
        let to = airport_codes(&dest)
            .ok_or_else(|| SerpError(format!("destination: give an IATA airport code for {dest:?} (e.g. SIN, or HND,NRT for several)")))?;
        let date = get_str(a, "date");
        if date.len() != 10 {
            return Err(SerpError("date must be YYYY-MM-DD".into()));
        }
        let ret = get_str(a, "return_date");
        let currency = {
            let c = get_str(a, "currency").to_uppercase();
            if c.is_empty() {
                "USD".to_string()
            } else {
                c
            }
        };
        let cabin = match get_str(a, "cabin").to_lowercase().as_str() {
            "premium_economy" | "premium economy" => "2",
            "business" => "3",
            "first" => "4",
            _ => "1",
        };
        let sort = match get_str(a, "sort").to_lowercase().as_str() {
            "price" => "2",
            "departure" => "3",
            "arrival" => "4",
            "duration" => "5",
            _ => "1",
        };
        let adults = people(a, "adults", 1, 9);
        let mut params = vec![
            ("engine", "google_flights".to_string()),
            ("departure_id", from),
            ("arrival_id", to),
            ("outbound_date", date),
            (
                "type",
                if ret.is_empty() {
                    "2".into()
                } else {
                    "1".into()
                },
            ),
            ("return_date", ret),
            ("adults", adults.to_string()),
            ("travel_class", cabin.to_string()),
            ("currency", currency.clone()),
            ("hl", "en".to_string()),
            ("sort_by", sort.to_string()),
        ];
        if let Some(st) = stops_param(a) {
            params.push(("stops", st.to_string()));
        }
        if let Some(m) = price_cap(a, "max_price") {
            params.push(("max_price", m.to_string()));
        }
        // preferred airlines given as 2-letter codes or alliances narrow Google's search; names only flag the offers
        let airlines = pref_list(a, "airlines");
        let google: Vec<String> = airlines
            .iter()
            .filter_map(|x| {
                if x.len() == 2 && x.chars().all(|c| c.is_ascii_alphanumeric()) {
                    Some(x.to_ascii_uppercase())
                } else {
                    crate::loyalty::alliance(x).map(|al| al.1.to_string())
                }
            })
            .collect();
        if !airlines.is_empty() && google.len() == airlines.len() {
            params.push(("include_airlines", google.join(",")));
        }
        let data = self.get("search.json", &params).await?;
        let mut out = compact_flights(&data, &currency);
        let found = mark_preferred(&mut out["offers"], &airlines, "airlines");
        tag_cabins(&mut out["offers"], a);
        out["searched"] = searched(a, &params[1].1, &params[2].1);
        if !airlines.is_empty() {
            out["searched"]["airlines"] = json!(airlines);
            out["searched"]["preferred_found"] = json!(found);
        }
        // a booking-options request must repeat the route and dates: keep them with each offer's token
        let route = json!({"departure_id": params[1].1, "arrival_id": params[2].1, "outbound_date": params[3].1,
                           "type": params[4].1, "return_date": params[5].1});
        for o in out["offers"].as_array().into_iter().flatten() {
            let id = get_str(o, "offer_id");
            if let Some(tok) = token_of(&id) {
                if !tok.trim_start().starts_with('{') {
                    let mut t = TOKENS.lock().unwrap_or_else(|e| e.into_inner());
                    t.insert(id, json!({"token": tok, "route": route}).to_string());
                }
            }
        }
        Ok(out)
    }

    /// `flight_offer`: the booking options of an itinerary from an earlier `flights_search` (its offer id).
    pub async fn flight_offer(&self, a: &Value) -> Result<Value, SerpError> {
        let id = get_str(a, "offer_id");
        let raw = if id.starts_with("fl_") {
            token_of(&id).ok_or_else(|| SerpError(format!("unknown offer id {id}: search again with flights_search and use an offer_id from that result")))?
        } else if id.len() > 40 {
            id.clone()
        } else {
            return Err(SerpError(
                "offer_id: use an offer_id from flights_search".into(),
            ));
        };
        // the token, with the route and dates of the search it came from (SerpApi wants them repeated)
        let (token, route) = match serde_json::from_str::<Value>(&raw) {
            Ok(v) if v.is_object() => (get_str(&v, "token"), v["route"].clone()),
            _ => (raw, Value::Null),
        };
        let currency = {
            let c = get_str(a, "currency").to_uppercase();
            if c.is_empty() {
                "USD".to_string()
            } else {
                c
            }
        };
        let mut params = vec![
            ("engine", "google_flights".to_string()),
            ("booking_token", token),
            ("currency", currency.clone()),
            ("hl", "en".to_string()),
        ];
        for k in [
            "departure_id",
            "arrival_id",
            "outbound_date",
            "type",
            "return_date",
        ] {
            let v = get_str(&route, k);
            if !v.is_empty() {
                params.push((k, v));
            }
        }
        let data = self.get("search.json", &params).await?;
        Ok(compact_booking(&data, &currency))
    }

    /// `hotels_search`: Google Hotels for a place and dates.
    pub async fn hotels(&self, a: &Value) -> Result<Value, SerpError> {
        let city = get_str(a, "city");
        if city.trim().is_empty() {
            return Err(SerpError("city is required".into()));
        }
        let (check_in, check_out) = (get_str(a, "check_in"), get_str(a, "check_out"));
        if check_in.len() != 10 || check_out.len() != 10 {
            return Err(SerpError(
                "check_in and check_out must be YYYY-MM-DD".into(),
            ));
        }
        let area = get_str(a, "area");
        let brands = pref_list(a, "brands");
        // one or two preferred brands or groups go into Google's query ("Hilton hotels near Ginza Tokyo", "IHG hotels
        // near …" — a programme's name is cut to its first word); more only flag the results
        let word = |b: &String| match crate::loyalty::hotel_group(b) {
            Some(g) => crate::loyalty::group_word(g.0).to_string(),
            None => b.clone(),
        };
        let lead = match brands.len() {
            1 => format!("{} ", word(&brands[0])),
            2 => format!("{} or {} ", word(&brands[0]), word(&brands[1])),
            _ => String::new(),
        };
        let q = if area.trim().is_empty() {
            format!("{lead}{} hotels", city.trim())
        } else {
            format!("{lead}hotels near {} {}", area.trim(), city.trim())
        };
        let currency = {
            let c = get_str(a, "currency").to_uppercase();
            if c.is_empty() {
                "USD".to_string()
            } else {
                c
            }
        };
        let guests = people(a, "guests", 2, 20);
        let mut params = vec![
            ("engine", "google_hotels".to_string()),
            ("q", q),
            ("check_in_date", check_in),
            ("check_out_date", check_out),
            ("adults", guests.to_string()),
            ("currency", currency.clone()),
            ("hl", "en".to_string()),
        ];
        if let Some(s) = a
            .get("min_stars")
            .and_then(Value::as_i64)
            .filter(|s| (2..=5).contains(s))
        {
            params.push((
                "hotel_class",
                (s..=5).map(|x| x.to_string()).collect::<Vec<_>>().join(","),
            ));
        }
        if let Some(m) = price_cap(a, "max_price_per_night") {
            params.push(("max_price", m.to_string()));
        }
        if truthy(&a["free_cancellation"]) {
            params.push(("free_cancellation", "true".to_string()));
        }
        if let Some(r) = a.get("min_rating").and_then(Value::as_f64) {
            let code = if r >= 4.5 {
                "9"
            } else if r >= 4.0 {
                "8"
            } else if r >= 3.5 {
                "7"
            } else {
                ""
            };
            // Google's rating filter starts at 3.5: a lower minimum is no filter at all, not an empty parameter
            if !code.is_empty() {
                params.push(("rating", code.to_string()));
            }
        }
        match get_str(a, "sort").to_lowercase().as_str() {
            "price" => params.push(("sort_by", "3".to_string())),
            "rating" => params.push(("sort_by", "8".to_string())),
            "reviews" => params.push(("sort_by", "13".to_string())),
            _ => {}
        }
        let data = self.get("search.json", &params).await?;
        let mut out = compact_hotels(&data, &currency);
        let found = mark_preferred(&mut out["hotels"], &brands, "name");
        out["searched"] = json!({
            "city": city.trim(), "area": area.trim(), "check_in": params[2].1, "check_out": params[3].1, "guests": guests,
            "brands": brands, "preferred_found": found,
            "max_price_per_night": price_cap(a, "max_price_per_night").map_or_else(|| json!("no limit (every price)"), |m| json!(m)),
            "rule": if brands.is_empty() { "answer for the city, area, dates and guests searched" } else { "this search used the user's preferred brands (hotels marked preferred are them) and is the only one — do not search again without them; show the best of them next to the best value, with the price difference; when no preferred hotel is good (preferred_found 0, or none fits the request), answer with what was found and ask whether to search all hotels" },
        });
        Ok(out)
    }

    /// `places_search`: restaurants, cafes, shops and other places on Google Maps for a query in a city or area.
    pub async fn places(&self, a: &Value) -> Result<Value, SerpError> {
        let query = get_str(a, "query");
        let city = get_str(a, "city");
        if query.trim().is_empty() && city.trim().is_empty() {
            return Err(SerpError(
                "query (what — ramen, sushi, cafe, pharmacy …) and city are required".into(),
            ));
        }
        let area = get_str(a, "area");
        let q = match (area.trim().is_empty(), city.trim().is_empty()) {
            (false, false) => format!("{} near {}, {}", query.trim(), area.trim(), city.trim()),
            (true, false) => format!("{} in {}", query.trim(), city.trim()),
            _ => query.trim().to_string(),
        };
        let mut params = vec![
            ("engine", "google_maps".to_string()),
            ("type", "search".to_string()),
            ("q", q),
            ("hl", "en".to_string()),
        ];
        if let Some(r) = a.get("min_rating").and_then(Value::as_f64) {
            params.push(("min_rating", format!("{:.1}", r.clamp(2.0, 4.5))));
        }
        if truthy(&a["open_now"]) {
            params.push(("open_state", "now".to_string()));
        }
        if let Some(p) = a
            .get("max_price_level")
            .and_then(Value::as_i64)
            .filter(|p| (1..=4).contains(p))
        {
            params.push(("max_price", p.to_string()));
        }
        if let Some(p) = a
            .get("min_price_level")
            .and_then(Value::as_i64)
            .filter(|p| (1..=4).contains(p))
        {
            params.push(("min_price", p.to_string()));
        }
        let data = self.get("search.json", &params).await?;
        Ok(compact_places(&data))
    }

    /// `place_details`: one place by its Google place id — hours per day, contact, the links Google lists.
    pub async fn place(&self, a: &Value) -> Result<Value, SerpError> {
        let id = get_str(a, "place_id");
        if id.trim().is_empty() {
            return Err(SerpError(
                "place_id: use a place_id from places_search".into(),
            ));
        }
        let data = self
            .get(
                "search.json",
                &[
                    ("engine", "google_maps".to_string()),
                    ("place_id", id.trim().to_string()),
                    ("hl", "en".to_string()),
                ],
            )
            .await?;
        Ok(compact_place_details(&data))
    }

    /// `hotel_rooms`: one property's details and rates per seller (its hotel id from `hotels_search`).
    pub async fn hotel_rooms(&self, a: &Value) -> Result<Value, SerpError> {
        let id = get_str(a, "hotel_id");
        let token = if id.starts_with("ht_") {
            token_of(&id).ok_or_else(|| SerpError(format!("unknown hotel id {id}: search again with hotels_search and use a hotel_id from that result")))?
        } else if id.len() > 40 {
            id.clone()
        } else {
            return Err(SerpError(
                "hotel_id: use a hotel_id from hotels_search".into(),
            ));
        };
        let (check_in, check_out) = (get_str(a, "check_in"), get_str(a, "check_out"));
        if check_in.len() != 10 || check_out.len() != 10 {
            return Err(SerpError(
                "check_in and check_out must be YYYY-MM-DD".into(),
            ));
        }
        let currency = {
            let c = get_str(a, "currency").to_uppercase();
            if c.is_empty() {
                "USD".to_string()
            } else {
                c
            }
        };
        let guests = people(a, "guests", 2, 20);
        let data = self
            .get(
                "search.json",
                &[
                    ("engine", "google_hotels".to_string()),
                    ("property_token", token),
                    ("check_in_date", check_in),
                    ("check_out_date", check_out),
                    ("adults", guests.to_string()),
                    ("currency", currency.clone()),
                    ("hl", "en".to_string()),
                ],
            )
            .await?;
        Ok(compact_rooms(&data, &currency))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preferred_airlines_and_brands_are_flagged_not_filtered() {
        assert_eq!(
            pref_list(&json!({"airlines": ["SQ", " ANA ", ""]}), "airlines"),
            ["SQ", "ANA"]
        );
        assert_eq!(
            pref_list(&json!({"airlines": "Singapore Airlines, NH"}), "airlines"),
            ["Singapore Airlines", "NH"]
        );
        assert!(pref_list(&json!({}), "airlines").is_empty());
        let mut offers = json!([
            {"airlines": "Singapore Airlines", "legs": [{"flight_number": "SQ 638"}], "price": 620},
            {"airlines": "Cathay Pacific", "legs": [{"flight_number": "CX 710"}], "price": 510},
            {"airlines": "ANA", "legs": [{"flight_number": "NH 842"}], "price": 700}]);
        assert_eq!(
            mark_preferred(
                &mut offers,
                &["Singapore Airlines".into(), "NH".into()],
                "airlines"
            ),
            2
        );
        assert_eq!(offers[0]["preferred"], true);
        assert!(offers[1].get("preferred").is_none());
        assert_eq!(offers[2]["preferred"], true);
        assert_eq!(offers.as_array().unwrap().len(), 3, "nothing is dropped");
        let mut hotels = json!([{"name": "Hilton Tokyo"}, {"name": "Hotel Ginza Grand"}]);
        assert_eq!(mark_preferred(&mut hotels, &["hilton".into()], "name"), 1);
        assert_eq!(hotels[0]["preferred"], true);
        assert_eq!(mark_preferred(&mut hotels, &[], "name"), 0);
        // an alliance / a group stands for its members
        let mut offers = json!([{"airlines": "United", "legs": [{"flight_number": "UA 838"}]}, {"airlines": "Delta", "legs": [{"flight_number": "DL 1"}]}]);
        assert_eq!(
            mark_preferred(&mut offers, &["Star Alliance".into()], "airlines"),
            1
        );
        assert_eq!(offers[0]["preferred"], true);
        let mut hotels = json!([{"name": "Holiday Inn Express Tokyo"}, {"name": "Hilton Tokyo"}]);
        assert_eq!(mark_preferred(&mut hotels, &["IHG".into()], "name"), 1);
        assert_eq!(hotels[0]["preferred"], true);
    }

    #[test]
    fn mixed_cabin_itineraries_count_as_the_premium_cabin() {
        let mut offers = json!([
            {"legs": [{"flight_number": "NH 1", "from": "KIX", "to": "HND", "cabin": "Economy"}, {"flight_number": "NH 841", "from": "HND", "to": "SIN", "cabin": "Business"}]},
            {"legs": [{"flight_number": "SQ 637", "from": "HND", "to": "SIN", "cabin": "Business"}]},
            {"legs": [{"flight_number": "TR 801", "from": "NRT", "to": "SIN", "cabin": "Economy"}]}]);
        tag_cabins(&mut offers, &json!({"cabin": "business"}));
        assert_eq!(offers[0]["cabin"], "business");
        assert_eq!(offers[0]["cabin_mix"], "business + economy");
        assert_eq!(offers[0]["economy_legs"], json!(["NH 1 KIX-HND"]));
        assert_eq!(offers[1]["cabin"], "business");
        assert!(offers[1].get("cabin_mix").is_none());
        assert_eq!(offers[2]["cabin"], "economy (not the cabin searched)");
        // economy searches are not labelled
        let mut e = json!([{"legs": [{"cabin": "Economy"}]}]);
        tag_cabins(&mut e, &json!({"cabin": "economy"}));
        assert!(e[0].get("cabin").is_none());
        assert!(searched(&json!({"cabin": "business"}), "A", "B")["rule"]
            .as_str()
            .unwrap()
            .contains("counts as a business itinerary"));
        assert!(!searched(&json!({}), "A", "B")["rule"]
            .as_str()
            .unwrap()
            .contains("counts as"));
    }

    #[test]
    fn a_date_range_is_one_call_with_a_moving_return() {
        let (d, cut) = date_range(&json!({"date": "2026-12-01"}), 7).unwrap();
        assert_eq!(d, [("2026-12-01".to_string(), String::new())]);
        assert!(!cut);
        let (d, cut) = date_range(
            &json!({"date": "2026-12-30", "date_to": "2027-01-02", "return_date": "2027-01-06"}),
            7,
        )
        .unwrap();
        assert_eq!(
            d.iter().map(|x| x.0.as_str()).collect::<Vec<_>>(),
            ["2026-12-30", "2026-12-31", "2027-01-01", "2027-01-02"]
        );
        assert_eq!(d[3].1, "2027-01-09", "the stay keeps its length");
        assert!(!cut);
        let (d, cut) =
            date_range(&json!({"date": "2026-12-01", "date_to": "2026-12-31"}), 7).unwrap();
        assert!(d.len() == 7 && cut);
        assert!(date_range(&json!({"date": "2026-12-05", "date_to": "2026-12-01"}), 7).is_err());
        let m = merge_days(
            vec![
                (
                    "2026-12-01".into(),
                    json!({"offers": [{"offer_id": "a", "price": 500, "airlines": "X"}, {"offer_id": "b", "price": 300, "airlines": "Y"}], "searched": {"cabin": "business"}}),
                ),
                (
                    "2026-12-02".into(),
                    json!({"offers": [{"offer_id": "c", "price": 200, "airlines": "Z"}]}),
                ),
                (
                    "2026-12-03".into(),
                    json!({"offers": [], "note": "no flights"}),
                ),
            ],
            false,
            "2026-12-03",
        );
        assert_eq!(m["offers"][0]["offer_id"], "c");
        assert_eq!(m["offers"][0]["date"], "2026-12-02");
        assert_eq!(m["count"], 3);
        assert_eq!(m["by_date"][0]["cheapest"], 300.0);
        assert_eq!(m["by_date"][2]["count"], 0);
        assert_eq!(m["searched"]["dates"], "2026-12-01 to 2026-12-03 (3 days)");
        assert_eq!(m["searched"]["cabin"], "business");
    }

    #[test]
    fn stop_limit_and_the_searched_echo() {
        // the schema's `stops`: "any" (a filled-in default) means no limit
        assert_eq!(
            max_stops(&json!({"stops": "any", "max_stops": 0, "nonstop_only": false})),
            None
        );
        assert_eq!(max_stops(&json!({"stops": "nonstop"})), Some(0));
        assert_eq!(max_stops(&json!({"stops": "one_stop_max"})), Some(1));
        assert_eq!(max_stops(&json!({"stops": "two_stops_max"})), Some(2));
        assert_eq!(price_cap(&json!({"max_price": 0}), "max_price"), None);
        assert_eq!(price_cap(&json!({"max_price": -5}), "max_price"), None);
        assert_eq!(people(&json!({}), "adults", 1, 9), 1);
        assert_eq!(people(&json!({}), "guests", 2, 20), 2);
        for bad in [json!(0), json!(-3), json!("x"), json!(0.5), json!(true)] {
            assert_eq!(people(&json!({"adults": bad}), "adults", 1, 9), 1);
        }
        assert_eq!(people(&json!({"adults": "3"}), "adults", 1, 9), 3);
        assert_eq!(people(&json!({"adults": 2.9}), "adults", 1, 9), 2);
        assert_eq!(people(&json!({"adults": 40}), "adults", 1, 9), 9);
        assert_eq!(
            price_cap(&json!({"max_price": 900.5}), "max_price"),
            Some(900)
        );
        assert_eq!(stops_param(&json!({})), None);
        assert_eq!(stops_param(&json!({"nonstop_only": true})), Some(1));
        assert_eq!(stops_param(&json!({"max_stops": 0})), Some(1));
        assert_eq!(stops_param(&json!({"max_stops": 1})), Some(2));
        assert_eq!(stops_param(&json!({"max_stops": 7})), Some(3));
        let s = searched(
            &json!({"cabin": "business", "max_stops": 0, "date": "2026-12-01", "adults": 2}),
            "HND,NRT",
            "SIN",
        );
        assert_eq!(s["cabin"], "business");
        assert_eq!(s["stops"], "nonstop only");
        assert_eq!(s["trip"], "one way");
        assert_eq!(s["adults"], 2);
        assert!(s["rule"]
            .as_str()
            .unwrap()
            .contains("business fare with nonstop only"));
        assert!(s["rule"]
            .as_str()
            .unwrap()
            .contains("name the cabin (business)"));
        let s = searched(&json!({"return_date": "2026-12-09"}), "SIN", "TYO");
        assert_eq!(s["cabin"], "economy");
        assert_eq!(s["stops"], "any number of stops");
        assert_eq!(s["trip"], "round trip, return 2026-12-09");
    }

    #[test]
    fn flights_compact_and_offer_ids_resolve() {
        let data = json!({
            "search_metadata": {"google_flights_url": "https://www.google.com/travel/flights?x"},
            "best_flights": [{
                "flights": [{"departure_airport": {"id": "SIN", "time": "2026-11-20 09:25"}, "arrival_airport": {"id": "NRT", "time": "2026-11-20 17:15"},
                             "airline": "Singapore Airlines", "flight_number": "SQ 638", "duration": 410, "airplane": "Boeing 787", "travel_class": "Economy"}],
                "layovers": [], "total_duration": 410, "price": 620, "type": "One way", "booking_token": "WlRCdw…longtoken…abcdefghijklmnopqrstuvwxyz0123456789",
                "extensions": ["Checked baggage for a fee"]}],
            "other_flights": [{
                "flights": [{"departure_airport": {"id": "SIN", "time": "2026-11-20 23:55"}, "arrival_airport": {"id": "HKG", "time": "2026-11-21 03:40"}, "airline": "Cathay Pacific", "flight_number": "CX 710", "duration": 225},
                            {"departure_airport": {"id": "HKG", "time": "2026-11-21 08:00"}, "arrival_airport": {"id": "NRT", "time": "2026-11-21 13:30"}, "airline": "Cathay Pacific", "flight_number": "CX 500", "duration": 270}],
                "layovers": [{"id": "HKG", "duration": 260, "name": "Hong Kong International Airport"}], "total_duration": 755, "price": 510, "type": "One way", "booking_token": "token-two-"}],
            "price_insights": {"lowest_price": 510, "price_level": "typical", "typical_price_range": [480, 760]}
        });
        let out = compact_flights(&data, "SGD");
        assert_eq!(out["count"], 2);
        let first = &out["offers"][0];
        assert_eq!(first["airlines"], "Singapore Airlines");
        assert_eq!(first["stops"], 0);
        assert_eq!(first["price"], 620);
        assert_eq!(first["currency"], "SGD");
        assert_eq!(first["rank"], 1);
        assert_eq!(
            first["route"],
            "Singapore Airlines SQ 638 SIN 2026-11-20 09:25 → NRT 2026-11-20 17:15"
        );
        assert_eq!(first["notes"][0], "Checked baggage for a fee");
        let id = first["offer_id"].as_str().unwrap().to_string();
        assert!(id.starts_with("fl_"));
        assert!(token_of(&id).unwrap().starts_with("WlRCdw"));
        let second = &out["offers"][1];
        assert_eq!(second["stops"], 1);
        assert_eq!(second["layovers"][0], "HKG 260 min");
        assert_eq!(second["legs"].as_array().unwrap().len(), 2);
        assert_eq!(out["price_insight"]["lowest_price"], 510);
        assert_eq!(out["google_url"], "https://www.google.com/travel/flights?x");
    }

    #[test]
    fn places_compact_with_hours_and_links() {
        let data = json!({"local_results": [{"position": 1, "title": "Kyushu Jangara Ginza", "place_id": "ChIJ7", "rating": 4.4, "reviews": 2349, "price": "¥1,000–2,000",
            "type": "Ramen restaurant", "types": ["Ramen restaurant", "Restaurant"], "address": "6 Chome-12-17 Ginza, Tokyo", "open_state": "Closed · Opens 11 AM",
            "hours": "Closed ⋅ Opens 11 AM", "phone": "+81 3-3572-3025", "website": "https://kyushujangara.co.jp/shops/ginza/", "service_options": {"dine_in": true, "delivery": false}}]});
        let out = compact_places(&data);
        assert_eq!(out["count"], 1);
        let p = &out["places"][0];
        assert_eq!(p["name"], "Kyushu Jangara Ginza");
        assert_eq!(p["place_id"], "ChIJ7");
        assert_eq!(p["type"], "Ramen restaurant");
        assert_eq!(p["rating"], 4.4);
        assert_eq!(p["website"], "https://kyushujangara.co.jp/shops/ginza/");
        assert_eq!(p["service_options"], json!(["dine_in"]));
        let d = compact_place_details(
            &json!({"place_results": {"title": "Kyushu Jangara Ginza", "place_id": "ChIJ7", "type": ["Ramen restaurant", "Restaurant"], "rating": 4.4,
            "reviews": 2349, "address": "Ginza", "phone": "+81", "website": "https://k.example/", "open_state": "Open",
            "hours": [{"wednesday": "11 AM–10:30 PM"}, {"thursday": "11 AM–10:30 PM"}], "rating_summary": [{"stars": 5, "amount": 1500}],
            "user_reviews": {"most_relevant": [{"description": "Great tonkotsu, long queue at noon."}]}}}),
        );
        assert_eq!(d["type"], "Ramen restaurant");
        assert_eq!(
            d["hours"],
            json!(["wednesday: 11 AM–10:30 PM", "thursday: 11 AM–10:30 PM"])
        );
        assert_eq!(d["rating_summary"][0], "5★ 1500");
        assert_eq!(
            d["review_snippets"][0],
            "Great tonkotsu, long queue at noon."
        );
    }

    #[test]
    fn booking_options_and_hotels_compact() {
        let b = json!({"booking_options": [{"together": {"book_with": "Singapore Airlines", "marketed_as": ["SQ 638"], "price": 620,
            "booking_request": {"url": "https://www.google.com/travel/clk/f", "post_data": "u=abc"}, "baggage_prices": ["1 free checked bag"], "option_title": "Economy"}}],
            "selected_flights": []});
        let o = compact_booking(&b, "SGD");
        assert_eq!(o["count"], 1);
        assert_eq!(o["options"][0]["book_with"], "Singapore Airlines");
        assert!(o["options"][0]["link_method"]
            .as_str()
            .unwrap()
            .starts_with("POST"));
        assert_eq!(o["options"][0]["baggage"][0], "1 free checked bag");
        let h = json!({"properties": [{"type": "hotel", "name": "Hotel Ginza Grand", "property_token": "ChkI-very-long-property-token-0123456789abcdef", "extracted_hotel_class": 4,
            "overall_rating": 4.4, "reviews": 2100, "rate_per_night": {"lowest": "S$210", "extracted_lowest": 210}, "total_rate": {"lowest": "S$630", "extracted_lowest": 630},
            "prices": [{"source": "Booking.com", "free_cancellation": true}], "nearby_places": [{"name": "Ginza Station", "transportations": [{"type": "Walking", "duration": "4 min"}]}],
            "amenities": ["Free Wi-Fi", "Breakfast"], "check_in_time": "3:00 PM", "check_out_time": "11:00 AM"}],
            "search_metadata": {"google_hotels_url": "https://www.google.com/travel/search?x"}});
        let out = compact_hotels(&h, "SGD");
        assert_eq!(out["count"], 1);
        let p = &out["hotels"][0];
        assert_eq!(p["name"], "Hotel Ginza Grand");
        assert_eq!(p["stars"], 4);
        assert_eq!(p["price_per_night"], 210);
        assert_eq!(p["total"], 630);
        assert_eq!(p["free_cancellation"], true);
        assert_eq!(p["near"][0], "Ginza Station (4 min Walking)");
        let hid = p["hotel_id"].as_str().unwrap().to_string();
        assert!(hid.starts_with("ht_") && token_of(&hid).unwrap().starts_with("ChkI"));
        let rooms = compact_rooms(
            &json!({"name": "Hotel Ginza Grand", "address": "1-1 Ginza", "featured_prices": [{"source": "Hotel site", "rate_per_night": {"extracted_lowest": 199},
            "total_rate": {"extracted_lowest": 597}, "free_cancellation": true, "link": "https://hotel.example/book",
            "rooms": [{"name": "Deluxe Twin", "rate_per_night": {"extracted_lowest": 199}}]}]}),
            "SGD",
        );
        assert_eq!(rooms["count"], 1);
        assert_eq!(rooms["offers"][0]["rooms"][0]["name"], "Deluxe Twin");
        assert_eq!(rooms["address"], "1-1 Ginza");
    }
}
