//! The claims machine on workflow tools (docs/json-tools.md): `claim_file` (the Chase Sapphire trip cancellation /
//! interruption claim on eClaimsLine, the FWD flight-delay claim — Singapore's online form, Hong Kong's bolttech PDF
//! and email draft), `insurance_buy` (FWD Singapore's quote, plan pick and application; the user pays) and `bag_trace`
//! (the delayed bag's status on WorldTracer, `super::bagtrace`).
//!
//! The model reads the user's words and the mails (natural language) and passes the facts as the tool's JSON
//! arguments; code validates them ([`check`]), reads the mail with fixed searches when facts are missing (the result
//! `mail_read` lists the emails; the model calls the tool again with the facts from them), and runs every step: the
//! benefit's amount by arithmetic, the deadlines, the form filled by its function (`crate::forms`, fixed ids), the
//! user's own steps and the final Submit / Pay handed over, the reference read from the confirmation page's fixed
//! parts, the reminders. The machine's JSON result carries `hints` (deadlines, amounts, the status, the reference);
//! the model writes the reply from it. As in [`super::booking`], the next step is a pure function of the validated
//! arguments and the transcript after the tool call, so a run suspended for a takeover carries on where it was.
//!
//! Never decided by code: the health and declaration questions of an application (left for the user and named in the
//! hand-over), the final click (Submit, Pay now), the signature and the HKID / bank details on a PDF form.

use std::sync::LazyLock;

use chrono::{Datelike, Duration, NaiveDate};
use omuse_common::util::{get_str, truthy};
use regex::Regex;
use serde_json::{json, Map, Value};

use super::driver::{handed_back, json_of, pos, results_of, stopped, walk_messages, Step};
use super::show as S;
use super::tools::fn_;
use super::util::{content_of, sget, stored_call, tool_calls};
use super::workflow::{self, Checked};
use crate::llm::ToolCall;

/// The id prefix of the machine's code calls.
pub const PREFIX: &str = "clm_";
pub const CLAIM_FILE: &str = "claim_file";
pub const INSURANCE_BUY: &str = "insurance_buy";
pub const BAG_TRACE: &str = "bag_trace";
/// The workflow tools this machine runs.
pub const TOOLS: [&str; 3] = [CLAIM_FILE, INSURANCE_BUY, BAG_TRACE];
/// The result status of a run that read the mail for missing facts: the model calls the tool again with them (the
/// workflow tools stay offered on that turn, `workflow::reply_turn`).
pub const MAIL_READ: &str = "mail_read";

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
    /// the delayed bag's status on WorldTracer, by the PIR's file reference (`super::bagtrace`)
    BagTrace,
}

/// One claim or purchase, as data.
pub struct ClaimSpec {
    pub skill: &'static str,
    pub flow: Flow,
    /// the claim's name in the result
    pub name: &'static str,
    /// who decides the claim
    pub decides: &'static str,
    /// the page to open (the claim form, the quote, the PDF)
    pub start_url: &'static str,
    /// its host, to know the page was reached
    pub host: &'static str,
    /// the fixed mail searches; `{flight}` is the flight number's words when the arguments name one
    pub searches: &'static [&'static str],
    /// hits of each search to read
    pub hits: usize,
    /// days after the loss to notify the insurer (the policy's own limit, when the model read it, wins)
    pub notify_days: i64,
    /// days after the loss for the documents (0 = none)
    pub documents_days: i64,
    /// what the user does after the review hand-over
    pub user_steps: &'static str,
    /// the confirmation's reference element on the page after the user's Submit / Pay (a CSS selector, read by
    /// `browser_read_elements`); empty = none
    pub ref_sel: &'static str,
    /// the reference's format, checked over that element's whole value
    pub ref_re: &'static str,
    pub currency: &'static str,
    /// a delay benefit when the policy's own rule was not read: (amount per block, hours per block, the cap)
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
const BAG_SEARCHES: &[&str] = &[
    "(baggage OR bag OR luggage OR PIR OR \"property irregularity\" OR WorldTracer) newer_than:30d",
];

pub const SPECS: &[ClaimSpec] = &[
    ClaimSpec {
        skill: "csr-trip-claim",
        flow: Flow::CsrTrip,
        name: "Chase Sapphire trip claim",
        decides: "Card Benefit Services (Chase's benefit administrator)",
        // www.eclaimsline.com redirects here since 2026 (the old portal's claim page is gone)
        start_url: "https://cardclaimcenter.com/internal/log-in",
        host: "cardclaimcenter.com",
        searches: CSR_SEARCHES,
        hits: 5,
        notify_days: 20,
        documents_days: 90,
        user_steps: "sign in to Card Claim Center (or create your account), file the claim and press Submit",
        ref_sel: "",
        ref_re: "",
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
        user_steps: "press next (FWD checks the policy on its server), give the bank account for the payment, then the incident with the delay certificate, and press Submit",
        ref_sel: "",
        ref_re: "",
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
        user_steps: "answer FWD's eligibility statements (\"Yes, continue\" only if they are all true for you), choose any add-on, check your details, read the policy wording and press Pay",
        ref_sel: "",
        ref_re: "",
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
        ref_sel: "",
        ref_re: "",
        currency: "HKD",
        delay: (300.0, 6, 1_500.0),
        caps: &[],
        claims_email: "claims.hk@bolttechinsurance.com",
    },
    ClaimSpec {
        skill: super::bagtrace::SKILL,
        flow: Flow::BagTrace,
        name: "WorldTracer baggage tracing",
        decides: "the airline's baggage services",
        start_url: super::bagtrace::TRACE_URL,
        host: super::bagtrace::TRACE_HOST,
        searches: BAG_SEARCHES,
        hits: 3,
        notify_days: 0,
        documents_days: 0,
        user_steps: "",
        ref_sel: "",
        ref_re: "",
        currency: "",
        delay: (0.0, 0, 0.0),
        caps: &[],
        claims_email: "",
    },
];

/// FWD Hong Kong's MyTravel plan page (no form function: its quote goes to the `browser-use` skill).
const HK_QUOTE_URL: &str = "https://www.fwd.com.hk/online-insurance/mytravel-insurance/en/";

/// Card Claim Center's sign-in page by its password box (a fixed part of the page): still there after the hand-back =
/// the user did not sign in.
const CCC_PASSWORD: &str = "input[type=password]";

fn spec(flow: Flow) -> &'static ClaimSpec {
    SPECS.iter().find(|s| s.flow == flow).unwrap()
}

// ---------------------------------------------------------------- the workflow tools' schemas

/// The catalog entries of the workflow tools (JSON schemas; the descriptions say what the tool does).
pub fn tool_schemas() -> Vec<Value> {
    let s = json!({"type": "string"});
    let date = |what: &str| json!({"type": "string", "description": format!("YYYY-MM-DD: {what}")});
    let money = |what: &str| json!({"type": "number", "description": what});
    let mut claim_props = json!({
        "insurer": {"type": "string", "enum": ["chase_sapphire", "fwd"]},
        "benefit": {"type": "string", "enum": ["trip_cancellation", "trip_interruption", "flight_delay", "other"],
                    "description": "Chase: trip_cancellation when the trip had not started, trip_interruption when it had started and was cut short; FWD: flight_delay (other FWD claims: other)"},
        "market": {"type": "string", "enum": ["sg", "hk"], "description": "FWD: the policy's market (the policy email's sender fwd.com.sg / fwd.com.hk / bolttech, or the user's words)"},
        "incident_date": date("the day the trip was cancelled or cut short (Chase), or the delayed flight's date (FWD)"),
        "reason": {"type": "string", "enum": ["sickness", "injury", "death", "severe_weather", "quarantine", "other"], "description": "Chase: the covered reason in general terms (never the diagnosis)"},
        "who": {"type": "string", "enum": ["cardholder", "family_member", "travelling_companion"], "description": "Chase: whose sickness or injury"},
        "travellers": {"type": "integer", "description": "the travellers on the claim"},
        "paid": money("Chase: what was paid for the prepaid bookings (USD), from the booking and charge emails"),
        "refunded": money("Chase: what the airline / hotel refunded or credited (USD)"),
        "non_refundable": money("Chase: what the suppliers' letters state as not refunded (USD)"),
        "change_fee": money("Chase: a change fee or fare difference paid to fly home early (USD)"),
        "bookings": {"type": "array", "description": "Chase: the suppliers and their booking references",
                     "items": {"type": "object", "properties": {"supplier": s.clone(), "reference": s.clone()}}},
        "trip": {"type": "array", "items": s.clone(), "description": "Chase: the flights and stays (\"UA 837 SFO–NRT\", \"Park Hyatt Tokyo\")"},
        "card_product": {"type": "string", "enum": ["Chase Sapphire Reserve", "Chase Sapphire Preferred"]},
        "card_last4": {"type": "string", "description": "Chase: the card's last 4 digits from the charge email (never a full number)"},
        "doctor_letter": {"type": "boolean", "description": "the user has a doctor's letter"},
        "supplier_letter": {"type": "boolean", "description": "a supplier's written refund / non-refundable statement is in the mail"},
        "policy_no": {"type": "string", "description": "FWD: the policy number from the policy email"},
        "plan": {"type": "string", "description": "FWD: the plan name from the policy email"},
        "flight_no": {"type": "string", "description": "the delayed flight (\"SQ 636\")"},
        "from": {"type": "string", "description": "FWD: the delayed flight's departure city"},
        "to": {"type": "string", "description": "FWD: its arrival city"},
        "airline": {"type": "string", "description": "FWD: the airline that wrote the delay letter"},
        "airline_ref": {"type": "string", "description": "FWD: the airline letter's reference"},
        "scheduled_departure": {"type": "string", "description": "FWD: HH:MM, the scheduled departure from the airline's delay letter"},
        "actual_departure": {"type": "string", "description": "FWD: HH:MM, the actual departure"},
        "delay_reason": {"type": "string", "description": "FWD: the cause the airline gives"},
        "benefit_per_block": money("FWD: the policy's delay benefit per block of hours (\"SGD 100 for every 6 hours\" → 100)"),
        "block_hours": {"type": "integer", "description": "FWD: the hours per block (6)"},
        "benefit_cap": money("FWD: the delay benefit's cap (\"up to SGD 1,500\" → 1500)"),
        "claim_within_days": {"type": "integer", "description": "FWD: the policy's time limit for a claim in days, when the policy email states one"},
        "emergency_phone": {"type": "string", "description": "FWD: the 24-hour emergency line from the policy email"},
        "certificate_message_id": {"type": "string", "description": "FWD: the message_id of the airline's delay email (its PDF is attached to the claim)"}
    });
    // FWD Singapore's online claim (outside the literal above: json!'s recursion limit)
    claim_props["leave_sg"] = date("FWD Singapore: the day the trip left Singapore");
    claim_props["return_sg"] = date("FWD Singapore: the day the trip was to return to Singapore");
    claim_props["destination"] =
        json!({"type": "string", "description": "FWD Singapore: the country travelled to"});
    claim_props["itinerary_file"] = json!({"type": "string", "description": "FWD Singapore: the itinerary or e-ticket, its path in this chat (uploads/… or attachments/…)"});
    claim_props["boarding_pass_file"] = json!({"type": "string", "description": "FWD Singapore: the (re-scheduled) boarding pass, its path in this chat"});
    claim_props["airline_letter_file"] = json!({"type": "string", "description": "FWD Singapore: the airline's letter stating the hours of delay and the reason, its path in this chat"});
    claim_props["other_file"] = json!({"type": "string", "description": "FWD Singapore: another supporting document, its path in this chat"});
    let claim = fn_(
        CLAIM_FILE,
        "File an insurance claim: the Chase Sapphire Reserve / Preferred trip cancellation or interruption claim \
(eClaimsLine), or an FWD travel insurance flight-delay claim (Singapore: FWD's online form; Hong Kong: bolttech's claim \
form and an email draft). Pass the facts you read in the user's words and in their emails (the booking, the charge, the \
refund letters, the policy, the airline's delay letter). Code reads the mail with fixed searches when facts are missing \
(status mail_read: the emails are above — call claim_file again with every fact from them), computes the amount and the \
deadlines and fills the form from the profile and the Vault. FWD Singapore: code fills every step of FWD's online claim \
(the documents of this chat, PayNow by the Vault's NRIC/FIN) and submits it once the user approves it in the app — pass \
the policy number (from the policy email, else ask), the trip's dates and country and the documents' paths; the result \
has FWD's claim page and says FWD emails the claim link. status needs_input: ask the user for the missing facts, then call \
again with every field; invalid: fix the arguments and call again. Write the \
reply from the JSON result, using its hints (code appends the facts in show to the reply).",
        claim_props,
        &["insurer"],
    );
    let buy = fn_(
        INSURANCE_BUY,
        "Quote and buy FWD travel insurance (Singapore: code fills FWD's quote, reads the plans, picks one by the \
user's rule, shows them as choice cards and, for buy, saves the quote with FWD and gives FWD's own link to it, which \
the user opens on any device — the eligibility statements, the details and Pay are the user's; Hong Kong: the plan page goes to the browser-use skill). Pass the trip from the user's words and their \
booking emails; code reads the booking mail when the trip is missing (status mail_read: call insurance_buy again with \
the trip from those emails). needs_input: ask the user, then call again with every field. Write the reply from the \
JSON result, using its hints (code appends the facts in show to the reply).",
        json!({
            "market": {"type": "string", "enum": ["sg", "hk"], "description": "where the user lives and buys (FWD Singapore or FWD Hong Kong)"},
            "action": {"type": "string", "enum": ["compare", "buy"], "description": "compare: show the plans only; buy: also fill the application"},
            "destination": {"type": "string", "description": "the destination COUNTRY's ISO 3166 2-letter code (JP for a trip to Tokyo)"},
            "depart": date("the departure day"),
            "return": date("the return day"),
            "travellers": {"type": "integer"},
            "cover": {"type": "string", "enum": ["medical", "cancellation", "baggage", "delay"], "description": "the cover the user wants at least an amount of"},
            "cover_at_least": money("that cover's minimum (\"at least SGD 10,000 trip cancellation\" → 10000)"),
            "plan": {"type": "string", "description": "the plan the user chose: its name exactly as the plans card listed it"},
            "card": {"type": "string", "description": "the Vault card the user named to pay with: its id from vault_list (id=…), its card_id, or its label exactly as listed"},
            "title": {"type": "string", "enum": ["Mr.", "Miss", "Mrs."], "description": "Hong Kong: the insured's title, from the profile or the conversation"},
            "hkid_issue_date": {"type": "string", "description": "Hong Kong: YYYY-MM-DD, the date of issue on the insured's new Smart HKID card (asked)"},
            "family_name": {"type": "string", "description": "Hong Kong: the insured's family name as on the HKID (\"CHAN\" of \"CHAN TAI MAN\")"},
            "given_names": {"type": "string", "description": "Hong Kong: the insured's given names as on the HKID (\"TAI MAN\")"}
        }),
        &[],
    );
    let bag = fn_(
        BAG_TRACE,
        "Look up a delayed bag on WorldTracer by the Property Irregularity Report's file reference: code fills the \
tracing form and reads the status from the result page (a lookup changes nothing) and, with watch, sets a check twice a \
day. Pass the file reference from the user's words or the PIR email; code reads the baggage mail when it is missing \
(status mail_read: call bag_trace again with it). Write the reply from the JSON result, using its hints \
(code appends the facts in show to the reply); drafts and advice from the skill follow as usual.",
        json!({
            "file_ref": {"type": "string", "description": "the PIR's file reference (\"NRTSQ12345\")"},
            "family_name": {"type": "string", "description": "the passenger's family name as on the PIR"},
            "flight_date": date("the day the bag should have arrived (its flight)"),
            "watch": {"type": "boolean", "description": "the user asked to keep an eye on the bag"}
        }),
        &[],
    );
    vec![claim, buy, bag]
}

