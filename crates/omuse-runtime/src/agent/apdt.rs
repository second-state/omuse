//! The code driver of the Hong Kong air passenger departure tax refund (`hk-apdt-refund`): once the skill is loaded
//! for a request to apply, code decides every step — the fixed mail search, the trip documents saved from the mail
//! (`gmail_save_attachment`) and read (`files_read`), the passport photo the user attached, the eligibility rule (left
//! Hong Kong the same or the next calendar day, within 28 days of the departure flight, the tax on the receipt), the
//! one question before the form opens (whatever is missing: a document, the photo, the refund method), the platform's
//! pages filled by their form functions (`crate::forms`, fixed ids), the uploads into their boxes, the card from the
//! Vault (`browser_fill_secret`; a card the Vault cannot fill is typed by the user in a takeover), the reference read
//! from the page and the answer. The face recognition is the user's own step on their phone: never opened, never
//! handed over (`guards::apdt_face_guard`). As in [`super::driver`], the next step is a function of the transcript
//! alone (`next`), so a run suspended for a takeover carries on where it was. No question is asked once the platform
//! is open: every input is settled before.

use std::sync::LazyLock;

use chrono::{Duration, NaiveDate};
use omuse_common::util::get_str;
use regex::Regex;
use serde_json::{json, Value};

use super::claims::{pick_card, vault_cards, VaultCard};
use super::driver::{
    asked_before, asks_to_act, dates_in, flight_no_in, handed_back, json_of, pos, results_of,
    snapshot_text, user_words, walk_messages, Step, World,
};
use super::util::{content_of, sget, stored_call, tool_calls};
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
    /// whether the platform asks for a residential address (collected before the form opens: the profile's home
    /// address, else the one question) — the platform's pages as mapped have no such field
    pub needs_address: bool,
}

pub const APDT: ApdtSpec = ApdtSpec {
    skill: "hk-apdt-refund",
    name: "Hong Kong departure tax refund",
    start_url: "https://app.apdtrefund.hk/hkTaxMobile/?lang=en",
    host: "app.apdtrefund.hk",
    search: "(\"boarding pass\" OR e-ticket OR receipt OR itinerary) (\"Hong Kong\" OR HKG) newer_than:40d",
    window_days: 28,
    needs_address: false,
};

/// The application for this run: the skill loaded and the user's own request asks for it to be made (`asks_to_act`) —
/// a question about the refund leaves the run to the model.
pub fn active(transcript: &[Value]) -> Option<&'static ApdtSpec> {
    let loaded = transcript.iter().flat_map(tool_calls).any(|tc| {
        let (name, args) = stored_call(tc);
        name == "load_skill"
            && sget(&args, "file").trim().is_empty()
            && sget(&args, "name") == APDT.skill
    });
    (loaded && asks_to_act(transcript)).then_some(&APDT)
}

const PREFIX: &str = "apd_";

fn call(n: usize, name: &str, args: Value) -> ToolCall {
    ToolCall {
        id: format!("{PREFIX}{n}"),
        name: name.into(),
        args,
    }
}

fn step_count(transcript: &[Value]) -> usize {
    transcript
        .iter()
        .flat_map(tool_calls)
        .filter(|tc| get_str(tc, "id").starts_with(PREFIX))
        .count()
        + 1
}

fn failed(m: &Value) -> bool {
    let c = content_of(m);
    let head = c.trim_start();
    head.starts_with("ERROR") || head.starts_with("DENIED")
}

// ---------------------------------------------------------------- the documents

/// What a trip document is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DocKind {
    /// the boarding pass of the flight into Hong Kong
    Inbound,
    /// the boarding pass of the flight out of Hong Kong
    Outbound,
    /// the ticket receipt / itinerary (the proof of the tax)
    Receipt,
    Other,
}

/// One document read: its workspace path, what it is, the flight and date it shows, and the departure tax (HK$) a
/// receipt shows.
#[derive(Debug, Clone, PartialEq)]
pub struct Doc {
    pub path: String,
    pub kind: DocKind,
    pub flight: String,
    pub date: Option<NaiveDate>,
    pub tax: Option<u32>,
}

static HK: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)hong\s?kong|\bHKG\b").unwrap());
static TO_LINE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?im)^\s*to\s*:\s*(.+)$").unwrap());
static FROM_LINE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?im)^\s*from\s*:\s*(.+)$").unwrap());
static TO_HK: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)\bto\s+(hong\s?kong|\(?HKG\b)").unwrap());
static FROM_HK: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\bfrom\s+(hong\s?kong|\(?HKG\b)|hong\s?kong\s*(\(HKG\))?\s*(\d{1,2}:\d{2}\s*)?(to|→|-)\s+[A-Z]").unwrap()
});
static BOARDING: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)boarding\s*pass|boardingpass").unwrap());
static RECEIPT: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)receipt|itinerary|e-?ticket|\bETKT").unwrap());
static TAX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)(?:HKD|HK\$)\s*(120|200)(?:\.00)?[^\n]{0,40}departure\s+tax|departure\s+tax[^\n]{0,40}?(?:HKD|HK\$)\s*(120|200)\b")
        .unwrap()
});
static TAX_WORDS: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)passenger\s+departure\s+tax|\bAPDT\b").unwrap());

/// The document `text` describes (its own text first, then the mail it came with): a boarding pass into or out of
/// Hong Kong by its From / To lines (else the mail's "to Hong Kong" / "Hong Kong to …"), or a receipt with the tax.
pub fn classify(path: &str, text: &str, mail: &str) -> Doc {
    let name = path.rsplit('/').next().unwrap_or(path);
    let both = format!("{text}\n{mail}");
    let boarding = BOARDING.is_match(text) || BOARDING.is_match(name);
    let receipt = !boarding && (RECEIPT.is_match(text) || RECEIPT.is_match(name));
    let line_hk = |r: &Regex| r.captures(text).is_some_and(|c| HK.is_match(&c[1]));
    let kind = if boarding {
        if line_hk(&TO_LINE) {
            DocKind::Inbound
        } else if line_hk(&FROM_LINE) {
            DocKind::Outbound
        } else if TO_HK.is_match(&both) && !FROM_HK.is_match(&both) {
            DocKind::Inbound
        } else if FROM_HK.is_match(&both) && !TO_HK.is_match(&both) {
            DocKind::Outbound
        } else {
            DocKind::Other
        }
    } else if receipt {
        DocKind::Receipt
    } else {
        DocKind::Other
    };
    let flight = flight_no_in(text)
        .or_else(|| flight_no_in(mail))
        .unwrap_or_default();
    let date = dates_in(text)
        .first()
        .map(|(_, d)| *d)
        .or_else(|| dates_in(mail).first().map(|(_, d)| *d));
    let tax = TAX
        .captures(text)
        .and_then(|c| c.get(1).or(c.get(2)))
        .and_then(|m| m.as_str().parse().ok())
        .or_else(|| TAX_WORDS.is_match(text).then_some(0));
    Doc {
        path: path.to_string(),
        kind,
        flight,
        date,
        tax,
    }
}

