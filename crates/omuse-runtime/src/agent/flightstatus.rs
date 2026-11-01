//! The flight-status machine behind the workflow tool `flight_status` (docs/json-tools.md, "Reading pages"): a flight's
//! scheduled and estimated times, read by code from the airline's own status page — a known site whose table code
//! reads by its HTML elements (the rows of the result table, their cells), never by a pattern over the page's text.
//! Today: Singapore Airlines (claims_real 2026-10-31: the delayed SQ 638's old time left out of the reply; the times
//! were found by a pattern over any page's prose before).
//!
//! The model passes the flight number and the date as JSON; code opens the airline's status page for them
//! (`browser_navigate`, prefix `fst_`), reads the table's rows (`browser_read_elements` with `cells`), takes the
//! departure and arrival column of the rows labelled "Scheduled", "Estimated" and "Status", and returns the times with
//! `must_say`. An airline with no known status page: `not_known` (the model reads the airline's page itself; its times
//! are then the model's).

use chrono::{NaiveDate, NaiveTime};
use omuse_common::util::get_str;
use serde_json::{json, Map, Value};

use super::driver::Step;
use super::tools::fn_;
use super::util::{sget, stored_call, tool_calls};
use super::workflow::{self, Checked};

/// The workflow tool.
pub const FLIGHT_STATUS: &str = "flight_status";
/// The machine's tools.
pub const TOOLS: [&str; 1] = [FLIGHT_STATUS];
/// The id prefix of the machine's code calls.
pub const PREFIX: &str = "fst_";
/// The element read that holds the status table's rows.
const ROWS: &str = "rows";

/// An airline whose status page code reads.
struct Airline {
    /// the IATA code of its flight numbers
    code: &'static str,
    name: &'static str,
    /// the status page for a flight ("SQ638") and a date (YYYY-MM-DD)
    page: fn(&str, &str) -> String,
    /// the result table's rows (a CSS selector); the first cell is the row's label, then departure and arrival
    rows_sel: &'static str,
}

fn sq_page(flight: &str, date: &str) -> String {
    format!("https://www.singaporeair.com/en_UK/sg/travel-info/flightstatus.html?flight={flight}&date={date}")
}

const AIRLINES: [Airline; 1] = [Airline {
    code: "SQ",
    name: "Singapore Airlines",
    page: sq_page,
    rows_sel: "#res tr",
}];

/// The catalog entry of the workflow tool.
pub fn tool_schemas() -> Vec<Value> {
    vec![fn_(
        FLIGHT_STATUS,
        "A flight's live status from the airline's own status page (today: Singapore Airlines, SQ flights): code opens the \
page for the flight and the date and reads the scheduled and estimated departure and arrival times and the status from \
its table, as JSON. Call it whenever the user asks whether a flight is delayed or when it leaves; write the reply from \
the result, with every fact in its must_say. A result not_known means OMuse has no reader for that airline's page: read \
the airline's status page yourself.",
        json!({
            "flight": {"type": "string", "description": "the flight number, e.g. SQ 638"},
            "date": {"type": "string", "description": "YYYY-MM-DD, the flight's departure date (resolve relative dates from today in the live state)"}
        }),
        &["flight", "date"],
    )]
}

/// "SQ 638" / "sq638" → "SQ638" when it has a flight number's format (2 letters or digits, 1–4 digits).
fn compact(flight: &str) -> Option<String> {
    let f: String = flight
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect::<String>()
        .to_uppercase();
    let (code, num) = (f.get(..2)?, f.get(2..)?);
    let ok = code.chars().all(|c| c.is_ascii_alphanumeric())
        && (1..=4).contains(&num.len())
        && num.chars().all(|c| c.is_ascii_digit());
    ok.then_some(f)
}

/// The arguments checked: the flight number's format, the date.
pub fn check(tool: &str, a: &Value) -> Checked {
    if tool != FLIGHT_STATUS {
        return Checked::Invalid(vec![format!("unknown tool {tool}")]);
    }
    let mut out = Map::new();
    let mut problems = Vec::new();
    match compact(&sget(a, "flight")) {
        Some(f) => {
            out.insert("flight".into(), json!(f));
        }
        None => problems.push("flight: a flight number such as SQ 638".into()),
    }
    match NaiveDate::parse_from_str(sget(a, "date").trim(), "%Y-%m-%d") {
        Ok(d) => {
            out.insert("date".into(), json!(d.format("%Y-%m-%d").to_string()));
        }
        Err(_) => problems.push("date: YYYY-MM-DD".into()),
    }
    if problems.is_empty() {
        Checked::Valid(Value::Object(out))
    } else {
        Checked::Invalid(problems)
    }
}

