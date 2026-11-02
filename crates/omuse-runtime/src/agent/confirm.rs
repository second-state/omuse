//! The confirmation reference after the user's hand-back, as data (docs/json-tools.md, "Facts the reply must carry").
//!
//! * **A known site** (`claims::SPECS`, `driver::CARDS`, APDT): its code machine reads the
//!   confirmation page's reference element by the site's fixed selector (`browser_read_elements`, [`read_args`]),
//!   checks the value's format (`outcome::reference_in`) and puts it in its JSON result; a page without it is
//!   `not_confirmed`. No pattern is ever tried over the page's text.
//! * **A site no code knows** (the `browser-use` skill): the reference is the model's. After the hand-back the model
//!   reads the confirmation page (natural language) and passes `confirm_reference{reference, label}` as JSON. Code
//!   validates that JSON — the value is on a page read after the hand-back (an exact substring check, as
//!   `mail_results_to`), the hand-back was not a code machine's — and records it as model-read.
//!
//! [`confirmed`] is the run's newest confirmed reference with its source, for the outcome check and the ledger;
//! [`fact`] is the model-read one the reply must carry (`hints`).

use omuse_common::util::get_str;
use serde_json::{json, Value};

use super::guards::{TAKEOVER_DONE_EN, TAKEOVER_DONE_MARK};
use super::tools::fn_;
use super::util::{content_of, sget, stored_call, tool_calls};
use super::workflow::{RESULT_PREFIX, RESULT_SUFFIX};
use crate::outcome::{Confirmed, KNOWN_PAGE, MODEL_READ};

/// The tool.
pub const CONFIRM_REFERENCE: &str = "confirm_reference";
/// The longest reference (or label) taken.
const MAX_LEN: usize = 40;
/// The id prefixes of the code machines whose hand-back pages are known sites (their reference is code's).
const MACHINE_PREFIXES: [&str; 3] = ["drv_", "clm_", "apd_"];

/// The catalog entry.
pub fn schema() -> Value {
    fn_(
        CONFIRM_REFERENCE,
        "Record the confirmation reference of a page you filled with the browser-use skill (a site with no form \
function), after the user pressed Submit / Pay and handed the browser back: read the confirmation page \
(browser_snapshot) and pass the order / booking / application number exactly as the page shows it, with the label the \
page puts before it. Not for a known site's workflow (its JSON result has the reference). Code appends the reference (its show) to the reply.",
        json!({"reference": {"type": "string", "description": "the reference exactly as the confirmation page shows it, e.g. AB-123456"},
               "label": {"type": "string", "description": "the page's label before it, e.g. \"Order number\" or \"Booking reference\""}}),
        &["reference"],
    )
}

/// The name of the element read that holds a confirmation reference.
pub const REFERENCE: &str = "reference";

/// The arguments of `browser_read_elements` for a known site's reference element (its fixed CSS selector).
pub fn read_args(sel: &str) -> Value {
    json!({"read": [{"name": REFERENCE, "sel": sel}]})
}

/// The attribute a snapshot gives each element it lists: its ref (`e12`), as `browser_click` / `browser_fill_secret` /
/// `browser_upload` take it.
pub const REF_ATTR: &str = "data-persona-ref";

/// The arguments of `browser_read_elements` for the refs of a known page's inputs by their fixed selectors (`(name,
/// selector)`; hidden inputs too), read after a snapshot gave the page's elements their refs.
pub fn refs_args(items: &[(&str, &str)]) -> Value {
    let read: Vec<Value> = items
        .iter()
        .map(|(name, sel)| json!({"name": name, "sel": sel, "attr": REF_ATTR, "hidden": true}))
        .collect();
    json!({"read": read})
}

/// The first non-empty value a `browser_read_elements` result holds under `name`.
pub fn first_value(m: &Value, name: &str) -> Option<String> {
    element_values(m, name)
        .into_iter()
        .find(|v| !v.trim().is_empty())
}

/// The `elements` object of a `browser_read_elements` result (the JSON inside its untrusted-content wrapper).
fn elements_of(m: &Value) -> Value {
    let c = content_of(m);
    let v: Value = match (c.find('{'), c.rfind('}')) {
        (Some(i), Some(j)) => serde_json::from_str(&c[i..=j]).unwrap_or(Value::Null),
        _ => Value::Null,
    };
    v["elements"].clone()
}

