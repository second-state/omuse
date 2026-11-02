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
//! waiting room, mapped 2026-11-02) through its notes, "Start", "New Application" and the entry mode up to the travel
//! document's upload, by its form function (`crate::forms`). The documents' photos, the refund method and the face
//! recognition are the user's own steps, on their phone: never uploaded from a made-up file, never handed over
//! (`guards::apdt_face_guard`). The machine ends with [`Step::Done`]: a JSON result (status, the rule's verdict, the
//! amount, the deadline, the steps left and the link — as `show`, appended to the reply by code — and `hints`); the
//! model writes the reply from it.

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
        "Apply for the Hong Kong air passenger departure tax (APDT) refund on the Airport Authority's platform — only \
when the user asks to apply. Call it with the flights the user gave (or the boarding passes and the receipt show) and the \
refund method: no document is needed for the call — the user uploads the documents themselves, on their phone. When \
the flights are not given and a mailbox is connected, code fetches the documents (status mail_read: read the boarding \
passes and the receipt just read and call again with the flights). Code checks the rule (left Hong Kong the same or the next day, \
within 28 days, the tax paid), opens the platform and goes through its notes and entry mode to the document upload, \
and stops there: the documents' photos, the refund method and the face recognition are the user's, on their phone. \
The JSON result has the rule's verdict, the deadline, the amount and the link (status ready) or says why not. status \
needs_input: ask the user for the missing fields, then call again with every field; invalid: fix the arguments and call \
again. Write the reply from the result (code appends the facts in show to the reply).",
        json!({
            "in_flight": {"type": "string", "description": "the flight INTO Hong Kong: airline code and number (\"SQ 890\")"},
            "in_date": date("the day that flight arrived in Hong Kong"),
            "out_flight": {"type": "string", "description": "the flight OUT of Hong Kong (the departure the tax was paid on)"},
            "out_date": date("the day that flight left Hong Kong"),
            "tax_hkd": {"type": "integer", "description": "the Hong Kong Air Passenger Departure Tax the receipt shows (120 or 200); 0 when the receipt shows none"},
            "refund_method": {"type": "string", "enum": ["card", "cash", "alipay", "wechat", "alipayHK", "wechatHK"], "description": "how the user wants the refund"},
            "wallet_account": {"type": "string", "description": "the e-wallet account the refund goes to (Alipay / WeChat Pay)"}
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
    if !problems.is_empty() {
        return Checked::Invalid(problems);
    }
    // the trip decides the rule (the same or the next day, within 28 days): known before the platform opens
    let trip_known = ["in_flight", "in_date", "out_flight", "out_date"]
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
    if !out.contains_key("passport_file") {
        if let Some(p) = passport_default(&user_files(cx.transcript)) {
            out.insert("passport_file".into(), json!(p));
        }
    }
    let has = |k: &str| out.contains_key(k);
    let mut missing: Vec<String> = Vec::new();
    if !(has("in_flight") && has("in_date")) {
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
    if !missing.is_empty() {
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
    let (Some(a), Some(d)) = (f.in_date, f.out_date) else {
        return None;
    };
    let gap = (d - a).num_days();
    if !(0..=1).contains(&gap) {
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
        return Step::Calls(vec![call(
            n,
            "browser_navigate",
            json!({"url": s.start_url}),
        )]);
    }
    // 3. the notes, "Start", "New Application" and the entry mode, up to the travel document's upload (its form
    //    function, `crate::forms`); the photos from there on are the user's
    let Some((_, fill)) = results_of(t, "form_fill")
        .into_iter()
        .find(|(a, _)| sget(a, "stage") == "start")
    else {
        return Step::Calls(vec![call(n, "form_fill", json!({"stage": "start"}))]);
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
    Step::Done(result(&f, m.today))
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
    let mut show = vec![S::fact(
        "Eligible",
        "符合条件",
        &format!(
            "arrived on {} {}, left on {} {} — the same or the next calendar day",
            f.in_flight,
            day(f.in_date),
            f.out_flight,
            day(f.out_date)
        ),
    )];
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

    fn checked(args: Value, t: &[Value], mail_ready: bool) -> Checked {
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
        json!({"in_flight": "SQ 890", "in_date": "2026-10-29", "out_flight": "CX 500", "out_date": "2026-10-30",
               "in_pass_file": SQ_PASS, "out_pass_file": CX_PASS, "receipt_file": RECEIPT, "tax_hkd": 120,
               "refund_method": "card"})
    }

    const FILLED: &str = r#"{"url": "https://app.apdtrefund.hk/hkTaxMobile/process/upload", "filled": [{"op": "click", "field": "entry_air", "ok": true}]}"#;

    #[test]
    fn the_trip_and_the_refund_method_are_asked_before_the_platform_the_documents_are_not() {
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
                assert!(
                    m.contains("into Hong Kong")
                        && m.contains("out of Hong Kong")
                        && m.contains("refund_method"),
                    "{m}"
                );
                assert!(
                    !m.contains("passport_file") && !m.contains("receipt_file"),
                    "{m}"
                );
            }
            other => panic!("{other:?}"),
        }
        // the trip given in words: valid with no files
        match checked(
            json!({"in_flight": "sq890", "in_date": "2026-10-29", "out_flight": "CX 500", "out_date": "2026-10-30", "refund_method": "card"}),
            &bare,
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
        let (t, args) = started(t, facts(), true);
        let (t, res) = run(t, &args, FILLED);
        assert_eq!(res["status"], "ready");
        assert_eq!(calls_of(&t, "browser_navigate")[0]["url"], APDT.start_url);
        assert_eq!(calls_of(&t, "form_fill"), [json!({"stage": "start"})]);
        // no upload, no card, no hand-over: the documents and the face recognition are the user's, on their phone
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
        let bare = vec![json!({"role": "user", "content": "apply"})];
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
        let (t, args) = started(vec![json!({"role": "user", "content": "apply"})], a, false);
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
}