/// `ask_user` in a claim skill's run before its workflow tool was called: refused, as data. What the user must give is
/// the tool's `needs_input` (after code read the mail); so the tool is called first. `events` are the task's.
pub fn ask_before_tool(events: &[Value]) -> Option<Value> {
    let skill = events
        .iter()
        .filter(|e| get_str(e, "type") == "tool_call")
        .filter(|e| get_str(&e["data"], "name") == "load_skill")
        .map(|e| get_str(&e["data"]["args"], "name"))
        .next_back()?;
    let tool = match skill.as_str() {
        "csr-trip-claim" => CLAIM_FILE,
        "fwd-travel-insurance" => "claim_file or insurance_buy",
        _ => return None,
    };
    let called = events.iter().any(|e| {
        get_str(e, "type") == "workflow_call"
            && TOOLS.contains(&get_str(&e["data"], "tool").as_str())
    });
    if called {
        return None;
    }
    Some(json!({"refused": "call_the_tool_first", "tool": tool,
                "why": format!("the {skill} skill asks through {tool}: call it with what the user gave; code reads the mail, and its needs_input lists what only the user can give")}))
}

// ---------------------------------------------------------------- validation (code, on the JSON)

/// What validation reads besides the arguments.
pub struct Ctx<'a> {
    pub profile: &'a Value,
    pub today: NaiveDate,
    /// a mail account is connected (code reads it for missing facts)
    pub mail_ready: bool,
    /// the transcript: whether code already read the mail for this tool (a `mail_read` result) and the policy email's
    /// sender domain (structured data)
    pub transcript: &'a [Value],
}

static HHMM: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^([01]?\d|2[0-3])[:.]([0-5]\d)$").unwrap());
static FLIGHT_NO: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^([A-Z][A-Z0-9]|[0-9][A-Z])\s?(\d{1,4})$").unwrap());
static LAST4: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^\d{4}$").unwrap());

fn s_arg(a: &Value, k: &str) -> String {
    match a.get(k) {
        Some(Value::String(s)) => s.trim().to_string(),
        Some(Value::Number(n)) => n.to_string(),
        _ => String::new(),
    }
}

fn n_arg(a: &Value, k: &str) -> Option<f64> {
    match a.get(k)? {
        Value::Number(n) => n.as_f64(),
        Value::String(s) => s.trim().replace(',', "").parse().ok(),
        _ => None,
    }
    .filter(|x: &f64| x.is_finite())
}

fn i_arg(a: &Value, k: &str) -> Option<i64> {
    n_arg(a, k).filter(|x| x.fract() == 0.0).map(|x| x as i64)
}

fn b_arg(a: &Value, k: &str) -> bool {
    a.get(k).is_some_and(truthy)
}

fn parse_iso(s: &str) -> Option<NaiveDate> {
    NaiveDate::parse_from_str(s, "%Y-%m-%d").ok()
}

fn iso(d: NaiveDate) -> String {
    d.format("%Y-%m-%d").to_string()
}

fn long(d: NaiveDate) -> String {
    d.format("%-d %B %Y").to_string()
}

/// Validates a workflow tool's arguments: the arguments to run with (normalised), the problems the model can fix, or
/// the facts only the user can give (asked before any page opens). When facts are missing and the mail is connected
/// but not read yet for this tool, the run reads it first (`_read_mail`).
pub fn check(tool: &str, args: &Value, cx: &Ctx) -> Checked {
    let (out, problems, missing) = match tool {
        INSURANCE_BUY => check_buy(args, cx),
        BAG_TRACE => check_bag(args),
        _ => check_claim(args, cx),
    };
    if !problems.is_empty() {
        return Checked::Invalid(problems);
    }
    let mut out = out;
    if !missing.is_empty() {
        if cx.mail_ready && !mail_read(cx.transcript, tool) {
            out.insert("_read_mail".into(), json!(true));
            out.insert("_missing".into(), json!(missing));
            return Checked::Valid(Value::Object(out));
        }
        return Checked::NeedsInput(missing);
    }
    Checked::Valid(Value::Object(out))
}

type Checks = (Map<String, Value>, Vec<String>, Vec<String>);

/// A date argument: kept when it parses, a problem when it does not or (`past`) lies in the future.
fn date_arg(
    a: &Value,
    k: &str,
    today: NaiveDate,
    past: Option<bool>,
    out: &mut Map<String, Value>,
    problems: &mut Vec<String>,
) -> Option<NaiveDate> {
    let v = s_arg(a, k);
    if v.is_empty() {
        return None;
    }
    let Some(d) = parse_iso(&v) else {
        problems.push(format!("{k}: \"{v}\" is not a date: give YYYY-MM-DD"));
        return None;
    };
    match past {
        Some(true) if d > today => {
            problems.push(format!(
                "{k}: {v} is in the future — give the day it happened"
            ));
            None
        }
        Some(false) if d < today => {
            problems.push(format!("{k}: {v} is in the past"));
            None
        }
        _ => {
            out.insert(k.into(), json!(iso(d)));
            Some(d)
        }
    }
}

/// An amount argument (0 or more), kept when given.
fn money_arg(a: &Value, k: &str, out: &mut Map<String, Value>, problems: &mut Vec<String>) {
    if let Some(x) = n_arg(a, k) {
        if x < 0.0 {
            problems.push(format!("{k}: {x} — an amount is 0 or more"));
        } else {
            out.insert(k.into(), json!((x * 100.0).round() / 100.0));
        }
    }
}

fn text_arg(a: &Value, k: &str, out: &mut Map<String, Value>) {
    let v = s_arg(a, k);
    if !v.is_empty() {
        out.insert(k.into(), json!(v));
    }
}

/// "SQ636" → "SQ 636"; `None` when it is no flight number.
fn flight_no(v: &str) -> Option<String> {
    let v = v.trim().to_uppercase();
    FLIGHT_NO
        .captures(&v)
        .map(|c| format!("{} {}", &c[1], &c[2]))
}

/// "9.05" / "09:05" → "09:05".
fn hhmm(v: &str) -> Option<String> {
    HHMM.captures(v.trim())
        .map(|c| format!("{:02}:{}", c[1].parse::<u32>().unwrap_or(0), &c[2]))
}

/// The market of the FWD policy from the structured sender of an FWD / bolttech email the run read (`fwd.com.hk` or
/// bolttech: Hong Kong; `fwd.com.sg`: Singapore); `None` when there is none.
fn market_from_mail(transcript: &[Value]) -> Option<&'static str> {
    let mut mails: Vec<Value> = Vec::new();
    for tool in ["gmail_search", "gmail_get_message"] {
        for (_, m) in results_of(transcript, tool) {
            walk_messages(&json_of(m), &mut mails);
            let v = json_of(m);
            if v.get("from").is_some() && v.get("id").is_none() {
                mails.push(v);
            }
        }
    }
    mails.iter().find_map(|v| {
        let from = get_str(v, "from").to_lowercase();
        if !(from.contains("fwd") || from.contains("bolttech")) {
            return None;
        }
        let domain = from
            .rsplit('@')
            .next()
            .unwrap_or("")
            .trim_end_matches('>')
            .trim();
        if domain.ends_with(".hk") || domain.contains("bolttech") {
            Some("hk")
        } else if domain.ends_with(".sg") {
            Some("sg")
        } else {
            None
        }
    })
}

/// The market from the profile's structured home country (`home_country`, else the home city's), else Singapore.
fn market_from_profile(profile: &Value) -> &'static str {
    if crate::places::home_country(profile) == "HK" {
        "hk"
    } else {
        "sg"
    }
}

