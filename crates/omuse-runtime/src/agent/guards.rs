//! Loop guards, page hints and answer checks: the module-level helpers of `app/runtime/agent.py`.
//!
//! All of them are pure functions over the transcript (a list of chat messages as JSON) and the task's event log,
//! so they are unit-tested directly (`tests/test_027.py`, `tests/test_units.py`).

use std::collections::HashSet;
use std::sync::LazyLock;

use indexmap::IndexMap;
use omuse_common::guard::urlparse;
use omuse_common::util::{get_str, get_truthy, take_chars, truncate, truthy};
use regex::Regex;
use serde_json::{json, Map, Value};

use super::util::{arr, content_of, dumps_sorted, pystr_or_empty, sget, stored_call, tool_calls};
use crate::llm::ToolCall;

/// Loop guard: small local models sometimes repeat the exact same call until the step budget is gone
/// (e.g. opening one RSS feed 24 times in a row). Identical calls are refused after REPEAT_STREAK in a row,
/// and read-type calls after REPEAT_TOTAL anywhere in the task. Tools where repeating is normal are exempt from the streak rule.
pub const REPEAT_STREAK: usize = 2;
pub const REPEAT_TOTAL: usize = 3;
/// update_plan calls in a row (with nothing else between) before the next one is refused
pub const PLAN_STREAK: usize = 3;
pub const REPEAT_STREAK_OK: [&str; 7] = [
    "browser_scroll",
    "browser_press",
    "browser_wait",
    "browser_click",
    "browser_back",
    "update_plan",
    "phone_call_status", // polling a live call is expected to repeat
];
static REPEAT_READ: LazyLock<Regex> =
    LazyLock::new(|| rx(r"navigate|search|read|list|_get|fetch|query"));

/// Dead-end sources: a web host that failed (error, bot wall, refused repeat) HOST_FAIL_MAX times in one task is
/// blocked for the rest of the task, so the agent has to switch source instead of hammering the same link.
/// When every call in a turn is refused, the agent is stuck: 1st time it gets a firm nudge, 2nd time a re-plan that is
/// told which sources to avoid, 3rd time (or after MAX_REPLANS re-plans) it stops and answers with what it has.
pub const HOST_FAIL_MAX: i64 = 3;
pub const MAX_REPLANS: i64 = 4;
/// chars of context: compress old tool results in one batch
pub const COMPRESS_HIGH: usize = 80000;
pub const COMPRESS_LOW: usize = 40000;
/// share of the time budget (Settings → max_minutes) after which only the finishing tools remain (`Runtime::budget`)
pub const TIME_NUDGE: f64 = 0.7;

/// The least step budget of a form-filling skill's task (`form: on`): pages, fields, checks and the hand-over.
pub const FORM_STEPS: i64 = 70;

pub const COMPRESSED_MARK: &str = "older result compressed";

// step budget: keep the last steps for producing / sending what the user asked for
pub const BUDGET_RESERVE: i64 = 5;
pub const RESEARCH_NUDGE_PAGES: i64 = 10;
/// web tool calls in one task after which the page-reading tools are withdrawn (`Runtime::budget`)
pub const WEB_NUDGE: i64 = 18;
/// the 2026-10-02 itinerary run re-made the same Excel file 9 times (≈5 minutes of generation)
pub const REMAKE_MAX: i64 = 4;

pub const TAKEOVER_DONE_MARK: &str = "用户已完成接管";
pub const TAKEOVER_DONE_EN: &str = "Takeover finished";

fn rx(p: &str) -> Regex {
    Regex::new(p).unwrap()
}

/// Order- and case-insensitive key for a web search, so a re-worded copy of an earlier search is recognised.
pub fn search_key(q: &Value) -> String {
    static W: LazyLock<Regex> = LazyLock::new(|| rx(r"[\w一-鿿]+"));
    let s = pystr_or_empty(q).to_lowercase();
    let words: std::collections::BTreeSet<&str> = W.find_iter(&s).map(|m| m.as_str()).collect();
    words.into_iter().collect::<Vec<_>>().join(" ")
}

/// `name + " " + json.dumps(args or {}, ensure_ascii=False, sort_keys=True)`; JSON text arguments are parsed first.
pub fn call_sig(name: &str, args: &Value) -> String {
    let mut a = args.clone();
    if let Value::String(s) = &a {
        if let Ok(v) = serde_json::from_str::<Value>(if s.is_empty() { "{}" } else { s }) {
            a = v;
        }
    }
    let mut a = if truthy(&a) { a } else { json!({}) };
    // the same link, as written or as its stand-in (crate::links), is the same call
    crate::links::decode_value(&mut a);
    format!("{} {}", name, dumps_sorted(&a))
}

/// `(name, args)` of every tool call in the transcript before `call_id` (all of them when the id is not found).
fn calls_before(transcript: &[Value], call_id: &str) -> Vec<(String, Value)> {
    let mut prev = Vec::new();
    for m in transcript {
        for tc in tool_calls(m) {
            if get_str(tc, "id") == call_id {
                return prev;
            }
            prev.push(stored_call(tc));
        }
    }
    prev
}

/// The page a browser call ended on (the `"url"` its result reports), by call id.
fn result_urls(transcript: &[Value]) -> std::collections::HashMap<String, String> {
    static URL: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#""url":\s*"([^"]+)""#).unwrap());
    transcript
        .iter()
        .filter(|m| get_str(m, "role") == "tool")
        .filter_map(|m| {
            let c = content_of(m);
            URL.captures(&c)
                .map(|u| (get_str(m, "tool_call_id"), u[1].to_string()))
        })
        .collect()
}

/// browser_type into a field the agent already filled on this page, with different text, is almost always a mix-up
/// (e.g. typing the first name into the booking-number box and wiping it). Ask it to use the right field instead.
/// Refs are per page: a field typed on another page (2026-10-29: "Japan" in the quote page's e1, then the application
/// page's e1 is the full name — reached by a plain link click) is a different field.
pub fn retype_guard(transcript: &[Value], call: &ToolCall) -> Option<String> {
    if call.name != "browser_type" {
        return None;
    }
    let args = &call.args;
    let (r, text) = (sget(args, "ref"), sget(args, "text"));
    if r.is_empty() || get_truthy(args, "replace") {
        return None;
    }
    let urls = result_urls(transcript);
    let mut ids: Vec<String> = Vec::new();
    'outer: for m in transcript {
        for tc in tool_calls(m) {
            if get_str(tc, "id") == call.id {
                break 'outer;
            }
            ids.push(get_str(tc, "id"));
        }
    }
    // the page the agent is on now: the last browser call's
    let here = ids.iter().rev().find_map(|i| urls.get(i)).cloned();
    let prev = calls_before(transcript, &call.id);
    // a call that was refused (a guard's ERROR, Sentinel's DENIED) typed nothing (arrival_cards_real 2026-10-30: the
    // refused departure date then blocked the right one as "already typed")
    let refused: HashSet<String> = transcript
        .iter()
        .filter(|m| get_str(m, "role") == "tool")
        .filter(|m| {
            let c = content_of(m);
            c.starts_with("ERROR") || c.starts_with("DENIED")
        })
        .map(|m| get_str(m, "tool_call_id"))
        .collect();
    for ((nm, a), id) in prev.iter().zip(ids.iter()).rev() {
        if refused.contains(id) {
            continue;
        }
        if nm == "browser_type" && sget(a, "ref") == r {
            if let (Some(then), Some(now)) = (urls.get(id), here.as_ref()) {
                if then != now {
                    return None;
                }
            }
        }
        if nm == "browser_navigate" || nm == "browser_back" {
            return None;
        }
        if nm == "browser_click" && get_truthy(a, "submit") {
            return None;
        }
        if nm == "browser_type" && sget(a, "ref") == r {
            if get_truthy(a, "submit") {
                // a search box: typing a new query after a submitted one is a new search
                return None;
            }
            let old = sget(a, "text");
            if !old.is_empty() && old != text {
                let (o, t) = (take_chars(&old, 40), take_chars(&text, 40));
                return Some(format!(
                    "ERROR: [retype] 没有执行 — {r} 里已经输入了「{o}」，「{t}」会把它覆盖掉（replace=true 才会覆盖）。 \
Not done: you already typed \"{o}\" into {r}; \"{t}\" would replace it (replace=true replaces)."
                ));
            }
            return None;
        }
    }
    None
}

/// An email with no recipient, no body and no thread is refused before it reaches Sentinel: no approval dialog for an
/// empty mail
/// (arrival_cards2_real 2026-10-30: two `gmail_send` calls with every argument empty at the end of a form fill).
pub fn empty_mail_guard(call: &ToolCall) -> Option<String> {
    if !matches!(call.name.as_str(), "gmail_send" | "gmail_create_draft") {
        return None;
    }
    let args = &call.args;
    // a reply (the thread gives the recipient) or a draft with a body is a real email, even with `to` empty
    let empty = ["to", "body", "reply_to_message_id"]
        .iter()
        .all(|k| sget(args, k).trim().is_empty());
    empty.then(|| {
        format!(
            "ERROR: [empty_mail] 没有执行 — 这封邮件没有收件人、没有正文，也不是回复。 Not done: this {} has no recipient and no body, and \
replies to nothing.",
            call.name
        )
    })
}

/// Whether the run used the browser (any `browser_*` call).
pub fn browser_used(transcript: &[Value]) -> bool {
    transcript
        .iter()
        .flat_map(tool_calls)
        .any(|tc| stored_call(tc).0.starts_with("browser_"))
}

/// The words that place an address in an arrival skill's country (`sg-arrival-card` → Singapore …).
fn arrival_country(skill: &str) -> &'static [&'static str] {
    match skill.split('-').next().unwrap_or("") {
        "sg" => &["Singapore"],
        "th" => &["Thailand", "Bangkok", "Phuket", "Chiang Mai"],
        "my" => &["Malaysia", "Kuala Lumpur", "Penang"],
        "kr" => &["Korea", "Seoul", "Busan"],
        "jp" => &["Japan", "Tokyo", "Osaka", "Kyoto"],
        "cn" => &["China", "Shanghai", "Beijing", "Shenzhen"],
        "tw" => &["Taiwan", "Taipei"],
        "id" => &["Indonesia", "Bali", "Jakarta"],
        "ph" => &["Philippines", "Manila", "Cebu"],
        "kh" => &["Cambodia", "Phnom Penh", "Siem Reap"],
        _ => &[],
    }
}

