//! The Hong Kong air passenger departure tax refund behind the workflow tool `apdt_refund` (`hk-apdt-refund`;
//! docs/json-tools.md, the protocol is [`super::workflow`]).
//!
//! The model reads the user's words — and the boarding passes and the receipt the machine fetched — and calls the
//! tool with JSON arguments: the flights into and out of Hong Kong with their dates, the documents' paths, the
//! departure tax the receipt shows, the passport photo the user attached, the refund method (and the e-wallet account,
//! the Vault card the user named). [`check`] validates them in code (flight numbers, dates, the files are this chat's,
//! the method is the platform's) and decides what happens before the platform opens: the fixed mail search when the
//! documents are not known yet (the machine saves the attachments and reads them; its result `mail_read` has the model
//! call again with the facts in them), `needs_input` for what only the user can give. Nothing here reads the user's
//! wording, a mail's text or a document's text.
//!
//! Then every step is code ([`next`]): the eligibility rule (left Hong Kong the same or the next calendar day, within
//! 28 days of the departure flight, after departing, the tax paid), the card from the Vault, the platform's pages
//! filled by their form functions (`crate::forms`, fixed ids), the uploads into their boxes (refs from the page's fixed
//! labels), the card by `browser_fill_secret` (a card the Vault cannot fill is typed by the user in a takeover), the
//! reference read from the page. The face recognition is the user's own step on their phone: never opened, never
//! handed over (`guards::apdt_face_guard`). The machine ends with [`Step::Done`]: a JSON result (status, the
//! reference, the link, the deadline, the refund, `must_say`); the model writes the reply from it.

use std::sync::LazyLock;

use chrono::{Duration, NaiveDate};
use omuse_common::util::{get_str, truthy};
use regex::Regex;
use serde_json::{json, Map, Value};

