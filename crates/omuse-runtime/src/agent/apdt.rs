//! The Hong Kong air passenger departure tax refund behind the workflow tool `apdt_refund` (`hk-apdt-refund`;
//! docs/json-tools.md, the protocol is [`super::workflow`]).
//!
//! The model reads the user's words — and the boarding passes and the receipt the machine fetched — and calls the
//! tool with JSON arguments: the flights into and out of Hong Kong with their dates, the documents' paths, the
//! departure tax the receipt shows, the refund method (and the e-wallet account). [`check`] validates them in code
//! (flight numbers, dates, the files are this chat's, the method is the platform's) and decides what happens before
//! the platform opens: the fixed mail search when the trip is not known yet (the machine saves the attachments and reads
//! them; its result `mail_read` has the model call again with the facts in them), `needs_input` for what only the user
//! can give (the trip, the refund method). Nothing here reads the user's wording, a mail's text or a document's text.
//!
//! Then every step is code ([`next`]): the eligibility rule (left Hong Kong the same or the next calendar day, within
//! 28 days of the departure flight, after departing, the tax paid), then the real platform (a Vue app behind a Queue-it
//! waiting room, mapped 2026-11-02 with the user's own documents) through its notes, "Start", "New Application" and the
//! entry mode, then uploads the documents of this chat into their slots, by its form function (`crate::forms`) — never a
//! made-up file. The recognised travel document saves the application under a reference number, read from its fixed
//! element, which opens the application on any device ("View Application / Continue Application"): no hand-over (the
//! user, 2026-11-02). The refund method, the face recognition and Submit are the user's (`guards::apdt_face_guard`).
//! The machine ends with [`Step::Done`]: a JSON result (status, the rule's verdict, the amount, the deadline, the
//! reference number, the steps left and the link — as `show`, appended to the reply by code — and `hints`); the model
//! writes the reply from it.

use std::sync::LazyLock;

use chrono::{Duration, NaiveDate};
use omuse_common::util::{get_str, truthy};
use regex::Regex;
use serde_json::{json, Map, Value};

use super::driver::{json_of, norm_flight, results_of, stopped, walk_messages, Step};
use super::show as S;
use super::tools::fn_;
use super::util::{content_of, sget, stored_call, tool_calls};
use super::workflow::{self, Checked};
use crate::llm::ToolCall;

/// The refund application, as data.
pub struct ApdtSpec {
    pub skill: &'static str,
    pub name: &'static str,
    /// the platform's start page
    pub start_url: &'static str,
    pub host: &'static str,
    /// the fixed mail search (quoted phrases: an unquoted "boarding pass OR receipt" reads as boarding AND (pass OR
    /// receipt) and misses the receipt — apdt_real 2026-11-01)
    pub search: &'static str,
    /// days after the departure flight to apply
    pub window_days: i64,
}

pub const APDT: ApdtSpec = ApdtSpec {
    skill: "hk-apdt-refund",
    name: "Hong Kong departure tax refund",
    start_url: "https://app.apdtrefund.hk/hkTaxMobile/?lang=en",
    host: "app.apdtrefund.hk",
    search: "(\"boarding pass\" OR e-ticket OR receipt OR itinerary) (\"Hong Kong\" OR HKG) newer_than:40d",
    window_days: 28,
};

/// The id prefix of the machine's code calls.
pub const PREFIX: &str = "apd_";
pub const APDT_REFUND: &str = "apdt_refund";
/// The workflow tools this machine runs.
pub const TOOLS: [&str; 1] = [APDT_REFUND];
/// The result that has the model call the tool again: the documents were fetched and read, and are in the transcript
/// for it to read (`workflow::CALL_AGAIN`).
pub const MAIL_READ: &str = super::driver::MAIL_READ;
/// The platform's home: where "Resume Application" is.
const HOME: &str = "https://app.apdtrefund.hk/hkTaxMobile/?lang=en";

fn call(n: usize, name: &str, args: Value) -> ToolCall {
    workflow::code_call(PREFIX, n, name, args)
}

fn failed(m: &Value) -> bool {
    let c = content_of(m);
    let head = c.trim_start();
    head.starts_with("ERROR") || head.starts_with("DENIED")
}

/// The catalog entry of the workflow tool.
pub fn tool_schemas() -> Vec<Value> {
    let date = |what: &str| json!({"type": "string", "description": format!("YYYY-MM-DD: {what}")});
    vec![fn_(
        APDT_REFUND,
        "The Hong Kong air passenger departure tax (APDT) refund on the Airport Authority's platform. By default (file \
false) code answers who can claim, the deadline, the documents and the official link — nothing is opened. file true \
ONLY when the user explicitly asks you to apply for them: then call it with the flights the user gave (or the boarding passes and the receipt show) and the \
refund method, and the documents of this chat (the passport's photo page, the boarding passes, the receipt): code \
uploads them, which saves the application under a reference number that opens it on any device. When \
the flights are not given and a mailbox is connected, code fetches the documents (status mail_read: read the boarding \
passes and the receipt just read and call again with the flights). Code checks the rule (left Hong Kong the same or the next day, \
within 28 days, the tax paid), opens the platform, goes through its notes and entry mode and uploads the documents; \
the refund method, the face recognition and Submit are the user's. The JSON result has the rule's verdict, the \
deadline, the amount, the link and the reference number (status saved; ready without a travel document) or says why \
not. status \
needs_input: ask the user for the missing fields, then call again with every field; invalid: fix the arguments and call \
again. Write the reply from the result (code appends the facts in show to the reply).",
        json!({
            "file": {"type": "boolean", "description": "true when the user asks you to do it — apply, start or prepare the application for them (even when they will upload, check or submit something themselves); false for a question about the refund or how to get it"},
            "entry_mode": {"type": "string", "enum": ["air", "land_sea", "skypier"], "description": "how the user entered Hong Kong, from the conversation: air (a flight — then also in_flight, in_date and the arrival boarding pass), land_sea (a land or sea control point such as Shenzhen Bay), skypier (the SkyPier ferry / coach or Park & Fly)"},
            "in_flight": {"type": "string", "description": "the flight INTO Hong Kong: airline code and number (\"SQ 890\"); leave it out when the user did not give it (they add it on the platform)"},
            "in_date": date("the day that flight arrived in Hong Kong"),
            "out_flight": {"type": "string", "description": "the flight OUT of Hong Kong (the departure the tax was paid on)"},
            "out_date": date("the day that flight left Hong Kong"),
            "tax_hkd": {"type": "integer", "description": "the Hong Kong Air Passenger Departure Tax the receipt shows (120 or 200); 0 when the receipt shows none"},
            "refund_method": {"type": "string", "enum": ["card", "cash", "alipay", "wechat", "alipayHK", "wechatHK"], "description": "how the user wants the refund"},
            "wallet_account": {"type": "string", "description": "the e-wallet account the refund goes to (Alipay / WeChat Pay)"},
            "passport_file": {"type": "string", "description": "the travel document's photo page: its path in this chat (uploads/… or attachments/…); it saves the application under a reference number"},
            "out_pass_file": {"type": "string", "description": "the departure flight's boarding pass: its path in this chat"},
            "in_pass_file": {"type": "string", "description": "the arrival flight's boarding pass: its path in this chat"},
            "receipt_file": {"type": "string", "description": "the ticket or e-ticket receipt showing the departure tax: its path in this chat"}
        }),
        &[],
    )]
}

/// `ask_user` in the refund's run before `apdt_refund` was called: refused, as data (its `needs_input` decides what
/// to ask). `events` are the task's.
pub fn ask_before_tool(events: &[Value]) -> Option<Value> {
    let loaded = events
        .iter()
        .filter(|e| get_str(e, "type") == "tool_call")
        .filter(|e| get_str(&e["data"], "name") == "load_skill")
        .any(|e| get_str(&e["data"]["args"], "name") == APDT.skill);
    let called = events.iter().any(|e| {
        get_str(e, "type") == "workflow_call" && get_str(&e["data"], "tool") == APDT_REFUND
    });
    (loaded && !called).then(|| {
        json!({"refused": "call_the_tool_first", "tool": APDT_REFUND,
               "why": format!("the {} skill asks through {APDT_REFUND}: call it with what the user gave; its needs_input lists what only the user can give", APDT.skill)})
    })
}

// ---------------------------------------------------------------- the files of the chat

/// The files the user attached in this chat (the `uploads/…` paths the attachment notes carry).
pub fn user_files(transcript: &[Value]) -> Vec<String> {
    static UP: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r#"uploads/[^\s,;()\[\]"'<>]+"#).unwrap());
    let mut out: Vec<String> = Vec::new();
    for m in transcript.iter().filter(|m| get_str(m, "role") == "user") {
        for f in UP.find_iter(&content_of(m)) {
            let p = f.as_str().trim_end_matches(['.', ':']).to_string();
            if !out.contains(&p) {
                out.push(p);
            }
        }
    }
    out
}

fn is_image(p: &str) -> bool {
    let l = p.to_lowercase();
    l.ends_with(".jpg") || l.ends_with(".jpeg") || l.ends_with(".png")
}

