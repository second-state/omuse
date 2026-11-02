//! Flight data from Kiwi.com's public MCP server (`https://mcp.kiwi.com/`, streamable HTTP, no key): the first source
//! behind `flights_search`. One Kiwi search per call (a range of dates included); connections on one booking only
//! unless the user explicitly asks for self-transfer (separate tickets). Each offer keeps Kiwi's booking link (a
//! `kiwi.com` address only), which opens Kiwi's booking page for that itinerary; the airline's own site (`book_at`) is
//! the other way to book. Called by Sentinel itself (one JSON-RPC request per search: `tools/call` without the MCP handshake), not registered as a connector.

use std::fmt;
use std::time::Duration;

use omuse_common::util::{get_str, truthy};
use serde_json::{json, Map, Value};

use crate::serpapi::{short_id, token_of};

/// A Kiwi failure (HTTP, a refused request, the `error` of a reply).
#[derive(Debug)]
pub struct KiwiError(pub String);

impl fmt::Display for KiwiError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for KiwiError {}

/// The client for Kiwi's MCP endpoint.
pub struct KiwiMcp {
    client: reqwest::Client,
    url: String,
    timeout: Duration,
}

/// The `data:` payload of a streamable-HTTP reply, or the plain JSON body.
fn payload(text: &str) -> Option<Value> {
    let t = text.trim();
    if t.starts_with('{') {
        return serde_json::from_str(t).ok();
    }
    let mut last = None;
    for line in t.lines() {
        if let Some(d) = line.strip_prefix("data:") {
            if let Ok(v) = serde_json::from_str::<Value>(d.trim()) {
                last = Some(v);
            }
        }
    }
    last
}

fn arr<'a>(v: &'a Value, k: &str) -> &'a [Value] {
    v.get(k)
        .and_then(Value::as_array)
        .map_or(&[], Vec::as_slice)
}

fn mins(secs: Option<&Value>) -> Value {
    secs.and_then(Value::as_f64)
        .map(|s| json!((s / 60.0).round() as i64))
        .unwrap_or(Value::Null)
}

/// One direction (outbound / inbound) of a Kiwi itinerary: the legs and a one-line route.
fn direction(d: &Value) -> (Vec<Value>, String, Vec<String>) {
    let legs: Vec<Value> = arr(d, "segments")
        .iter()
        .map(|s| {
            json!({
                "airline": get_str(s, "carrierName"),
                "flight_number": get_str(s, "flightNumber"),
                "from": get_str(s, "from"),
                "depart": get_str(s, "departureTime").replace('T', " "),
                "to": get_str(s, "to"),
                "arrive": get_str(s, "arrivalTime").replace('T', " "),
                "duration_min": mins(s.get("durationSeconds")),
                "cabin": get_str(s, "cabinClass"),
            })
        })
        .collect();
    let route = legs
        .iter()
        .map(|l| {
            format!(
                "{} {} {} {} → {} {}",
                get_str(l, "airline"),
                get_str(l, "flight_number"),
                get_str(l, "from"),
                get_str(l, "depart"),
                get_str(l, "to"),
                get_str(l, "arrive")
            )
        })
        .collect::<Vec<_>>()
        .join(" | ");
    let stops: Vec<String> = arr(d, "route")
        .iter()
        .filter_map(Value::as_str)
        .map(str::to_string)
        .collect();
    let layovers = if stops.len() > 2 {
        stops[1..stops.len() - 1].to_vec()
    } else {
        vec![]
    };
    (legs, route, layovers)
}

/// Kiwi's booking link when it is a `kiwi.com` address (`https://kiwi.com/u/…`), else "" — the page it opens is the
/// booking page for that itinerary.
pub fn booking_link(it: &Value) -> String {
    let url = get_str(it, "bookingUrl");
    let ok = url::Url::parse(&url).ok().is_some_and(|u| {
        u.scheme() == "https"
            && u.host_str()
                .is_some_and(|h| h == "kiwi.com" || h.ends_with(".kiwi.com"))
    });
    if ok {
        url
    } else {
        String::new()
    }
}

