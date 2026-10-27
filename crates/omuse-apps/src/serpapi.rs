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

/// The airports Google Flights should take for a place: an IATA code (or a comma list) as given; a city name that
/// has several airports becomes all of them; `None` for a name this table does not know.
pub fn airport_codes(place: &str) -> Option<String> {
    let p = place.trim();
    if p.is_empty() {
        return None;
    }
    let codes: Vec<&str> = p.split(',').map(str::trim).collect();
    if codes
        .iter()
        .all(|c| c.len() == 3 && c.chars().all(|ch| ch.is_ascii_alphabetic()))
    {
        return Some(
            codes
                .iter()
                .map(|c| c.to_ascii_uppercase())
                .collect::<Vec<_>>()
                .join(","),
        );
    }
    let key = p
        .to_lowercase()
        .replace(['-', '_'], " ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    let table: &[(&str, &str)] = &[
        ("tokyo", "HND,NRT"),
        ("osaka", "KIX,ITM"),
        ("seoul", "ICN,GMP"),
        ("singapore", "SIN"),
        ("hong kong", "HKG"),
        ("bangkok", "BKK,DMK"),
        ("taipei", "TPE,TSA"),
        ("shanghai", "PVG,SHA"),
        ("beijing", "PEK,PKX"),
        ("shenzhen", "SZX"),
        ("guangzhou", "CAN"),
        ("chengdu", "TFU,CTU"),
        ("kuala lumpur", "KUL"),
        ("ho chi minh city", "SGN"),
        ("saigon", "SGN"),
        ("hanoi", "HAN"),
        ("manila", "MNL"),
        ("jakarta", "CGK"),
        ("bali", "DPS"),
        ("denpasar", "DPS"),
        ("phuket", "HKT"),
        ("sydney", "SYD"),
        ("melbourne", "MEL"),
        ("auckland", "AKL"),
        ("delhi", "DEL"),
        ("new delhi", "DEL"),
        ("mumbai", "BOM"),
        ("dubai", "DXB"),
        ("doha", "DOH"),
        ("london", "LHR,LGW,STN,LCY"),
        ("paris", "CDG,ORY"),
        ("amsterdam", "AMS"),
        ("frankfurt", "FRA"),
        ("munich", "MUC"),
        ("zurich", "ZRH"),
        ("rome", "FCO"),
        ("milan", "MXP,LIN"),
        ("madrid", "MAD"),
        ("barcelona", "BCN"),
        ("new york", "JFK,EWR,LGA"),
        ("nyc", "JFK,EWR,LGA"),
        ("san francisco", "SFO"),
        ("los angeles", "LAX"),
        ("seattle", "SEA"),
        ("chicago", "ORD,MDW"),
        ("boston", "BOS"),
        ("washington", "IAD,DCA"),
        ("toronto", "YYZ"),
        ("vancouver", "YVR"),
    ];
    table
        .iter()
        .find(|(name, _)| *name == key)
        .map(|(_, codes)| (*codes).to_string())
}

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
        let adults = a
            .get("adults")
            .and_then(Value::as_i64)
            .filter(|n| *n >= 1)
            .unwrap_or(1);
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
        if truthy(&a["nonstop_only"]) {
            params.push(("stops", "1".to_string()));
        }
        if let Some(m) = a.get("max_price").and_then(Value::as_f64) {
            params.push(("max_price", format!("{}", m as i64)));
        }
        let data = self.get("search.json", &params).await?;
        let out = compact_flights(&data, &currency);
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
        let q = if area.trim().is_empty() {
            format!("{} hotels", city.trim())
        } else {
            format!("hotels near {} {}", area.trim(), city.trim())
        };
        let currency = {
            let c = get_str(a, "currency").to_uppercase();
            if c.is_empty() {
                "USD".to_string()
            } else {
                c
            }
        };
        let guests = a
            .get("guests")
            .and_then(Value::as_i64)
            .filter(|n| *n >= 1)
            .unwrap_or(2);
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
        if let Some(m) = a.get("max_price_per_night").and_then(Value::as_f64) {
            params.push(("max_price", format!("{}", m as i64)));
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
            params.push(("rating", code.to_string()));
        }
        match get_str(a, "sort").to_lowercase().as_str() {
            "price" => params.push(("sort_by", "3".to_string())),
            "rating" => params.push(("sort_by", "8".to_string())),
            "reviews" => params.push(("sort_by", "13".to_string())),
            _ => {}
        }
        let data = self.get("search.json", &params).await?;
        Ok(compact_hotels(&data, &currency))
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
        let guests = a
            .get("guests")
            .and_then(Value::as_i64)
            .filter(|n| *n >= 1)
            .unwrap_or(2);
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
    fn airports_from_codes_and_city_names() {
        assert_eq!(airport_codes("sin").as_deref(), Some("SIN"));
        assert_eq!(airport_codes("HND, NRT").as_deref(), Some("HND,NRT"));
        assert_eq!(airport_codes("Tokyo").as_deref(), Some("HND,NRT"));
        assert_eq!(airport_codes("Ho-Chi-Minh City").as_deref(), Some("SGN"));
        assert_eq!(airport_codes("Atlantis"), None);
        assert_eq!(airport_codes(""), None);
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
