//! The code driver of the claim skills and the FWD purchase (`csr-trip-claim`, `fwd-travel-insurance`): once the
//! skill is loaded, code decides every step — the fixed mail searches, the facts read from the mail by regex (an
//! extraction call only as the last resort), the benefit by rule, the amount by arithmetic, the deadlines, the form
//! filled by its function (`crate::forms`, fixed ids), the user's own steps and the final Submit / Pay handed over, the
//! reference read from the confirmation, the reminders and the answer. As in [`super::driver`], the next step is a
//! function of the transcript alone (`next`), so a run suspended for a takeover carries on where it was.
//!
//! Never decided by code: the health and declaration questions of an application (left for the user and named in the
//! hand-over), the final click (Submit, Pay now), the signature and the HKID / bank details on a PDF form.

use std::sync::LazyLock;

use chrono::{Datelike, Duration, NaiveDate};
use omuse_common::util::{get_str, take_chars};
use regex::Regex;
use serde_json::{json, Map, Value};

use super::driver::{
    asked_before, asks_to_act, country_of_city, dates_in, flight_no_in, handed_back, json_of, pos,
    results_of, snapshot_text, stopped, user_asks, user_words, walk_messages, Step, World,
};
use super::util::{content_of, sget, stored_call, tool_calls};
use crate::llm::ToolCall;

/// What a spec drives.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Flow {
    /// the Chase Sapphire trip cancellation / interruption claim on eClaimsLine
    CsrTrip,
    /// FWD Singapore's online travel-delay claim
    FwdSgClaim,
    /// FWD Singapore's quote, plan pick and application (the user pays)
    FwdSgBuy,
    /// FWD Hong Kong: bolttech's PDF claim form and the email draft
    FwdHkClaim,
}

/// One claim or purchase: what the skill text used to tell the model, as data.
pub struct ClaimSpec {
    pub skill: &'static str,
    pub flow: Flow,
    /// the claim's name in the answer
    pub name: &'static str,
    /// who decides the claim (the answer says so)
    pub decides: &'static str,
    /// the page to open (the claim form, the quote, the PDF)
    pub start_url: &'static str,
    /// its host, to know the page was reached
    pub host: &'static str,
    /// the fixed mail searches; `{flight}` is the flight number's words when the request names one
    pub searches: &'static [&'static str],
    /// hits of each search to read
    pub hits: usize,
    /// days after the loss to notify the insurer (0 = from the policy wording)
    pub notify_days: i64,
    /// days after the loss for the documents (0 = none)
    pub documents_days: i64,
    /// what the user does after the review hand-over
    pub user_steps: &'static str,
    /// the confirmation's reference, on the page after the user's Submit / Pay
    pub ref_re: &'static str,
    pub currency: &'static str,
    /// a delay benefit: (amount per block, hours per block, the cap); 0 when not a delay benefit
    pub delay: (f64, i64, f64),
    /// the benefit's caps: (what, amount)
    pub caps: &'static [(&'static str, f64)],
    /// where a claim goes by email (Hong Kong), else empty
    pub claims_email: &'static str,
}

const CSR_SEARCHES: &[&str] = &[
    "(e-ticket OR eTicket OR itinerary OR booking OR reservation OR confirmation) newer_than:180d",
    "(Chase OR Sapphire) (transaction OR charged OR statement OR receipt) newer_than:180d",
    "(refund OR refunded OR credit OR voucher OR cancellation OR cancelled OR \"early departure\" OR \"change fee\" OR \"itinerary change\") newer_than:180d",
    "insurance (policy OR certificate OR travel) newer_than:365d",
];
const FWD_CLAIM_SEARCHES: &[&str] = &[
    "(FWD OR bolttech) (policy OR certificate) travel newer_than:365d",
    "{flight}(delay OR delayed OR cancelled OR irregularity OR baggage OR certificate) newer_than:60d",
];
const FWD_BUY_SEARCHES: &[&str] =
    &["(flight OR e-ticket OR itinerary OR booking OR reservation) newer_than:90d"];

pub const SPECS: &[ClaimSpec] = &[
    ClaimSpec {
        skill: "csr-trip-claim",
        flow: Flow::CsrTrip,
        name: "Chase Sapphire trip claim",
        decides: "Card Benefit Services (Chase's benefit administrator)",
        start_url: "https://www.eclaimsline.com/",
        host: "www.eclaimsline.com",
        searches: CSR_SEARCHES,
        hits: 5,
        notify_days: 20,
        documents_days: 90,
        user_steps: "press Submit claim",
        ref_re: r"\bCBS-\d{2}-\d{6,}\b",
        currency: "USD",
        delay: (0.0, 0, 0.0),
        caps: &[
            ("per traveller", 10_000.0),
            ("per trip", 20_000.0),
            ("per 12 months", 40_000.0),
        ],
        claims_email: "",
    },
    ClaimSpec {
        skill: "fwd-travel-insurance",
        flow: Flow::FwdSgClaim,
        name: "FWD Singapore travel delay claim",
        decides: "FWD Singapore Pte. Ltd.",
        start_url: "https://www.fwd.com.sg/claim-online/travel.html",
        host: "www.fwd.com.sg",
        searches: FWD_CLAIM_SEARCHES,
        hits: 3,
        notify_days: 30,
        documents_days: 0,
        user_steps: "press Submit claim",
        ref_re: r"\bCL-[A-Z]{2}-\d{4}-\d{4,}\b",
        currency: "SGD",
        delay: (100.0, 6, 1_500.0),
        caps: &[],
        claims_email: "",
    },
    ClaimSpec {
        skill: "fwd-travel-insurance",
        flow: Flow::FwdSgBuy,
        name: "FWD Singapore travel insurance",
        decides: "FWD Singapore Pte. Ltd.",
        start_url: "https://www.fwd.com.sg/travel-insurance/online-quote/",
        host: "www.fwd.com.sg",
        searches: FWD_BUY_SEARCHES,
        hits: 3,
        notify_days: 0,
        documents_days: 0,
        user_steps: "read the policy wording and the declarations, tick the box if you agree, and press Pay now",
        ref_re: r"\bTI-[A-Z]{2}-\d{4}-\d{4,}\b",
        currency: "SGD",
        delay: (0.0, 0, 0.0),
        caps: &[],
        claims_email: "",
    },
    ClaimSpec {
        skill: "fwd-travel-insurance",
        flow: Flow::FwdHkClaim,
        name: "FWD Hong Kong travel delay claim",
        decides: "bolttech Insurance (Hong Kong) Company Limited, the underwriter of FWD's travel plans",
        start_url: "https://www.fwd.com.hk/files/v3/assets/blta9d684affff23c8c/bltf22c68114bd0cfb6/649e7bda1ea829102cf584d2/travel-insurance-clm-form-fillable.pdf",
        host: "www.fwd.com.hk",
        searches: FWD_CLAIM_SEARCHES,
        hits: 3,
        notify_days: 0,
        documents_days: 0,
        user_steps: "sign the form and send the draft",
        ref_re: "",
        currency: "HKD",
        delay: (300.0, 6, 1_500.0),
        caps: &[],
        claims_email: "claims.hk@bolttechinsurance.com",
    },
];

/// The card fields of FWD Singapore's application, filled from the Vault after `purchase_confirm`: (Vault field, the
/// input's label in the snapshot).
const CARD_FIELDS: &[(&str, &str)] = &[
    ("number", "Card number"),
    ("expiry", "Expiry"),
    ("cvc", "CVC"),
    ("holder", "Name on card"),
];

fn spec(flow: Flow) -> &'static ClaimSpec {
    SPECS.iter().find(|s| s.flow == flow).unwrap()
}

static CLAIM_WORDS: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)\bclaims?\b|\breimburs|理赔|索赔").unwrap());
static BUY_WORDS: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\b(buy|purchase|get me|quote|insure|cover(age)? for|insurance for|which .* plan|cheapest)\b|买|购买|报价")
        .unwrap()
});
/// A request to buy (not a question about a plan).
static BUY_ACT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\b(buy|purchase|get me|insure me|sign me up)\b|帮我买|买一份|购买").unwrap()
});
static DELAY_WORDS: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)\bdelay(ed|s)?\b|延误|延誤").unwrap());
static HK_WORDS: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)hong\s?kong|\bhk\b|mytravel|travelcare|\bgba\b|bolttech|fwd\.com\.hk|香港")
        .unwrap()
});
static SG_WORDS: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)singapore|\bsg\b|fwd\.com\.sg|\bsgd\b|s\$|新加坡").unwrap());

/// The claim or purchase this run's last loaded skill asks for, when it is one the driver knows: the CSR claim; for
/// the FWD skill the flow from the user's words (claim or buy; Hong Kong or Singapore — the market also from the
/// policy email's sender once mail was read, Singapore otherwise). Only a flight-delay claim is driven; other FWD
/// claims and the Hong Kong purchase stay on the model path.
pub fn active(transcript: &[Value]) -> Option<&'static ClaimSpec> {
    let loaded: Vec<String> = transcript
        .iter()
        .flat_map(tool_calls)
        .filter_map(|tc| {
            let (name, args) = stored_call(tc);
            (name == "load_skill" && sget(&args, "file").trim().is_empty())
                .then(|| sget(&args, "name"))
        })
        .filter(|n| SPECS.iter().any(|s| s.skill == n))
        .collect();
    let skill = loaded.last()?;
    // the user's own request asks for the claim to be made (or the policy bought): a skill loaded for information
    // ("what can I claim?") leaves the run to the model
    let act = asks_to_act(transcript);
    if skill == "csr-trip-claim" {
        return act.then(|| spec(Flow::CsrTrip));
    }
    let texts = user_words(transcript);
    let words = texts.join("\n");
    let claim = CLAIM_WORDS.is_match(&words);
    let buy = !claim && BUY_WORDS.is_match(&words);
    if !(act || (buy && user_asks(&texts, &BUY_ACT))) {
        return None;
    }
    let hk = HK_WORDS.is_match(&words);
    let sg = SG_WORDS.is_match(&words);
    if claim || (!buy && DELAY_WORDS.is_match(&words)) {
        if !DELAY_WORDS.is_match(&words) {
            return None;
        }
        let hk_mail = policy_market(transcript) == Some(Market::Hk);
        return Some(if hk || (!sg && hk_mail) {
            spec(Flow::FwdHkClaim)
        } else {
            spec(Flow::FwdSgClaim)
        });
    }
    if buy && !hk {
        return Some(spec(Flow::FwdSgBuy));
    }
    None
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Market {
    Sg,
    Hk,
}

/// The market of the FWD policy in the mail: only the emails of an FWD or bolttech sender count (the policy email),
/// read from the structured results of `gmail_search` / `gmail_get_message` — the sender's domain first
/// (`fwd.com.hk` or bolttech: Hong Kong; `fwd.com.sg`: Singapore), then the currency of its subject and body. `None`
/// when no such email was read (the caller defaults to Singapore). The skill's own text, which names bolttech and
/// fwd.com.hk, never counts (the gate on b24962d sent a Singapore claim to bolttech because of it).
fn policy_market(transcript: &[Value]) -> Option<Market> {
    static HKD: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\bHKD\b|HK\$").unwrap());
    static SGD: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\bSGD\b|S\$").unwrap());
    let mut mails: Vec<Value> = Vec::new();
    for tool in ["gmail_search", "gmail_get_message"] {
        for (_, m) in results_of(transcript, tool) {
            let v = json_of(m);
            if v.get("from").is_some() {
                mails.push(v.clone());
            }
            if let Some(list) = v.get("messages").and_then(Value::as_array) {
                mails.extend(list.iter().cloned());
            }
        }
    }
    let mut found = None;
    for v in &mails {
        let from = get_str(v, "from").to_lowercase();
        if !(from.contains("fwd") || from.contains("bolttech")) {
            continue;
        }
        let domain = from.rsplit('@').next().unwrap_or("").trim_end_matches('>');
        let by_domain = if domain.ends_with(".hk") || domain.contains("bolttech") {
            Some(Market::Hk)
        } else if domain.ends_with(".sg") {
            Some(Market::Sg)
        } else {
            None
        };
        let text = ["subject", "snippet", "body"]
            .iter()
            .map(|k| get_str(v, k))
            .collect::<Vec<_>>()
            .join("\n");
        let by_currency = if HKD.is_match(&text) {
            Some(Market::Hk)
        } else if SGD.is_match(&text) {
            Some(Market::Sg)
        } else {
            None
        };
        if let Some(m) = by_domain.or(by_currency) {
            // a fetched message (with its body) outranks a search hit read earlier
            found = Some(m);
            if !get_str(v, "body").is_empty() {
                return found;
            }
        }
    }
    found
}

// ---------------------------------------------------------------- the mail, read back

/// One fetched email.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Msg {
    pub id: String,
    pub from: String,
    pub subject: String,
    pub body: String,
    pub attachments: Vec<String>,
}

impl Msg {
    fn text(&self) -> String {
        format!("{}\n{}", self.subject, self.body)
    }
    /// The sender's display name ("United Airlines").
    fn sender(&self) -> String {
        let name = self
            .from
            .split('<')
            .next()
            .unwrap_or("")
            .trim()
            .trim_matches('"');
        if name.contains('@') {
            String::new()
        } else {
            name.to_string()
        }
    }
}

fn fetched(transcript: &[Value]) -> Vec<Msg> {
    results_of(transcript, "gmail_get_message")
        .iter()
        .map(|(a, m)| {
            let v = json_of(m);
            Msg {
                id: sget(a, "message_id"),
                from: get_str(&v, "from"),
                subject: get_str(&v, "subject"),
                body: get_str(&v, "body"),
                attachments: v
                    .get("attachments")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .map(|x| {
                        if x.is_object() {
                            get_str(x, "filename")
                        } else {
                            x.as_str().unwrap_or("").to_string()
                        }
                    })
                    .filter(|x| !x.is_empty())
                    .collect(),
            }
        })
        .collect()
}

fn call(n: usize, name: &str, args: Value) -> ToolCall {
    ToolCall {
        id: format!("clm_{n}"),
        name: name.into(),
        args,
    }
}

/// The fixed searches, then the first hits of each: the step to run, or `None` once the mail is in.
fn gather(s: &ClaimSpec, transcript: &[Value], w: &World, n: usize, flight: &str) -> Option<Step> {
    let searches = results_of(transcript, "gmail_search");
    if w.mail_ready && searches.is_empty() {
        let fw = if flight.is_empty() {
            String::new()
        } else {
            format!("(\"{}\" OR {}) ", flight, flight.replace(' ', ""))
        };
        return Some(Step::Calls(
            s.searches
                .iter()
                .enumerate()
                .map(|(i, q)| {
                    call(
                        n + i,
                        "gmail_search",
                        json!({"query": q.replace("{flight}", &fw), "max_results": 10}),
                    )
                })
                .collect(),
        ));
    }
    let have: Vec<String> = fetched(transcript).iter().map(|m| m.id.clone()).collect();
    let mut want: Vec<String> = Vec::new();
    for (_, m) in &searches {
        let mut hits = Vec::new();
        walk_messages(&json_of(m), &mut hits);
        for h in hits.iter().take(s.hits) {
            let id = get_str(h, "id");
            if !id.is_empty() && !have.contains(&id) && !want.contains(&id) {
                want.push(id);
            }
        }
    }
    (!want.is_empty()).then(|| {
        Step::Calls(
            want.iter()
                .enumerate()
                .map(|(i, id)| call(n + i, "gmail_get_message", json!({"message_id": id})))
                .collect(),
        )
    })
}

// ---------------------------------------------------------------- text helpers

static AMOUNT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)(?:USD|SGD|HKD|US\$|S\$|HK\$|\$)\s?(\d{1,3}(?:,\d{3})+|\d+)(?:\.(\d{1,2}))?\b|\b(\d{1,3}(?:,\d{3})+|\d+)(?:\.(\d{2}))?\s?(?:USD|SGD|HKD)\b").unwrap()
});

/// Every amount of money written in `text`, in order.
pub fn amounts_in(text: &str) -> Vec<f64> {
    AMOUNT
        .captures_iter(text)
        .filter_map(|c| {
            let (whole, cents) = match (c.get(1), c.get(3)) {
                (Some(w), _) => (w.as_str(), c.get(2).map(|x| x.as_str()).unwrap_or("0")),
                (None, Some(w)) => (w.as_str(), c.get(4).map(|x| x.as_str()).unwrap_or("0")),
                _ => return None,
            };
            let w: f64 = whole.replace(',', "").parse().ok()?;
            let c: f64 = format!("0.{cents:0<2}").parse().ok()?;
            Some(w + c)
        })
        .collect()
}

static SENTENCE_END: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\.\s+|\.$|\n+|;\s+").unwrap());

fn sentences(text: &str) -> Vec<&str> {
    SENTENCE_END
        .split(text)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .collect()
}

/// "1,753.60"
pub fn money(x: f64) -> String {
    let s = format!("{x:.2}");
    let (int, frac) = s.split_once('.').unwrap_or((&s, "00"));
    let mut out = String::new();
    for (i, ch) in int.chars().enumerate() {
        if i > 0 && (int.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(ch);
    }
    format!("{out}.{frac}")
}

fn iso(d: NaiveDate) -> String {
    d.format("%Y-%m-%d").to_string()
}

fn long(d: NaiveDate) -> String {
    d.format("%-d %B %Y").to_string()
}

fn parse_iso(s: &str) -> Option<NaiveDate> {
    NaiveDate::parse_from_str(s, "%Y-%m-%d").ok()
}

/// The first date in the user's words that is not in the future (the day it happened).
fn past_date_in(texts: &[String], today: NaiveDate) -> Option<NaiveDate> {
    texts
        .iter()
        .flat_map(|t| dates_in(t))
        .map(|(_, d)| d)
        .find(|d| *d <= today)
}

// ---------------------------------------------------------------- the CSR trip claim: facts and rules

/// The facts of a Chase Sapphire trip claim, read from the mail and the user's words.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TripClaim {
    /// "Trip Cancellation" | "Trip Interruption"
    pub benefit: String,
    /// YYYY-MM-DD
    pub loss_date: String,
    pub paid: f64,
    pub refunded: f64,
    pub change_fee: f64,
    /// what the suppliers' letters state as not refunded (the unused prepaid part)
    pub lost: f64,
    pub amount: f64,
    /// (supplier, booking reference)
    pub suppliers: Vec<(String, String)>,
    pub last4: String,
    pub card_product: String,
    pub reason: String,
    pub travellers: usize,
    /// the flights and stays, for the answer
    pub trip: String,
    pub has_doctor_letter: bool,
    pub has_supplier_letter: bool,
}