/// The reference of the confirmation page after the user's hand-back on a site no code knows, as a fact the reply must
/// carry ("<label>: <code>", `agent/facts.rs`): the model read it on the page and passed it as JSON
/// (`confirm_reference`, checked in `agent/confirm.rs`). A known site's reference is in its machine's result; no
/// page's text is scanned for one (arrival_cards2_real 2026-10-30: Visit Japan Web's Registration ID left out of the
/// reply — now the arrival-card machine's `must_say`).
pub fn confirmation_ref_fact(transcript: &[Value]) -> Option<String> {
    super::confirm::fact(transcript)
}

/// The label the newest page snapshot gives a field ref (`[e17] textbox "Intended date of departure"`).
/// The browser tools that put a value into a page. The model may use them only while the `browser-use` skill is
/// loaded (a site with no form function); everywhere else forms are filled by code with fixed input ids.
pub const MODEL_INPUT_TOOLS: [&str; 5] = [
    "browser_type",
    "browser_select",
    "browser_fill_secret",
    "browser_upload",
    "browser_press",
];

/// Whether this run loaded the `browser-use` skill (the only place the model may fill a page).
pub fn browser_use_active(transcript: &[Value]) -> bool {
    transcript.iter().flat_map(tool_calls).any(|tc| {
        let (name, args) = stored_call(tc);
        name == "load_skill" && sget(&args, "name") == "browser-use"
    })
}

/// A call made by a code machine (the arrival cards, the claims, APDT, booking): its fills are code's, with fixed
/// ids, never the model's.
pub fn code_call(call: &ToolCall) -> bool {
    [
        "drv_",
        "clm_",
        "apd_",
        "bkg_",
        "nws_",
        "trn_",
        "fst_",
        "auto_handover_",
        "news_",
        "handback_snapshot_",
    ]
    .iter()
    .any(|p| call.id.starts_with(p))
        || call.id.ends_with("_handover")
}

/// The role a snapshot gave an element (`[e12] checkbox "…"` → "checkbox").
fn ref_role(transcript: &[Value], r: &str) -> Option<String> {
    let re = Regex::new(&format!(r#"\[{}\]\s+(\w+)"#, regex::escape(r))).ok()?;
    transcript
        .iter()
        .rev()
        .filter(|m| get_str(m, "role") == "tool")
        .find_map(|m| re.captures(&content_of(m)).map(|c| c[1].to_lowercase()))
}

/// A model call that would put a value into a page outside the `browser-use` skill is refused: the input tools,
/// `form_fill` (code computes its facts), a `browser_click_at` that types, and a click on a checkbox, radio, option or
/// combobox. Code machines' calls are exempt (they fill by fixed ids).
pub fn input_scope_guard(transcript: &[Value], call: &ToolCall) -> Option<String> {
    if code_call(call) || browser_use_active(transcript) {
        return None;
    }
    let refused = match call.name.as_str() {
        n if MODEL_INPUT_TOOLS.contains(&n) => true,
        "form_fill" => true,
        "browser_click_at" => {
            !sget(&call.args, "text").is_empty()
                || call.args.get("submit").and_then(Value::as_bool) == Some(true)
        }
        "browser_click" => ref_role(transcript, &sget(&call.args, "ref")).is_some_and(|r| {
            matches!(
                r.as_str(),
                "checkbox" | "radio" | "option" | "combobox" | "switch"
            )
        }),
        _ => false,
    };
    refused.then(|| {
        "ERROR: [input_scope] form inputs are filled by code with fixed ids; a site with no form function is filled \
only inside the browser-use skill."
            .to_string()
    })
}

fn field_label(transcript: &[Value], r: &str) -> Option<String> {
    let re = Regex::new(&format!(
        r#"\[{}\]\s+\w+\s+\\?"([^"\\]+)"#,
        regex::escape(r)
    ))
    .ok()?;
    transcript
        .iter()
        .rev()
        .filter(|m| get_str(m, "role") == "tool")
        .find_map(|m| re.captures(&content_of(m)).map(|c| c[1].to_string()))
}

/// An arrival card's final Submit is the traveller's own declaration, a claim's Submit the user's own claim: on such a
/// run a click on a button or link named submit / send / confirm / declare (a claim: submit) is refused like a
/// purchase's last click, and the runtime hands the
/// filled page over (`exec::hand_over_button`) (arrival_cards2_real 2026-10-30: "I will submit as soon as you hand
/// the browser back", then the agent pressed Submit itself).
pub fn arrival_submit_guard(transcript: &[Value], call: &ToolCall) -> Option<String> {
    static SUBMIT: LazyLock<Regex> = LazyLock::new(|| {
        // not "Confirm": a terms page's Confirm only opens the form (cards_real 2026-10-31: Korea's e-Arrival card)
        // nor "Declaration" (a heading or a tile: China's "Entry Declaration" opens the form)
        Regex::new(r"(?i)\bsubmit|\bsend\b|\bpay\b|提交|递交|送出").unwrap()
    });
    // a claim the user submits (cards_claim_real 2026-10-31: the agent pressed "Submit claim" itself): its Submit only
    static SUBMIT_CLAIM: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"(?i)\bsubmit|提交|递交").unwrap());
    if call.name != "browser_click" {
        return None;
    }
    let arrival = arrival_run(transcript);
    if !arrival && !claim_run(transcript) {
        return None;
    }
    let r = sget(&call.args, "ref");
    let re = Regex::new(&format!(
        r#"\[{}\]\s+(\w+)\s+\\?"([^"\\]+)"#,
        regex::escape(&r)
    ))
    .ok()?;
    let (role, label) = transcript
        .iter()
        .rev()
        .filter(|m| get_str(m, "role") == "tool")
        .find_map(|m| {
            re.captures(&content_of(m))
                .map(|c| (c[1].to_string(), c[2].trim().to_string()))
        })?;
    let submits = if arrival { &SUBMIT } else { &SUBMIT_CLAIM };
    // a button that submits; a link is navigation (Taiwan's "Submit Arrival Card" tile opens the form)
    if role != "button" || !submits.is_match(&label) {
        return None;
    }
    Some(format!(
        "ERROR: [user_submits] 没有执行 — 「{label}」由用户自己提交。 (\"{label}\" submits the user's own declaration or claim: \
that click is the user's own.)"
    ))
}

/// The names of the skills this run loaded whose front matter says `form_kind: <kind>` — `arrival` (an arrival card or
/// travel authorisation the traveller submits: the arrival cards, Visit Japan Web, eTravel, the UK ETA, ETIAS) or
/// `claim` (a claim the user submits on a web form: CSR, FWD). Read from the loaded skill's own text, so an imported
/// skill with the flag gets the same guards (2026-10-31: ETIAS, missing from a hard-coded list, got none of them).
fn form_kind_skills(transcript: &[Value], kind: &str) -> Vec<String> {
    let mut out = Vec::new();
    for tc in transcript.iter().flat_map(tool_calls) {
        let (name, args) = stored_call(tc);
        if name != "load_skill" || !sget(&args, "file").trim().is_empty() {
            continue;
        }
        let id = get_str(tc, "id");
        let loaded = transcript
            .iter()
            .find(|m| get_str(m, "role") == "tool" && get_str(m, "tool_call_id") == id)
            .map(content_of)
            .unwrap_or_default();
        let skill = sget(&args, "name");
        if loaded.starts_with("ERROR") {
            continue;
        }
        let mut fm = crate::skills::front_matter(loaded.trim_start());
        if !loaded.trim_start().starts_with("---") || !fm.contains_key("form_kind") {
            // no (whole) front matter in what came back — a profile note on top, or a result the compression cut to
            // its first 500 characters, inside the long description (cards_interruption_real 2026-10-31: the claim
            // was filled and never handed over): the built-in skill's own
            let own = crate::skills::BUILTIN
                .get_file(format!("{skill}/SKILL.md"))
                .and_then(|f| f.contents_utf8())
                .map(crate::skills::front_matter)
                .unwrap_or_default();
            if !own.is_empty() {
                fm = own;
            }
        }
        if fm
            .get("form_kind")
            .is_some_and(|k| k.eq_ignore_ascii_case(kind))
        {
            out.push(skill);
        }
    }
    out
}

/// The Hong Kong departure-tax refund's face recognition is the user's own step, on their phone: a click on "Proceed with
/// Facial Recognition" (or any face-recognition button) in that run is refused, and the agent stops with the reference
/// code and the link (`with_apdt_link`).
pub fn apdt_face_guard(transcript: &[Value], call: &ToolCall) -> Option<String> {
    static FACE: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?i)facial|face[- ]?(recognition|verification|check)|人脸|人臉|刷脸").unwrap()
    });
    // a takeover once the card is in (from the Vault, or typed by the user in an earlier takeover): only the face step
    // is left, and that is on the phone (apdt_real 2026-10-31: "take over the browser to complete that verification")
    if call.name == "browser_request_takeover" && apdt_card_done(transcript) {
        return Some(
            "ERROR: [apdt_face] 没有交接 — 银行卡已经填好，剩下的人脸识别由用户本人在手机上完成。 Not handed over: the card is in, \
and the face recognition left is the user's own step on their phone."
                .to_string(),
        );
    }
    if !matches!(call.name.as_str(), "browser_click" | "browser_click_at") {
        return None;
    }
    if !apdt_run(transcript) {
        return None;
    }
    let label = field_label(transcript, &sget(&call.args, "ref")).unwrap_or_default();
    FACE.is_match(&label).then(|| {
        "ERROR: [apdt_face] 没有执行 — 人脸识别由用户本人在手机上完成。 Not done: the face recognition is the user's own step, on \
their phone."
            .to_string()
    })
}

/// Whether the run loaded the Hong Kong departure-tax refund skill.
fn apdt_run(transcript: &[Value]) -> bool {
    transcript.iter().flat_map(tool_calls).any(|tc| {
        let (name, args) = stored_call(tc);
        name == "load_skill" && sget(&args, "name") == "hk-apdt-refund"
    })
}

/// A departure-tax refund run whose refund card is in: the Vault card's number filled, or a completed takeover (the
/// user typed it). What is left is the face recognition, on the user's phone — nothing to hand over in the browser.
pub fn apdt_card_done(transcript: &[Value]) -> bool {
    if !apdt_run(transcript) {
        return false;
    }
    let calls: Vec<(String, String, Value)> = transcript
        .iter()
        .flat_map(tool_calls)
        .map(|tc| {
            let (n, a) = stored_call(tc);
            (get_str(tc, "id"), n, a)
        })
        .collect();
    transcript.iter().any(|m| {
        get_str(m, "role") == "tool"
            && !content_of(m).starts_with("ERROR")
            && calls.iter().any(|(id, n, a)| {
                *id == get_str(m, "tool_call_id")
                    && ((n == "browser_fill_secret" && sget(a, "field") == "number")
                        || n == "browser_request_takeover")
            })
    })
}