/// A cell's time (its first word, HH:MM) and what follows it ("01:00 (+1 day)" → 01:00, "(+1 day)").
fn time_of(cell: &str) -> Option<(String, String)> {
    let mut words = cell.split_whitespace();
    let t = NaiveTime::parse_from_str(words.next()?, "%H:%M").ok()?;
    Some((
        t.format("%H:%M").to_string(),
        words.collect::<Vec<_>>().join(" "),
    ))
}

/// The departure and arrival of the row labelled `label` (its first cell).
fn row<'a>(rows: &'a [(Vec<String>, Vec<String>)], label: &str) -> Option<&'a [String]> {
    rows.iter()
        .find(|(cells, _)| cells.first().is_some_and(|c| c.trim() == label))
        .map(|(cells, _)| &cells[1..])
}

fn leg(cells: Option<&[String]>, i: usize) -> Option<(String, String)> {
    cells.and_then(|c| c.get(i)).and_then(|c| time_of(c))
}

/// The machine's state as it needs it.
pub struct Machine {
    /// the machine's code calls in the transcript before this run's tool call (the ids go on)
    pub first: usize,
}

/// The first result of a code call named `name` in the run (its arguments and message).
fn result_of<'a>(t: &'a [Value], name: &str) -> Option<(Value, &'a Value)> {
    t.iter().enumerate().find_map(|(i, msg)| {
        tool_calls(msg).iter().find_map(|tc| {
            let (n, a) = stored_call(tc);
            let id = get_str(tc, "id");
            (n.ends_with(name) && id.starts_with(PREFIX))
                .then(|| {
                    t[i..]
                        .iter()
                        .find(|r| get_str(r, "role") == "tool" && get_str(r, "tool_call_id") == id)
                })
                .flatten()
                .map(|r| (a, r))
        })
    })
}