/// A Kiwi itinerary, compact, with its booking link; the `offer_id` (`kw_…`) brings the same details back through
/// `flight_offer`. `self_transfer`: the search allowed separate-ticket connections.
pub fn compact_itinerary(it: &Value, currency: &str, self_transfer: bool) -> Value {
    let (legs, route, layovers) = direction(&it["outbound"]);
    let mut airlines: Vec<String> = Vec::new();
    for l in &legs {
        let a = get_str(l, "airline");
        if !a.is_empty() && !airlines.contains(&a) {
            airlines.push(a);
        }
    }
    // where to book: each operating airline's own website (Kiwi gives no link), from the flight numbers
    let mut book_at: Vec<&str> = Vec::new();
    for l in &legs {
        if let Some(site) = crate::loyalty::airline_site(&get_str(l, "flight_number")) {
            if !book_at.contains(&site) {
                book_at.push(site);
            }
        }
    }
    let mut out = Map::new();
    let id = get_str(it, "id");
    let mut offer = json!({
        "airlines": airlines.join(" + "),
        "route": route,
        "departure": {"airport": get_str(&it["outbound"], "from"), "time": get_str(&it["outbound"], "departureTime").replace('T', " ")},
        "arrival": {"airport": get_str(&it["outbound"], "to"), "time": get_str(&it["outbound"], "arrivalTime").replace('T', " ")},
        "duration_min": mins(it.get("totalDurationSeconds")),
        "stops": it["outbound"].get("stops").cloned().unwrap_or(json!(layovers.len())),
        "layovers": layovers,
        "price": it.get("price").cloned().unwrap_or(Value::Null),
        "currency": currency,
        "trip": if truthy(&it["inbound"]) { "Round trip" } else { "One way" },
        "legs": legs,
        "baggage": it.get("baggage").cloned().unwrap_or(json!({})),
        // without self-transfer (the default) the connections are on one booking
        "ticket": if !self_transfer || legs.len() < 2 {
            "one booking, no self-transfer connections"
        } else {
            "may be separate tickets (self-transfer allowed): a missed connection is the traveller's risk"
        },
        "data": "Kiwi.com",
        "booking_url": booking_link(it),
        "book": format!("on Kiwi.com with booking_url (its booking page for this itinerary), or on the airline's own site ({})", airlines.join(", ")),
        "book_at": book_at,
    });
    if truthy(&it["inbound"]) {
        let (ilegs, iroute, ilay) = direction(&it["inbound"]);
        offer["inbound"] = json!({"route": iroute, "legs": ilegs, "layovers": ilay,
            "departure": {"airport": get_str(&it["inbound"], "from"), "time": get_str(&it["inbound"], "departureTime").replace('T', " ")},
            "arrival": {"airport": get_str(&it["inbound"], "to"), "time": get_str(&it["inbound"], "arrivalTime").replace('T', " ")}});
    }
    if !id.is_empty() {
        // the offer id resolves to this very offer (no token at Kiwi: the details are the token)
        out.insert("offer_id".into(), json!(short_id("kw", &offer.to_string())));
    }
    for (k, v) in offer.as_object().cloned().unwrap_or_default() {
        out.insert(k, v);
    }
    Value::Object(out)
}

/// The whole reply, compact: every itinerary (none cut — the pick is made over all of them).
pub fn compact_flights(data: &Value, self_transfer: bool) -> Value {
    let currency = get_str(data, "currency");
    let mut offers: Vec<Value> = arr(data, "itineraries")
        .iter()
        .map(|it| compact_itinerary(it, &currency, self_transfer))
        .collect();
    for (i, o) in offers.iter_mut().enumerate() {
        o["rank"] = json!(i + 1);
    }
    let mut out = json!({
        "count": offers.len(),
        "currency": currency,
        "offers": offers,
        "data": "Kiwi.com",
        "book": "on Kiwi.com with the offer's booking_url, or on the airline's own site (book_at)",
    });
    if let Some(e) = data.get("error").filter(|e| truthy(e)) {
        out["note"] = json!(e
            .as_str()
            .map(str::to_string)
            .unwrap_or_else(|| e.to_string()));
    }
    out
}