use super::claims::vault_cards;
use super::driver::{
    handed_back, json_of, norm_flight, pos, results_of, snapshot_text, walk_messages, Step,
};
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
    let s = json!({"type": "string"});
    let date = |what: &str| json!({"type": "string", "description": format!("YYYY-MM-DD: {what}")});
    let file = |what: &str| json!({"type": "string", "description": format!("the workspace path of {what} (a path this chat shows: attachments/… or uploads/…)")});
    vec![fn_(
        APDT_REFUND,
        "Apply for the Hong Kong air passenger departure tax (APDT) refund on the Airport Authority's platform — only \
when the user asks to apply. Pass what the user gave and what the boarding passes and the receipt show. When the \
documents are not given and a mailbox is connected, code fetches them (status mail_read: read the boarding passes and \
the receipt just read and call again with every field). Code checks the rule (left Hong Kong the same or the next day, \
within 28 days, the tax paid), fills every page and uploads every document, the card from the Vault, and stops before \
the face recognition, which is the user's own step on their phone. The JSON result has the reference and the link \
(status confirmed) or says why not. status needs_input: ask the user for the missing fields, then call again with \
every field; invalid: fix the arguments and call again. Write the reply from the result, with every fact in its \
must_say.",
        json!({
            "in_flight": {"type": "string", "description": "the flight INTO Hong Kong: airline code and number (\"SQ 890\")"},
            "in_date": date("the day that flight arrived in Hong Kong"),
            "out_flight": {"type": "string", "description": "the flight OUT of Hong Kong (the departure the tax was paid on)"},
            "out_date": date("the day that flight left Hong Kong"),
            "in_pass_file": file("the boarding pass of the flight into Hong Kong"),
            "out_pass_file": file("the boarding pass of the flight out of Hong Kong"),
            "receipt_file": file("the e-ticket receipt or itinerary that shows the departure tax"),
            "tax_hkd": {"type": "integer", "description": "the Hong Kong Air Passenger Departure Tax the receipt shows (120 or 200); 0 when the receipt shows none"},
            "passport_file": file("the photo / scan of the passport photo page the user attached"),
            "refund_method": {"type": "string", "enum": ["card", "cash", "alipay", "wechat", "alipayHK", "wechatHK"], "description": "how the user wants the refund"},
            "wallet_account": {"type": "string", "description": "the e-wallet account the refund goes to (Alipay / WeChat Pay)"},
            "card": {"type": "string", "description": "the Vault card the user named for the refund: its id from vault_list (id=…), its card_id, or its label exactly as listed"},
            "address": s
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
    "card",
    "address",
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
    for k in ["wallet_account", "card", "address"] {
        let v = s_arg(&a, k);
        if !v.is_empty() {
            out.insert(k.into(), json!(v));
        }
    }
    if !problems.is_empty() {
        return Checked::Invalid(problems);
    }
    let docs_known = [
        "in_flight",
        "in_date",
        "out_flight",
        "out_date",
        "in_pass_file",
        "out_pass_file",
        "receipt_file",
    ]
    .iter()
    .all(|k| out.contains_key(*k));
    // the documents not known yet: fetched and read first (the mail's, and the PDFs the user attached)
    let user_pdfs: Vec<String> = user_files(cx.transcript)
        .into_iter()
        .filter(|f| f.to_lowercase().ends_with(".pdf"))
        .collect();
    if !docs_known && (cx.mail_ready || !user_pdfs.is_empty()) && !docs_fetched(cx.transcript) {
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
    if !has("passport_file") {
        missing.push(
            "passport_file (a photo or scan of the passport photo page, attached here)".into(),
        );
    }
    if !(has("in_flight") && has("in_date") && has("in_pass_file")) {
        missing.push(
            "the boarding pass of the flight into Hong Kong (in_flight, in_date, in_pass_file)"
                .into(),
        );
    }
    if !(has("out_flight") && has("out_date") && has("out_pass_file")) {
        missing.push("the boarding pass of the flight out of Hong Kong (out_flight, out_date, out_pass_file)".into());
    }
    if !has("receipt_file") {
        missing.push(
            "receipt_file (the e-ticket receipt or itinerary that shows the departure tax)".into(),
        );
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

// ---------------------------------------------------------------- the platform's pages

/// The address of a browser result (the envelope's `url`, else its `source="web page …"`).
fn url_of(m: &Value) -> String {
    static SRC: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r#"source="web page (https?://[^"\s]+)"#).unwrap());
    let c = crate::links::decode(&content_of(m));
    let v = json_of(m);
    let u = get_str(&v, "url");
    if !u.is_empty() {
        return crate::links::decode(&u);
    }
    SRC.captures(&c)
        .map(|x| x[1].to_string())
        .unwrap_or_default()
}

/// The page a URL is on: the start page, the upload, refund, confirm or submit page (the face page is never opened).
fn page_of(url: &str) -> &'static str {
    let path = url.split('?').next().unwrap_or("").to_lowercase();
    for p in [
        "apdt_upload",
        "apdt_refund",
        "apdt_confirm",
        "apdt_submit",
        "apdt_face",
    ] {
        if path.contains(p) {
            return p;
        }
    }
    if path.contains("hktaxmobile") {
        "start"
    } else {
        ""
    }
}

/// The upload boxes of the upload page in a snapshot: (box, ref) — each file chooser named by the text since the one
/// before it (its section's heading and label): the travel document, the arrival and the departure boarding pass, the
/// proof of tax payment (a supplementary box is left alone).
pub fn upload_boxes(snapshot: &str) -> Vec<(&'static str, String)> {
    static REF: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r#"^\[(e\d+)\] filechooser\b"#).unwrap());
    let mut out: Vec<(&'static str, String)> = Vec::new();
    let mut chunk = String::new();
    for line in snapshot.lines() {
        let l = line.trim();
        chunk.push(' ');
        chunk.push_str(&l.to_lowercase());
        let Some(c) = REF.captures(l) else {
            continue;
        };
        let what = if chunk.contains("supplementary") {
            ""
        } else if chunk.contains("tax payment") {
            "paydoc"
        } else if chunk.contains("departure flight") {
            "outboard"
        } else if chunk.contains("arrival flight") {
            "inboard"
        } else if chunk.contains("travel document") || chunk.contains("passport") {
            "doc"
        } else {
            ""
        };
        if !what.is_empty() && !out.iter().any(|(w, _)| *w == what) {
            out.push((what, c[1].to_string()));
        }
        chunk.clear();
    }
    out
}

/// The ref of the textbox labelled `label` in a snapshot.
fn textbox(snapshot: &str, label: &str) -> Option<String> {
    let r = Regex::new(&format!(r#"\[(e\d+)\] textbox "{}"#, regex::escape(label))).ok()?;
    r.captures(snapshot).map(|c| c[1].to_string())
}

/// The ref of the textbox labelled `label` in the newest browser result after position `after` that still shows it
/// (a snapshot shortened by the transcript's compression no longer does).
fn newest_ref(transcript: &[Value], after: usize, label: &str) -> Option<String> {
    transcript[after.min(transcript.len())..]
        .iter()
        .rev()
        .filter(|m| get_str(m, "role") == "tool")
        .find_map(|m| textbox(&snapshot_text(m), label))
}

/// The platform's error messages in a snapshot (under a field, or for the whole page), in its own words.
pub fn site_errors(snapshot: &str) -> Vec<String> {
    static ERR: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?i)please (upload|enter|select|check|read)|invalid|exceeded|no matching|must be|cannot be|could not be|not accepted|too large|is later than|do not upload|record not found|not eligible").unwrap()
    });
    let mut out: Vec<String> = Vec::new();
    for l in snapshot.lines().map(str::trim) {
        if l.starts_with('[') || !ERR.is_match(l) {
            continue;
        }
        let l: String = l.chars().take(220).collect();
        if !out.contains(&l) {
            out.push(l);
        }
        if out.len() >= 5 {
            break;
        }
    }
    out
}

/// The reference the platform issued: the `bizId` parameter the submit page's address carries (a fixed part of the
/// known page), in its format (4–12 letters and digits); nothing is looked for in the page's text.
pub fn reference_on(url: &str) -> Option<String> {
    let u = url::Url::parse(url).ok()?;
    let (_, v) = u.query_pairs().find(|(k, _)| k == "bizId")?;
    let ok = (4..=12).contains(&v.len()) && v.chars().all(|c| c.is_ascii_alphanumeric());
    ok.then(|| v.to_string())
}

/// The first result of `tool` after position `after` (and its position).
fn first_after<'a>(
    transcript: &'a [Value],
    tool: &str,
    after: usize,
) -> Option<(Value, &'a Value, usize)> {
    results_of(transcript, tool)
        .into_iter()
        .map(|(a, m)| {
            let p = pos(transcript, m);
            (a, m, p)
        })
        .find(|(_, _, p)| *p > after)
}

/// The fill of `stage` (once per run, after position `after`): `Err(step)` to run it, `Ok(position)` once it ran.
fn stage(
    transcript: &[Value],
    n: usize,
    name: &str,
    after: usize,
    mut args: Value,
) -> Result<usize, Step> {
    args["stage"] = json!(name);
    match results_of(transcript, "form_fill")
        .into_iter()
        .rfind(|(a, m)| sget(a, "stage") == name && pos(transcript, m) > after)
    {
        Some((_, m)) => Ok(pos(transcript, m)),
        None => Err(Step::Calls(vec![call(n, "form_fill", args)])),
    }
}

/// The page right after position `after` (the first snapshot since): `Err(step)` to take it, `Ok((url, text,
/// position))`.
fn snap_after(
    transcript: &[Value],
    n: usize,
    after: usize,
) -> Result<(String, String, usize), Step> {
    match first_after(transcript, "browser_snapshot", after) {
        Some((_, m, p)) => Ok((url_of(m), snapshot_text(m), p)),
        None => Err(Step::Calls(vec![call(
            n,
            "browser_snapshot",
            json!({"max_chars": 12000}),
        )])),
    }
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

/// Everything the application needs, from the validated arguments.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Facts {
    pub in_flight: String,
    pub in_date: Option<NaiveDate>,
    pub out_flight: String,
    pub out_date: Option<NaiveDate>,
    pub in_file: String,
    pub out_file: String,
    pub receipt_file: String,
    /// the departure tax the receipt shows (HK$), when the arguments say
    pub tax: Option<u32>,
    pub passport: String,
    pub method: Option<Method>,
    /// the Vault card the user named
    pub card: String,
    pub address: String,
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
            in_file: g("in_pass_file"),
            out_file: g("out_pass_file"),
            receipt_file: g("receipt_file"),
            tax: a.get("tax_hkd").and_then(Value::as_u64).map(|t| t as u32),
            passport: g("passport_file"),
            method,
            card: g("card"),
            address: g("address"),
        }
    }

    /// The `form_fill` facts (the platform's flight fields take the number without the space); the card picked
    /// before the platform opened rides along (`kept`): a call's arguments survive the transcript's compression, the
    /// Vault list does not.
    fn args(&self, card: Option<&Card>, holder: &str) -> Value {
        let d = |x: Option<NaiveDate>| {
            x.map(|d| d.format("%Y-%m-%d").to_string())
                .unwrap_or_default()
        };
        let (method, account) = match &self.method {
            Some(Method::Card) => ("card".to_string(), String::new()),
            Some(Method::Cash) => ("cash".to_string(), String::new()),
            Some(Method::Wallet(w, a)) => (w.clone(), a.clone()),
            None => (String::new(), String::new()),
        };
        json!({"in_flight": self.in_flight.replace(' ', ""), "in_date": d(self.in_date),
               "out_flight": self.out_flight.replace(' ', ""), "out_date": d(self.out_date),
               "method": method, "account": account, "holder": holder,
               "card": if card.is_some_and(|c| !c.name_only) { "vault" } else { "type" },
               "address": self.address,
               "kept": {"card": card.map(|c| json!({"id": c.id, "label": c.label, "name_only": c.name_only,
                                                     "last4": c.last4, "holder": c.holder}))}})
    }
}

/// The refund card from the Vault: the one the user named, else the first with its details, else the first.
#[derive(Debug, Clone, PartialEq)]
pub struct Card {
    pub id: String,
    pub label: String,
    /// saved by its name only (number UNKNOWN): the user types it
    pub name_only: bool,
    pub last4: String,
    /// the Vault keeps the holder's name
    pub holder: bool,
}

/// The card of a `vault_list` result: the one the tool argument names exactly (its Vault id, its card_id or its whole
/// label — `claims::pick_card`), else the first with its details, else the first.
fn vault_card(text: &str, named: &str) -> Option<Card> {
    static L4: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"•+\s*(\d{4})").unwrap());
    let cards = vault_cards(text);
    let c = super::claims::pick_card(&cards, &[named.to_string()])?;
    let line = text
        .lines()
        .find(|l| l.contains(&format!("id={} ", c.id)))
        .unwrap_or("");
    Some(Card {
        id: c.id.clone(),
        label: c.label.clone(),
        name_only: c.name_only,
        last4: L4
            .captures(line)
            .map(|x| x[1].to_string())
            .unwrap_or_default(),
        holder: line.contains("holder"),
    })
}