/// The fixed answer on the departure-tax refund's upload page: OMuse submits for the applicant, so "I have authorized a
/// third party to submit my information" is always ticked — in code, before the agent's first click on that page moves
/// on (apdt_real 2026-10-31: the declaration ticked, the authorisation left blank). The `browser_fill_form` args.
pub fn apdt_fixed_ticks(transcript: &[Value], call: &ToolCall) -> Option<Value> {
    static PAGE: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r#"source="web page (https?://[^"\s]+)"#).unwrap());
    if call.name != "browser_click" || !apdt_run(transcript) {
        return None;
    }
    let url = transcript
        .iter()
        .rev()
        .filter(|m| get_str(m, "role") == "tool")
        .find_map(|m| PAGE.captures(&content_of(m)).map(|c| c[1].to_string()))?;
    if !(url.contains("apdtrefund.hk") && url.contains("apdt_upload")) {
        return None;
    }
    // `on_box`: the click is on that box itself — it becomes the tick (a click could untick it)
    let label = field_label(transcript, &sget(&call.args, "ref")).unwrap_or_default();
    let on_box = label.to_lowercase().contains("third party");
    let fills: Vec<Value> = [crate::forms::tick(
        "third_party",
        "",
        "authorized a third party",
    )]
    .iter()
    .filter_map(crate::forms::Op::to_json)
    .collect();
    Some(json!({"form": "hk-apdt-refund", "fills": fills, "on_box": on_box}))
}

/// Whether the run loaded a skill whose claim the user submits themselves on a web form (`form_kind: claim`).
pub fn claim_run(transcript: &[Value]) -> bool {
    !form_kind_skills(transcript, "claim").is_empty()
}

/// Whether the run loaded an arrival-card skill (`form_kind: arrival`).
pub fn arrival_run(transcript: &[Value]) -> bool {
    !form_kind_skills(transcript, "arrival").is_empty()
}

/// On an arrival card, clearing a field the agent typed on this page is refused (the field keeps its value): its values are the defaults the user
/// asked for (departure = arrival + 7 days, next city = the flight's origin), which the user reviews and corrects on
/// the page after the hand-over (arrival_real 2026-10-30: the TDAC's next city and departure date typed, then wiped,
/// and the user asked for them in the chat).
pub fn clear_guard(transcript: &[Value], call: &ToolCall) -> Option<String> {
    let args = &call.args;
    let r = sget(args, "ref");
    if call.name != "browser_type" || r.is_empty() || !sget(args, "text").trim().is_empty() {
        return None;
    }
    if !arrival_run(transcript) {
        return None;
    }
    let typed = calls_before(transcript, &call.id)
        .iter()
        .rev()
        .take_while(|(nm, _)| nm != "browser_navigate" && nm != "browser_back")
        .any(|(nm, a)| nm == "browser_type" && sget(a, "ref") == r && !sget(a, "text").is_empty());
    typed.then(|| {
        format!(
            "ERROR: [keep_default] 没有清空 {r} — 那是用户要的默认值，用户接手后在页面上核对。 Not cleared: {r} keeps its value, \
the user's default, reviewed on the page after the hand-over."
        )
    })
}

/// Whether a run ends with a filled form the user must finish but no hand-over: an arrival card or a claim whose
/// form was typed into (no completed takeover after the last fill), or a browser run whose purchase the user approved
/// — transcript facts only; the runtime then hands the page over itself (cards_claim_real 2026-10-31: "submit it
/// yourself when ready", twice, no takeover).
pub fn needs_handover(transcript: &[Value]) -> bool {
    let calls: Vec<(String, String)> = transcript
        .iter()
        .flat_map(tool_calls)
        .map(|tc| (get_str(tc, "id"), stored_call(tc).0))
        .collect();
    // handed over = a completed takeover after the last fill: a sign-in before the form is not the hand-over
    // (arrival_cards2_real 2026-10-31: Visit Japan Web, signed in, filled, never handed over)
    let fills = |n: &str| {
        matches!(
            n,
            "browser_type"
                | "browser_select"
                | "browser_fill_secret"
                | "form_fill"
                | "browser_fill_form"
        )
    };
    let last_fill = transcript
        .iter()
        .rposition(|m| tool_calls(m).iter().any(|tc| fills(&stored_call(tc).0)));
    let handed = transcript.iter().enumerate().any(|(i, m)| {
        last_fill.is_none_or(|lf| i > lf)
            && get_str(m, "role") == "tool"
            && !content_of(m).starts_with("ERROR")
            && calls
                .iter()
                .any(|(id, n)| *id == get_str(m, "tool_call_id") && n == "browser_request_takeover")
    });
    if handed || apdt_card_done(transcript) {
        return false;
    }
    let filled = calls.iter().any(|(_, n)| {
        matches!(
            n.as_str(),
            "browser_type"
                | "browser_select"
                | "browser_fill_secret"
                | "form_fill"
                | "browser_fill_form"
        )
    });
    (filled && (arrival_run(transcript) || claim_run(transcript)))
        || (purchase_approved(transcript, &calls) && browser_used(transcript))
}