/// The details behind a `kw_…` offer id from an earlier search (the compact offer itself).
pub fn offer_details(id: &str) -> Option<Value> {
    let raw = token_of(id)?;
    let mut v: Value = serde_json::from_str(&raw).ok()?;
    v["offer_id"] = json!(id);
    let mut opts = vec![
        json!({"book_with": "Kiwi.com", "link": v["booking_url"].clone(),
        "note": "Kiwi.com's booking page for this itinerary: the traveller's details, then the payment"}),
    ];
    for site in v["book_at"].as_array().cloned().unwrap_or_default() {
        opts.push(json!({"book_with": site, "link": "", "note": "the airline's own site: search the same flights there"}));
    }
    v["booking_options"] = json!(opts);
    Some(v)
}

impl KiwiMcp {
    pub fn new(client: reqwest::Client, url: &str) -> Self {
        Self {
            client,
            url: url.to_string(),
            timeout: Duration::from_secs(90),
        }
    }

    /// `flights_search` through Kiwi: the same arguments as the Google path (origin, destination, date, date_to,
    /// return_date, adults, cabin, stops, max_price, airlines, sort, currency) in ONE Kiwi search — a range of
    /// departure dates (up to 14 days) is Kiwi's own `departureDateTo`, and a round trip over a range keeps the stay's
    /// length as nights at the destination. Every offer carries its date; `by_date` gives the cheapest per day.
    pub async fn flights(&self, a: &Value) -> Result<Value, KiwiError> {
        let (days, cut) = crate::serpapi::date_range(a, 14).map_err(KiwiError)?;
        if days.len() == 1 && !cut {
            return self.search(a, None).await;
        }
        let last = days.last().map(|d| d.0.clone()).unwrap_or_default();
        let out = self.search(a, Some(&last)).await?;
        // the one result, split by departure date for merge_days (offers, by_date, searched.dates)
        let mut per_day: Vec<(String, Value)> = days
            .iter()
            .map(|(d, _)| {
                let mut v = out.clone();
                v["offers"] = json!(arr(&out, "offers")
                    .iter()
                    .filter(|o| get_str(&o["departure"], "time").starts_with(d.as_str()))
                    .cloned()
                    .collect::<Vec<_>>());
                (d.clone(), v)
            })
            .collect();
        if let Some(first) = per_day.first_mut() {
            first.1["searched"] = out["searched"].clone();
        }
        Ok(crate::serpapi::merge_days(
            per_day,
            cut,
            &get_str(a, "date_to"),
        ))
    }