static LETTER_WORDS: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\brefund(?:ed|s)?\b|early departure|\bcancell(?:ed|ation)\b|\bcredit(?:ed)?\b|\bvoucher\b|checked out|change fee|itinerary change").unwrap()
});
static BOOKING_SUBJECT: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)e-?ticket|itinerary|booking|reservation|confirm").unwrap());
static PAID_WORDS: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)\b(fare|total|rate|price|amount|paid|charged)\b").unwrap());
static FEE_WORDS: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)\b(change|rebooking) fee\b|fare difference").unwrap());
static REFUNDED_WORDS: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)\brefunded\b|\bcredited\b|\bcredit of\b").unwrap());
static LOST_WORDS: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)non-?refundable|not refundable|forfeit|no refund").unwrap());
static INTERRUPTED: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)cut (?:the |my |our )?trip short|cut (?:it )?short|interrupt|fl(?:ew|y|own) (?:back|home)|(?:came|come|went|go|return(?:ed)?) home (?:early|that)|had to (?:leave|return|go home)|中断|提前回").unwrap()
});
static CANCELLED: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)couldn'?t (?:take|make|fly|travel|go)|could not (?:take|make|fly|travel|go)|missed (?:my|the|our) flight|never left|didn'?t (?:go|fly|travel)|unable to travel|cancel|没去成|取消").unwrap()
});
static MAIL_INTERRUPTED: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)early departure|checked out|itinerary change|moved (?:from|forward)|return(?:ed)? early").unwrap()
});
static INJURY: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\b(injur|broke|broken|fractur|sprain|accident|wrist|ankle|concussion)")
        .unwrap()
});
static FAMILY: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\b(?:my|our) (?:wife|husband|partner|son|daughter|mother|father|mom|dad|child|kids?|parents?|brother|sister)\b").unwrap()
});
static COMPANION: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)travel(?:ling)? companion|my friend").unwrap());
static DOCTOR_LETTER: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)(?:doctor|dr\.?|physician|clinic|hospital)[^.\n]{0,80}\b(letter|certificate|note|statement|report)\b|\b(letter|certificate|note)\b[^.\n]{0,40}(?:doctor|physician|clinic)").unwrap()
});
static BOOKING_REF: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\b(?i:confirmation|booking reference|booking ref|reservation|record locator|PNR)\b(?:\s+(?i:confirmed|number|no\.?|code|is))?[^A-Za-z0-9\n]{0,12}([A-Z]{2}-\d{4,8}|[A-Z0-9]{6})\b").unwrap()
});
static LAST4: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)(?:ending(?: in)?|\.{3}|•{3,}|\*{3,}|x{4}|last (?:4|four)(?: digits)?)\s?(\d{4})\b",
    )
    .unwrap()
});
static TRAVELLERS: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\b(\d+) (?:of us|travellers|travelers|passengers|people|adults|tickets)\b|\b(?:my|our) (?:wife|husband|partner|son|daughter) and (?:I|me)\b|\b(?:two|both) of us\b").unwrap()
});

fn is_letter(m: &Msg) -> bool {
    LETTER_WORDS.is_match(&m.text())
}

fn is_booking(m: &Msg) -> bool {
    BOOKING_SUBJECT.is_match(&m.subject) && !is_letter(m)
}

/// The first amount of the first sentence of `text` that matches `words` (and not `not`).
fn amount_where(text: &str, words: &Regex, not: Option<&Regex>, max: bool) -> Option<f64> {
    sentences(text)
        .into_iter()
        .filter(|s| words.is_match(s) && !not.is_some_and(|n| n.is_match(s)))
        .find_map(|s| {
            let a = amounts_in(s);
            if max && s.to_lowercase().contains("total") {
                a.iter()
                    .cloned()
                    .fold(None, |m: Option<f64>, x| Some(m.map_or(x, |m| m.max(x))))
            } else {
                a.first().cloned()
            }
        })
}

fn travellers_in(texts: &[String]) -> usize {
    for t in texts {
        if let Some(c) = TRAVELLERS.captures(t) {
            return c.get(1).and_then(|m| m.as_str().parse().ok()).unwrap_or(2);
        }
    }
    1
}

/// The claim's facts: the benefit by the skill's rule (the trip had started → interruption), the amounts from the
/// bookings and the suppliers' letters, the loss date from the user's words (else the letters, else the e-ticket),
/// the reason in general terms. Regexes only.
pub fn trip_claim(msgs: &[Msg], user_texts: &[String], today: NaiveDate) -> TripClaim {
    let mut t = TripClaim {
        travellers: travellers_in(user_texts),
        ..Default::default()
    };
    let words = user_texts.join("\n");
    let letters: Vec<&Msg> = msgs.iter().filter(|m| is_letter(m)).collect();
    let bookings: Vec<&Msg> = msgs.iter().filter(|m| is_booking(m)).collect();
    // the benefit: the user's words first, then the mail (an early departure, a moved flight), else cancellation
    t.benefit = if INTERRUPTED.is_match(&words) && !CANCELLED.is_match(&words) {
        "Trip Interruption"
    } else if CANCELLED.is_match(&words) {
        "Trip Cancellation"
    } else if letters.iter().any(|m| MAIL_INTERRUPTED.is_match(&m.text())) {
        "Trip Interruption"
    } else {
        "Trip Cancellation"
    }
    .into();
    // the amounts
    for m in &bookings {
        if let Some(a) = amount_where(&m.body, &PAID_WORDS, Some(&FEE_WORDS), true) {
            t.paid += a;
        }
    }
    for m in &letters {
        if let Some(a) = amount_where(&m.text(), &REFUNDED_WORDS, None, false) {
            t.refunded += a;
        }
        if let Some(a) = amount_where(&m.text(), &LOST_WORDS, Some(&REFUNDED_WORDS), false) {
            t.lost += a;
            t.has_supplier_letter = true;
        }
    }
    for m in msgs {
        if let Some(a) = amount_where(&m.text(), &FEE_WORDS, None, false) {
            t.change_fee += a;
        }
    }
    if t.paid == 0.0 {
        // the card's charge alerts, the change fee left out
        for m in msgs
            .iter()
            .filter(|m| m.subject.to_lowercase().contains("transaction"))
        {
            if let Some(a) = amounts_in(&m.subject)
                .first()
                .or(amounts_in(&m.body).first())
            {
                if (a - t.change_fee).abs() > 0.005 {
                    t.paid += a;
                }
            }
        }
    }
    // the user's own figures when the mail has none
    static USER_PAID: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"(?i)\b(paid|cost|fare|total)\b").unwrap());
    if t.paid == 0.0 {
        if let Some(a) = amount_where(&words, &USER_PAID, Some(&FEE_WORDS), false) {
            t.paid = a;
        }
    }
    if t.refunded == 0.0 {
        if let Some(a) = amount_where(&words, &REFUNDED_WORDS, None, false) {
            t.refunded = a;
        }
    }
    if t.change_fee == 0.0 {
        if let Some(a) = amount_where(&words, &FEE_WORDS, None, false) {
            t.change_fee = a;
        }
    }
    // the loss date
    if let Some(d) = past_date_in(user_texts, today) {
        t.loss_date = iso(d);
    } else if t.benefit == "Trip Interruption" {
        if let Some(d) = letters.iter().find_map(|m| {
            sentences(&m.text())
                .into_iter()
                .filter(|s| MAIL_INTERRUPTED.is_match(s))
                .find_map(|s| {
                    dates_in(s)
                        .into_iter()
                        .map(|(_, d)| d)
                        .find(|d| *d <= today)
                })
        }) {
            t.loss_date = iso(d);
        }
    } else if let Some(d) = bookings
        .iter()
        .find_map(|m| dates_in(&m.text()).first().map(|(_, d)| *d))
    {
        t.loss_date = iso(d);
    }
    // the reason, in general terms
    let what = if INJURY.is_match(&words) {
        "injury"
    } else {
        "sickness"
    };
    let who = if FAMILY.is_match(&words) {
        "an immediate family member"
    } else if COMPANION.is_match(&words) {
        "a travelling companion"
    } else {
        "the cardholder"
    };
    t.reason = format!("{what} of {who}");
    t.has_doctor_letter = DOCTOR_LETTER.is_match(&words);
    // the suppliers and their references, the card
    let all_text: String = msgs.iter().map(Msg::text).collect::<Vec<_>>().join("\n");
    static ISSUER: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"(?i)chase|sapphire|card benefit").unwrap());
    for m in msgs {
        let sender = m.sender();
        if sender.is_empty() || ISSUER.is_match(&sender) {
            continue;
        }
        if let Some(c) = BOOKING_REF.captures(&m.text()) {
            let r = c[1].to_string();
            if !t.suppliers.iter().any(|(_, x)| *x == r) {
                t.suppliers.push((sender, r));
            }
        }
    }
    t.last4 = LAST4
        .captures(&format!("{all_text}\n{words}"))
        .map(|c| c[1].to_string())
        .unwrap_or_default();
    t.card_product = if format!("{all_text}\n{words}")
        .to_lowercase()
        .contains("sapphire preferred")
    {
        "Chase Sapphire Preferred"
    } else {
        "Chase Sapphire Reserve"
    }
    .into();
    // the trip, for the answer
    let mut flights: Vec<String> = Vec::new();
    for m in &bookings {
        for s in sentences(&m.text()) {
            if let Some(f) = flight_no_in(s) {
                if !flights.contains(&f) && flights.len() < 4 {
                    flights.push(f);
                }
            }
        }
    }
    let stays: Vec<String> = bookings
        .iter()
        .filter(|m| {
            Regex::new(r"(?i)hotel|hyatt|marriott|hilton|resort|inn\b|nights?|check-in")
                .unwrap()
                .is_match(&m.text())
                && flight_no_in(&m.subject).is_none()
        })
        .map(|m| m.sender())
        .filter(|s| !s.is_empty())
        .collect();
    t.trip = [flights.join(", "), stays.join(", ")]
        .into_iter()
        .filter(|x| !x.is_empty())
        .collect::<Vec<_>>()
        .join("; ");
    t.amount = claim_amount(&t, spec(Flow::CsrTrip));
    t
}

/// The amount to claim: what the suppliers state as not refunded (else paid − refunded), plus the change fee for an
/// interruption, within the benefit's caps.
pub fn claim_amount(t: &TripClaim, s: &ClaimSpec) -> f64 {
    let base = if t.lost > 0.0 {
        t.lost
    } else {
        (t.paid - t.refunded).max(0.0)
    };
    let mut a = if t.benefit == "Trip Interruption" {
        base + t.change_fee
    } else {
        base
    };
    for (what, cap) in s.caps {
        let cap = if *what == "per traveller" {
            cap * t.travellers.max(1) as f64
        } else {
            *cap
        };
        if *what != "per 12 months" {
            a = a.min(cap);
        }
    }
    (a * 100.0).round() / 100.0
}

impl TripClaim {
    fn booking(&self) -> String {
        self.suppliers
            .iter()
            .map(|(s, r)| format!("{s} {r}"))
            .collect::<Vec<_>>()
            .join("; ")
    }
    /// How the amount was computed, in words: paid − refunded when that is the figure (a cancellation), else what
    /// the supplier stated as not refunded; plus the change fee for an interruption.
    fn arithmetic(&self) -> String {
        let diff = (self.paid - self.refunded).max(0.0);
        let base = if self.paid > 0.0 && (self.lost == 0.0 || (diff - self.lost).abs() < 0.01) {
            format!(
                "paid {} − refunded {}",
                money(self.paid),
                money(self.refunded)
            )
        } else {
            format!(
                "{} stated as non-refundable by the supplier",
                money(self.lost)
            )
        };
        if self.benefit == "Trip Interruption" && self.change_fee > 0.0 {
            format!(
                "{base} + change fee {} to fly home early",
                money(self.change_fee)
            )
        } else {
            base
        }
    }
    fn args(&self) -> Value {
        json!({
            "benefit": self.benefit, "loss_date": self.loss_date,
            "amount": if self.amount > 0.0 { format!("{:.2}", self.amount) } else { String::new() },
            "reason": self.reason, "booking": self.booking(), "card_product": self.card_product, "last4": self.last4,
            "paid": self.paid, "refunded": self.refunded, "change_fee": self.change_fee, "lost": self.lost,
            "trip": self.trip, "travellers": self.travellers,
        })
    }
    fn keep_from(&mut self, a: &Value) {
        let s = |k: &str| sget(a, k).trim().to_string();
        if self.loss_date.is_empty() {
            self.loss_date = s("loss_date");
        }
        if self.amount == 0.0 {
            self.amount = s("amount").parse().unwrap_or(0.0);
            self.paid = a.get("paid").and_then(Value::as_f64).unwrap_or(0.0);
            self.refunded = a.get("refunded").and_then(Value::as_f64).unwrap_or(0.0);
            self.change_fee = a.get("change_fee").and_then(Value::as_f64).unwrap_or(0.0);
            self.lost = a.get("lost").and_then(Value::as_f64).unwrap_or(0.0);
        }
        if self.suppliers.is_empty() && !s("booking").is_empty() {
            self.suppliers = s("booking")
                .split("; ")
                .filter_map(|x| {
                    x.rsplit_once(' ')
                        .map(|(a, b)| (a.to_string(), b.to_string()))
                })
                .collect();
        }
        if self.trip.is_empty() {
            self.trip = s("trip");
        }
        if self.last4.is_empty() {
            self.last4 = s("last4");
        }
    }
}

// ---------------------------------------------------------------- the FWD delay claim: facts and rules

/// The facts of a flight-delay claim on an FWD policy.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct DelayClaim {
    pub policy_no: String,
    pub plan: String,
    pub flight_no: String,
    pub from: String,
    pub to: String,
    pub airline: String,
    pub airline_ref: String,
    /// YYYY-MM-DD
    pub incident_date: String,
    /// "10:30"
    pub scheduled: String,
    pub actual: String,
    pub minutes: i64,
    pub reason: String,
    pub per_block: f64,
    pub block_hours: i64,
    pub cap: f64,
    pub amount: f64,
    /// the policy's time limit in days (the spec's when the wording is not in mail)
    pub limit_days: i64,
    /// whether the limit came from the policy email
    pub limit_from_policy: bool,
    pub emergency: String,
    /// the certificate attached to the airline's email: (message id, file name)
    pub attachment: Option<(String, String)>,
}

static TIMES: LazyLock<[Regex; 3]> = LazyLock::new(|| {
    [
        Regex::new(r"(?i)scheduled to depart at (\d{1,2}:\d{2}) and (?:actually )?departed at (\d{1,2}:\d{2})").unwrap(),
        Regex::new(r"(?i)scheduled departure:? (\d{1,2}:\d{2}),? actual departure:? (\d{1,2}:\d{2})").unwrap(),
        Regex::new(r"(?i)\bdelay[^.\n]{0,60}?(\d{1,2}:\d{2})\s*(?:→|->|to)\s*(\d{1,2}:\d{2})").unwrap(),
    ]
});
static ROUTE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"\b[A-Z]{2}\s?\d{2,4}\s+([A-Z][A-Za-z ]{2,30}?)\s?[–—-]\s?([A-Z][A-Za-z ]{2,30}?)\s+on\b",
    )
    .unwrap()
});
static ROUTE_TO: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"\b[A-Z]{2}\s?\d{2,4}\s+([A-Z][a-z]+(?: [A-Z][a-z]+)?) to ([A-Z][a-z]+(?: [A-Z][a-z]+)?)",
    )
    .unwrap()
});
static DELAY_REASON: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)(?:because of|due to|after an?|owing to|reason:)\s+([^.\n]{4,80})").unwrap()
});
static POLICY_NO: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\bpolicy\b[^A-Za-z0-9\n]{0,3}(?:number|no\.?)?[^A-Za-z0-9\n]{0,4}([A-Z]{2}-[A-Z]{2}-\d{4}-\d{4,}|[A-Z]{2,5}-?\d{6,})").unwrap()
});
static PLAN_NAME: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)\bplan:?\s+([A-Z][a-z]+)\b|\(([A-Z][a-z]+ Plan)").unwrap());
static PER_BLOCK: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)(?:SGD|HKD|S\$|HK\$)\s?(\d+) for every (\d+) (?:consecutive |full )?hours")
        .unwrap()
});
static CAP: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)up to (?:SGD|HKD|S\$|HK\$)\s?([\d,]+)").unwrap());
static LIMIT_DAYS: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)(?:notified|submitted|reported|tell|notify|made)[^.\n]{0,30}within (\d+) days")
        .unwrap()
});
static AIRLINE_REF: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)reference:?\s*([A-Z]{2}-[A-Z]{2,4}-\d{4,}|[A-Z0-9]{2,4}-[A-Z0-9-]{5,})")
        .unwrap()
});
static EMERGENCY: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)emergency assistance[^+\n]{0,40}(\+\d[\d \-]{7,16}\d)").unwrap()
});

fn minutes_of(t: &str) -> Option<i64> {
    let (h, m) = t.split_once(':')?;
    Some(h.parse::<i64>().ok()? * 60 + m.parse::<i64>().ok()?)
}

/// "7 h 15 min"
pub fn hours_text(minutes: i64) -> String {
    format!("{} h {:02} min", minutes / 60, minutes % 60)
}