/// What the user changed in approval dialogs during this run: (the record's id, field, the user's value), read from
/// results that carry `user_edited` (Sentinel adds it when the user edits an action before approving).
fn user_edits(transcript: &[Value]) -> Vec<(String, String, String)> {
    let mut out = Vec::new();
    for m in transcript.iter().filter(|m| get_str(m, "role") == "tool") {
        let text = content_of(m);
        let Some(i) = text.find('{') else { continue };
        // a result cut by the transcript's compression is no longer JSON: its `user_edited` object (Sentinel puts it
        // first) is read on its own
        static EDITED: LazyLock<Regex> =
            LazyLock::new(|| Regex::new(r#""user_edited":\s*(\{[^{}]*\})"#).unwrap());
        let v = match serde_json::from_str::<Value>(&text[i..]) {
            Ok(v) => v,
            Err(_) => match EDITED
                .captures(&text)
                .and_then(|c| serde_json::from_str::<Value>(&c[1]).ok())
            {
                Some(e) => json!({"user_edited": e}),
                None => continue,
            },
        };
        let Some(edited) = v.get("user_edited").and_then(Value::as_object) else {
            continue;
        };
        let id = v
            .get("event")
            .and_then(|e| e.get("id"))
            .or_else(|| v.get("id"))
            .map(omuse_common::util::py_str)
            .unwrap_or_default();
        for (k, val) in edited {
            out.push((id.clone(), k.clone(), omuse_common::util::py_str(val)));
        }
    }
    out
}

/// An update that would set back what the user changed in an approval dialog does not run: the change is the user's
/// own (gcal_real 2026-10-31: the user moved the invite to 15:30, the agent called calendar_update_event with 15:00
/// again despite the note). Other fields of the update are the agent's business.
pub fn user_edit_guard(transcript: &[Value], call: &ToolCall) -> Option<String> {
    if !call.name.ends_with("_update_event") && !call.name.ends_with("_update") {
        return None;
    }
    let id = ["event_id", "id", "page_id"]
        .iter()
        .map(|k| sget(&call.args, k))
        .find(|v| !v.is_empty())?;
    let reverted: Vec<String> = user_edits(transcript)
        .into_iter()
        .filter(|(rid, field, val)| {
            *rid == id && {
                let now = sget(&call.args, field);
                !now.is_empty() && now.trim() != val.trim()
            }
        })
        .map(|(_, field, val)| format!("{field} = {val}"))
        .collect();
    if reverted.is_empty() {
        return None;
    }
    Some(format!(
        "ERROR: [user_edit] 没有执行 — 这是用户在审批时自己改的：{0}，保持不变。 Not changed: the user set {0} themselves in the \
approval dialog; it stays.",
        reverted.join("; ")
    ))
}

/// What the user changed in approval dialogs, as a fact the reply must carry, so the reply never presents the user's
/// own change as a mistake (gcal_real 2026-10-31: "Google Calendar reports it as 3:30 … rather than the 3:00 slot I
/// requested").
pub fn user_edits_fact(transcript: &[Value], zh: bool) -> Option<String> {
    let edits = user_edits(transcript);
    if edits.is_empty() {
        return None;
    }
    let what: Vec<String> = edits.iter().map(|(_, f, v)| format!("{f} = {v}")).collect();
    Some(if zh {
        format!("你在审批时改了：{}，已按你的修改保存。", what.join("；"))
    } else {
        format!(
            "You changed this in the approval: {} — saved with your change.",
            what.join("; ")
        )
    })
}

/// Whether the run loaded the news research skill (its searches stay in the browser).
pub fn news_research_run(transcript: &[Value]) -> bool {
    transcript.iter().flat_map(tool_calls).any(|tc| {
        let (name, args) = stored_call(tc);
        name == "load_skill" && sget(&args, "name") == "news-research"
    })
}

/// A hand-over on a known form before code has filled it (`form_fill` / `browser_fill_form` never ran) is refused
/// once: the form function fills it first, and a step that is the user's (a photo, a sign-in) is named in its result
/// (arrival_cards2_real 2026-10-31: the China card handed over empty for the passport photo, then filled after the
/// user's Submit). A second takeover goes ahead.
pub fn takeover_before_fill_guard(transcript: &[Value], call: &ToolCall) -> Option<String> {
    const MARK: &str = "Not handed over yet: this page is a known form";
    // the hand-overs code makes itself (the automatic one, after a refused final click) always go ahead
    if call.name != "browser_request_takeover"
        || call.id.starts_with(AUTO_HANDOVER)
        || call.id.ends_with("_handover")
        || call.id.starts_with("drv_")
    {
        return None;
    }
    let calls: Vec<String> = transcript
        .iter()
        .flat_map(tool_calls)
        .map(|tc| stored_call(tc).0)
        .collect();
    if calls
        .iter()
        .any(|n| n == "form_fill" || n == "browser_fill_form")
    {
        return None;
    }
    if transcript
        .iter()
        .any(|m| get_str(m, "role") == "tool" && content_of(m).contains(MARK))
    {
        return None;
    }
    static PAGE: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r#"source="web page (https?://[^"\s]+)"#).unwrap());
    let url = transcript
        .iter()
        .rev()
        .filter(|m| get_str(m, "role") == "tool")
        .find_map(|m| PAGE.captures(&content_of(m)).map(|c| c[1].to_string()))?;
    let (form, _) = crate::forms::form_for(&url)?;
    Some(format!(
        "ERROR: [form_first] {MARK} ({form}) that code has not filled."
    ))
}

/// After the user handed the browser back, the page is read by code: `true` when no `browser_snapshot` followed the
/// last hand-back (the loop then takes one before the answer, and the model reads the reference on it and passes it
/// as JSON, `confirm_reference`; arrival_cards2_real 2026-11-01: Visit Japan Web answered without looking at the page,
/// no reference — now the arrival-card machine reads it by the page's element).
pub fn needs_handback_snapshot(transcript: &[Value]) -> bool {
    let last_back = transcript.iter().rposition(|m| {
        get_str(m, "role") == "tool" && {
            let c = content_of(m);
            c.contains(TAKEOVER_DONE_MARK) || c.contains(TAKEOVER_DONE_EN)
        }
    });
    let Some(back) = last_back else { return false };
    !transcript[back + 1..]
        .iter()
        .flat_map(tool_calls)
        .any(|tc| stored_call(tc).0 == "browser_snapshot")
}

/// The reason the runtime's own hand-over gives the user: an approved purchase is an order to place; anything else is
/// a form to review and submit (shopping_real 2026-10-31: the checkout was handed over as "the form is filled in").
pub fn handover_reason(transcript: &[Value], generic: &str) -> String {
    let calls: Vec<(String, String)> = transcript
        .iter()
        .flat_map(tool_calls)
        .map(|tc| (get_str(tc, "id"), stored_call(tc).0))
        .collect();
    if purchase_approved(transcript, &calls) {
        "The checkout is filled in with the order you approved. Review it and place the order yourself — the final \
place-order / pay click is yours; hand the browser back when it is done."
            .to_string()
    } else {
        generic.to_string()
    }
}

/// Whether the user approved a purchase in this run (`purchase_confirm` came back approved): the order is then placed
/// by the user, so a browser run that ends without a hand-over hands the checkout over (a purchase made with no page
/// open — a connector, a booking API — has nothing to hand over) (shopping_real 2026-10-31: the card
/// and address filled, "I did not press Place your order", nothing handed over).
fn purchase_approved(transcript: &[Value], calls: &[(String, String)]) -> bool {
    transcript.iter().any(|m| {
        get_str(m, "role") == "tool"
            && calls.iter().any(|(id, n)| {
                *id == get_str(m, "tool_call_id")
                    && n.rsplit("__").next() == Some("purchase_confirm")
            })
            && content_of(m).contains("\"status\": \"approved\"")
    })
}

/// The files the run wrote for the user to receive (`files_write`, `make_pdf`, `make_xlsx`, `make_docx` with
/// `deliver_as`, without an error) that never reached the chat (`send_file`): the runtime sends them before the answer
/// (chat_real 2026-10-31: "as a Markdown file I can download" — written, never sent). Read from the calls' JSON.
pub fn unsent_files(transcript: &[Value]) -> Vec<String> {
    let calls: Vec<(String, String, Value)> = transcript
        .iter()
        .flat_map(tool_calls)
        .map(|tc| {
            let (n, a) = stored_call(tc);
            (get_str(tc, "id"), n, a)
        })
        .collect();
    let ok = |id: &str| {
        transcript.iter().any(|m| {
            get_str(m, "role") == "tool"
                && get_str(m, "tool_call_id") == id
                && !content_of(m).starts_with("ERROR")
        })
    };
    let sent: Vec<String> = calls
        .iter()
        .filter(|(_, n, _)| n == "send_file")
        .flat_map(|(_, _, a)| {
            std::iter::once(sget(a, "path"))
                .chain(arr(a, "paths").iter().map(pystr_or_empty))
                .collect::<Vec<_>>()
        })
        .collect();
    let mut out: Vec<String> = Vec::new();
    for (id, n, a) in &calls {
        if !delivers(a) {
            continue;
        }
        let path = match n.as_str() {
            "files_write" => sget(a, "path"),
            "make_pdf" | "make_xlsx" | "make_docx" => {
                let o = sget(a, "output");
                if o.is_empty() {
                    sget(a, "path")
                } else {
                    o
                }
            }
            _ => continue,
        };
        if path.is_empty() || !ok(id) || sent.iter().any(|s| s == &path) || out.contains(&path) {
            continue;
        }
        out.push(path);
    }
    out
}

/// The file formats a file tool's call says the user asked to receive (`deliver_as`: pdf · xlsx · docx · md).
pub fn deliver_as(args: &Value) -> Vec<String> {
    arr(args, "deliver_as")
        .iter()
        .map(pystr_or_empty)
        .map(|f| f.trim().trim_start_matches('.').to_lowercase())
        .filter(|f| ["pdf", "xlsx", "docx", "md"].contains(&f.as_str()))
        .collect()
}

/// Whether a file tool's call says the user asked to receive files (`deliver_as` not empty).
pub fn delivers(args: &Value) -> bool {
    !deliver_as(args).is_empty()
}

/// ETIAS's status, as the eu-etias skill states it (checked 2026-10-30): not in operation, launch expected in 2027.
pub const ETIAS_STATUS_EN: &str =
    "ETIAS is not in operation yet (launch expected in 2027), so there is nothing to apply \
for now; check https://travel-europe.europa.eu/etias before you travel.";
pub const ETIAS_STATUS_ZH: &str = "ETIAS 目前还没有上线（预计 2027 年启用），现在不需要也无法申请；出发前请看 https://travel-europe.europa.eu/etias。";

/// ETIAS's status when the eu-etias skill ran, as a fact the reply must carry — the skill's central fact
/// (arrival_cards4_real 2026-10-31: an answer about the visa only).
pub fn etias_fact(transcript: &[Value], zh: bool) -> Option<String> {
    let etias = transcript.iter().flat_map(tool_calls).any(|tc| {
        let (name, args) = stored_call(tc);
        name == "load_skill" && sget(&args, "name") == "eu-etias"
    });
    etias.then(|| if zh { ETIAS_STATUS_ZH } else { ETIAS_STATUS_EN }.to_string())
}

/// Content the run read (a page, an email, a file) that carried instructions aimed at the assistant
/// (`injection_warning` on its untrusted wrapper), as a fact the reply must carry — the security rule's "tell the user".
pub fn injection_fact(transcript: &[Value], zh: bool) -> Option<String> {
    let mut srcs: Vec<String> = Vec::new();
    for m in transcript.iter().filter(|m| get_str(m, "role") == "tool") {
        let c = content_of(m);
        let Some(i) = c.find("injection_warning=") else {
            continue;
        };
        let Some(src) = c[..i]
            .rsplit("source=\"")
            .next()
            .and_then(|s| s.split('"').next())
        else {
            continue;
        };
        if !srcs.iter().any(|x| x == src) {
            srcs.push(src.to_string());
        }
    }
    if srcs.is_empty() {
        return None;
    }
    let list = srcs.into_iter().take(3).collect::<Vec<_>>().join("; ");
    Some(if zh {
        format!("注意：{list} 里有针对助手的指令，已忽略，没有照做。")
    } else {
        format!("Note: {list} contained instructions aimed at the assistant; they were ignored.")
    })
}

/// `answer` with every full card number masked to its last four digits ("•••• 1111"): a 13–19 digit run (spaces or
/// dashes between groups) that passes the Luhn check never reaches the user's chat, wherever the model read it from
/// (a photo of a card, a page) — the secrets rule in code.
pub fn with_masked_cards(answer: &str) -> String {
    static RUN: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"\b\d(?:[ -]?\d){12,18}\b").unwrap());
    let luhn = |digits: &[u32]| {
        let sum: u32 = digits
            .iter()
            .rev()
            .enumerate()
            .map(|(i, d)| {
                if i % 2 == 1 {
                    let x = d * 2;
                    if x > 9 {
                        x - 9
                    } else {
                        x
                    }
                } else {
                    *d
                }
            })
            .sum();
        sum.is_multiple_of(10)
    };
    RUN.replace_all(answer, |c: &regex::Captures| {
        let digits: Vec<u32> = c[0].chars().filter_map(|ch| ch.to_digit(10)).collect();
        // a card network's first digit (3 Amex / JCB, 4 Visa, 5 Mastercard, 6 Discover / UnionPay) and the Luhn check
        if (13..=19).contains(&digits.len()) && (3..=6).contains(&digits[0]) && luhn(&digits) {
            let last: String = digits[digits.len() - 4..]
                .iter()
                .map(|d| char::from_digit(*d, 10).unwrap_or('0'))
                .collect();
            format!("•••• {last}")
        } else {
            c[0].to_string()
        }
    })
    .into_owned()
}

/// The id prefix of the hand-over the runtime makes itself (`Runtime::run`, `needs_handover`).
pub const AUTO_HANDOVER: &str = "auto_handover_";

/// `answer` opened by the answer the agent gave before the runtime handed its form over (carried on the hand-over call,
/// the model's own text): always, decided from the transcript's structure, never by reading the reply
/// (cards_claim_real 2026-10-31: the benefit, amount and deadlines were in the answer before the hand-over, and the user
/// only got "the browser was handed back").
pub fn with_handover_answer(transcript: &[Value], answer: &str) -> String {
    let Some(before) = transcript.iter().rev().find_map(|m| {
        tool_calls(m)
            .iter()
            .any(|tc| get_str(tc, "id").starts_with(AUTO_HANDOVER))
            .then(|| content_of(m).trim().to_string())
    }) else {
        return answer.to_string();
    };
    if before.is_empty() || before == answer.trim() {
        return answer.to_string();
    }
    format!("{before}\n\n{}", answer.trim())
}

