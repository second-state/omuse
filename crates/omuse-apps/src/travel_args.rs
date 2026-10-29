//! Input validation of the travel search tools, in code and deterministic: Sentinel runs [`validate`] on the arguments
//! of `flights_search`, `flight_offer`, `hotels_search`, `hotel_rooms`, `places_search` and `place_details` before any
//! data source is asked, and on `web_search` / `news_search` (car rental and everything else without a data tool).
//!
//! A value that makes no sense is replaced with what the user means when there is one, and the change is reported
//! (`fixed`, shown to the model with the result):
//!
//! * price caps (`max_price`, `max_price_per_night`): not a positive number → removed — no budget means every price;
//! * people (`adults` 1–9, `guests` 1–20): 0, negative or not a number → 1; above the limit → the limit;
//! * minimum rating outside 0–5, star class outside 2–5, price level outside 1–4 → removed (no filter); a minimum price
//!   level above the maximum → the two swapped;
//! * cabin, stops, sort: an unknown value → the default (economy, any number of stops, the source's order); `max_stops`
//!   below 0 or above 2 → removed; yes/no flags (`free_cancellation`, `open_now`, `nonstop_only`) → a real true, or
//!   removed;
//! * currency: not a three-letter code → removed (the tool's default); airlines and brands: a list of names, empty and
//!   repeated ones dropped; dates written `2026/11/20` or `2026-11-5` → `2026-11-20`, `2026-11-05`;
//! * `max_results` of the web searches: not 1–20 → 8.
//!
//! What cannot be guessed is refused with a message that says how to fix it (`Err`): a missing or impossible date, a
//! date in the past, a return before the departure, a range that ends before it starts, check-out on or before
//! check-in, a stay over 30 nights, the same origin and destination, a missing city, hotel, offer or place id.

use chrono::NaiveDate;
use serde_json::{json, Map, Value};

/// The tools [`validate`] knows.
pub const TOOLS: [&str; 8] = [
    "flights_search",
    "flight_offer",
    "hotels_search",
    "hotel_rooms",
    "places_search",
    "place_details",
    "web_search",
    "news_search",
];

/// The longest hotel stay one search takes.
pub const MAX_NIGHTS: i64 = 30;

/// Validated arguments and one note per value that was replaced or dropped.
#[derive(Debug, Clone, PartialEq)]
pub struct Checked {
    pub args: Value,
    pub fixed: Vec<String>,
}

fn num(v: &Value) -> Option<f64> {
    match v {
        Value::Number(n) => n.as_f64(),
        Value::String(s) => s.trim().parse::<f64>().ok(),
        _ => None,
    }
    .filter(|x| x.is_finite())
}

fn text(m: &Map<String, Value>, k: &str) -> String {
    match m.get(k) {
        Some(Value::String(s)) => s.trim().to_string(),
        Some(Value::Number(n)) => n.to_string(),
        _ => String::new(),
    }
}

/// `YYYY-MM-DD`, also from `YYYY/MM/DD`, `YYYY.MM.DD` and one-digit months or days.
pub fn parse_date(s: &str) -> Option<NaiveDate> {
    let parts: Vec<&str> = s.trim().split(['-', '/', '.']).collect();
    if parts.len() != 3 || parts[0].len() != 4 {
        return None;
    }
    let n: Vec<u32> = parts.iter().filter_map(|p| p.parse().ok()).collect();
    if n.len() != 3 {
        return None;
    }
    NaiveDate::from_ymd_opt(n[0] as i32, n[1], n[2])
}

struct V<'a> {
    m: Map<String, Value>,
    fixed: Vec<String>,
    today: NaiveDate,
    tool: &'a str,
}

