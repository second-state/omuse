//! The delayed-bag lookup's known page: WorldTracer's passenger site, mybag.aero (the old public tracing page,
//! wtrweb.worldtracer.aero/WorldTracerWeb/bagtrace.html, answers 404 since 2026). The `bag_trace` workflow tool
//! (`super::claims`) takes the PIR's file reference as its JSON argument (the model reads it in the user's words or the
//! PIR email); code opens the airline's "Manage your delayed baggage report" page (the airline from the reference's
//! letters 4–5, its slug from [`AIRLINES`]), fills it by its form function (`crate::forms`, fixed ids: the reference
//! and the family name), presses "LOG IN" (a lookup changes nothing) and reads the answer by its HTML elements
//! (`browser_read_elements`: the error box `#error-body-head` / `#error-body-text`, or the sign-in form gone —
//! docs/json-tools.md "Reading pages"). An airline that does not use mybag.aero is said so, with no page opened.

use std::sync::LazyLock;

use regex::Regex;
use serde_json::Value;

use super::util::content_of;

/// The skill whose runs call `bag_trace`.
pub const SKILL: &str = "baggage-delayed-lost";
/// WorldTracer's passenger site (its landing page: reporting and tracking, for the airlines that use it).
pub const TRACE_URL: &str = "https://mybag.aero/baggage/#/pax";
pub const TRACE_HOST: &str = "mybag.aero";

/// The airlines on mybag.aero: (IATA code, the site's slug), from the site's own configuration
/// (https://mybag.aero/baggage/assets/static/airline-configuration.json, `AIRLINE_CODE_TO_NAME_MAP`, 2026-11-02).
/// Singapore Airlines, Cathay Pacific and United are not on it: their bags are traced on their own sites.
pub const AIRLINES: &[(&str, &str)] = &[
    ("3M", "silver"),
    ("4Y", "ewdiscover"),
    ("6B", "tui6b"),
    ("AD", "voeazul"),
    ("AY", "finnair"),
    ("AZ", "itaairways"),
    ("BA", "britishairways"),
    ("BT", "airbaltic"),
    ("BY", "tuiby"),
    ("CM", "copaair"),
    ("DE", "condor"),
    ("DI", "marabu"),
    ("DY", "norwegian"),
    ("E2", "airhaifa"),
    ("EI", "aerlingus"),
    ("EN", "airdolomiti"),
    ("ET", "ethiopianairlines"),
    ("EW", "eurowings"),
    ("FI", "icelandair"),
    ("FJ", "fijiairways"),
    ("FR", "ryanair"),
    ("FZ", "flydubai"),
    ("G3", "gol"),
    ("GR", "aurigny"),
    ("HV", "transaviaairlines"),
    ("JL", "jal"),
    ("KM", "malta"),
    ("KX", "caymanairways"),
    ("LA", "latam"),
    ("LH", "lufthansa"),
    ("LL", "level"),
    ("LM", "loganair"),
    ("LO", "lot"),
    ("LS", "jet2com"),
    ("LX", "swiss"),
    ("LY", "elal"),
    ("ME", "mea"),
    ("MH", "mab"),
    ("N0", "flynorse"),
    ("NH", "ana"),
    ("NZ", "airnz"),
    ("OR", "tuior"),
    ("OS", "austrian"),
    ("PC", "pegasusairlines"),
    ("PD", "porter"),
    ("PR", "philippineairlines"),
    ("QF", "qantas"),
    ("QS", "smartwingsgroup"),
    ("RC", "atlanticairways"),
    ("RJ", "rj"),
    ("SB", "aircalin"),
    ("SK", "sas"),
    ("SN", "brusselsairlines"),
    ("SP", "sata"),
    ("SS", "corsairairlines"),
    ("SV", "saudia"),
    ("TB", "tuitb"),
    ("TG", "thai"),
    ("TK", "turkishairlines"),
    ("TO", "transaviafrance"),
    ("TP", "tapairportugal"),
    ("TS", "airtransat"),
    ("U2", "easyjet"),
    ("UO", "hkexpress"),
    ("UX", "aireuropa"),
    ("V7", "volotea"),
    ("VB", "viva"),
    ("VJ", "vietjet"),
    ("VL", "lufthansacity"),
    ("VN", "vna"),
    ("VS", "virginatlantic"),
    ("VY", "vueling"),
    ("W6", "wizzair"),
    ("WF", "wideroe"),
    ("WK", "edelweiss"),
    ("WS", "westjet"),
    ("X3", "tuifly"),
    ("XQ", "sunexpress"),
    ("Y4", "volaris"),
];

/// The shape of a WorldTracer file reference: the station (3 letters), the airline (2), five digits — "NRTNH12345".
static FILE_REF: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^([A-Z]{3})[ -]?([A-Z][A-Z0-9]|[0-9][A-Z])[ -]?(\d{5})$").unwrap()
});