/// Refusal text if `call` repeats an identical earlier call too often, else None.
pub fn repeat_guard(transcript: &[Value], call: &ToolCall) -> Option<String> {
    let name = call.name.as_str();
    if name == "update_plan" {
        // 2026-10-04 TC17: 58 update_plan calls in a row (no real work between them) used up all 80 steps
        let names: Vec<String> = transcript
            .iter()
            .flat_map(tool_calls)
            .filter(|tc| get_str(tc, "id") != call.id)
            .map(|tc| stored_call(tc).0)
            .collect();
        if names.len() >= PLAN_STREAK
            && names[names.len() - PLAN_STREAK..]
                .iter()
                .all(|n| n == "update_plan")
        {
            return Some(
                "ERROR: [repeat] 计划已经连续更新过好几次，这次没有执行。 The plan was already updated several times in a row; \
this call was not run."
                    .to_string(),
            );
        }
        return None;
    }
    let sigs: Vec<String> = calls_before(transcript, &call.id)
        .iter()
        .map(|(n, a)| call_sig(n, a))
        .collect();
    let me = call_sig(name, &call.args);
    let streak = sigs.iter().rev().take_while(|s| **s == me).count();
    let total = sigs.iter().filter(|s| **s == me).count();
    if total > 0 && total < REPEAT_TOTAL + 2 && last_result_compressed(transcript, call, &me) {
        // 2026-10-04 M2-06: the earlier result was compressed out of the context, so the model cannot see it any more —
        // blocking the re-read left it asking for the same two searches until it gave up
        return None;
    }
    let searchy = name == "browser_type" && get_truthy(&call.args, "submit"); // a site search box (V3-03)
    if (name == "browser_click" || name == "browser_click_at") && total >= 4 {
        // 2026-10-04 V3-10: the same "Add to cart" ref was clicked 22 times while the cart stayed at 0 items
        return Some(format!(
            "ERROR: [repeat] 同一个元素已经点了 {total} 次，这次没有执行。 This element was already clicked {total}x; not \
clicked again."
        ));
    }
    if (!REPEAT_STREAK_OK.contains(&name) && streak >= REPEAT_STREAK)
        || ((REPEAT_READ.is_match(name) || searchy) && total >= REPEAT_TOTAL)
    {
        let n = total + 1;
        return Some(format!(
            "ERROR: [repeat] 重复调用已拦截 — 这是第 {n} 次用完全相同的参数调用 {name}，本次没有执行。 Repeated identical call \
blocked ({name}, {n}x): the result is above."
        ));
    }
    None
}

fn last_result_compressed(transcript: &[Value], call: &ToolCall, me: &str) -> bool {
    let mut last_id: Option<String> = None;
    for m in transcript {
        for tc in tool_calls(m) {
            let (n, a) = stored_call(tc);
            if get_str(tc, "id") != call.id && call_sig(&n, &a) == me {
                last_id = Some(get_str(tc, "id"));
            }
        }
    }
    let Some(last_id) = last_id else {
        return false;
    };
    transcript.iter().any(|m| {
        get_str(m, "role") == "tool"
            && get_str(m, "tool_call_id") == last_id
            && content_of(m).contains(COMPRESSED_MARK)
    })
}

/// Mail ids must come from a tool result: 2026-10-04 M2-10 the model "continued" a list of ids by arithmetic
/// (1878038389737846821, …86788965087, …83840083353 …) and burned 20 minutes on "message not found".
const ID_ARGS: [(&str, &str); 3] = [
    ("gmail_get_message", "message_id"),
    ("gmail_get_thread", "thread_id"),
    ("gmail_save_attachment", "message_id"),
];

pub fn invented_id_guard(
    transcript: &[Value],
    call: &ToolCall,
    known: Option<&HashSet<String>>,
) -> Option<String> {
    static SPLIT: LazyLock<Regex> = LazyLock::new(|| rx(r"[,\s]+"));
    let key = ID_ARGS
        .iter()
        .find(|(n, _)| *n == call.name)
        .map(|(_, k)| *k);
    let mut ids: Vec<String> = Vec::new();
    if let Some(key) = key {
        ids.push(sget(&call.args, key));
    } else if call.name == "gmail_read_amounts" {
        match call.args.get("message_ids") {
            Some(Value::Array(v)) => ids.extend(v.iter().map(omuse_common::util::py_str)),
            Some(v) if truthy(v) => {
                ids.extend(
                    SPLIT
                        .split(&omuse_common::util::py_str(v))
                        .map(str::to_string),
                );
            }
            _ => {}
        }
    }
    let ids: Vec<String> = ids
        .into_iter()
        .filter(|i| !i.is_empty())
        .map(|i| i.trim().to_string())
        .filter(|i| !i.is_empty())
        .collect();
    if ids.is_empty() {
        return None;
    }
    let seen: String = transcript
        .iter()
        .filter(|m| matches!(get_str(m, "role").as_str(), "tool" | "user" | "system"))
        .map(content_of)
        .collect::<Vec<_>>()
        .join("\n");
    let empty = HashSet::new();
    let known = known.unwrap_or(&empty);
    let bad: Vec<&String> = ids
        .iter()
        .filter(|i| {
            let tail = i.split_once(':').map(|(_, t)| t).unwrap_or(i);
            !known.contains(i.as_str()) && !known.contains(tail) && !seen.contains(tail)
        })
        .collect();
    if bad.is_empty() {
        return None;
    }
    if bad.len() < ids.len() {
        // drop the made-up ones, keep the real ones
        return None;
    }
    let shown = bad
        .iter()
        .take(3)
        .map(|s| s.as_str())
        .collect::<Vec<_>>()
        .join(", ");
    Some(format!(
        "ERROR: [unknown_id] 这些 id 没有出现在任何搜索结果里（{shown}…）。 These ids do not appear in any search result ({shown}…); \
an id is copied from a result as returned."
    ))
}

/// Reading the same mail / page / file again while its full result is still in the context only makes the context (and
/// every following model call) bigger: answer with a pointer instead. A compressed or failed earlier result is re-read.
static REREAD_TOOLS: LazyLock<Regex> = LazyLock::new(|| {
    rx(
        r"^(gmail_get_message|gmail_get_thread|notion_get_page|notion_read|files_read|web_read|browser_read|web_search|gmail_search|gmail_find_receipts|gmail_read_amounts)$",
    )
});

pub fn reread_guard(transcript: &[Value], call: &ToolCall) -> Option<String> {
    let name = call.name.as_str();
    if !REREAD_TOOLS.is_match(name) {
        return None;
    }
    let me = call_sig(name, &call.args);
    let mut earlier: HashSet<String> = HashSet::new();
    for m in transcript {
        for tc in tool_calls(m) {
            if get_str(tc, "id") == call.id {
                continue;
            }
            let (n, a) = stored_call(tc);
            if call_sig(&n, &a) == me {
                earlier.insert(get_str(tc, "id"));
            }
        }
    }
    if earlier.is_empty() {
        return None;
    }
    for m in transcript {
        if get_str(m, "role") == "tool" && earlier.contains(&get_str(m, "tool_call_id")) {
            let c = content_of(m);
            let head = take_chars(&c, 10);
            if !c.is_empty()
                && !c.starts_with("ERROR")
                && !c.contains(COMPRESSED_MARK)
                && !head.contains("已跳过")
                && !head.contains("已读取过")
            {
                let id = get_str(m, "tool_call_id");
                return Some(format!(
                    "已读取过 — 这个内容在上文调用 {id} 的结果里，完整保留着，没有再读一次。 Already read: the full result is \
above (call {id})."
                ));
            }
        }
    }
    None
}

/// The web searches the run made so far (`web_search`, `news_search`, `browser_news_search`).
pub fn web_searches(transcript: &[Value]) -> i64 {
    transcript
        .iter()
        .filter(|m| get_str(m, "role") == "assistant")
        .flat_map(tool_calls)
        .filter(|c| {
            matches!(
                stored_call(c).0.as_str(),
                "web_search" | "news_search" | "browser_news_search"
            )
        })
        .count() as i64
}

/// A web search past the task's WEB_NUDGE searches is refused: the search tools are withdrawn from the tool list by
/// then (`Runtime::budget`); this holds the line for the searches of the same turn (2026-10-02 R4-04b: a "top 3
/// products" lookup made 47 web calls).
pub fn web_budget_guard(transcript: &[Value], call: &ToolCall) -> Option<String> {
    if !matches!(
        call.name.as_str(),
        "web_search" | "news_search" | "browser_news_search"
    ) {
        return None;
    }
    let done = transcript
        .iter()
        .flat_map(tool_calls)
        .take_while(|tc| get_str(tc, "id") != call.id)
        .filter(|c| {
            matches!(
                stored_call(c).0.as_str(),
                "web_search" | "news_search" | "browser_news_search"
            )
        })
        .count() as i64;
    (done >= WEB_NUDGE).then(|| {
        format!(
            "ERROR: [web_budget] 这个任务已经做了 {done} 次网页搜索，不再搜索。 Blocked: this task already ran {done} web \
searches; no more are run."
        )
    })
}

/// `urlparse(url).hostname` lowercased without a leading `www.` ("" when unparsable).
pub fn host_of(url: &Value) -> String {
    let s = pystr_or_empty(url);
    let h = match urlparse(&s) {
        Ok(p) => p.hostname().unwrap_or_default().to_lowercase(),
        Err(_) => return String::new(),
    };
    h.strip_prefix("www.").map(str::to_string).unwrap_or(h)
}

/// Failures per web host in this task, from the event log (survives restarts and context compression).
pub fn source_failures(events: &[Value]) -> IndexMap<String, i64> {
    let mut calls: IndexMap<String, Value> = IndexMap::new();
    let mut fails: IndexMap<String, i64> = IndexMap::new();
    for e in events {
        let d = e
            .get("data")
            .filter(|d| truthy(d))
            .cloned()
            .unwrap_or_else(|| json!({}));
        let ty = get_str(e, "type");
        if ty == "tool_call" {
            let a = d
                .get("args")
                .filter(|a| a.is_object())
                .cloned()
                .unwrap_or_else(|| json!({}));
            calls.insert(sget(&d, "call_id"), a);
        } else if ty == "tool_result"
            && d.get("ok") == Some(&Value::Bool(false))
            && !get_truthy(&d, "skipped")
        {
            let args = calls
                .get(&sget(&d, "call_id"))
                .cloned()
                .unwrap_or_else(|| json!({}));
            let host = host_of(args.get("url").unwrap_or(&Value::Null));
            if !host.is_empty() {
                *fails.entry(host).or_insert(0) += 1;
            }
        }
    }
    fails
}

pub fn dead_ends_text(fails: &IndexMap<String, i64>, lang: &str) -> String {
    let mut bad: Vec<(&String, &i64)> = fails.iter().filter(|(_, n)| **n >= 2).collect();
    bad.sort_by_key(|(_, n)| -**n); // stable: ties keep insertion order, like Python's sorted
    bad.truncate(8);
    if bad.is_empty() {
        return String::new();
    }
    let lst = bad
        .iter()
        .map(|(h, n)| format!("{} ({}x)", h, n))
        .collect::<Vec<_>>()
        .join(", ");
    if lang == "en" {
        return format!("Dead ends in this task (failed or blocked repeatedly; do NOT use them again, pick a different source): {lst}");
    }
    format!(
        "本任务里已经反复失败/被拦截的来源（不要再用，换别的网站或方法）: {lst}\
\nDead ends in this task (do NOT use them again, pick a different source): {lst}"
    )
}