/// The card the start page's fill kept.
fn kept_card(a: &Value) -> Option<Card> {
    let c = &a["kept"]["card"];
    c.is_object().then(|| Card {
        id: get_str(c, "id"),
        label: get_str(c, "label"),
        name_only: c["name_only"].as_bool().unwrap_or(false),
        last4: get_str(c, "last4"),
        holder: c["holder"].as_bool().unwrap_or(false),
    })
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
        return Step::Done(json!({"status": MAIL_READ, "files": files, "must_say": []}));
    }
    let opened = results_of(t, "browser_navigate")
        .iter()
        .any(|(a, _)| sget(a, "url").contains(s.host));
    // 2. the rule, then the card from the Vault, then the platform
    if !opened {
        if let Some((code, why)) = ineligible(&f, m.today, s.window_days) {
            return Step::Done(
                json!({"status": "not_eligible", "reason": code, "rule": why,
                "must_say": [format!("not eligible for the Hong Kong departure tax refund: {why}"), "nothing was submitted"]}),
            );
        }
        if f.method == Some(Method::Card) && results_of(t, "vault_list").is_empty() {
            return Step::Calls(vec![call(n, "vault_list", json!({}))]);
        }
        return Step::Calls(vec![call(
            n,
            "browser_navigate",
            json!({"url": s.start_url}),
        )]);
    }
    // the card: the one the start page's fill kept (the Vault list may have been compressed since), else the list's
    let card = match results_of(t, "form_fill")
        .into_iter()
        .find(|(a, _)| sget(a, "stage") == "start")
    {
        Some((a, _)) => kept_card(&a),
        None if f.method == Some(Method::Card) => results_of(t, "vault_list")
            .last()
            .and_then(|(_, msg)| vault_card(&content_of(msg), &f.card)),
        None => None,
    };
    let card = card.as_ref();
    let holder = get_str(m.profile, "name_en").trim().to_uppercase();
    let args_fill = f.args(card, &holder);
    let stuck = |url: &str, text: &str, wanted: &str| -> Step {
        let errs = site_errors(text);
        let page = if page_of(url).is_empty() {
            "current"
        } else {
            page_of(url)
        };
        let mut must = vec![format!(
            "the refund application stopped on the platform's {page} page instead of the {wanted} page"
        )];
        if !errs.is_empty() {
            must.push(format!("the site says: {}", errs.join(" ")));
        }
        must.push("the face recognition was not started; nothing was submitted".into());
        Step::Done(
            json!({"status": "stopped", "page": page, "expected": wanted, "site_errors": errs,
            "url": url, "must_say": must}),
        )
    };
    // a page is checked only until the stage after it ran: the transcript's compression may have shortened an older
    // snapshot since (apdt_real 2026-11-01: the saved-reference page read again after the card fill, cut short)
    let ran = |name: &str| {
        results_of(t, "form_fill")
            .iter()
            .any(|(a, _)| sget(a, "stage") == name)
    };
    // 3. the notes and "Start New Application" → the upload page
    let start = match stage(t, n, "start", 0, args_fill.clone()) {
        Ok(p) => p,
        Err(st) => return st,
    };
    let (url, text, snap_pos) = match snap_after(t, n, start) {
        Ok(x) => x,
        Err(st) => return st,
    };
    if !ran("details") && page_of(&url) != "apdt_upload" {
        return stuck(&url, &text, "upload");
    }
    // 4. the uploads, each in its box (refs from the upload page's snapshot)
    let files = [
        ("doc", f.passport.clone()),
        ("inboard", f.in_file.clone()),
        ("outboard", f.out_file.clone()),
        ("paydoc", f.receipt_file.clone()),
    ];
    // a box's ref from the newest page that still lists it (an upload's result is the page again; the first snapshot
    // may have been compressed by then)
    let box_ref = |what: &str| {
        t[snap_pos.saturating_sub(1).min(t.len())..]
            .iter()
            .rev()
            .filter(|msg| get_str(msg, "role") == "tool")
            .find_map(|msg| {
                upload_boxes(&snapshot_text(msg))
                    .into_iter()
                    .find(|(w, _)| *w == what)
                    .map(|(_, r)| r)
            })
    };
    let uploads = results_of(t, "browser_upload");
    let mut uploaded = snap_pos;
    for (what, path) in &files {
        if path.is_empty() {
            continue;
        }
        let tries: Vec<&(Value, &Value)> = uploads
            .iter()
            .filter(|(a, msg)| sget(a, "path") == *path && pos(t, msg) > snap_pos)
            .collect();
        if let Some((_, msg)) = tries.iter().find(|(_, msg)| !failed(msg)) {
            uploaded = uploaded.max(pos(t, msg));
            continue;
        }
        if let Some((_, msg)) = tries.first() {
            let file = path.rsplit('/').next().unwrap_or(path);
            let err: String = content_of(msg).chars().take(160).collect();
            return Step::Done(json!({"status": "error", "file": file, "error": err,
                "must_say": [format!("{file} could not be uploaded to the platform: {err}"), "nothing was submitted"]}));
        }
        let Some(r) = box_ref(what) else {
            return stuck(&url, &text, "upload");
        };
        return Step::Calls(vec![call(
            n,
            "browser_upload",
            json!({"ref": r, "path": path}),
        )]);
    }
    // 5. the details, the declarations and "Next Step" → the reference; "Continue" → the refund page
    let details = match stage(t, n, "details", uploaded, args_fill.clone()) {
        Ok(p) => p,
        Err(st) => return st,
    };
    let (url, text, details_snap) = match snap_after(t, n, details) {
        Ok(x) => x,
        Err(st) => return st,
    };
    let low = text.to_lowercase();
    if !ran("continue")
        && !(low.contains("uploaded successfully") || low.contains("saved automatically"))
    {
        return stuck(&url, &text, "reference");
    }
    let cont = match stage(t, n, "continue", details_snap, json!({})) {
        Ok(p) => p,
        Err(st) => return st,
    };
    let (url, text, refund_snap) = match snap_after(t, n, cont) {
        Ok(x) => x,
        Err(st) => return st,
    };
    if !ran("refund") && page_of(&url) != "apdt_refund" {
        return stuck(&url, &text, "refund method");
    }
    // 6. the refund method; the card from the Vault (or typed by the user), then "Next"
    let refund = match stage(t, n, "refund", refund_snap, args_fill.clone()) {
        Ok(p) => p,
        Err(st) => return st,
    };
    let mut before_next = refund;
    if f.method == Some(Method::Card) {
        let (_, text, card_snap) = match snap_after(t, n, refund) {
            Ok(x) => x,
            Err(st) => return st,
        };
        let fills = results_of(t, "browser_fill_secret");
        let done = |field: &str| {
            fills
                .iter()
                .find(|(a, msg)| sget(a, "field") == field && pos(t, msg) > card_snap)
                .map(|(_, msg)| !failed(msg))
        };
        let vault_ok = card.is_some_and(|c| !c.name_only) && done("number") != Some(false);
        if let (true, Some(c)) = (vault_ok, card) {
            let wanted: &[(&str, &str)] = if c.holder {
                &[
                    ("number", "Card Number"),
                    ("expiry", "Validity Period"),
                    ("holder", "Name of Cardholder"),
                ]
            } else {
                &[("number", "Card Number"), ("expiry", "Validity Period")]
            };
            for (field, label) in wanted {
                if done(field).is_some() {
                    continue;
                }
                if let Some(r) = textbox(&text, label).or_else(|| newest_ref(t, card_snap, label)) {
                    return Step::Calls(vec![call(
                        n,
                        "browser_fill_secret",
                        json!({"ref": r, "item_id": c.id, "field": field}),
                    )]);
                }
            }
            before_next = fills
                .iter()
                .map(|(_, msg)| pos(t, msg))
                .filter(|p| *p > card_snap)
                .max()
                .unwrap_or(card_snap);
        } else {
            // no complete card in the Vault: the user types it, in a takeover (the only one of the run)
            let takeovers = results_of(t, "browser_request_takeover");
            match takeovers.iter().rfind(|(_, msg)| pos(t, msg) > card_snap) {
                None => {
                    let what = match card {
                        Some(c) => format!("your {} is in the Vault by its name only", c.label),
                        None => "there is no card in your Vault".to_string(),
                    };
                    return Step::Calls(vec![call(
                        n,
                        "browser_request_takeover",
                        json!({"reason": format!("This step is yours: {what}. Type your card number for the refund into Card Number, then the expiry (MM/YY) into Validity Period; hand the browser back when done and I will finish the application.")}),
                    )]);
                }
                Some((_, msg)) if handed_back(msg) => before_next = pos(t, msg),
                Some(_) => {
                    return Step::Done(json!({"status": "handed_over", "page": "refund",
                        "must_say": ["the refund page is waiting in the browser tab for your card number and expiry",
                                     "nothing was submitted"]}))
                }
            }
        }
    }
    let next_pos = match stage(t, n, "next", before_next, json!({})) {
        Ok(p) => p,
        Err(st) => return st,
    };
    let (url, text, confirm_snap) = match snap_after(t, n, next_pos) {
        Ok(x) => x,
        Err(st) => return st,
    };
    if !ran("confirm") && page_of(&url) != "apdt_confirm" {
        return stuck(&url, &text, "confirmation");
    }
    // 7. "Please confirm your application information": the declaration and "Confirm and Next Step"
    let confirm = match stage(t, n, "confirm", confirm_snap, json!({})) {
        Ok(p) => p,
        Err(st) => return st,
    };
    let (url, text, _) = match snap_after(t, n, confirm) {
        Ok(x) => x,
        Err(st) => return st,
    };
    // 8. "Submit your application — Proceed with Facial Recognition": stop; the reference from the page
    if page_of(&url) != "apdt_submit" {
        return stuck(&url, &text, "face recognition");
    }
    Step::Done(result(&f, reference_on(&url).as_deref(), &url, card))
}