    /// One Kiwi search: `date`, or every departure from `date` to `last` (inclusive).
    async fn search(&self, a: &Value, last: Option<&str>) -> Result<Value, KiwiError> {
        let (origin, dest, date) = (
            get_str(a, "origin"),
            get_str(a, "destination"),
            get_str(a, "date"),
        );
        if origin.trim().is_empty() || dest.trim().is_empty() {
            return Err(KiwiError("origin and destination are required".into()));
        }
        if date.len() != 10 {
            return Err(KiwiError("date must be YYYY-MM-DD".into()));
        }
        let cabin = match get_str(a, "cabin").to_lowercase().as_str() {
            "premium_economy" | "premium economy" => "PREMIUM_ECONOMY",
            "business" => "BUSINESS",
            "first" => "FIRST_CLASS",
            _ => "ECONOMY",
        };
        let sort = match get_str(a, "sort").to_lowercase().as_str() {
            "duration" => "duration",
            "best" | "quality" => "quality",
            _ => "price",
        };
        let currency = {
            let c = get_str(a, "currency").to_uppercase();
            if c.is_empty() {
                "USD".to_string()
            } else {
                c
            }
        };
        let (from, to) = (
            crate::airports::kiwi_place(&origin),
            crate::airports::kiwi_place(&dest),
        );
        let mut args = json!({
            "flyFrom": from.clone(), "flyTo": to.clone(), "departureDate": date,
            "adults": crate::serpapi::people(a, "adults", 1, 9),
            "cabinClass": cabin, "currency": currency, "sort": sort,
        });
        // connections on one booking only, unless the user explicitly asked for self-transfer (separate tickets: a
        // missed connection is the traveller's problem)
        let self_transfer = a.get("self_transfer").and_then(Value::as_bool) == Some(true);
        args["allow_self_transfer"] = json!(self_transfer);
        let ret = get_str(a, "return_date");
        match last {
            Some(last) => {
                args["departureDateTo"] = json!(last);
                // over a range the return moves with each departure: the stay's length as nights at the destination
                let stay = chrono::NaiveDate::parse_from_str(&ret, "%Y-%m-%d")
                    .ok()
                    .zip(chrono::NaiveDate::parse_from_str(&date, "%Y-%m-%d").ok())
                    .map(|(r, d)| (r - d).num_days());
                if let Some(n) = stay.filter(|n| *n >= 0) {
                    args["nights_in_dst_from"] = json!(n);
                    args["nights_in_dst_to"] = json!(n);
                }
            }
            None if ret.len() == 10 => args["returnDate"] = json!(ret),
            None => {}
        }
        if let Some(n) = crate::serpapi::max_stops(a) {
            args["max_sector_stopovers"] = json!(n);
        }
        if let Some(m) = crate::serpapi::price_cap(a, "max_price") {
            args["price_to"] = json!(m);
        }
        // preferred airlines narrow the search itself: Kiwi takes IATA codes only, so an alliance goes in as all its
        // members and a name as its code (crate::loyalty::airline_codes); one it cannot place leaves all airlines in
        let prefs = crate::serpapi::pref_list(a, "airlines");
        let filter = crate::loyalty::airline_codes(&prefs);
        if let Ok(codes) = &filter {
            if !codes.is_empty() {
                args["select_airlines"] = json!(codes.join(","));
            }
        }
        // the user's time window (the model's JSON: depart_after … arrive_before) as Kiwi's own hour filters, in the
        // same ONE request
        for (k, v) in kiwi_times(a) {
            args[k] = json!(v);
        }
        let data = self.call(&args).await?;
        let mut out = compact_flights(&data, self_transfer);
        let found = crate::serpapi::mark_preferred(
            &mut out["offers"],
            &crate::serpapi::with_codes(&prefs, &filter),
            "airlines",
        );
        crate::serpapi::tag_cabins(&mut out["offers"], a);
        out["searched"] = crate::serpapi::searched(a, &from, &to);
        if !prefs.is_empty() {
            out["searched"]["airlines"] = json!(prefs);
            out["searched"]["preferred_found"] = json!(found);
            out["searched"]["airline_filter"] = crate::serpapi::airline_filter_note(&filter);
        }
        Ok(out)
    }

