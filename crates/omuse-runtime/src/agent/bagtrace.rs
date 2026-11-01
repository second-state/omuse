//! The delayed-bag lookup's known page: WorldTracer's public tracing page (claims_real 2026-11-01, twice: "WorldTracer's
//! tracking form was available, but I couldn't submit the reference and surname from this bot"). The `bag_trace`
//! workflow tool (`super::claims`) takes the PIR's file reference as its JSON argument (the model reads it in the
//! user's words or the PIR email), opens the tracing page, fills it by its form function (`crate::forms`, fixed ids:
//! the reference and the family name), presses Track (a lookup changes nothing) and reads the result page's fixed rows
//! (Status, Last update, Flight) here; the status goes into the result's `must_say`.

use std::sync::LazyLock;

use chrono::NaiveDate;
use regex::Regex;
use serde_json::Value;

use super::util::content_of;

/// The skill whose runs call `bag_trace`.
pub const SKILL: &str = "baggage-delayed-lost";
/// WorldTracer's public tracing page (the airlines' "track your bag" links).
pub const TRACE_URL: &str = "https://wtrweb.worldtracer.aero/WorldTracerWeb/bagtrace.html";
pub const TRACE_HOST: &str = "wtrweb.worldtracer.aero";

/// The shape of a WorldTracer file reference: the station (3 letters), the airline (2), five digits — "NRTSQ12345".
static FILE_REF: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^([A-Z]{3})[ -]?([A-Z][A-Z0-9]|[0-9][A-Z])[ -]?(\d{5})$").unwrap()
});

/// A file reference argument, normalised without spaces ("nrt sq 12345" → "NRTSQ12345"); `None` when it is not one.
pub fn file_ref_of(value: &str) -> Option<String> {
    FILE_REF
        .captures(&value.trim().to_uppercase())
        .map(|c| format!("{}{}{}", &c[1], &c[2], &c[3]))
}

/// A tool result that failed (the step is not retried; the result says the lookup could not be done).
pub fn failed(m: &Value) -> bool {
    let c = content_of(m);
    let c = c.trim_start();
    c.starts_with("ERROR") || c.starts_with("DENIED") || c.starts_with("BLOCKED")
}

/// What WorldTracer said about the file.
#[derive(Debug, Clone, PartialEq)]
pub struct Trace {
    pub file_ref: String,
    pub status: String,
    pub last_update: String,
    /// the file's flight date (its Flight row)
    pub flight_date: Option<NaiveDate>,
    /// WorldTracer knows the file (false: "No file found")
    pub found: bool,
}

static STATUS: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?m)^\s*Status\s*[:|]?\s+(\S.*?)\s*$").unwrap());
static LAST_UPDATE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?m)^\s*Last update\s*[:|]?\s+(\S.*?)\s*$").unwrap());
static FLIGHT_ROW: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?m)^\s*Flight\s*[:|]?\s+.*?\b(20\d\d-\d\d-\d\d)\b").unwrap());
static NO_FILE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)no file found[^\n.]*").unwrap());

/// The status on the tracing result page (its fixed rows: Status, Last update, Flight; or "No file found"); `None` when
/// the page shows neither.
pub fn trace_of(snapshot: &str, file_ref: &str) -> Option<Trace> {
    let (status, found) = match STATUS.captures(snapshot) {
        Some(c) => (c[1].to_string(), true),
        None => (NO_FILE.find(snapshot)?.as_str().trim().to_string(), false),
    };
    let last_update = LAST_UPDATE
        .captures(snapshot)
        .map(|c| c[1].to_string())
        .unwrap_or_default();
    let flight_date = FLIGHT_ROW
        .captures(snapshot)
        .and_then(|c| NaiveDate::parse_from_str(&c[1], "%Y-%m-%d").ok());
    Some(Trace {
        file_ref: file_ref.to_string(),
        status,
        last_update,
        flight_date,
        found,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const RESULT: &str = "# Track your delayed baggage\n[e1] textbox \"File reference\" value=\"NRTSQ12345\"\n[e2] textbox \"Family name\" value=\"MING\"\n[e3] button \"Track\"\n## File NRTSQ12345\n  Status Tracing continues — your bag has not been located yet\n  Bag Black hard-shell suitcase, tag SQ 123456\n  Flight SQ 638 Singapore (SIN) – Tokyo Narita (NRT), 2026-10-31\n  Last update 2026-10-31 22:10 local time — trace sent to Singapore (SIN)\n  Delivery address on file Hotel Ginza Grand, 1-1-1 Ginza, Chuo City, Tokyo";

    #[test]
    fn the_result_pages_fixed_rows_are_read() {
        assert_eq!(
            trace_of(RESULT, "NRTSQ12345"),
            Some(Trace {
                file_ref: "NRTSQ12345".into(),
                status: "Tracing continues — your bag has not been located yet".into(),
                last_update: "2026-10-31 22:10 local time — trace sent to Singapore (SIN)".into(),
                flight_date: NaiveDate::from_ymd_opt(2026, 10, 31),
                found: true,
            })
        );
        let none = trace_of("## Track your delayed baggage\n  No file found for this reference. Check the reference on your Property Irregularity Report.", "NRTSQ99999").unwrap();
        assert_eq!(
            (none.status.as_str(), none.found),
            ("No file found for this reference", false)
        );
        assert!(trace_of("Maintenance", "NRTSQ12345").is_none());
    }

    #[test]
    fn a_file_reference_argument_is_checked_by_its_shape() {
        assert_eq!(file_ref_of("NRT SQ 12345").as_deref(), Some("NRTSQ12345"));
        assert_eq!(file_ref_of("nrtsq12345").as_deref(), Some("NRTSQ12345"));
        assert_eq!(file_ref_of("SIN-3K-00123").as_deref(), Some("SIN3K00123"));
        assert!(file_ref_of("7HK2LM").is_none());
        assert!(file_ref_of("NRTSQ1234").is_none());
    }
}