/// The options a comparison the model showed as cards (`present_choices`) lists, as facts the reply must carry, so the
/// reply stands on its own — in Telegram, a notification, a later "book the second one" (travel_brave_real 2026-10-31:
/// "you can choose from the cards shown", no price in the answer). A code machine's own cards (`bkg_` …) are its
/// result's business.
pub fn choices_facts(transcript: &[Value]) -> Vec<String> {
    let machine: HashSet<String> = transcript
        .iter()
        .flat_map(tool_calls)
        .map(|tc| get_str(tc, "id"))
        .filter(|id| {
            code_call(&ToolCall {
                id: id.clone(),
                name: String::new(),
                args: Value::Null,
            })
        })
        .collect();
    let Some(listing) = transcript
        .iter()
        .rev()
        .filter(|m| get_str(m, "role") == "tool" && !machine.contains(&get_str(m, "tool_call_id")))
        .map(content_of)
        .find(|c| {
            serde_json::from_str::<Value>(c.lines().next().unwrap_or(""))
                .is_ok_and(|v| v.get("shown").is_some())
        })
    else {
        return Vec::new();
    };
    listing
        .lines()
        .filter(|l| {
            l.split_once(". ")
                .is_some_and(|(n, _)| !n.is_empty() && n.chars().all(|c| c.is_ascii_digit()))
        })
        .map(|l| l.trim().to_string())
        .collect()
}

static CATALOG_AS_OF: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"# Credit cards — .*catalog as of (\d{4})-(\d{2})\)").unwrap());

/// The date of the card terms the run used, as a fact the reply must carry: the run read a credit-card-picker catalog
/// table (`# Credit cards — US (…; catalog as of 2026-08)`) — the user must always see how old the terms behind a pick
/// are (cards_real 2026-10-30: the right pick, no date).
pub fn catalog_date_fact(transcript: &[Value], zh: bool) -> Option<String> {
    let (y, m) = transcript
        .iter()
        .filter(|t| get_str(t, "role") == "tool")
        .find_map(|t| {
            let text = content_of(t);
            let c = CATALOG_AS_OF.captures(&text)?;
            Some((c[1].parse::<i32>().ok()?, c[2].parse::<u32>().ok()?))
        })?;
    let d = chrono::NaiveDate::from_ymd_opt(y, m, 1)?;
    Some(if zh {
        format!("信用卡权益数据截至 {y} 年 {m} 月；条款可能变化。")
    } else {
        format!(
            "Card terms from the catalog as of {}; terms change.",
            d.format("%B %Y")
        )
    })
}

/// Which of the user's cards (the Vault cards this run saw) are only unverified seeds in the shipped catalog
/// (`verified: false`), as a fact the reply must carry (cards_markets_real 2026-10-31: Singapore picks with seed rates,
/// nothing said). Judged against the catalog row of each card's `card_id`.
pub fn unverified_cards_fact(transcript: &[Value], zh: bool) -> Option<String> {
    if !card_run(transcript) {
        return None;
    }
    let mut named: Vec<String> = Vec::new();
    for card in vault_cards(transcript) {
        let Some(c) = catalog_card(&card.card_id) else {
            continue;
        };
        if c.get("verified").is_some_and(|v| v.as_bool() == Some(true)) {
            continue;
        }
        if !named.contains(&card.label) {
            named.push(card.label);
        }
    }
    if named.is_empty() {
        return None;
    }
    let list = named.join(", ");
    Some(if zh {
        format!("{list} 在信用卡数据库里的条款是未核实的初始数据，请以发卡行网页为准。")
    } else {
        format!(
            "The catalog's terms for {list} are unverified seeds; check them on the issuer's page."
        )
    })
}

/// Whether the run read a card catalog table (a credit-card-picker run).
pub(crate) fn card_run(transcript: &[Value]) -> bool {
    transcript
        .iter()
        .any(|m| get_str(m, "role") == "tool" && content_of(m).contains("# Credit cards — "))
}

/// The shipped card catalog's cards (`skills/credit-card-picker/cards/catalog.json`), parsed once.
pub(crate) fn catalog_cards() -> &'static [Value] {
    static CARDS: LazyLock<Vec<Value>> = LazyLock::new(|| {
        crate::skills::BUILTIN
            .get_file("credit-card-picker/cards/catalog.json")
            .and_then(|f| f.contents_utf8())
            .and_then(|t| serde_json::from_str::<Value>(t).ok())
            .map(|v| arr(&v, "cards").to_vec())
            .unwrap_or_default()
    });
    &CARDS
}

/// The catalog row with this id (`card_id`: the ID column of the tables, kept with a Vault card); `None` for "" or an
/// id the catalog lacks. Code finds a card's terms by its id only, never by the words of its name.
pub fn catalog_card(card_id: &str) -> Option<&'static Value> {
    let id = card_id.trim();
    if id.is_empty() {
        return None;
    }
    catalog_cards().iter().find(|c| get_str(c, "id") == id)
}

/// The foreign transaction fee of the user's cards issued in another country than the merchant's — the market of the
/// card table the run read last — from the catalog's structured `fx_fee_pct` (researched per card, with its source), as
/// facts the reply must carry: a card with a fee above 0 is named with it, a card whose fee the catalog does not know
/// (no `card_id`, or no fee) is said to be unknown (cards_markets_real 2026-10-31: a Hong Kong dinner, the Singapore
/// cards' fee left out).
pub fn fx_fees_facts(transcript: &[Value], zh: bool) -> Vec<String> {
    static MARKET: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"# Credit cards — ([A-Z]{2}) ").unwrap());
    let country = |c: &str| -> String {
        match c.trim().to_lowercase().as_str() {
            "sg" | "singapore" | "新加坡" => "SG",
            "hk" | "hong kong" | "香港" => "HK",
            "us" | "usa" | "united states" | "united states of america" | "美国" => "US",
            other => return other.to_uppercase(),
        }
        .to_string()
    };
    let place = |code: &str| -> String {
        match (code, zh) {
            ("SG", false) => "Singapore".into(),
            ("HK", false) => "Hong Kong".into(),
            ("US", false) => "the United States".into(),
            ("SG", true) => "新加坡".into(),
            ("HK", true) => "香港".into(),
            ("US", true) => "美国".into(),
            (c, _) => c.to_string(),
        }
    };
    let mut market = String::new();
    for m in transcript.iter().filter(|m| get_str(m, "role") == "tool") {
        let text = content_of(m).replace("\\n", "\n");
        if let Some(c) = MARKET.captures_iter(&text).last() {
            market = c[1].to_string();
        }
    }
    if market.is_empty() {
        return Vec::new();
    }
    let (mut fees, mut unknown): (Vec<String>, Vec<String>) = (Vec::new(), Vec::new());
    let mut seen: Vec<String> = Vec::new();
    for card in vault_cards(transcript) {
        if card.country.is_empty() || country(&card.country) == market || seen.contains(&card.label)
        {
            continue;
        }
        seen.push(card.label.clone());
        match catalog_card(&card.card_id)
            .and_then(|c| c.get("fx_fee_pct"))
            .and_then(Value::as_f64)
        {
            Some(fee) if fee > 0.0 => fees.push(format!("{} {}%", card.label, pyfmt_pct(fee))),
            Some(_) => {}
            None => unknown.push(card.label.clone()),
        }
    }
    let mut lines: Vec<String> = Vec::new();
    if !fees.is_empty() {
        lines.push(if zh {
            format!(
                "在{}以外发行的卡在这里会收境外交易手续费：{}（数据库）。",
                place(&market),
                fees.join("，")
            )
        } else {
            format!(
                "Cards issued outside {} add a foreign transaction fee here: {} (catalog).",
                place(&market),
                fees.join(", ")
            )
        });
    }
    if !unknown.is_empty() {
        lines.push(if zh {
            format!("{} 的境外交易手续费不在数据库里，请以发卡行条款为准。", unknown.join("、"))
        } else {
            format!(
                "The catalog does not know the foreign transaction fee of {}; check the issuer's terms.",
                unknown.join(", ")
            )
        });
    }
    lines
}

/// A percentage as written: 3.25, 2.7, 3 (no trailing zeros).
fn pyfmt_pct(v: f64) -> String {
    let s = format!("{v:.2}");
    s.trim_end_matches('0').trim_end_matches('.').to_string()
}

/// A Vault card as this run saw it: its label, its catalog id (`none`: checked, no row; "": not matched yet) and its
/// issuing country.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SeenCard {
    pub label: String,
    pub card_id: String,
    pub country: String,
}

/// The user's cards as this run saw them: the `card` items of `vault_list` results (the listing's fixed fields:
/// `card_id=`, `issued in`) and the cards `vault_add_card` added or found (its JSON arguments: name, country, card_id —
/// with the card_id the tool kept). A card seen twice keeps the fields either sighting gave.
pub fn vault_cards(transcript: &[Value]) -> Vec<SeenCard> {
    let mut out: Vec<SeenCard> = Vec::new();
    let mut add = |c: SeenCard| match out
        .iter_mut()
        .find(|x| x.label.eq_ignore_ascii_case(&c.label))
    {
        Some(x) => {
            if x.card_id.is_empty() {
                x.card_id = c.card_id;
            }
            if x.country.is_empty() {
                x.country = c.country;
            }
        }
        None => out.push(c),
    };
    let ok: HashSet<String> = transcript
        .iter()
        .filter(|m| get_str(m, "role") == "tool" && !content_of(m).starts_with("ERROR"))
        .map(|m| get_str(m, "tool_call_id"))
        .collect();
    for m in transcript {
        if get_str(m, "role") == "tool" {
            for c in crate::agent::local::listed_vault_cards(&content_of(m)) {
                add(c);
            }
        }
        for tc in tool_calls(m) {
            let (name, args) = stored_call(tc);
            if name == "vault_add_card" && ok.contains(&get_str(tc, "id")) {
                let card_id = sget(&args, "card_id");
                add(SeenCard {
                    label: sget(&args, "name").trim().to_string(),
                    card_id: if card_id == "none" || catalog_card(&card_id).is_some() {
                        card_id
                    } else {
                        String::new()
                    },
                    country: sget(&args, "country").trim().to_string(),
                });
            }
        }
    }
    out.retain(|c| !c.label.is_empty());
    out
}