fn check_claim(a: &Value, cx: &Ctx) -> Checks {
    let mut out = Map::new();
    let mut problems = Vec::new();
    let mut missing = Vec::new();
    let insurer = s_arg(a, "insurer");
    if !matches!(insurer.as_str(), "chase_sapphire" | "fwd") {
        problems.push("insurer: chase_sapphire or fwd".into());
        return (out, problems, missing);
    }
    out.insert("insurer".into(), json!(insurer));
    let incident = date_arg(
        a,
        "incident_date",
        cx.today,
        Some(true),
        &mut out,
        &mut problems,
    );
    if let Some(n) = i_arg(a, "travellers") {
        if (1..=9).contains(&n) {
            out.insert("travellers".into(), json!(n));
        } else {
            problems.push(format!("travellers: {n} — 1 to 9"));
        }
    }
    if insurer == "chase_sapphire" {
        let benefit = s_arg(a, "benefit");
        match benefit.as_str() {
            "trip_cancellation" | "trip_interruption" => {
                out.insert("benefit".into(), json!(benefit));
            }
            "" => missing.push("benefit".into()),
            other => problems.push(format!(
                "benefit: \"{other}\" — a Chase Sapphire trip claim is trip_cancellation or trip_interruption"
            )),
        }
        for k in ["paid", "refunded", "non_refundable", "change_fee"] {
            money_arg(a, k, &mut out, &mut problems);
        }
        let paid = n_arg(a, "paid").unwrap_or(0.0);
        let refunded = n_arg(a, "refunded").unwrap_or(0.0);
        if paid > 0.0 && refunded > paid + 0.005 {
            problems.push(format!(
                "refunded: {refunded} is more than paid {paid} — check the amounts"
            ));
        }
        if paid <= 0.0 && n_arg(a, "non_refundable").unwrap_or(0.0) <= 0.0 {
            missing.push("paid (what was paid for the trip) and refunded".into());
        }
        if incident.is_none() && !problems.iter().any(|p| p.starts_with("incident_date")) {
            missing.push("incident_date (the day the trip was cancelled or cut short)".into());
        }
        out.insert(
            "reason".into(),
            json!(match s_arg(a, "reason") {
                r if r.is_empty() => "sickness".to_string(),
                r => r,
            }),
        );
        out.insert(
            "who".into(),
            json!(match s_arg(a, "who") {
                r if r.is_empty() => "cardholder".to_string(),
                r => r,
            }),
        );
        out.insert(
            "card_product".into(),
            json!(match s_arg(a, "card_product") {
                r if r.is_empty() => "Chase Sapphire Reserve".to_string(),
                r => r,
            }),
        );
        let last4 = s_arg(a, "card_last4");
        if LAST4.is_match(&last4) {
            out.insert("card_last4".into(), json!(last4));
        }
        let bookings: Vec<Value> = a
            .get("bookings")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|b| {
                let (s, r) = (s_arg(b, "supplier"), s_arg(b, "reference"));
                (!s.is_empty() && !r.is_empty()).then(|| json!({"supplier": s, "reference": r}))
            })
            .collect();
        if !bookings.is_empty() {
            out.insert("bookings".into(), json!(bookings));
        }
        let trip: Vec<String> = a
            .get("trip")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .map(|x| x.trim().to_string())
            .filter(|x| !x.is_empty())
            .collect();
        if !trip.is_empty() {
            out.insert("trip".into(), json!(trip));
        }
        for k in ["doctor_letter", "supplier_letter"] {
            out.insert(k.into(), json!(b_arg(a, k)));
        }
        return (out, problems, missing);
    }
    // FWD
    let benefit = match s_arg(a, "benefit").as_str() {
        "" | "flight_delay" => "flight_delay".to_string(),
        "other" => "other".to_string(),
        other => {
            problems.push(format!(
                "benefit: \"{other}\" — an FWD claim here is flight_delay (or other)"
            ));
            return (out, problems, missing);
        }
    };
    out.insert("benefit".into(), json!(benefit));
    let market = match s_arg(a, "market").as_str() {
        m @ ("sg" | "hk") => m.to_string(),
        _ => market_from_mail(cx.transcript)
            .unwrap_or_else(|| market_from_profile(cx.profile))
            .to_string(),
    };
    out.insert("market".into(), json!(market));
    if benefit == "other" {
        return (out, problems, missing);
    }
    for k in [
        "policy_no",
        "plan",
        "from",
        "to",
        "airline",
        "airline_ref",
        "delay_reason",
        "emergency_phone",
        "certificate_message_id",
    ] {
        text_arg(a, k, &mut out);
    }
    let f = s_arg(a, "flight_no");
    if !f.is_empty() {
        match flight_no(&f) {
            Some(x) => {
                out.insert("flight_no".into(), json!(x));
            }
            None => problems.push(format!(
                "flight_no: \"{f}\" is not a flight number (\"SQ 636\")"
            )),
        }
    }
    for k in ["scheduled_departure", "actual_departure"] {
        let v = s_arg(a, k);
        if v.is_empty() {
            continue;
        }
        match hhmm(&v) {
            Some(x) => {
                out.insert(k.into(), json!(x));
            }
            None => problems.push(format!("{k}: \"{v}\" is not a time: give HH:MM")),
        }
    }
    // a delay rule's amount of 0 is no rule (insurance_real 2026-11-02: the cap passed as 0 when the user gave none,
    // and a 7 h delay came out as nothing to claim): dropped, the market's rule stands
    for k in ["benefit_per_block", "benefit_cap"] {
        if n_arg(a, k).is_some_and(|x| x > 0.0) {
            money_arg(a, k, &mut out, &mut problems);
        }
    }
    for k in ["block_hours", "claim_within_days"] {
        if let Some(n) = i_arg(a, k) {
            if n > 0 {
                out.insert(k.into(), json!(n));
            } else {
                problems.push(format!("{k}: {n} — more than 0"));
            }
        }
    }
    // FWD Singapore's online claim (walked 2026-11-03, docs/sites/fwd-claim.md): the policy number in FWD's own
    // format, the trip's dates and country, and the documents of this chat — all into code's form
    if market == "sg" {
        let p = s_arg(a, "policy_no").to_uppercase().replace(' ', "");
        if !p.is_empty() {
            match sg_policy_no(&p) {
                Some(n) => {
                    out.insert("policy_no".into(), json!(n));
                }
                None => {
                    out.remove("policy_no");
                    problems.push(format!(
                        "policy_no: \"{p}\" — FWD Singapore's travel policy numbers read PNTR20NN-NNNNNNNN (the policy email's \"Policy Number\")"
                    ));
                }
            }
        }
        let leave = date_arg(a, "leave_sg", cx.today, Some(true), &mut out, &mut problems);
        let back = date_arg(a, "return_sg", cx.today, None, &mut out, &mut problems);
        if let (Some(l), Some(b)) = (leave, back) {
            if b < l {
                problems.push(format!(
                    "return_sg: {} is before leave_sg {}",
                    iso(b),
                    iso(l)
                ));
            }
        }
        if leave.is_none() && !problems.iter().any(|p| p.starts_with("leave_sg")) {
            missing.push("leave_sg (the day the trip left Singapore)".into());
        }
        if back.is_none() && !problems.iter().any(|p| p.starts_with("return_sg")) {
            missing.push("return_sg (the day the trip was to return to Singapore)".into());
        }
        let dest = s_arg(a, "destination");
        if dest.is_empty() {
            missing.push("destination (the country travelled to)".into());
        } else {
            let iso2 = crate::places::country(&dest)
                .map(|c| c.iso)
                .or_else(|| Some(crate::places::iso_of(&dest)).filter(|c| !c.is_empty()))
                .or_else(|| crate::places::city_named(&dest).map(|c| c.country));
            match iso2.and_then(crate::places::country) {
                Some(c) => {
                    out.insert("destination".into(), json!(c.name));
                }
                None => problems.push(format!(
                    "destination: \"{dest}\" — give the country travelled to (its 2-letter code: JP for Tokyo)"
                )),
            }
        }
        // FWD's documents step requires the itinerary and the re-scheduled boarding pass ("… is required"; walked
        // 2026-11-03 — without them "next" stays put): asked like the policy number; the airline's letter and
        // other documents are optional
        for (k, what) in [
            ("itinerary_file", "itinerary_file (the itinerary or e-ticket — FWD requires it)"),
            ("boarding_pass_file", "boarding_pass_file (the boarding pass of the delayed or re-scheduled flight — FWD requires it)"),
        ] {
            if s_arg(a, k).is_empty() {
                missing.push(what.into());
            }
        }
        for k in CLAIM_FILES {
            let f = s_arg(a, k);
            if f.is_empty() {
                continue;
            }
            if f.starts_with("uploads/") || f.starts_with("attachments/") {
                out.insert(k.into(), json!(f));
            } else {
                problems.push(format!(
                    "{k}: \"{f}\" — a file of this chat (uploads/… or attachments/…)"
                ));
            }
        }
    }
    if s_arg(&Value::Object(out.clone()), "policy_no").is_empty()
        && !problems.iter().any(|p| p.starts_with("policy_no"))
    {
        missing.push(
            "policy_no (the FWD policy number — from the policy email, else ask the user)".into(),
        );
    }
    if incident.is_none() && !problems.iter().any(|p| p.starts_with("incident_date")) {
        missing.push("incident_date (the delayed flight's date)".into());
    }
    if !out.contains_key("flight_no") && !problems.iter().any(|p| p.starts_with("flight_no")) {
        missing.push("flight_no".into());
    }
    if !(out.contains_key("scheduled_departure") && out.contains_key("actual_departure")) {
        missing.push(
            "scheduled_departure and actual_departure (the airline's delay letter states them)"
                .into(),
        );
    }
    (out, problems, missing)
}

/// The documents an FWD Singapore delay claim uploads, as the call names them.
const CLAIM_FILES: [&str; 4] = [
    "itinerary_file",
    "boarding_pass_file",
    "airline_letter_file",
    "other_file",
];

/// An FWD Singapore travel policy number in FWD's format (its claim form checks the part after the fixed `PNTR20`:
/// `NN-NNNNNNNN`, or `NN-NNNNNNNN-NN`): the whole number, `PNTR20` first; `None` for anything else. A structured
/// value checked, not a pattern over words.
pub fn sg_policy_no(p: &str) -> Option<String> {
    static RE: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"^(?:PNTR20)?(\d{2}-\d{8}(?:-\d{2})?)$").unwrap());
    RE.captures(p).map(|c| format!("PNTR20{}", &c[1]))
}

fn check_buy(a: &Value, cx: &Ctx) -> Checks {
    let mut out = Map::new();
    let mut problems = Vec::new();
    let mut missing = Vec::new();
    let market = match s_arg(a, "market").as_str() {
        m @ ("sg" | "hk") => m.to_string(),
        _ => market_from_profile(cx.profile).to_string(),
    };
    out.insert("market".into(), json!(market));
    out.insert(
        "action".into(),
        json!(if s_arg(a, "action") == "compare" {
            "compare"
        } else {
            "buy"
        }),
    );
    // the destination's country, from the structured value: an ISO code, a country's name or a city of the table,
    // each compared whole; anything else goes back to the model (FWD's region is picked by the country — "Tokyo,
    // Japan" left it blank and the quote never opened, insurance_real 2026-11-02)
    let dest = s_arg(a, "destination");
    if !dest.is_empty() {
        let iso = crate::places::country(&dest)
            .map(|c| c.iso)
            .or_else(|| Some(crate::places::iso_of(&dest)).filter(|c| !c.is_empty()))
            .or_else(|| crate::places::city_named(&dest).map(|c| c.country));
        match iso.and_then(crate::places::country) {
            Some(c) => {
                out.insert("destination".into(), json!(c.name));
            }
            None => problems.push(format!(
                "destination: \"{dest}\" — give the destination country's 2-letter code (JP for Tokyo)"
            )),
        }
    } else {
        missing.push("destination".into());
    }
    let dep = date_arg(a, "depart", cx.today, Some(false), &mut out, &mut problems);
    let ret = date_arg(a, "return", cx.today, Some(false), &mut out, &mut problems);
    if let (Some(d), Some(r)) = (dep, ret) {
        if r < d {
            problems.push(format!(
                "return: {} is before the departure {}",
                iso(r),
                iso(d)
            ));
        }
    }
    if (dep.is_none() || ret.is_none())
        && !problems
            .iter()
            .any(|p| p.starts_with("depart") || p.starts_with("return"))
    {
        missing.push("depart and return (the trip's dates)".into());
    }
    let travellers = i_arg(a, "travellers").unwrap_or(1);
    if (1..=9).contains(&travellers) {
        out.insert("travellers".into(), json!(travellers));
    } else {
        problems.push(format!("travellers: {travellers} — 1 to 9"));
    }
    let cover = s_arg(a, "cover");
    if let (false, Some(x)) = (cover.is_empty(), n_arg(a, "cover_at_least")) {
        if x > 0.0 {
            out.insert("cover".into(), json!(cover));
            out.insert("cover_at_least".into(), json!(x));
        }
    }
    text_arg(a, "plan", &mut out);
    text_arg(a, "card", &mut out);
    // FWD Hong Kong's application (walked 2026-11-03): one traveller on the quote, the insured's title, and the date of
    // issue of their new Smart HKID (the Vault keeps the number, not the date) — asked for a purchase
    if market == "hk" {
        if travellers > 1 {
            problems.push("travellers: FWD Hong Kong's quote is filled for one traveller here — 1, and the user adds the others on FWD's page".into());
        }
        let title = s_arg(a, "title");
        match title.trim_end_matches('.').to_lowercase().as_str() {
            "" => {}
            "mr" => {
                out.insert("title".into(), json!("Mr."));
            }
            "miss" | "ms" => {
                out.insert("title".into(), json!("Miss"));
            }
            "mrs" => {
                out.insert("title".into(), json!("Mrs."));
            }
            other => problems.push(format!(
                "title: \"{other}\" — Mr., Miss or Mrs. (FWD's list)"
            )),
        }
        let issued = date_arg(
            a,
            "hkid_issue_date",
            cx.today,
            Some(true),
            &mut out,
            &mut problems,
        );
        // the name's parts are the model's reading of it (Hong Kong names come family name first)
        for k in ["family_name", "given_names"] {
            let v = s_arg(a, k).to_uppercase();
            if !v.is_empty() {
                out.insert(k.into(), json!(v));
            }
        }
        if s_arg(&Value::Object(out.clone()), "action") == "buy"
            && !(out.contains_key("family_name") && out.contains_key("given_names"))
        {
            missing.push(
                "family_name and given_names (the insured's name as on the HKID, split)".into(),
            );
        }
        if s_arg(&Value::Object(out.clone()), "action") == "buy" {
            if !out.contains_key("title") && !problems.iter().any(|p| p.starts_with("title")) {
                missing.push("title (Mr., Miss or Mrs. — FWD's application asks it)".into());
            }
            if issued.is_none() && !problems.iter().any(|p| p.starts_with("hkid_issue_date")) {
                missing.push(
                    "hkid_issue_date (the date of issue on the user's new Smart HKID card)".into(),
                );
            }
        }
    }
    (out, problems, missing)
}

fn check_bag(a: &Value) -> Checks {
    let mut out = Map::new();
    let mut problems = Vec::new();
    let mut missing = Vec::new();
    let r = s_arg(a, "file_ref");
    if r.is_empty() {
        missing.push(
            "file_ref (the file reference on the Property Irregularity Report, e.g. NRTSQ12345)"
                .into(),
        );
    } else {
        match super::bagtrace::file_ref_of(&r) {
            Some(x) => {
                out.insert("file_ref".into(), json!(x));
            }
            None => problems.push(format!(
                "file_ref: \"{r}\" — a WorldTracer file reference is the station (3 letters), the airline (2) and 5 digits, e.g. NRTSQ12345"
            )),
        }
    }
    let family = s_arg(a, "family_name");
    if !family.is_empty() {
        out.insert("family_name".into(), json!(family.to_uppercase()));
    }
    let d = s_arg(a, "flight_date");
    if let Some(d) = parse_iso(&d) {
        out.insert("flight_date".into(), json!(iso(d)));
    }
    out.insert("watch".into(), json!(b_arg(a, "watch")));
    (out, problems, missing)
}

/// Whether code already read the mail for `tool` in this chat (a `T_result` with status `mail_read`).
fn mail_read(transcript: &[Value], tool: &str) -> bool {
    let want = format!("{tool}{}", workflow::RESULT_SUFFIX);
    transcript
        .iter()
        .flat_map(tool_calls)
        .filter(|tc| {
            get_str(tc, "id").starts_with(workflow::RESULT_PREFIX) && stored_call(tc).0 == want
        })
        .any(|tc| {
            let id = get_str(tc, "id");
            transcript
                .iter()
                .find(|m| get_str(m, "role") == "tool" && get_str(m, "tool_call_id") == id)
                .is_some_and(|m| get_str(&json_of(m), "status") == MAIL_READ)
        })
}

// ---------------------------------------------------------------- the mail, read back (structured fields only)

/// One fetched email: its structured fields (the body is the model's to read).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Msg {
    pub id: String,
    pub from: String,
    pub subject: String,
    pub attachments: Vec<String>,
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