/// The machine's JSON result once the application is confirmed: the reference, the trip, what was uploaded, the
/// refund, and the face recognition with the application's own link and the last day for it.
pub fn result(f: &Facts, reference: Option<&str>, url: &str, card: Option<&Card>) -> Value {
    let leg = |no: &str, d: Option<NaiveDate>| json!({"flight": no, "date": d.map(|d| d.format("%Y-%m-%d").to_string()).unwrap_or_default()});
    let deadline = f.out_date.map(|d| d + Duration::days(APDT.window_days));
    let refund = match (&f.method, card) {
        (Some(Method::Card), Some(c)) if !c.last4.is_empty() => {
            json!({"method": "card", "card": c.label, "last4": c.last4,
            "fee": "up to HK$18", "arrives": "about 3 working days after approval"})
        }
        (Some(Method::Card), _) => {
            json!({"method": "card", "fee": "up to HK$18", "arrives": "about 3 working days after approval"})
        }
        (Some(Method::Cash), _) => {
            json!({"method": "cash", "where": "the airport's designated counter, after approval"})
        }
        (Some(Method::Wallet(w, _)), _) => {
            json!({"method": w, "fee": "the e-wallet's processing fee"})
        }
        (None, _) => Value::Null,
    };
    let common = json!({"trip": {"arrival": leg(&f.in_flight, f.in_date), "departure": leg(&f.out_flight, f.out_date),
                                  "tax_hkd": f.tax},
        "uploaded": ["the passport photo page", "both boarding passes", "the e-ticket receipt"],
        "identity": "name, date of birth and nationality from the profile, the passport number from the Vault (not shown)",
        "refund": refund});
    let Some(code) = reference else {
        let mut v = json!({"status": "not_confirmed", "home": HOME,
            "must_say": ["the refund application is filled in and confirmed, but no reference number showed on the page",
                         format!("find it with \"Resume Application\" at {HOME}"),
                         "the face recognition is yours, on your phone"]});
        v.as_object_mut()
            .unwrap()
            .extend(common.as_object().cloned().unwrap_or_default());
        return v;
    };
    let link = if url.contains("bizId=") { url } else { HOME };
    let mut must = vec![
        format!("reference number {code}"),
        format!("the face recognition is your step, on your phone: open {link} and press \"Proceed with Facial Recognition\""),
        format!("if asked to sign in again: \"Resume Application\" at {HOME} with your travel document number and the reference {code}"),
    ];
    if let Some(d) = deadline {
        must.push(format!(
            "do the face recognition by {} (28 days after the departure flight)",
            long(d)
        ));
    }
    if let (Some(Method::Card), Some(c)) = (&f.method, card) {
        if !c.last4.is_empty() {
            must.push(format!(
                "refund to your {} card ending {}",
                c.label, c.last4
            ));
        }
    }
    let mut v = json!({"status": "confirmed", "reference": code, "link": link, "home": HOME,
        "face_deadline": deadline.map(|d| d.format("%Y-%m-%d").to_string()), "must_say": must});
    v.as_object_mut()
        .unwrap()
        .extend(common.as_object().cloned().unwrap_or_default());
    v
}