/// The delay claim's facts: the policy from the insurer's email, the delay from the airline's certificate (scheduled
/// vs actual departure), the benefit by the policy's own rule (amount per block of hours, the cap), the amount by
/// arithmetic. Regexes only.
pub fn delay_claim(
    s: &ClaimSpec,
    msgs: &[Msg],
    user_texts: &[String],
    today: NaiveDate,
) -> DelayClaim {
    let mut d = DelayClaim {
        per_block: s.delay.0,
        block_hours: s.delay.1,
        cap: s.delay.2,
        limit_days: s.notify_days,
        ..Default::default()
    };
    let words = user_texts.join("\n");
    let is_policy = |m: &Msg| {
        Regex::new(r"(?i)fwd|bolttech|policy")
            .unwrap()
            .is_match(&format!("{} {}", m.from, m.subject))
    };
    let policy = msgs
        .iter()
        .find(|m| is_policy(m) && POLICY_NO.is_match(&m.text()));
    let cert = msgs.iter().find(|m| {
        !is_policy(m)
            && Regex::new(r"(?i)delay|irregularity")
                .unwrap()
                .is_match(&m.text())
    });
    if let Some(p) = policy {
        let txt = p.text();
        d.policy_no = POLICY_NO
            .captures(&txt)
            .map(|c| c[1].to_string())
            .unwrap_or_default();
        d.plan = PLAN_NAME
            .captures(&txt)
            .and_then(|c| c.get(1).or(c.get(2)).map(|m| m.as_str().to_string()))
            .unwrap_or_default();
        if let Some(c) = PER_BLOCK.captures(&txt) {
            d.per_block = c[1].parse().unwrap_or(d.per_block);
            d.block_hours = c[2].parse().unwrap_or(d.block_hours);
        }
        if let Some(c) = CAP.captures(&txt) {
            d.cap = c[1].replace(',', "").parse().unwrap_or(d.cap);
        }
        if let Some(c) = LIMIT_DAYS.captures(&txt) {
            d.limit_days = c[1].parse().unwrap_or(d.limit_days);
            d.limit_from_policy = true;
        }
        d.emergency = EMERGENCY
            .captures(&txt)
            .map(|c| c[1].trim().to_string())
            .unwrap_or_default();
    }
    let cert_text = cert.map(Msg::text).unwrap_or_default();
    for text in [cert_text.as_str(), words.as_str()] {
        if d.minutes > 0 {
            break;
        }
        if let Some(c) = TIMES.iter().find_map(|r| r.captures(text)) {
            if let (Some(a), Some(b)) = (minutes_of(&c[1]), minutes_of(&c[2])) {
                d.scheduled = c[1].to_string();
                d.actual = c[2].to_string();
                d.minutes = if b >= a { b - a } else { b + 24 * 60 - a };
            }
        }
    }
    d.flight_no = flight_no_in(&cert_text)
        .or_else(|| flight_no_in(&words))
        .unwrap_or_default();
    if let Some(c) = ROUTE
        .captures(&cert_text)
        .or_else(|| ROUTE_TO.captures(&cert_text))
    {
        d.from = c[1].trim().to_string();
        d.to = c[2].trim().to_string();
    }
    if let Some(c) = cert {
        d.airline = c.sender();
        d.airline_ref = AIRLINE_REF
            .captures(&c.text())
            .map(|x| x[1].to_string())
            .unwrap_or_default();
        d.attachment = c
            .attachments
            .iter()
            .find(|a| a.to_lowercase().ends_with(".pdf"))
            .map(|a| (c.id.clone(), a.clone()));
        d.reason = DELAY_REASON
            .captures(&c.body)
            .map(|x| x[1].trim().trim_end_matches(',').to_string())
            .unwrap_or_default();
    }
    d.incident_date = past_date_in(user_texts, today)
        .or_else(|| cert.and_then(|c| dates_in(&c.text()).first().map(|(_, x)| *x)))
        .map(iso)
        .unwrap_or_default();
    d.amount = delay_amount(&d);
    d
}

/// The delay benefit: the amount per full block of hours, within the cap.
pub fn delay_amount(d: &DelayClaim) -> f64 {
    if d.block_hours <= 0 {
        return 0.0;
    }
    let blocks = d.minutes / (d.block_hours * 60);
    (blocks as f64 * d.per_block).min(d.cap)
}

impl DelayClaim {
    fn blocks(&self) -> i64 {
        if self.block_hours > 0 {
            self.minutes / (self.block_hours * 60)
        } else {
            0
        }
    }
    fn route(&self) -> String {
        if self.from.is_empty() {
            String::new()
        } else {
            format!(" {}–{}", self.from, self.to)
        }
    }
    /// What happened, in one sentence for the form.
    fn what(&self) -> String {
        let why = if self.reason.is_empty() {
            String::new()
        } else {
            format!(" ({})", self.reason)
        };
        format!(
            "{}{} on {} was scheduled to depart at {} and departed at {}: delayed {}{why}.",
            self.flight_no,
            self.route(),
            self.incident_date,
            self.scheduled,
            self.actual,
            hours_text(self.minutes)
        )
    }
    fn deadline(&self) -> Option<NaiveDate> {
        parse_iso(&self.incident_date).map(|d| d + Duration::days(self.limit_days))
    }
    fn args(&self, currency: &str) -> Value {
        json!({
            "policy_no": self.policy_no, "claim_type": "Travel delay", "incident_date": self.incident_date,
            "what": self.what(), "amount": if self.amount > 0.0 { format!("{:.2}", self.amount) } else { String::new() },
            "flight_no": self.flight_no, "from": self.from, "to": self.to, "scheduled": self.scheduled,
            "actual": self.actual, "hours": hours_text(self.minutes), "minutes": self.minutes,
            "airline": self.airline, "airline_ref": self.airline_ref, "currency": currency,
        })
    }
    fn keep_from(&mut self, a: &Value) {
        let s = |k: &str| sget(a, k).trim().to_string();
        if self.policy_no.is_empty() {
            self.policy_no = s("policy_no");
        }
        if self.incident_date.is_empty() {
            self.incident_date = s("incident_date");
        }
        if self.minutes == 0 {
            self.minutes = a.get("minutes").and_then(Value::as_i64).unwrap_or(0);
            self.scheduled = s("scheduled");
            self.actual = s("actual");
            self.amount = delay_amount(self);
        }
        if self.flight_no.is_empty() {
            self.flight_no = s("flight_no");
            self.from = s("from");
            self.to = s("to");
            self.airline = s("airline");
            self.airline_ref = s("airline_ref");
        }
    }
}

// ---------------------------------------------------------------- the FWD purchase: facts and the plan rule

/// The trip to insure and the user's rule for the plan.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TripQuote {
    /// the country (the destination city's, else the city as written)
    pub destination: String,
    pub depart: String,
    pub ret: String,
    pub travellers: usize,
    /// (the cover the user named — medical | cancellation | baggage | delay, its minimum)
    pub minimum: Option<(String, f64)>,
}

static MINIMUM: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)at least\s+(?:SGD|S\$|\$)?\s?([\d,]+)(?:\s*(?:SGD|S\$))?\s*(?:of\s+)?(?:(?:trip|overseas|travel)\s+)?(cancellation|medical|baggage|delay)").unwrap()
});
static DEST_WORDS: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\b(?i:trip|travel(?:ling)?|holiday|flying|going|fly)\s+(?i:to)\s+([A-Z][a-z]+(?: [A-Z][a-z]+)?)")
        .unwrap()
});
static SUBJECT_TO: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\bto ([A-Z][a-z]+(?: [A-Z][a-z]+)?)").unwrap());

/// The trip from the booking and the request: the destination's country, the dates (the booking's future dates in
/// order), the travellers (one unless the request says more), the user's minimum cover.
pub fn trip_quote(msgs: &[Msg], user_texts: &[String], today: NaiveDate) -> TripQuote {
    let words = user_texts.join("\n");
    let mut q = TripQuote {
        travellers: travellers_in(user_texts),
        ..Default::default()
    };
    let city = DEST_WORDS
        .captures(&words)
        .map(|c| c[1].to_string())
        .or_else(|| {
            msgs.iter()
                .find_map(|m| SUBJECT_TO.captures(&m.subject).map(|c| c[1].to_string()))
        })
        .unwrap_or_default();
    // the city's country ("Tokyo" → Japan; "Hong Kong" as two words first, then the first word alone)
    let first = city.split(' ').next().unwrap_or("").to_string();
    let country = match country_of_city(&city) {
        "" => country_of_city(&first),
        c => c,
    };
    q.destination = if !country.is_empty() {
        country.to_string()
    } else if city.is_empty() {
        city
    } else {
        first
    };
    let mut ds: Vec<NaiveDate> = msgs
        .iter()
        .flat_map(|m| dates_in(&m.text()))
        .chain(user_texts.iter().flat_map(|t| dates_in(t)))
        .map(|(_, d)| d)
        .filter(|d| *d > today)
        .collect();
    ds.sort();
    ds.dedup();
    if let Some(d) = ds.first() {
        q.depart = iso(*d);
    }
    if let Some(d) = ds.get(1) {
        q.ret = iso(*d);
    }
    if let Some(c) = MINIMUM.captures(&words) {
        if let Ok(v) = c[1].replace(',', "").parse::<f64>() {
            q.minimum = Some((c[2].to_lowercase(), v));
        }
    }
    q
}

impl TripQuote {
    fn complete(&self) -> bool {
        !self.destination.is_empty() && !self.depart.is_empty() && !self.ret.is_empty()
    }
    fn args(&self) -> Value {
        json!({"destination": self.destination, "depart": self.depart, "return": self.ret,
               "travellers": self.travellers.to_string()})
    }
    fn keep_from(&mut self, a: &Value) {
        let s = |k: &str| sget(a, k).trim().to_string();
        if self.destination.is_empty() {
            self.destination = s("destination");
        }
        if self.depart.is_empty() {
            self.depart = s("depart");
            self.ret = s("return");
        }
    }
}

/// One plan the quote page lists.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Plan {
    pub name: String,
    /// the ref of its "Select" link
    pub link: String,
    pub premium: f64,
    /// (cover, amount) in the page's column order
    pub cover: Vec<(String, f64)>,
}

impl Plan {
    fn cover_of(&self, col: &str) -> Option<f64> {
        self.cover.iter().find(|(c, _)| c == col).map(|(_, v)| *v)
    }
    fn summary(&self, cur: &str) -> String {
        let covers: Vec<String> = self
            .cover
            .iter()
            .filter(|(c, _)| c != "premium")
            .map(|(c, v)| format!("{c} {cur} {}", money(*v).trim_end_matches(".00")))
            .collect();
        format!(
            "{} — {cur} {}{}",
            self.name,
            money(self.premium),
            if covers.is_empty() {
                String::new()
            } else {
                format!(" ({})", covers.join(", "))
            }
        )
    }
}

static SELECT_LINK: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"^\[(e\d+)\] link "Select ([^"]+)""#).unwrap());
static DELAY_PHRASE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)(?:SGD|HKD|S\$|HK\$)\s?\d+ (?:for )?every \d+ hours,? up to ").unwrap()
});

/// The plans on the quote page's snapshot: a table whose rows end in a "Select <plan>" link; the columns from the
/// header row (premium, medical, cancellation, delay, baggage — whichever the page names, in its order).
pub fn plans_in(snapshot: &str) -> Vec<Plan> {
    const COLS: &[(&str, &str)] = &[
        ("premium", r"(?i)\bpremium\b|\bprice\b"),
        ("medical", r"(?i)medical"),
        ("cancellation", r"(?i)cancell"),
        ("delay", r"(?i)delay"),
        ("baggage", r"(?i)baggage|luggage"),
    ];
    let default: Vec<String> = COLS.iter().map(|(c, _)| c.to_string()).collect();
    let mut cols: Vec<String> = Vec::new();
    let mut last_text = String::new();
    let mut out = Vec::new();
    for raw in snapshot.lines() {
        let line = raw.trim();
        if let Some(c) = SELECT_LINK.captures(line) {
            let row = DELAY_PHRASE.replace_all(&last_text, "");
            let amounts = amounts_in(&row);
            if amounts.is_empty() {
                continue;
            }
            let names = if cols.is_empty() { &default } else { &cols };
            let cover: Vec<(String, f64)> =
                names.iter().cloned().zip(amounts.iter().cloned()).collect();
            let premium = cover
                .iter()
                .find(|(c, _)| c == "premium")
                .map(|(_, v)| *v)
                .unwrap_or(amounts[0]);
            out.push(Plan {
                name: c[2].trim().to_string(),
                link: c[1].to_string(),
                premium,
                cover,
            });
            continue;
        }
        if line.starts_with('[') {
            continue;
        }
        if out.is_empty() && cols.is_empty() {
            let lower = line.to_lowercase();
            if lower.contains("plan") && lower.contains("premium") && amounts_in(line).is_empty() {
                let mut found: Vec<(usize, String)> = COLS
                    .iter()
                    .filter_map(|(c, r)| {
                        Regex::new(r)
                            .ok()?
                            .find(line)
                            .map(|m| (m.start(), c.to_string()))
                    })
                    .collect();
                found.sort();
                cols = found.into_iter().map(|(_, c)| c).collect();
            }
        }
        last_text = line.to_string();
    }
    out
}

/// The plan by the user's rule: the cheapest plan whose named cover is at least the minimum, else the cheapest. The
/// second value says which rule picked it.
pub fn pick_plan<'a>(
    plans: &'a [Plan],
    minimum: &Option<(String, f64)>,
) -> Option<(&'a Plan, String)> {
    let cheapest = |ps: &[&'a Plan]| {
        ps.iter().cloned().min_by(|a, b| {
            a.premium
                .partial_cmp(&b.premium)
                .unwrap_or(std::cmp::Ordering::Equal)
        })
    };
    if let Some((col, min)) = minimum {
        let ok: Vec<&Plan> = plans
            .iter()
            .filter(|p| p.cover_of(col).is_some_and(|v| v >= *min))
            .collect();
        if let Some(p) = cheapest(&ok) {
            return Some((
                p,
                format!(
                    "the cheapest plan with {col} cover of at least {}",
                    money(*min).trim_end_matches(".00")
                ),
            ));
        }
        let all: Vec<&Plan> = plans.iter().collect();
        return cheapest(&all).map(|p| {
            (
                p,
                format!(
                    "no plan has {col} cover of at least {}: the cheapest plan instead",
                    money(*min).trim_end_matches(".00")
                ),
            )
        });
    }
    let all: Vec<&Plan> = plans.iter().collect();
    cheapest(&all).map(|p| (p, "the cheapest plan".to_string()))
}

/// A card of the Vault, as `vault_list` lists it.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct VaultCard {
    pub id: String,
    pub label: String,
    /// added by its name only: number, expiry and CVC UNKNOWN
    pub name_only: bool,
}

static VAULT_LINE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?m)^- id=(\S+) · (.+?) · card · ").unwrap());

/// The cards in a `vault_list` result.
pub fn vault_cards(text: &str) -> Vec<VaultCard> {
    text.lines()
        .filter_map(|l| {
            let c = VAULT_LINE.captures(l)?;
            let label = c[2]
                .split(" · issued in")
                .next()
                .unwrap_or("")
                .trim()
                .to_string();
            Some(VaultCard {
                id: c[1].to_string(),
                label,
                name_only: l.contains("UNKNOWN"),
            })
        })
        .collect()
}

/// The card the user named (a word of its label in their words), else the first with its details, else the first.
pub fn pick_card<'a>(cards: &'a [VaultCard], user_texts: &[String]) -> Option<&'a VaultCard> {
    let words = user_texts.join(" ").to_lowercase();
    cards
        .iter()
        .find(|c| {
            c.label
                .split_whitespace()
                .filter(|w| {
                    w.len() >= 3
                        && !matches!(
                            w.to_lowercase().as_str(),
                            "visa" | "card" | "mastercard" | "the"
                        )
                })
                .any(|w| words.contains(&w.to_lowercase()))
        })
        .or_else(|| cards.iter().find(|c| !c.name_only))
        .or_else(|| cards.first())
}