/// The PDF to attach: the one on the email the model named, else the first PDF on a fetched email that is not the
/// insurer's (the airline's letter) — by the structured sender and file names.
fn attachment(transcript: &[Value], message_id: &str) -> Option<(String, String)> {
    let msgs = fetched(transcript);
    let pdf = |m: &Msg| {
        m.attachments
            .iter()
            .find(|a| a.to_lowercase().ends_with(".pdf"))
            .map(|a| (m.id.clone(), a.clone()))
    };
    if !message_id.is_empty() {
        if let Some(x) = msgs.iter().filter(|m| m.id == message_id).find_map(pdf) {
            return Some(x);
        }
    }
    msgs.iter()
        .filter(|m| {
            let f = m.from.to_lowercase();
            !(f.contains("fwd") || f.contains("bolttech"))
        })
        .find_map(pdf)
}

/// The machine's ids go on from the ones already in the transcript.
struct Ids(usize);

impl Ids {
    fn call(&self, k: usize, name: &str, args: Value) -> ToolCall {
        workflow::code_call(PREFIX, self.0 + k, name, args)
    }
    fn one(&self, name: &str, args: Value) -> Step {
        Step::Calls(vec![self.call(0, name, args)])
    }
}

/// The fixed searches, then the first hits of each: the step to run, or `None` once the mail is in.
fn gather(s: &ClaimSpec, run: &[Value], ids: &Ids, flight: &str) -> Option<Step> {
    let searches = results_of(run, "gmail_search");
    if searches.is_empty() {
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
                    ids.call(
                        i,
                        "gmail_search",
                        json!({"query": q.replace("{flight}", &fw), "max_results": 10}),
                    )
                })
                .collect(),
        ));
    }
    let have: Vec<String> = fetched(run).iter().map(|m| m.id.clone()).collect();
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
                .map(|(i, id)| ids.call(i, "gmail_get_message", json!({"message_id": id})))
                .collect(),
        )
    })
}

/// The mail read for missing facts: the searches and the hits, then the result that lists the emails (the model reads
/// them and calls the tool again with the facts).
fn read_mail(tool: &str, s: &ClaimSpec, args: &Value, run: &[Value], ids: &Ids) -> Step {
    let flight = s_arg(args, "flight_no");
    if let Some(step) = gather(s, run, ids, &flight) {
        return step;
    }
    let emails: Vec<Value> = fetched(run)
        .iter()
        .map(|m| json!({"message_id": m.id, "from": m.from, "subject": m.subject, "attachments": m.attachments}))
        .collect();
    let missing = args.get("_missing").cloned().unwrap_or_else(|| json!([]));
    Step::Done(
        json!({"status": MAIL_READ, "call_again": tool, "emails": emails, "facts_needed": missing}),
    )
}

// ---------------------------------------------------------------- text helpers

static AMOUNT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)(?:USD|SGD|HKD|US\$|S\$|HK\$|\$)\s?(\d{1,3}(?:,\d{3})+|\d+)(?:\.(\d{1,2}))?\b|\b(\d{1,3}(?:,\d{3})+|\d+)(?:\.(\d{2}))?\s?(?:USD|SGD|HKD)\b").unwrap()
});

/// Every amount of money written in `text` (a known page's row), in order.
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

fn whole(x: f64) -> String {
    money(x).trim_end_matches(".00").to_string()
}

// ---------------------------------------------------------------- the CSR trip claim: facts and rules

/// The facts of a Chase Sapphire trip claim, from the validated arguments.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TripClaim {
    /// "Trip Cancellation" | "Trip Interruption"
    pub benefit: String,
    pub loss: Option<NaiveDate>,
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
    /// "sickness of the cardholder" (never the diagnosis)
    pub reason: String,
    pub travellers: usize,
    pub trip: Vec<String>,
    pub has_doctor_letter: bool,
    pub has_supplier_letter: bool,
}

impl TripClaim {
    pub fn from_args(a: &Value) -> TripClaim {
        let what = s_arg(a, "reason");
        let who = match s_arg(a, "who").as_str() {
            "family_member" => "an immediate family member",
            "travelling_companion" => "a travelling companion",
            _ => "the cardholder",
        };
        let reason = match what.as_str() {
            "injury" | "sickness" | "death" | "" => {
                format!(
                    "{} of {who}",
                    if what.is_empty() { "sickness" } else { &what }
                )
            }
            "severe_weather" => "severe weather".into(),
            "quarantine" => format!("physician-imposed quarantine of {who}"),
            _ => "another covered reason".into(),
        };
        let mut t = TripClaim {
            benefit: if s_arg(a, "benefit") == "trip_interruption" {
                "Trip Interruption"
            } else {
                "Trip Cancellation"
            }
            .into(),
            loss: parse_iso(&s_arg(a, "incident_date")),
            paid: n_arg(a, "paid").unwrap_or(0.0),
            refunded: n_arg(a, "refunded").unwrap_or(0.0),
            change_fee: n_arg(a, "change_fee").unwrap_or(0.0),
            lost: n_arg(a, "non_refundable").unwrap_or(0.0),
            suppliers: a
                .get("bookings")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .map(|b| (s_arg(b, "supplier"), s_arg(b, "reference")))
                .collect(),
            last4: s_arg(a, "card_last4"),
            card_product: match s_arg(a, "card_product") {
                p if p.is_empty() => "Chase Sapphire Reserve".into(),
                p => p,
            },
            reason,
            travellers: i_arg(a, "travellers").unwrap_or(1).max(1) as usize,
            trip: a
                .get("trip")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect(),
            has_doctor_letter: b_arg(a, "doctor_letter"),
            has_supplier_letter: b_arg(a, "supplier_letter")
                || n_arg(a, "non_refundable").is_some(),
            amount: 0.0,
        };
        t.amount = claim_amount(&t, spec(Flow::CsrTrip));
        t
    }
    /// How the amount was computed: paid − refunded when that is the figure (a cancellation), else what the supplier
    /// stated as not refunded; plus the change fee for an interruption.
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

// ---------------------------------------------------------------- the FWD delay claim: facts and rules

/// The facts of a flight-delay claim on an FWD policy, from the validated arguments (the policy's own rule when the
/// model read it, else the market's).
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
    /// the time limit in days (the policy's when the model read it, else the market's)
    pub limit_days: i64,
    /// whether the limit came from the policy email
    pub limit_from_policy: bool,
    pub emergency: String,
    /// the message id of the airline's delay email
    pub certificate: String,
}

fn minutes_of(t: &str) -> Option<i64> {
    let (h, m) = t.split_once(':')?;
    Some(h.parse::<i64>().ok()? * 60 + m.parse::<i64>().ok()?)
}

/// "7 h 15 min"
pub fn hours_text(minutes: i64) -> String {
    format!("{} h {:02} min", minutes / 60, minutes % 60)
}

impl DelayClaim {
    pub fn from_args(s: &ClaimSpec, a: &Value) -> DelayClaim {
        let mut d = DelayClaim {
            policy_no: s_arg(a, "policy_no"),
            plan: s_arg(a, "plan"),
            flight_no: s_arg(a, "flight_no"),
            from: s_arg(a, "from"),
            to: s_arg(a, "to"),
            airline: s_arg(a, "airline"),
            airline_ref: s_arg(a, "airline_ref"),
            incident_date: s_arg(a, "incident_date"),
            scheduled: s_arg(a, "scheduled_departure"),
            actual: s_arg(a, "actual_departure"),
            reason: s_arg(a, "delay_reason"),
            per_block: n_arg(a, "benefit_per_block").unwrap_or(s.delay.0),
            block_hours: i_arg(a, "block_hours").unwrap_or(s.delay.1),
            cap: n_arg(a, "benefit_cap").unwrap_or(s.delay.2),
            limit_days: i_arg(a, "claim_within_days").unwrap_or(s.notify_days),
            limit_from_policy: i_arg(a, "claim_within_days").is_some(),
            emergency: s_arg(a, "emergency_phone"),
            certificate: s_arg(a, "certificate_message_id"),
            ..Default::default()
        };
        if let (Some(x), Some(y)) = (minutes_of(&d.scheduled), minutes_of(&d.actual)) {
            d.minutes = if y >= x { y - x } else { y + 24 * 60 - x };
        }
        d.amount = delay_amount(&d);
        d
    }
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
        if self.limit_days <= 0 {
            return None;
        }
        parse_iso(&self.incident_date).map(|d| d + Duration::days(self.limit_days))
    }
    /// The form functions' values (FWD Singapore's form; bolttech's PDF).
    fn form_args(&self, currency: &str) -> Value {
        json!({
            "policy_no": self.policy_no, "claim_type": "Travel delay", "incident_date": self.incident_date,
            "what": self.what(), "amount": if self.amount > 0.0 { format!("{:.2}", self.amount) } else { String::new() },
            "flight_no": self.flight_no, "from": self.from, "to": self.to, "scheduled": self.scheduled,
            "actual": self.actual, "hours": hours_text(self.minutes), "minutes": self.minutes,
            "airline": self.airline, "airline_ref": self.airline_ref, "currency": currency,
        })
    }
    /// The benefit by the policy's rule, in words: "SGD 100.00 (SGD 100 for every 6 hours: 1 × 6 h, up to SGD 1,500)".
    fn rule(&self, cur: &str) -> String {
        format!(
            "{cur} {} for every {} hours of delay ({} × {} h), up to {cur} {}",
            whole(self.per_block),
            self.block_hours,
            self.blocks(),
            self.block_hours,
            whole(self.cap)
        )
    }
}

/// The delay benefit: the amount per full block of hours, within the cap.
pub fn delay_amount(d: &DelayClaim) -> f64 {
    if d.block_hours <= 0 {
        return 0.0;
    }
    let blocks = d.minutes / (d.block_hours * 60);
    (blocks as f64 * d.per_block).min(d.cap)
}

// ---------------------------------------------------------------- the FWD purchase: plans and the rule

/// One plan the quote page lists.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Plan {
    pub name: String,
    /// the address of its "Select" link
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
            .map(|(c, v)| format!("{c} {cur} {}", whole(*v)))
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

/// The covers the plans are compared on, by the benefit line's label on FWD's summary (the page's fixed labels; the
/// first line that matches): medical, trip cancellation (the single-traveller line), travel delay, baggage delay.
const PLAN_COVERS: [(&str, &str, bool); 4] = [
    ("medical", "Overseas medical expenses", false),
    (
        "cancellation",
        "Trip cancelled before you leave Singapore",
        true,
    ),
    ("delay", "Travel delay", false),
    ("baggage", "Baggage delay", false),
];
/// The id prefix and suffix of a plan's button on the summary (`#TI_QuoteSummarypage_Business_Buy`).
const PLAN_BUTTON: (&str, &str) = ("TI_QuoteSummarypage_", "_Buy");

/// The arguments of `browser_read_elements` for FWD's quote summary (`/travel-insurance/online-quote/summary`, mapped
/// 2026-11-02): the plans' buttons by their ids, the prices above them, and every benefit line (its label and the
/// three plans' values, folded sections too).
fn plans_read_args() -> Value {
    let col = |n: usize| format!(".cp-col-benefit > .cp-col:nth-child({n})");
    json!({"read": [
        {"name": "buy", "sel": format!("#plan-table .coverage-plan__subheader button[id^='{}']", PLAN_BUTTON.0), "attr": "id"},
        {"name": "price", "sel": "#plan-table .coverage-plan__subheader .cp-col h5"},
        {"name": "lines", "sel": "#plan-table section.benefit-line", "hidden": true, "max": 80,
         "fields": {"label": ".cp-col-policy", "p1": col(1), "p2": col(2), "p3": col(3)}}
    ]})
}

/// FWD's link to a saved travel quote: its route `emailRedirect.html` with the quote's `HashKey` (mapped 2026-11-02:
/// it opens FWD's own resume page, which asks for the mobile number the quote was saved with).
const FWD_SAVED_QUOTE: &str = "https://www.fwd.com.sg/travel-insurance/emailRedirect.html?hash=";

/// What is read after FWD's save: the quote's keys in the site's own storage (`HashKey`, `quotationNo`) and whether the
/// save dialog is still open (a field FWD refused).
fn saved_read_args() -> Value {
    json!({"read": [
        {"name": "hash", "storage": "local", "key": "HashKey"},
        {"name": "quotation", "storage": "local", "key": "quotationNo"},
        {"name": "open", "sel": ".fwd-save-dialog input[name=nric]"}
    ]})
}

/// The saved quote's link and quotation number from [`saved_read_args`]' result; `None` when the dialog is still open
/// or no quote key was kept. The key is checked as FWD writes it (letters and digits only) before it goes in a link.
fn saved_quote(read: &Value) -> Option<(String, String)> {
    use super::confirm::element_values;
    if !element_values(read, "open").is_empty() {
        return None;
    }
    let hash = element_values(read, "hash").into_iter().next()?;
    if hash.len() < 20 || !hash.chars().all(|c| c.is_ascii_alphanumeric()) {
        return None;
    }
    let quotation = element_values(read, "quotation")
        .into_iter()
        .next()
        .filter(|q| q.len() <= 30 && q.chars().all(|c| c.is_ascii_alphanumeric()))
        .unwrap_or_default();
    Some((format!("{FWD_SAVED_QUOTE}{hash}"), quotation))
}