#[cfg(test)]
mod tests {
    use super::super::driver::questions_after_navigate;
    use super::*;

    const BACK: &str = "\"Takeover finished: the user handed browser control back.\"";

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
the Hong Kong departure tax refund for me. The boarding passes and the e-ticket receipt are in my email, and I attached \
the photo page of my passport. Refund it to my Visa card.";

    const UPLOAD_SNAP: &str = "  Document Upload Supports image files or mobile document uploads\n  Entered Hong Kong by *\n\
[e4] radio \"Air (2 one-way air tickets)\" [unchecked]\n[e5] radio \"Land / Sea\" [unchecked]\n\
  Please select your mode of entry into Hong Kong first.\n  Travel document for entry into Hong Kong *\n\
[e7] button \"Why do you need to upload your travel document?\"\n[e8] combobox \"Document type *\" selected=\"Please select document type\" options=[Passport]\n\
[e9] filechooser \"Click here to upload (the page with your photo)\" (file upload)\n  Please ensure the name, ID number, and date of birth are clear.\n\
  Boarding pass of the relevant arrival flight in Hong Kong *\n[e10] button \"Why do you need to upload a boarding pass of the arrival flight?\"\n\
[e11] filechooser \"Click here to upload\" (file upload)\n[e12] textbox \"Arrival flight no. *\" value=\"\"\n[e13] textbox \"Date of arrival *\" value=\"\"\n\
  Boarding pass of the departure flight from Hong Kong *\n[e14] button \"Why do you need to upload the boarding pass for your departure flight from Hong Kong?\"\n\
[e15] filechooser \"Click here to upload\" (file upload)\n[e16] textbox \"Departure Flight No. *\" value=\"\"\n[e17] textbox \"Flight Departure Date *\" value=\"\"\n\
  Proof of tax payment *\n[e18] button \"Why do we require proof of tax payment?\"\n[e19] button \"Important Notes · Upload Example\"\n\
[e20] filechooser \"Click here to upload\" (file upload)\n  Other supplementary supporting documents (if applicable)\n\
[e21] filechooser \"Upload Extra Supplementary Documents\" (file upload)\n[e22] checkbox \"I confirm that I have authorized a third party to submit my information for this application\" [unchecked]\n\
[e23] button \"Previous Step\"\n[e24] button \"Next Step\"";

    const SAVED_SNAP: &str = "  Uploaded successfully. Your application has been saved automatically.\n  Reference number\n  K7P2QD\n[e30] button \"Continue\"";

    const REFUND_SNAP: &str = "  Refund Method\n[e5] radio \"Credit / Debit Card\" [checked]\n[e6] textbox \"Card Number *\" placeholder=\"Please enter card number\" value=[hidden]\n\
[e7] textbox \"Name of Cardholder *\" value=\"LUCAS LU\"\n[e8] textbox \"Validity Period *\" placeholder=\"MM/YY\" value=\"\"\n\
[e9] checkbox \"By selecting credit / debit card for my refund, I agree to a processing fee of up to HK$18\" [checked]\n[e10] button \"Next\"";

    const SUBMIT_URL: &str = "https://app.apdtrefund.hk/hkTaxMobile/apdt_submit.html?lang=en&bizId=K7P2QD&type=1&tsType=bank&t=eyJyZWYiOiJLN1AyUUQifQ";

    const SQ_PASS: &str = "attachments/BoardingPass_SQ890_LU_LUCAS.pdf";
    const CX_PASS: &str = "attachments/BoardingPass_CX500_LU_LUCAS.pdf";
    const RECEIPT: &str = "attachments/ETKT_1602345678901_Receipt.pdf";
    const PASSPORT: &str = "uploads/2026-11/passport.jpg";