impl V<'_> {
    fn drop(&mut self, k: &str, why: &str) {
        if let Some(v) = self.m.remove(k) {
            self.fixed.push(format!("{k} {v} → {why}"));
        }
    }

    fn set(&mut self, k: &str, new: Value) {
        let old = self.m.get(k).cloned().unwrap_or(Value::Null);
        if old != new {
            self.fixed.push(format!("{k} {old} → {new}"));
            self.m.insert(k.to_string(), new);
        }
    }

    fn price_cap(&mut self, k: &str) {
        if let Some(v) = self.m.get(k) {
            match num(v).filter(|x| *x > 0.0) {
                Some(x) => {
                    let x = json!(x);
                    if self.m.get(k).and_then(Value::as_f64).is_none() {
                        self.set(k, x);
                    }
                }
                None => self.drop(k, "no limit (every price)"),
            }
        }
    }

    fn people(&mut self, k: &str, max: i64) {
        match self.m.get(k) {
            None => {}
            Some(Value::Null) => {
                self.m.remove(k);
            }
            Some(v) => {
                let n = num(v)
                    .filter(|x| *x >= 1.0)
                    .map(|x| (x.floor() as i64).min(max))
                    .unwrap_or(1);
                self.set(k, json!(n));
            }
        }
    }

    /// A number filter in `lo..=hi` (whole numbers when `int`); anything else is removed.
    fn range(&mut self, k: &str, lo: f64, hi: f64, lo_open: bool, int: bool) {
        let Some(v) = self.m.get(k) else { return };
        match num(v).filter(|x| (if lo_open { *x > lo } else { *x >= lo }) && *x <= hi) {
            Some(x) => {
                let new = if int {
                    json!(x.round() as i64)
                } else {
                    json!(x)
                };
                if v != &new {
                    self.set(k, new);
                }
            }
            None => self.drop(k, "removed (out of range: no filter)"),
        }
    }

    fn flag(&mut self, k: &str) {
        let Some(v) = self.m.get(k) else { return };
        let on = match v {
            Value::Bool(b) => *b,
            Value::Number(n) => n.as_f64().is_some_and(|x| x != 0.0),
            Value::String(s) => matches!(
                s.trim().to_lowercase().as_str(),
                "true" | "yes" | "y" | "1" | "on"
            ),
            _ => false,
        };
        if on {
            if v != &json!(true) {
                self.set(k, json!(true));
            }
        } else {
            self.m.remove(k);
        }
    }

    fn choice(
        &mut self,
        k: &str,
        allowed: &[&str],
        aliases: &[(&str, &str)],
        default: Option<&str>,
    ) {
        let Some(v) = self.m.get(k) else { return };
        let raw = match v {
            Value::String(s) => s.trim().to_lowercase().replace(['-', ' '], "_"),
            Value::Number(n) => n.to_string(),
            _ => String::new(),
        };
        let hit = allowed
            .iter()
            .find(|a| **a == raw)
            .copied()
            .or_else(|| aliases.iter().find(|(a, _)| *a == raw).map(|(_, b)| *b));
        match (hit, default) {
            (Some(h), _) => {
                if v != &json!(h) {
                    self.set(k, json!(h));
                }
            }
            (None, Some(d)) => self.set(k, json!(d)),
            (None, None) => self.drop(k, "removed (unknown value: the default)"),
        }
    }

    fn currency(&mut self) {
        let Some(v) = self.m.get("currency") else {
            return;
        };
        let c = v.as_str().unwrap_or("").trim().to_uppercase();
        if c.len() == 3 && c.chars().all(|x| x.is_ascii_alphabetic()) {
            if v != &json!(c) {
                self.set("currency", json!(c));
            }
        } else {
            self.drop("currency", "removed (not a three-letter code: the default)");
        }
    }

    fn names(&mut self, k: &str) {
        let Some(v) = self.m.get(k) else { return };
        let list: Vec<String> = match v {
            Value::Array(xs) => xs
                .iter()
                .filter_map(|x| x.as_str().map(|s| s.trim().to_string()))
                .collect(),
            Value::String(s) => s.split(',').map(|x| x.trim().to_string()).collect(),
            _ => vec![],
        };
        let mut out: Vec<String> = Vec::new();
        for x in list {
            if !x.is_empty() && !out.iter().any(|y| y.eq_ignore_ascii_case(&x)) {
                out.push(x);
            }
        }
        if out.is_empty() {
            self.drop(k, "removed (no names)");
        } else if v != &json!(out) {
            self.set(k, json!(out));
        }
    }

    fn need(&self, k: &str, msg: &str) -> Result<String, String> {
        let t = text(&self.m, k);
        if t.is_empty() {
            Err(format!("{}: {k} is required — {msg}", self.tool))
        } else {
            Ok(t)
        }
    }

    /// A date argument: written as YYYY-MM-DD; `Err` when it is not a date. `None` when absent or empty (removed).
    fn date(&mut self, k: &str) -> Result<Option<NaiveDate>, String> {
        let t = text(&self.m, k);
        if t.is_empty() {
            self.m.remove(k);
            return Ok(None);
        }
        let d = parse_date(&t).ok_or_else(|| {
            format!(
                "{}: {k} {t:?} is not a date — write it as YYYY-MM-DD (e.g. {})",
                self.tool,
                self.today.format("%Y-%m-%d")
            )
        })?;
        let s = d.format("%Y-%m-%d").to_string();
        if s != t {
            self.set(k, json!(s));
        }
        Ok(Some(d))
    }

    /// The first day of a trip or stay: required, not before yesterday (the user may be a time zone behind).
    fn first_day(&mut self, k: &str) -> Result<NaiveDate, String> {
        let d = self
            .date(k)?
            .ok_or_else(|| format!("{}: {k} is required (YYYY-MM-DD)", self.tool))?;
        if d < self.today.pred_opt().unwrap_or(self.today) {
            return Err(format!(
                "{}: {k} {} is in the past (today is {}) — use the date of the trip",
                self.tool,
                d.format("%Y-%m-%d"),
                self.today.format("%Y-%m-%d")
            ));
        }
        Ok(d)
    }

    fn stay(&mut self) -> Result<(), String> {
        let ci = self.first_day("check_in")?;
        let co = self
            .date("check_out")?
            .ok_or_else(|| format!("{}: check_out is required (YYYY-MM-DD)", self.tool))?;
        let nights = (co - ci).num_days();
        if nights < 1 {
            return Err(format!(
                "{}: check_out must be after check_in (at least one night)",
                self.tool
            ));
        }
        if nights > MAX_NIGHTS {
            return Err(format!(
                "{}: {nights} nights is more than one search takes ({MAX_NIGHTS}) — split the stay",
                self.tool
            ));
        }
        Ok(())
    }
}