/// The plans on FWD's quote summary (a known page), from its elements: each plan button's id gives the plan's name
/// ("Business"), the price above it the premium, and the benefit lines its covers ([`PLAN_COVERS`]; a value's amount
/// is the last money value in it).
pub fn plans_of(read: &Value) -> Vec<Plan> {
    let buttons = super::confirm::element_values(read, "buy");
    let prices = super::confirm::element_values(read, "price");
    let lines = super::confirm::element_fields(read, "lines");
    buttons
        .iter()
        .enumerate()
        .filter_map(|(i, id)| {
            let name = id
                .strip_prefix(PLAN_BUTTON.0)?
                .strip_suffix(PLAN_BUTTON.1)?
                .to_string();
            if name.is_empty() || !name.chars().all(|c| c.is_ascii_alphabetic()) {
                return None;
            }
            let premium = *amounts_in(prices.get(i)?).first()?;
            let col = format!("p{}", i + 1);
            let cover = PLAN_COVERS
                .iter()
                .filter_map(|(key, label, exact)| {
                    let line = lines.iter().find(|l| {
                        let t = l.get("label").map(|x| x.trim()).unwrap_or("");
                        if *exact {
                            t == *label
                        } else {
                            t.starts_with(label)
                        }
                    })?;
                    amounts_in(line.get(&col)?)
                        .last()
                        .map(|v| (key.to_string(), *v))
                })
                .collect();
            Some(Plan {
                link: id.clone(),
                name,
                premium,
                cover,
            })
        })
        .collect()
}

/// The plan by the user's rule: the plan they chose (its listed name, exactly); else the cheapest plan whose named cover is at least the
/// minimum, else the cheapest. The second value says which rule picked it.
pub fn pick_plan<'a>(
    plans: &'a [Plan],
    minimum: &Option<(String, f64)>,
    chosen: &str,
) -> Option<(&'a Plan, String)> {
    // the plan the user chose: the name exactly as the choice card listed it (any case), never a word of it
    if let Some(p) = plans
        .iter()
        .find(|p| !chosen.trim().is_empty() && p.name.eq_ignore_ascii_case(chosen.trim()))
    {
        return Some((p, "the plan you chose".to_string()));
    }
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
                    whole(*min)
                ),
            ));
        }
        let all: Vec<&Plan> = plans.iter().collect();
        return cheapest(&all).map(|p| {
            (
                p,
                format!(
                    "no plan has {col} cover of at least {}: the cheapest plan instead",
                    whole(*min)
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
    /// the catalog id kept with the card ("" when none)
    pub card_id: String,
    /// added by its name only: number, expiry and CVC UNKNOWN
    pub name_only: bool,
}

static VAULT_LINE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?m)^- id=(\S+) · (.+?) · card · ").unwrap());

/// The cards in a `vault_list` result (the tool's own listing, by its fixed fields).
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
            let card_id = l
                .split(" · ")
                .find_map(|p| p.strip_prefix("card_id="))
                .filter(|id| *id != "none" && *id != "unchecked")
                .unwrap_or("")
                .to_string();
            Some(VaultCard {
                id: c[1].to_string(),
                label,
                card_id,
                name_only: l.contains("UNKNOWN"),
            })
        })
        .collect()
}

/// The card the tool argument names — exactly, by its Vault id, its catalog `card_id` or its whole label (any case) —
/// else the first with its details, else the first. A name is never matched word by word.
pub fn pick_card<'a>(cards: &'a [VaultCard], names: &[String]) -> Option<&'a VaultCard> {
    cards
        .iter()
        .find(|c| {
            names.iter().map(|n| n.trim()).any(|n| {
                !n.is_empty()
                    && (c.id == n
                        || (!c.card_id.is_empty() && c.card_id == n)
                        || c.label.eq_ignore_ascii_case(n))
            })
        })
        .or_else(|| cards.iter().find(|c| !c.name_only))
        .or_else(|| cards.first())
}

// ---------------------------------------------------------------- the transcript's gates (fill, review, snapshot)

const OWN_STEP: &str = "This step is yours";

/// The facts a run that ends early still reports (its claim, its amounts), merged into the gate's own result.
fn finish(step: Step, base: &Value) -> Step {
    let Step::Done(mut v) = step else {
        return step;
    };
    if let (Some(o), Some(b)) = (v.as_object_mut(), base.as_object()) {
        // the base's `show` and `hints` first, then the gate's own
        for list in ["show", "hints"] {
            let mut all: Vec<Value> = b
                .get(list)
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            all.extend(
                o.get(list)
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default(),
            );
            o.insert(list.into(), json!(all));
        }
        for (k, x) in b {
            if k != "status" && k != "hints" && k != "show" && !o.contains_key(k) {
                o.insert(k.clone(), x.clone());
            }
        }
    }
    Step::Done(v)
}

/// The fill of one form (`stage`), a user's own step the function stopped at handed over and the fill run again
/// (never more than twice).
fn fill_gate(
    run: &[Value],
    ids: &Ids,
    stage: &str,
    mut args: Value,
    name: &str,
) -> Result<usize, Step> {
    args["stage"] = json!(stage);
    let fills: Vec<(Value, &Value)> = results_of(run, "form_fill")
        .into_iter()
        .filter(|(a, _)| sget(a, "stage") == stage)
        .collect();
    let takeovers = results_of(run, "browser_request_takeover");
    let own_backs: Vec<usize> = takeovers
        .iter()
        .filter(|(a, m)| sget(a, "reason").starts_with(OWN_STEP) && handed_back(m))
        .map(|(_, m)| pos(run, m))
        .collect();
    let Some((_, last)) = fills.last() else {
        return Err(ids.one("form_fill", args));
    };
    let fill_pos = pos(run, last);
    let Some(why) = stopped(last) else {
        return Ok(fill_pos);
    };
    if own_backs.iter().any(|&p| p > fill_pos) {
        return Err(ids.one("form_fill", args));
    }
    let step = why.split(" is the user's").next().unwrap_or(&why).trim();
    let step = step.strip_prefix("the ").unwrap_or(step).to_string();
    if takeovers.iter().any(|(_, m)| pos(run, m) > fill_pos) || own_backs.len() >= 2 {
        return Err(Step::Done(
            json!({"status": "waiting_on_user", "user_step": step,
            "show": [S::fact(&format!("Your {name} is waiting on your step"), &format!("你的{name}在等你完成"), &step)],
            "hints": ["finish it in the browser tab, then ask me to continue — nothing has been submitted"]}),
        ));
    }
    Err(ids.one(
        "browser_request_takeover",
        json!({"reason": format!("{OWN_STEP}: {step}. Do it on the page, then hand the browser back and I will fill in the rest of the form.")}),
    ))
}

/// The review hand-over after position `after`: the step to run, or the position of the hand-back. A takeover that
/// ended without the hand-back: the form stays in the browser tab for the user.
fn review_gate(run: &[Value], ids: &Ids, after: usize, reason: String) -> Result<usize, Step> {
    let takeovers = results_of(run, "browser_request_takeover");
    let review: Vec<&(Value, &Value)> = takeovers
        .iter()
        .filter(|(a, m)| pos(run, m) > after && !sget(a, "reason").starts_with(OWN_STEP))
        .collect();
    match review.last() {
        None => Err(ids.one("browser_request_takeover", json!({"reason": reason}))),
        Some((_, m)) if handed_back(m) => Ok(pos(run, m)),
        Some(_) => Err(Step::Done(json!({"status": "not_confirmed",
            "show": [S::fact("Status", "状态", "the form is filled in and waiting in the browser tab — nothing has been submitted or paid")],
            "hints": []}))),
    }
}

/// A reminder's cron at 08:00 on day `d`.
fn cron_at(d: NaiveDate) -> String {
    format!("0 8 {} {} *", d.day(), d.month())
}

fn opened(run: &[Value], host: &str, path: &str) -> bool {
    results_of(run, "browser_navigate").iter().any(|(a, _)| {
        let u = sget(a, "url");
        u.contains(host) && u.contains(path)
    })
}

fn step_count(transcript: &[Value]) -> usize {
    transcript
        .iter()
        .flat_map(tool_calls)
        .filter(|tc| get_str(tc, "id").starts_with(PREFIX))
        .count()
        + 1
}

/// The path a `gmail_save_attachment` result names.
fn saved_path(run: &[Value]) -> Option<String> {
    static SAVED: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"saved: (\S+)").unwrap());
    results_of(run, "gmail_save_attachment")
        .iter()
        .find_map(|(_, m)| SAVED.captures(&content_of(m)).map(|c| c[1].to_string()))
}

// ---------------------------------------------------------------- the machine

/// What the machine reads besides the validated arguments and the transcript.
pub struct Machine<'a> {
    pub profile: &'a Value,
    pub today: NaiveDate,
}

/// The next step of a run of workflow tool `tool` with its validated arguments `args`; `transcript` is the whole
/// transcript and `from` the index of the tool's call (the machine's steps read only what follows it; the mail code
/// read for the facts, before it, gives the attachments).
pub fn next(tool: &str, args: &Value, transcript: &[Value], from: usize, m: &Machine) -> Step {
    let from = from.min(transcript.len());
    let run = &transcript[from..];
    let ids = Ids(step_count(transcript));
    let read = b_arg(args, "_read_mail");
    match tool {
        BAG_TRACE if read => read_mail(tool, spec(Flow::BagTrace), args, run, &ids),
        BAG_TRACE => next_bag(args, run, &ids),
        INSURANCE_BUY if read => read_mail(tool, spec(Flow::FwdSgBuy), args, run, &ids),
        INSURANCE_BUY if s_arg(args, "market") == "hk" => next_buy_hk(args, run, &ids),
        INSURANCE_BUY => next_buy(args, run, &ids, m),
        _ => {
            let s = if s_arg(args, "insurer") == "chase_sapphire" {
                spec(Flow::CsrTrip)
            } else if s_arg(args, "market") == "hk" {
                spec(Flow::FwdHkClaim)
            } else {
                spec(Flow::FwdSgClaim)
            };
            if read {
                return read_mail(tool, s, args, run, &ids);
            }
            match s.flow {
                Flow::CsrTrip => next_csr(s, args, run, &ids, m),
                _ if s_arg(args, "benefit") == "other" => not_supported(s),
                Flow::FwdHkClaim => next_hk_claim(s, args, transcript, run, &ids, m),
                _ => next_sg_claim(s, args, transcript, run, &ids, m),
            }
        }
    }
}

/// An FWD claim other than a flight delay: no code form for it; where it goes.
fn not_supported(s: &ClaimSpec) -> Step {
    let channel = if s.claims_email.is_empty() {
        s.start_url.to_string()
    } else {
        format!("{} (bolttech's claim form by email)", s.claims_email)
    };
    Step::Done(
        json!({"status": "not_supported", "insurer": s.decides, "channel": channel,
        "show": [S::fact("Where this claim goes", "这类理赔提交到", &channel)],
        "hints": ["this kind of claim is not filled in by code"]}),
    )
}

// ---------------------------------------------------------------- the CSR trip claim

fn next_csr(s: &ClaimSpec, args: &Value, run: &[Value], ids: &Ids, m: &Machine) -> Step {
    let t = TripClaim::from_args(args);
    if !opened(run, s.host, "/") {
        return ids.one("browser_navigate", json!({"url": s.start_url}));
    }
    // the sign-in page: the account's email from the profile (its form function); the password and the sign-in are
    // the user's, and the claim pages behind them are not mapped
    let Some((_, fill)) = results_of(run, "form_fill").into_iter().next_back() else {
        return ids.one("form_fill", json!({"stage": "login"}));
    };
    let fill_pos = pos(run, fill);
    let reason = format!(
        "{OWN_STEP}: sign in to Card Claim Center (where eClaimsLine moved; your email is filled in) or create your account. Then the {} claim: date of loss {}, amount USD {} ({}), reason \"{}\" — {} yourself. Hand the browser back after signing in and I will go on with the claim form.",
        t.benefit,
        t.loss.map(iso).unwrap_or_else(|| "to fill in".into()),
        money(t.amount),
        t.arithmetic(),
        t.reason,
        s.user_steps
    );
    let takeovers = results_of(run, "browser_request_takeover");
    let back = match takeovers.iter().rfind(|(_, x)| pos(run, x) > fill_pos) {
        None => return ids.one("browser_request_takeover", json!({"reason": reason})),
        Some((_, x)) if handed_back(x) => pos(run, x),
        Some(_) => return Step::Done(csr_result(s, &t, "waiting_on_user", m.today)),
    };
    // after the hand-back: the sign-in page still there (its password box) = not signed in
    let read = results_of(run, "browser_read_elements")
        .into_iter()
        .find(|(_, x)| pos(run, x) > back);
    let Some((_, read)) = read else {
        return ids.one(
            "browser_read_elements",
            json!({"read": [{"name": "password", "sel": CCC_PASSWORD, "attr": "type"}]}),
        );
    };
    if !super::confirm::element_values(read, "password").is_empty() {
        return Step::Done(csr_result(s, &t, "waiting_on_user", m.today));
    }
    // signed in: the claim pages have no form function — the browser-use skill fills them with these facts
    let url = match get_str(&json_of(read), "url") {
        u if u.is_empty() => s.start_url.to_string(),
        u => u,
    };
    let mut v = csr_result(s, &t, "browser_use", m.today);
    v["url"] = json!(url);
    Step::Done(v)
}