    fn mails() -> Vec<(String, Value)> {
        vec![
            (
                "i-17".into(),
                json!({"from": "Singapore Airlines <boardingpass@singaporeair.example>",
                "subject": "Your boarding pass: SQ 890 Singapore to Hong Kong on 2026-10-29",
                "body": "You are checked in for SQ 890 from Singapore (SIN) to Hong Kong (HKG) on 2026-10-29.",
                "attachments": [{"filename": "BoardingPass_SQ890_LU_LUCAS.pdf", "type": "application/pdf"}]}),
            ),
            (
                "i-18".into(),
                json!({"from": "Cathay Pacific <eticket@cathaypacific.example>",
                "subject": "Your e-ticket receipt: CX 500 Hong Kong to Tokyo Narita, 2026-10-30",
                "body": "CX 500 Hong Kong (HKG) 09:05 to Tokyo Narita (NRT) 14:30 on 2026-10-30. Your receipt is attached.",
                "attachments": [{"filename": "ETKT_1602345678901_Receipt.pdf", "type": "application/pdf"}]}),
            ),
            (
                "i-19".into(),
                json!({"from": "Cathay Pacific <checkin@cathaypacific.example>",
                "subject": "Boarding pass: CX 500 Hong Kong to Tokyo Narita, 2026-10-30",
                "body": "You are checked in for CX 500 on 2026-10-30. Please find your boarding pass attached.",
                "attachments": [{"filename": "BoardingPass_CX500_LU_LUCAS.pdf", "type": "application/pdf"}]}),
            ),
        ]
    }