/// The ref of the input labelled `label` (a textbox or a file chooser) in a snapshot.
fn ref_of(snapshot: &str, role: &str, label: &str) -> Option<String> {
    let r = Regex::new(&format!(r#"\[(e\d+)\] {role} "{}"#, regex::escape(label))).ok()?;
    r.captures(snapshot).map(|c| c[1].to_string())
}

// ---------------------------------------------------------------- the transcript's gates (fill, review, snapshot)

/// What a fill stage reports: the step to run, or the position of the fill that went through.
enum Gate {
    Step(Step),
    Done(usize),
}

const OWN_STEP: &str = "This step is yours";

/// The fill of one form (`stage`), a user's own step the function stopped at handed over and the fill run again
/// (never more than twice), as in `driver::next`.
fn fill_gate(
    transcript: &[Value],
    n: usize,
    stage: &str,
    mut args: Value,
    name: &str,
    zh: bool,
) -> Gate {
    args["stage"] = json!(stage);
    let fills: Vec<(Value, &Value)> = results_of(transcript, "form_fill")
        .into_iter()
        .filter(|(a, _)| sget(a, "stage") == stage)
        .collect();
    let takeovers = results_of(transcript, "browser_request_takeover");
    let own_backs: Vec<usize> = takeovers
        .iter()
        .filter(|(a, m)| sget(a, "reason").starts_with(OWN_STEP) && handed_back(m))
        .map(|(_, m)| pos(transcript, m))
        .collect();
    let Some((_, last)) = fills.last() else {
        return Gate::Step(Step::Calls(vec![call(n, "form_fill", args)]));
    };
    let fill_pos = pos(transcript, last);
    let Some(why) = stopped(last) else {
        return Gate::Done(fill_pos);
    };
    if own_backs.iter().any(|&p| p > fill_pos) {
        return Gate::Step(Step::Calls(vec![call(n, "form_fill", args)]));
    }
    if takeovers.iter().any(|(_, m)| pos(transcript, m) > fill_pos) {
        return Gate::Step(Step::Final(String::new()));
    }
    let step = why.split(" is the user's").next().unwrap_or(&why).trim();
    let step = step.strip_prefix("the ").unwrap_or(step).to_string();
    if own_backs.len() >= 2 {
        return Gate::Step(Step::Final(if zh {
            format!("你的{name}还停在需要你本人完成的一步：{step}。请到浏览器标签页完成它，再让我继续；目前什么都没有提交。")
        } else {
            format!("Your {name} is waiting on a step only you can do: {step}. Finish it in the browser tab, then ask me to continue; nothing has been submitted.")
        }));
    }
    Gate::Step(Step::Calls(vec![call(
        n,
        "browser_request_takeover",
        json!({"reason": format!("{OWN_STEP}: {step}. Do it on the page, then hand the browser back and I will fill in the rest of the form.")}),
    )]))
}

/// The review hand-over after position `after`: the step to run, or the position of the hand-back.
fn review_gate(
    transcript: &[Value],
    n: usize,
    after: usize,
    reason: String,
) -> Result<usize, Step> {
    let takeovers = results_of(transcript, "browser_request_takeover");
    let review: Vec<&(Value, &Value)> = takeovers
        .iter()
        .filter(|(a, m)| pos(transcript, m) > after && !sget(a, "reason").starts_with(OWN_STEP))
        .collect();
    match review.last() {
        None => Err(Step::Calls(vec![call(
            n,
            "browser_request_takeover",
            json!({"reason": reason}),
        )])),
        Some((_, m)) if handed_back(m) => Ok(pos(transcript, m)),
        Some(_) => Err(Step::Final(String::new())),
    }
}

/// The last snapshot after position `after`: its text, or the step that takes one.
fn snapshot_after(transcript: &[Value], n: usize, after: usize) -> Result<String, Step> {
    results_of(transcript, "browser_snapshot")
        .last()
        .filter(|(_, m)| pos(transcript, m) > after)
        .map(|(_, m)| snapshot_text(m))
        .ok_or_else(|| {
            Step::Calls(vec![call(
                n,
                "browser_snapshot",
                json!({"max_chars": 6000}),
            )])
        })
}

fn opened(transcript: &[Value], host: &str, path: &str) -> bool {
    results_of(transcript, "browser_navigate")
        .iter()
        .any(|(a, _)| {
            let u = sget(a, "url");
            u.contains(host) && u.contains(path)
        })
}

fn reference_in(page: &str, ref_re: &str) -> Option<String> {
    if ref_re.is_empty() {
        return None;
    }
    Regex::new(ref_re)
        .ok()
        .and_then(|r| r.find(page).map(|m| m.as_str().to_string()))
        .or_else(|| crate::outcome::refs_in(page).into_iter().next())
}

fn cron_at(d: NaiveDate) -> String {
    format!("0 8 {} {} *", d.day(), d.month())
}

fn step_count(transcript: &[Value]) -> usize {
    transcript
        .iter()
        .flat_map(tool_calls)
        .filter(|tc| get_str(tc, "id").starts_with("clm_"))
        .count()
        + 1
}

/// The facts the extraction call returned, if it ran.
fn extracted(transcript: &[Value]) -> Option<Value> {
    results_of(transcript, "claim_facts")
        .last()
        .map(|(_, m)| json_of(m))
}

fn extract_text(msgs: &[Msg], user_texts: &[String]) -> String {
    let joined: String = msgs
        .iter()
        .map(|m| format!("From: {}\nSubject: {}\n{}", m.from, m.subject, m.body))
        .chain(user_texts.iter().map(|u| format!("User: {u}")))
        .collect::<Vec<_>>()
        .join("\n\n");
    take_chars(&joined, 12000).to_string()
}

/// The next step of the run.
pub fn next(s: &ClaimSpec, transcript: &[Value], w: &World) -> Step {
    let n = step_count(transcript);
    match s.flow {
        Flow::CsrTrip => next_csr(s, transcript, w, n),
        Flow::FwdSgClaim => next_sg_claim(s, transcript, w, n),
        Flow::FwdSgBuy => next_sg_buy(s, transcript, w, n),
        Flow::FwdHkClaim => next_hk_claim(s, transcript, w, n),
    }
}

const ASK_LOSS: &str = "which day the trip was cancelled or cut short";
const ASK_TRIP: &str = "the departure and return dates";
const ASK_DELAY: &str = "the scheduled and the actual departure time";

// ---------------------------------------------------------------- the CSR trip claim

fn next_csr(s: &ClaimSpec, transcript: &[Value], w: &World, n: usize) -> Step {
    if let Some(step) = gather(s, transcript, w, n, "") {
        return step;
    }
    let msgs = fetched(transcript);
    let mut t = trip_claim(&msgs, w.user_texts, w.today);
    if let Some((a, _)) = results_of(transcript, "form_fill").last() {
        t.keep_from(a);
    }
    if let Some(v) = extracted(transcript) {
        if t.loss_date.is_empty() {
            t.loss_date = get_str(&v, "loss_date");
        }
        if t.amount == 0.0 {
            let f = |k: &str| {
                get_str(&v, k)
                    .replace(',', "")
                    .parse::<f64>()
                    .unwrap_or(0.0)
            };
            t.paid = f("paid");
            t.refunded = f("refunded");
            t.change_fee = f("change_fee");
            t.amount = claim_amount(&t, s);
        }
    }
    let missing = t.loss_date.is_empty() || t.amount == 0.0;
    if missing && (!msgs.is_empty() || w.user_texts.len() > 1) && extracted(transcript).is_none() {
        return Step::Extract(extract_text(&msgs, w.user_texts));
    }
    if missing && !asked_before(transcript, ASK_LOSS) && !asked_before(transcript, "哪一天") {
        let mut lines = Vec::new();
        if t.loss_date.is_empty() {
            lines.push(format!("- {ASK_LOSS} (the date)"));
        }
        if t.amount == 0.0 {
            lines.push("- what was paid with the card for the trip, and what the airline or hotel refunded (USD)".to_string());
        }
        let mut q = format!(
            "To file your {} claim I still need:\n{}",
            t.benefit,
            lines.join("\n")
        );
        if !w.mail_ready {
            q.push_str("\n\n(Connect your mailbox in Connections and I can read the booking, the charge and the refund from your emails myself.)");
        }
        return Step::Final(q);
    }
    if !opened(transcript, s.host, "/") {
        return Step::Calls(vec![call(
            n,
            "browser_navigate",
            json!({"url": s.start_url}),
        )]);
    }
    let fill_pos = match fill_gate(transcript, n, "claim", t.args(), s.name, w.zh) {
        Gate::Step(st) => return st,
        Gate::Done(p) => p,
    };
    let reason = format!(
        "The {} claim is filled in on eClaimsLine from your {}profile and the card's charge: date of loss {}, amount USD {} ({}), reason \"{}\". Review it, then {} yourself; hand the browser back when the confirmation shows (or without submitting).",
        t.benefit,
        if msgs.is_empty() { "" } else { "bookings, the refund emails, " },
        if t.loss_date.is_empty() { "to fill in" } else { &t.loss_date },
        money(t.amount),
        t.arithmetic(),
        t.reason,
        s.user_steps
    );
    let back = match review_gate(transcript, n, fill_pos, reason) {
        Ok(p) => p,
        Err(st) => return st,
    };
    let page = match snapshot_after(transcript, n, back) {
        Ok(p) => p,
        Err(st) => return st,
    };
    let reference = reference_in(&page, s.ref_re);
    // the reminders: 7 days before the documents deadline, and a mail check every 10 days
    if let (Some(loss), true) = (
        parse_iso(&t.loss_date),
        results_of(transcript, "schedule_create").is_empty(),
    ) {
        let docs = loss + Duration::days(s.documents_days);
        let claim_no = reference.clone().unwrap_or_else(|| "number pending".into());
        let mut calls = vec![call(
            n,
            "schedule_create",
            json!({"name": format!("{} documents due {}", t.benefit, iso(docs)),
                   "goal": format!("Remind me that the documents for my Chase Sapphire {} claim (loss date {}, claim {claim_no}) are due on eClaimsLine by {}: the physician statement, the itinerary, the card statement showing the charge and the supplier's refund statement. Check whether they were uploaded and notify me.", t.benefit, t.loss_date, long(docs)),
                   "kind": "cron", "spec": cron_at(docs - Duration::days(7))}),
        )];
        if w.today <= docs {
            calls.push(call(
                n + 1,
                "schedule_create",
                json!({"name": format!("{} claim check", t.benefit),
                       "goal": format!("Search my mail (gmail_search) for any email from Card Benefit Services, eClaimsLine or Chase about my {} claim {claim_no} (loss date {}) and notify me of news; stop when the claim is paid or closed.", t.benefit, t.loss_date),
                       "kind": "interval", "spec": "14400"}),
            ));
        }
        return Step::Calls(calls);
    }
    Step::Final(answer_csr(s, &t, reference.as_deref(), w))
}

/// The CSR claim's answer: the benefit and the two dates first, the amount and its arithmetic, the status, the
/// documents, the reminders, the limits.
pub fn answer_csr(s: &ClaimSpec, t: &TripClaim, reference: Option<&str>, w: &World) -> String {
    let dates = match parse_iso(&t.loss_date) {
        Some(loss) => {
            let d20 = loss + Duration::days(s.notify_days);
            let d90 = loss + Duration::days(s.documents_days);
            let late = if w.today > d20 {
                if w.zh {
                    format!(
                        "（{}天通知期已于 {} 过去：今天就立案，并在理赔中说明。）",
                        s.notify_days,
                        long(d20)
                    )
                } else {
                    format!(" The {}-day notice window closed on {}: file today and say so in the claim.", s.notify_days, long(d20))
                }
            } else {
                String::new()
            };
            if w.zh {
                format!("通知理赔管理方截止 **{}**（损失日 {} + {} 天）；提交材料截止 **{}**（+ {} 天）。{late}", long(d20), long(loss), s.notify_days, long(d90), s.documents_days)
            } else {
                format!("notify the administrator by **{}** (loss date {} + {} days); documents by **{}** (+ {} days).{late}", long(d20), long(loss), s.notify_days, long(d90), s.documents_days)
            }
        }
        None => {
            if w.zh {
                "损失日期未知：请告诉我哪一天没去成/中断，我再算 20 天和 90 天的期限。".into()
            } else {
                "the loss date is not known yet: tell me the day the trip was cancelled or cut short and I will give the 20-day and 90-day dates.".into()
            }
        }
    };
    let status = match (reference, w.zh) {
        (Some(r), false) => format!("You submitted it on eClaimsLine: claim number **{r}**."),
        (Some(r), true) => format!("你已在 eClaimsLine 提交：理赔编号 **{r}**。"),
        (None, false) => "The claim is filled in on eClaimsLine and waiting for your Submit — nothing has been submitted for you.".into(),
        (None, true) => "理赔表已在 eClaimsLine 填好，等你按 Submit——没有替你提交任何内容。".into(),
    };
    let doctor = if t.has_doctor_letter {
        "your doctor's letter (have — upload it; the administrator's physician statement form may follow)"
    } else {
        "the physician statement, signed by the treating doctor (to get)"
    };
    let supplier = if t.has_supplier_letter {
        "have, from the refund email"
    } else {
        "ask the airline or hotel for it in writing"
    };
    let reminders = match parse_iso(&t.loss_date) {
        Some(loss) => format!(
            "Reminders set: {} (7 days before the documents deadline) and a mail check every 10 days until the claim is paid.",
            long(loss + Duration::days(s.documents_days - 7))
        ),
        None => "No reminders yet (the loss date is missing).".into(),
    };
    if w.zh {
        return format!(
            "**{}**（{}）：{dates}\n\n- 索赔金额：**USD {}** = {}。\n- 理赔原因：{}（不写诊断）。行程：{}；供应商和订单号：{}。\n- {status}\n- 需要的材料：{}；行程单/电子机票；信用卡账单（显示该笔消费）；航空公司/酒店的书面不可退款说明（{}）。\n- {reminders}\n\n由 {} 决定是否赔付；不保的情形：订票前 60 天内治疗过的既往病症、自行改变计划、违背医嘱出行、超过 60 天的行程。限额：每位旅客 USD 10,000、每次行程 20,000、每 12 个月 40,000。",
            t.benefit, t.card_product, money(t.amount), t.arithmetic(), t.reason, t.trip, t.booking(), doctor, supplier, s.decides
        );
    }
    format!(
        "**{}** claim on your {} — {dates}\n\n- Amount to claim: **USD {}** = {}.\n- Reason filed: {} (no diagnosis). Trip: {}; supplier and booking reference: {}.\n- {status}\n- Documents: {}; the e-ticket / itinerary; the card statement showing the charge; the supplier's written refund / non-refundable statement ({}).\n- {reminders}\n\n{} decides the claim. Excluded: pre-existing conditions (treated in the 60 days before the booking), a change of plans, travel against medical advice and trips longer than 60 days. Limits: USD 10,000 per traveller, 20,000 per trip, 40,000 per 12 months.",
        t.benefit, t.card_product, money(t.amount), t.arithmetic(), t.reason,
        if t.trip.is_empty() { "—" } else { &t.trip },
        if t.suppliers.is_empty() { "—".to_string() } else { t.booking() },
        doctor, supplier, s.decides
    )
}

// ---------------------------------------------------------------- the FWD Singapore delay claim

/// The path a `gmail_save_attachment` result names.
fn saved_path(transcript: &[Value]) -> Option<String> {
    static SAVED: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"saved: (\S+)").unwrap());
    results_of(transcript, "gmail_save_attachment")
        .iter()
        .find_map(|(_, m)| SAVED.captures(&content_of(m)).map(|c| c[1].to_string()))
}

fn delay_facts(s: &ClaimSpec, transcript: &[Value], w: &World) -> (Vec<Msg>, DelayClaim) {
    let msgs = fetched(transcript);
    let mut d = delay_claim(s, &msgs, w.user_texts, w.today);
    let fill_args = results_of(transcript, "form_fill")
        .last()
        .map(|(a, _)| a.clone())
        .or_else(|| {
            results_of(transcript, "pdf_form_fill")
                .last()
                .map(|(a, _)| a["values"].clone())
        });
    if let Some(a) = fill_args {
        d.keep_from(&a);
    }
    if let Some(v) = extracted(transcript) {
        if d.minutes == 0 {
            if let (Some(a), Some(b)) = (
                minutes_of(&get_str(&v, "scheduled_departure")),
                minutes_of(&get_str(&v, "actual_departure")),
            ) {
                d.scheduled = get_str(&v, "scheduled_departure");
                d.actual = get_str(&v, "actual_departure");
                d.minutes = if b >= a { b - a } else { b + 24 * 60 - a };
                d.amount = delay_amount(&d);
            }
        }
        if d.incident_date.is_empty() {
            d.incident_date = get_str(&v, "incident_date");
        }
        if d.policy_no.is_empty() {
            d.policy_no = get_str(&v, "policy_no");
        }
    }
    (msgs, d)
}

/// The delay's missing facts: the extraction call once, then the question once; `None` when the run can go on.
fn delay_missing(transcript: &[Value], w: &World, msgs: &[Msg], d: &DelayClaim) -> Option<Step> {
    let missing = d.minutes == 0 || d.incident_date.is_empty() || d.policy_no.is_empty();
    if !missing {
        return None;
    }
    if (!msgs.is_empty() || w.user_texts.len() > 1) && extracted(transcript).is_none() {
        return Some(Step::Extract(extract_text(msgs, w.user_texts)));
    }
    if asked_before(transcript, ASK_DELAY) || asked_before(transcript, "原定") {
        return None;
    }
    let mut lines = Vec::new();
    if d.policy_no.is_empty() {
        lines.push("- your FWD policy number (it is in the policy email)");
    }
    if d.incident_date.is_empty() {
        lines.push("- the date of the delayed flight");
    }
    if d.minutes == 0 {
        lines.push("- the scheduled and the actual departure time (the airline's delay certificate states them)");
    }
    let mut q = format!(
        "To claim the travel delay I still need:\n{}",
        lines.join("\n")
    );
    if !w.mail_ready {
        q.push_str("\n\n(Connect your mailbox in Connections and I can read the policy and the delay certificate from your emails myself.)");
    }
    Some(Step::Final(q))
}

fn next_sg_claim(s: &ClaimSpec, transcript: &[Value], w: &World, n: usize) -> Step {
    let flight = w
        .user_texts
        .iter()
        .find_map(|t| flight_no_in(t))
        .unwrap_or_default();
    if let Some(step) = gather(s, transcript, w, n, &flight) {
        return step;
    }
    let (msgs, d) = delay_facts(s, transcript, w);
    if let Some(step) = delay_missing(transcript, w, &msgs, &d) {
        return step;
    }
    if d.minutes > 0 && d.amount == 0.0 {
        return Step::Final(answer_no_benefit(s, &d, w.zh));
    }
    // the certificate, saved for the upload
    if let Some((id, name)) = &d.attachment {
        if results_of(transcript, "gmail_save_attachment").is_empty() {
            return Step::Calls(vec![call(
                n,
                "gmail_save_attachment",
                json!({"message_id": id, "filename": name}),
            )]);
        }
    }
    if !opened(transcript, s.host, "/claim-online/") {
        return Step::Calls(vec![call(
            n,
            "browser_navigate",
            json!({"url": s.start_url}),
        )]);
    }
    let fill_pos = match fill_gate(transcript, n, "claim", d.args(s.currency), s.name, w.zh) {
        Gate::Step(st) => return st,
        Gate::Done(p) => p,
    };
    // the certificate uploaded into the form's file input (its ref from a snapshot after the fill)
    let uploaded = results_of(transcript, "browser_upload")
        .last()
        .is_some_and(|(_, m)| pos(transcript, m) > fill_pos);
    if let (Some(path), false) = (saved_path(transcript), uploaded) {
        let snap = results_of(transcript, "browser_snapshot")
            .last()
            .filter(|(_, m)| pos(transcript, m) > fill_pos)
            .map(|(_, m)| snapshot_text(m));
        match snap {
            None => {
                return Step::Calls(vec![call(
                    n,
                    "browser_snapshot",
                    json!({"max_chars": 6000}),
                )])
            }
            Some(text) => {
                if let Some(r) =
                    ref_of(&text, "filechooser", "Supporting documents").or_else(|| {
                        Regex::new(r"\[(e\d+)\] filechooser")
                            .unwrap()
                            .captures(&text)
                            .map(|c| c[1].to_string())
                    })
                {
                    return Step::Calls(vec![call(
                        n,
                        "browser_upload",
                        json!({"ref": r, "path": path}),
                    )]);
                }
            }
        }
    }
    let after = results_of(transcript, "browser_upload")
        .last()
        .map(|(_, m)| pos(transcript, m))
        .unwrap_or(fill_pos)
        .max(fill_pos);
    let reason = format!(
        "The travel delay claim is filled in on FWD's form from your policy email, {}'s delay certificate, your profile and your passport (Vault): policy {}, {} on {} delayed {} ({} × {} h → {} {}){}. Check it, then {} yourself; hand the browser back when the confirmation shows (or without submitting).",
        if d.airline.is_empty() { "the airline" } else { &d.airline },
        d.policy_no, d.flight_no, d.incident_date, hours_text(d.minutes), d.blocks(), d.block_hours, s.currency, money(d.amount),
        if uploaded || saved_path(transcript).is_none() { "" } else { "; attach the delay certificate from attachments/ if it is not on the form" },
        s.user_steps
    );
    let back = match review_gate(transcript, n, after, reason) {
        Ok(p) => p,
        Err(st) => return st,
    };
    let page = match snapshot_after(transcript, n, back) {
        Ok(p) => p,
        Err(st) => return st,
    };
    let reference = reference_in(&page, s.ref_re);
    if results_of(transcript, "schedule_create").is_empty() {
        let mut calls = vec![call(
            n,
            "schedule_create",
            json!({"name": format!("FWD claim check {}", d.policy_no),
                   "goal": format!("Search my mail (gmail_search) for any email from FWD about my travel delay claim{} on policy {} and notify me of news; stop when the claim is paid or closed.", reference.as_ref().map(|r| format!(" {r}")).unwrap_or_default(), d.policy_no),
                   "kind": "interval", "spec": "10080"}),
        )];
        if let (None, Some(due)) = (&reference, d.deadline()) {
            if w.today <= due - Duration::days(5) {
                calls.push(call(
                    n + 1,
                    "schedule_create",
                    json!({"name": format!("FWD claim due {}", iso(due)),
                           "goal": format!("Remind me that my FWD travel delay claim for {} on {} (policy {}) must be submitted by {} if it has not been submitted yet.", d.flight_no, d.incident_date, d.policy_no, long(due)),
                           "kind": "cron", "spec": cron_at(due - Duration::days(5))}),
                ));
            }
        }
        return Step::Calls(calls);
    }
    Step::Final(answer_delay(s, &d, reference.as_deref(), w, None))
}

fn answer_no_benefit(s: &ClaimSpec, d: &DelayClaim, zh: bool) -> String {
    if zh {
        format!(
            "{} 于 {} 延误 {}，不足保单的 {} 小时起赔区间（每满 {} 小时 {} {}），没有可索赔的延误赔偿；未提交任何内容。",
            d.flight_no, d.incident_date, hours_text(d.minutes), d.block_hours, d.block_hours, s.currency, money(d.per_block)
        )
    } else {
        format!(
            "{} on {} was delayed {}: under the policy's first {}-hour block ({} {} for every {} hours), so no travel delay benefit is due and nothing was filed.",
            d.flight_no, d.incident_date, hours_text(d.minutes), d.block_hours, s.currency, money(d.per_block), d.block_hours
        )
    }
}