    /// One `search-flight` request: Kiwi's reply data (`{currency, itineraries, …}`).
    async fn call(&self, args: &Value) -> Result<Value, KiwiError> {
        let post = |body: Value| {
            self.client
                .post(&self.url)
                .header("Content-Type", "application/json")
                .header("Accept", "application/json, text/event-stream")
                .json(&body)
                .timeout(self.timeout)
                .send()
        };
        // Kiwi's server is stateless and answers tools/call without the MCP handshake (checked 2026-10-29), so the
        // `initialize` round trip is skipped
        let r = post(json!({"jsonrpc": "2.0", "id": 2, "method": "tools/call", "params": {"name": "search-flight", "arguments": args}}))
            .await
            .map_err(|e| KiwiError(format!("Kiwi: {e}")))?;
        let status = r.status().as_u16();
        let text = r
            .text()
            .await
            .map_err(|e| KiwiError(format!("Kiwi: {e}")))?;
        if status >= 400 {
            return Err(KiwiError(format!(
                "Kiwi HTTP {status}: {}",
                text.chars().take(160).collect::<String>()
            )));
        }
        let msg = payload(&text).ok_or_else(|| KiwiError("Kiwi: unexpected reply".into()))?;
        if let Some(e) = msg.get("error") {
            return Err(KiwiError(format!("Kiwi: {}", get_str(e, "message"))));
        }
        let content = msg["result"]["content"]
            .as_array()
            .map(|a| {
                a.iter()
                    .map(|c| get_str(c, "text"))
                    .collect::<Vec<_>>()
                    .join("\n")
            })
            .unwrap_or_default();
        if msg["result"]["isError"].as_bool().unwrap_or(false) {
            return Err(KiwiError(format!(
                "Kiwi: {}",
                content.chars().take(200).collect::<String>()
            )));
        }
        serde_json::from_str(content.trim()).map_err(|_| {
            KiwiError(format!(
                "Kiwi: unexpected reply: {}",
                content.chars().take(120).collect::<String>()
            ))
        })
    }
}