/// The files the user attached in this chat (`uploads/…` paths in their messages).
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

/// The photo / scan of the passport's photo page the user attached: one named for it, else their only picture.
pub fn passport_file(files: &[String]) -> Option<String> {
    static PASSPORT: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"(?i)passport|photo.?page|护照|護照").unwrap());
    let named = files
        .iter()
        .find(|f| is_doc_file(f) && PASSPORT.is_match(f.rsplit('/').next().unwrap_or(f)));
    let images: Vec<&String> = files
        .iter()
        .filter(|f| is_image(f) && !BOARDING.is_match(f) && !RECEIPT.is_match(f))
        .collect();
    named
        .or(if images.len() == 1 {
            Some(images[0])
        } else {
            None
        })
        .cloned()
}

/// The attachments of the fetched mail worth saving (a boarding pass, a receipt): (message id, file name, the mail's
/// subject and body).
fn mail_attachments(transcript: &[Value]) -> Vec<(String, String, String)> {
    let mut out = Vec::new();
    for (a, m) in results_of(transcript, "gmail_get_message") {
        let v = json_of(m);
        let mail = format!("{}\n{}", get_str(&v, "subject"), get_str(&v, "body"));
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
            let doc = BOARDING.is_match(&name)
                || RECEIPT.is_match(&name)
                || BOARDING.is_match(&mail)
                || RECEIPT.is_match(&mail);
            if is_doc_file(&name) && doc && !out.iter().any(|(i, n, _)| *i == id && *n == name) {
                out.push((id.clone(), name, mail.clone()));
            }
        }
    }
    out
}

/// The saved attachments: (workspace path, the mail it came with).
fn saved(transcript: &[Value]) -> Vec<(String, String)> {
    static SAVED: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"saved: (\S+)").unwrap());
    let atts = mail_attachments(transcript);
    results_of(transcript, "gmail_save_attachment")
        .into_iter()
        .filter(|(_, m)| !failed(m))
        .filter_map(|(a, m)| {
            let path = SAVED.captures(&content_of(m))?[1].to_string();
            let mail = atts
                .iter()
                .find(|(i, n, _)| *i == sget(&a, "message_id") && *n == sget(&a, "filename"))
                .map(|(_, _, t)| t.clone())
                .unwrap_or_default();
            Some((path, mail))
        })
        .collect()
}

/// The documents read so far (`files_read` of each saved attachment and each PDF the user attached).
fn docs(transcript: &[Value]) -> Vec<Doc> {
    let mails = saved(transcript);
    results_of(transcript, "files_read")
        .into_iter()
        .filter(|(_, m)| !failed(m))
        .map(|(a, m)| {
            let path = sget(&a, "path");
            let mail = mails
                .iter()
                .find(|(p, _)| *p == path)
                .map(|(_, t)| t.clone())
                .unwrap_or_default();
            classify(&path, &content_of(m), &mail)
        })
        .collect()
}

// ---------------------------------------------------------------- the refund method

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Method {
    Card,
    Cash,
    /// the platform's radio value (alipay, wechat, alipayHK, wechatHK) and the account the user gave
    Wallet(&'static str, String),
}

/// The refund method the user's own words ask for; `None` when they name none.
pub fn method_in(texts: &[String]) -> Option<Method> {
    static CARD: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?i)\b(visa|master\s?card|amex|american express|credit|debit|card)\b|信用卡|银行卡|借记卡|扣账卡").unwrap()
    });
    static BOARDING_CARD: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"(?i)boarding\s+cards?").unwrap());
    static CASH: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)\bcash\b|现金").unwrap());
    static ACCOUNT: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?i)(?:account|globalpass|\bid)\s*(?:no\.?|number|id)?\s*(?:is|:|：)?\s*([A-Za-z0-9@._+-]{5,})")
            .unwrap()
    });
    const WALLETS: &[(&str, &str)] = &[
        (r"(?i)alipay\s?hk|支付宝香港", "alipayHK"),
        (r"(?i)wechat\s?pay\s?hk|微信支付香港", "wechatHK"),
        (r"(?i)alipay|支付宝", "alipay"),
        (r"(?i)wechat|微信", "wechat"),
    ];
    let words = BOARDING_CARD
        .replace_all(&texts.join("\n"), " ")
        .into_owned();
    if let Some((_, v)) = WALLETS
        .iter()
        .find(|(r, _)| Regex::new(r).is_ok_and(|r| r.is_match(&words)))
    {
        let account = ACCOUNT
            .captures(&words)
            .map(|c| c[1].to_string())
            .unwrap_or_default();
        return Some(Method::Wallet(v, account));
    }
    if CARD.is_match(&words) {
        return Some(Method::Card);
    }
    CASH.is_match(&words).then_some(Method::Cash)
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

/// The reference the platform issued, read from the page: the `bizId` its address carries, else the 6 letters and
/// digits it shows.
pub fn reference_on(url: &str, snapshot: &str) -> Option<String> {
    static BIZ: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"[?&]bizId=([A-Za-z0-9]{4,12})").unwrap());
    static TOKEN: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\b[A-Z0-9]{6}\b").unwrap());
    if let Some(c) = BIZ.captures(url) {
        return Some(c[1].to_string());
    }
    TOKEN
        .find_iter(snapshot)
        .map(|m| m.as_str())
        .find(|t| {
            t.chars().any(|c| c.is_ascii_digit()) && t.chars().any(|c| c.is_ascii_uppercase())
        })
        .map(str::to_string)
}

// ---------------------------------------------------------------- the step machine

const ASK_MARK: &str = "To apply for your Hong Kong departure tax refund I still need";
const ASK_MARK_ZH: &str = "申请香港离境税退款还需要";
const OWN_STEP: &str = "This step is yours";

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

fn long(d: NaiveDate) -> String {
    d.format("%-d %B %Y").to_string()
}

/// Everything the application needs, settled before the platform opens.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Facts {
    pub inbound: Option<Doc>,
    pub outbound: Option<Doc>,
    pub receipt: Option<Doc>,
    pub passport: Option<String>,
    pub method: Option<Method>,
    pub address: String,
}