/// The delay claim's answer: the channel and the deadline first, the delay and the amount, the status, the documents,
/// the reminders.
pub fn answer_delay(
    s: &ClaimSpec,
    d: &DelayClaim,
    reference: Option<&str>,
    w: &World,
    hk: Option<&HkDone>,
) -> String {
    let deadline = match d.deadline() {
        Some(due) if d.limit_from_policy || s.notify_days > 0 => {
            if w.zh {
                format!(
                    "须在 **{}** 前{}（事故日 {} + {} 天{}）",
                    long(due),
                    if hk.is_some() { "提交" } else { "通知 FWD" },
                    long(parse_iso(&d.incident_date).unwrap()),
                    d.limit_days,
                    if d.limit_from_policy {
                        "，按保单邮件"
                    } else {
                        ""
                    }
                )
            } else {
                format!(
                    "{} by **{}** (incident {} + {} days{})",
                    if hk.is_some() {
                        "claims within"
                    } else {
                        "tell FWD"
                    },
                    long(due),
                    long(parse_iso(&d.incident_date).unwrap()),
                    d.limit_days,
                    if d.limit_from_policy {
                        ", from your policy email"
                    } else {
                        ""
                    }
                )
            }
        }
        _ => {
            if w.zh {
                "时限无法从邮件核实：请尽快提交".into()
            } else {
                "the time limit could not be checked in your mail: file as soon as possible".into()
            }
        }
    };
    let delay_line = format!(
        "{}{} on {}, scheduled {}, departed {}: **{}**{}",
        d.flight_no,
        d.route(),
        d.incident_date,
        d.scheduled,
        d.actual,
        hours_text(d.minutes),
        if d.reason.is_empty() {
            String::new()
        } else {
            format!(" ({})", d.reason)
        }
    );
    let amount_line = format!(
        "**{} {}** — {} {} for every {} hours of delay ({} × {} h), up to {} {}, policy {}{}",
        s.currency,
        money(d.amount),
        s.currency,
        money(d.per_block).trim_end_matches(".00"),
        d.block_hours,
        d.blocks(),
        d.block_hours,
        s.currency,
        money(d.cap).trim_end_matches(".00"),
        d.policy_no,
        if d.plan.is_empty() {
            String::new()
        } else {
            format!(" ({})", d.plan)
        }
    );
    let status = match (hk, reference) {
        (Some(h), _) => format!(
            "The claim form is filled in and sent to this chat ({}): policy, your name and address, the flight, the hours delayed, the amount, {}'s reference — **you still add** your HKID card number, the bank account for the payment, your signature and the date.\n- An email draft to {} is in your Drafts{} — **not sent**: sign the form, replace it in the draft and send it.",
            h.form, if d.airline.is_empty() { "the airline" } else { &d.airline }, s.claims_email,
            if h.letter.is_empty() { " with the form attached".to_string() } else { format!(" with the form and {}'s delay letter attached", d.airline) }
        ),
        (None, Some(r)) => format!("You submitted it on FWD's claim form: reference **{r}**."),
        (None, None) => "The claim is filled in on FWD's online form (policy, your details, the passport from the Vault, the incident, the amount, the delay certificate) and waiting for your Submit — nothing has been submitted for you.".into(),
    };
    let reminders = if hk.is_some() || reference.is_some() {
        "Reminder set: a weekly mail check for the insurer's reply.".to_string()
    } else {
        format!(
            "Reminders set: a weekly mail check for FWD's reply{}.",
            d.deadline()
                .map(|due| format!(
                    ", and one on {} if the claim is not submitted by then",
                    long(due - Duration::days(5))
                ))
                .unwrap_or_default()
        )
    };
    let emergency = if d.emergency.is_empty() {
        String::new()
    } else {
        format!(
            " 24-hour emergency assistance: {} (from your policy email).",
            d.emergency
        )
    };
    let who = if hk.is_some() {
        format!("handled by **{}**", s.decides)
    } else {
        format!("{} decides", s.decides)
    };
    if w.zh {
        return format!(
            "**{}** — {}；{deadline}。\n\n- 延误：{delay_line}\n- 金额：{amount_line}\n- {status}\n- 材料：航空公司的延误证明{}；如需要再补登机牌/电子机票。\n- {reminders}\n\n由保险公司决定是否赔付。{emergency}",
            s.name, who, if d.attachment.is_some() { "（已从邮件附件取得）" } else { "（请向航空公司索取）" }
        );
    }
    format!(
        "**{}** — {}; {deadline}.\n\n- Delay: {delay_line}\n- Amount: {amount_line}\n- {status}\n- Documents: the airline's delay certificate{}; the boarding pass or e-ticket if asked.\n- {reminders}\n\nThe insurer decides the claim.{emergency}",
        s.name, who, if d.attachment.is_some() { " (have, from the airline's email)" } else { " (ask the airline for it)" }
    )
}

// ---------------------------------------------------------------- the FWD Hong Kong claim (bolttech's PDF form)

/// What the Hong Kong flow produced, for the answer.
pub struct HkDone {
    /// the filled form's workspace path
    pub form: String,
    /// the airline's letter, saved from mail (empty when none)
    pub letter: String,
}