/// The next step: open the status page, read its table, then the times as the result.
pub fn next(_tool: &str, args: &Value, t: &[Value], m: &Machine) -> Step {
    let flight = sget(args, "flight");
    let date = sget(args, "date");
    let shown = format!(
        "{} {}",
        flight.get(..2).unwrap_or(""),
        flight.get(2..).unwrap_or("")
    );
    let Some(airline) = AIRLINES.iter().find(|a| flight.starts_with(a.code)) else {
        return Step::Done(json!({"status": "not_known", "flight": shown, "date": date,
            "reason": "OMuse has no reader for this airline's status page", "must_say": []}));
    };
    let url = (airline.page)(&flight, &date);
    let n = m.first
        + t.iter()
            .flat_map(tool_calls)
            .filter(|tc| get_str(tc, "id").starts_with(PREFIX))
            .count()
        + 1;
    if result_of(t, "browser_navigate").is_none() {
        return Step::Calls(vec![workflow::code_call(
            PREFIX,
            n,
            "browser_navigate",
            json!({"url": url}),
        )]);
    }
    let Some((_, read)) = result_of(t, "browser_read_elements") else {
        return Step::Calls(vec![workflow::code_call(
            PREFIX,
            n,
            "browser_read_elements",
            json!({"read": [{"name": ROWS, "sel": airline.rows_sel, "cells": true}]}),
        )]);
    };
    let rows = super::confirm::element_rows(read, ROWS);
    let (sched, est) = (row(&rows, "Scheduled"), row(&rows, "Estimated"));
    let status = row(&rows, "Status")
        .and_then(|c| c.first())
        .cloned()
        .unwrap_or_default();
    let base = json!({"flight": shown, "date": date, "airline": airline.name, "source": url});
    let Some((dep, _)) = leg(sched, 0) else {
        let mut v = base;
        v["status"] = json!("not_read");
        v["must_say"] = json!([format!(
            "{}'s status page did not show {shown} on {date} just now: check {url}",
            airline.name
        )]);
        return Step::Done(v);
    };
    let arr = leg(sched, 1);
    let (est_dep, est_arr) = (leg(est, 0), leg(est, 1));
    let both = |x: &Option<(String, String)>| {
        x.as_ref()
            .map(|(t, more)| {
                if more.is_empty() {
                    t.clone()
                } else {
                    format!("{t} {more}")
                }
            })
            .unwrap_or_default()
    };
    let mut v = base;
    v["status"] = json!("shown");
    v["scheduled"] = json!({"departure": dep, "arrival": both(&arr)});
    v["estimated"] = json!({"departure": both(&est_dep), "arrival": both(&est_arr)});
    v["flight_status"] = json!(status);
    let detail = if status.is_empty() {
        String::new()
    } else {
        format!(" ({status})")
    };
    let line = match &est_dep {
        Some((new, _)) if *new != dep => format!(
            "{shown} on {date}: scheduled departure {dep}, now estimated {new}{}{detail} — {}'s status page",
            est_arr
                .as_ref()
                .map(|_| format!(", arrival {}", both(&est_arr)))
                .unwrap_or_default(),
            airline.name
        ),
        _ => format!(
            "{shown} on {date}: departure {dep} as scheduled{detail} — {}'s status page",
            airline.name
        ),
    };
    v["must_say"] = json!([line, format!("source: {url}")]);
    Step::Done(v)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(args: &Value, answer: impl Fn(&str) -> String) -> (Vec<String>, Value) {
        let mut t: Vec<Value> = Vec::new();
        let mut names = Vec::new();
        for _ in 0..5 {
            match next(FLIGHT_STATUS, args, &t, &Machine { first: 0 }) {
                Step::Done(v) => return (names, v),
                Step::Calls(calls) => {
                    for c in calls {
                        assert!(c.id.starts_with(PREFIX));
                        names.push(c.name.clone());
                        t.push(json!({"role": "assistant", "content": "", "tool_calls": [{"id": c.id, "type": "function",
                            "function": {"name": c.name, "arguments": c.args.to_string()}}]}));
                        t.push(json!({"role": "tool", "tool_call_id": c.id, "content": answer(&c.name)}));
                    }
                }
            }
        }
        panic!("no result");
    }

    const TABLE: &str = r#"<untrusted_content source="web page https://www.singaporeair.com/x">
{"trust": "untrusted", "url": "https://www.singaporeair.com/x", "elements": {"rows": [
 {"cells": ["", "Departure — Singapore Changi T3", "Arrival — Tokyo Narita T1"], "links": []},
 {"cells": ["Scheduled", "09:25", "17:15"], "links": []},
 {"cells": ["Estimated", "17:10", "01:00 (+1 day)"], "links": []},
 {"cells": ["Status", "Delayed 7 h 45 min — late arrival of the incoming aircraft"], "links": []}]}}
</untrusted_content>"#;

    #[test]
    fn a_delayed_flights_times_come_from_the_status_tables_cells() {
        let args = match check(
            FLIGHT_STATUS,
            &json!({"flight": "sq 638", "date": "2026-11-01"}),
        ) {
            Checked::Valid(a) => a,
            other => panic!("{other:?}"),
        };
        let (calls, v) = run(&args, |name| match name {
            "browser_read_elements" => TABLE.into(),
            _ => "{\"url\": \"https://www.singaporeair.com/x\"}".into(),
        });
        assert_eq!(calls, ["browser_navigate", "browser_read_elements"]);
        assert_eq!(v["status"], json!("shown"));
        assert_eq!(v["scheduled"]["departure"], json!("09:25"));
        assert_eq!(v["estimated"]["departure"], json!("17:10"));
        assert_eq!(v["estimated"]["arrival"], json!("01:00 (+1 day)"));
        let must = v["must_say"][0].as_str().unwrap();
        assert!(
            must.contains("scheduled departure 09:25, now estimated 17:10"),
            "{must}"
        );
        assert!(must.contains("Delayed 7 h 45 min"), "{must}");
    }

    #[test]
    fn a_page_without_the_table_or_an_unknown_airline_gives_no_times() {
        let args = json!({"flight": "SQ638", "date": "2026-11-01"});
        let (_, v) = run(&args, |name| match name {
            "browser_read_elements" => "{\"elements\": {\"rows\": []}}".into(),
            _ => "{}".into(),
        });
        assert_eq!(v["status"], json!("not_read"));
        let (calls, v) = run(&json!({"flight": "CX543", "date": "2026-11-01"}), |_| {
            "{}".into()
        });
        assert!(calls.is_empty());
        assert_eq!(v["status"], json!("not_known"));
        assert!(matches!(
            check(
                FLIGHT_STATUS,
                &json!({"flight": "delayed", "date": "2026-11-01"})
            ),
            Checked::Invalid(_)
        ));
    }
}