/// A compact table of the user's cards from the catalog (its structured rows: market, base rate, dining, travel,
/// foreign transaction fee, verified), as a fact the reply must carry, when a card run (a catalog table read) knows two or
/// more of the user's cards — the comparison the pick is made against, rendered from data (cards_markets_real
/// 2026-10-30: "Use your Hang Seng Travel+", nothing about the user's two other cards).
pub fn card_comparison_fact(transcript: &[Value], zh: bool) -> Option<String> {
    let cards = vault_cards(transcript);
    if !card_run(transcript) || cards.len() < 2 {
        return None;
    }
    let rate = |c: &Value, k: &str| {
        c["rules"]
            .get(k)
            .and_then(Value::as_f64)
            .map(pyfmt_pct)
            .unwrap_or_else(|| "—".into())
    };
    let rows: Vec<String> = cards
        .iter()
        .filter_map(|card| {
            let c = catalog_card(&card.card_id)?;
            let fx = c
                .get("fx_fee_pct")
                .and_then(Value::as_f64)
                .map(|f| format!("{}%", pyfmt_pct(f)))
                .unwrap_or_else(|| "?".into());
            let verified = if c.get("verified").and_then(Value::as_bool) == Some(true) {
                sget(c, "verified_at")
            } else {
                "seed".to_string()
            };
            Some(format!(
                "| {} | {} | {} | {} | {} | {} | {fx} | {verified} |",
                card.label,
                sget(c, "region"),
                sget(c, "unit"),
                rate(c, "other"),
                rate(c, "dining"),
                rate(c, "travel"),
            ))
        })
        .collect();
    if rows.len() < 2 {
        return None;
    }
    let head = if zh {
        "**你的卡（数据库）**\n\n| 卡 | 市场 | 单位 | 基础 | 餐饮 | 旅行 | 境外手续费 | 核实 |\n|---|---|---|---|---|---|---|---|"
    } else {
        "**Your cards (catalog)**\n\n| Card | Market | Unit | Base | Dining | Travel | FX fee | Verified |\n|---|---|---|---|---|---|---|---|"
    };
    Some(format!("{head}\n{}", rows.join("\n")))
}

/// Which of the user's cards the card catalog lacks, as facts the reply must carry: when a run read a catalog table, a
/// Vault card this run saw (`vault_list`, `vault_add_card`) whose catalog id is `none` (checked: no row) gets
/// "<card> is not in the card catalog; its terms were not verified there." — a rate given for it was looked up or
/// assumed, and the user must know (cards_real 2026-10-30: "assuming it earns 2%"). Decided from the cards' `card_id`
/// and the shipped catalog, never from the words of a name.
pub fn uncatalogued_cards_facts(transcript: &[Value], zh: bool) -> Vec<String> {
    if !card_run(transcript) {
        return Vec::new();
    }
    vault_cards(transcript)
        .into_iter()
        .filter(|c| c.card_id == "none")
        .map(|c| {
            if zh {
                format!("{} 不在信用卡权益数据库里，它的条款没有核实。", c.label)
            } else {
                format!(
                    "{} is not in the card catalog; its terms were not verified there.",
                    c.label
                )
            }
        })
        .collect()
}

/// What a scheduled run notified the user (`notify_user` that went through), as facts the reply must carry — a run's
/// Results must not contradict the 🔔 the user got (2026-10-30, assistant_real: a summary of 2 emails notified, the
/// answer "No new emails have arrived").
pub fn notification_facts(transcript: &[Value], zh: bool) -> Vec<String> {
    let sent: HashSet<String> = transcript
        .iter()
        .filter(|m| get_str(m, "role") == "tool" && !content_of(m).starts_with("ERROR"))
        .map(|m| get_str(m, "tool_call_id"))
        .collect();
    let mut lines = Vec::new();
    for tc in transcript.iter().flat_map(tool_calls) {
        let (name, args) = stored_call(tc);
        if name != "notify_user" || !sent.contains(&get_str(tc, "id")) {
            continue;
        }
        let title = sget(&args, "title");
        let msg = sget(&args, "message");
        if title.trim().is_empty() {
            continue;
        }
        lines.push(if zh {
            format!("🔔 已通知你：{title} — {}", take_chars(&msg, 400))
        } else {
            format!("🔔 Notified you: {title} — {}", take_chars(&msg, 400))
        });
    }
    lines
}

/// The amount the user approved, with what it was for, as facts the reply must carry: every purchase approved in the
/// run (`purchase_confirm` → `"status": "approved"`) — whatever happened next, the user must see what they agreed to
/// pay (travel_michelin_e2e 2026-10-30: the table was booked with a JPY 28,000 deposit and the answer gave only the
/// booking reference).
pub fn purchase_total_facts(transcript: &[Value], zh: bool) -> Vec<String> {
    let approved: HashSet<String> = transcript
        .iter()
        .filter(|m| {
            get_str(m, "role") == "tool" && content_of(m).contains("\"status\": \"approved\"")
        })
        .map(|m| get_str(m, "tool_call_id"))
        .collect();
    let mut lines = Vec::new();
    for tc in transcript.iter().flat_map(tool_calls) {
        let (name, args) = stored_call(tc);
        if name != "purchase_confirm" || !approved.contains(&get_str(tc, "id")) {
            continue;
        }
        let Some(total) = args.get("total").and_then(omuse_common::util::as_f64) else {
            continue;
        };
        let what: Vec<String> = arr(&args, "items")
            .iter()
            .map(|x| sget(x, "name"))
            .filter(|x| !x.is_empty())
            .collect();
        let cur = sget(&args, "currency");
        let amount = if total.fract() == 0.0 {
            fmt_thousands(&format!("{total:.0}"))
        } else {
            let s = format!("{total:.2}");
            let (i, f) = s.split_once('.').unwrap_or((&s, "00"));
            format!("{}.{f}", fmt_thousands(i))
        };
        let what = if what.is_empty() {
            String::new()
        } else {
            format!(" ({})", what.join("; "))
        };
        let line = if zh {
            format!("你批准的金额：{cur} {amount}{what}")
        } else {
            format!("Amount you approved: {cur} {amount}{what}")
        };
        if !lines.contains(&line) {
            lines.push(line);
        }
    }
    lines
}