impl Facts {
    /// What is still missing, as the one question names it.
    fn missing(&self, needs_address: bool) -> Vec<&'static str> {
        let mut out = Vec::new();
        if self.passport.is_none() {
            out.push("the photo page of your passport (attach a photo or scan of it here)");
        }
        if self.inbound.is_none() {
            out.push("the boarding pass of your flight into Hong Kong");
        }
        if self.outbound.is_none() {
            out.push("the boarding pass of your flight out of Hong Kong");
        }
        if self.receipt.is_none() {
            out.push(
                "the e-ticket receipt or itinerary that shows the Hong Kong departure tax you paid",
            );
        }
        match &self.method {
            None => out.push("how you want the refund: to a credit / debit card (Visa, Mastercard, AmEx), Alipay / WeChat Pay (with the account), or cash at the airport"),
            Some(Method::Wallet(_, a)) if a.is_empty() => out.push("the e-wallet account the refund goes to"),
            _ => {}
        }
        if needs_address && self.address.is_empty() {
            out.push("your residential address");
        }
        out
    }

    /// The `form_fill` facts — and, for the run's later turns, everything settled before the form opened (the
    /// documents' paths and facts, the card): a call's arguments survive the transcript's compression, the mail and
    /// the documents read do not.
    fn args(&self, card: Option<&Card>, holder: &str) -> Value {
        let flight = |d: &Option<Doc>| {
            d.as_ref()
                .map(|d| d.flight.replace(' ', ""))
                .unwrap_or_default()
        };
        let date = |d: &Option<Doc>| {
            d.as_ref()
                .and_then(|d| d.date)
                .map(|d| d.format("%Y-%m-%d").to_string())
                .unwrap_or_default()
        };
        let (method, account) = match &self.method {
            Some(Method::Card) => ("card", String::new()),
            Some(Method::Cash) => ("cash", String::new()),
            Some(Method::Wallet(w, a)) => (*w, a.clone()),
            None => ("", String::new()),
        };
        let path = |d: &Option<Doc>| d.as_ref().map(|d| d.path.clone()).unwrap_or_default();
        let no = |d: &Option<Doc>| d.as_ref().map(|d| d.flight.clone()).unwrap_or_default();
        json!({"in_flight": flight(&self.inbound), "in_date": date(&self.inbound),
               "out_flight": flight(&self.outbound), "out_date": date(&self.outbound),
               "method": method, "account": account, "holder": holder,
               "card": if card.is_some_and(|c| !c.name_only) { "vault" } else { "type" },
               "address": self.address,
               "kept": {"passport": self.passport.clone().unwrap_or_default(),
                        "in_path": path(&self.inbound), "in_no": no(&self.inbound),
                        "out_path": path(&self.outbound), "out_no": no(&self.outbound),
                        "receipt_path": path(&self.receipt),
                        "tax": self.receipt.as_ref().and_then(|r| r.tax),
                        "card": card.map(|c| json!({"id": c.id, "label": c.label, "name_only": c.name_only,
                                                     "last4": c.last4, "holder": c.holder}))}})
    }

    /// The facts and the card as the start page's fill kept them (`args`), the method and address as now.
    fn restored(&self, a: &Value) -> (Facts, Option<Card>) {
        let k = &a["kept"];
        let doc = |p: &str, no: &str, d: &str, kind: DocKind| {
            let path = get_str(k, p);
            (!path.is_empty()).then(|| Doc {
                path,
                kind,
                flight: get_str(k, no),
                date: NaiveDate::parse_from_str(&get_str(a, d), "%Y-%m-%d").ok(),
                tax: None,
            })
        };
        let mut receipt = doc("receipt_path", "", "", DocKind::Receipt);
        if let Some(r) = receipt.as_mut() {
            r.tax = k["tax"].as_u64().map(|t| t as u32);
        }
        let passport = Some(get_str(k, "passport")).filter(|p| !p.is_empty());
        let c = &k["card"];
        let card = c.is_object().then(|| Card {
            id: get_str(c, "id"),
            label: get_str(c, "label"),
            name_only: c["name_only"].as_bool().unwrap_or(false),
            last4: get_str(c, "last4"),
            holder: c["holder"].as_bool().unwrap_or(false),
        });
        (
            Facts {
                inbound: doc("in_path", "in_no", "in_date", DocKind::Inbound),
                outbound: doc("out_path", "out_no", "out_date", DocKind::Outbound),
                receipt,
                passport,
                method: self.method.clone(),
                address: self.address.clone(),
            },
            card,
        )
    }
}

/// The refund card from the Vault: the one the user named, else the first with its details (`claims::pick_card`).
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

fn vault_card(transcript: &[Value], user_texts: &[String]) -> Option<Card> {
    static L4: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"•+\s*(\d{4})").unwrap());
    let (_, m) = results_of(transcript, "vault_list").pop()?;
    let text = content_of(m);
    let cards: Vec<VaultCard> = vault_cards(&text);
    let c = pick_card(&cards, user_texts)?;
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

/// The facts read so far.
pub fn facts(transcript: &[Value], w: &World) -> Facts {
    let ds = docs(transcript);
    let pick = |k: DocKind| ds.iter().find(|d| d.kind == k).cloned();
    let files = user_files(transcript);
    Facts {
        inbound: pick(DocKind::Inbound),
        outbound: pick(DocKind::Outbound),
        // a receipt that shows the tax first
        receipt: ds
            .iter()
            .find(|d| d.kind == DocKind::Receipt && d.tax.is_some())
            .or_else(|| ds.iter().find(|d| d.kind == DocKind::Receipt))
            .cloned(),
        passport: passport_file(&files),
        method: method_in(&user_words(transcript)),
        address: get_str(w.profile, "address_home").trim().to_string(),
    }
}

/// Why the trip does not qualify (the rule, in the answer), or `None` when it does.
pub fn ineligible(f: &Facts, today: NaiveDate, window: i64) -> Option<String> {
    let (Some(i), Some(o)) = (&f.inbound, &f.outbound) else {
        return None;
    };
    let (Some(a), Some(d)) = (i.date, o.date) else {
        return None;
    };
    let gap = (d - a).num_days();
    if !(0..=1).contains(&gap) {
        return Some(format!(
            "you arrived on {} ({}) and left on {} ({}): the refund is only for passengers who leave Hong Kong by air on the same or the next calendar day after arriving",
            long(a), i.flight, long(d), o.flight
        ));
    }
    if d > today {
        return Some(format!(
            "your departure flight {} is on {}: the platform takes the application only after you have left (it checks the departure record)",
            o.flight,
            long(d)
        ));
    }
    if (today - d).num_days() > window {
        return Some(format!(
            "your departure flight {} was on {}: applications close {} calendar days after the departure flight (that was {})",
            o.flight,
            long(d),
            window,
            long(d + Duration::days(window))
        ));
    }
    if f.receipt.as_ref().is_some_and(|r| r.tax.is_none()) {
        return Some(
            "the e-ticket receipt does not show the Hong Kong Air Passenger Departure Tax (HK$120 / HK$200) as paid: there is nothing to refund (a transfer on one through ticket is usually not taxed)"
                .into(),
        );
    }
    None
}

