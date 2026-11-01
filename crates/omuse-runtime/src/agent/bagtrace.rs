//! The delayed-bag lookup, in code (claims_real 2026-11-01, twice: "WorldTracer's tracking form was available, but I
//! couldn't submit the reference and surname from this bot"). Once the `baggage-delayed-lost` skill is loaded and the
//! PIR's file reference is known — from the user's words, else from a baggage email the run read — the claims machine
//! (`super::claims`, `Flow::BagTrace`) opens WorldTracer's tracing page, fills it by its form function
//! (`crate::forms`, fixed ids: the reference and the family name) and presses Track (a lookup changes nothing), then
//! takes one snapshot of the result. The status is read from that page's fixed rows (Status, Last update) and the
//! final answer states it ([`with_bag_status`]). Everything else in the skill (the drafts, the watch, the advice) stays
//! with the model, which reads the result page in its transcript.

use std::sync::LazyLock;

use omuse_common::util::get_str;
use regex::Regex;
use serde_json::{json, Value};

use super::driver::{snapshot_text, stopped, user_words};
use super::util::{content_of, sget, stored_call, tool_calls};
use crate::llm::ToolCall;

/// The skill whose runs get the lookup.
pub const SKILL: &str = "baggage-delayed-lost";
/// WorldTracer's public tracing page (the airlines' "track your bag" links).
pub const TRACE_URL: &str = "https://wtrweb.worldtracer.aero/WorldTracerWeb/bagtrace.html";
pub const TRACE_HOST: &str = "wtrweb.worldtracer.aero";
/// The id prefix of the lookup's calls (a claims-machine call: `clm_`).
const ID: &str = "clm_bag";

/// A WorldTracer file reference: the station (3 letters), the airline (2), five digits — "NRTSQ12345".
static FILE_REF: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\b([A-Z]{3})[ -]?([A-Z][A-Z0-9]|[0-9][A-Z])[ -]?(\d{5})\b").unwrap()
});
/// An email about a bag (only those are read for the reference).
static BAG_MAIL: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\bbag(gage)?s?\b|luggage|\bPIR\b|property irregularity|file reference|行李")
        .unwrap()
});
/// The family name as a PIR writes the passenger ("TAN/WEIMING") or as the user names it.
static PIR_NAME: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\b([A-Z][A-Z'-]{1,30})/[A-Z][A-Z ]{1,40}\b").unwrap());
static SAID_NAME: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)\b(?:family name|surname|last name)\s*(?:is|:)?\s*([A-Za-z][A-Za-z'-]{1,30})\b",
    )
    .unwrap()
});

/// The file reference in `text`, without spaces.
pub fn file_ref_in(text: &str) -> Option<String> {
    FILE_REF
        .captures(text)
        .map(|c| format!("{}{}{}", &c[1], &c[2], &c[3]))
}

/// The contents of the run's mail results (searches and fetched emails) that are about a bag.
fn bag_mail(transcript: &[Value]) -> Vec<String> {
    let mut out = Vec::new();
    for tc in transcript.iter().flat_map(tool_calls) {
        let (name, _) = stored_call(tc);
        if name != "gmail_search" && name != "gmail_get_message" {
            continue;
        }
        let id = get_str(tc, "id");
        if let Some(m) = transcript
            .iter()
            .find(|m| get_str(m, "role") == "tool" && get_str(m, "tool_call_id") == id)
        {
            let c = content_of(m);
            if BAG_MAIL.is_match(&c) {
                out.push(c);
            }
        }
    }
    out
}

/// The PIR's file reference: the user's words first, then a baggage email the run read.
pub fn file_ref(transcript: &[Value]) -> Option<String> {
    user_words(transcript)
        .iter()
        .rev()
        .find_map(|t| file_ref_in(t))
        .or_else(|| bag_mail(transcript).iter().find_map(|t| file_ref_in(t)))
}