fn is_doc_file(p: &str) -> bool {
    is_image(p) || p.to_lowercase().ends_with(".pdf")
}

/// The default passport photo: the user's only picture in this chat.
pub fn passport_default(files: &[String]) -> Option<String> {
    let images: Vec<&String> = files.iter().filter(|f| is_image(f)).collect();
    (images.len() == 1).then(|| images[0].clone())
}

/// The attachments of the fetched mail (a PDF or a picture, by the API's file names): (message id, file name, the
/// mail's subject).
fn mail_attachments(transcript: &[Value]) -> Vec<(String, String, String)> {
    let mut out = Vec::new();
    for (a, m) in results_of(transcript, "gmail_get_message") {
        let v = json_of(m);
        let id = sget(&a, "message_id");
        for att in v
            .get("attachments")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            let name = if att.is_object() {
                get_str(att, "filename")
            } else {
                att.as_str().unwrap_or("").to_string()
            };
            if is_doc_file(&name) && !out.iter().any(|(i, n, _)| *i == id && *n == name) {
                out.push((id.clone(), name, get_str(&v, "subject")));
            }
        }
    }
    out
}

/// The attachments saved into the workspace: (path, the subject of the mail it came with).
fn saved(transcript: &[Value]) -> Vec<(String, String)> {
    static SAVED: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"saved: (\S+)").unwrap());
    let atts = mail_attachments(transcript);
    results_of(transcript, "gmail_save_attachment")
        .into_iter()
        .filter(|(_, m)| !failed(m))
        .filter_map(|(a, m)| {
            let path = SAVED.captures(&content_of(m))?[1].to_string();
            let subject = atts
                .iter()
                .find(|(i, n, _)| *i == sget(&a, "message_id") && *n == sget(&a, "filename"))
                .map(|(_, _, s)| s.clone())
                .unwrap_or_default();
            Some((path, subject))
        })
        .collect()
}

/// The files of this chat a document argument may name: the user's attachments and the mail's saved attachments.
fn known_files(transcript: &[Value]) -> Vec<String> {
    let mut out = user_files(transcript);
    for (p, _) in saved(transcript) {
        if !out.contains(&p) {
            out.push(p);
        }
    }
    out
}

// ---------------------------------------------------------------- validation (code, on the JSON)

/// What validation reads besides the arguments.
pub struct Ctx<'a> {
    pub profile: &'a Value,
    pub today: NaiveDate,
    /// a mailbox is connected
    pub mail_ready: bool,
    /// the chat's transcript: its files, the earlier calls' arguments, whether the documents were already fetched
    pub transcript: &'a [Value],
}

const KEYS: &[&str] = &[
    "entry_mode",
    "in_flight",
    "in_date",
    "out_flight",
    "out_date",
    "in_pass_file",
    "out_pass_file",
    "receipt_file",
    "tax_hkd",
    "passport_file",
    "refund_method",
    "wallet_account",
];
const FILES: &[&str] = &[
    "in_pass_file",
    "out_pass_file",
    "receipt_file",
    "passport_file",
];
/// How the user entered Hong Kong, as the platform's three tiles (the model reads it from the conversation).
const MODES: &[&str] = &["air", "land_sea", "skypier"];
const METHODS: &[&str] = &["card", "cash", "alipay", "wechat", "alipayHK", "wechatHK"];

fn s_arg(a: &Value, k: &str) -> String {
    match a.get(k) {
        Some(Value::String(s)) => s.trim().to_string(),
        Some(Value::Number(n)) => n.to_string(),
        _ => String::new(),
    }
}

fn date_of(v: &str) -> Option<NaiveDate> {
    NaiveDate::parse_from_str(v.trim(), "%Y-%m-%d").ok()
}

fn long(d: NaiveDate) -> String {
    d.format("%-d %B %Y").to_string()
}

/// The arguments of the earlier calls in this chat, merged (a later value wins), under `args`.
fn with_earlier(args: &Value, transcript: &[Value]) -> Value {
    let mut merged = Map::new();
    let calls = transcript
        .iter()
        .flat_map(tool_calls)
        .map(stored_call)
        .filter(|(n, _)| n == APDT_REFUND)
        .map(|(_, a)| a)
        .chain(std::iter::once(args.clone()));
    for a in calls {
        for k in KEYS {
            let v = s_arg(&a, k);
            if !v.is_empty() {
                merged.insert((*k).into(), json!(v));
            }
        }
    }
    Value::Object(merged)
}

/// The documents asked once in the conversation, all optional: what the user skips they upload later in the saved
/// application (the user, 2026-11-02: "have a conversation with the user to collect as much as possible").
const DOCS_ASK: &str = "the documents still missing, if the user has them (optional — anything skipped is uploaded later \
in the saved application), attached in this chat:";

/// The documents a call may give, as the user is asked for them.
const DOC_NAMES: &[(&str, &str)] = &[
    ("passport_file", "the passport's photo page (passport_file; it saves the application under a reference number)"),
    ("out_pass_file", "the departure flight's boarding pass (out_pass_file)"),
    ("in_pass_file", "the arrival flight's boarding pass (in_pass_file)"),
    ("receipt_file", "the ticket receipt showing the departure tax (receipt_file)"),
];

/// The start of the one-time send-back when the user attached files and the call names none.
const MAP_FILES: &str = "the user attached these files in this chat";

/// Whether an earlier `apdt_refund` result in this chat holds a line starting with `start` (its `missing` or
/// `problems`): what the tool already said once is not said again.
fn said_before(transcript: &[Value], start: &str) -> bool {
    transcript
        .iter()
        .flat_map(tool_calls)
        .filter(|tc| stored_call(tc).0 == APDT_REFUND)
        .filter_map(|tc| {
            let id = get_str(tc, "id");
            transcript
                .iter()
                .find(|m| get_str(m, "role") == "tool" && get_str(m, "tool_call_id") == id)
        })
        .any(|m| {
            let v = json_of(m);
            ["missing", "problems"].iter().any(|k| {
                v[*k].as_array().is_some_and(|a| {
                    a.iter()
                        .any(|x| x.as_str().is_some_and(|x| x.starts_with(start)))
                })
            })
        })
}

/// Whether an earlier `apdt_refund` call in this chat already asked for the documents ([`DOCS_ASK`]).
/// Whether an earlier `apdt_refund` call in this chat already asked the user (`needs_input`).
fn asked_once(transcript: &[Value]) -> bool {
    transcript
        .iter()
        .flat_map(tool_calls)
        .filter(|tc| stored_call(tc).0 == APDT_REFUND)
        .filter_map(|tc| {
            let id = get_str(tc, "id");
            transcript
                .iter()
                .find(|m| get_str(m, "role") == "tool" && get_str(m, "tool_call_id") == id)
        })
        .any(|m| get_str(&json_of(m), "status") == "needs_input")
}

fn docs_asked(transcript: &[Value]) -> bool {
    said_before(transcript, DOCS_ASK)
}

/// Whether this chat's documents were already fetched by the machine (a `mail_read` result).
fn docs_fetched(transcript: &[Value]) -> bool {
    let want = format!("{APDT_REFUND}{}", workflow::RESULT_SUFFIX);
    transcript
        .iter()
        .flat_map(tool_calls)
        .filter(|tc| {
            get_str(tc, "id").starts_with(workflow::RESULT_PREFIX) && stored_call(tc).0 == want
        })
        .filter_map(|tc| {
            let id = get_str(tc, "id");
            transcript
                .iter()
                .find(|m| get_str(m, "role") == "tool" && get_str(m, "tool_call_id") == id)
        })
        .any(|m| get_str(&json_of(m), "status") == MAIL_READ)
}