/// The request's local time window (`depart_after` / `depart_before` / `arrive_after` / `arrive_before`, HH:MM) as
/// Kiwi's hour filters (`dtime_from` / `dtime_to`, `atime_from` / `atime_to`, 0–23, local at each airport). Checked on
/// mcp.kiwi.com 2026-11-02: `dtime_to` h takes departures up to h:00 and 23 is its largest value — so an end is rounded
/// up to the next hour (never cutting a flight inside the window) and an end after 23:00 is no end; a start is its
/// hour (00 is no start). The window is a superset at most an hour wider: the pick over the result is the model's.
pub fn kiwi_times(a: &Value) -> Vec<(&'static str, u32)> {
    let w = crate::serpapi::time_window(a);
    let mut out = Vec::new();
    for (range, from_k, to_k) in [
        (w.depart, "dtime_from", "dtime_to"),
        (w.arrive, "atime_from", "atime_to"),
    ] {
        let Some((from, to)) = range else { continue };
        if from / 60 > 0 {
            out.push((from_k, from / 60));
        }
        let end = to.div_ceil(60);
        if end <= 23 {
            out.push((to_k, end));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_users_time_window_becomes_kiwis_hour_filters() {
        // "the last flight that lands before midnight", narrowed by the model to arrivals from 18:00
        let w = kiwi_times(&json!({"arrive_after": "18:00", "arrive_before": "23:59"}));
        assert_eq!(
            w,
            [("atime_from", 18)],
            "23:59 is no end (dtime_to 23 would cut 23:01–23:59)"
        );
        // an end is rounded up to the next hour; a start of 00:00 is no start
        let w = kiwi_times(
            &json!({"depart_after": "00:00", "depart_before": "09:30", "arrive_after": "06:15"}),
        );
        assert_eq!(w, [("dtime_to", 10), ("atime_from", 6)]);
        assert_eq!(
            kiwi_times(&json!({"depart_before": "18:00"})),
            [("dtime_to", 18)]
        );
        // no time words: no filter (one normal search)
        assert!(kiwi_times(&json!({"origin": "HKG"})).is_empty());
        assert!(kiwi_times(&json!({"depart_after": "late"})).is_empty());
    }

    #[test]
    fn kiwi_itineraries_compact_with_the_kiwi_booking_link() {
        let data = json!({"query": "SIN → NRT", "currency": "SGD", "resultsCount": 1, "itineraries": [{
            "id": "abc_0|abc_1", "price": 341.0, "priceFormatted": "341 SGD", "totalDurationSeconds": 62700, "bookingUrl": "https://kiwi.com/u/y5yxg2q",
            "baggage": {"personalItem": 1, "cabinBag": 1, "checkedBag": 1},
            "outbound": {"from": "SIN", "to": "NRT", "departureTime": "2026-11-26T02:30:00", "arrivalTime": "2026-11-26T20:55:00", "durationSeconds": 62700, "stops": 2,
                         "route": ["SIN", "KMG", "PVG", "NRT"], "segments": [
                {"from": "SIN", "to": "KMG", "departureTime": "2026-11-26T02:30:00", "arrivalTime": "2026-11-26T06:55:00", "durationSeconds": 15900, "carrier": "MU", "carrierName": "China Eastern Airlines", "flightNumber": "MU9648", "cabinClass": "Economy"},
                {"from": "KMG", "to": "PVG", "departureTime": "2026-11-26T10:30:00", "arrivalTime": "2026-11-26T13:30:00", "durationSeconds": 10800, "carrier": "MU", "carrierName": "China Eastern Airlines", "flightNumber": "MU9743", "cabinClass": "Economy"},
                {"from": "PVG", "to": "NRT", "departureTime": "2026-11-26T17:10:00", "arrivalTime": "2026-11-26T20:55:00", "durationSeconds": 9900, "carrier": "MU", "carrierName": "China Eastern Airlines", "flightNumber": "MU271", "cabinClass": "Economy"}]},
            "inbound": null}]});
        let out = compact_flights(&data, false);
        assert!(!out.to_string().contains("bookingUrl"));
        assert_eq!(out["count"], 1);
        let o = &out["offers"][0];
        assert_eq!(o["airlines"], "China Eastern Airlines");
        assert_eq!(o["stops"], 2);
        assert_eq!(o["layovers"], json!(["KMG", "PVG"]));
        assert_eq!(o["price"], 341.0);
        assert_eq!(o["currency"], "SGD");
        assert_eq!(o["duration_min"], 1045);
        assert_eq!(o["legs"].as_array().unwrap().len(), 3);
        assert_eq!(o["legs"][0]["flight_number"], "MU9648");
        assert_eq!(o["booking_url"], "https://kiwi.com/u/y5yxg2q");
        assert!(o["book"].as_str().unwrap().contains("booking_url"));
        assert_eq!(o["ticket"], "one booking, no self-transfer connections");
        // self-transfer allowed (the user asked): a connection may be on separate tickets
        let st = compact_flights(&data, true);
        assert!(st["offers"][0]["ticket"]
            .as_str()
            .unwrap()
            .starts_with("may be separate tickets"));
        // a link that is not Kiwi's is dropped
        for bad in [
            "http://kiwi.com/u/x",
            "https://kiwi.com.evil.test/u/x",
            "https://evil.test/?kiwi.com",
            "javascript:alert(1)",
        ] {
            assert_eq!(booking_link(&json!({"bookingUrl": bad})), "", "{bad}");
        }
        assert_eq!(
            booking_link(&json!({"bookingUrl": "https://www.kiwi.com/en/booking/?token=x"})),
            "https://www.kiwi.com/en/booking/?token=x"
        );
        let id = o["offer_id"].as_str().unwrap().to_string();
        assert!(id.starts_with("kw_"));
        let d = offer_details(&id).unwrap();
        assert_eq!(d["offer_id"], id);
        assert_eq!(d["airlines"], "China Eastern Airlines");
        assert_eq!(d["booking_options"][0]["book_with"], "Kiwi.com");
        assert_eq!(
            d["booking_options"][0]["link"],
            "https://kiwi.com/u/y5yxg2q"
        );
        assert!(payload("event: message\ndata: {\"a\": 1}\n").unwrap()["a"] == 1);
    }
}