/// Validate one travel tool call (see the module doc). `today` is the date the past is measured from.
pub fn validate(tool: &str, args: &Value, today: NaiveDate) -> Result<Checked, String> {
    let m = args.as_object().cloned().unwrap_or_default();
    let mut v = V {
        m,
        fixed: vec![],
        today,
        tool,
    };
    match tool {
        "flights_search" => {
            let o = v.need(
                "origin",
                "an airport code (SIN), a city code (TYO) or a city name",
            )?;
            let d = v.need(
                "destination",
                "an airport code (NRT), a city code (TYO) or a city name",
            )?;
            if o.eq_ignore_ascii_case(&d) {
                return Err(format!(
                    "flights_search: origin and destination are the same ({o})"
                ));
            }
            let first = v.first_day("date")?;
            if let Some(r) = v.date("return_date")? {
                if r < first {
                    return Err("flights_search: return_date is before date".into());
                }
            }
            if let Some(last) = v.date("date_to")? {
                if last < first {
                    return Err("flights_search: date_to is before date".into());
                }
                if last == first {
                    v.m.remove("date_to");
                }
            }
            v.people("adults", 9);
            v.price_cap("max_price");
            v.choice(
                "cabin",
                &["economy", "premium_economy", "business", "first"],
                &[
                    ("premium", "premium_economy"),
                    ("coach", "economy"),
                    ("biz", "business"),
                    ("first_class", "first"),
                ],
                Some("economy"),
            );
            v.choice(
                "stops",
                &["any", "nonstop", "one_stop_max", "two_stops_max"],
                &[
                    ("direct", "nonstop"),
                    ("non_stop", "nonstop"),
                    ("0", "nonstop"),
                    ("1", "one_stop_max"),
                    ("2", "two_stops_max"),
                ],
                Some("any"),
            );
            v.range("max_stops", 0.0, 2.0, false, true);
            v.flag("nonstop_only");
            v.choice(
                "sort",
                &["best", "price", "departure", "arrival", "duration"],
                &[("cheapest", "price")],
                None,
            );
            v.currency();
            v.names("airlines");
        }
        "flight_offer" => {
            v.need("offer_id", "use an offer_id from flights_search")?;
            v.currency();
        }
        "hotels_search" | "hotel_rooms" => {
            if tool == "hotels_search" {
                v.need("city", "the city (and area) to stay in")?;
            } else {
                v.need("hotel_id", "use a hotel_id from hotels_search")?;
            }
            v.stay()?;
            v.people("guests", 20);
            v.price_cap("max_price_per_night");
            v.range("min_stars", 2.0, 5.0, false, true);
            v.range("min_rating", 0.0, 5.0, true, false);
            v.flag("free_cancellation");
            v.choice(
                "sort",
                &["best", "price", "rating", "reviews"],
                &[("cheapest", "price")],
                None,
            );
            v.currency();
            v.names("brands");
        }
        "places_search" => {
            if text(&v.m, "query").is_empty() && text(&v.m, "city").is_empty() {
                return Err(
                    "places_search: give query (what: ramen, sushi, pharmacy …) and city".into(),
                );
            }
            v.range("min_rating", 0.0, 5.0, true, false);
            v.flag("open_now");
            v.range("min_price_level", 1.0, 4.0, false, true);
            v.range("max_price_level", 1.0, 4.0, false, true);
            let (lo, hi) = (
                v.m.get("min_price_level").and_then(Value::as_i64),
                v.m.get("max_price_level").and_then(Value::as_i64),
            );
            if let (Some(lo), Some(hi)) = (lo, hi) {
                if lo > hi {
                    v.set("min_price_level", json!(hi));
                    v.set("max_price_level", json!(lo));
                }
            }
        }
        "place_details" => {
            v.need("place_id", "use a place_id from places_search")?;
        }
        "web_search" | "news_search" => {
            v.need("query", "the search words")?;
            if let Some(x) = v.m.get("max_results") {
                if !num(x).is_some_and(|n| (1.0..=20.0).contains(&n) && n.fract() == 0.0) {
                    v.set("max_results", json!(8));
                }
            }
        }
        _ => {}
    }
    Ok(Checked {
        args: Value::Object(v.m),
        fixed: v.fixed,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn today() -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 10, 29).unwrap()
    }

    fn ok(tool: &str, a: Value) -> Checked {
        validate(tool, &a, today()).unwrap()
    }

    fn err(tool: &str, a: Value) -> String {
        validate(tool, &a, today()).unwrap_err()
    }

    #[test]
    fn flights_numbers_and_choices_become_what_the_user_means() {
        let c = ok(
            "flights_search",
            json!({"origin": "SIN", "destination": "TYO", "date": "2026/11/5", "adults": 0, "max_price": 0,
                   "cabin": "Premium Economy", "stops": "direct", "max_stops": -1, "sort": "cheapest", "currency": "sgd",
                   "airlines": ["SQ", " ", "sq", "NH"], "nonstop_only": "false"}),
        );
        assert_eq!(
            c.args,
            json!({"origin": "SIN", "destination": "TYO", "date": "2026-11-05", "adults": 1, "cabin": "premium_economy",
                   "stops": "nonstop", "sort": "price", "currency": "SGD", "airlines": ["SQ", "NH"]})
        );
        assert!(
            c.fixed
                .iter()
                .any(|f| f.starts_with("max_price 0 → no limit")),
            "{:?}",
            c.fixed
        );
        let c = ok(
            "flights_search",
            json!({"origin": "SIN", "destination": "TYO", "date": "2026-11-20", "adults": 40,
                                             "cabin": "spaceship", "stops": "lots", "currency": "S$", "max_price": "-5"}),
        );
        assert_eq!(c.args["adults"], 9);
        assert_eq!(
            (c.args["cabin"].as_str(), c.args["stops"].as_str()),
            (Some("economy"), Some("any"))
        );
        assert!(c.args.get("currency").is_none() && c.args.get("max_price").is_none());
        // valid input passes untouched
        let good = json!({"origin": "SIN", "destination": "NRT", "date": "2026-11-20", "return_date": "2026-11-27",
                          "adults": 2, "max_price": 900, "cabin": "business", "stops": "any", "currency": "SGD"});
        let c = ok("flights_search", good.clone());
        assert_eq!((c.args, c.fixed.len()), (good, 0));
    }

    #[test]
    fn flights_dates_that_cannot_be_guessed_are_refused() {
        let base = |extra: Value| {
            let mut a = json!({"origin": "SIN", "destination": "NRT", "date": "2026-11-20"});
            for (k, v) in extra.as_object().unwrap() {
                a[k] = v.clone();
            }
            a
        };
        assert!(
            err("flights_search", base(json!({"date": "next friday"}))).contains("is not a date")
        );
        assert!(
            err("flights_search", base(json!({"date": "2026-02-30"}))).contains("is not a date")
        );
        assert!(
            err("flights_search", base(json!({"date": "2026-10-01"}))).contains("is in the past")
        );
        assert!(
            err("flights_search", base(json!({"return_date": "2026-11-10"})))
                .contains("return_date is before date")
        );
        assert!(
            err("flights_search", base(json!({"date_to": "2026-11-01"})))
                .contains("date_to is before date")
        );
        assert!(err("flights_search", base(json!({"destination": "sin"}))).contains("the same"));
        assert!(err(
            "flights_search",
            json!({"destination": "NRT", "date": "2026-11-20"})
        )
        .contains("origin is required"));
        // yesterday is still fine (time zones), an empty return date is a one-way trip
        let c = ok(
            "flights_search",
            base(json!({"date": "2026-10-28", "return_date": "", "date_to": "2026-10-28"})),
        );
        assert!(c.args.get("return_date").is_none() && c.args.get("date_to").is_none());
    }

    #[test]
    fn hotels_stay_guests_and_filters() {
        let c = ok(
            "hotels_search",
            json!({"city": "Tokyo", "check_in": "2026-11-20", "check_out": "2026-11-23", "guests": -2,
                   "max_price_per_night": "0", "min_stars": 9, "min_rating": 0, "free_cancellation": "no",
                   "sort": "stars", "brands": "Hilton, , hilton"}),
        );
        assert_eq!(
            c.args,
            json!({"city": "Tokyo", "check_in": "2026-11-20", "check_out": "2026-11-23", "guests": 1, "brands": ["Hilton"]})
        );
        let c = ok(
            "hotels_search",
            json!({"city": "Tokyo", "check_in": "2026-11-20", "check_out": "2026-11-21",
                                            "guests": "3", "min_stars": "4", "min_rating": 4.5, "free_cancellation": "yes"}),
        );
        assert_eq!(
            (
                c.args["guests"].clone(),
                c.args["min_stars"].clone(),
                c.args["free_cancellation"].clone()
            ),
            (json!(3), json!(4), json!(true))
        );
        let stay = |ci: &str, co: &str| {
            err(
                "hotels_search",
                json!({"city": "Tokyo", "check_in": ci, "check_out": co}),
            )
        };
        assert!(stay("2026-11-20", "2026-11-20").contains("after check_in"));
        assert!(stay("2026-11-20", "2026-11-19").contains("after check_in"));
        assert!(stay("2026-11-01", "2027-01-01").contains("split the stay"));
        assert!(stay("2026-09-01", "2026-09-03").contains("in the past"));
        assert!(err(
            "hotels_search",
            json!({"check_in": "2026-11-20", "check_out": "2026-11-21"})
        )
        .contains("city is required"));
        assert!(err(
            "hotel_rooms",
            json!({"check_in": "2026-11-20", "check_out": "2026-11-21"})
        )
        .contains("hotel_id is required"));
    }

    #[test]
    fn restaurants_and_web_searches() {
        let c = ok(
            "places_search",
            json!({"query": "ramen", "city": "Tokyo", "min_rating": 7, "open_now": "true",
                                            "min_price_level": 4, "max_price_level": 2}),
        );
        assert_eq!(
            c.args,
            json!({"query": "ramen", "city": "Tokyo", "open_now": true, "min_price_level": 2, "max_price_level": 4})
        );
        let c = ok(
            "places_search",
            json!({"query": "sushi", "city": "Osaka", "max_price_level": 0, "open_now": false}),
        );
        assert_eq!(c.args, json!({"query": "sushi", "city": "Osaka"}));
        assert!(err("places_search", json!({"query": " ", "city": ""})).contains("give query"));
        assert!(err("place_details", json!({})).contains("place_id is required"));
        assert!(err("flight_offer", json!({"currency": "SGD"})).contains("offer_id is required"));
        for bad in [json!(0), json!(-1), json!(500), json!("x"), json!(2.5)] {
            let c = ok(
                "web_search",
                json!({"query": "Hertz car rental Narita", "max_results": bad}),
            );
            assert_eq!(c.args["max_results"], 8);
        }
        assert_eq!(
            ok("web_search", json!({"query": "q", "max_results": 5}))
                .fixed
                .len(),
            0
        );
        assert!(err("web_search", json!({"query": ""})).contains("query is required"));
    }

    #[test]
    fn parse_date_accepts_common_spellings_only() {
        assert_eq!(
            parse_date("2026/11/5"),
            NaiveDate::from_ymd_opt(2026, 11, 5)
        );
        assert_eq!(
            parse_date("2026.11.05"),
            NaiveDate::from_ymd_opt(2026, 11, 5)
        );
        assert_eq!(parse_date("20/11/2026"), None);
        assert_eq!(parse_date("2026-13-01"), None);
    }
}
