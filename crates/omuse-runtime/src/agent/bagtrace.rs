//! The delayed-bag lookup's known page: WorldTracer's public tracing page (claims_real 2026-11-01, twice: "WorldTracer's
//! tracking form was available, but I couldn't submit the reference and surname from this bot"). The `bag_trace`
//! workflow tool (`super::claims`) takes the PIR's file reference as its JSON argument (the model reads it in the
//! user's words or the PIR email), opens the tracing page, fills it by its form function (`crate::forms`, fixed ids:
//! the reference and the family name), presses Track (a lookup changes nothing) and reads the result page's table by its
//! HTML elements (`browser_read_elements`: the rows of `#out` with their cells — Status, Last update, Flight —, and its
//! notes, docs/json-tools.md "Reading pages"); the status goes into the result's `must_say`.

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

/// The element reads of the result page: its table's rows (label cell, value cell) and its notes.
const ROWS: &str = "rows";
const NOTES: &str = "notes";

/// The arguments of `browser_read_elements` for the result page.
pub fn read_args() -> serde_json::Value {
    serde_json::json!({"read": [{"name": ROWS, "sel": "#out tr", "cells": true},
                                {"name": NOTES, "sel": "#out p"}]})
}

/// The status on the tracing result page, from its elements read (`read`, a `browser_read_elements` result): the value
/// cells of the rows labelled Status, Last update and Flight (its date, the cell's last part); with no Status row, the
/// page's first note ("No file found for this reference.") as a file WorldTracer does not know. `None` when the page
/// shows neither.
pub fn trace_of(read: &Value, file_ref: &str) -> Option<Trace> {
    let rows = super::confirm::element_rows(read, ROWS);
    let cell = |label: &str| {
        rows.iter()
            .find(|(c, _)| c.first().is_some_and(|l| l.trim() == label))
            .and_then(|(c, _)| c.get(1))
            .map(|v| v.trim().to_string())
    };
    let (status, found) = match cell("Status").filter(|s| !s.is_empty()) {
        Some(s) => (s, true),
        None => {
            let note = super::confirm::element_values(read, NOTES)
                .into_iter()
                .find(|n| !n.trim().is_empty())?;
            // the note's first sentence
            let first = note.split('.').next().unwrap_or(&note).trim().to_string();
            (first, false)
        }
    };
    let flight_date = cell("Flight").and_then(|f| {
        let last = f.rsplit(',').next().unwrap_or("").trim().to_string();
        NaiveDate::parse_from_str(&last, "%Y-%m-%d").ok()
    });
    Some(Trace {
        file_ref: file_ref.to_string(),
        status,
        last_update: cell("Last update").unwrap_or_default(),
        flight_date,
        found,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    use serde_json::json;

    /// A `browser_read_elements` result as the sentinel returns it.
    fn read(elements: Value) -> Value {
        let env = json!({"trust": "untrusted", "source": "web page", "elements": elements});
        json!({"role": "tool", "tool_call_id": "c1",
               "content": format!("<untrusted_content source=\"web page\">\n{env}\n</untrusted_content>")})
    }

    fn result_rows() -> Value {
        let row = |l: &str, v: &str| json!({"cells": [l, v], "links": []});
        json!([
            row(
                "Status",
                "Tracing continues — your bag has not been located yet"
            ),
            row("Bag", "Black hard-shell suitcase, tag SQ 123456"),
            row(
                "Flight",
                "SQ 638 Singapore (SIN) – Tokyo Narita (NRT), 2026-10-31"
            ),
            row(
                "Last update",
                "2026-10-31 22:10 local time — trace sent to Singapore (SIN)"
            )
        ])
    }

    #[test]
    fn the_result_pages_rows_are_read_by_their_cells() {
        let page = read(json!({"rows": result_rows(),
            "notes": ["Bags are usually found within a few days."]}));
        assert_eq!(
            trace_of(&page, "NRTSQ12345"),
            Some(Trace {
                file_ref: "NRTSQ12345".into(),
                status: "Tracing continues — your bag has not been located yet".into(),
                last_update: "2026-10-31 22:10 local time — trace sent to Singapore (SIN)".into(),
                flight_date: NaiveDate::from_ymd_opt(2026, 10, 31),
                found: true,
            })
        );
        let none = trace_of(&read(json!({"rows": [], "notes": ["No file found for this reference. Check the reference on your Property Irregularity Report."]})), "NRTSQ99999").unwrap();
        assert_eq!(
            (none.status.as_str(), none.found),
            ("No file found for this reference", false)
        );
        assert!(trace_of(&read(json!({"rows": [], "notes": []})), "NRTSQ12345").is_none());
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