/// Validates the tool's arguments: the arguments to run with, the problems the model can fix, or the fields only the
/// user can give — before the platform opens.
pub fn check(tool: &str, args: &Value, cx: &Ctx) -> Checked {
    debug_assert_eq!(tool, APDT_REFUND);
    let a = with_earlier(args, cx.transcript);
    let mut problems: Vec<String> = Vec::new();
    let mut out = Map::new();
    for k in ["in_flight", "out_flight"] {
        let v = s_arg(&a, k);
        if v.is_empty() {
            continue;
        }
        match norm_flight(&v) {
            Some(f) => {
                out.insert(k.into(), json!(f));
            }
            None => problems.push(format!(
                "{k}: {v:?} is not an airline code and a number (\"SQ 890\")"
            )),
        }
    }
    for k in ["in_date", "out_date"] {
        let v = s_arg(&a, k);
        if v.is_empty() {
            continue;
        }
        match date_of(&v) {
            Some(d) if d > cx.today + Duration::days(366) => {
                problems.push(format!("{k}: {v} is too far ahead"))
            }
            Some(d) => {
                out.insert(k.into(), json!(d.format("%Y-%m-%d").to_string()));
            }
            None => problems.push(format!("{k}: {v:?} is not a YYYY-MM-DD date")),
        }
    }
    // the documents: files of this chat (a bare file name is matched to its path)
    let known = known_files(cx.transcript);
    for k in FILES {
        let v = s_arg(&a, k);
        if v.is_empty() {
            continue;
        }
        let base = |p: &str| p.rsplit('/').next().unwrap_or(p).to_lowercase();
        match known
            .iter()
            .find(|f| **f == v)
            .or_else(|| known.iter().find(|f| base(f) == base(&v)))
        {
            Some(f) => {
                out.insert((*k).into(), json!(f));
            }
            None if known.is_empty() => problems.push(format!("{k}: no file in this chat")),
            None => problems.push(format!(
                "{k}: {v:?} is not a file of this chat (one of {})",
                known.join(", ")
            )),
        }
    }
    let tax = s_arg(&a, "tax_hkd");
    if !tax.is_empty() {
        match tax.parse::<f64>() {
            Ok(x) if (0.0..1000.0).contains(&x) => {
                out.insert("tax_hkd".into(), json!(x.round() as u32));
            }
            _ => problems.push(format!("tax_hkd: {tax:?} is not an amount in HK$")),
        }
    }
    let method = s_arg(&a, "refund_method");
    if !method.is_empty() {
        match METHODS.iter().find(|m| m.eq_ignore_ascii_case(&method)) {
            Some(m) => {
                out.insert("refund_method".into(), json!(m));
            }
            None => problems.push(format!("refund_method: one of {}", METHODS.join(", "))),
        }
    }
    let wallet = s_arg(&a, "wallet_account");
    if !wallet.is_empty() {
        out.insert("wallet_account".into(), json!(wallet));
    }
    // "land / sea", "Land-Sea" → land_sea: separators folded, letters kept
    let mode = s_arg(&a, "entry_mode")
        .to_lowercase()
        .split(|c: char| !c.is_ascii_alphabetic())
        .filter(|w| !w.is_empty())
        .collect::<Vec<_>>()
        .join("_");
    if !mode.is_empty() {
        match MODES.iter().find(|m| **m == mode) {
            Some(m) => {
                out.insert("entry_mode".into(), json!(m));
            }
            None => problems.push(format!("entry_mode: one of {}", MODES.join(", "))),
        }
    }
    // by air, the arrival is a flight (its boarding pass is the platform's second ticket); by land / sea or SkyPier
    // there is no arrival flight
    if matches!(
        out.get("entry_mode").and_then(Value::as_str),
        Some("land_sea" | "skypier")
    ) {
        for k in ["in_flight", "in_pass_file"] {
            out.remove(k);
        }
    }
    if !problems.is_empty() {
        return Checked::Invalid(problems);
    }
    // the information path (the user asked how to get the refund, not to apply): what the call gave, nothing asked,
    // no mail read, no page opened (the user, 2026-11-04)
    if !truthy(args.get("file").unwrap_or(&Value::Null)) {
        out.insert("file".into(), json!(false));
        return Checked::Valid(Value::Object(out));
    }
    out.insert("file".into(), json!(true));
    // the trip decides the rule (within 28 days of the departure, the same or the next day): known before the platform opens
    // the departure decides it (the arrival may be left for the platform: the user, 2026-11-02)
    let trip_known = ["out_flight", "out_date"]
        .iter()
        .all(|k| out.contains_key(*k));
    // the trip not known yet: the documents are fetched and read first (the mail's, and the PDFs the user attached)
    let user_pdfs: Vec<String> = user_files(cx.transcript)
        .into_iter()
        .filter(|f| f.to_lowercase().ends_with(".pdf"))
        .collect();
    if !trip_known && (cx.mail_ready || !user_pdfs.is_empty()) && !docs_fetched(cx.transcript) {
        out.insert("read_mail".into(), json!(true));
        out.insert("user_pdfs".into(), json!(user_pdfs));
        return Checked::Valid(Value::Object(out));
    }
    // the user attached files and the call names none of them: the model maps each to its field (apdt_real
    // 2026-11-03: three documents attached, the tool called twice without them)
    // once only: the tool always runs on what the user gave (the user, 2026-11-03) — a call that still names none goes
    // on without them
    let attached = user_files(cx.transcript);
    if !attached.is_empty()
        && !FILES.iter().any(|k| out.contains_key(*k))
        && !said_before(cx.transcript, MAP_FILES)
    {
        return Checked::Invalid(vec![format!(
            "{MAP_FILES} {}: pass each document by its path as passport_file, out_pass_file, in_pass_file or \
             receipt_file (leave out only the documents the user did not give)",
            attached.join(", ")
        )]);
    }
    if !out.contains_key("passport_file") {
        if let Some(p) = passport_default(&attached) {
            out.insert("passport_file".into(), json!(p));
        }
    }
    let has = |k: &str| out.contains_key(k);
    let mut missing: Vec<String> = Vec::new();
    // the arrival into Hong Kong may be left out: the user adds it on the platform (the user, 2026-11-02); given, it
    // needs both its flight and its date
    if !has("entry_mode") {
        missing.push("entry_mode (how the user entered Hong Kong: air — a flight; land_sea — through a land or sea control point such as Shenzhen Bay; skypier — the SkyPier ferry / coach or Park & Fly)".into());
    }
    let by_air = out.get("entry_mode").and_then(Value::as_str) == Some("air");
    if by_air && !(has("in_flight") && has("in_date")) {
        missing.push("the flight into Hong Kong and its date (in_flight, in_date)".into());
    }
    if !(has("out_flight") && has("out_date")) {
        missing.push("the flight out of Hong Kong and its date (out_flight, out_date)".into());
    }
    match out.get("refund_method").and_then(Value::as_str) {
        None => missing.push("refund_method (a credit / debit card, Alipay / WeChat Pay with the account, or cash at the airport)".into()),
        Some(m) if m.starts_with("alipay") || m.starts_with("wechat") => {
            if !has("wallet_account") {
                missing.push("wallet_account (the e-wallet account the refund goes to)".into());
            }
        }
        Some(_) => {}
    }
    // the documents neither the mail nor the chat gave: asked once, with the rest, before the platform opens — then the
    // call goes on with whatever the user gave (the user, 2026-11-02)
    // the arrival's boarding pass only by air; SkyPier's platform asks the face check first, so code uploads nothing
    let mode_now = out.get("entry_mode").and_then(Value::as_str).unwrap_or("");
    let docs: Vec<&str> = DOC_NAMES
        .iter()
        .filter(|(k, _)| !out.contains_key(*k))
        .filter(|(k, _)| *k != "in_pass_file" || mode_now == "air")
        .filter(|_| mode_now != "skypier")
        .map(|(_, what)| *what)
        .collect();
    if !docs.is_empty() && !docs_asked(cx.transcript) {
        missing.push(format!("{DOCS_ASK} {}", docs.join("; ")));
    }
    // asked once: the next call runs with whatever the user gave (the user, 2026-11-03: "the code tool is called
    // regardless") — the platform asks for what is still missing, and the result says what was not given
    if !missing.is_empty() && !asked_once(cx.transcript) {
        return Checked::NeedsInput(missing);
    }
    Checked::Valid(Value::Object(out))
}

// ---------------------------------------------------------------- the machine

/// The refund method of the validated arguments.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Method {
    Card,
    Cash,
    /// the platform's radio value (alipay, wechat, alipayHK, wechatHK) and the account the user gave
    Wallet(String, String),
}

impl Method {
    /// The method as the user reads it.
    fn words(&self) -> String {
        match self {
            Method::Card => "to a credit / debit card (the platform's fee up to HK$18)".into(),
            Method::Cash => "in cash at the airport's counter, after approval".into(),
            Method::Wallet(w, a) => format!("to {w} ({a})"),
        }
    }
}

/// Everything the rule and the result need, from the validated arguments.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Facts {
    /// how the user entered Hong Kong: air, land_sea or skypier ("" when not given)
    pub entry_mode: String,
    pub in_flight: String,
    pub in_date: Option<NaiveDate>,
    pub out_flight: String,
    pub out_date: Option<NaiveDate>,
    /// the departure tax the receipt shows (HK$), when the arguments say
    pub tax: Option<u32>,
    pub method: Option<Method>,
}

impl Facts {
    pub fn from_args(a: &Value) -> Facts {
        let g = |k: &str| s_arg(a, k);
        let method = match g("refund_method").as_str() {
            "card" => Some(Method::Card),
            "cash" => Some(Method::Cash),
            "" => None,
            w => Some(Method::Wallet(w.to_string(), g("wallet_account"))),
        };
        Facts {
            entry_mode: g("entry_mode"),
            in_flight: g("in_flight"),
            in_date: date_of(&g("in_date")),
            out_flight: g("out_flight"),
            out_date: date_of(&g("out_date")),
            tax: a.get("tax_hkd").and_then(Value::as_u64).map(|t| t as u32),
            method,
        }
    }
}