/// The next step of the run.
pub fn next(s: &ApdtSpec, transcript: &[Value], w: &World) -> Step {
    let n = step_count(transcript);
    let opened = results_of(transcript, "browser_navigate")
        .iter()
        .any(|(a, _)| sget(a, "url").contains(s.host));
    // 1. the mail: the fixed search (this run's own), the hits, the attachments saved, every document read
    if !opened {
        let searched = results_of(transcript, "gmail_search")
            .iter()
            .any(|(_, m)| get_str(m, "tool_call_id").starts_with(PREFIX));
        if w.mail_ready && !searched {
            return Step::Calls(vec![call(
                n,
                "gmail_search",
                json!({"query": s.search, "max_results": 10}),
            )]);
        }
        let have: Vec<String> = results_of(transcript, "gmail_get_message")
            .iter()
            .map(|(a, _)| sget(a, "message_id"))
            .collect();
        let mut want: Vec<String> = Vec::new();
        for (_, m) in results_of(transcript, "gmail_search")
            .iter()
            .filter(|(_, m)| get_str(m, "tool_call_id").starts_with(PREFIX))
        {
            let mut hits = Vec::new();
            walk_messages(&json_of(m), &mut hits);
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
        let tried: Vec<(String, String)> = results_of(transcript, "gmail_save_attachment")
            .iter()
            .map(|(a, _)| (sget(a, "message_id"), sget(a, "filename")))
            .collect();
        let to_save: Vec<ToolCall> = mail_attachments(transcript)
            .into_iter()
            .filter(|(i, f, _)| !tried.contains(&(i.clone(), f.clone())))
            .enumerate()
            .map(|(k, (i, f, _))| {
                call(
                    n + k,
                    "gmail_save_attachment",
                    json!({"message_id": i, "filename": f}),
                )
            })
            .collect();
        if !to_save.is_empty() {
            return Step::Calls(to_save);
        }
        let read: Vec<String> = results_of(transcript, "files_read")
            .iter()
            .map(|(a, _)| sget(a, "path"))
            .collect();
        let passport = passport_file(&user_files(transcript));
        let to_read: Vec<ToolCall> = saved(transcript)
            .into_iter()
            .map(|(p, _)| p)
            .chain(
                user_files(transcript)
                    .into_iter()
                    .filter(|p| p.to_lowercase().ends_with(".pdf") && Some(p) != passport.as_ref()),
            )
            .filter(|p| p.to_lowercase().ends_with(".pdf") && !read.contains(p))
            .enumerate()
            .map(|(k, p)| call(n + k, "files_read", json!({"path": p, "max_chars": 4000})))
            .collect();
        if !to_read.is_empty() {
            return Step::Calls(to_read);
        }
    }
    let f = facts(transcript, w);
    // 2. the rule, then the one question before the form (whatever is missing), then the card from the Vault
    if !opened {
        if let Some(why) = ineligible(&f, w.today, s.window_days) {
            return Step::Final(if w.zh {
                format!("这次不能申请香港机场离境税退款：{why}。什么都没有提交。")
            } else {
                format!("You cannot claim the Hong Kong departure tax refund for this trip: {why}. Nothing was submitted.")
            });
        }
        let missing = f.missing(s.needs_address);
        if !missing.is_empty() {
            if asked_before(transcript, ASK_MARK) || asked_before(transcript, ASK_MARK_ZH) {
                return Step::Final(if w.zh {
                    format!(
                        "没有这些就无法申请香港离境税退款：{}。补上后再让我申请；什么都没有提交。",
                        missing.join("；")
                    )
                } else {
                    format!(
                        "Without {} the Hong Kong departure tax refund cannot be applied for: send them and ask me again. Nothing was submitted.",
                        missing.join("; ")
                    )
                });
            }
            let mut q = if w.zh {
                format!(
                    "{ASK_MARK_ZH}：\n{}",
                    missing
                        .iter()
                        .map(|m| format!("- {m}"))
                        .collect::<Vec<_>>()
                        .join("\n")
                )
            } else {
                format!(
                    "{ASK_MARK}:\n{}\nEverything else is filled in from your mail, profile and Vault.",
                    missing.iter().map(|m| format!("- {m}")).collect::<Vec<_>>().join("\n")
                )
            };
            if !w.mail_ready {
                q.push_str("\n\n(Connect your mailbox in Connections and I can take the boarding passes and the receipt from your airline emails myself.)");
            }
            return Step::Final(q);
        }
        if f.method == Some(Method::Card) && results_of(transcript, "vault_list").is_empty() {
            return Step::Calls(vec![call(n, "vault_list", json!({}))]);
        }
        return Step::Calls(vec![call(
            n,
            "browser_navigate",
            json!({"url": s.start_url}),
        )]);
    }
    // once the start page was filled, the facts are the ones its fill kept (the mail and the documents read may have
    // been compressed since); before, the ones just read
    let (f, card) = match results_of(transcript, "form_fill")
        .into_iter()
        .find(|(a, _)| sget(a, "stage") == "start")
    {
        Some((a, _)) => f.restored(&a),
        None => {
            let card = (f.method == Some(Method::Card))
                .then(|| vault_card(transcript, w.user_texts))
                .flatten();
            (f, card)
        }
    };
    let card = card.as_ref();
    let holder = get_str(w.profile, "name_en").trim().to_uppercase();
    let args = f.args(card, &holder);
    let stuck = |url: &str, text: &str, wanted: &str| -> Step {
        let errs = site_errors(text);
        Step::Final(if w.zh {
            format!(
                "退税申请停在了平台的{}页（预期是{}）{}。请打开浏览器标签页查看；人脸识别没有开始。",
                page_of(url),
                wanted,
                if errs.is_empty() { String::new() } else { format!("：{}", errs.join(" ")) }
            )
        } else {
            format!(
                "The refund application stopped on the platform's {} page instead of the {} page{}. Open the browser tab to see it; the face recognition was not started.",
                if page_of(url).is_empty() { "current" } else { page_of(url) },
                wanted,
                if errs.is_empty() { String::new() } else { format!(" — the site says: {}", errs.join(" ")) }
            )
        })
    };
    // a page is checked only until the stage after it ran: the transcript's compression may have shortened an older
    // snapshot since (apdt_real 2026-11-01: the saved-reference page read again after the card fill, cut short)
    let ran = |name: &str| {
        results_of(transcript, "form_fill")
            .iter()
            .any(|(a, _)| sget(a, "stage") == name)
    };
    // 3. the notes and "Start New Application" → the upload page
    let start = match stage(transcript, n, "start", 0, args.clone()) {
        Ok(p) => p,
        Err(st) => return st,
    };
    let (url, text, snap_pos) = match snap_after(transcript, n, start) {
        Ok(x) => x,
        Err(st) => return st,
    };
    if !ran("details") && page_of(&url) != "apdt_upload" {
        return stuck(&url, &text, "upload");
    }
    // 4. the uploads, each in its box (refs from the upload page's snapshot)
    let files = [
        ("doc", f.passport.clone()),
        ("inboard", f.inbound.as_ref().map(|d| d.path.clone())),
        ("outboard", f.outbound.as_ref().map(|d| d.path.clone())),
        ("paydoc", f.receipt.as_ref().map(|d| d.path.clone())),
    ];
    // a box's ref from the newest page that still lists it (an upload's result is the page again; the first snapshot
    // may have been compressed by then)
    let box_ref = |what: &str| {
        transcript[snap_pos.saturating_sub(1).min(transcript.len())..]
            .iter()
            .rev()
            .filter(|m| get_str(m, "role") == "tool")
            .find_map(|m| {
                upload_boxes(&snapshot_text(m))
                    .into_iter()
                    .find(|(w, _)| *w == what)
                    .map(|(_, r)| r)
            })
    };
    let uploads = results_of(transcript, "browser_upload");
    let mut uploaded = snap_pos;
    for (what, path) in &files {
        let Some(path) = path else { continue };
        let tries: Vec<&(Value, &Value)> = uploads
            .iter()
            .filter(|(a, m)| sget(a, "path") == *path && pos(transcript, m) > snap_pos)
            .collect();
        if let Some((_, m)) = tries.iter().find(|(_, m)| !failed(m)) {
            uploaded = uploaded.max(pos(transcript, m));
            continue;
        }
        if let Some((_, m)) = tries.first() {
            return Step::Final(format!(
                "The {} could not be uploaded to the platform ({}). Open the browser tab to see it; nothing was submitted.",
                path.rsplit('/').next().unwrap_or(path),
                content_of(m).chars().take(160).collect::<String>()
            ));
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
    let details = match stage(transcript, n, "details", uploaded, args.clone()) {
        Ok(p) => p,
        Err(st) => return st,
    };
    let (url, text, details_snap) = match snap_after(transcript, n, details) {
        Ok(x) => x,
        Err(st) => return st,
    };
    let low = text.to_lowercase();
    if !ran("continue")
        && !(low.contains("uploaded successfully") || low.contains("saved automatically"))
    {
        return stuck(&url, &text, "reference");
    }
    let cont = match stage(transcript, n, "continue", details_snap, json!({})) {
        Ok(p) => p,
        Err(st) => return st,
    };
    let (url, text, refund_snap) = match snap_after(transcript, n, cont) {
        Ok(x) => x,
        Err(st) => return st,
    };
    if !ran("refund") && page_of(&url) != "apdt_refund" {
        return stuck(&url, &text, "refund method");
    }
    // 6. the refund method; the card from the Vault (or typed by the user), then "Next"
    let refund = match stage(transcript, n, "refund", refund_snap, args.clone()) {
        Ok(p) => p,
        Err(st) => return st,
    };
    let mut before_next = refund;
    if f.method == Some(Method::Card) {
        let (_, text, card_snap) = match snap_after(transcript, n, refund) {
            Ok(x) => x,
            Err(st) => return st,
        };
        let fills = results_of(transcript, "browser_fill_secret");
        let done = |field: &str| {
            fills
                .iter()
                .find(|(a, m)| sget(a, "field") == field && pos(transcript, m) > card_snap)
                .map(|(_, m)| !failed(m))
        };
        let vault_ok = card.is_some_and(|c| !c.name_only) && done("number") != Some(false);
        if let (true, Some(c)) = (vault_ok, card) {
            let has_holder = c.holder;
            let wanted: &[(&str, &str)] = if has_holder {
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
                if let Some(r) =
                    textbox(&text, label).or_else(|| newest_ref(transcript, card_snap, label))
                {
                    return Step::Calls(vec![call(
                        n,
                        "browser_fill_secret",
                        json!({"ref": r, "item_id": c.id, "field": field}),
                    )]);
                }
            }
            before_next = fills
                .iter()
                .map(|(_, m)| pos(transcript, m))
                .filter(|p| *p > card_snap)
                .max()
                .unwrap_or(card_snap);
        } else {
            // no complete card in the Vault: the user types it, in a takeover (the only one of the run)
            let takeovers = results_of(transcript, "browser_request_takeover");
            match takeovers
                .iter()
                .rfind(|(_, m)| pos(transcript, m) > card_snap)
            {
                None => {
                    let what = match card {
                        Some(c) => format!("your {} is in the Vault by its name only", c.label),
                        None => "there is no card in your Vault".to_string(),
                    };
                    return Step::Calls(vec![call(
                        n,
                        "browser_request_takeover",
                        json!({"reason": format!("{OWN_STEP}: {what}. Type your card number for the refund into Card Number, then the expiry (MM/YY) into Validity Period; hand the browser back when done and I will finish the application.")}),
                    )]);
                }
                Some((_, m)) if handed_back(m) => before_next = pos(transcript, m),
                Some(_) => return Step::Final(String::new()),
            }
        }
    }
    let next_pos = match stage(transcript, n, "next", before_next, json!({})) {
        Ok(p) => p,
        Err(st) => return st,
    };
    let (url, text, confirm_snap) = match snap_after(transcript, n, next_pos) {
        Ok(x) => x,
        Err(st) => return st,
    };
    if !ran("confirm") && page_of(&url) != "apdt_confirm" {
        return stuck(&url, &text, "confirmation");
    }
    // 7. "Please confirm your application information": the declaration and "Confirm and Next Step"
    let confirm = match stage(transcript, n, "confirm", confirm_snap, json!({})) {
        Ok(p) => p,
        Err(st) => return st,
    };
    let (url, text, _) = match snap_after(transcript, n, confirm) {
        Ok(x) => x,
        Err(st) => return st,
    };
    // 8. "Submit your application — Proceed with Facial Recognition": stop; the reference from the page
    if page_of(&url) != "apdt_submit" {
        return stuck(&url, &text, "face recognition");
    }
    let reference = reference_on(&url, &text);
    Step::Final(answer(
        &f,
        reference.as_deref(),
        &url,
        card.map(|c| c.label.as_str()),
        card.map(|c| c.last4.as_str()).filter(|l| !l.is_empty()),
        w.zh,
    ))
}

/// The answer once the application is confirmed: the reference, what was filled and uploaded, the refund, and the
/// face recognition with the application's own link and the last day for it.
pub fn answer(
    f: &Facts,
    reference: Option<&str>,
    url: &str,
    card: Option<&str>,
    last4: Option<&str>,
    zh: bool,
) -> String {
    const HOME: &str = "https://app.apdtrefund.hk/hkTaxMobile/?lang=en";
    let leg = |d: &Option<Doc>| {
        d.as_ref()
            .map(|d| format!("{} on {}", d.flight, d.date.map(long).unwrap_or_default()))
            .unwrap_or_default()
    };
    let out_date = f.outbound.as_ref().and_then(|d| d.date);
    let deadline = out_date.map(|d| long(d + Duration::days(APDT.window_days)));
    let tax = match f.receipt.as_ref().and_then(|r| r.tax) {
        Some(t) if t > 0 => format!("the HK${t} departure tax"),
        _ => "the departure tax".to_string(),
    };
    let refund = match (&f.method, card, last4) {
        (Some(Method::Card), Some(c), Some(l)) => format!("to your {c} card ending {l} (from the Vault); a processing fee of up to HK$18 is deducted, and the money arrives about 3 working days after approval"),
        (Some(Method::Card), _, _) => "to your card; a processing fee of up to HK$18 is deducted, and the money arrives about 3 working days after approval".to_string(),
        (Some(Method::Cash), _, _) => "in cash (HKD), collected at the airport's designated counter after approval".to_string(),
        (Some(Method::Wallet(w, _)), _, _) => format!("to your {w} account; the e-wallet's processing fee is deducted, usually about one day after approval"),
        (None, _, _) => String::new(),
    };
    let Some(code) = reference else {
        return if zh {
            format!("退税申请已填好并确认，但页面上没有看到参考编号。请打开浏览器标签页查看，或在 {HOME} 用「Resume Application」找回；人脸识别由你自己在手机上完成。")
        } else {
            format!("The refund application is filled in and confirmed, but no reference number showed on the page. Open the browser tab to check it, or find it with \"Resume Application\" at {HOME}; the face recognition is yours, on your phone.")
        };
    };
    let link = if url.contains("bizId=") { url } else { HOME };
    if zh {
        return format!(
            "你的香港机场离境税退款申请已填好并确认：参考编号 **{code}**。\n\n\
- 行程：{} 抵港，{} 离港（符合同日或次日离境的规定），收据上有{}。\n\
- 已上传：护照照片页、两张登机牌和电子机票收据；姓名、出生日期和国籍来自档案，护照号来自保险箱（这里不显示）。\n\
- 退款：{}。\n\n\
**人脸识别——由你自己完成：** 参考编号 **{code}**。在手机上打开[你的退税申请 {code}]({link})，点「Proceed with Facial Recognition」，用手机前置摄像头完成。如果要求重新登录，在 {HOME} 选「Resume Application」，输入旅行证件号码和参考编号 {code}。{}",
            leg(&f.inbound), leg(&f.outbound), tax, refund,
            deadline.map(|d| format!("请在 **{d}** 之前完成（离港航班后 28 天内）。")).unwrap_or_default()
        );
    }
    let gap = match (f.inbound.as_ref().and_then(|d| d.date), out_date) {
        (Some(a), Some(d)) if a == d => "the same day",
        _ => "the next day",
    };
    format!(
        "Your Hong Kong departure tax refund application is filled in and confirmed on the Airport Authority's platform: reference number **{code}**.\n\n\
- Trip: arrived {}, left {} ({gap}), with {} on the e-ticket receipt — eligible.\n\
- Uploaded: the photo page of your passport, both boarding passes and the e-ticket receipt; name, date of birth and nationality from your profile, the passport number from the Vault (not shown here).\n\
- Refund: {}.\n\n\
**Face recognition — your step:** reference number **{code}**. Open [your refund application {code}]({link}) on your phone and press \"Proceed with Facial Recognition\" (the phone's front camera). If it asks you to sign in again, choose \"Resume Application\" at {HOME} with your travel document number and the reference {code}.{}",
        leg(&f.inbound),
        leg(&f.outbound),
        tax,
        refund,
        deadline
            .map(|d| format!(" Do it by **{d}** (28 days after the departure flight)."))
            .unwrap_or_default()
    )
}

#[cfg(test)]
mod tests {
    use super::super::driver::questions_after_navigate;
    use super::*;

    const BACK: &str = "\"Takeover finished: the user handed browser control back.\"";

    fn today() -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 11, 1).unwrap()
    }

    fn world<'a>(user: &'a [String], profile: &'a Value, mail: bool) -> World<'a> {
        World {
            user_texts: user,
            profile,
            today: today(),
            mail_ready: mail,
            zh: false,
        }
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

    fn pdf_text(path: &str) -> String {
        if path.contains("SQ890") {
            "--- page 1 ---\nSINGAPORE AIRLINES  -  BOARDING PASS\nName: LU/LUCAS MR\nFlight: SQ 890\nFrom: SINGAPORE (SIN) Changi Terminal 3\nTo: HONG KONG (HKG)\nDate: 29 OCT 2026".into()
        } else if path.contains("CX500") {
            "--- page 1 ---\nCATHAY PACIFIC  -  BOARDING PASS\nName: LU/LUCAS MR\nFlight: CX 500\nFrom: HONG KONG (HKG) Terminal 1\nTo: TOKYO NARITA (NRT)\nDate: 30 OCT 2026".into()
        } else {
            "--- page 1 ---\nCATHAY PACIFIC  -  ELECTRONIC TICKET ITINERARY / RECEIPT\nPassenger: LU/LUCAS MR\nCX 500   HONG KONG (HKG)      TOKYO NARITA (NRT)    30 OCT 2026   09:05\nTaxes, fees and charges:\n  HK   HKD 120.00   Hong Kong Air Passenger Departure Tax\n  G3   HKD  90.00   Hong Kong Airport Construction Fee".into()
        }
    }

    /// The world's answer to one of the driver's calls (the stand-in platform's pages, page by page).
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
            "files_read" => format!(
                "<untrusted_content source=\"file {}\">\n{}\n</untrusted_content>",
                sget(args, "path"),
                pdf_text(&sget(args, "path"))
            ),
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

    fn start(user: &str) -> Vec<Value> {
        let mut t = vec![
            json!({"role": "user", "content": format!("{user}\n\n### passport.jpg — image, 60000 bytes, path: uploads/2026-11/passport.jpg")}),
        ];
        t.push(json!({"role": "assistant", "content": "", "tool_calls": [{"id": "k1", "type": "function",
            "function": {"name": "load_skill", "arguments": json!({"name": "hk-apdt-refund", "file": ""}).to_string()}}]}));
        t.push(json!({"role": "tool", "tool_call_id": "k1", "content": "---"}));
        t
    }

    /// Runs `next` to the end against the stand-in platform; panics on a question after the platform opened.
    fn run(t: Vec<Value>, w: &World, vault: &str) -> (Vec<Value>, String) {
        run_with(t, w, vault, false)
    }

    /// `squeeze`: once the platform is open, after each turn every tool result but the last two is cut as the
    /// transcript's compression cuts it (the mail, the documents, the vault list and the older pages).
    fn run_with(mut t: Vec<Value>, w: &World, vault: &str, squeeze: bool) -> (Vec<Value>, String) {
        for i in 0..60 {
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
            match next(&APDT, &t, w) {
                Step::Calls(c) => {
                    for (j, call) in c.iter().enumerate() {
                        let id = format!("{}{i}_{j}", call.id);
                        let result = respond(&t, &call.name, &call.args, vault);
                        t.push(json!({"role": "assistant", "content": "", "tool_calls": [{"id": id, "type": "function",
                            "function": {"name": call.name, "arguments": call.args.to_string()}}]}));
                        t.push(json!({"role": "tool", "tool_call_id": id, "content": result}));
                    }
                    let q = questions_after_navigate(&t);
                    assert!(q.is_empty(), "a question after the form opened: {q:?}");
                }
                Step::Final(a) => return (t, a),
                Step::Extract(_) => panic!("no extraction in this flow"),
                Step::Done(v) => panic!("this machine is not converted yet: {v}"),
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

    const VAULT: &str = "- id=vlt_1 · Passport Lucas · id_document · •••• 567X · fields: number, name, expiry, country · only on: apdtrefund.hk\n\
- id=vlt_2 · Visa Lucas · card · •••• 1111 · fields: number, expiry, cvc, holder · only on: apdtrefund.hk";

    fn profile() -> Value {
        json!({"name_en": "LUCAS LU", "birthday": "1985-04-12", "nationality": "Australia", "email_personal": "lucas@example.com"})
    }

    #[test]
    fn it_runs_only_on_a_request_to_apply() {
        assert!(active(&start(REQUEST)).is_some());
        let q =
            start("Do I need to claim the Hong Kong departure tax back? How does the refund work?");
        assert!(active(&q).is_none());
    }

    #[test]
    fn the_refund_is_applied_by_code_from_mail_to_the_face_step() {
        // apdt_real 2026-11-01: the model stopped on the upload page asking for a residential address the platform
        // never asks for; code runs every step, no question once the platform is open, and stops before the face step
        let user = vec![REQUEST.to_string()];
        let p = profile();
        let w = world(&user, &p, true);
        let (t, a) = run(start(REQUEST), &w, VAULT);
        // the fixed search, the three documents saved and read
        let q = calls_of(&t, "gmail_search");
        assert_eq!(q.len(), 1);
        assert!(sget(&q[0], "query").contains("\"boarding pass\""));
        assert_eq!(calls_of(&t, "gmail_save_attachment").len(), 3);
        // every upload in its box: the passport the user attached, the two boarding passes, the receipt
        let ups: Vec<(String, String)> = calls_of(&t, "browser_upload")
            .iter()
            .map(|a| (sget(a, "ref"), sget(a, "path")))
            .collect();
        assert_eq!(
            ups,
            vec![
                ("e9".into(), "uploads/2026-11/passport.jpg".into()),
                (
                    "e11".into(),
                    "attachments/BoardingPass_SQ890_LU_LUCAS.pdf".into()
                ),
                (
                    "e15".into(),
                    "attachments/BoardingPass_CX500_LU_LUCAS.pdf".into()
                ),
                (
                    "e20".into(),
                    "attachments/ETKT_1602345678901_Receipt.pdf".into()
                ),
            ]
        );
        // the facts the details stage got
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
        // the Visa card from the Vault, field by field; no takeover, no face click
        let secrets: Vec<String> = calls_of(&t, "browser_fill_secret")
            .iter()
            .map(|a| format!("{}:{}", sget(a, "field"), sget(a, "ref")))
            .collect();
        assert_eq!(secrets, ["number:e6", "expiry:e8", "holder:e7"]);
        assert!(calls_of(&t, "browser_request_takeover").is_empty());
        assert!(calls_of(&t, "browser_click").is_empty());
        assert!(calls_of(&t, "ask_user").is_empty());
        // the answer: the reference from the page, the application's link, the card by its last digits, the deadline
        assert!(a.contains("reference number **K7P2QD**"), "{a}");
        assert!(a.contains(SUBMIT_URL), "{a}");
        assert!(a.contains("Visa Lucas card ending 1111"), "{a}");
        assert!(
            a.contains("SQ 890 on 29 October 2026") && a.contains("CX 500 on 30 October 2026"),
            "{a}"
        );
        assert!(a.contains("HK$120 departure tax"), "{a}");
        assert!(a.contains("by **27 November 2026**"), "{a}");
        assert!(
            !a.contains("E1234567X") && !a.to_lowercase().contains("address"),
            "{a}"
        );
    }

    #[test]
    fn older_pages_cut_by_the_compression_do_not_stop_the_run() {
        // apdt_real 2026-11-01: after the card number went in, the saved-reference snapshot (compressed by then) was
        // read again and the run ended "stopped on the upload page"
        let user = vec![REQUEST.to_string()];
        let p = profile();
        let w = world(&user, &p, true);
        let (t, a) = run_with(start(REQUEST), &w, VAULT, true);
        assert_eq!(calls_of(&t, "browser_fill_secret").len(), 3, "{a}");
        assert_eq!(calls_of(&t, "browser_upload").len(), 4, "{a}");
        assert!(a.contains("reference number **K7P2QD**"), "{a}");
        assert!(
            a.contains("Visa Lucas card ending 1111") && a.contains("SQ 890 on 29 October 2026"),
            "{a}"
        );
    }

    #[test]
    fn no_question_after_the_form_opens_and_a_card_by_name_only_is_typed_by_the_user() {
        let user = vec![REQUEST.to_string()];
        let p = profile();
        let w = world(&user, &p, true);
        let vault = "- id=vlt_2 · Visa Lucas · card · UNKNOWN · fields: number, expiry · only on: apdtrefund.hk";
        let (t, a) = run(start(REQUEST), &w, vault);
        assert!(calls_of(&t, "browser_fill_secret").is_empty());
        let tk = calls_of(&t, "browser_request_takeover");
        assert_eq!(tk.len(), 1);
        assert!(sget(&tk[0], "reason").contains("Card Number"));
        assert!(questions_after_navigate(&t).is_empty());
        assert!(a.contains("K7P2QD"), "{a}");
    }

    #[test]
    fn whatever_is_missing_is_asked_once_before_the_platform_opens() {
        // no mail, no photo, no refund method: one question, and the platform is not opened
        let req = "Please apply for my Hong Kong departure tax refund.";
        let user = vec![req.to_string()];
        let p = profile();
        let w = world(&user, &p, false);
        let mut t = start(req);
        t[0] = json!({"role": "user", "content": req});
        let Step::Final(q) = next(&APDT, &t, &w) else {
            panic!("expected the question")
        };
        assert!(q.starts_with(ASK_MARK), "{q}");
        assert!(
            q.contains("photo page of your passport") && q.contains("how you want the refund"),
            "{q}"
        );
        assert!(!q.to_lowercase().contains("address"), "{q}");
        // asked once: the next turn without the answers ends, never opens the form
        t.push(json!({"role": "assistant", "content": q}));
        t.push(json!({"role": "user", "content": "hmm"}));
        let user = vec![req.to_string(), "hmm".to_string()];
        let w = world(&user, &p, false);
        let Step::Final(end) = next(&APDT, &t, &w) else {
            panic!("expected the end")
        };
        assert!(end.contains("cannot be applied for"), "{end}");
        // a platform that asks for an address: the profile's home address, or the one question
        let mut f = Facts {
            passport: Some("uploads/p.jpg".into()),
            method: Some(Method::Cash),
            ..trip_facts()
        };
        assert!(f.missing(false).is_empty());
        assert_eq!(f.missing(true), ["your residential address"]);
        f.address = "1 George St, Sydney NSW 2000, Australia".into();
        assert!(f.missing(true).is_empty());
    }

    /// The three documents of the trip, read.
    fn trip_facts() -> Facts {
        Facts {
            inbound: Some(classify("a/BoardingPass_SQ890.pdf", &pdf_text("SQ890"), "")),
            outbound: Some(classify("a/BoardingPass_CX500.pdf", &pdf_text("CX500"), "")),
            receipt: Some(classify("a/ETKT_Receipt.pdf", &pdf_text("receipt"), "")),
            ..Default::default()
        }
    }

    #[test]
    fn the_eligibility_rule_is_code() {
        let mut f = trip_facts();
        assert_eq!(f.inbound.as_ref().unwrap().kind, DocKind::Inbound);
        assert_eq!(f.outbound.as_ref().unwrap().kind, DocKind::Outbound);
        assert_eq!(f.receipt.as_ref().unwrap().tax, Some(120));
        assert_eq!(ineligible(&f, today(), 28), None);
        // more than 28 days after the departure flight
        let late = NaiveDate::from_ymd_opt(2026, 11, 28).unwrap();
        assert!(ineligible(&f, late, 28)
            .unwrap()
            .contains("28 calendar days"));
        // not yet departed
        let early = NaiveDate::from_ymd_opt(2026, 10, 29).unwrap();
        assert!(ineligible(&f, early, 28)
            .unwrap()
            .contains("only after you have left"));
        // stayed two nights
        f.outbound.as_mut().unwrap().date = NaiveDate::from_ymd_opt(2026, 10, 31);
        assert!(ineligible(&f, today(), 28)
            .unwrap()
            .contains("same or the next calendar day"));
        f.outbound.as_mut().unwrap().date = NaiveDate::from_ymd_opt(2026, 10, 30);
        // a receipt without the tax
        f.receipt = Some(classify(
            "a/ETKT_Receipt.pdf",
            "ELECTRONIC TICKET RECEIPT\nFare HKD 2,860.00\nYR HKD 180.00 surcharge",
            "",
        ));
        assert!(ineligible(&f, today(), 28)
            .unwrap()
            .contains("does not show"));
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
        assert_eq!(reference_on(SUBMIT_URL, ""), Some("K7P2QD".into()));
        assert_eq!(
            reference_on(
                "https://app.apdtrefund.hk/x",
                "  Submit your application Q3WX8Z Proceed"
            ),
            Some("Q3WX8Z".into())
        );
        assert_eq!(
            method_in(&["refund it to my Visa card".into()]),
            Some(Method::Card)
        );
        assert_eq!(
            method_in(&["my boarding card is attached; cash please".into()]),
            Some(Method::Cash)
        );
        assert_eq!(
            method_in(&["to AlipayHK, account id: lucas88".into()]),
            Some(Method::Wallet("alipayHK", "lucas88".into()))
        );
        assert_eq!(method_in(&["please apply".into()]), None);
        assert_eq!(
            passport_file(&[
                "uploads/2026-11/passport.jpg".into(),
                "uploads/x.pdf".into()
            ]),
            Some("uploads/2026-11/passport.jpg".into())
        );
    }

    #[test]
    fn a_site_error_ends_the_run_with_the_sites_words() {
        let user = vec![REQUEST.to_string()];
        let p = profile();
        let w = world(&user, &p, true);
        let mut t = start(REQUEST);
        // run up to the details fill, then the platform refuses
        loop {
            let Step::Calls(c) = next(&APDT, &t, &w) else {
                panic!("ended early")
            };
            let call = &c[0];
            let stage = sget(&call.args, "stage");
            let result = if call.name == "browser_snapshot"
                && results_of(&t, "form_fill")
                    .last()
                    .map(|(a, _)| sget(a, "stage"))
                    == Some("details".into())
            {
                page("https://app.apdtrefund.hk/hkTaxMobile/apdt_upload.html?lang=en",
                     "  Departure date has exceeded the application deadline (28 calendar days from the departure date).\n  Please check the items marked above.")
            } else {
                respond(&t, &call.name, &call.args, VAULT)
            };
            for (j, cl) in c.iter().enumerate() {
                let id = format!("{}_{j}", cl.id);
                let r = if j == 0 {
                    result.clone()
                } else {
                    respond(&t, &cl.name, &cl.args, VAULT)
                };
                t.push(json!({"role": "assistant", "content": "", "tool_calls": [{"id": id, "type": "function",
                    "function": {"name": cl.name, "arguments": cl.args.to_string()}}]}));
                t.push(json!({"role": "tool", "tool_call_id": id, "content": r}));
            }
            if stage == "details" {
                break;
            }
        }
        let Step::Calls(c) = next(&APDT, &t, &w) else {
            panic!()
        };
        assert_eq!(c[0].name, "browser_snapshot");
        let id = "s_err";
        t.push(
            json!({"role": "assistant", "content": "", "tool_calls": [{"id": id, "type": "function",
            "function": {"name": "browser_snapshot", "arguments": "{}"}}]}),
        );
        t.push(json!({"role": "tool", "tool_call_id": id, "content": page("https://app.apdtrefund.hk/hkTaxMobile/apdt_upload.html?lang=en",
            "  Departure date has exceeded the application deadline (28 calendar days from the departure date).\n  Please check the items marked above.")}));
        let Step::Final(a) = next(&APDT, &t, &w) else {
            panic!("expected the end")
        };
        assert!(a.contains("Departure date has exceeded"), "{a}");
        assert!(questions_after_navigate(&t).is_empty());
    }
}
