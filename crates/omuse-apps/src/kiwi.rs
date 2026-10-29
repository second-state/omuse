//! Flight data from Kiwi.com's public MCP server (`https://mcp.kiwi.com/`, streamable HTTP, no key): the default
//! source behind `flights_search` when no SerpApi key is on hand. Information only — itineraries, times, carriers,
//! prices, baggage — the booking links Kiwi returns are dropped: the booking happens on the airline's own site or
//! with an agency the user chooses. Called by Sentinel itself (two JSON-RPC requests), not registered as a connector.

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

/// A Kiwi itinerary, compact and without its booking link; the `offer_id` (`kw_…`) brings the same details back
/// through `flight_offer`.
pub fn compact_itinerary(it: &Value, currency: &str) -> Value {
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
        "data": "Kiwi.com",
        "book": format!("on the airline's own site ({}) or with an agency the user chooses — never through Kiwi", airlines.join(", ")),
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

/// The whole reply, compact (at most 12 itineraries, no links).
pub fn compact_flights(data: &Value) -> Value {
    let currency = get_str(data, "currency");
    let mut offers: Vec<Value> = arr(data, "itineraries")
        .iter()
        .take(12)
        .map(|it| compact_itinerary(it, &currency))
        .collect();
    for (i, o) in offers.iter_mut().enumerate() {
        o["rank"] = json!(i + 1);
    }
    let mut out = json!({
        "count": offers.len(),
        "currency": currency,
        "offers": offers,
        "data": "Kiwi.com (information only)",
        "book": "on the airline's own site or with an agency the user chooses — never through Kiwi",
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
    v["booking_options"] = json!([{"book_with": "the airline (see airlines) or an agency the user chooses", "link": "",
        "note": "Kiwi.com data is for information only; the booking happens on the airline's own site or with an agency of the user's choice."}]);
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

    /// `flights_search` through Kiwi: the same arguments as the Google path (origin, destination, date, return_date,
    /// adults, cabin, nonstop_only, max_price, sort, currency).
    pub async fn flights(&self, a: &Value) -> Result<Value, KiwiError> {
        let (days, cut) = crate::serpapi::date_range(a, 14).map_err(KiwiError)?;
        if days.len() == 1 && !cut {
            return self.flights_day(a).await;
        }
        // four at a time: one call per day, merged into one result
        use futures_util::StreamExt;
        let runs = days.iter().cloned().map(|(d, r)| {
            let mut x = a.clone();
            x["date"] = json!(d);
            x["return_date"] = json!(r);
            async move {
                let res = self.flights_day(&x).await;
                (d, res)
            }
        });
        let mut got: Vec<(String, Result<Value, KiwiError>)> =
            futures_util::stream::iter(runs).buffered(4).collect().await;
        got.sort_by(|x, y| x.0.cmp(&y.0));
        let out = got
            .into_iter()
            .map(|(d, r)| (d, r.unwrap_or_else(|e| json!({"offers": [], "note": e.0}))))
            .collect();
        Ok(crate::serpapi::merge_days(out, cut, &get_str(a, "date_to")))
    }

    /// One departure date (the range is handled by [`KiwiMcp::flights`]).
    async fn flights_day(&self, a: &Value) -> Result<Value, KiwiError> {
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
        let ret = get_str(a, "return_date");
        if ret.len() == 10 {
            args["returnDate"] = json!(ret);
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
        let post = |body: Value| {
            self.client
                .post(&self.url)
                .header("Content-Type", "application/json")
                .header("Accept", "application/json, text/event-stream")
                .json(&body)
                .timeout(self.timeout)
                .send()
        };
        let init = post(json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {"protocolVersion": "2025-03-26",
                               "capabilities": {}, "clientInfo": {"name": "omuse", "version": "1"}}}))
            .await
            .map_err(|e| KiwiError(format!("Kiwi: {e}")))?;
        if init.status().as_u16() >= 400 {
            return Err(KiwiError(format!("Kiwi HTTP {}", init.status().as_u16())));
        }
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
        let data: Value = serde_json::from_str(content.trim()).map_err(|_| {
            KiwiError(format!(
                "Kiwi: unexpected reply: {}",
                content.chars().take(120).collect::<String>()
            ))
        })?;
        let mut out = compact_flights(&data);
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
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kiwi_itineraries_compact_without_links() {
        let data = json!({"query": "SIN → NRT", "currency": "SGD", "resultsCount": 1, "itineraries": [{
            "id": "abc_0|abc_1", "price": 341.0, "priceFormatted": "341 SGD", "totalDurationSeconds": 62700, "bookingUrl": "https://kiwi.com/u/y5yxg2q",
            "baggage": {"personalItem": 1, "cabinBag": 1, "checkedBag": 1},
            "outbound": {"from": "SIN", "to": "NRT", "departureTime": "2026-11-26T02:30:00", "arrivalTime": "2026-11-26T20:55:00", "durationSeconds": 62700, "stops": 2,
                         "route": ["SIN", "KMG", "PVG", "NRT"], "segments": [
                {"from": "SIN", "to": "KMG", "departureTime": "2026-11-26T02:30:00", "arrivalTime": "2026-11-26T06:55:00", "durationSeconds": 15900, "carrier": "MU", "carrierName": "China Eastern Airlines", "flightNumber": "MU9648", "cabinClass": "Economy"},
                {"from": "KMG", "to": "PVG", "departureTime": "2026-11-26T10:30:00", "arrivalTime": "2026-11-26T13:30:00", "durationSeconds": 10800, "carrier": "MU", "carrierName": "China Eastern Airlines", "flightNumber": "MU9743", "cabinClass": "Economy"},
                {"from": "PVG", "to": "NRT", "departureTime": "2026-11-26T17:10:00", "arrivalTime": "2026-11-26T20:55:00", "durationSeconds": 9900, "carrier": "MU", "carrierName": "China Eastern Airlines", "flightNumber": "MU271", "cabinClass": "Economy"}]},
            "inbound": null}]});
        let out = compact_flights(&data);
        let s = out.to_string();
        assert!(
            !s.contains("kiwi.com/u/") && !s.contains("bookingUrl"),
            "{s}"
        );
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
        assert!(o["book"].as_str().unwrap().contains("never through Kiwi"));
        let id = o["offer_id"].as_str().unwrap().to_string();
        assert!(id.starts_with("kw_"));
        let d = offer_details(&id).unwrap();
        assert_eq!(d["offer_id"], id);
        assert_eq!(d["airlines"], "China Eastern Airlines");
        assert!(d["booking_options"][0]["link"].as_str().unwrap().is_empty());
        assert!(payload("event: message\ndata: {\"a\": 1}\n").unwrap()["a"] == 1);
    }
}