/// Why the trip does not qualify (the rule), or `None` when it does.
pub fn ineligible(f: &Facts, today: NaiveDate, window: i64) -> Option<(&'static str, String)> {
    let d = f.out_date?;
    // the same or the next day after arriving: checked when the arrival is given (else the user adds it on the platform,
    // which checks it)
    if let Some(a) = f
        .in_date
        .filter(|a| !(0..=1).contains(&(d - *a).num_days()))
    {
        return Some(("stayed_longer", format!(
            "arrived on {} ({}) and left on {} ({}): the refund is only for passengers who leave Hong Kong by air on the same or the next calendar day after arriving",
            long(a), f.in_flight, long(d), f.out_flight
        )));
    }
    if d > today {
        return Some(("not_departed", format!(
            "the departure flight {} is on {}: the platform takes the application only after the departure (it checks the departure record)",
            f.out_flight,
            long(d)
        )));
    }
    if (today - d).num_days() > window {
        return Some(("too_late", format!(
            "the departure flight {} was on {}: applications close {} calendar days after the departure flight (that was {})",
            f.out_flight,
            long(d),
            window,
            long(d + Duration::days(window))
        )));
    }
    if f.tax == Some(0) {
        return Some(("no_tax", "the e-ticket receipt does not show the Hong Kong Air Passenger Departure Tax (HK$120 / HK$200) as paid: there is nothing to refund (a transfer on one through ticket is usually not taxed)".into()));
    }
    None
}

/// What the machine reads besides its arguments and the transcript.
pub struct Machine<'a> {
    pub profile: &'a Value,
    pub mail_ready: bool,
    pub today: NaiveDate,
    /// the machine's code calls in the transcript before this run's tool call: the ids go on from there
    pub first: usize,
}

/// The next step of a run: the validated arguments, the transcript after the tool call.
/// How to get the Hong Kong departure tax refund (the user asked about it, not to apply — the user, 2026-11-04): the
/// rule's verdict when the departure is known, the deadline as a date, the documents, the official platform's link.
/// Applying is for when the user asks for it (`file`).
fn refund_info(f: &Facts, today: NaiveDate, window_days: i64) -> Value {
    if let Some((code, why)) = ineligible(f, today, window_days) {
        return json!({"status": "not_eligible", "reason": code, "rule": why,
            "show": [S::fact("Not eligible for the Hong Kong departure tax refund", "不符合香港离境税退税条件", &why),
                     S::link("Refund platform", "退税平台", "Airport Authority Hong Kong", HOME)],
            "hints": ["the user asked about the refund: nothing was applied for"]});
    }
    let mut show = vec![S::fact(
        "Who can claim",
        "谁可以申请",
        "a passenger who arrived in Hong Kong (by air, by land / sea, or through SkyPier) and flew out on the same or \
         the next calendar day, paying the HK$120 / HK$200 Air Passenger Departure Tax on the ticket",
    )];
    if let Some(d) = f.out_date {
        show.push(S::fact(
            "Apply by",
            "申请截止",
            &format!(
                "{} (28 days after the departure flight)",
                long(d + Duration::days(window_days))
            ),
        ));
    } else {
        show.push(S::fact(
            "Apply by",
            "申请截止",
            "within 28 days after the departure flight",
        ));
    }
    show.push(S::fact(
        "You need",
        "需要",
        "your passport's photo page, the boarding pass of the flight out (and of the flight in, if you arrived by \
         air), the ticket or e-ticket receipt showing the departure tax, and how you want the refund (card, cash, \
         Alipay / WeChat Pay)",
    ));
    show.push(S::link(
        "Refund platform",
        "退税平台",
        "Airport Authority Hong Kong",
        HOME,
    ));
    show.push(S::fact(
        "Applying",
        "代办",
        "the platform is free; ask me to apply and I upload your documents there and give you the application's reference — the face check and Submit stay yours",
    ));
    json!({"status": "info", "home": HOME, "show": show,
        "apply_by": f.out_date.map(|d| (d + Duration::days(window_days)).format("%Y-%m-%d").to_string()),
        "hints": ["the user asked how to get the refund: the rule, the documents and the official link — nothing was applied for"]})
}

pub fn next(tool: &str, args: &Value, t: &[Value], m: &Machine) -> Step {
    debug_assert_eq!(tool, APDT_REFUND);
    let s = &APDT;
    let n = m.first
        + t.iter()
            .flat_map(tool_calls)
            .filter(|tc| get_str(tc, "id").starts_with(PREFIX))
            .count()
        + 1;
    let f = Facts::from_args(args);
    if !truthy(args.get("file").unwrap_or(&Value::Null)) {
        return Step::Done(refund_info(&f, m.today, s.window_days));
    }
    // 1. the documents: the fixed search, the hits, the attachments saved, every PDF read; the model reads them and
    //    calls again
    if truthy(args.get("read_mail").unwrap_or(&Value::Null)) {
        if m.mail_ready && results_of(t, "gmail_search").is_empty() {
            return Step::Calls(vec![call(
                n,
                "gmail_search",
                json!({"query": s.search, "max_results": 10}),
            )]);
        }
        let have: Vec<String> = results_of(t, "gmail_get_message")
            .iter()
            .map(|(a, _)| sget(a, "message_id"))
            .collect();
        let mut want: Vec<String> = Vec::new();
        for (_, msg) in results_of(t, "gmail_search") {
            let mut hits = Vec::new();
            walk_messages(&json_of(msg), &mut hits);
            for h in hits.iter().take(6) {
                let id = get_str(h, "id");
                if !id.is_empty() && !have.contains(&id) && !want.contains(&id) {
                    want.push(id);
                }
            }
        }
        if !want.is_empty() {
            return Step::Calls(
                want.iter()
                    .enumerate()
                    .map(|(i, id)| call(n + i, "gmail_get_message", json!({"message_id": id})))
                    .collect(),
            );
        }
        let tried: Vec<(String, String)> = results_of(t, "gmail_save_attachment")
            .iter()
            .map(|(a, _)| (sget(a, "message_id"), sget(a, "filename")))
            .collect();
        let to_save: Vec<ToolCall> = mail_attachments(t)
            .into_iter()
            .filter(|(i, fl, _)| !tried.contains(&(i.clone(), fl.clone())))
            .enumerate()
            .map(|(k, (i, fl, _))| {
                call(
                    n + k,
                    "gmail_save_attachment",
                    json!({"message_id": i, "filename": fl}),
                )
            })
            .collect();
        if !to_save.is_empty() {
            return Step::Calls(to_save);
        }
        // the user's attachments are in the user's messages, before the tool call: the whole chat is not `t`, so
        // the PDFs to read are the saved ones and the ones the arguments' files name besides
        let read: Vec<String> = results_of(t, "files_read")
            .iter()
            .map(|(a, _)| sget(a, "path"))
            .collect();
        let saved_paths: Vec<(String, String)> = saved(t);
        let user_pdfs: Vec<String> = args
            .get("user_pdfs")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .map(str::to_string)
            .collect();
        let to_read: Vec<ToolCall> = saved_paths
            .iter()
            .map(|(p, _)| p.clone())
            .chain(user_pdfs.iter().cloned())
            .filter(|p| p.to_lowercase().ends_with(".pdf") && !read.contains(p))
            .enumerate()
            .map(|(k, p)| call(n + k, "files_read", json!({"path": p, "max_chars": 4000})))
            .collect();
        if !to_read.is_empty() {
            return Step::Calls(to_read);
        }
        let files: Vec<Value> = saved_paths
            .iter()
            .map(|(p, subj)| json!({"path": p, "mail": subj}))
            .chain(user_pdfs.iter().map(|p| json!({"path": p, "mail": ""})))
            .collect();
        return Step::Done(json!({"status": MAIL_READ, "files": files, "hints": []}));
    }
    // 2. the rule, then the platform
    let opened = results_of(t, "browser_navigate")
        .iter()
        .any(|(a, _)| sget(a, "url").contains(s.host));
    if !opened {
        if let Some((code, why)) = ineligible(&f, m.today, s.window_days) {
            return Step::Done(
                json!({"status": "not_eligible", "reason": code, "rule": why,
                "show": [S::fact("Not eligible for the Hong Kong departure tax refund", "不符合香港离境税退税条件", &why)],
                "hints": ["nothing was submitted"]}),
            );
        }
        // SkyPier: the platform's face check comes right after the travel document and before any saved application
        // (walked 2026-11-03), so there is no session for code to make: the facts, the link and the user's steps
        if f.entry_mode == "skypier" {
            return Step::Done(skypier_result(&f, m.today));
        }
        return Step::Calls(vec![call(
            n,
            "browser_navigate",
            json!({"url": s.start_url}),
        )]);
    }
    // 3. the notes, "Start", "New Application" and the entry mode, up to the travel document's upload (its form
    //    function, `crate::forms`); the photos from there on are the user's
    // the chat's documents go with it: the form function uploads each into its slot (the user, 2026-11-02: the
    // recognised travel document creates the application, whose reference number opens it on any device)
    let mut fill_args = json!({"stage": "start", "entry_mode": if f.entry_mode.is_empty() { "air" } else { f.entry_mode.as_str() }});
    for k in FILES {
        let v = s_arg(args, k);
        if !v.is_empty() {
            fill_args[*k] = json!(v);
        }
    }
    let Some((_, fill)) = results_of(t, "form_fill")
        .into_iter()
        .find(|(a, _)| sget(a, "stage") == "start")
    else {
        return Step::Calls(vec![call(n, "form_fill", fill_args)]);
    };
    if failed(fill) {
        return Step::Done(stopped_result(
            &f,
            "the platform's page could not be filled",
        ));
    }
    if let Some(why) = stopped(fill) {
        return Step::Done(stopped_result(&f, &why));
    }
    if s_arg(args, "passport_file").is_empty() {
        return Step::Done(result(&f, m.today));
    }
    // the application's reference number, from its fixed element (one character per span)
    let Some((_, read)) = results_of(t, "browser_read_elements").into_iter().next() else {
        return Step::Calls(vec![call(
            n,
            "browser_read_elements",
            json!({"read": [{"name": "reference", "sel": REFERENCE_SEL}]}),
        )]);
    };
    match reference_of(read) {
        Some(code) => Step::Done(saved_result(&f, &code, &not_given(args))),
        None => Step::Done(stopped_result(
            &f,
            "the platform did not recognise the travel document's photo, so no application was saved",
        )),
    }
}