/// The family name the user or the PIR gives; empty when neither does (the form function then takes the profile's).
pub fn family_name(transcript: &[Value]) -> String {
    let words = user_words(transcript);
    words
        .iter()
        .rev()
        .find_map(|t| SAID_NAME.captures(t).map(|c| c[1].to_uppercase()))
        .or_else(|| {
            bag_mail(transcript)
                .iter()
                .find_map(|t| PIR_NAME.captures(t).map(|c| c[1].to_string()))
        })
        .unwrap_or_default()
}

/// Whether the run loaded the baggage skill (a whole skill, not one of its files).
fn loaded(transcript: &[Value]) -> bool {
    transcript.iter().flat_map(tool_calls).any(|tc| {
        let (name, args) = stored_call(tc);
        name == "load_skill"
            && sget(&args, "name") == SKILL
            && sget(&args, "file").trim().is_empty()
    })
}

/// The lookup's own call to `tool`, if it ran: its arguments and its result.
fn own_call<'a>(transcript: &'a [Value], tool: &str) -> Option<(Value, &'a Value)> {
    let (id, args) = transcript.iter().flat_map(tool_calls).find_map(|tc| {
        let id = get_str(tc, "id");
        let (name, args) = stored_call(tc);
        (id.starts_with(ID) && name == tool).then_some((id, args))
    })?;
    let m = transcript
        .iter()
        .find(|m| get_str(m, "role") == "tool" && get_str(m, "tool_call_id") == id)?;
    Some((args, m))
}

fn own<'a>(transcript: &'a [Value], tool: &str) -> Option<&'a Value> {
    own_call(transcript, tool).map(|(_, m)| m)
}

fn failed(m: &Value) -> bool {
    let c = content_of(m);
    let c = c.trim_start();
    c.starts_with("ERROR") || c.starts_with("DENIED") || c.starts_with("BLOCKED")
}

/// The lookup's next call, or `None` when it is done, cannot start (no reference) or a step failed (each step runs
/// once; the model carries on). `n` numbers the call.
pub fn next_call(transcript: &[Value], n: usize) -> Option<ToolCall> {
    if !loaded(transcript) {
        return None;
    }
    let file_ref = file_ref(transcript)?;
    let call = |name: &str, args: Value| ToolCall {
        id: format!("{ID}{n}"),
        name: name.into(),
        args,
    };
    let Some(nav) = own(transcript, "browser_navigate") else {
        return Some(call("browser_navigate", json!({"url": TRACE_URL})));
    };
    if failed(nav) {
        return None;
    }
    let Some(fill) = own(transcript, "form_fill") else {
        let mut args = json!({"file_ref": file_ref});
        let family = family_name(transcript);
        if !family.is_empty() {
            args["family_name"] = json!(family);
        }
        return Some(call("form_fill", args));
    };
    if failed(fill) || stopped(fill).is_some() {
        return None;
    }
    if own(transcript, "browser_snapshot").is_none() {
        return Some(call("browser_snapshot", json!({"max_chars": 6000})));
    }
    None
}

/// Whether the lookup has a step left to run (the claims machine drives the run until it has none).
pub fn pending(transcript: &[Value]) -> bool {
    next_call(transcript, 0).is_some()
}

/// What WorldTracer said about the file.
#[derive(Debug, Clone, PartialEq)]
pub struct Trace {
    pub file_ref: String,
    pub status: String,
    pub last_update: String,
}

static STATUS: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?m)^\s*Status\s*[:|]?\s+(\S.*?)\s*$").unwrap());
static LAST_UPDATE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?m)^\s*Last update\s*[:|]?\s+(\S.*?)\s*$").unwrap());
static NO_FILE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)no file found[^\n.]*").unwrap());

/// The status on the lookup's result page (its Status and Last update rows, or "No file found"); `None` before the
/// lookup ran or when the page shows neither.
pub fn trace(transcript: &[Value]) -> Option<Trace> {
    let snap = snapshot_text(own(transcript, "browser_snapshot")?);
    let file_ref = own_call(transcript, "form_fill")
        .map(|(a, _)| sget(&a, "file_ref"))
        .unwrap_or_default();
    let status = match STATUS.captures(&snap) {
        Some(c) => c[1].to_string(),
        None => NO_FILE.find(&snap)?.as_str().trim().to_string(),
    };
    let last_update = LAST_UPDATE
        .captures(&snap)
        .map(|c| c[1].to_string())
        .unwrap_or_default();
    Some(Trace {
        file_ref,
        status,
        last_update,
    })
}