fn strings(x: &Value) -> Vec<String> {
    x.as_array()
        .map(|a| {
            a.iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

/// The values a `browser_read_elements` result holds under `name` (strings only; a row read with `cells` is skipped).
pub fn element_values(m: &Value, name: &str) -> Vec<String> {
    strings(&elements_of(m)[name])
}

/// The rows a `browser_read_elements` result holds under `name` (read with `cells`): each row's cells and links.
pub fn element_rows(m: &Value, name: &str) -> Vec<(Vec<String>, Vec<String>)> {
    elements_of(m)[name]
        .as_array()
        .map(|a| {
            a.iter()
                .map(|r| (strings(&r["cells"]), strings(&r["links"])))
                .collect()
        })
        .unwrap_or_default()
}

/// The matches a `browser_read_elements` result holds under `name` (read with `fields`): each match's fields, as
/// strings (a field the page did not have is `""`).
pub fn element_fields(m: &Value, name: &str) -> Vec<std::collections::BTreeMap<String, String>> {
    elements_of(m)[name]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(Value::as_object)
                .map(|o| {
                    o.iter()
                        .map(|(k, v)| (k.clone(), v.as_str().unwrap_or_default().to_string()))
                        .collect()
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The position of the latest tool message that says the user handed the browser back, and its call's id.
fn hand_back(transcript: &[Value]) -> Option<(usize, String)> {
    transcript.iter().enumerate().rev().find_map(|(i, m)| {
        let c = content_of(m);
        (get_str(m, "role") == "tool"
            && (c.contains(TAKEOVER_DONE_MARK) || c.contains(TAKEOVER_DONE_EN)))
        .then(|| (i, get_str(m, "tool_call_id")))
    })
}

/// The name of the call a tool message answers.
fn call_name(transcript: &[Value], id: &str) -> String {
    transcript
        .iter()
        .flat_map(tool_calls)
        .find(|tc| get_str(tc, "id") == id)
        .map(|tc| stored_call(tc).0)
        .unwrap_or_default()
}

fn clean(s: &str) -> String {
    s.trim()
        .trim_matches(|c: char| c == '*' || c == '"' || c == '\'' || c == '`')
        .trim()
        .to_string()
}

/// `confirm_reference{reference, label}`: the model's JSON checked in code. `recorded` with the reference to `show`, or `invalid`
/// with the problems the model can fix.
pub fn record(transcript: &[Value], args: &Value) -> Value {
    let reference = clean(&sget(args, "reference"));
    let label = match clean(&sget(args, "label")).trim_end_matches([':', '：']) {
        "" => "Reference".to_string(),
        l => l.chars().take(MAX_LEN).collect(),
    };
    let invalid = |why: &str| json!({"status": "invalid", "problems": [why]});
    if reference.is_empty()
        || reference.chars().count() > MAX_LEN
        || !reference.chars().any(|c| c.is_ascii_alphanumeric())
    {
        return invalid("reference: the order / booking / application number as the page shows it (up to 40 characters)");
    }
    let Some((back, back_id)) = hand_back(transcript) else {
        return invalid("no hand-back in this run: a reference is recorded only from the confirmation page after the user handed the browser back");
    };
    if MACHINE_PREFIXES.iter().any(|p| back_id.starts_with(p)) {
        return invalid("this page is a known site's: its workflow reads the reference at the page's fixed label (its JSON result)");
    }
    let shown = transcript[back + 1..]
        .iter()
        .filter(|m| get_str(m, "role") == "tool")
        .any(|m| content_of(m).contains(&reference));
    if !shown {
        return invalid("the reference is not on any page read after the hand-back: read the confirmation page (browser_snapshot) and pass the reference exactly as it shows");
    }
    // the label is the page's own (no translation); code appends the reference to the reply (agent/show.rs)
    json!({"status": "recorded", "reference": reference, "label": label, "source": MODEL_READ,
           "show": [super::show::reference(&label, "", &reference)], "hints": []})
}

/// The reference in a code machine's JSON result: read at a known site's fixed label (`reference`, a bought policy's
/// `policy_no`); none for a result that is not confirmed.
fn machine_reference(v: &Value) -> Option<String> {
    let status = get_str(v, "status");
    if matches!(status.as_str(), "not_confirmed" | "error" | "") {
        return None;
    }
    [
        v.get("reference"),
        (status == "bought").then(|| v.get("policy_no")).flatten(),
    ]
    .into_iter()
    .flatten()
    .filter_map(Value::as_str)
    .map(str::trim)
    .find(|r| !r.is_empty())
    .map(str::to_string)
}

/// The run's newest confirmed reference: a code machine's result (`known_page`) or a recorded `confirm_reference`
/// (`model_read`). Only structured JSON is read.
pub fn confirmed(transcript: &[Value]) -> Option<Confirmed> {
    transcript
        .iter()
        .rev()
        .filter(|m| get_str(m, "role") == "tool")
        .find_map(|m| {
            let id = get_str(m, "tool_call_id");
            let name = call_name(transcript, &id);
            let v: Value = serde_json::from_str(&content_of(m)).ok()?;
            if id.starts_with(RESULT_PREFIX) && name.ends_with(RESULT_SUFFIX) {
                machine_reference(&v).map(|reference| Confirmed {
                    reference,
                    source: KNOWN_PAGE,
                })
            } else if name == CONFIRM_REFERENCE && get_str(&v, "status") == "recorded" {
                Some(Confirmed {
                    reference: get_str(&v, "reference"),
                    source: MODEL_READ,
                })
            } else {
                None
            }
        })
}

/// The model-read reference recorded after the latest hand-back, as the fact the reply must carry ("<label>: <code>").
pub fn fact(transcript: &[Value]) -> Option<String> {
    let (back, _) = hand_back(transcript)?;
    transcript[back + 1..]
        .iter()
        .rev()
        .filter(|m| get_str(m, "role") == "tool")
        .filter(|m| call_name(transcript, &get_str(m, "tool_call_id")) == CONFIRM_REFERENCE)
        .find_map(|m| {
            let v: Value = serde_json::from_str(&content_of(m)).ok()?;
            (get_str(&v, "status") == "recorded")
                .then(|| format!("{}: {}", get_str(&v, "label"), get_str(&v, "reference")))
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn call(id: &str, name: &str, args: Value) -> Value {
        json!({"role": "assistant", "content": "", "tool_calls": [{"id": id, "type": "function",
               "function": {"name": name, "arguments": args.to_string()}}]})
    }

    fn tool(id: &str, content: &str) -> Value {
        json!({"role": "tool", "tool_call_id": id, "content": content})
    }

    const BACK: &str = "用户已完成接管 Takeover finished: the user handed the browser back.";
    const PAGE: &str = "<untrusted_content source=\"web page https://shop.example/thanks\">\n{\"url\": \"https://shop.example/thanks\", \"snapshot\": \"# Thank you\\n  Your order AB-778812 is confirmed\"}\n</untrusted_content>";

    fn unknown_site_run() -> Vec<Value> {
        vec![
            json!({"role": "user", "content": "buy the lamp on shop.example"}),
            call(
                "c1",
                "browser_request_takeover",
                json!({"reason": "press Pay"}),
            ),
            tool("c1", BACK),
            call("c2", "browser_snapshot", json!({})),
            tool("c2", PAGE),
        ]
    }

    #[test]
    fn an_unknown_sites_reference_is_the_models_json_checked_against_the_page() {
        let mut t = unknown_site_run();
        let r = record(
            &t,
            &json!({"reference": "AB-778812", "label": "Your order"}),
        );
        assert_eq!(r["status"], json!("recorded"));
        assert_eq!(r["source"], json!(MODEL_READ));
        assert_eq!(
            super::super::show::lines(&r, false),
            vec!["- Your order: **AB-778812**"]
        );
        t.push(call(
            "c3",
            CONFIRM_REFERENCE,
            json!({"reference": "AB-778812"}),
        ));
        t.push(tool("c3", &r.to_string()));
        assert_eq!(fact(&t).as_deref(), Some("Your order: AB-778812"));
        assert_eq!(
            confirmed(&t),
            Some(Confirmed {
                reference: "AB-778812".into(),
                source: MODEL_READ
            })
        );
    }

    #[test]
    fn a_reference_no_page_after_the_hand_back_shows_is_refused() {
        let t = unknown_site_run();
        let r = record(&t, &json!({"reference": "ZZ-000001"}));
        assert_eq!(r["status"], json!("invalid"));
        // before any hand-back
        let r = record(&t[..2], &json!({"reference": "AB-778812"}));
        assert_eq!(r["status"], json!("invalid"));
        assert_eq!(
            record(&t, &json!({"reference": " "}))["status"],
            json!("invalid")
        );
        // nothing recorded: no fact, nothing confirmed
        assert_eq!(fact(&t), None);
        assert_eq!(confirmed(&t), None);
    }

    #[test]
    fn a_known_sites_hand_back_is_its_machines_and_its_result_is_the_reference() {
        let mut t = vec![
            json!({"role": "user", "content": "fill my SG arrival card"}),
            call(
                "drv_9",
                "browser_request_takeover",
                json!({"reason": "press Submit"}),
            ),
            tool("drv_9", BACK),
            call("drv_10", "browser_snapshot", json!({})),
            tool("drv_10", "{\"snapshot\": \"  DE Number: DE202611020123\"}"),
        ];
        assert_eq!(
            record(&t, &json!({"reference": "DE202611020123"}))["status"],
            json!("invalid")
        );
        t.push(call("res_1", "arrival_card_fill_result", json!({})));
        t.push(tool(
            "res_1",
            &json!({"status": "submitted", "reference": "DE202611020123"}).to_string(),
        ));
        assert_eq!(
            confirmed(&t),
            Some(Confirmed {
                reference: "DE202611020123".into(),
                source: KNOWN_PAGE
            })
        );
        // a machine's not_confirmed result confirms nothing
        let n = t.len();
        t[n - 1] = tool("res_1", &json!({"status": "not_confirmed"}).to_string());
        assert_eq!(confirmed(&t), None);
        // a bought policy's number
        t[n - 1] = tool(
            "res_1",
            &json!({"status": "bought", "policy_no": "TI-SG-2026-70412"}).to_string(),
        );
        assert_eq!(
            confirmed(&t).map(|c| c.reference).as_deref(),
            Some("TI-SG-2026-70412")
        );
        // a claim result's policy number (the user's existing policy, from mail) is no confirmation
        t[n - 1] = tool(
            "res_1",
            &json!({"status": "handed_over", "policy_no": "TI-SG-2026-55123"}).to_string(),
        );
        assert_eq!(confirmed(&t), None);
    }
}