fn next_hk_claim(s: &ClaimSpec, transcript: &[Value], w: &World, n: usize) -> Step {
    let flight = w
        .user_texts
        .iter()
        .find_map(|t| flight_no_in(t))
        .unwrap_or_default();
    if let Some(step) = gather(s, transcript, w, n, &flight) {
        return step;
    }
    let (msgs, d) = delay_facts(s, transcript, w);
    if let Some(step) = delay_missing(transcript, w, &msgs, &d) {
        return step;
    }
    if d.minutes > 0 && d.amount == 0.0 {
        return Step::Final(answer_no_benefit(s, &d, w.zh));
    }
    if let Some((id, name)) = &d.attachment {
        if results_of(transcript, "gmail_save_attachment").is_empty() {
            return Step::Calls(vec![call(
                n,
                "gmail_save_attachment",
                json!({"message_id": id, "filename": name}),
            )]);
        }
    }
    // the claim form: the PDF the page links; else the link on FWD's site, clicked, and the download
    let navs = results_of(transcript, "browser_navigate");
    let pdf_path = navs
        .iter()
        .chain(results_of(transcript, "browser_click").iter())
        .find_map(|(_, m)| {
            let v = json_of(m);
            let p = get_str(&v["pdf"], "path");
            (!p.is_empty()).then_some(p)
        })
        .or_else(|| {
            results_of(transcript, "browser_downloads")
                .last()
                .and_then(|(_, m)| {
                    Regex::new(r"[\w./-]+\.pdf\b")
                        .unwrap()
                        .find(&content_of(m))
                        .map(|x| x.as_str().to_string())
                })
        });
    let form_path = match pdf_path {
        Some(p) => p,
        None => {
            if !opened(transcript, s.host, "/files/") {
                return Step::Calls(vec![call(
                    n,
                    "browser_navigate",
                    json!({"url": s.start_url}),
                )]);
            }
            if !opened(transcript, s.host, "fwd.com.hk/") || navs.len() < 2 {
                return Step::Calls(vec![call(
                    n,
                    "browser_navigate",
                    json!({"url": "https://www.fwd.com.hk/"}),
                )]);
            }
            let site = navs.last().map(|(_, m)| content_of(m)).unwrap_or_default();
            let link = Regex::new(r#"\[(e\d+)\] link "[^"]*[Cc]laim [Ff]orm[^"]*""#)
                .unwrap()
                .captures(&site)
                .map(|c| c[1].to_string());
            match (
                link,
                results_of(transcript, "browser_click").is_empty(),
                results_of(transcript, "browser_downloads").is_empty(),
            ) {
                (Some(r), true, _) => {
                    return Step::Calls(vec![call(n, "browser_click", json!({"ref": r}))])
                }
                (Some(_), false, true) => {
                    return Step::Calls(vec![call(n, "browser_downloads", json!({}))])
                }
                _ => {
                    return Step::Final(if w.zh {
                        "打不开 bolttech 的旅游保险索偿表（FWD 香港网站上的 PDF）。请把表格上传到对话里，我来填写。".into()
                    } else {
                        "bolttech's Travel Insurance Claim Form (the PDF on FWD Hong Kong's site) could not be opened or downloaded. Upload the form to this chat and I will fill it in.".into()
                    })
                }
            }
        }
    };
    let output = format!(
        "attachments/bolttech-travel-claim-{}.pdf",
        if d.policy_no.is_empty() {
            "form".to_string()
        } else {
            d.policy_no.clone()
        }
    );
    let filled = results_of(transcript, "pdf_form_fill")
        .last()
        .map(|(_, m)| get_str(&json_of(m), "output"))
        .filter(|p| !p.is_empty());
    let Some(filled) = filled else {
        return Step::Calls(vec![call(
            n,
            "pdf_form_fill",
            json!({"path": form_path, "values": crate::forms::bolttech_delay_claim(&d.args(s.currency), w.profile), "output": output}),
        )]);
    };
    let letter = saved_path(transcript).unwrap_or_default();
    if results_of(transcript, "send_file").is_empty() {
        return Step::Calls(vec![call(
            n,
            "send_file",
            json!({"path": filled, "note": "bolttech's travel claim form, filled in — add your HKID number, bank account, signature and date before sending"}),
        )]);
    }
    if results_of(transcript, "gmail_create_draft").is_empty() {
        let mut atts = vec![filled.clone()];
        if !letter.is_empty() {
            atts.push(letter.clone());
        }
        let body = format!(
            "Dear bolttech claims team,\n\nPlease find attached my travel insurance claim for a flight delay.\n\nPolicyholder: {}\nPolicy number: {}{}\nFlight: {}{} on {}, scheduled departure {}, actual departure {} — delayed {}{}.\nAmount claimed: {} {} ({} × {} hours of delay).\n\nAttachments: the signed claim form{}.\n\nKind regards,\n{}",
            get_str(w.profile, "name_en"), d.policy_no,
            if d.plan.is_empty() { String::new() } else { format!(" ({})", d.plan) },
            d.flight_no, d.route(), d.incident_date, d.scheduled, d.actual, hours_text(d.minutes),
            if d.reason.is_empty() { String::new() } else { format!(" ({})", d.reason) },
            s.currency, money(d.amount), d.blocks(), d.block_hours,
            if letter.is_empty() { String::new() } else { format!(", the airline's delay letter{}", if d.airline_ref.is_empty() { String::new() } else { format!(" (reference {})", d.airline_ref) }) },
            get_str(w.profile, "name_en")
        );
        return Step::Calls(vec![call(
            n,
            "gmail_create_draft",
            json!({"to": s.claims_email,
                   "subject": format!("Travel insurance claim — policy {} — flight delay {} {}", d.policy_no, d.flight_no, d.incident_date),
                   "body": body, "attachments": atts}),
        )]);
    }
    if results_of(transcript, "schedule_create").is_empty() {
        return Step::Calls(vec![call(
            n,
            "schedule_create",
            json!({"name": format!("bolttech claim check {}", d.policy_no),
                   "goal": format!("Search my mail (gmail_search) for any email from bolttech or FWD about my travel delay claim on policy {} and notify me of news; stop when the claim is paid or closed.", d.policy_no),
                   "kind": "interval", "spec": "10080"}),
        )]);
    }
    Step::Final(answer_delay(
        s,
        &d,
        None,
        w,
        Some(&HkDone {
            form: filled,
            letter,
        }),
    ))
}

// ---------------------------------------------------------------- the FWD Singapore purchase

fn next_sg_buy(s: &ClaimSpec, transcript: &[Value], w: &World, n: usize) -> Step {
    if let Some(step) = gather(s, transcript, w, n, "") {
        return step;
    }
    let msgs = fetched(transcript);
    let mut q = trip_quote(&msgs, w.user_texts, w.today);
    if let Some((a, _)) = results_of(transcript, "form_fill")
        .into_iter()
        .find(|(a, _)| sget(a, "stage") == "quote")
    {
        q.keep_from(&a);
    }
    if !q.complete() {
        if !asked_before(transcript, ASK_TRIP) && !asked_before(transcript, "出发和返回") {
            let mut lines = Vec::new();
            if q.destination.is_empty() {
                lines.push("- the destination".to_string());
            }
            if q.depart.is_empty() || q.ret.is_empty() {
                lines.push(format!("- {ASK_TRIP}"));
            }
            let mut qn = format!(
                "To quote FWD's travel insurance I still need:\n{}",
                lines.join("\n")
            );
            if !w.mail_ready {
                qn.push_str("\n\n(Connect your mailbox in Connections and I can read the trip from your booking emails myself.)");
            }
            return Step::Final(qn);
        }
        return Step::Final(if w.zh {
            "没有出发/返回日期和目的地就无法报价；请把它们告诉我，或上传机票预订。".into()
        } else {
            "Without the destination and the departure and return dates FWD cannot quote the trip; tell me them or upload the flight booking and ask me again.".into()
        });
    }
    if !opened(transcript, s.host, "/travel-insurance/online-quote") {
        return Step::Calls(vec![call(
            n,
            "browser_navigate",
            json!({"url": s.start_url}),
        )]);
    }
    let quote_pos = match fill_gate(transcript, n, "quote", q.args(), s.name, w.zh) {
        Gate::Step(st) => return st,
        Gate::Done(p) => p,
    };
    // the plans the page lists, read from a snapshot after the quote
    let (plan_text, plan_pos) = match results_of(transcript, "browser_snapshot")
        .iter()
        .find(|(_, m)| pos(transcript, m) > quote_pos)
    {
        Some((_, m)) => (snapshot_text(m), pos(transcript, m)),
        None => {
            return Step::Calls(vec![call(
                n,
                "browser_snapshot",
                json!({"max_chars": 8000}),
            )])
        }
    };
    let plans = plans_in(&plan_text);
    let Some((picked, rule)) = pick_plan(&plans, &q.minimum) else {
        return Step::Final(if w.zh {
            format!("FWD 的报价页没有列出 {} {} 至 {} 的方案；请到浏览器标签页自己看一下。没有买任何保险。", q.destination, q.depart, q.ret)
        } else {
            format!("FWD's quote page listed no plans for {} ({} to {}, {} traveller(s)); open the browser tab to see what it says. Nothing was bought.", q.destination, q.depart, q.ret, q.travellers)
        });
    };
    if results_of(transcript, "present_choices").is_empty() {
        let options: Vec<Value> = plans
            .iter()
            .map(|p| {
                json!({"label": p.name, "source_url": s.start_url,
                       "details": p.cover.iter().map(|(_, v)| format!("{} {}", s.currency, money(*v).trim_end_matches(".00"))).collect::<Vec<_>>(),
                       "note": if p.name == picked.name { format!("picked: {rule}") } else { String::new() }})
            })
            .collect();
        return Step::Calls(vec![call(
            n,
            "present_choices",
            json!({"question": format!("FWD Singapore plans for {} ({} to {}, {} traveller(s)): premium, then the covers in the page's order ({})", q.destination, q.depart, q.ret, q.travellers,
                                       picked.cover.iter().map(|(c, _)| c.as_str()).collect::<Vec<_>>().join(", ")),
                   "kind": "comparison", "options": options, "picked": picked.name}),
        )]);
    }
    // the plan's Select link, then the application
    let clicked = results_of(transcript, "browser_click")
        .iter()
        .any(|(a, m)| sget(a, "ref") == picked.link && pos(transcript, m) > plan_pos);
    if !clicked {
        return Step::Calls(vec![call(n, "browser_click", json!({"ref": picked.link}))]);
    }
    let apply_pos = match fill_gate(
        transcript,
        n,
        "apply",
        json!({"plan": picked.name}),
        s.name,
        w.zh,
    ) {
        Gate::Step(st) => return st,
        Gate::Done(p) => p,
    };
    // the card: the Vault's list once, the user's card by its name
    let vaults = results_of(transcript, "vault_list");
    let Some((_, vault)) = vaults.last() else {
        return Step::Calls(vec![call(n, "vault_list", json!({}))]);
    };
    let cards = vault_cards(&content_of(vault));
    let card = pick_card(&cards, w.user_texts);
    let card_ok = card.is_some_and(|c| !c.name_only);
    let item_name = format!(
        "{} travel insurance, {}, {} to {}, {} traveller(s)",
        picked.name, q.destination, q.depart, q.ret, q.travellers
    );
    let mut secrets_pos = apply_pos;
    if let (Some(c), true) = (card, card_ok) {
        let purchases = results_of(transcript, "purchase_confirm");
        let Some((_, pm)) = purchases.last() else {
            return Step::Calls(vec![call(
                n,
                "purchase_confirm",
                json!({"site": s.host, "items": [{"name": item_name, "qty": 1, "price": picked.premium}], "shipping": 0,
                       "total": picked.premium, "currency": s.currency, "delivery": "policy documents by email",
                       "card_item_id": c.id}),
            )]);
        };
        let pc = content_of(pm);
        if pc.starts_with("DENIED") || pc.to_lowercase().contains("\"status\": \"denied\"") {
            return Step::Final(answer_buy(
                s, &q, &plans, picked, &rule, None, card, false, w,
            ));
        }
        let ppos = pos(transcript, pm);
        // the card fields' refs from a snapshot after the approval, then one secret fill per field
        let snap = results_of(transcript, "browser_snapshot")
            .iter()
            .find(|(_, m)| pos(transcript, m) > ppos)
            .map(|(_, m)| snapshot_text(m));
        let Some(snap) = snap else {
            return Step::Calls(vec![call(
                n,
                "browser_snapshot",
                json!({"max_chars": 6000}),
            )]);
        };
        let done: Vec<String> = results_of(transcript, "browser_fill_secret")
            .iter()
            .filter(|(_, m)| pos(transcript, m) > ppos)
            .map(|(a, _)| sget(a, "field"))
            .collect();
        for (field, label) in CARD_FIELDS {
            if done.iter().any(|d| d == field) {
                continue;
            }
            if let Some(r) = ref_of(&snap, "textbox", label) {
                return Step::Calls(vec![call(
                    n,
                    "browser_fill_secret",
                    json!({"ref": r, "item_id": c.id, "field": field}),
                )]);
            }
        }
        secrets_pos = results_of(transcript, "browser_fill_secret")
            .last()
            .map(|(_, m)| pos(transcript, m))
            .unwrap_or(ppos)
            .max(ppos);
    }
    let card_note = match card {
        Some(c) if card_ok => format!(" and your {} (Vault)", c.label),
        Some(c) => format!(". Your {} in the Vault has no number, expiry or CVC (added by its name only): type the card on the page yourself, or add the details in Memory → Vault first", c.label),
        None => ". There is no card in your Vault: type the card on the page yourself".to_string(),
    };
    let reason = format!(
        "The {} plan ({} {}) application is filled in from your booking, profile and passport (Vault){}. The health and declaration questions are yours: {}; hand the browser back when the confirmation shows (or without paying).",
        picked.name, s.currency, money(picked.premium), card_note, s.user_steps
    );
    let back = match review_gate(transcript, n, secrets_pos, reason) {
        Ok(p) => p,
        Err(st) => return st,
    };
    let page = match snapshot_after(transcript, n, back) {
        Ok(p) => p,
        Err(st) => return st,
    };
    let policy = reference_in(&page, s.ref_re);
    Step::Final(answer_buy(
        s,
        &q,
        &plans,
        picked,
        &rule,
        policy.as_deref(),
        card,
        card_ok,
        w,
    ))
}

/// The purchase's answer: the market and insurer, the plans with their premiums, the plan chosen and the rule, the
/// status (paid by the user: the policy number; else what is left to them), the emergency line.
#[allow(clippy::too_many_arguments)]
pub fn answer_buy(
    s: &ClaimSpec,
    q: &TripQuote,
    plans: &[Plan],
    picked: &Plan,
    rule: &str,
    policy: Option<&str>,
    card: Option<&VaultCard>,
    card_ok: bool,
    w: &World,
) -> String {
    let list: Vec<String> = plans
        .iter()
        .map(|p| format!("- {}", p.summary(s.currency)))
        .collect();
    let status = match (policy, card, card_ok) {
        (Some(p), _, _) => format!("**Bought** — you paid on FWD's page; policy number **{p}**, the documents come by email."),
        (None, Some(c), true) => format!("The application is filled in (name, date of birth, contact, passport and your {} from the Vault); the declaration and **Pay now** are yours — nothing has been paid.", c.label),
        (None, Some(c), false) => format!("The application is filled in (name, date of birth, contact, passport from the Vault). Your {} is in the Vault by its name only — its number, expiry and CVC are UNKNOWN, so nothing was charged: type the card on the page yourself, or add the details in Memory → Vault and ask me again. The declaration and Pay now are yours.", c.label),
        (None, None, _) => "The application is filled in (name, date of birth, contact, passport from the Vault). There is no card in your Vault: type the card on the page yourself; the declaration and Pay now are yours — nothing has been paid.".into(),
    };
    if w.zh {
        return format!(
            "**FWD 新加坡旅行保险**（{}）：{}，{} 至 {}，{} 人。报价的方案：\n{}\n\n选定：**{}，{} {}** —— {}。\n\n{status}",
            s.decides, q.destination, q.depart, q.ret, q.travellers, list.join("\n"), picked.name, s.currency, money(picked.premium), rule
        );
    }
    format!(
        "**FWD Singapore travel insurance** ({}) for {}, {} to {}, {} traveller(s). Plans quoted:\n{}\n\nChosen: **{}, {} {}** — {}.\n\n{status}",
        s.decides, q.destination, q.depart, q.ret, q.travellers, list.join("\n"), picked.name, s.currency, money(picked.premium), rule
    )
}

// ---------------------------------------------------------------- the extraction call (the last resort)

/// The extraction call's instruction when the regexes found no loss date, amounts or delay times: JSON only.
pub const EXTRACT_SYSTEM: &str = "Read the emails and the user's words below and return JSON only, exactly these keys: \
{\"loss_date\": \"YYYY-MM-DD\", \"paid\": \"\", \"refunded\": \"\", \"change_fee\": \"\", \"incident_date\": \"YYYY-MM-DD\", \
\"scheduled_departure\": \"HH:MM\", \"actual_departure\": \"HH:MM\", \"policy_no\": \"\"}. \
loss_date is the day a trip was cancelled or cut short; paid what was paid for the bookings, refunded what the supplier gave back, \
change_fee the fee charged to change a flight (plain numbers like 1840.00); incident_date the day of a delayed flight, scheduled_departure and \
actual_departure its times from the airline's certificate; policy_no the insurance policy number. Leave a value empty when the texts do not \
give it. Never invent.";

/// The facts the extraction call returned, as the transcript records them (a `claim_facts` tool exchange).
pub fn extract_exchange(n: usize, text: &str, facts: &Value) -> Vec<Value> {
    let id = format!("clm_{n}");
    let mut m = Map::new();
    for k in [
        "loss_date",
        "paid",
        "refunded",
        "change_fee",
        "incident_date",
        "scheduled_departure",
        "actual_departure",
        "policy_no",
    ] {
        m.insert(k.into(), json!(get_str(facts, k)));
    }
    vec![
        json!({"role": "assistant", "content": "", "tool_calls": [{"id": id, "type": "function",
               "function": {"name": "claim_facts", "arguments": json!({"chars": text.chars().count()}).to_string()}}]}),
        json!({"role": "tool", "tool_call_id": id, "content": Value::Object(m).to_string()}),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    const BACK: &str = "\"Takeover finished: the user handed browser control back.\"";

    fn step(id: &str, name: &str, args: Value, result: &str) -> Vec<Value> {
        vec![
            json!({"role": "assistant", "content": "", "tool_calls": [{"id": id, "type": "function",
                   "function": {"name": name, "arguments": args.to_string()}}]}),
            json!({"role": "tool", "tool_call_id": id, "content": result}),
        ]
    }

    /// A `browser_snapshot` result as the sentinel returns it: the snapshot inside the untrusted JSON envelope.
    fn page(url: &str, snapshot: &str) -> String {
        format!(
            "<untrusted_content source=\"web page {url}\">\n{}\n</untrusted_content>",
            json!({"trust": "untrusted", "source": format!("web page {url}"), "url": url,
                   "title": "FWD", "tabs": [], "snapshot": snapshot})
        )
    }

    fn msg(id: &str, from: &str, subject: &str, body: &str) -> Msg {
        Msg {
            id: id.into(),
            from: from.into(),
            subject: subject.into(),
            body: body.into(),
            attachments: vec![],
        }
    }

    fn mail_step(n: &mut usize, m: &Msg) -> Vec<Value> {
        *n += 1;
        step(
            &format!("clm_{}", *n - 1),
            "gmail_get_message",
            json!({"message_id": m.id}),
            &json!({"from": m.from, "subject": m.subject, "body": m.body,
                    "attachments": m.attachments.iter().map(|a| json!({"filename": a})).collect::<Vec<_>>()})
            .to_string(),
        )
    }

    fn world<'a>(user: &'a [String], profile: &'a Value, mail: bool) -> World<'a> {
        World {
            user_texts: user,
            profile,
            today: NaiveDate::from_ymd_opt(2026, 10, 31).unwrap(),
            mail_ready: mail,
            zh: false,
        }
    }

    fn cancellation_mail() -> Vec<Msg> {
        vec![
            msg("m1", "United Airlines <noreply@united.example>", "Your eTicket receipt — Confirmation ABC123 — UA 837 SFO-NRT 2026-10-28",
                "Passenger LUCAS LU. UA 837 San Francisco (SFO) 2026-10-28 11:05 → Tokyo Narita (NRT) 14:30 (+1). Fare USD 1,840.00, Basic Economy, non-refundable. Paid with Visa ending 4321 on 2026-09-21."),
            msg("m2", "Chase <no.reply.alerts@chase.example>", "Your $1,840.00 transaction with UNITED AIRLINES",
                "A $1,840.00 transaction with UNITED AIRLINES was made on your Sapphire Reserve (...4321) on 2026-09-21."),
            msg("m3", "United Refunds <refunds@united.example>", "Refund processed — confirmation ABC123",
                "Taxes and fees of USD 86.40 have been refunded to Visa ending 4321. The Basic Economy fare of USD 1,753.60 is non-refundable under the fare rules."),
        ]
    }

    fn interruption_mail() -> Vec<Msg> {
        vec![
            msg("m1", "United Airlines <noreply@united.example>", "Your eTicket receipt — Confirmation QX7P2L — SFO-NRT 2026-10-22, NRT-SFO 2026-10-28",
                "Passenger LUCAS LU. UA 837 San Francisco (SFO) 2026-10-22 11:05 → Tokyo Narita (NRT) 2026-10-23 14:30. UA 838 Tokyo Narita (NRT) 2026-10-28 17:00 → San Francisco (SFO) 2026-10-28 10:20. Fare USD 1,620.00 round trip, Economy. Paid with Visa ending 4321 on 2026-09-01."),
            msg("m2", "Chase <no.reply.alerts@chase.example>", "Your $1,620.00 transaction with UNITED AIRLINES",
                "A $1,620.00 transaction with UNITED AIRLINES was made on your Sapphire Reserve (...4321) on 2026-09-01."),
            msg("m3", "Park Hyatt Tokyo <reservations@parkhyatttokyo.example>", "Reservation confirmed — HY-55821 — 2026-10-23 to 2026-10-28",
                "Guest LUCAS LU. 5 nights, check-in 2026-10-23, check-out 2026-10-28, Park Deluxe King. Prepaid rate USD 250.00 per night, USD 1,250.00 total, non-refundable. Charged to Visa ending 4321 on 2026-09-01."),
            msg("m4", "United Airlines <noreply@united.example>", "Itinerary change confirmed — QX7P2L — UA 838 now on 2026-10-24",
                "Your return flight UA 838 Tokyo Narita (NRT) → San Francisco (SFO) was moved from 2026-10-28 to 2026-10-24 at 17:00 at your request. Change fee USD 300.00 charged to Visa ending 4321 on 2026-10-24."),
            msg("m5", "Chase <no.reply.alerts@chase.example>", "Your $300.00 transaction with UNITED AIRLINES",
                "A $300.00 transaction with UNITED AIRLINES was made on your Sapphire Reserve (...4321) on 2026-10-24."),
            msg("m6", "Park Hyatt Tokyo <reservations@parkhyatttokyo.example>", "Early departure — reservation HY-55821",
                "Dear Mr Lu, you checked out on 2026-10-24 after 1 night of your 5-night stay. As a courtesy we have refunded USD 250.00 (one night) to Visa ending 4321. The remaining 3 nights (USD 750.00) are non-refundable under the prepaid rate. We wish you a speedy recovery."),
        ]
    }

    #[test]
    fn the_benefit_and_the_amount_follow_the_rules() {
        let today = NaiveDate::from_ymd_opt(2026, 10, 31).unwrap();
        let user = vec!["I came down with food poisoning and couldn't take my United flight to Tokyo on 2026-10-28. I paid for it with my Chase Sapphire Reserve — can you claim the card's trip cancellation insurance for me? I saw Dr. Amy Chen at Mission Bay Clinic on 2026-10-28 and she gave me a letter saying I was unable to travel. I'll press Submit myself.".to_string()];
        let t = trip_claim(&cancellation_mail(), &user, today);
        assert_eq!(t.benefit, "Trip Cancellation");
        assert_eq!(t.loss_date, "2026-10-28");
        assert_eq!(
            (t.paid, t.refunded, t.lost, t.change_fee),
            (1840.0, 86.40, 1753.60, 0.0)
        );
        assert_eq!(t.amount, 1753.60);
        assert_eq!(t.last4, "4321");
        assert_eq!(t.card_product, "Chase Sapphire Reserve");
        assert_eq!(t.reason, "sickness of the cardholder");
        assert_eq!(t.booking(), "United Airlines ABC123");
        assert!(t.has_doctor_letter && t.has_supplier_letter);
        assert_eq!(t.trip, "UA 837");
        // the interruption: the unrefunded nights plus the change fee
        let user = vec!["I broke my wrist in Tokyo on 2026-10-24 and the doctor at St. Luke's International Hospital told me to go home, so I cut my trip short and flew back that evening. The flights and the hotel were paid with my Chase Sapphire Reserve. Please start the card's insurance claim for me on the claims site — I'll review it and press Submit myself.".to_string()];
        let t = trip_claim(&interruption_mail(), &user, today);
        assert_eq!(t.benefit, "Trip Interruption");
        assert_eq!(t.loss_date, "2026-10-24");
        assert_eq!(
            (t.paid, t.refunded, t.lost, t.change_fee),
            (2870.0, 250.0, 750.0, 300.0)
        );
        assert_eq!(t.amount, 1050.0);
        assert_eq!(t.reason, "injury of the cardholder");
        assert_eq!(
            t.booking(),
            "United Airlines QX7P2L; Park Hyatt Tokyo HY-55821"
        );
        assert!(
            t.trip.contains("UA 837") && t.trip.contains("Park Hyatt Tokyo"),
            "{}",
            t.trip
        );
        // no words about the trip's start: the hotel's early-departure letter decides, and the date comes from it
        let t = trip_claim(
            &interruption_mail(),
            &["Claim the trip insurance on my Chase card please.".into()],
            today,
        );
        assert_eq!(
            (t.benefit.as_str(), t.loss_date.as_str()),
            ("Trip Interruption", "2026-10-24")
        );
        // the caps are data
        let big = TripClaim {
            benefit: "Trip Cancellation".into(),
            paid: 25_000.0,
            travellers: 1,
            ..Default::default()
        };
        assert_eq!(claim_amount(&big, spec(Flow::CsrTrip)), 10_000.0);
        let two = TripClaim {
            travellers: 2,
            ..big.clone()
        };
        assert_eq!(claim_amount(&two, spec(Flow::CsrTrip)), 20_000.0);
    }

    #[test]
    fn the_interruption_answer_carries_both_deadlines_and_the_status() {
        // cards_interruption_real on b24962d: the answer the user saw had no 90-day date and "may have been submitted"
        // (the driven answer was replaced by the runtime's rescue; this is the answer code gives)
        let s = spec(Flow::CsrTrip);
        let user = vec!["I broke my wrist in Tokyo on 2026-10-25 and cut my trip short. Please start the card's insurance claim for me on the claims site — I'll review it and press Submit myself.".to_string()];
        let profile = json!({});
        let w = world(&user, &profile, true);
        let t = TripClaim {
            benefit: "Trip Interruption".into(),
            loss_date: "2026-10-25".into(),
            change_fee: 300.0,
            lost: 750.0,
            amount: 1050.0,
            card_product: "Chase Sapphire Reserve".into(),
            reason: "injury of the cardholder".into(),
            travellers: 1,
            ..Default::default()
        };
        for (reference, status) in [
            (
                Some("CBS-26-118204"),
                "You submitted it on eClaimsLine: claim number **CBS-26-118204**.",
            ),
            (None, "nothing has been submitted for you"),
        ] {
            let a = answer_csr(s, &t, reference, &w);
            assert!(
                a.contains("notify the administrator by **14 November 2026**")
                    && a.contains("documents by **23 January 2027**")
                    && a.contains(status),
                "{a}"
            );
        }
    }

    #[test]
    fn the_csr_claim_steps_follow_the_transcript() {
        let s = spec(Flow::CsrTrip);
        let user = vec!["I came down with food poisoning and couldn't take my United flight to Tokyo on 2026-10-28. I paid for it with my Chase Sapphire Reserve — claim the card's trip cancellation insurance. I'll press Submit myself.".to_string()];
        let profile = json!({"name_en": "LUCAS LU", "email_personal": "lucas@example.com", "phone": "+1 415 555 0100"});
        let w = world(&user, &profile, true);
        let mut t = vec![json!({"role": "user", "content": user[0]})];
        t.extend(step(
            "k1",
            "load_skill",
            json!({"name": "csr-trip-claim"}),
            "---",
        ));
        assert_eq!(active(&t).map(|s| s.flow), Some(Flow::CsrTrip));
        // the same skill loaded for a question about the benefit: not driven
        let mut info = vec![
            json!({"role": "user", "content": "What does my Chase Sapphire Reserve's trip cancellation insurance cover? Can I claim a cancelled flight?"}),
        ];
        info.extend(step(
            "k1",
            "load_skill",
            json!({"name": "csr-trip-claim"}),
            "---",
        ));
        assert!(active(&info).is_none());
        // 1. the four fixed searches
        let Step::Calls(c) = next(s, &t, &w) else {
            panic!()
        };
        assert_eq!(c.len(), 4);
        assert!(sget(&c[1].args, "query").contains("(Chase OR Sapphire)"));
        let mut n = 1;
        for (i, call_) in c.iter().enumerate() {
            let hits: Vec<Value> = cancellation_mail()
                .iter()
                .map(|m| json!({"id": m.id, "subject": m.subject}))
                .collect();
            let r = if i == 0 {
                json!({"count": hits.len(), "messages": hits})
            } else {
                json!({"count": 0, "messages": []})
            };
            t.extend(step(
                &format!("clm_{n}"),
                "gmail_search",
                call_.args.clone(),
                &r.to_string(),
            ));
            n += 1;
        }
        // 2. the hits
        let Step::Calls(c) = next(s, &t, &w) else {
            panic!()
        };
        assert_eq!(
            c.iter()
                .map(|x| sget(&x.args, "message_id"))
                .collect::<Vec<_>>(),
            ["m1", "m2", "m3"]
        );
        for m in cancellation_mail() {
            t.extend(mail_step(&mut n, &m));
        }
        // 3. the portal
        let Step::Calls(c) = next(s, &t, &w) else {
            panic!()
        };
        assert_eq!(
            (c[0].name.as_str(), sget(&c[0].args, "url").as_str()),
            ("browser_navigate", s.start_url)
        );
        t.extend(step(&format!("clm_{n}"), "browser_navigate", c[0].args.clone(), "<untrusted_content source=\"web page https://www.eclaimsline.com/\">{}</untrusted_content>"));
        n += 1;
        // 4. the fill: the benefit by rule, the amount by arithmetic, the reason without the diagnosis
        let Step::Calls(c) = next(s, &t, &w) else {
            panic!()
        };
        assert_eq!(c[0].name, "form_fill");
        let a = &c[0].args;
        assert_eq!(
            (
                sget(a, "benefit").as_str(),
                sget(a, "loss_date").as_str(),
                sget(a, "amount").as_str()
            ),
            ("Trip Cancellation", "2026-10-28", "1753.60")
        );
        assert_eq!(
            (
                sget(a, "reason").as_str(),
                sget(a, "last4").as_str(),
                sget(a, "booking").as_str()
            ),
            (
                "sickness of the cardholder",
                "4321",
                "United Airlines ABC123"
            )
        );
        assert!(!a.to_string().contains("poisoning"));
        t.extend(step(&format!("clm_{n}"), "form_fill", c[0].args.clone(), "{\"url\": \"https://www.eclaimsline.com/\", \"filled\": [{\"op\": \"text\", \"field\": \"name\", \"ok\": true}]}"));
        n += 1;
        // 5. the review hand-over: the user presses Submit
        let Step::Calls(c) = next(s, &t, &w) else {
            panic!()
        };
        assert_eq!(c[0].name, "browser_request_takeover");
        let reason = sget(&c[0].args, "reason");
        assert!(
            reason.contains("Trip Cancellation")
                && reason.contains("1,753.60")
                && reason.contains("paid 1,840.00 − refunded 86.40")
                && reason.contains("press Submit claim"),
            "{reason}"
        );
        t.extend(step(
            &format!("clm_{n}"),
            "browser_request_takeover",
            c[0].args.clone(),
            BACK,
        ));
        n += 1;
        // 6. the confirmation (none: the user handed it back unsent), the reminders, the answer
        let Step::Calls(c) = next(s, &t, &w) else {
            panic!()
        };
        assert_eq!(c[0].name, "browser_snapshot");
        t.extend(step(&format!("clm_{n}"), "browser_snapshot", c[0].args.clone(), "<untrusted_content source=\"web page https://www.eclaimsline.com/\">File a claim. Cardholder name LUCAS LU</untrusted_content>"));
        n += 1;
        let Step::Calls(c) = next(s, &t, &w) else {
            panic!()
        };
        assert_eq!(c.len(), 2);
        assert_eq!(
            (c[0].name.as_str(), sget(&c[0].args, "spec").as_str()),
            ("schedule_create", "0 8 19 1 *")
        ); // 2027-01-26 − 7
        assert_eq!(
            (
                sget(&c[1].args, "kind").as_str(),
                sget(&c[1].args, "spec").as_str()
            ),
            ("interval", "14400")
        );
        for x in &c {
            t.extend(step(
                &format!("clm_{n}"),
                "schedule_create",
                x.args.clone(),
                "created",
            ));
            n += 1;
        }
        let Step::Final(a) = next(s, &t, &w) else {
            panic!()
        };
        assert!(a.starts_with("**Trip Cancellation**"), "{a}");
        assert!(
            a.contains("17 November 2026")
                && a.contains("26 January 2027")
                && a.contains("USD 1,753.60")
                && a.contains("waiting for your Submit")
                && a.contains("4321").not(),
            "{a}"
        );
        assert!(!a.contains("poisoning") && a.contains("Card Benefit Services"));
        // the user submitted it: the claim number is in the answer
        let mut t2 = t[..t.len() - 6].to_vec();
        t2.extend(step("clm_x", "browser_snapshot", json!({}), "<untrusted_content source=\"web page https://www.eclaimsline.com/eclaims_done.html\">Claim received. Your claim number is CBS-26-118204.</untrusted_content>"));
        let Step::Calls(c) = next(s, &t2, &w) else {
            panic!()
        };
        assert!(sget(&c[0].args, "goal").contains("CBS-26-118204"));
        t2.extend(step(
            "clm_y",
            "schedule_create",
            c[0].args.clone(),
            "created",
        ));
        let Step::Final(a) = next(s, &t2, &w) else {
            panic!()
        };
        assert!(
            a.contains("You submitted it on eClaimsLine: claim number **CBS-26-118204**"),
            "{a}"
        );
    }

    trait Not {
        fn not(self) -> bool;
    }
    impl Not for bool {
        fn not(self) -> bool {
            !self
        }
    }

    #[test]
    fn missing_facts_get_one_extraction_then_one_question() {
        let s = spec(Flow::CsrTrip);
        let user = vec!["Claim the trip insurance on my Chase Sapphire card.".to_string()];
        let profile = json!({});
        let w = world(&user, &profile, false);
        let t = step("k1", "load_skill", json!({"name": "csr-trip-claim"}), "---");
        // no mail, the bare request: the question (no extraction without text to read)
        let Step::Final(q) = next(s, &t, &w) else {
            panic!()
        };
        assert!(
            q.contains(ASK_LOSS) && q.contains("Connect your mailbox"),
            "{q}"
        );
        // the user's answer: an extraction call once, its facts then used
        let answered = vec![
            user[0].clone(),
            "It was the flight on 28 October 2026; I paid $1,200 and got nothing back.".to_string(),
        ];
        let w2 = world(&answered, &profile, false);
        let mut t2 = vec![
            json!({"role": "user", "content": answered[0]}),
            json!({"role": "assistant", "content": q}),
            json!({"role": "user", "content": answered[1]}),
        ];
        t2.extend(step(
            "k2",
            "load_skill",
            json!({"name": "csr-trip-claim"}),
            "---",
        ));
        // the regexes read the date and the amount from the words: straight to the portal
        let Step::Calls(c) = next(s, &t2, &w2) else {
            panic!()
        };
        assert_eq!(c[0].name, "browser_navigate");
        // nothing the regexes can read: the extraction call, then its facts
        let vague = vec![
            user[0].clone(),
            "the flight last week, the fare was about twelve hundred dollars".to_string(),
        ];
        let w3 = world(&vague, &profile, false);
        let mut t3 = vec![
            json!({"role": "user", "content": vague[0]}),
            json!({"role": "assistant", "content": q}),
            json!({"role": "user", "content": vague[1]}),
        ];
        t3.extend(step(
            "k3",
            "load_skill",
            json!({"name": "csr-trip-claim"}),
            "---",
        ));
        let Step::Extract(text) = next(s, &t3, &w3) else {
            panic!()
        };
        assert!(text.contains("twelve hundred"));
        t3.extend(extract_exchange(
            1,
            &text,
            &json!({"loss_date": "2026-10-24", "paid": "1200", "refunded": ""}),
        ));
        let Step::Calls(c) = next(s, &t3, &w3) else {
            panic!()
        };
        assert_eq!(c[0].name, "browser_navigate");
    }

    fn sg_mail() -> Vec<Msg> {
        vec![
            msg("p1", "FWD Singapore <noreply@fwd.com.sg>", "Your FWD Travel Insurance policy TI-SG-2026-55123",
                "Dear TAN WEI MING,\n\nThank you for choosing FWD. Policy number: TI-SG-2026-55123. Plan: Business (single trip), Japan. Period of insurance: 2026-10-28 to 2026-11-04.\nTravel delay: SGD 100 for every 6 consecutive hours of delay, up to SGD 1,500. Claims must be notified within 30 days.\n24-hour emergency assistance overseas: +65 6322 2072."),
            Msg { attachments: vec!["delay_certificate_SQ636.pdf".into()], ..msg("c1", "Singapore Airlines <noreply@singaporeair.example>", "Delay certificate — SQ 636 on 2026-10-29",
                "Dear TAN WEI MING,\n\nSQ 636 Singapore–Tokyo Haneda on 2026-10-29 was scheduled to depart at 10:30 and departed at 17:45 because of the late arrival of the inbound aircraft. Our delay certificate is attached.\n\nSingapore Airlines") },
        ]
    }

    #[test]
    fn the_delay_is_hours_from_the_certificate_and_the_benefit_the_policys_rule() {
        let today = NaiveDate::from_ymd_opt(2026, 10, 31).unwrap();
        let s = spec(Flow::FwdSgClaim);
        let user = vec!["My flight SQ 636 to Tokyo on 2026-10-29 was delayed by more than 7 hours. Please claim it on my FWD travel insurance — the policy and the airline's delay certificate are in my email.".to_string()];
        let d = delay_claim(s, &sg_mail(), &user, today);
        assert_eq!(
            (d.policy_no.as_str(), d.plan.as_str()),
            ("TI-SG-2026-55123", "Business")
        );
        assert_eq!(
            (d.scheduled.as_str(), d.actual.as_str(), d.minutes),
            ("10:30", "17:45", 435)
        );
        assert_eq!(hours_text(d.minutes), "7 h 15 min");
        assert_eq!(
            (d.per_block, d.block_hours, d.cap, d.amount),
            (100.0, 6, 1500.0, 100.0)
        );
        assert_eq!((d.limit_days, d.limit_from_policy), (30, true));
        assert_eq!(
            (
                d.flight_no.as_str(),
                d.from.as_str(),
                d.to.as_str(),
                d.airline.as_str()
            ),
            ("SQ 636", "Singapore", "Tokyo Haneda", "Singapore Airlines")
        );
        assert_eq!(d.incident_date, "2026-10-29");
        assert_eq!(d.reason, "the late arrival of the inbound aircraft");
        assert_eq!(
            d.attachment,
            Some(("c1".to_string(), "delay_certificate_SQ636.pdf".to_string()))
        );
        assert_eq!(d.emergency, "+65 6322 2072");
        // 13 hours → two blocks; 5 hours → nothing
        let two = DelayClaim {
            minutes: 13 * 60,
            ..d.clone()
        };
        assert_eq!(delay_amount(&two), 200.0);
        let none = DelayClaim {
            minutes: 5 * 60,
            ..d.clone()
        };
        assert_eq!(delay_amount(&none), 0.0);
    }

    #[test]
    fn the_sg_claim_steps_follow_the_transcript() {
        let s = spec(Flow::FwdSgClaim);
        let user = vec!["My flight SQ 636 to Tokyo on 2026-10-29 was delayed by more than 7 hours. Please claim it on my FWD travel insurance; I'll check it and submit it myself.".to_string()];
        let profile = json!({"name_en": "TAN WEI MING", "email_personal": "weiming@example.com", "phone": "+65 9123 4567"});
        let w = world(&user, &profile, true);
        let mut t = vec![json!({"role": "user", "content": user[0]})];
        t.extend(step(
            "k1",
            "load_skill",
            json!({"name": "fwd-travel-insurance"}),
            "---",
        ));
        assert_eq!(active(&t).map(|s| s.flow), Some(Flow::FwdSgClaim));
        let Step::Calls(c) = next(s, &t, &w) else {
            panic!()
        };
        assert_eq!(c.len(), 2);
        assert!(
            sget(&c[1].args, "query").starts_with("(\"SQ 636\" OR SQ636) (delay"),
            "{}",
            sget(&c[1].args, "query")
        );
        let mut n = 1;
        for (i, call_) in c.iter().enumerate() {
            let m = &sg_mail()[i];
            t.extend(step(
                &format!("clm_{n}"),
                "gmail_search",
                call_.args.clone(),
                &json!({"count": 1, "messages": [{"id": m.id, "subject": m.subject}]}).to_string(),
            ));
            n += 1;
        }
        let Step::Calls(c) = next(s, &t, &w) else {
            panic!()
        };
        assert_eq!(c.len(), 2);
        for m in sg_mail() {
            t.extend(mail_step(&mut n, &m));
        }
        // the certificate saved, the form opened and filled
        let Step::Calls(c) = next(s, &t, &w) else {
            panic!()
        };
        assert_eq!(
            (c[0].name.as_str(), sget(&c[0].args, "filename").as_str()),
            ("gmail_save_attachment", "delay_certificate_SQ636.pdf")
        );
        t.extend(step(&format!("clm_{n}"), "gmail_save_attachment", c[0].args.clone(), "附件已保存 attachment saved: attachments/delay_certificate_SQ636.pdf (12 KB, application/pdf)."));
        n += 1;
        let Step::Calls(c) = next(s, &t, &w) else {
            panic!()
        };
        assert_eq!(
            (c[0].name.as_str(), sget(&c[0].args, "url").as_str()),
            ("browser_navigate", s.start_url)
        );
        t.extend(step(&format!("clm_{n}"), "browser_navigate", c[0].args.clone(), "<untrusted_content source=\"web page https://www.fwd.com.sg/claim-online/travel.html\">{}</untrusted_content>"));
        n += 1;
        let Step::Calls(c) = next(s, &t, &w) else {
            panic!()
        };
        assert_eq!(c[0].name, "form_fill");
        let a = &c[0].args;
        assert_eq!(
            (
                sget(a, "policy_no").as_str(),
                sget(a, "incident_date").as_str(),
                sget(a, "amount").as_str(),
                sget(a, "claim_type").as_str()
            ),
            ("TI-SG-2026-55123", "2026-10-29", "100.00", "Travel delay")
        );
        assert!(
            sget(a, "what").contains("delayed 7 h 15 min"),
            "{}",
            sget(a, "what")
        );
        t.extend(step(
            &format!("clm_{n}"),
            "form_fill",
            c[0].args.clone(),
            "{\"url\": \"https://www.fwd.com.sg/claim-online/travel.html\", \"filled\": []}",
        ));
        n += 1;
        // the upload: the file input's ref from a snapshot
        let Step::Calls(c) = next(s, &t, &w) else {
            panic!()
        };
        assert_eq!(c[0].name, "browser_snapshot");
        t.extend(step(&format!("clm_{n}"), "browser_snapshot", c[0].args.clone(), "[e3] textbox \"Policy number\" value=\"TI-SG-2026-55123\"\n[e9] filechooser \"Supporting documents\" (file upload)\n[e10] button \"Submit claim\""));
        n += 1;
        let Step::Calls(c) = next(s, &t, &w) else {
            panic!()
        };
        assert_eq!(
            (
                c[0].name.as_str(),
                sget(&c[0].args, "ref").as_str(),
                sget(&c[0].args, "path").as_str()
            ),
            (
                "browser_upload",
                "e9",
                "attachments/delay_certificate_SQ636.pdf"
            )
        );
        t.extend(step(
            &format!("clm_{n}"),
            "browser_upload",
            c[0].args.clone(),
            "uploaded",
        ));
        n += 1;
        // the hand-over for Submit, the confirmation the user reached, the reminders, the answer
        let Step::Calls(c) = next(s, &t, &w) else {
            panic!()
        };
        assert_eq!(c[0].name, "browser_request_takeover");
        assert!(
            sget(&c[0].args, "reason").contains("7 h 15 min")
                && sget(&c[0].args, "reason").contains("press Submit claim")
        );
        t.extend(step(
            &format!("clm_{n}"),
            "browser_request_takeover",
            c[0].args.clone(),
            BACK,
        ));
        n += 1;
        let Step::Calls(c) = next(s, &t, &w) else {
            panic!()
        };
        assert_eq!(c[0].name, "browser_snapshot");
        t.extend(step(&format!("clm_{n}"), "browser_snapshot", c[0].args.clone(), "<untrusted_content source=\"web page https://www.fwd.com.sg/fwd_sg_claim_done.html\">We have received your claim. Claim reference: CL-TR-2026-118204.</untrusted_content>"));
        n += 1;
        let Step::Calls(c) = next(s, &t, &w) else {
            panic!()
        };
        assert_eq!(c.len(), 1); // submitted: the weekly check only, no deadline reminder
        assert_eq!(sget(&c[0].args, "spec"), "10080");
        t.extend(step(
            &format!("clm_{n}"),
            "schedule_create",
            c[0].args.clone(),
            "created",
        ));
        let Step::Final(a) = next(s, &t, &w) else {
            panic!()
        };
        assert!(
            a.contains("28 November 2026")
                && a.contains("**7 h 15 min**")
                && a.contains("**SGD 100.00**")
                && a.contains("**CL-TR-2026-118204**"),
            "{a}"
        );
        assert!(a.contains("+65 6322 2072") && !a.contains("K1234567Z"));
    }

    const QUOTE_SNAPSHOT: &str = "# Get a travel insurance quote\n  Single trip cover for Singapore residents.\n[e1] textbox \"Destination\" value=\"Japan\"\n[e5] button \"Get quote\"\n## Your plans\n  Trip: Japan, 2026-11-24 to 2026-12-01, 1 traveller(s)\n  Plan Premium Overseas medical Trip cancellation Travel delay Baggage\n  Premium SGD 34.00 SGD 200,000 SGD 5,000 SGD 100 every 6 hours, up to SGD 1,000 SGD 3,000\n[e6] link \"Select Premium\" → https://www.fwd.com.sg/fwd_sg_apply.html?plan=Premium&premium=34.00\n  Business SGD 47.00 SGD 1,000,000 SGD 10,000 SGD 100 every 6 hours, up to SGD 1,500 SGD 5,000\n[e7] link \"Select Business\" → https://www.fwd.com.sg/fwd_sg_apply.html?plan=Business&premium=47.00\n  First SGD 68.00 SGD 1,500,000 SGD 15,000 SGD 100 every 6 hours, up to SGD 2,000 SGD 7,500\n[e8] link \"Select First\" → https://www.fwd.com.sg/fwd_sg_apply.html?plan=First&premium=68.00\n  Premiums include 9% GST.";

    #[test]
    fn a_snapshot_is_read_out_of_its_envelope() {
        let url = "https://www.fwd.com.sg/travel-insurance/online-quote/";
        let m = json!({"role": "tool", "content": page(url, QUOTE_SNAPSHOT)});
        assert_eq!(snapshot_text(&m), QUOTE_SNAPSHOT);
        assert_eq!(plans_in(&snapshot_text(&m)).len(), 3);
        // an envelope cut short (a long page) is still read line by line
        let full = page(url, QUOTE_SNAPSHOT);
        let cut = &full[..full.find("[e8]").unwrap()];
        let text = snapshot_text(&json!({"role": "tool", "content": cut}));
        assert!(text.contains("\n[e7] link \"Select Business\""), "{text}");
        assert_eq!(plans_in(&text).len(), 2);
        // a plain message stays as it is
        let plain = json!({"role": "tool", "content": "[e3] textbox \"Policy number\""});
        assert_eq!(snapshot_text(&plain), "[e3] textbox \"Policy number\"");
    }

    #[test]
    fn the_plans_are_read_from_the_page_and_picked_by_rule() {
        let plans = plans_in(QUOTE_SNAPSHOT);
        assert_eq!(
            plans.iter().map(|p| p.name.as_str()).collect::<Vec<_>>(),
            ["Premium", "Business", "First"]
        );
        assert_eq!((plans[1].link.as_str(), plans[1].premium), ("e7", 47.0));
        assert_eq!(plans[1].cover_of("medical"), Some(1_000_000.0));
        assert_eq!(plans[1].cover_of("cancellation"), Some(10_000.0));
        assert_eq!(plans[1].cover_of("baggage"), Some(5_000.0));
        let (p, rule) = pick_plan(&plans, &Some(("cancellation".into(), 10_000.0))).unwrap();
        assert_eq!(
            (p.name.as_str(), rule.as_str()),
            (
                "Business",
                "the cheapest plan with cancellation cover of at least 10,000"
            )
        );
        let (p, _) = pick_plan(&plans, &Some(("medical".into(), 1_200_000.0))).unwrap();
        assert_eq!(p.name, "First");
        let (p, rule) = pick_plan(&plans, &None).unwrap();
        assert_eq!(
            (p.name.as_str(), rule.as_str()),
            ("Premium", "the cheapest plan")
        );
        let (p, rule) = pick_plan(&plans, &Some(("medical".into(), 9_000_000.0))).unwrap();
        assert!(p.name == "Premium" && rule.starts_with("no plan has medical cover"));
        // the trip and the rule from the request and the booking
        let today = NaiveDate::from_ymd_opt(2026, 10, 31).unwrap();
        let booking = msg("b1", "Singapore Airlines <noreply@singaporeair.example>", "Booking confirmed — SQ 636 Singapore to Tokyo, 2026-11-24",
            "Passenger TAN WEI MING. SQ 636 SIN 10:30 → HND 18:25 on 2026-11-24; SQ 637 HND → SIN on 2026-12-01. Booking reference 7XKQ2P.");
        let q = trip_quote(&[booking], &["I need travel insurance from FWD for my trip next month, just me — the flight booking is in my email. Get me the cheapest plan with at least SGD 10,000 trip cancellation and fill in the application with my card from the Vault; I'll press Pay myself.".into()], today);
        assert_eq!(
            (
                q.destination.as_str(),
                q.depart.as_str(),
                q.ret.as_str(),
                q.travellers
            ),
            ("Japan", "2026-11-24", "2026-12-01", 1)
        );
        assert_eq!(q.minimum, Some(("cancellation".into(), 10_000.0)));
        // the Vault's cards, and the one the user named
        let listing = "- id=v1 · DBS Altitude Visa Signature · issued in Singapore · card · •••• UNKNOWN · fields: number, expiry, cvc, holder · number, expiry, cvc UNKNOWN (name only: the user adds them in Memory → Vault before it can be filled or paid with)\n- id=v2 · Visa Wei Ming · card · •••• 1111 · fields: number, expiry, cvc, holder · only on: fwd.com.sg\n- id=v3 · Passport Wei Ming · id_document · •••• 567Z · fields: number, expiry";
        let cards = vault_cards(listing);
        assert_eq!(cards.len(), 2);
        assert!(cards[0].name_only && !cards[1].name_only);
        assert_eq!(
            pick_card(&cards, &["pay with my DBS Altitude card".into()]).map(|c| c.id.as_str()),
            Some("v1")
        );
        assert_eq!(
            pick_card(&cards, &["with my card from the Vault".into()]).map(|c| c.id.as_str()),
            Some("v2")
        );
    }

    #[test]
    fn the_purchase_steps_follow_the_transcript() {
        let s = spec(Flow::FwdSgBuy);
        let user = vec!["I need travel insurance from FWD for my trip to Tokyo next month, just me. Get me the cheapest plan with at least SGD 10,000 trip cancellation and fill in the application with my card from the Vault; I'll press Pay myself. Flying 2026-11-24, back 2026-12-01.".to_string()];
        let profile = json!({"name_en": "TAN WEI MING", "birthday": "1985-04-12", "email_personal": "weiming@example.com", "phone": "+65 9123 4567"});
        let w = world(&user, &profile, false);
        let mut t = vec![json!({"role": "user", "content": user[0]})];
        t.extend(step(
            "k1",
            "load_skill",
            json!({"name": "fwd-travel-insurance"}),
            "---",
        ));
        assert_eq!(active(&t).map(|s| s.flow), Some(Flow::FwdSgBuy));
        let mut n = 1;
        let mut run = |t: &mut Vec<Value>, name: &str, result: &str| -> Value {
            let Step::Calls(c) = next(s, t, &w) else {
                panic!("expected {name}")
            };
            assert_eq!(c[0].name, name, "{:?}", c[0].args);
            t.extend(step(&format!("clm_{n}"), name, c[0].args.clone(), result));
            n += 1;
            c[0].args.clone()
        };
        // no mail: the quote page, the trip typed, the plans read, the comparison shown with the pick, the plan clicked
        run(&mut t, "browser_navigate", "<untrusted_content source=\"web page https://www.fwd.com.sg/travel-insurance/online-quote/\">{}</untrusted_content>");
        let a = run(
            &mut t,
            "form_fill",
            "{\"url\": \"https://www.fwd.com.sg/travel-insurance/online-quote/\", \"filled\": []}",
        );
        assert_eq!(
            (
                sget(&a, "destination").as_str(),
                sget(&a, "depart").as_str(),
                sget(&a, "return").as_str(),
                sget(&a, "travellers").as_str()
            ),
            ("Japan", "2026-11-24", "2026-12-01", "1")
        );
        // the snapshot as the sentinel returns it (the JSON envelope): the gate on b24962d read no plan from it
        run(
            &mut t,
            "browser_snapshot",
            &page(
                "https://www.fwd.com.sg/travel-insurance/online-quote/",
                QUOTE_SNAPSHOT,
            ),
        );
        let a = run(
            &mut t,
            "present_choices",
            "Shown to the user as 3 cards; the user's request already picked \"Business\"",
        );
        assert_eq!(
            (sget(&a, "picked").as_str(), sget(&a, "kind").as_str()),
            ("Business", "comparison")
        );
        assert_eq!(a["options"][1]["details"][0], "SGD 47");
        let a = run(&mut t, "browser_click", "<untrusted_content source=\"web page https://www.fwd.com.sg/fwd_sg_apply.html?plan=Business&premium=47.00\">{}</untrusted_content>");
        assert_eq!(sget(&a, "ref"), "e7");
        // the application, the Vault, the purchase approved, the card filled field by field, the hand-over for Pay
        let a = run(
            &mut t,
            "form_fill",
            "{\"url\": \"https://www.fwd.com.sg/fwd_sg_apply.html?plan=Business\", \"filled\": []}",
        );
        assert_eq!(sget(&a, "stage"), "apply");
        run(&mut t, "vault_list", "- id=v2 · Visa Wei Ming · card · •••• 1111 · fields: number, expiry, cvc, holder · only on: fwd.com.sg");
        let a = run(
            &mut t,
            "purchase_confirm",
            "{\"status\": \"ok\", \"purchase_id\": \"p1\"}",
        );
        assert_eq!(
            (
                sget(&a, "card_item_id").as_str(),
                a["total"].as_f64(),
                sget(&a, "currency").as_str()
            ),
            ("v2", Some(47.0), "SGD")
        );
        assert!(sget(&a["items"][0], "name").starts_with("Business travel insurance, Japan"));
        run(&mut t, "browser_snapshot", &page("https://www.fwd.com.sg/fwd_sg_apply.html?plan=Business", "[e11] textbox \"Card number\" value=[hidden]\n[e12] textbox \"Expiry\" placeholder=\"MM/YY\" value=[hidden]\n[e13] textbox \"CVC\" value=[hidden]\n[e14] textbox \"Name on card\" value=\"\"\n[e15] checkbox \"I have read the policy wording and the declarations.\" [unchecked]\n[e16] button \"Pay now\""));
        for (field, r) in [
            ("number", "e11"),
            ("expiry", "e12"),
            ("cvc", "e13"),
            ("holder", "e14"),
        ] {
            let a = run(&mut t, "browser_fill_secret", "{\"filled\": \"ok\"}");
            assert_eq!(
                (
                    sget(&a, "field").as_str(),
                    sget(&a, "ref").as_str(),
                    sget(&a, "item_id").as_str()
                ),
                (field, r, "v2")
            );
        }
        let a = run(&mut t, "browser_request_takeover", BACK);
        let reason = sget(&a, "reason");
        assert!(
            reason.contains("Business plan (SGD 47.00)")
                && reason.contains("declaration questions are yours")
                && reason.contains("press Pay now"),
            "{reason}"
        );
        run(&mut t, "browser_snapshot", "<untrusted_content source=\"web page https://www.fwd.com.sg/fwd_sg_apply.html?plan=Business\">Your details. Pay now</untrusted_content>");
        let Step::Final(a) = next(s, &t, &w) else {
            panic!()
        };
        assert!(
            a.contains("Business, SGD 47.00")
                && a.contains("SGD 34.00")
                && a.contains("SGD 68.00")
                && a.contains("nothing has been paid")
                && a.contains("cheapest plan with cancellation cover of at least 10,000"),
            "{a}"
        );
        // a card by its name only: no purchase_confirm, the hand-over says the card is the user's to type
        let mut t2 = t[..t.len() - 18].to_vec();
        let Step::Calls(c) = next(s, &t2, &w) else {
            panic!()
        };
        assert_eq!(c[0].name, "vault_list");
        t2.extend(step("clm_v", "vault_list", json!({}), "- id=v1 · DBS Altitude Visa Signature · issued in Singapore · card · •••• UNKNOWN · fields: number, expiry, cvc, holder · number, expiry, cvc UNKNOWN (name only: the user adds them in Memory → Vault before it can be filled or paid with)"));
        let Step::Calls(c) = next(s, &t2, &w) else {
            panic!()
        };
        assert_eq!(c[0].name, "browser_request_takeover");
        assert!(
            sget(&c[0].args, "reason")
                .contains("DBS Altitude Visa Signature in the Vault has no number, expiry or CVC"),
            "{}",
            sget(&c[0].args, "reason")
        );
    }

    #[test]
    fn the_hk_claim_fills_the_pdf_and_drafts_the_email() {
        let s = spec(Flow::FwdHkClaim);
        let user = vec!["My Cathay Pacific flight CX 543 to Tokyo on 2026-10-29 was delayed more than 6 hours. Please prepare the delay claim on my FWD MyTravel policy: fill in the claim form and put it in an email draft with the airline's delay letter. Don't send it.".to_string()];
        let profile = json!({"name_en": "CHAN TAI MAN", "email_personal": "taiman@example.com", "phone": "+852 9123 4567", "address_home": "Flat 12B, 88 Robinson Road, Mid-Levels, Hong Kong"});
        let w = world(&user, &profile, true);
        let mail = vec![
            msg("p1", "FWD Hong Kong <noreply@fwd.com.hk>", "Your MyTravel Insurance policy MT-HK-2026-00881",
                "Dear CHAN TAI MAN,\n\nYour MyTravel Insurance policy MT-HK-2026-00881 (Premium Plan, single trip to Japan) is underwritten by bolttech Insurance (Hong Kong) Company Limited. Period of insurance: 2026-10-28 to 2026-11-03.\nTravel delay: HKD 300 for every 6 full hours of delay, up to HKD 1,500. Claims must be submitted within 90 days of the incident.\n24-hour emergency assistance: +852 2000 0100.\n\nFWD Financial Limited"),
            Msg { attachments: vec!["delay_letter_CX543.pdf".into()], ..msg("c1", "Cathay Pacific <noreply@cathaypacific.example>", "Delay confirmation — CX 543 on 2026-10-29",
                "Dear CHAN TAI MAN,\n\nCX 543 Hong Kong–Tokyo Narita on 2026-10-29 was scheduled to depart at 09:05 and departed at 15:20 after a technical inspection of the aircraft. Our delay confirmation is attached (reference CX-DLY-55821).\n\nCathay Pacific") },
        ];
        let mut t = vec![json!({"role": "user", "content": user[0]})];
        t.extend(step(
            "k1",
            "load_skill",
            json!({"name": "fwd-travel-insurance"}),
            "---",
        ));
        assert_eq!(active(&t).map(|s| s.flow), Some(Flow::FwdHkClaim));
        let Step::Calls(c) = next(s, &t, &w) else {
            panic!()
        };
        let mut n = 1;
        for (i, call_) in c.iter().enumerate() {
            t.extend(step(
                &format!("clm_{n}"),
                "gmail_search",
                call_.args.clone(),
                &json!({"count": 1, "messages": [{"id": mail[i].id, "subject": mail[i].subject}]})
                    .to_string(),
            ));
            n += 1;
        }
        next(s, &t, &w);
        for m in &mail {
            t.extend(mail_step(&mut n, m));
        }
        let mut run = |t: &mut Vec<Value>, name: &str, result: &str| -> Value {
            let Step::Calls(c) = next(s, t, &w) else {
                panic!("expected {name}")
            };
            assert_eq!(c[0].name, name, "{:?}", c[0].args);
            t.extend(step(&format!("clm_{n}"), name, c[0].args.clone(), result));
            n += 1;
            c[0].args.clone()
        };
        run(&mut t, "gmail_save_attachment", "附件已保存 attachment saved: attachments/delay_letter_CX543.pdf (9 KB, application/pdf).");
        let a = run(&mut t, "browser_navigate", "<untrusted_content source=\"web page https://www.fwd.com.hk/files/x.pdf\">{\"url\": \"https://www.fwd.com.hk/files/x.pdf\", \"pdf\": {\"path\": \"downloads/travel-insurance-clm-form-fillable.pdf\"}}</untrusted_content>");
        assert_eq!(sget(&a, "url"), s.start_url);
        let a = run(&mut t, "pdf_form_fill", "{\"output\": \"attachments/bolttech-travel-claim-MT-HK-2026-00881.pdf\", \"filled\": 14, \"problems\": []}");
        assert_eq!(
            sget(&a, "path"),
            "downloads/travel-insurance-clm-form-fillable.pdf"
        );
        let v = &a["values"];
        assert_eq!(
            (
                sget(v, "fill_17").as_str(),
                sget(v, "Full name").as_str(),
                sget(v, "fill_14_2").as_str()
            ),
            ("MT-HK-2026-00881", "CHAN TAI MAN", "CX 543")
        );
        assert_eq!(
            (
                sget(v, "fill_1").as_str(),
                sget(v, "fill_7").as_str(),
                sget(v, "fill_8").as_str(),
                sget(v, "fill_9_2").as_str()
            ),
            ("6 h 15 min", "300.00", "Cathay Pacific", "CX-DLY-55821")
        );
        assert!(
            v.get("HKID Card no").is_none()
                && v.get("fill_4_5").is_none()
                && v.get("Claimants signature").is_none()
                && v.get("Date").is_none()
        );
        let a = run(&mut t, "send_file", "sent");
        assert_eq!(
            sget(&a, "path"),
            "attachments/bolttech-travel-claim-MT-HK-2026-00881.pdf"
        );
        let a = run(&mut t, "gmail_create_draft", "draft created");
        assert_eq!(sget(&a, "to"), "claims.hk@bolttechinsurance.com");
        assert!(sget(&a, "subject").contains("MT-HK-2026-00881"));
        assert_eq!(
            a["attachments"],
            json!([
                "attachments/bolttech-travel-claim-MT-HK-2026-00881.pdf",
                "attachments/delay_letter_CX543.pdf"
            ])
        );
        assert!(!sget(&a, "body").contains("HKID"));
        let a = run(&mut t, "schedule_create", "created");
        assert_eq!(sget(&a, "spec"), "10080");
        let Step::Final(a) = next(s, &t, &w) else {
            panic!()
        };
        assert!(
            a.contains("bolttech")
                && a.contains("**HKD 300.00**")
                && a.contains("HKID card number")
                && a.contains("signature")
                && a.contains("**not sent**")
                && a.contains("27 January 2027"),
            "{a}"
        );
        assert!(a.contains("+852 2000 0100"));
    }

    #[test]
    fn the_fwd_flow_comes_from_the_words_and_the_mail() {
        let load = |words: &str| {
            let mut t = vec![json!({"role": "user", "content": words})];
            t.extend(step(
                "k1",
                "load_skill",
                json!({"name": "fwd-travel-insurance"}),
                "---",
            ));
            t
        };
        assert_eq!(active(&load("Buy me FWD's cheapest travel insurance plan for my Tokyo trip next month and pay for it with my DBS Altitude card.")).map(|s| s.flow), Some(Flow::FwdSgBuy));
        assert_eq!(
            active(&load(
                "claim my FWD travel insurance for the delayed flight"
            ))
            .map(|s| s.flow),
            Some(Flow::FwdSgClaim)
        );
        assert_eq!(
            active(&load(
                "Please prepare the delay claim on my FWD MyTravel policy"
            ))
            .map(|s| s.flow),
            Some(Flow::FwdHkClaim)
        );
        // a question about the policy, not a request to claim: not driven
        assert!(active(&load("Is my SQ 638 flight to Tokyo today delayed? What are my options, what can I claim from my FWD travel insurance?")).is_none());
        assert!(active(&load("Which FWD plan is the cheapest for Japan?")).is_none());
        // a lost bag or a clinic bill: not driven (the model path)
        assert!(active(&load(
            "file a claim with FWD for the clinic bill in Bangkok"
        ))
        .is_none());
        assert!(active(&load("buy FWD Hong Kong travel insurance for my trip")).is_none());
        // the market from the policy email once mail was read
        let mut t = load("claim the flight delay on my FWD policy");
        t.extend(step(
            "clm_1",
            "gmail_search",
            json!({}),
            "{\"messages\": [{\"id\": \"p1\", \"from\": \"FWD Hong Kong <noreply@fwd.com.hk>\"}]}",
        ));
        assert_eq!(active(&t).map(|s| s.flow), Some(Flow::FwdHkClaim));
        // the skill's own text names bolttech and fwd.com.hk: a Singapore policy email keeps the Singapore claim
        // (the gate on b24962d: "FWD Hong Kong travel delay claim — handled by bolttech" for TI-SG-2026-55123)
        let skill = "---\nname: fwd-travel-insurance\ndescription: FWD travel insurance in Singapore and Hong Kong (Hong Kong claims: bolttech, claims.hk@bolttechinsurance.com, fwd.com.hk)";
        let words = "My flight SQ 636 to Tokyo on 2026-10-30 was delayed by more than 7 hours. Please claim it on my FWD travel insurance — the policy and the airline's delay certificate are in my email.";
        let mut t = vec![json!({"role": "user", "content": words})];
        t.extend(step(
            "k1",
            "load_skill",
            json!({"name": "fwd-travel-insurance"}),
            skill,
        ));
        assert_eq!(active(&t).map(|s| s.flow), Some(Flow::FwdSgClaim));
        t.extend(step("clm_1", "gmail_search", json!({}), "<untrusted_content source=\"gmail_search\">\n{\"count\": 1, \"messages\": [{\"id\": \"i-8\", \"from\": \"FWD Singapore <noreply@fwd.com.sg>\", \"subject\": \"Your FWD Travel Insurance policy TI-SG-2026-55123\"}]}\n</untrusted_content>"));
        t.extend(step("clm_2", "gmail_get_message", json!({"message_id": "i-8"}), "<untrusted_content source=\"email\">\n{\"id\": \"i-8\", \"from\": \"FWD Singapore <noreply@fwd.com.sg>\", \"subject\": \"Your FWD Travel Insurance policy TI-SG-2026-55123\", \"body\": \"Travel delay: SGD 100 for every 6 hours\"}\n</untrusted_content>"));
        assert_eq!(active(&t).map(|s| s.flow), Some(Flow::FwdSgClaim));
        // a policy in Hong Kong dollars from an FWD sender of no country domain: Hong Kong
        let mut t2 = t[..3].to_vec();
        t2.extend(step("clm_2", "gmail_get_message", json!({"message_id": "p"}), "{\"id\": \"p\", \"from\": \"FWD <noreply@fwd.com>\", \"subject\": \"Your MyTravel policy\", \"body\": \"Travel delay: HKD 300 for every 6 hours\"}"));
        assert_eq!(active(&t2).map(|s| s.flow), Some(Flow::FwdHkClaim));
        // the arrival-card skill is not a claim
        assert!(active(&step(
            "k1",
            "load_skill",
            json!({"name": "sg-arrival-card"}),
            "---"
        ))
        .is_none());
    }

    #[test]
    fn amounts_and_money() {
        assert_eq!(
            amounts_in("Fare USD 1,840.00, taxes $86.40 and 250.00 SGD"),
            vec![1840.0, 86.40, 250.0]
        );
        assert_eq!(money(1753.6), "1,753.60");
        assert_eq!(money(1_000_000.0), "1,000,000.00");
        assert_eq!(money(47.0), "47.00");
        assert!(amounts_in("Visa ending 4321 on 2026-09-21").is_empty());
    }
}