/// `"28000"` → `"28,000"`.
fn fmt_thousands(int: &str) -> String {
    let (sign, d) = int.strip_prefix('-').map_or(("", int), |d| ("-", d));
    let mut out = String::new();
    for (i, c) in d.chars().enumerate() {
        if i > 0 && (d.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    format!("{sign}{out}")
}

/// The travel searches, which share ONE search per request (the user's message) and per model turn: `flights_search`,
/// `hotels_search`, `cars_search`, `places_search`, or a travel connector's `mcp_<server>__…` of the same name — what
/// it searches (`(English, 中文)`), `None` for any other tool. Web and news search are not counted.
pub fn travel_search(name: &str) -> Option<(&'static str, &'static str)> {
    match name.rsplit("__").next().unwrap_or(name) {
        "flights_search" => Some(("flights", "航班")),
        "hotels_search" => Some(("hotels", "酒店")),
        "cars_search" => Some(("rental cars", "租车")),
        "places_search" => Some(("places", "餐厅和地点")),
        _ => None,
    }
}

/// The flight and hotel searches only the booking machine calls in a bot that has the workflow tools
/// (`travel_search` / `travel_book` run them with the model's JSON arguments, agent/booking.rs): they are not offered to
/// the model there, and its own call to one — a connector's of the same name too — is refused, naming the workflow
/// tool. A bot without the workflow tools (Insurance: the alternatives of a disrupted flight) keeps them.
pub const MACHINE_ONLY: [&str; 4] = [
    "flights_search",
    "flight_offer",
    "hotels_search",
    "hotel_rooms",
];

/// Whether `name` (or a connector's `mcp_<server>__<name>`) is one of [`MACHINE_ONLY`].
pub fn machine_only(name: &str) -> bool {
    MACHINE_ONLY.contains(&name.rsplit("__").next().unwrap_or(name))
}

/// The model's call to a machine-only search in a bot with the workflow tools (`workflow_bot`), refused as data (a
/// code machine's own call goes through).
pub fn machine_only_guard(call: &ToolCall, workflow_bot: bool) -> Option<String> {
    if !workflow_bot || code_call(call) || !machine_only(&call.name) {
        return None;
    }
    Some(
        json!({"refused": "machine_only", "tool": call.name, "use": "travel_search",
               "rule": "flights and hotels are searched by ONE travel_search call (travel_book to book) with the request's details as its arguments"})
        .to_string(),
    )
}

/// One travel search per request: a travel search in a task that already ran one (a successful result in its events —
/// a call that failed, e.g. on a bad date, may be corrected) is refused, naming that search.
pub fn one_search_guard(events: impl FnOnce() -> Vec<Value>, call: &ToolCall) -> Option<String> {
    travel_search(&call.name)?;
    // the booking machine's own calls: it searches once, and the user's connector once more only when the first search
    // gave nothing priced (booking.rs)
    if call.id.starts_with("bkg_") {
        return None;
    }
    let done = events().into_iter().find(|e| {
        get_str(e, "type") == "tool_result"
            && e["data"]["ok"].as_bool() == Some(true)
            && travel_search(&get_str(&e["data"], "name")).is_some()
            && get_str(&e["data"], "call_id") != call.id
    })?;
    let first = get_str(&done["data"], "call_id");
    let (what, zh) = travel_search(&get_str(&done["data"], "name")).unwrap_or(("travel", "旅行"));
    Some(format!(
        "ERROR: [one_search] 已拦截 — 这个请求已经做过它唯一的一次旅行搜索（调用 {first}，{zh}）。 Blocked: this request already \
ran its one travel search (call {first}: {what}); ONE travel search per request."
    ))
}

/// Refusal text if the call goes to a web host that already failed HOST_FAIL_MAX times in this task.
/// `events` is read only when the call has a URL.
pub fn host_guard(events: impl FnOnce() -> Vec<Value>, call: &ToolCall) -> Option<String> {
    let host = if call.args.is_object() {
        host_of(call.args.get("url").unwrap_or(&Value::Null))
    } else {
        String::new()
    };
    if host.is_empty() {
        return None;
    }
    let n = source_failures(&events()).get(&host).copied().unwrap_or(0);
    if n < HOST_FAIL_MAX {
        return None;
    }
    Some(format!(
        "ERROR: [dead_host] 已拦截 — {host} 在本任务里已经失败或被拦截 {n} 次，不再执行。 Blocked: {host} already failed or was \
refused {n} times in this task and is not called again."
    ))
}

/// `str(args.get("output") or args.get("title") or "").strip().lower()`.
pub fn out_key(args: &Value) -> String {
    let v = args
        .get("output")
        .filter(|v| truthy(v))
        .or_else(|| args.get("title").filter(|v| truthy(v)));
    v.map(omuse_common::util::py_str)
        .unwrap_or_default()
        .trim()
        .to_lowercase()
}

static PARKED: LazyLock<Regex> = LazyLock::new(|| {
    rx(
        r"(?i)(this|the) domain (name )?(\S+ )?(is|may be) for sale|\S+\.\w+ is for sale|domain is for sale|buy this domain|is parked free|parked (domain|by|courtesy)|sedoparking|domain parking|godaddy\.com/domain|域名.{0,6}(出售|转让)",
    )
});

/// A page that is a parked / for-sale domain, not the company's site (2026-10-04 V3-07: "scoot.com" instead of
/// flyscoot.com): the runtime then searches for the official site and opens it itself (`exec::official_site`).
pub fn parked_domain(title: &str, snapshot: &str) -> bool {
    PARKED.is_match(&format!("{}\n{}", title, take_chars(snapshot, 3000)))
}

fn norm_goal(s: &str) -> String {
    static R: LazyLock<Regex> = LazyLock::new(|| rx(r"[\s\W_]+"));
    take_chars(&R.replace_all(&s.to_lowercase(), ""), 300).to_string()
}

/// An enabled schedule doing the same job (same name, or the same goal text).
pub fn same_schedule(schedules: &[Value], name: &str, goal: &str) -> Option<Value> {
    let ng = norm_goal(goal);
    for s in schedules {
        if !get_truthy(s, "enabled") {
            continue;
        }
        if (!name.is_empty() && get_str(s, "name") == take_chars(name, 80))
            || (ng.chars().count() >= 20 && norm_goal(&sget(s, "goal")) == ng)
        {
            return Some(s.clone());
        }
    }
    None
}

fn is_system_user(m: &Value) -> bool {
    let c = content_of(m);
    c.starts_with("(System)") || c.starts_with("（系统）")
}

/// 2026-10-02 R7-02: the model wrote the full answer (a table of bills) as text next to an update_plan call, then
/// finished with a short closing line — the chat only shows the final message, so the table was lost. When the final
/// answer is short (a measured length, not its wording), the last substantial mid-task text goes in front of it.
pub fn merge_stranded_answer(transcript: &[Value], final_: &str) -> String {
    let f = final_.trim();
    if f.chars().count() > 300 {
        return final_.to_string();
    }
    for m in transcript.iter().rev() {
        if get_str(m, "role") == "user" && !is_system_user(m) {
            break; // only look inside this run
        }
        let txt = if get_str(m, "role") == "assistant" && m.get("tool_calls").is_some_and(truthy) {
            content_of(m).trim().to_string()
        } else {
            String::new()
        };
        if txt.chars().count() >= 300 && !f.contains(&txt) && !f.contains(take_chars(&txt, 160)) {
            // V2-10: same answer twice when only the end differed
            return format!("{}\n\n{}", txt, f);
        }
    }
    final_.to_string()
}

const CC_SLD: [&str; 10] = [
    "co", "com", "net", "org", "gov", "ac", "edu", "or", "ne", "go",
];

/// opentable.com / opentable.sg / m.opentable.sg → 'opentable' (one site, many domains).
pub fn site_brand(url: &str) -> String {
    let u = if url.contains("://") {
        url.to_string()
    } else {
        format!("https://{}", url)
    };
    let host = urlparse(&u)
        .ok()
        .and_then(|p| p.hostname())
        .unwrap_or_default()
        .to_lowercase();
    let host = host.trim_matches('.').to_string();
    let parts: Vec<&str> = host.split('.').filter(|p| !p.is_empty()).collect();
    if parts.len() >= 3
        && parts[parts.len() - 1].len() == 2
        && CC_SLD.contains(&parts[parts.len() - 2])
    {
        return parts[parts.len() - 3].to_string();
    }
    if parts.len() >= 2 {
        parts[parts.len() - 2].to_string()
    } else {
        host
    }
}

/// The site of `url` showed a bot wall earlier in this task and the user hasn't taken over since → its brand.
pub fn site_blocked(transcript: &[Value], url: &str) -> String {
    let brand = site_brand(url);
    if brand.is_empty() {
        return String::new();
    }
    let mark = format!("[SITE BLOCKED site={}]", brand);
    for m in transcript.iter().rev() {
        let c = content_of(m);
        let is_tool = get_str(m, "role") == "tool";
        if is_tool && (c.contains(TAKEOVER_DONE_MARK) || c.contains(TAKEOVER_DONE_EN)) {
            return String::new();
        }
        if is_tool && c.starts_with(&mark) {
            return brand;
        }
    }
    String::new()
}

/// The `ask_user` call of the run (the newest whose result went through): the fields asked for and the question.
/// Read from the tool call and its structured result, never from the model's prose.
pub fn asked_user(transcript: &[Value]) -> Option<(Vec<String>, String)> {
    let ids: Vec<String> = transcript
        .iter()
        .flat_map(tool_calls)
        .filter(|tc| stored_call(tc).0 == "ask_user")
        .map(|tc| get_str(tc, "id"))
        .collect();
    transcript
        .iter()
        .rev()
        .filter(|m| get_str(m, "role") == "tool" && ids.contains(&get_str(m, "tool_call_id")))
        .find_map(|m| {
            let v: Value = serde_json::from_str(content_of(m).trim()).ok()?;
            if !get_truthy(&v, "asked") {
                return None;
            }
            let fields = arr(&v, "fields")
                .iter()
                .map(omuse_common::util::py_str)
                .collect();
            Some((fields, sget(&v, "question")))
        })
}

/// The search tools whose results are a listing for the user (flights, hotels, cars, places, the MICHELIN guide's
/// restaurants), by their base name (an MCP server's prefix dropped).
fn listing_search(name: &str) -> bool {
    let base = name.rsplit("__").next().unwrap_or(name);
    travel_search(base).is_some() || matches!(base, "michelin_search" | "restaurants_search")
}

/// `ask_user` refused while a listing search's results are in the run and not shown yet: a request for options is
/// answered with the options first (the answer or `present_choices`); what only matters for a booking (party size, a
/// time) is asked after the user has seen them. Decided from the task's `tool_result` events (the tool, whether it
/// went through, in order), never from the question's words. `Some(refusal)`: the structured refusal to return.
/// travel_ask_real 2026-11-01: "What are my options?" for a 3-star restaurant in Tokyo got "How many people …?" with
/// the guide's results in hand.
pub fn ask_before_listing(events: &[Value]) -> Option<Value> {
    let mut last_search: Option<(usize, String)> = None;
    let mut last_shown: Option<usize> = None;
    for (i, e) in events.iter().enumerate() {
        if get_str(e, "type") != "tool_result" || e["data"]["ok"].as_bool() != Some(true) {
            continue;
        }
        let name = get_str(&e["data"], "name");
        if listing_search(&name) {
            last_search = Some((i, name.rsplit("__").next().unwrap_or(&name).to_string()));
        } else if name == "present_choices" {
            last_shown = Some(i);
        }
    }
    let (at, tool) = last_search?;
    if last_shown.is_some_and(|s| s > at) {
        return None;
    }
    Some(json!({"refused": "results_not_shown", "search": tool,
                "rule": "the user asked for options: show the results of the search first (the answer, or present_choices); anything only a booking needs is asked after they have seen them"}))
}

/// The fields `ask_user` may name.
pub const ASK_FIELDS: [&str; 15] = [
    "flight",
    "arrival_date",
    "stay",
    "purpose",
    "which_address",
    "cards_held",
    "trip_dates",
    "nationality",
    "birthday",
    "home_address",
    "sex",
    "departure_date",
    "next_city",
    "declarations",
    "other",
];

/// What the profile or the form's defaults already answer for an `ask_user` field: `Some((source, value))` when the
/// question must not be asked — the profile's nationality, birthday, home address or sex; an arrival card's departure
/// date (arrival + 7 days), next city (where the flight came from) and health / declaration answers (No); the one
/// profile address in the card's country. `None`: a question only the user can answer.
pub fn known_answer(
    field: &str,
    profile: &Value,
    transcript: &[Value],
) -> Option<(&'static str, String)> {
    let prof = |k: &str| {
        let v = get_str(profile, k);
        (!v.trim().is_empty()).then(|| ("profile", v.trim().to_string()))
    };
    match field {
        "nationality" => prof("nationality"),
        "birthday" => prof("birthday"),
        "home_address" => prof("address_home"),
        "sex" => prof("gender"),
        "departure_date" => arrival_run(transcript).then(|| {
            (
                "default",
                "the arrival date + 7 days (the user's default)".to_string(),
            )
        }),
        "next_city" => arrival_run(transcript).then(|| {
            (
                "default",
                "the city the flight came from (the user's default)".to_string(),
            )
        }),
        "declarations" => arrival_run(transcript).then(|| {
            (
                "default",
                "No to every health and declaration question (the user's default)".to_string(),
            )
        }),
        "which_address" => {
            let skill = form_kind_skills(transcript, "arrival").into_iter().next()?;
            let country = arrival_country(&skill);
            let there: Vec<String> = profile
                .as_object()?
                .iter()
                .filter(|(k, _)| k.starts_with("address"))
                .map(|(_, v)| omuse_common::util::py_str(v))
                .filter(|v| country.iter().any(|w| v.contains(w)))
                .collect();
            (there.len() == 1).then(|| ("profile", there[0].clone()))
        }
        _ => None,
    }
}

/// Tool arguments for the event log: long strings cut to 300 characters.
pub fn preview_args(a: &Value) -> Value {
    let mut out = Map::new();
    if let Some(o) = a.as_object() {
        for (k, v) in o {
            out.insert(
                k.clone(),
                match v {
                    Value::String(s) => Value::String(truncate(s, 300)),
                    other => other.clone(),
                },
            );
        }
    }
    Value::Object(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_question_before_a_listing_is_shown() {
        // travel_ask_real on b24962d: michelin_search returned the 3-star restaurant, then ask_user("How many people?")
        let ev = |name: &str, ok: bool| json!({"type": "tool_result", "data": {"call_id": "c", "name": name, "ok": ok}});
        let mut events = vec![ev("load_skill", true)];
        assert_eq!(ask_before_listing(&events), None);
        events.push(ev("mcp_travel__michelin_search", true));
        let r = ask_before_listing(&events).unwrap();
        assert_eq!(
            (sget(&r, "refused").as_str(), sget(&r, "search").as_str()),
            ("results_not_shown", "michelin_search")
        );
        // shown as cards: a question may follow
        events.push(ev("present_choices", true));
        assert_eq!(ask_before_listing(&events), None);
        // a choice card refused (not shown) does not count
        let refused = vec![ev("flights_search", true), ev("present_choices", false)];
        assert!(ask_before_listing(&refused).is_some());
        // a failed search holds no results; a mail search is not a listing
        assert_eq!(ask_before_listing(&[ev("flights_search", false)]), None);
        assert_eq!(ask_before_listing(&[ev("gmail_search", true)]), None);
    }
}