/// The application's reference number on the platform's pages (mapped 2026-11-02): one character per span.
const REFERENCE_SEL: &str = ".bottom span.card-number.code";

/// The reference number from [`REFERENCE_SEL`]'s values: six letters and digits, else `None`.
fn reference_of(read: &Value) -> Option<String> {
    let code: String = super::confirm::element_values(read, "reference")
        .iter()
        .map(|c| c.trim())
        .collect();
    (code.len() == 6 && code.chars().all(|c| c.is_ascii_alphanumeric())).then_some(code)
}

/// The application saved on the platform with the chat's documents: the reference number and how to open it on any
/// device; the refund method, the face recognition and Submit are the user's.
/// The documents the call did not give (the user skipped them): they upload them in the saved application.
fn not_given(args: &Value) -> Vec<&'static str> {
    [
        ("out_pass_file", "the departure flight's boarding pass"),
        ("in_pass_file", "the arrival flight's boarding pass"),
        (
            "receipt_file",
            "the ticket or e-ticket receipt showing the departure tax",
        ),
    ]
    .into_iter()
    .filter(|(k, _)| s_arg(args, k).is_empty())
    .map(|(_, what)| what)
    .collect()
}

fn saved_result(f: &Facts, code: &str, missing: &[&str]) -> Value {
    let mut v = result(f, NaiveDate::MIN);
    v["status"] = json!("saved");
    v["reference"] = json!(code);
    v["to_upload"] = json!(missing);
    if let Some(show) = v["show"].as_array_mut() {
        show.retain(|x| get_str(x, "label") != "Next steps (yours)");
        let at = show
            .iter()
            .position(|x| get_str(x, "label") == "Refund platform")
            .unwrap_or(show.len());
        show.insert(at, S::reference("Application reference", "申请编号", code));
        // what the user skipped: theirs to upload in the saved application (the user, 2026-11-02)
        if !missing.is_empty() {
            show.insert(
                at + 1,
                S::fact(
                    "Still to upload (yours)",
                    "还需上传（你的）",
                    &format!("{} — in the saved application", missing.join("; ")),
                ),
            );
        }
        show.push(S::fact(
            "Next steps (yours)",
            "下一步（你的）",
            &format!(
                "the documents are uploaded and the application is saved, not submitted. On any device open the platform, \
                 choose \"View Application / Continue Application\", enter your travel document number and the reference \
                 number {code}, then \"Continue Process\": {}upload the documents still missing, choose the refund {}, do the face \
                 recognition and submit",
                if f.in_flight.is_empty() {
                    "add your arrival into Hong Kong (the arrival flight's boarding pass, or change the entry mode if you came by land or sea), "
                } else {
                    ""
                },
                f.method
                    .as_ref()
                    .map(Method::words)
                    .unwrap_or_else(|| "method".into())
            ),
        ));
    }
    v["hints"] = json!([
        "the application is saved with the user's documents, not submitted; the refund method, the face recognition and Submit are the user's, on any device with the reference number"
    ]);
    v
}

/// Entry by the SkyPier Terminal: the platform verifies the travel document with the face check before it saves an
/// application, so the steps are the user's, on their phone, from the start.
fn skypier_result(f: &Facts, today: NaiveDate) -> Value {
    let mut v = result(f, today);
    v["status"] = json!("skypier");
    if let Some(show) = v["show"].as_array_mut() {
        show.retain(|x| get_str(x, "label") != "Next steps (yours)");
        show.push(S::fact(
            "Next steps (yours)",
            "下一步（你的）",
            "on your phone, open the platform and choose \"SkyPier Terminal (Ferry/Coach/Park & Fly)\": it checks your travel document with a face check first (the platform saves the application only after it), then the departure boarding pass and the receipt with the tax, the refund method, and submit",
        ));
    }
    v["hints"] = json!([
        "SkyPier: the face check comes first on the platform, so nothing was uploaded for the user"
    ]);
    v
}

/// The run stopped before the document upload (the platform's waiting room, a page that changed): the facts, the
/// link, nothing submitted.
fn stopped_result(f: &Facts, why: &str) -> Value {
    let mut v = result(f, NaiveDate::MIN);
    v["status"] = json!("stopped");
    v["why"] = json!(why);
    if let Some(show) = v["show"].as_array_mut() {
        show.retain(|x| get_str(x, "label") != "Next steps (yours)");
        show.push(S::fact(
            "Stopped",
            "已停止",
            &format!("{why} — nothing was submitted; apply on your phone at the link"),
        ));
    }
    v
}

/// The machine's JSON result at the document upload: the rule's verdict, the trip, the amount, the deadline, the
/// steps that are the user's (the documents' photos, the refund method, the face recognition — on their phone) and
/// the platform's link.
pub fn result(f: &Facts, _today: NaiveDate) -> Value {
    let leg = |no: &str, d: Option<NaiveDate>| json!({"flight": no, "date": d.map(|d| d.format("%Y-%m-%d").to_string()).unwrap_or_default()});
    let deadline = f.out_date.map(|d| d + Duration::days(APDT.window_days));
    let day = |d: Option<NaiveDate>| d.map(long).unwrap_or_else(|| "?".into());
    // the facts the user must read: appended to the reply by code (agent/show.rs)
    let mut show = vec![if f.entry_mode == "land_sea" || f.entry_mode == "skypier" {
        S::fact(
            "Entered Hong Kong by",
            "入境方式",
            &format!(
                "{}{} — the refund is for leaving by air ({} {}) on the same or the next calendar day",
                if f.entry_mode == "land_sea" {
                    "land / sea (a control point such as Shenzhen Bay)"
                } else {
                    "the SkyPier Terminal (ferry / coach, Park & Fly)"
                },
                f.in_date.map(|d| format!(" on {}", long(d))).unwrap_or_default(),
                f.out_flight,
                day(f.out_date)
            ),
        )
    } else if f.in_flight.is_empty() {
        // the arrival left for the user to add on the platform (the user, 2026-11-02)
        S::fact(
            "Arrival into Hong Kong",
            "抵港航班",
            &format!(
                "not given — add it on the platform (the arrival flight's boarding pass, or the entry mode if you came by land or sea): the refund is only for leaving by air ({} {}) on the same or the next calendar day after arriving",
                f.out_flight,
                day(f.out_date)
            ),
        )
    } else {
        S::fact(
            "Eligible",
            "符合条件",
            &format!(
                "arrived on {} {}, left on {} {} — the same or the next calendar day",
                f.in_flight,
                day(f.in_date),
                f.out_flight,
                day(f.out_date)
            ),
        )
    }];
    if let Some(tax) = f.tax {
        show.push(S::amount(
            "Departure tax to be refunded",
            "待退离境税",
            f64::from(tax),
            "HKD",
        ));
    }
    if let Some(d) = deadline {
        show.push(S::fact(
            "Apply by",
            "申请截止",
            &format!("{} (28 days after the departure flight)", long(d)),
        ));
    }
    let refund = f
        .method
        .as_ref()
        .map(Method::words)
        .unwrap_or_else(|| "the method you choose".into());
    show.push(S::fact(
        "Next steps (yours)",
        "下一步（你的）",
        &format!("the platform is open at its document upload (entry by air chosen): upload your passport's photo page, both boarding passes and the e-ticket receipt showing the tax, choose the refund {refund}, then do the face recognition — on your phone; nothing was submitted"),
    ));
    show.push(S::link(
        "Refund platform",
        "退税平台",
        "Airport Authority Hong Kong",
        HOME,
    ));
    json!({"status": "ready", "home": HOME,
        "trip": {"arrival": leg(&f.in_flight, f.in_date), "departure": leg(&f.out_flight, f.out_date), "tax_hkd": f.tax},
        "apply_by": deadline.map(|d| d.format("%Y-%m-%d").to_string()), "refund": refund,
        "show": show, "hints": ["nothing was submitted; the documents, the refund method and the face recognition are the user's, on their phone"]})
}

#[cfg(test)]
mod tests {
    use super::super::driver::questions_after_navigate;
    use super::*;