    /// The world's answer to one of the machine's calls (the stand-in platform's pages, page by page).
    fn respond(t: &[Value], name: &str, args: &Value, vault: &str) -> String {
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
            "vault_list" => vault.into(),
            "browser_navigate" => page("https://app.apdtrefund.hk/hkTaxMobile/?lang=en", "  Notes for Application"),
            // an action's result is the page it leaves, as a snapshot
            "browser_upload" => page("https://app.apdtrefund.hk/hkTaxMobile/apdt_upload.html?lang=en", UPLOAD_SNAP),
            "browser_fill_secret" => page("https://app.apdtrefund.hk/hkTaxMobile/apdt_refund.html?lang=en&bizId=K7P2QD", REFUND_SNAP),
            "browser_request_takeover" => BACK.into(),
            "form_fill" => json!({"url": "x", "filled": []}).to_string(),
            "browser_snapshot" => {
                // the page the last fill left
                let last = results_of(t, "form_fill").last().map(|(a, _)| sget(a, "stage")).unwrap_or_default();
                match last.as_str() {
                    "start" => page("https://app.apdtrefund.hk/hkTaxMobile/apdt_upload.html?lang=en", UPLOAD_SNAP),
                    "details" => page("https://app.apdtrefund.hk/hkTaxMobile/apdt_upload.html?lang=en", SAVED_SNAP),
                    "continue" | "refund" => page("https://app.apdtrefund.hk/hkTaxMobile/apdt_refund.html?lang=en&bizId=K7P2QD", REFUND_SNAP),
                    "next" => page("https://app.apdtrefund.hk/hkTaxMobile/apdt_confirm.html?lang=en&bizId=K7P2QD", "  Please confirm your application information"),
                    "confirm" => page(SUBMIT_URL, "  Submit your application K7P2QD Proceed with Facial Recognition\n[e9] button \"Proceed with Facial Recognition\""),
                    _ => page("https://app.apdtrefund.hk/hkTaxMobile/?lang=en", "  Notes for Application"),
                }
            }
            _ => "ok".into(),
        }
    }

    /// The user's request with the passport photo attached.
    fn user_msg(user: &str) -> Value {
        json!({"role": "user", "content": format!("{user}\n\n### passport.jpg — image, 60000 bytes, path: {PASSPORT}")})
    }

    fn step(id: &str, name: &str, args: &Value, result: &str) -> Vec<Value> {
        vec![
            json!({"role": "assistant", "content": "", "tool_calls": [{"id": id, "type": "function",
                   "function": {"name": name, "arguments": args.to_string()}}]}),
            json!({"role": "tool", "tool_call_id": id, "content": result}),
        ]
    }

    fn checked(args: Value, t: &[Value], mail_ready: bool) -> Checked {
        let p = profile();
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

    fn machine<'a>(p: &'a Value, t: &[Value], from: usize) -> Machine<'a> {
        Machine {
            profile: p,
            mail_ready: true,
            today: today(),
            first: t[..from]
                .iter()
                .flat_map(tool_calls)
                .filter(|tc| get_str(tc, "id").starts_with(PREFIX))
                .count(),
        }
    }

    /// Runs the machine from the latest `call_w` to its result against the stand-in platform; panics on a question after
    /// the platform opened. `squeeze`: once the platform is open, after each turn every tool result but the last two
    /// is cut as the transcript's compression cuts it.
    fn run_with(
        mut t: Vec<Value>,
        args: &Value,
        vault: &str,
        squeeze: bool,
    ) -> (Vec<Value>, Value) {
        let from = t
            .iter()
            .rposition(|x| tool_calls(x).iter().any(|tc| get_str(tc, "id") == "call_w"))
            .unwrap();
        let p = profile();
        for _ in 0..60 {
            if squeeze && !calls_of(&t, "browser_navigate").is_empty() {
                let tools: Vec<usize> = (0..t.len())
                    .filter(|&k| get_str(&t[k], "role") == "tool")
                    .collect();
                for &k in tools.iter().rev().skip(2) {
                    let c = content_of(&t[k]);
                    if c.chars().count() > 600 && !c.contains(crate::agent::guards::COMPRESSED_MARK)
                    {
                        let head: String = c.chars().take(500).collect();
                        t[k]["content"] = json!(format!(
                            "{head} … [{}]",
                            crate::agent::guards::COMPRESSED_MARK
                        ));
                    }
                }
            }
            let m = machine(&p, &t, from);
            match next(APDT_REFUND, args, &t[from..], &m) {
                Step::Calls(c) => {
                    for call in &c {
                        assert!(call.id.starts_with(PREFIX), "{}", call.id);
                        let result = respond(&t, &call.name, &call.args, vault);
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

    fn run(t: Vec<Value>, args: &Value, vault: &str) -> (Vec<Value>, Value) {
        run_with(t, args, vault, false)
    }

    fn calls_of(t: &[Value], name: &str) -> Vec<Value> {
        t.iter()
            .flat_map(tool_calls)
            .map(stored_call)
            .filter(|(n, _)| n == name)
            .map(|(_, a)| a)
            .collect()
    }

    const VAULT: &str = "- id=vlt_1 · Passport Lucas · id_document · •••• 567X · fields: number, name, expiry, country · only on: apdtrefund.hk\n\
- id=vlt_2 · Visa Lucas · card · •••• 1111 · fields: number, expiry, cvc, holder · only on: apdtrefund.hk";

    fn profile() -> Value {
        json!({"name_en": "LUCAS LU", "birthday": "1985-04-12", "nationality": "Australia", "email_personal": "lucas@example.com"})
    }

    /// What the model reads in the boarding passes and the receipt the machine fetched.
    fn facts() -> Value {
        json!({"in_flight": "SQ 890", "in_date": "2026-10-29", "out_flight": "CX 500", "out_date": "2026-10-30",
               "in_pass_file": SQ_PASS, "out_pass_file": CX_PASS, "receipt_file": RECEIPT, "tax_hkd": 120,
               "refund_method": "card", "card": "Visa"})
    }

    /// The request, the bare call, the documents fetched and read (`mail_read`), then the call again with the facts.
    fn fetched_then_called() -> (Vec<Value>, Value) {
        let (t, args) = started(
            vec![user_msg(REQUEST)],
            json!({"refund_method": "card", "card": "Visa"}),
            true,
        );
        assert_eq!(args["read_mail"], true);
        let (mut t, res) = run(t, &args, VAULT);
        assert_eq!(res["status"], MAIL_READ);
        let paths: Vec<String> = res["files"]
            .as_array()
            .unwrap()
            .iter()
            .map(|f| sget(f, "path"))
            .collect();
        assert_eq!(paths, [SQ_PASS, RECEIPT, CX_PASS]);
        assert!(calls_of(&t, "browser_navigate").is_empty());
        t.extend(workflow::result_exchange(&t, APDT_REFUND, &res));
        started(t, facts(), true)
    }

    #[test]
    fn the_arguments_are_validated_and_what_is_missing_is_asked_before_the_platform() {
        let t = vec![user_msg(REQUEST)];
        // a mailbox and nothing known: the documents are fetched first
        match checked(json!({}), &t, true) {
            Checked::Valid(a) => assert_eq!(a["read_mail"], true),
            other => panic!("{other:?}"),
        }
        // no mailbox, no documents: one needs_input naming everything only the user can give
        let bare = vec![
            json!({"role": "user", "content": "Please apply for my Hong Kong departure tax refund."}),
        ];
        match checked(json!({}), &bare, false) {
            Checked::NeedsInput(m) => {
                let m = m.join(" | ");
                assert!(
                    m.contains("passport_file")
                        && m.contains("into Hong Kong")
                        && m.contains("receipt_file")
                        && m.contains("refund_method"),
                    "{m}"
                );
                assert!(!m.to_lowercase().contains("address"), "{m}");
            }
            other => panic!("{other:?}"),
        }
        // the fields the model can fix
        let (t, _) = fetched_then_called();
        for (k, bad, why) in [
            ("in_flight", json!("890"), "airline code"),
            ("out_date", json!("30/10"), "not a YYYY-MM-DD"),
            (
                "receipt_file",
                json!("attachments/other.pdf"),
                "not a file of this chat",
            ),
            ("refund_method", json!("bitcoin"), "one of card"),
        ] {
            let mut a = facts();
            a[k] = bad;
            match checked(a, &t[..t.len() - 2], true) {
                Checked::Invalid(p) => assert!(p.iter().any(|x| x.contains(why)), "{k}: {p:?}"),
                other => panic!("{k}: {other:?}"),
            }
        }
        // a bare file name is the chat's file; the passport defaults to the user's only picture; flights normalised
        let mut a = facts();
        a["receipt_file"] = json!("ETKT_1602345678901_Receipt.pdf");
        a["in_flight"] = json!("sq890");
        match checked(a, &t[..t.len() - 2], true) {
            Checked::Valid(v) => {
                assert_eq!(
                    (v["receipt_file"].as_str(), v["passport_file"].as_str()),
                    (Some(RECEIPT), Some(PASSPORT))
                );
                assert_eq!(v["in_flight"], "SQ 890");
            }
            other => panic!("{other:?}"),
        }
        // an e-wallet needs its account
        let mut a = facts();
        a["refund_method"] = json!("alipayHK");
        a["card"] = json!("");
        match checked(a, &t[..t.len() - 2], true) {
            Checked::NeedsInput(m) => assert!(m[0].contains("wallet_account"), "{m:?}"),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn the_refund_is_applied_by_code_from_the_mail_to_the_face_step() {
        // apdt_real 2026-11-01: the model stopped on the upload page asking for a residential address the platform
        // never asks for; code runs every step, no question once the platform is open, and stops before the face step
        let (t, args) = fetched_then_called();
        let q = calls_of(&t, "gmail_search");
        assert_eq!(q.len(), 1);
        assert!(sget(&q[0], "query").contains("\"boarding pass\""));
        assert_eq!(calls_of(&t, "gmail_save_attachment").len(), 3);
        assert_eq!(calls_of(&t, "files_read").len(), 3);
        let (t, res) = run(t, &args, VAULT);
        // every upload in its box: the passport the user attached, the two boarding passes, the receipt
        let ups: Vec<(String, String)> = calls_of(&t, "browser_upload")
            .iter()
            .map(|a| (sget(a, "ref"), sget(a, "path")))
            .collect();
        assert_eq!(
            ups,
            vec![
                ("e9".into(), PASSPORT.into()),
                ("e11".into(), SQ_PASS.into()),
                ("e15".into(), CX_PASS.into()),
                ("e20".into(), RECEIPT.into()),
            ]
        );
        let fills = calls_of(&t, "form_fill");
        let stages: Vec<String> = fills.iter().map(|a| sget(a, "stage")).collect();
        assert_eq!(
            stages,
            ["start", "details", "continue", "refund", "next", "confirm"]
        );
        let d = &fills[1];
        assert_eq!(
            (
                sget(d, "in_flight"),
                sget(d, "in_date"),
                sget(d, "out_flight"),
                sget(d, "out_date")
            ),
            (
                "SQ890".into(),
                "2026-10-29".into(),
                "CX500".into(),
                "2026-10-30".into()
            )
        );
        assert_eq!(sget(&fills[3], "method"), "card");
        // the Visa card from the Vault, field by field; no takeover, no face click, no question
        let secrets: Vec<String> = calls_of(&t, "browser_fill_secret")
            .iter()
            .map(|a| format!("{}:{}", sget(a, "field"), sget(a, "ref")))
            .collect();
        assert_eq!(secrets, ["number:e6", "expiry:e8", "holder:e7"]);
        assert!(calls_of(&t, "browser_request_takeover").is_empty());
        assert!(calls_of(&t, "browser_click").is_empty());
        assert!(calls_of(&t, "ask_user").is_empty());
        // the result: the reference from the page, the application's link, the card by its last digits, the deadline
        assert_eq!(res["status"], "confirmed");
        assert_eq!(
            (res["reference"].as_str(), res["link"].as_str()),
            (Some("K7P2QD"), Some(SUBMIT_URL))
        );
        assert_eq!(res["face_deadline"], "2026-11-27");
        assert_eq!(res["trip"]["departure"]["flight"], "CX 500");
        assert_eq!(res["trip"]["tax_hkd"], 120);
        let must = workflow::must_say(&res).join(" | ");
        assert!(
            must.contains("reference number K7P2QD") && must.contains(SUBMIT_URL),
            "{must}"
        );
        assert!(
            must.contains("Visa Lucas card ending 1111") && must.contains("by 27 November 2026"),
            "{must}"
        );
        assert!(!res.to_string().contains("E1234567X"));
    }

    #[test]
    fn older_pages_cut_by_the_compression_do_not_stop_the_run() {
        // apdt_real 2026-11-01: after the card number went in, the saved-reference snapshot (compressed by then) was
        // read again and the run ended "stopped on the upload page"
        let (t, args) = fetched_then_called();
        let (t, res) = run_with(t, &args, VAULT, true);
        assert_eq!(calls_of(&t, "browser_fill_secret").len(), 3, "{res}");
        assert_eq!(calls_of(&t, "browser_upload").len(), 4, "{res}");
        assert_eq!(res["reference"], "K7P2QD");
        assert!(workflow::must_say(&res)
            .join(" ")
            .contains("Visa Lucas card ending 1111"));
    }

    #[test]
    fn a_card_by_name_only_is_typed_by_the_user_and_no_question_after_the_platform_opens() {
        let (t, args) = fetched_then_called();
        let vault = "- id=vlt_2 · Visa Lucas · card · UNKNOWN · fields: number, expiry · only on: apdtrefund.hk";
        let (t, res) = run(t, &args, vault);
        assert!(calls_of(&t, "browser_fill_secret").is_empty());
        let tk = calls_of(&t, "browser_request_takeover");
        assert_eq!(tk.len(), 1);
        assert!(sget(&tk[0], "reason").contains("Card Number"));
        assert!(questions_after_navigate(&t).is_empty());
        assert_eq!(res["reference"], "K7P2QD");
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
        let (mut t, _) = fetched_then_called();
        t.truncate(t.len() - 2);
        let mut a = facts();
        a["out_date"] = json!("2026-10-31");
        let (t, args) = started(t, a, true);
        let (t2, res) = run(t, &args, VAULT);
        assert_eq!(
            (res["status"].as_str(), res["reason"].as_str()),
            (Some("not_eligible"), Some("stayed_longer"))
        );
        assert!(workflow::must_say(&res)
            .join(" ")
            .contains("nothing was submitted"));
        assert!(calls_of(&t2, "browser_navigate").is_empty());
    }

    #[test]
    fn the_upload_boxes_the_errors_and_the_reference_are_read_from_the_page() {
        let b = upload_boxes(UPLOAD_SNAP);
        assert_eq!(
            b,
            vec![
                ("doc", "e9".to_string()),
                ("inboard", "e11".to_string()),
                ("outboard", "e15".to_string()),
                ("paydoc", "e20".to_string())
            ]
        );
        let errs = site_errors("  Flight Departure Date *\n  Departure date has exceeded the application deadline (28 calendar days from the departure date).\n  Please check the items marked above.\n[e3] button \"Next Step\"");
        assert_eq!(errs.len(), 2);
        assert!(errs[0].starts_with("Departure date has exceeded"));
        assert_eq!(reference_on(SUBMIT_URL), Some("K7P2QD".into()));
        // the page's text is never searched for a reference: no bizId in the address, no reference
        assert_eq!(reference_on("https://app.apdtrefund.hk/x?lang=en"), None);
        assert_eq!(
            reference_on("https://app.apdtrefund.hk/x?bizId=K7%20P2"),
            None
        );
        assert_eq!(
            passport_default(&[PASSPORT.into(), "uploads/x.pdf".into()]),
            Some(PASSPORT.into())
        );
        assert_eq!(
            passport_default(&["uploads/a.jpg".into(), "uploads/b.png".into()]),
            None
        );
        assert_eq!(
            vault_card(VAULT, "Visa").map(|c| c.last4),
            Some("1111".into())
        );
    }

    #[test]
    fn a_site_error_ends_the_run_with_the_sites_words() {
        let (mut t, args) = fetched_then_called();
        let from = t.len() - 2;
        let p = profile();
        let err_page = page(
            "https://app.apdtrefund.hk/hkTaxMobile/apdt_upload.html?lang=en",
            "  Departure date has exceeded the application deadline (28 calendar days from the departure date).\n  Please check the items marked above.",
        );
        let res = loop {
            let m = machine(&p, &t, from);
            match next(APDT_REFUND, &args, &t[from..], &m) {
                Step::Calls(c) => {
                    for call in &c {
                        let details_done = results_of(&t, "form_fill")
                            .last()
                            .map(|(a, _)| sget(a, "stage"))
                            == Some("details".into());
                        let r = if call.name == "browser_snapshot" && details_done {
                            err_page.clone()
                        } else {
                            respond(&t, &call.name, &call.args, VAULT)
                        };
                        t.extend(step(&call.id, &call.name, &call.args, &r));
                    }
                }
                Step::Done(v) => break v,
            }
        };
        assert_eq!(
            (res["status"].as_str(), res["expected"].as_str()),
            (Some("stopped"), Some("reference"))
        );
        assert!(
            workflow::must_say(&res)
                .join(" ")
                .contains("Departure date has exceeded"),
            "{res}"
        );
        assert!(questions_after_navigate(&t).is_empty());
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