/// The CSR claim's result: the benefit, the two dates, the amount and its arithmetic, the status, the documents, the
/// reminders, the limits — and `hints`.
fn csr_result(s: &ClaimSpec, t: &TripClaim, status: &str, today: NaiveDate) -> Value {
    let amount = format!("USD {}", money(t.amount));
    let mut must = vec![format!("{} claim on the {}", t.benefit, t.card_product)];
    // the facts the user must read: appended to the reply by code (agent/show.rs)
    let mut show = vec![
        S::amount(
            &format!("{} claim on the {}", t.benefit, t.card_product),
            &format!("{}理赔（{}）", t.benefit, t.card_product),
            t.amount,
            "USD",
        ),
        S::fact("How it adds up", "计算方式", &t.arithmetic()),
    ];
    let mut dates = json!({});
    if let Some(loss) = t.loss {
        let d20 = loss + Duration::days(s.notify_days);
        let d90 = loss + Duration::days(s.documents_days);
        let late = today > d20;
        show.push(S::fact(
            "Notify the administrator by",
            "通知理赔管理方截止",
            &format!(
                "{} (loss date {} + {} days){}",
                long(d20),
                long(loss),
                s.notify_days,
                if late {
                    " — that window has closed: file today and say so in the claim"
                } else {
                    ""
                }
            ),
        ));
        show.push(S::fact(
            "Documents by",
            "材料截止",
            &format!("{} (loss date + {} days)", long(d90), s.documents_days),
        ));
        dates = json!({"loss_date": iso(loss), "notify_by": iso(d20), "documents_by": iso(d90), "notice_window_closed": late});
    }
    if status == "waiting_on_user" {
        show.push(S::fact(
            "Status",
            "状态",
            "Card Claim Center's sign-in is open in the browser tab (your email filled in): sign in or create your account, then ask me to continue — nothing has been filed",
        ));
        must.push("the sign-in is the user's; nothing was filed".into());
    }
    show.push(S::fact("Decides the claim", "审核方", s.decides));
    let doctor = if t.has_doctor_letter {
        "your doctor's letter (have — upload it; the administrator's physician statement form may follow)"
    } else {
        "the physician statement, signed by the treating doctor (to get)"
    };
    let supplier = if t.has_supplier_letter {
        "the supplier's written refund / non-refundable statement (have, from the refund email)"
    } else {
        "the supplier's written refund / non-refundable statement (ask the airline or hotel for it in writing)"
    };
    let mut v = json!({
        "status": status, "claim": s.name, "benefit": t.benefit, "card": t.card_product,
        "amount": amount, "arithmetic": t.arithmetic(), "reason_filed": t.reason,
        "bookings": t.suppliers.iter().map(|(a, b)| json!({"supplier": a, "reference": b})).collect::<Vec<_>>(),
        "trip": t.trip, "portal": s.start_url,
        "documents": [doctor, "the e-ticket / itinerary", "the card statement showing the charge", supplier],
        "decides": s.decides,
        "excluded": "pre-existing conditions (treated in the 60 days before the booking), a change of plans, travel against medical advice, trips longer than 60 days",
        "limits": "USD 10,000 per traveller, 20,000 per trip, 40,000 per 12 months",
        "show": show,
        "hints": must,
    });
    if let (Some(o), Some(d)) = (v.as_object_mut(), dates.as_object()) {
        o.extend(d.clone());
    }
    v
}

// ---------------------------------------------------------------- the FWD delay claims

/// The delay claim's result: the channel and the deadline, the delay, the amount by the policy's rule, the status.
fn delay_result(s: &ClaimSpec, d: &DelayClaim, status: &str, extra: Value) -> Value {
    let cur = s.currency;
    let must: Vec<String> = Vec::new();
    // the facts the user must read: appended to the reply by code (agent/show.rs)
    let mut show = vec![
        S::fact(
            "Delay",
            "延误",
            &format!(
                "{}{} on {}: scheduled {}, departed {} — delayed {}",
                d.flight_no,
                d.route(),
                d.incident_date,
                d.scheduled,
                d.actual,
                hours_text(d.minutes)
            ),
        ),
        S::amount("Claim amount", "理赔金额", d.amount, cur),
        S::fact(
            "Rule",
            "赔付规则",
            &format!("{}, policy {}", d.rule(cur), d.policy_no),
        ),
    ];
    match d.deadline() {
        Some(due) => {
            let (en, zh) = if s.flow == Flow::FwdHkClaim {
                ("Claim by", "理赔截止")
            } else {
                ("Tell FWD by", "通知 FWD 截止")
            };
            show.push(S::fact(
                en,
                zh,
                &format!(
                    "{} (incident + {} days{})",
                    long(due),
                    d.limit_days,
                    if d.limit_from_policy {
                        ", from your policy email"
                    } else {
                        ""
                    }
                ),
            ));
        }
        None => show.push(S::fact(
            "Time limit",
            "时限",
            "could not be checked in your mail: file as soon as possible",
        )),
    }
    if !d.emergency.is_empty() {
        show.push(S::fact(
            "24-hour emergency assistance",
            "24 小时紧急援助",
            &format!("{} (from your policy email)", d.emergency),
        ));
    }
    let mut v = json!({
        "status": status, "claim": s.name, "decides": s.decides, "policy_no": d.policy_no, "plan": d.plan,
        "flight": d.flight_no, "incident_date": d.incident_date, "scheduled": d.scheduled, "actual": d.actual,
        "delay": hours_text(d.minutes), "amount": format!("{cur} {}", money(d.amount)), "rule": d.rule(cur),
        "deadline": d.deadline().map(iso), "emergency_phone": d.emergency, "show": show, "hints": must,
    });
    if let (Some(o), Some(e)) = (v.as_object_mut(), extra.as_object()) {
        for (k, x) in e {
            if k == "hints" || k == "show" {
                if let (Some(Value::Array(a)), Some(b)) = (o.get_mut(k), x.as_array()) {
                    a.extend(b.iter().cloned());
                }
            } else {
                o.insert(k.clone(), x.clone());
            }
        }
    }
    v
}

/// A delay under the policy's first block: nothing to claim.
fn no_benefit(s: &ClaimSpec, d: &DelayClaim) -> Step {
    Step::Done(
        json!({"status": "no_benefit", "claim": s.name, "flight": d.flight_no, "delay": hours_text(d.minutes),
        "show": [S::fact("No travel delay benefit due", "没有航班延误赔付", &format!("{} on {} was delayed {}: under the policy's first {}-hour block ({} {} for every {} hours); nothing was filed",
            d.flight_no, d.incident_date, hours_text(d.minutes), d.block_hours, s.currency, whole(d.per_block), d.block_hours))],
        "hints": []}),
    )
}

fn next_sg_claim(
    s: &ClaimSpec,
    args: &Value,
    transcript: &[Value],
    run: &[Value],
    ids: &Ids,
    m: &Machine,
) -> Step {
    let d = DelayClaim::from_args(s, args);
    if d.minutes > 0 && d.amount == 0.0 {
        return no_benefit(s, &d);
    }
    // FWD's online claim takes a trip that has ended: its return date's calendar has no day after today (walked
    // 2026-11-03 — a future return left the calendar open and the form stuck). Before the return: the facts, the
    // deadline and a reminder for the day after the return; nothing is filled.
    if let Some(back) = parse_iso(&s_arg(args, "return_sg")).filter(|b| *b > m.today) {
        if results_of(run, "schedule_create").is_empty() {
            return ids.one(
                "schedule_create",
                json!({"name": format!("FWD claim {} after return", d.flight_no),
                       "goal": format!("Remind me to file my FWD travel delay claim for {} on {} (policy {}) now that I am back; FWD's online claim takes the trip once it has ended.", d.flight_no, d.incident_date, d.policy_no),
                       "kind": "cron", "spec": cron_at(back + Duration::days(1))}),
            );
        }
        let show = vec![
            S::fact(
                "Not filed yet",
                "尚未提交",
                &format!(
                    "FWD's online claim takes a trip once it has ended: file it after you are back on {} — a reminder is set for {}",
                    long(back),
                    long(back + Duration::days(1))
                ),
            ),
            S::link("FWD's online claim", "FWD 网上理赔", "FWD", s.start_url),
        ];
        return Step::Done(delay_result(
            s,
            &d,
            "after_return",
            json!({"return": iso(back), "portal": s.start_url, "show": show,
                   "hints": ["nothing was filled or submitted: FWD's online claim opens once the trip has ended"]}),
        ));
    }
    let att = attachment(transcript, &d.certificate);
    let base = delay_result(s, &d, "", json!({}));
    // the airline's delay letter from the mail, saved for the upload
    if let Some((id, name)) = &att {
        if results_of(run, "gmail_save_attachment").is_empty() {
            return ids.one(
                "gmail_save_attachment",
                json!({"message_id": id, "filename": name}),
            );
        }
    }
    if !opened(run, s.host, "/claim-online/") {
        return ids.one("browser_navigate", json!({"url": s.start_url}));
    }
    // 1. every step of FWD's online claim, up to its declaration (the form function, docs/sites/fwd-claim.md): no
    //    hand-over — the user approves the submission in the app (the user, 2026-11-03)
    let mut form = d.form_args(s.currency);
    for k in ["leave_sg", "return_sg", "destination"] {
        form[k] = json!(s_arg(args, k));
    }
    for k in CLAIM_FILES {
        let f = s_arg(args, k);
        if !f.is_empty() {
            form[k] = json!(f);
        }
    }
    let letter = saved_path(run);
    if s_arg(args, "airline_letter_file").is_empty() {
        if let Some(p) = &letter {
            form["airline_letter_file"] = json!(p);
        }
    }
    let stage = |name: &str| {
        results_of(run, "form_fill")
            .into_iter()
            .find(|(a, _)| sget(a, "stage") == name)
            .map(|(_, msg)| msg)
    };
    let Some(filled) = stage("claim") else {
        let mut a = form.clone();
        a["stage"] = json!("claim");
        return ids.one("form_fill", a);
    };
    if let Some(why) = stopped(filled) {
        return finish(Step::Done(sg_claim_stopped(s, &why)), &base);
    }
    let uploaded: Vec<&str> = [
        ("itinerary_file", "the itinerary"),
        ("boarding_pass_file", "the boarding pass"),
        ("airline_letter_file", "the airline's delay letter"),
        ("other_file", "a supporting document"),
    ]
    .into_iter()
    .filter(|(k, _)| !sget(&form, k).is_empty())
    .map(|(_, what)| what)
    .collect();
    // 2. the user's approval of exactly what is submitted, and of the declaration that goes with it
    let Some((_, approval)) = results_of(run, "submit_confirm").into_iter().next() else {
        let mut summary = vec![
            format!("Policy {}", d.policy_no),
            format!(
                "{} on {}: scheduled {}, departed {} — delayed {}",
                d.flight_no,
                d.incident_date,
                d.scheduled,
                d.actual,
                hours_text(d.minutes)
            ),
            format!(
                "Claim: {} {} ({})",
                s.currency,
                money(d.amount),
                d.rule(s.currency)
            ),
            format!(
                "Trip: left Singapore {}, back {}, {}",
                s_arg(args, "leave_sg"),
                s_arg(args, "return_sg"),
                s_arg(args, "destination")
            ),
            "Payment: PayNow to your NRIC/FIN".to_string(),
        ];
        summary.push(if uploaded.is_empty() {
            "Documents: none attached (FWD may ask for them)".to_string()
        } else {
            format!("Documents: {}", uploaded.join(", "))
        });
        return ids.one(
            "submit_confirm",
            json!({"site": s.host, "form": "FWD Singapore travel delay claim", "summary": summary,
                   "declaration": SG_CLAIM_DECLARATION}),
        );
    };
    if declined(approval) {
        if stage("cancel").is_none() {
            return ids.one("form_fill", json!({"stage": "cancel"}));
        }
        let mut show = vec![S::fact(
            "Not submitted",
            "未提交",
            "you declined the submission: FWD's claim form was filled in and left unsubmitted — nothing was sent to FWD",
        )];
        show.push(S::link(
            "FWD's online claim",
            "FWD 网上理赔",
            "FWD",
            s.start_url,
        ));
        return Step::Done(delay_result(
            s,
            &d,
            "not_submitted",
            json!({"portal": s.start_url, "show": show, "hints": ["nothing was submitted: the user declined"]}),
        ));
    }
    // 3. submitted, then the page FWD lands on: its address (FWD's claim pages carry `?hash=<claim>`) and heading
    let Some(sent) = stage("submit") else {
        return ids.one("form_fill", json!({"stage": "submit"}));
    };
    let sent_pos = pos(run, sent);
    let Some((_, landed)) = results_of(run, "browser_read_elements")
        .into_iter()
        .find(|(_, x)| pos(run, x) > sent_pos)
    else {
        return ids.one(
            "browser_read_elements",
            json!({"read": [{"name": "heading", "sel": "h1, h2, h3, h4", "max": 4}]}),
        );
    };
    let page = get_str(&json_of(landed), "url");
    let claim_link = page.contains("hash=").then(|| page.clone());
    let headings = super::confirm::element_values(landed, "heading");
    let email = get_str(m.profile, "email_personal");
    let email = if email.is_empty() {
        get_str(m.profile, "email")
    } else {
        email
    };
    let mut show: Vec<Value> = vec![S::fact(
        "Submitted",
        "已提交",
        &format!(
            "FWD's online claim was submitted after your approval{}",
            headings
                .first()
                .map(|h| format!(" — FWD's page says \"{h}\""))
                .unwrap_or_default()
        ),
    )];
    match &claim_link {
        Some(link) => show.push(S::link("Your claim on FWD", "你的 FWD 理赔", "FWD", link)),
        None => show.push(S::fact(
            "Claim link",
            "理赔链接",
            &format!(
                "FWD emails it to {} — open it with your mobile number to follow the claim and add documents",
                if email.is_empty() { "your email" } else { &email }
            ),
        )),
    }
    // a weekly look in the mail for FWD's answer
    if results_of(run, "schedule_create").is_empty() {
        return ids.one(
            "schedule_create",
            json!({"name": format!("FWD claim check {}", d.policy_no),
                   "goal": format!("Search my mail (gmail_search) for any email from FWD about my travel delay claim on policy {} and notify me of news; stop when the claim is paid or closed.", d.policy_no),
                   "kind": "interval", "spec": "10080"}),
        );
    }
    show.push(S::fact(
        "Reminder set",
        "已设提醒",
        "a weekly mail check for FWD's answer",
    ));
    Step::Done(delay_result(
        s,
        &d,
        "submitted",
        json!({"portal": s.start_url, "claim_link": claim_link, "landed": page, "documents": uploaded,
               "show": show, "hints": ["submitted by code after the user's approval in the app"]}),
    ))
}