/// A file reference argument, normalised without spaces ("nrt nh 12345" → "NRTNH12345"); `None` when it is not one.
pub fn file_ref_of(value: &str) -> Option<String> {
    FILE_REF
        .captures(&value.trim().to_uppercase())
        .map(|c| format!("{}{}{}", &c[1], &c[2], &c[3]))
}

/// The airline of a (normalised) file reference: its letters 4–5 ("NRTNH12345" → "NH").
pub fn airline_of(file_ref: &str) -> String {
    file_ref.chars().skip(3).take(2).collect()
}

/// The airline's "Manage your delayed baggage report" page on mybag.aero; `None` when the airline is not on the site.
pub fn manage_url(airline: &str) -> Option<String> {
    AIRLINES
        .iter()
        .find(|(code, _)| code.eq_ignore_ascii_case(airline))
        .map(|(_, slug)| {
            format!("https://mybag.aero/baggage/#/pax/{slug}/en-gb/delayed/manage-bag")
        })
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
    /// the site's own words: its error message, or empty when the report opened
    pub status: String,
    /// WorldTracer opened the report (false: it found no record for the reference and the name)
    pub found: bool,
}

/// The element reads of the page after "LOG IN": the error box's heading and text, and the sign-in form's reference box.
const ERROR_HEAD: &str = "error_head";
const ERROR_TEXT: &str = "error_text";
const LOGIN: &str = "login";

/// The arguments of `browser_read_elements` for the page after "LOG IN".
pub fn read_args() -> Value {
    serde_json::json!({"read": [{"name": ERROR_HEAD, "sel": "#error-body-head"},
                                {"name": ERROR_TEXT, "sel": "#error-body-text"},
                                {"name": LOGIN, "sel": "#mngRptRefNoLTxt", "attr": "id"}]})
}

/// WorldTracer's answer from the elements read (`read`, a `browser_read_elements` result): the error box ("There was
/// a problem in finding your details" — "No record was found with that reference number and family name
/// combination…") as a file it does not know; the sign-in form gone as the report opened. `None` when the page shows
/// neither (no answer yet, or a check such as a CAPTCHA).
pub fn trace_of(read: &Value, file_ref: &str) -> Option<Trace> {
    let first = |name: &str| {
        super::confirm::element_values(read, name)
            .into_iter()
            .find(|v| !v.trim().is_empty())
    };
    if let Some(text) = first(ERROR_TEXT).or_else(|| first(ERROR_HEAD)) {
        return Some(Trace {
            file_ref: file_ref.to_string(),
            status: text.trim().trim_end_matches(" or").to_string(),
            found: false,
        });
    }
    if first(LOGIN).is_none() {
        return Some(Trace {
            file_ref: file_ref.to_string(),
            status: String::new(),
            found: true,
        });
    }
    None
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

    #[test]
    fn the_answer_is_read_by_its_elements() {
        // the real page's error box for a reference WorldTracer does not know (mybag.aero, 2026-11-02)
        let none = trace_of(&read(json!({"error_head": ["There was a problem in finding your details"],
            "error_text": ["No record was found with that reference number and family name combination. Check the values entered and try again or"],
            "login": ["mngRptRefNoLTxt"]})), "NRTNH12345").unwrap();
        assert!(!none.found);
        assert_eq!(
            none.status,
            "No record was found with that reference number and family name combination. Check the values entered and try again"
        );
        // the sign-in form gone: the report opened
        let open = trace_of(
            &read(json!({"error_head": [], "error_text": [], "login": []})),
            "NRTNH12345",
        )
        .unwrap();
        assert!(open.found);
        // still on the sign-in form, no error: no answer
        assert!(trace_of(
            &read(json!({"error_head": [], "error_text": [], "login": ["mngRptRefNoLTxt"]})),
            "NRTNH12345"
        )
        .is_none());
    }

    #[test]
    fn the_airline_comes_from_the_reference_and_its_page_from_the_table() {
        assert_eq!(airline_of("NRTNH12345"), "NH");
        assert_eq!(
            manage_url("NH").as_deref(),
            Some("https://mybag.aero/baggage/#/pax/ana/en-gb/delayed/manage-bag")
        );
        assert_eq!(
            manage_url("jl").as_deref(),
            Some("https://mybag.aero/baggage/#/pax/jal/en-gb/delayed/manage-bag")
        );
        // Singapore Airlines is not on mybag.aero
        assert!(manage_url("SQ").is_none());
    }

    #[test]
    fn a_file_reference_argument_is_checked_by_its_shape() {
        assert_eq!(file_ref_of("NRT NH 12345").as_deref(), Some("NRTNH12345"));
        assert_eq!(file_ref_of("nrtsq12345").as_deref(), Some("NRTSQ12345"));
        assert_eq!(file_ref_of("SIN-3K-00123").as_deref(), Some("SIN3K00123"));
        assert!(file_ref_of("7HK2LM").is_none());
        assert!(file_ref_of("NRTSQ1234").is_none());
    }
}