    fn today() -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 11, 1).unwrap()
    }

    fn page(url: &str, snapshot: &str) -> String {
        format!(
            "<untrusted_content source=\"web page {url}\">\n{}\n</untrusted_content>",
            json!({"trust": "untrusted", "source": format!("web page {url}"), "url": url,
                   "title": "APDT", "tabs": 1, "snapshot": snapshot})
        )
    }

    const REQUEST: &str = "I flew through Hong Kong this week on two separate tickets: Singapore Airlines into Hong Kong, then \
Cathay Pacific to Tokyo the next morning, and I paid the Hong Kong departure tax on the Cathay ticket. Please apply for \
the Hong Kong departure tax refund for me. The boarding passes and the e-ticket receipt are in my email. Refund it to my \
card.";

    const SQ_PASS: &str = "attachments/BoardingPass_SQ890_LU_LUCAS.pdf";
    const CX_PASS: &str = "attachments/BoardingPass_CX500_LU_LUCAS.pdf";
    const RECEIPT: &str = "attachments/ETKT_1602345678901_Receipt.pdf";

    fn mails() -> Vec<(String, Value)> {
        vec![
            (
                "i-17".into(),
                json!({"from": "Singapore Airlines <boardingpass@singaporeair.example>",
                "subject": "Your boarding pass: SQ 890 Singapore to Hong Kong on 2026-10-29",
                "attachments": [{"filename": "BoardingPass_SQ890_LU_LUCAS.pdf", "type": "application/pdf"}]}),
            ),
            (
                "i-18".into(),
                json!({"from": "Cathay Pacific <eticket@cathaypacific.example>",
                "subject": "Your e-ticket receipt: CX 500 Hong Kong to Tokyo Narita, 2026-10-30",
                "attachments": [{"filename": "ETKT_1602345678901_Receipt.pdf", "type": "application/pdf"}]}),
            ),
            (
                "i-19".into(),
                json!({"from": "Cathay Pacific <checkin@cathaypacific.example>",
                "subject": "Boarding pass: CX 500 Hong Kong to Tokyo Narita, 2026-10-30",
                "attachments": [{"filename": "BoardingPass_CX500_LU_LUCAS.pdf", "type": "application/pdf"}]}),
            ),
        ]
    }

    /// The world's answer to one of the machine's calls; `fill` is the start page's fill result.
    fn respond(name: &str, args: &Value, fill: &str) -> String {
        match name {
            "gmail_search" => json!({"count": 3, "messages": mails().iter().map(|(id, m)| json!({"id": id, "subject": m["subject"]})).collect::<Vec<_>>()}).to_string(),
            "gmail_get_message" => {
                let id = sget(args, "message_id");
                let m = mails().into_iter().find(|(i, _)| *i == id).unwrap().1;
                format!("<untrusted_content source=\"email\">\n{m}\n</untrusted_content>")
            }
            "gmail_save_attachment" => format!(
                "附件已保存 attachment saved: attachments/{} (1 KB, application/pdf).",
                sget(args, "filename")
            ),
            "files_read" => format!("<untrusted_content source=\"file {}\">\n--- page 1 ---\nBOARDING PASS\n</untrusted_content>", sget(args, "path")),
            "browser_navigate" => page(HOME, "  Notes for Application"),
            "form_fill" => fill.to_string(),
            // the reference number's spans (none when the fill's plan says the passport was not recognised)
            "browser_read_elements" => {
                let chars: Vec<&str> = if fill.contains("unrecognised") { vec![] } else { vec!["F", "4", "F", "T", "2", "3"] };
                format!("<untrusted_content source=\"web page {HOME}\">\n{}\n</untrusted_content>",
                        json!({"trust": "untrusted", "url": HOME, "elements": {"reference": chars}}))
            }
            _ => "ok".into(),
        }
    }

    fn step(id: &str, name: &str, args: &Value, result: &str) -> Vec<Value> {
        vec![
            json!({"role": "assistant", "content": "", "tool_calls": [{"id": id, "type": "function",
                   "function": {"name": name, "arguments": args.to_string()}}]}),
            json!({"role": "tool", "tool_call_id": id, "content": result}),
        ]
    }

    /// The check of a call that APPLIES (`file: true` unless the test says otherwise: the user asked for it).
    fn checked(mut args: Value, t: &[Value], mail_ready: bool) -> Checked {
        if args.get("file").is_none() {
            args["file"] = json!(true);
        }
        let p = json!({"name_en": "LUCAS LU"});
        check(
            APDT_REFUND,
            &args,
            &Ctx {
                profile: &p,
                today: today(),
                mail_ready,
                transcript: t,
            },
        )
    }

    /// The call as the run loop records it: the call, then `running` with the validated arguments.
    fn started(mut t: Vec<Value>, raw: Value, mail_ready: bool) -> (Vec<Value>, Value) {
        let args = match checked(raw.clone(), &t, mail_ready) {
            Checked::Valid(a) => a,
            other => panic!("not valid: {other:?}"),
        };
        t.extend(step(
            "call_w",
            APDT_REFUND,
            &raw,
            &json!({"status": "running", "args": args}).to_string(),
        ));
        (t, args)
    }

    /// Runs the machine from the latest `call_w` to its result; panics on a question after the platform opened.
    fn run(mut t: Vec<Value>, args: &Value, fill: &str) -> (Vec<Value>, Value) {
        let mut applying = args.clone();
        if applying.get("file").is_none() {
            applying["file"] = json!(true);
        }
        let args = &applying;
        let from = t
            .iter()
            .rposition(|x| tool_calls(x).iter().any(|tc| get_str(tc, "id") == "call_w"))
            .unwrap();
        let p = json!({});
        for _ in 0..30 {
            let m = Machine {
                profile: &p,
                mail_ready: true,
                today: today(),
                first: 0,
            };
            match next(APDT_REFUND, args, &t[from..], &m) {
                Step::Calls(c) => {
                    for call in &c {
                        assert!(call.id.starts_with(PREFIX), "{}", call.id);
                        let result = respond(&call.name, &call.args, fill);
                        t.extend(step(&call.id, &call.name, &call.args, &result));
                    }
                    let q = questions_after_navigate(&t);
                    assert!(q.is_empty(), "a question after the form opened: {q:?}");
                }
                Step::Done(v) => return (t, v),
            }
        }
        panic!("the run did not end")
    }

    #[test]
    fn a_question_about_the_refund_gets_the_rule_the_deadline_and_the_link_and_nothing_is_applied()
    {
        // the user, 2026-11-04: "can I get the departure tax back?" gets who can claim, the deadline, the documents and
        // the official link; applying is for when the user asks for it (file true)
        let (t, a) = started(
            Vec::new(),
            json!({"file": false, "out_flight": "CX 500", "out_date": "2026-10-28", "in_date": "2026-10-27"}),
            true,
        );
        assert_eq!(a["file"], false);
        let (t, res) = run(t, &a, "");
        assert!(
            calls_of(&t, "browser_navigate").is_empty() && calls_of(&t, "gmail_search").is_empty()
        );
        assert_eq!(res["status"], "info", "{res}");
        assert_eq!(res["apply_by"], "2026-11-25");
        let lines = super::super::show::lines(&res, false).join("\n");
        assert!(
            lines.contains(HOME)
                && lines.contains("boarding pass")
                && lines.contains("25 November 2026"),
            "{lines}"
        );
        // a trip outside the rule: not eligible, said without applying
        let (t, late) = started(
            Vec::new(),
            json!({"file": false, "out_flight": "CX 500", "out_date": "2026-09-01"}),
            true,
        );
        let (_, res) = run(t, &late, "");
        assert_eq!(res["status"], "not_eligible", "{res}");
    }

    fn calls_of(t: &[Value], name: &str) -> Vec<Value> {
        t.iter()
            .flat_map(tool_calls)
            .map(stored_call)
            .filter(|(n, _)| n == name)
            .map(|(_, a)| a)
            .collect()
    }

    /// What the model reads in the boarding passes and the receipt the machine fetched.
    fn facts() -> Value {
        json!({"entry_mode": "air", "in_flight": "SQ 890", "in_date": "2026-10-29", "out_flight": "CX 500", "out_date": "2026-10-30",
               "in_pass_file": SQ_PASS, "out_pass_file": CX_PASS, "receipt_file": RECEIPT, "tax_hkd": 120,
               "refund_method": "card"})
    }

    /// A chat in which an earlier call already asked for the documents (the user skipped them).
    fn docs_asked_chat(user: &str) -> Vec<Value> {
        skipped(vec![json!({"role": "user", "content": user})])
    }

    /// `t` and then the documents asked once and skipped by the user.
    fn skipped(mut t: Vec<Value>) -> Vec<Value> {
        t.extend(step(
            "call_a",
            APDT_REFUND,
            &json!({}),
            &json!({"status": "needs_input", "missing": [format!("{DOCS_ASK} the passport's photo page")]}).to_string(),
        ));
        t.push(json!({"role": "user", "content": "I have no documents at hand."}));
        t
    }

    const FILLED: &str = r#"{"url": "https://app.apdtrefund.hk/hkTaxMobile/process/upload", "filled": [{"op": "click", "field": "entry_air", "ok": true}]}"#;

    #[test]
    fn the_trip_the_refund_method_and_the_optional_documents_are_asked_once_before_the_platform() {
        let t = vec![json!({"role": "user", "content": REQUEST})];
        // a mailbox and the trip unknown: the documents are fetched first
        match checked(json!({}), &t, true) {
            Checked::Valid(a) => assert_eq!(a["read_mail"], true),
            other => panic!("{other:?}"),
        }
        // no mailbox: the trip and the method only the user can give — never the documents (their photos are the
        // user's step on the platform)
        let bare = vec![
            json!({"role": "user", "content": "Please apply for my Hong Kong departure tax refund."}),
        ];
        match checked(json!({}), &bare, false) {
            Checked::NeedsInput(m) => {
                let m = m.join(" | ");
                // the arrival may be left for the user to add on the platform (the user, 2026-11-02): not asked; the
                // documents are asked with the rest, as optional (what is skipped is uploaded later in the session)
                assert!(
                    !m.contains("into Hong Kong")
                        && m.contains("out of Hong Kong")
                        && m.contains("refund_method"),
                    "{m}"
                );
                assert!(m.contains("passport_file") && m.contains("optional"), "{m}");
            }
            other => panic!("{other:?}"),
        }
        // asked once already: the next call runs with whatever the user gave, even nothing (the user, 2026-11-03)
        let (t0, args0) = started(
            docs_asked_chat("Please apply for my Hong Kong departure tax refund."),
            json!({}),
            false,
        );
        let (_, res0) = run(t0, &args0, FILLED);
        assert_eq!(res0["status"], "ready", "{res0}");
        // the trip given in words, the documents asked once already and skipped: valid with no files
        match checked(
            json!({"in_flight": "sq890", "in_date": "2026-10-29", "out_flight": "CX 500", "out_date": "2026-10-30", "refund_method": "card"}),
            &docs_asked_chat("Please apply for my Hong Kong departure tax refund."),
            false,
        ) {
            Checked::Valid(v) => assert_eq!(v["in_flight"], "SQ 890"),
            other => panic!("{other:?}"),
        }
        // the fields the model can fix
        for (k, bad, why) in [
            ("in_flight", json!("890"), "airline code"),
            ("out_date", json!("30/10"), "not a YYYY-MM-DD"),
            ("refund_method", json!("bitcoin"), "one of card"),
        ] {
            let mut a = facts();
            a[k] = bad;
            a["in_pass_file"] = json!("");
            a["out_pass_file"] = json!("");
            a["receipt_file"] = json!("");
            match checked(a, &bare, false) {
                Checked::Invalid(p) => assert!(p.iter().any(|x| x.contains(why)), "{k}: {p:?}"),
                other => panic!("{k}: {other:?}"),
            }
        }
        // an e-wallet needs its account
        let mut a = facts();
        a["refund_method"] = json!("alipayHK");
        for k in ["in_pass_file", "out_pass_file", "receipt_file"] {
            a[k] = json!("");
        }
        match checked(a, &bare, false) {
            Checked::NeedsInput(m) => assert!(m[0].contains("wallet_account"), "{m:?}"),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn code_reads_the_mail_then_goes_to_the_platforms_document_upload_and_stops() {
        let (t, args) = started(
            vec![json!({"role": "user", "content": REQUEST})],
            json!({"refund_method": "card"}),
            true,
        );
        let (mut t, res) = run(t, &args, FILLED);
        assert_eq!(res["status"], MAIL_READ);
        assert_eq!(calls_of(&t, "gmail_save_attachment").len(), 3);
        assert_eq!(calls_of(&t, "files_read").len(), 3);
        assert!(calls_of(&t, "browser_navigate").is_empty());
        t.extend(workflow::result_exchange(&t, APDT_REFUND, &res));
        // the mail had the boarding passes and the receipt, not the passport: asked once (optional), the user skips it
        match checked(facts(), &t, true) {
            Checked::NeedsInput(m) => {
                assert_eq!(m.len(), 1, "{m:?}");
                assert!(
                    m[0].starts_with(DOCS_ASK)
                        && m[0].contains("passport_file")
                        && !m[0].contains("receipt_file"),
                    "{m:?}"
                );
            }
            other => panic!("{other:?}"),
        }
        let t = skipped(t);
        let (t, args) = started(t, facts(), true);
        let (t, res) = run(t, &args, FILLED);
        assert_eq!(res["status"], "ready");
        assert_eq!(calls_of(&t, "browser_navigate")[0]["url"], APDT.start_url);
        // the documents of the chat go to the form function, which uploads them into their slots
        assert_eq!(
            calls_of(&t, "form_fill"),
            [
                json!({"stage": "start", "entry_mode": "air", "in_pass_file": SQ_PASS, "out_pass_file": CX_PASS, "receipt_file": RECEIPT})
            ]
        );
        // no travel document in the chat: no application saved, nothing to read; no card, no hand-over
        assert!(calls_of(&t, "browser_read_elements").is_empty());
        for never in [
            "browser_upload",
            "browser_fill_secret",
            "browser_request_takeover",
            "vault_list",
        ] {
            assert!(calls_of(&t, never).is_empty(), "{never}");
        }
        let lines = super::super::show::lines(&res, false).join("\n");
        assert!(lines.contains("- Apply by: 27 November 2026"), "{lines}");
        assert!(lines.contains("HKD 120.00"), "{lines}");
        assert!(
            lines.contains("document upload") && lines.contains("face recognition"),
            "{lines}"
        );
        assert!(lines.contains(HOME), "{lines}");
    }

    #[test]
    fn a_page_that_did_not_reach_the_upload_stops_with_the_facts() {
        let bare = docs_asked_chat("apply");
        let mut a = facts();
        for k in ["in_pass_file", "out_pass_file", "receipt_file"] {
            a[k] = json!("");
        }
        let (t, args) = started(bare, a, false);
        let stop = r#"{"url": "x", "filled": [{"op": "require", "field": "travel_document", "ok": false, "stop": true, "why": "the platform did not reach its document upload"}]}"#;
        let (_, res) = run(t, &args, stop);
        assert_eq!(res["status"], "stopped");
        let lines = super::super::show::lines(&res, false).join("\n");
        assert!(
            lines.contains("did not reach its document upload"),
            "{lines}"
        );
        assert!(!lines.contains("Next steps (yours)"), "{lines}");
    }

    #[test]
    fn the_eligibility_rule_is_code_on_the_arguments() {
        let base = Facts::from_args(
            &json!({"in_flight": "SQ 890", "in_date": "2026-10-29", "out_flight": "CX 500",
                                            "out_date": "2026-10-30", "tax_hkd": 120}),
        );
        assert_eq!(ineligible(&base, today(), 28), None);
        let late = NaiveDate::from_ymd_opt(2026, 11, 28).unwrap();
        assert_eq!(ineligible(&base, late, 28).unwrap().0, "too_late");
        let early = NaiveDate::from_ymd_opt(2026, 10, 29).unwrap();
        assert_eq!(ineligible(&base, early, 28).unwrap().0, "not_departed");
        let mut f = base.clone();
        f.out_date = NaiveDate::from_ymd_opt(2026, 10, 31);
        assert!(ineligible(&f, today(), 28)
            .unwrap()
            .1
            .contains("same or the next calendar day"));
        let mut f = base.clone();
        f.tax = Some(0);
        assert_eq!(ineligible(&f, today(), 28).unwrap().0, "no_tax");
        // ineligible: the result says why, and the platform is never opened
        let mut a = facts();
        a["out_date"] = json!("2026-10-31");
        for k in ["in_pass_file", "out_pass_file", "receipt_file"] {
            a[k] = json!("");
        }
        let (t, args) = started(docs_asked_chat("apply"), a, false);
        let (t2, res) = run(t, &args, FILLED);
        assert_eq!(
            (res["status"].as_str(), res["reason"].as_str()),
            (Some("not_eligible"), Some("stayed_longer"))
        );
        assert!(super::super::show::lines(&res, false)[0]
            .starts_with("- Not eligible for the Hong Kong departure tax refund: "));
        assert!(calls_of(&t2, "browser_navigate").is_empty());
    }

    #[test]
    fn ask_user_before_the_tool_is_refused() {
        let ev = |ty: &str, data: Value| json!({"type": ty, "data": data});
        let loaded = vec![ev(
            "tool_call",
            json!({"name": "load_skill", "args": {"name": APDT.skill}}),
        )];
        assert_eq!(ask_before_tool(&loaded).unwrap()["tool"], APDT_REFUND);
        let mut called = loaded.clone();
        called.push(ev("workflow_call", json!({"tool": APDT_REFUND})));
        assert!(ask_before_tool(&called).is_none());
    }

    #[test]
    fn the_documents_uploaded_save_the_application_and_its_reference_number_opens_it_anywhere() {
        // the user, 2026-11-02: "give me a link with the reference code so that I can open my session" — the
        // recognised passport saves the application (mapped with the user's own documents); its reference number,
        // with the travel document number, opens it on any device
        let user = vec![
            json!({"role": "user", "content": "Please apply for my HK departure tax refund. [attached: uploads/passport.jpg, uploads/bp_in.jpg, uploads/bp_out.jpg, uploads/receipt.pdf]"}),
        ];
        let mut a = facts();
        a["passport_file"] = json!("uploads/passport.jpg");
        a["in_pass_file"] = json!("uploads/bp_in.jpg");
        a["out_pass_file"] = json!("uploads/bp_out.jpg");
        a["receipt_file"] = json!("uploads/receipt.pdf");
        let (t, args) = started(user.clone(), a.clone(), false);
        let (t, res) = run(t, &args, FILLED);
        assert_eq!(
            calls_of(&t, "form_fill")[0]["passport_file"],
            "uploads/passport.jpg"
        );
        assert_eq!(
            calls_of(&t, "browser_read_elements"),
            [json!({"read": [{"name": "reference", "sel": ".bottom span.card-number.code"}]})]
        );
        assert_eq!(
            (res["status"].as_str(), res["reference"].as_str()),
            (Some("saved"), Some("F4FT23"))
        );
        let lines = super::super::show::lines(&res, false).join("\n");
        assert!(
            lines.contains("- Application reference: **F4FT23**")
                && lines.contains("View Application / Continue Application")
                && lines.contains("travel document number")
                && lines.contains("not submitted")
                && lines.contains(HOME),
            "{lines}"
        );
        assert!(calls_of(&t, "browser_request_takeover").is_empty());
        // the photo not recognised: no reference, the facts and the link, nothing submitted
        let unrecognised = r#"{"url": "x", "filled": [{"op": "file", "field": "passport", "ok": true, "note": "unrecognised"}]}"#;
        let (t, args) = started(user, a, false);
        let (_, res) = run(t, &args, unrecognised);
        assert_eq!(res["status"], "stopped");
        assert!(super::super::show::lines(&res, false)
            .join("\n")
            .contains("did not recognise"));
        // the arrival left out (the user adds it on the platform): the departure rule still holds, and the steps say
        // to add the arrival
        let mut b = facts();
        for k in ["in_flight", "in_date", "in_pass_file"] {
            b.as_object_mut().unwrap().remove(k);
        }
        b["passport_file"] = json!("uploads/passport.jpg");
        b["out_pass_file"] = json!("uploads/bp_out.jpg");
        b["receipt_file"] = json!("uploads/receipt.pdf");
        let user2 = vec![
            json!({"role": "user", "content": "Apply please. [attached: uploads/passport.jpg, uploads/bp_out.jpg, uploads/receipt.pdf]"}),
        ];
        let (t, args) = started(skipped(user2), b, false);
        let (_, res) = run(t, &args, FILLED);
        assert_eq!(res["status"], "saved");
        let lines = super::super::show::lines(&res, false).join("\n");
        assert!(
            lines.contains("Arrival into Hong Kong: not given")
                && lines.contains("add your arrival into Hong Kong"),
            "{lines}"
        );
        // only the travel document given, the rest skipped: the session link, and the three documents still to upload
        let mut c = facts();
        for k in [
            "in_flight",
            "in_date",
            "in_pass_file",
            "out_pass_file",
            "receipt_file",
        ] {
            c.as_object_mut().unwrap().remove(k);
        }
        c["passport_file"] = json!("uploads/passport.jpg");
        let user3 = skipped(vec![
            json!({"role": "user", "content": "Apply please. [attached: uploads/passport.jpg]"}),
        ]);
        let (t, args) = started(user3, c, false);
        let (t, res) = run(t, &args, FILLED);
        assert_eq!(res["status"], "saved");
        assert_eq!(res["to_upload"].as_array().map(Vec::len), Some(3), "{res}");
        let fill = &calls_of(&t, "form_fill")[0];
        assert!(
            fill.get("out_pass_file").is_none() && fill.get("receipt_file").is_none(),
            "{fill}"
        );
        let lines = super::super::show::lines(&res, false).join("\n");
        assert!(
            lines.contains("Still to upload (yours): the departure flight's boarding pass;"),
            "{lines}"
        );
        // the documents attached and the call names none: sent back to the model to map them (no question to the user)
        let mut bare_call = facts();
        for k in ["in_pass_file", "out_pass_file", "receipt_file"] {
            bare_call.as_object_mut().unwrap().remove(k);
        }
        let attached = vec![
            json!({"role": "user", "content": "Apply please. [attached: uploads/passport.jpg, uploads/bp_out.jpg]"}),
        ];
        match checked(bare_call, &attached, false) {
            Checked::Invalid(p) => assert!(
                p[0].contains("uploads/passport.jpg") && p[0].contains("passport_file"),
                "{p:?}"
            ),
            other => panic!("{other:?}"),
        }
        // said once: the next call goes on with what it names (the tool always runs on what the user gave)
        let mut again = attached.clone();
        again.extend(step(
            "call_b",
            APDT_REFUND,
            &json!({}),
            &json!({"status": "invalid", "problems": [format!("{MAP_FILES} uploads/passport.jpg: pass each document")]})
                .to_string(),
        ));
        let mut bare_again = facts();
        for k in ["in_pass_file", "out_pass_file", "receipt_file"] {
            bare_again.as_object_mut().unwrap().remove(k);
        }
        assert!(matches!(
            checked(bare_again, &skipped(again), false),
            Checked::Valid(_)
        ));
        // a value that is no reference number is not one
        let odd = json!({"role": "tool", "content": json!({"elements": {"reference": ["<", "b", ">"]}}).to_string()});
        assert_eq!(reference_of(&odd), None);
    }

    #[test]
    fn the_documents_are_fields_of_the_tool() {
        // apdt_real 2026-11-03: the model said the tool had no fields for the files and could not pass them
        let schema = json!(tool_schemas()).to_string();
        for k in FILES {
            assert!(schema.contains(&format!("\"{k}\"")), "{k}");
        }
    }

    #[test]
    fn the_entry_mode_from_the_conversation_decides_the_platforms_path() {
        // the user, 2026-11-03: the entry mode is the model's JSON field from the conversation; by air it also asks the
        // arrival flight and its boarding pass; code takes the mode's own path on the platform
        let bare = vec![
            json!({"role": "user", "content": "Please apply for my Hong Kong departure tax refund."}),
        ];
        match checked(
            json!({"out_flight": "CX 500", "out_date": "2026-10-30", "refund_method": "card"}),
            &bare,
            false,
        ) {
            Checked::NeedsInput(m) => {
                assert!(m.iter().any(|x| x.starts_with("entry_mode")), "{m:?}")
            }
            other => panic!("{other:?}"),
        }
        match checked(
            json!({"entry_mode": "air", "out_flight": "CX 500", "out_date": "2026-10-30", "refund_method": "card"}),
            &bare,
            false,
        ) {
            Checked::NeedsInput(m) => {
                let m = m.join(" | ");
                assert!(
                    m.contains("into Hong Kong") && m.contains("in_pass_file"),
                    "{m}"
                );
            }
            other => panic!("{other:?}"),
        }
        // land / sea ("Land / Sea" as the model may write it): no arrival flight, no arrival boarding pass asked
        match checked(
            json!({"entry_mode": "Land / Sea", "out_flight": "CX 500", "out_date": "2026-10-30", "refund_method": "card"}),
            &bare,
            false,
        ) {
            Checked::NeedsInput(m) => {
                let m = m.join(" | ");
                assert!(
                    !m.contains("into Hong Kong")
                        && !m.contains("in_pass_file")
                        && m.contains("passport_file"),
                    "{m}"
                );
            }
            other => panic!("{other:?}"),
        }
        let no_files = || {
            let mut a = facts();
            for k in FILES {
                a.as_object_mut().unwrap().remove(*k);
            }
            a
        };
        let mut a = no_files();
        a["entry_mode"] = json!("land_sea");
        let (t, args) = started(docs_asked_chat("apply"), a, false);
        assert_eq!(args["entry_mode"], "land_sea");
        assert!(args.get("in_flight").is_none() && args.get("in_pass_file").is_none());
        let (t, res) = run(t, &args, FILLED);
        assert_eq!(calls_of(&t, "form_fill")[0]["entry_mode"], "land_sea");
        assert!(super::super::show::lines(&res, false)
            .join("\n")
            .contains("Entered Hong Kong by: land / sea"));
        // SkyPier: the platform's face check comes first — nothing opened, the user's steps
        let mut a = no_files();
        a["entry_mode"] = json!("skypier");
        let (t, args) = started(docs_asked_chat("apply"), a, false);
        let (t, res) = run(t, &args, FILLED);
        assert_eq!(res["status"], "skypier");
        assert!(calls_of(&t, "browser_navigate").is_empty());
        assert!(super::super::show::lines(&res, false)
            .join("\n")
            .contains("face check first"));
        // an unknown mode goes back to the model
        assert!(matches!(
            checked(json!({"entry_mode": "boat"}), &bare, false),
            Checked::Invalid(_)
        ));
    }
}