/// A sentence saying the status could not be had ("I couldn't retrieve a live tracking status …", "So I don't yet
/// know where the bag is.").
static NOT_RETRIEVED: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)[^.!?\n*]*(?:\b(?:couldn[’']?t|could not|unable to|(?:was|wasn[’']?t|was not) (?:not )?able to)\b[^.!?\n*]*\b(?:status|track(?:ing)?|trac(?:e|ing)|WorldTracer|where (?:the|your) bag)\b|\bdon[’']?t (?:yet )?know where (?:the|your) bag is)[^.!?\n*]*[.!?]?",
    )
    .unwrap()
});

/// `answer` stating WorldTracer's status of the bag when the lookup read one: a sentence saying it could not be had is
/// replaced by the status (the others like it are dropped), else the status is added when the answer does not give it.
pub fn with_bag_status(transcript: &[Value], answer: &str, zh: bool) -> String {
    let Some(t) = trace(transcript) else {
        return answer.to_string();
    };
    let update = if t.last_update.is_empty() {
        String::new()
    } else if zh {
        format!("最近更新：{}。", t.last_update)
    } else {
        format!(" Last update: {}.", t.last_update)
    };
    let status = t.status.trim_end_matches('.');
    let line = if zh {
        format!(
            "**行李状态（WorldTracer，文件 {}）：** {status}。{update}",
            t.file_ref
        )
    } else {
        format!(
            "**Your bag (WorldTracer, file {}):** {status}.{update}",
            t.file_ref
        )
    };
    // the status's lead words ("tracing continues") say the answer gives it
    let lead: String = status
        .split([' ', '—', '-', ','])
        .filter(|w| !w.is_empty())
        .take(2)
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase();
    let mut first = true;
    let mut out = NOT_RETRIEVED
        .replace_all(answer, |_: &regex::Captures| {
            if std::mem::take(&mut first) {
                line.clone()
            } else {
                String::new()
            }
        })
        .to_string();
    if !first {
        // a sentence went: its emphasis marks and blank lines with it
        out = out.replace("****", "");
        static BLANKS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\n{3,}").unwrap());
        out = BLANKS.replace_all(out.trim(), "\n\n").to_string();
        return out;
    }
    if !lead.is_empty() && out.to_lowercase().contains(&lead) {
        return out;
    }
    format!("{}\n\n{line}", out.trim_end())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn step(id: &str, name: &str, args: Value, result: &str) -> Vec<Value> {
        vec![
            json!({"role": "assistant", "content": "", "tool_calls": [{"id": id, "type": "function",
                   "function": {"name": name, "arguments": args.to_string()}}]}),
            json!({"role": "tool", "tool_call_id": id, "content": result}),
        ]
    }

    /// A browser result as Sentinel returns it: the page inside the untrusted JSON envelope.
    fn page(url: &str, snapshot: &str) -> String {
        format!(
            "<untrusted_content source=\"web page {url}\">\n{}\n</untrusted_content>",
            json!({"trust": "untrusted", "source": format!("web page {url}"), "url": url,
                   "title": "WorldTracer — Track your delayed baggage", "tabs": [], "snapshot": snapshot})
        )
    }

    const ASK: &str = "My checked bag didn't arrive in Tokyo on SQ 638 yesterday. I filed a report at Narita, file reference NRTSQ12345, and I've bought toiletries and a change of clothes for JPY 9,850 (I kept the receipts). What should I do? Check where my bag is, draft my expenses claim (don't send it), and keep an eye on the bag for me.";

    const FORM: &str = "# Track your delayed baggage\n[e1] textbox \"File reference\" placeholder=\"e.g. NRTSQ12345\" value=\"\"\n[e2] textbox \"Family name\" value=\"\"\n[e3] button \"Track\"\n  WorldTracer is operated for participating airlines.";
    const RESULT: &str = "# Track your delayed baggage\n[e1] textbox \"File reference\" value=\"NRTSQ12345\"\n[e2] textbox \"Family name\" value=\"MING\"\n[e3] button \"Track\"\n## File NRTSQ12345\n  Status Tracing continues — your bag has not been located yet\n  Bag Black hard-shell suitcase, tag SQ 123456\n  Flight SQ 638 Singapore (SIN) – Tokyo Narita (NRT), 2026-10-31\n  Last update 2026-10-31 22:10 local time — trace sent to Singapore (SIN)\n  Delivery address on file Hotel Ginza Grand, 1-1-1 Ginza, Chuo City, Tokyo\n  Bags are usually found within a few days. A bag not found within 21 days is considered lost: the airline then handles your claim for the contents.";

    fn start(ask: &str) -> Vec<Value> {
        let mut t = vec![
            json!({"role": "system", "content": ""}),
            json!({"role": "user", "content": ask}),
        ];
        t.extend(step(
            "c0",
            "load_skill",
            json!({"name": SKILL, "file": ""}),
            "---\nname: baggage-delayed-lost\n---\n# Baggage",
        ));
        t
    }

    /// The lookup run to its end with the stand-in's pages; the calls it made.
    fn run(t: &mut Vec<Value>, result: &str) -> Vec<ToolCall> {
        let mut calls = Vec::new();
        for n in 1..10 {
            let Some(c) = next_call(t, n) else { break };
            let out = match c.name.as_str() {
                "browser_navigate" => page(TRACE_URL, FORM),
                "form_fill" => "{\"url\": \"https://wtrweb.worldtracer.aero/bagtrace.html?ref=NRTSQ12345&name=MING\", \"filled\": [{\"op\": \"require\", \"field\": \"trace_form\", \"ok\": true}, {\"op\": \"text\", \"sel\": \"#ref\", \"field\": \"file_ref\", \"ok\": true}, {\"op\": \"text\", \"sel\": \"#name\", \"field\": \"family_name\", \"ok\": true}, {\"op\": \"button\", \"sel\": \"button\", \"field\": \"track\", \"ok\": true}], \"skipped\": []}".into(),
                "browser_snapshot" => page("https://wtrweb.worldtracer.aero/bagtrace.html?ref=NRTSQ12345&name=MING", result),
                _ => "{}".into(),
            };
            t.extend(step(&c.id, &c.name, c.args.clone(), &out));
            calls.push(c);
        }
        calls
    }

    #[test]
    fn the_bag_is_looked_up_on_worldtracer_by_code_with_the_file_reference() {
        use super::super::claims::{active, Flow};
        let mut t = start(ASK);
        // the claims machine drives the run while the lookup has a step left, then hands it back to the model
        assert_eq!(active(&t).map(|s| s.flow), Some(Flow::BagTrace));
        let calls = run(&mut t, RESULT);
        assert!(active(&t).is_none());
        let names: Vec<&str> = calls.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, ["browser_navigate", "form_fill", "browser_snapshot"]);
        assert_eq!(calls[0].args["url"], TRACE_URL);
        assert_eq!(calls[1].args["file_ref"], "NRTSQ12345");
        assert!(calls.iter().all(super::super::guards::code_call));
        assert!(!pending(&t));
        assert_eq!(
            trace(&t),
            Some(Trace {
                file_ref: "NRTSQ12345".into(),
                status: "Tracing continues — your bag has not been located yet".into(),
                last_update: "2026-10-31 22:10 local time — trace sent to Singapore (SIN)".into(),
            })
        );
    }

    #[test]
    fn the_answer_that_could_not_track_the_bag_gets_the_status_read_by_code() {
        // claims_real 2026-11-01 (g31, r32): the model's answer after its own fill was refused
        let mut t = start(ASK);
        run(&mut t, RESULT);
        let answer = "I couldn’t retrieve a live tracking status: WorldTracer’s tracking form was available, but I couldn’t submit the reference and surname from this bot. **So I don’t yet know where the bag is.**\n\nI’ve set up monitoring for **NRTSQ12345**: a recurring check is scheduled twice daily.\n\nThe 21-day rule: a bag not found by 21 November is treated as lost.";
        let out = with_bag_status(&t, answer, false);
        assert!(out.starts_with("**Your bag (WorldTracer, file NRTSQ12345):** Tracing continues — your bag has not been located yet."), "{out}");
        assert!(out.contains("Last update: 2026-10-31 22:10"), "{out}");
        assert!(!out.contains("couldn"), "{out}");
        assert!(!out.contains("don’t yet know"), "{out}");
        assert!(!out.contains("****"), "{out}");
        assert!(
            out.contains("I’ve set up monitoring") && out.contains("21-day rule"),
            "{out}"
        );
        // an answer that gives the status already is left as it is
        let good = "WorldTracer says tracing continues: the bag has not been located yet.";
        assert_eq!(with_bag_status(&t, good, false), good);
        // one that leaves it out gets it added
        let silent = "I drafted your expenses claim (not sent).";
        let out = with_bag_status(&t, silent, true);
        assert!(
            out.starts_with(silent) && out.contains("行李状态（WorldTracer，文件 NRTSQ12345）"),
            "{out}"
        );
    }

    #[test]
    fn the_reference_comes_from_the_words_or_a_bag_email_and_nothing_runs_without_it() {
        // no reference anywhere: no lookup
        let t = start("My bag didn't arrive in Tokyo. What should I do?");
        assert!(next_call(&t, 1).is_none());
        assert!(with_bag_status(&t, "x", false) == "x");
        // the reference in the PIR email the model read: the lookup starts, with the PIR's family name
        let mut t = t;
        t.extend(step(
            "c1",
            "gmail_get_message",
            json!({"message_id": "m1"}),
            "<untrusted_content source=\"email\">{\"from\": \"Narita Baggage Services <bags@narita.example>\", \"subject\": \"Property Irregularity Report NRTSQ12345\", \"body\": \"Passenger TAN/WEIMING. File reference NRT SQ 12345. 1 bag missing.\"}</untrusted_content>",
        ));
        let c = next_call(&t, 2).expect("navigate");
        assert_eq!(c.name, "browser_navigate");
        t.extend(step(&c.id, &c.name, c.args.clone(), &page(TRACE_URL, FORM)));
        let c = next_call(&t, 3).expect("fill");
        assert_eq!(
            c.args,
            json!({"file_ref": "NRTSQ12345", "family_name": "TAN"})
        );
        // a skill not loaded: nothing
        let mut other = vec![json!({"role": "user", "content": ASK})];
        assert!(next_call(&other, 1).is_none());
        other.extend(step(
            "c0",
            "load_skill",
            json!({"name": "flight-disruption"}),
            "---",
        ));
        assert!(next_call(&other, 1).is_none());
    }

    #[test]
    fn a_failed_step_ends_the_lookup_and_no_file_is_a_status() {
        let mut t = start(ASK);
        let c = next_call(&t, 1).unwrap();
        t.extend(step(
            &c.id,
            &c.name,
            c.args.clone(),
            "ERROR: navigation failed: net::ERR_NAME_NOT_RESOLVED",
        ));
        assert!(next_call(&t, 2).is_none());
        assert!(!pending(&t));
        // the form not on the page (the plan's require stops): no Track, no snapshot
        let mut t = start(ASK);
        let c = next_call(&t, 1).unwrap();
        t.extend(step(
            &c.id,
            &c.name,
            c.args.clone(),
            &page(TRACE_URL, "Maintenance"),
        ));
        let c = next_call(&t, 2).unwrap();
        t.extend(step(&c.id, &c.name, c.args.clone(), "{\"url\": \"x\", \"filled\": [{\"op\": \"require\", \"field\": \"trace_form\", \"ok\": false, \"stop\": true, \"why\": \"the tracing form is not on this page\"}]}"));
        assert!(next_call(&t, 3).is_none());
        // a reference WorldTracer does not know
        let mut t = start(ASK);
        run(&mut t, "## Track your delayed baggage\n  No file found for this reference. Check the reference on your Property Irregularity Report.");
        assert_eq!(
            trace(&t).map(|x| x.status),
            Some("No file found for this reference".into())
        );
    }
}