/// What the user declares by submitting FWD's claim (FWD's own words, shortened), shown in the approval.
const SG_CLAIM_DECLARATION: &str = "By submitting you confirm the information is true, complete and accurate; that anyone \
claimed for agreed to share their information; that you will produce the original bills on request; that you have not \
claimed from any other insurer; you agree to FWD's Privacy Policy and to indemnify FWD for any untrue, incomplete or \
inaccurate information.";

/// The approval was declined (the user's "no", or Sentinel's refusal).
fn declined(m: &Value) -> bool {
    let c = content_of(m);
    c.starts_with("DENIED") || c.starts_with("ERROR") || get_str(&json_of(m), "status") == "denied"
}

/// FWD's claim form stopped before its declaration (a step that did not open): the facts and the claim page, nothing
/// submitted.
fn sg_claim_stopped(s: &ClaimSpec, why: &str) -> Value {
    json!({"status": "stopped", "why": why, "portal": s.start_url,
           "show": [S::fact("Stopped", "已停止", &format!("{why} — nothing was submitted")),
                    S::link("FWD's online claim", "FWD 网上理赔", "FWD", s.start_url)],
           "hints": ["nothing was submitted"]})
}

fn next_hk_claim(
    s: &ClaimSpec,
    args: &Value,
    transcript: &[Value],
    run: &[Value],
    ids: &Ids,
    m: &Machine,
) -> Step {
    let d = DelayClaim::from_args(s, args);
    if d.minutes > 0 && d.amount == 0.0 {
        return no_benefit(s, &d);
    }
    if let Some((id, name)) = attachment(transcript, &d.certificate) {
        if results_of(run, "gmail_save_attachment").is_empty() {
            return ids.one(
                "gmail_save_attachment",
                json!({"message_id": id, "filename": name}),
            );
        }
    }
    // the claim form: bolttech's fillable PDF at its fixed address on FWD's site — opening it downloads it (2026-11-02);
    // the download's path from the navigation's result, else the browser's downloads
    let pdf_path = results_of(run, "browser_navigate")
        .iter()
        .find_map(|(_, m)| {
            let p = get_str(&json_of(m)["pdf"], "path");
            (!p.is_empty()).then_some(p)
        })
        .or_else(|| {
            results_of(run, "browser_downloads")
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
        None if !opened(run, s.host, "/files/") => {
            return ids.one("browser_navigate", json!({"url": s.start_url}))
        }
        None if results_of(run, "browser_downloads").is_empty() => {
            return ids.one("browser_downloads", json!({}))
        }
        None => {
            return Step::Done(delay_result(
                s,
                &d,
                "error",
                json!({"show": [S::fact("Claim form not opened", "理赔表未能打开", "bolttech's Travel Insurance Claim Form (the PDF on FWD Hong Kong's site) could not be downloaded: upload the form to this chat and I will fill it in")]}),
            ))
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
    let filled = results_of(run, "pdf_form_fill")
        .last()
        .map(|(_, m)| get_str(&json_of(m), "output"))
        .filter(|p| !p.is_empty());
    let Some(filled) = filled else {
        return ids.one(
            "pdf_form_fill",
            json!({"path": form_path, "values": crate::forms::bolttech_delay_claim(&d.form_args(s.currency), m.profile), "output": output}),
        );
    };
    let letter = saved_path(run).unwrap_or_default();
    if results_of(run, "send_file").is_empty() {
        return ids.one(
            "send_file",
            json!({"path": filled, "note": "bolttech's travel claim form, filled in — add your HKID number, bank account, signature and date before sending"}),
        );
    }
    // the draft and the mail check need a connected mailbox (`mailbox` in the machine's profile)
    let mailbox = !get_str(m.profile, "mailbox").trim().is_empty();
    if mailbox && results_of(run, "gmail_create_draft").is_empty() {
        let mut atts = vec![filled.clone()];
        if !letter.is_empty() {
            atts.push(letter.clone());
        }
        let name = get_str(m.profile, "name_en");
        let body = format!(
            "Dear bolttech claims team,\n\nPlease find attached my travel insurance claim for a flight delay.\n\nPolicyholder: {name}\nPolicy number: {}{}\nFlight: {}{} on {}, scheduled departure {}, actual departure {} — delayed {}{}.\nAmount claimed: {} {} ({} × {} hours of delay).\n\nAttachments: the signed claim form{}.\n\nKind regards,\n{name}",
            d.policy_no,
            if d.plan.is_empty() { String::new() } else { format!(" ({})", d.plan) },
            d.flight_no, d.route(), d.incident_date, d.scheduled, d.actual, hours_text(d.minutes),
            if d.reason.is_empty() { String::new() } else { format!(" ({})", d.reason) },
            s.currency, money(d.amount), d.blocks(), d.block_hours,
            if letter.is_empty() { String::new() } else { format!(", the airline's delay letter{}", if d.airline_ref.is_empty() { String::new() } else { format!(" (reference {})", d.airline_ref) }) },
        );
        return ids.one(
            "gmail_create_draft",
            json!({"to": s.claims_email,
                   "subject": format!("Travel insurance claim — policy {} — flight delay {} {}", d.policy_no, d.flight_no, d.incident_date),
                   "body": body, "attachments": atts}),
        );
    }
    if mailbox && results_of(run, "schedule_create").is_empty() {
        return ids.one(
            "schedule_create",
            json!({"name": format!("bolttech claim check {}", d.policy_no),
                   "goal": format!("Search my mail (gmail_search) for any email from bolttech or FWD about my travel delay claim on policy {} and notify me of news; stop when the claim is paid or closed.", d.policy_no),
                   "kind": "interval", "spec": "10080"}),
        );
    }
    let airline = if d.airline.is_empty() {
        "the airline".to_string()
    } else {
        d.airline.clone()
    };
    // the draft needs a mailbox: without one (an error result) the user sends the form themselves
    let drafted = results_of(run, "gmail_create_draft")
        .last()
        .is_some_and(|(_, m)| !super::bagtrace::failed(m));
    let email = if drafted {
        S::fact(
            "Email draft (not sent)",
            "邮件草稿（未发送）",
            &format!(
                "to {} in your Drafts{} — sign the form, replace it in the draft and send it",
                s.claims_email,
                if letter.is_empty() {
                    " with the form attached".to_string()
                } else {
                    format!(" with the form and {airline}'s delay letter attached")
                }
            ),
        )
    } else {
        S::fact("Send it to", "发送到", &format!("{} — no mailbox is connected, so no draft was made: sign the form and email it with the airline's delay letter", s.claims_email))
    };
    let mut show = vec![
        S::fact("Handled by", "处理方", s.decides),
        S::file("Claim form (filled in)", "理赔表（已填）", "", &filled),
        S::fact(
            "You still add",
            "你还需填写",
            "your HKID card number, the bank account for the payment, your signature and the date",
        ),
        email,
    ];
    if results_of(run, "schedule_create")
        .last()
        .is_some_and(|(_, m)| !super::bagtrace::failed(m))
    {
        show.push(S::fact(
            "Reminder set",
            "已设提醒",
            "a weekly mail check for the insurer's reply",
        ));
    }
    Step::Done(delay_result(
        s,
        &d,
        if drafted { "drafted" } else { "form_filled" },
        json!({"form": filled, "draft_to": s.claims_email, "drafted": drafted, "letter_attached": !letter.is_empty(),
        "show": show, "hints": []}),
    ))
}

// ---------------------------------------------------------------- the FWD Singapore purchase

/// FWD Hong Kong's MyTravel (bolttech underwrites; walked 2026-11-03): the quote, the plans read from their cards, the
/// pick by the user's rule, the comparison card; for a purchase the plan's card (FWD creates the application) and the
/// insured's details saved with Next — the application's link, which opens prefilled on any device, is the result.
fn next_buy_hk(args: &Value, run: &[Value], ids: &Ids) -> Step {
    let dest = s_arg(args, "destination");
    let (dep, ret) = (s_arg(args, "depart"), s_arg(args, "return"));
    let trip = format!("{dest}, {dep} to {ret}, 1 traveller");
    let insurer = "FWD Hong Kong (MyTravel, underwritten by bolttech Insurance (Hong Kong))";
    if !opened(
        run,
        "www.fwd.com.hk",
        "/online-insurance/mytravel-insurance",
    ) {
        return ids.one("browser_navigate", json!({"url": HK_QUOTE_URL}));
    }
    let stage = |name: &str| {
        results_of(run, "form_fill")
            .into_iter()
            .find(|(a, _)| sget(a, "stage") == name)
            .map(|(_, msg)| msg)
    };
    let stopped_at = |why: &str| {
        Step::Done(
            json!({"status": "stopped", "why": why, "insurer": insurer, "trip": trip, "quote_page": HK_QUOTE_URL,
            "show": [S::fact("Stopped", "已停止", &format!("{why} — nothing was bought")),
                     S::link("FWD's MyTravel page", "FWD MyTravel 页面", "FWD Hong Kong", HK_QUOTE_URL)],
            "hints": []}),
        )
    };
    let Some(quoted) = stage("quote") else {
        return ids.one(
            "form_fill",
            json!({"stage": "quote", "destination": dest, "depart": dep, "return": ret}),
        );
    };
    if let Some(why) = stopped(quoted) {
        return stopped_at(&why);
    }
    let quote_pos = pos(run, quoted);
    let Some((_, read)) = results_of(run, "browser_read_elements")
        .into_iter()
        .find(|(_, x)| pos(run, x) > quote_pos)
    else {
        return ids.one(
            "browser_read_elements",
            json!({"read": [{"name": "plans", "sel": "[data-testid=PlanOption]", "max": 6,
                             "fields": {"name": "[data-part=name]", "premium": {"sel": "[data-part=premium]", "attr": "data-value"},
                                        "medical": {"sel": "[data-part=currency]", "attr": "data-value"}}}]}),
        );
    };
    let plans = hk_plans_of(read);
    let minimum = match (s_arg(args, "cover"), n_arg(args, "cover_at_least")) {
        (c, Some(x)) if !c.is_empty() => Some((c, x)),
        _ => None,
    };
    let Some((picked, rule)) = pick_plan(&plans, &minimum, &s_arg(args, "plan")) else {
        return stopped_at("FWD's MyTravel page did not price its plans for these dates");
    };
    let listed: Vec<String> = plans.iter().map(|p| p.summary("HKD")).collect();
    let mut show = vec![
        S::fact("Plans quoted", "报价方案", &listed.join("; ")),
        S::amount(
            &format!("Chosen: {}", picked.name),
            &format!("已选：{}", picked.name),
            picked.premium,
            "HKD",
        ),
        S::fact("Why", "选择理由", &rule),
    ];
    let mut result = json!({"insurer": insurer, "trip": trip, "plans": listed, "picked": picked.name,
        "premium": format!("HKD {}", money(picked.premium)), "rule": rule, "quote_page": HK_QUOTE_URL});
    if results_of(run, "present_choices").is_empty() {
        let options: Vec<Value> = plans
            .iter()
            .map(|p| json!({"label": p.name, "source_url": HK_QUOTE_URL,
                            "details": p.cover.iter().map(|(c, v)| format!("{c} HKD {}", whole(*v))).collect::<Vec<_>>(),
                            "note": if p.name == picked.name { format!("picked: {rule}") } else { String::new() }}))
            .collect();
        return ids.one(
            "present_choices",
            json!({"question": format!("FWD Hong Kong MyTravel plans for {trip}: the premium and the covers"),
                   "kind": "comparison", "options": options, "picked": picked.name, "_code": true}),
        );
    }
    if s_arg(args, "action") == "compare" {
        result["status"] = json!("shown");
        result["show"] = json!(show);
        return Step::Done(result);
    }
    // the plan's card: FWD creates the application; then the insured's details, kept by FWD after Next
    let Some(pick) = stage("pick") else {
        return ids.one("form_fill", json!({"stage": "pick", "plan": picked.name}));
    };
    if let Some(why) = stopped(pick) {
        return stopped_at(&why);
    }
    let Some(details) = stage("details") else {
        return ids.one(
            "form_fill",
            json!({"stage": "details", "title": s_arg(args, "title"), "hkid_issue_date": s_arg(args, "hkid_issue_date"),
                   "family_name": s_arg(args, "family_name"), "given_names": s_arg(args, "given_names")}),
        );
    };
    let landed = get_str(&json_of(details), "url");
    let link = hk_application_link(&landed);
    match (stopped(details), link) {
        (None, Some(link)) => {
            show.push(S::link(
                "Your FWD application",
                "你的 FWD 投保申请",
                "Continue on FWD",
                &link,
            ));
            show.push(S::fact(
                "Next steps (yours)",
                "下一步（你的）",
                &format!("open the link on any device: your details are saved in FWD's application for the {} plan (HKD {}); choose any add-on, confirm FWD's declarations and pay there — nothing has been paid", picked.name, money(picked.premium)),
            ));
            result["status"] = json!("saved");
            result["quote_link"] = json!(link);
            result["show"] = json!(show);
            Step::Done(result)
        }
        (why, link) => {
            // the details not saved (a field FWD refused): the application still opens at its first step
            let why = why.unwrap_or_else(|| "FWD did not open its next step".into());
            let mut show = show;
            if let Some(link) = &link {
                show.push(S::link(
                    "Your FWD application",
                    "你的 FWD 投保申请",
                    "Continue on FWD",
                    link,
                ));
            }
            show.push(S::fact(
                "Details not saved",
                "资料未保存",
                &format!("{why} — fill them in on FWD's page; nothing has been paid"),
            ));
            result["status"] = json!("details_needed");
            result["quote_link"] = json!(link);
            result["show"] = json!(show);
            Step::Done(result)
        }
    }
}

/// The plans on FWD Hong Kong's MyTravel cards: the name, the premium FWD charges (`data-value`, after its offer) and
/// the medical cover (the card's first amount), all from fixed elements.
fn hk_plans_of(read: &Value) -> Vec<Plan> {
    super::confirm::element_fields(read, "plans")
        .into_iter()
        .filter_map(|f| {
            let name = f.get("name")?.trim().to_string();
            let premium = f.get("premium")?.trim().parse::<f64>().ok()?;
            if name.is_empty() || !name.chars().all(|c| c.is_ascii_alphabetic()) || premium <= 0.0 {
                return None;
            }
            let cover = f
                .get("medical")
                .and_then(|m| m.trim().parse::<f64>().ok())
                .map(|m| vec![("medical".to_string(), m)])
                .unwrap_or_default();
            Some(Plan {
                name,
                link: String::new(),
                premium,
                cover,
            })
        })
        .collect()
}

/// The application's own link from the page FWD Hong Kong landed on (`…/<step>/?leadId=<id>`): its first step,
/// `…/application/?leadId=<id>`, which opens the saved application on any device. `None` without a lead id.
fn hk_application_link(landed: &str) -> Option<String> {
    let u = url::Url::parse(landed).ok()?;
    if u.host_str()? != "www.fwd.com.hk" {
        return None;
    }
    let lead = u
        .query_pairs()
        .find(|(k, _)| k == "leadId")
        .map(|(_, v)| v.to_string())
        .filter(|v| !v.is_empty() && v.chars().all(|c| c.is_ascii_alphanumeric()))?;
    Some(format!(
        "https://www.fwd.com.hk/online-insurance/mytravel-insurance/en/application/?leadId={lead}"
    ))
}

fn next_buy(args: &Value, run: &[Value], ids: &Ids, m: &Machine) -> Step {
    let s = spec(Flow::FwdSgBuy);
    let dest = s_arg(args, "destination");
    let (dep, ret) = (s_arg(args, "depart"), s_arg(args, "return"));
    let travellers = i_arg(args, "travellers").unwrap_or(1).max(1);
    let trip = format!("{dest}, {dep} to {ret}, {travellers} traveller(s)");
    if !opened(run, s.host, "/travel-insurance/online-quote") {
        return ids.one("browser_navigate", json!({"url": s.start_url}));
    }
    let quote = json!({"destination": dest, "depart": dep, "return": ret, "travellers": travellers.to_string(),
                       "today": iso(m.today)});
    let base = json!({"insurer": s.decides, "trip": trip});
    let quote_pos = match fill_gate(run, ids, "quote", quote, s.name) {
        Ok(p) => p,
        Err(st) => return finish(st, &base),
    };
    // the plans the summary lists, read from its table's elements after the quote
    let (plans, plan_pos) = match results_of(run, "browser_read_elements")
        .iter()
        .find(|(_, x)| pos(run, x) > quote_pos)
    {
        Some((_, x)) => (plans_of(x), pos(run, x)),
        None => return ids.one("browser_read_elements", plans_read_args()),
    };
    let minimum = match (s_arg(args, "cover"), n_arg(args, "cover_at_least")) {
        (c, Some(x)) if !c.is_empty() => Some((c, x)),
        _ => None,
    };
    let Some((picked, rule)) = pick_plan(&plans, &minimum, &s_arg(args, "plan")) else {
        return Step::Done(
            json!({"status": "nothing_found", "insurer": s.decides, "trip": trip, "quote_page": s.start_url,
            "show": [S::fact("No plans listed", "没有可选方案", &format!("FWD's quote page listed no plans for {trip}: open the browser tab to see what it says — nothing was bought"))],
            "hints": []}),
        );
    };
    let listed: Vec<String> = plans.iter().map(|p| p.summary(s.currency)).collect();
    let must = vec![
        format!("FWD Singapore travel insurance for {trip}"),
        format!(
            "chosen: {}, {} {} — {rule}",
            picked.name,
            s.currency,
            money(picked.premium)
        ),
    ];
    // the facts the user must read: appended to the reply by code (agent/show.rs)
    let mut show = vec![
        S::fact("Plans quoted", "报价方案", &listed.join("; ")),
        S::amount(
            &format!("Chosen: {}", picked.name),
            &format!("已选：{}", picked.name),
            picked.premium,
            s.currency,
        ),
        S::fact("Why", "选择理由", &rule),
    ];
    let mut result = json!({"insurer": s.decides, "trip": trip, "plans": listed, "picked": picked.name,
        "premium": format!("{} {}", s.currency, money(picked.premium)), "rule": rule, "quote_page": s.start_url});
    let choices = results_of(run, "present_choices");
    let Some((_, shown)) = choices.last() else {
        let options: Vec<Value> = plans
            .iter()
            .map(|p| {
                json!({"label": p.name, "source_url": s.start_url,
                       "details": p.cover.iter().map(|(c, v)| format!("{c} {} {}", s.currency, whole(*v))).collect::<Vec<_>>(),
                       "note": if p.name == picked.name { format!("picked: {rule}") } else { String::new() }})
            })
            .collect();
        return ids.one(
            "present_choices",
            json!({"question": format!("FWD Singapore plans for {trip}: the premium and the covers"),
                   "kind": "comparison", "options": options, "picked": picked.name}),
        );
    };
    if s_arg(args, "action") == "compare" {
        result["status"] = json!("shown");
        result["show"] = json!(show);
        result["hints"] = json!(must);
        return Step::Done(result);
    }
    let shown_pos = pos(run, shown).max(plan_pos);
    // the quote saved under the user's details, and FWD's own link to it: the user carries on in their own browser on
    // any device, no takeover of ours (the user, 2026-11-02). Only with a Singapore mobile (FWD's save needs one, and
    // its link asks for it); else the hand-over below.
    let (code, number) = crate::forms::phone_parts(&get_str(m.profile, "phone"));
    if code == "65" && number.len() == 8 {
        let save_pos = match fill_gate(run, ids, "save", json!({}), s.name) {
            Ok(p) => p.max(shown_pos),
            Err(st) => return finish(st, &base),
        };
        let Some((_, saved)) = results_of(run, "browser_read_elements")
            .into_iter()
            .find(|(_, x)| pos(run, x) > save_pos)
        else {
            return ids.one("browser_read_elements", saved_read_args());
        };
        if let Some(link) = saved_quote(saved) {
            let (url, quotation) = link;
            let status_line = format!(
                "the quote is saved with FWD under your name, email and mobile; nothing has been paid. Open the link on any device and enter your mobile number (ending {}): FWD's application starts with its eligibility statements (yours to confirm), then the plan — choose {} ({} {}; the saved quote opens on FWD's first plan) — your details and Pay",
                &number[4..],
                picked.name,
                s.currency,
                money(picked.premium)
            );
            show.push(S::link(
                "Your saved FWD quote",
                "你保存的 FWD 报价",
                "Continue on FWD",
                &url,
            ));
            if !quotation.is_empty() {
                show.push(S::reference("FWD quotation", "FWD 报价编号", &quotation));
            }
            show.push(S::fact(
                "Next steps (yours)",
                "下一步（你的）",
                &status_line,
            ));
            result["status"] = json!("saved");
            result["quote_link"] = json!(url);
            result["quotation_no"] = json!(quotation);
            result["show"] = json!(show);
            result["hints"] = json!(must);
            return Step::Done(result);
        }
        // the save did not go through (its dialog still open, or no quote kept): the hand-over below
    }
    // the plan's button on the summary; FWD then asks its eligibility statements — the user's own answers
    let pick_pos = match fill_gate(run, ids, "pick", json!({"plan": picked.name}), s.name) {
        Ok(p) => p.max(shown_pos),
        Err(st) => return finish(st, &base),
    };
    let status_line = format!(
        "the {} plan is chosen on FWD's page; FWD's eligibility statements, the add-ons, your details and Pay are yours — nothing has been paid",
        picked.name
    );
    let base = {
        let mut b = result.clone();
        let mut ss = show.clone();
        ss.push(S::fact("Status", "状态", &status_line));
        b["show"] = json!(ss);
        b["hints"] = json!(must);
        b
    };
    let reason = format!(
        "The {} plan ({} {}) is chosen on FWD's quote. FWD now asks you to confirm its eligibility statements (a Singapore resident with an NRIC or FIN, never refused travel insurance, nothing material withheld): only you can answer them. Then {}; hand the browser back when done (or without paying).",
        picked.name,
        s.currency,
        money(picked.premium),
        s.user_steps
    );
    if let Err(st) = review_gate(run, ids, pick_pos, reason) {
        return finish(st, &base);
    }
    show.push(S::fact("Status", "状态", &status_line));
    result["status"] = json!("handed_over");
    result["show"] = json!(show);
    result["hints"] = json!(must);
    Step::Done(result)
}

// ---------------------------------------------------------------- the WorldTracer lookup

fn next_bag(args: &Value, run: &[Value], ids: &Ids) -> Step {
    use super::bagtrace;
    let file_ref = s_arg(args, "file_ref");
    let airline = bagtrace::airline_of(&file_ref);
    let flight_date = parse_iso(&s_arg(args, "flight_date"));
    let lost_after = flight_date.map(|d| {
        S::fact(
            "Treated as lost after",
            "视为遗失的日期",
            &format!(
                "{} (21 days after {}): the lost-bag claim starts then",
                long(d + Duration::days(21)),
                long(d)
            ),
        )
    });
    let watch = b_arg(args, "watch");
    let done = |status: &str, mut show: Vec<Value>, page: &str| {
        show.extend(lost_after.clone());
        Step::Done(
            json!({"status": status, "file_ref": file_ref, "airline": airline, "watch": watch,
            "lost_after": flight_date.map(|d| iso(d + Duration::days(21))), "tracing_page": page,
            "show": show, "hints": []}),
        )
    };
    // the airline's page on WorldTracer's passenger site; an airline not on it is said so, nothing opened
    let Some(page) = bagtrace::manage_url(&airline) else {
        return done(
            "not_on_worldtracer",
            vec![
                S::reference("WorldTracer file", "WorldTracer 文件号", &file_ref),
                S::fact(
                    "Bag status",
                    "行李状态",
                    &format!("not looked up: the airline {airline} does not track bags on WorldTracer's passenger site (mybag.aero) — use the airline's own baggage page or app with the file reference"),
                ),
            ],
            bagtrace::TRACE_URL,
        );
    };
    let fail = |why: &str| {
        done(
            "error",
            vec![
                S::fact(
                    &format!("WorldTracer not checked for file {file_ref}"),
                    &format!("未能在 WorldTracer 查询文件 {file_ref}"),
                    why,
                ),
                S::link("Track it yourself", "自己查询", "WorldTracer", &page),
            ],
            &page,
        )
    };
    let Some((_, nav)) = results_of(run, "browser_navigate").into_iter().next() else {
        return ids.one("browser_navigate", json!({"url": page}));
    };
    if bagtrace::failed(nav) {
        return fail("the airline's baggage page did not open");
    }
    let Some((_, fill)) = results_of(run, "form_fill").into_iter().next() else {
        let mut a = json!({"file_ref": file_ref});
        let family = s_arg(args, "family_name");
        if !family.is_empty() {
            a["family_name"] = json!(family);
        }
        return ids.one("form_fill", a);
    };
    if bagtrace::failed(fill) {
        return fail("the form could not be filled");
    }
    if let Some(why) = stopped(fill) {
        return fail(&why);
    }
    let Some((_, read)) = results_of(run, "browser_read_elements").into_iter().next() else {
        return ids.one("browser_read_elements", bagtrace::read_args());
    };
    let trace = bagtrace::trace_of(read, &file_ref);
    // a check twice a day only on a file WorldTracer has (claims_real 2026-11-02: a check was made on a file it did not
    // find, the model deleted it, and the block still said there was one)
    let found = trace.as_ref().is_some_and(|t| t.found);
    if watch && found && results_of(run, "schedule_create").is_empty() {
        return ids.one(
            "schedule_create",
            json!({"name": format!("Bag {file_ref} check"),
                   "goal": format!("Check the status of my delayed bag on WorldTracer (file {file_ref}, {page}) and notify me when it changes; stop when the bag is delivered or the claim is closed."),
                   "kind": "interval", "spec": "720"}),
        );
    }
    let Some(t) = trace else {
        return fail("WorldTracer gave no answer to the lookup (it may have asked for a check such as a CAPTCHA)");
    };
    let mut show = vec![
        S::reference("WorldTracer file", "WorldTracer 文件号", &file_ref),
        S::fact(
            "Bag status",
            "行李状态",
            &if t.found {
                "WorldTracer has the report: open it for the bag's latest status".to_string()
            } else {
                format!("WorldTracer says: {}", t.status.trim_end_matches('.'))
            },
        ),
        S::link(
            "Your report on WorldTracer",
            "WorldTracer 上的报告",
            "mybag.aero",
            &page,
        ),
    ];
    if watch && t.found {
        show.push(S::fact(
            "Bag check",
            "行李查询",
            "twice a day; you are told when its status changes",
        ));
    } else if watch {
        show.push(S::fact(
            "Bag check",
            "行李查询",
            "not set up — WorldTracer has no file with that reference and family name",
        ));
    }
    done(if t.found { "traced" } else { "no_file" }, show, &page)
}

#[cfg(test)]
#[path = "claims_tests.rs"]
mod tests;
