//! The arrival-card machine behind the workflow tool `arrival_card_fill` (docs/json-tools.md; the protocol is
//! [`super::workflow`]): the arrival cards and travel authorisations with a form function in `crate::forms` (11
//! countries, [`CARDS`]).
//!
//! The model reads the user's words — and the booking mails the machine fetched — and calls the tool with JSON
//! arguments: the country, the flight arriving there, the arrival day, where the flight comes from, the stay, the
//! purpose, the label of the user's own address they chose. [`check`] validates them in code (the country is in the
//! table, a flight number has an airline code and digits, the dates parse and the arrival is not past, the own address
//! is one of the profile's) and decides what happens before any page opens: the fixed mail searches when the trip is
//! not known yet and a mailbox is connected (the machine fetches the bookings and its result `mail_read` has the model
//! call again with the facts it read in them), the choice card when the profile has several addresses in the country,
//! `needs_input` for what only the user can give. Nothing here reads the user's wording or a mail's text.
//!
//! Then every step is code ([`next`]): the reminder when the filing window has not opened, the official page, the
//! form function (`form_fill`, fixed ids), the hand-over for the user's own steps and for the review, the reference read
//! from the confirmation page's fixed element. The machine ends with [`Step::Done`]: a JSON result (status, the
//! reference, the trip, what was assumed, `show` — appended to the reply by code — and `hints`). The model writes the
//! reply from it.
//!
//! The next step is a function of the validated arguments and the transcript after the tool call alone, so a run
//! suspended for a takeover carries on where it was.
//!
//! The shared transcript helpers at the end of the file (`results_of`, `json_of`, …) and [`Step`] serve every
//! code machine.

use std::sync::LazyLock;

use chrono::{Datelike, NaiveDate};
use omuse_common::util::{get_str, truthy};
use regex::Regex;
use serde_json::{json, Map, Value};

use super::show as S;
use super::tools::fn_;
use super::util::{content_of, sget, stored_call, tool_calls};
use super::workflow::{self, Checked};
use crate::llm::ToolCall;

/// One card: what the skill text used to tell the model, as data.
pub struct CardSpec {
    pub skill: &'static str,
    /// the card's name in the answer
    pub name: &'static str,
    /// the country, for the own-address rule and the answer
    pub country: &'static str,
    /// the words that find the trip in mail
    pub mail_words: &'static str,
    /// the official page to open
    pub start_url: &'static str,
    /// its host, to know the page was reached
    pub host: &'static str,
    /// the filing window in days before arrival (0 = none)
    pub window_days: i64,
    /// what the user does after the hand-over (the reason's last part)
    pub user_steps: &'static str,
    /// the confirmation's reference element on the page after the user's Submit (a CSS selector, read by
    /// `browser_read_elements`)
    pub ref_sel: &'static str,
    /// the reference's format, checked over that element's whole value
    pub ref_re: &'static str,
    /// the site's email verification step, run by code (`agent/email_code.rs`) after a fill that stopped at it
    pub email_code: Option<super::email_code::Spec>,
    /// the final page's Submit — the user's click, never code's — by a fixed selector taken from the real page: code
    /// checks it is there before it hands the filled form over (absent = the fill stopped early: `stopped_early`).
    /// `None`: the form lies behind a user's step that could not be passed to map it (a Cloudflare check, a sign-in,
    /// the passport photo)
    pub final_page: Option<Marker>,
    /// the elements that mark the user's own steps that come before the form (a Cloudflare check, the sign-in, the
    /// passport photo, the email code): a hand-over at such a step is right when one of them is on the page
    pub user_step_at: &'static [Marker],
}

/// An element that marks a page: its fixed selector, the text it must show ("" = any; an input's text is empty), and
/// whether a hidden one counts (a Turnstile's response field, a file input).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Marker {
    pub sel: &'static str,
    pub text: &'static str,
    pub hidden: bool,
}

const fn mark(sel: &'static str, text: &'static str) -> Marker {
    Marker {
        sel,
        text,
        hidden: false,
    }
}

const fn mark_hidden(sel: &'static str) -> Marker {
    Marker {
        sel,
        text: "",
        hidden: true,
    }
}

/// The code box after a Send: the first visible text-like input (the email box is `type=email`).
const CODE_BOX: &str = "input[type=text], input[type=tel], input[type=number], input:not([type])";

pub const CARDS: &[CardSpec] = &[
    CardSpec {
        skill: "sg-arrival-card",
        name: "SG Arrival Card",
        country: "Singapore",
        mail_words: "(Singapore OR SIN OR Changi)",
        start_url: "https://eservices.ica.gov.sg/sgarrivalcard/",
        host: "eservices.ica.gov.sg",
        window_days: 3,
        user_steps: "tick the declaration and press Submit",
        ref_sel: ".ref",
        ref_re: r"\bDE\d{9,}\b",
        email_code: None,
        final_page: Some(mark("button.general-button-container", "Submit")),
        user_step_at: &[],
    },
    CardSpec {
        skill: "my-arrival-card",
        name: "Malaysia Digital Arrival Card (MDAC)",
        country: "Malaysia",
        mail_words: "(Malaysia OR \"Kuala Lumpur\" OR KUL OR Penang OR PEN OR Johor OR \"Kota Kinabalu\")",
        // the registration form itself (the landing page's REGISTER opens it)
        start_url: "https://imigresen-online.imi.gov.my/mdac/main?registerMain",
        host: "imigresen-online.imi.gov.my",
        window_days: 3,
        user_steps: "solve the slider puzzle and press Submit",
        ref_sel: ".ref",
        ref_re: r"\bMDAC-[A-Z0-9-]{6,}\b",
        email_code: None,
        final_page: Some(mark("#submit", "")),
        user_step_at: &[],
    },
    CardSpec {
        skill: "kr-arrival-card",
        name: "Korea e-Arrival Card",
        country: "Korea",
        mail_words: "(Korea OR Seoul OR Incheon OR ICN OR GMP OR Busan OR PUS)",
        start_url: "https://www.e-arrivalcard.go.kr/portal/main/index.do?locale=EN",
        host: "www.e-arrivalcard.go.kr",
        window_days: 3,
        user_steps: "press Submit and enter the code Korea emails you",
        ref_sel: ".ref",
        ref_re: r"\bKR-EA-\d{6,}\b",
        email_code: None,
        final_page: Some(mark("#btnSubmit", "Submit")),
        user_step_at: &[],
    },
    CardSpec {
        skill: "id-arrival-card",
        name: "All Indonesia arrival card",
        country: "Indonesia",
        mail_words: "(Indonesia OR Bali OR Denpasar OR DPS OR Jakarta OR CGK OR Surabaya OR Batam)",
        start_url: "https://allindonesia.imigrasi.go.id/",
        host: "allindonesia.imigrasi.go.id",
        window_days: 3,
        user_steps: "add your seat number and baggage count, tick the certification and press Submit",
        ref_sel: ".ref",
        ref_re: r"\bALLIND-\d{6,}\b|\b[A-Z]{2,4}-?\d{8,}\b",
        email_code: None,
        // the Declaration step's Submit (/arrival-card-submission/declaration/individual, the last of the four steps)
        final_page: Some(mark("button", "Submit")),
        user_step_at: &[],
    },
    CardSpec {
        skill: "tw-arrival-card",
        name: "Taiwan Arrival Card (TWAC)",
        country: "Taiwan",
        mail_words: "(Taiwan OR Taipei OR TPE OR TSA OR Kaohsiung OR KHH OR Taichung)",
        start_url: "https://twac.immigration.gov.tw/",
        host: "twac.immigration.gov.tw",
        window_days: 3,
        user_steps: "tick the declaration and press SUBMIT",
        ref_sel: ".ref",
        ref_re: r"\bTWAC-?[A-Z0-9]{6,}\b",
        // step 1 (mapped 2026-11-02): the email box, SEND VERIFICATION CODE, the code box, VERIFY
        email_code: Some(super::email_code::Spec {
            email_sel: "input[name=email]",
            send_text: "SEND VERIFICATION CODE",
            code_sel: CODE_BOX,
            verify_text: "VERIFY",
            from: "immigration.gov.tw",
        }),
        // the stepper's current step is Review (then the user's SUBMIT and Confirmation)
        final_page: Some(mark(".steps li.current .stepTitle", "Review")),
        user_step_at: &[mark("input[name=email]", "")],
    },
    CardSpec {
        skill: "th-arrival-card",
        name: "Thailand Digital Arrival Card (TDAC)",
        country: "Thailand",
        mail_words: "(Thailand OR Bangkok OR BKK OR DMK OR Phuket OR HKT OR \"Chiang Mai\")",
        start_url: "https://tdac.immigration.go.th/",
        host: "tdac.immigration.go.th",
        window_days: 3,
        // the form is behind a Cloudflare check on the home page (the hand-over): the user fills it after the check
        user_steps: "pass the Cloudflare check, fill in the card, type the CAPTCHA and press Submit",
        ref_sel: ".ref",
        ref_re: r"\bTH[A-Z0-9]{7,}\b",
        email_code: None,
        final_page: None,
        user_step_at: &[mark_hidden("input[name=cf-turnstile-response]")],
    },
    CardSpec {
        skill: "cn-arrival-card",
        name: "China online arrival card",
        country: "China",
        mail_words: "(China OR Shanghai OR PVG OR SHA OR Beijing OR PEK OR PKX OR Shenzhen OR Guangzhou OR CAN OR Chengdu OR Hangzhou)",
        start_url: "https://s.nia.gov.cn/ArrivalCardFillingPC/",
        host: "s.nia.gov.cn",
        window_days: 0,
        user_steps: "answer the companion question and press Submit",
        ref_sel: ".ref",
        ref_re: r"\bCN-ARC-[A-Z0-9-]{6,}\b",
        email_code: None,
        final_page: None,
        user_step_at: &[mark_hidden("input[type=file][name=file]")],
    },
    CardSpec {
        skill: "jp-visit-japan-web",
        name: "Visit Japan Web registration",
        country: "Japan",
        mail_words: "(Japan OR Tokyo OR NRT OR HND OR Osaka OR KIX OR Nagoya OR Fukuoka OR Sapporo)",
        // the service moved from vjw-lp.digital.go.jp (2026-11-02): it opens on the sign-in to the user's account
        start_url: "https://www.vjw.digital.go.jp/main/#/vjwplo001",
        host: "www.vjw.digital.go.jp",
        // registered any time before the flight: no filing window
        window_days: 0,
        user_steps: "sign in, register the planned entry and press Register",
        ref_sel: ".ref",
        ref_re: r"\bJP-\d{4}-\d{4}\b",
        email_code: None,
        final_page: None,
        user_step_at: &[mark("#textbox02", "")],
    },
    CardSpec {
        skill: "kh-arrival-card",
        name: "Cambodia e-Arrival Card",
        country: "Cambodia",
        mail_words: "(Cambodia OR \"Phnom Penh\" OR PNH OR \"Siem Reap\" OR SAI OR REP)",
        start_url: "https://arrival.gov.kh/",
        host: "arrival.gov.kh",
        window_days: 7,
        user_steps: "check the card, tick the declaration and press Submit",
        ref_sel: ".ref",
        ref_re: r"\bKH-EAC-\d{6,}\b",
        email_code: None,
        // step 5 "Confirm Declaration": the stepper's action button reads Submit only there (Next on steps 1–4)
        final_page: Some(mark("button.swot-primary", "Submit")),
        user_step_at: &[],
    },
    CardSpec {
        skill: "ph-etravel",
        name: "Philippines eTravel registration",
        country: "Philippines",
        mail_words: "(Philippines OR Manila OR MNL OR Cebu OR CEB OR Clark OR CRK)",
        start_url: "https://etravel.gov.ph/",
        host: "etravel.gov.ph",
        // 72 hours before arrival
        window_days: 3,
        user_steps: "sign in, fill in the registration and press Submit",
        ref_sel: ".ref",
        ref_re: r"\bETRV-PH-\d{6,}\b",
        email_code: None,
        final_page: None,
        user_step_at: &[mark("#password", "")],
    },
    // The ETA is an authorisation, not an arrival card: the application has no trip or accommodation section, so the
    // driver's trip facts (the flight, the stay) are optional here — the mail searches still find the trip for the
    // answer, and `forms::uk_eta` fills the same fields with empty facts (the passport, the home address and the
    // occupation come from the profile and the Vault). The face photo and the payment are the user's.
    CardSpec {
        skill: "uk-eta",
        name: "UK ETA application",
        country: "United Kingdom",
        mail_words: "(London OR LHR OR LGW OR STN OR Manchester OR MAN OR Edinburgh OR EDI OR \"United Kingdom\" OR UK)",
        // the web application (gov.uk/eta's "Start now"), mapped 2026-11-02
        start_url: "https://apply-for-an-eta.homeoffice.gov.uk/apply/electronic-travel-authorisation",
        host: "apply-for-an-eta.homeoffice.gov.uk",
        window_days: 0,
        user_steps: "take the passport and face photos, answer the questions, then press Submit and pay the ETA fee",
        ref_sel: ".ref",
        ref_re: r"\b\d{4}-\d{4}-\d{4}\b",
        // the email page ("Continue application" sends a security code), then the code page's Continue
        email_code: Some(super::email_code::Spec {
            email_sel: "#enter-email-address-value",
            send_text: "Continue application",
            code_sel: CODE_BOX,
            verify_text: "Continue",
            from: "gov.uk",
        }),
        final_page: None,
        // the email page and the security-code page (mapped 2026-11-02); the identity / photo pages after them are the
        // Home Office's confirm-your-identity service
        user_step_at: &[
            mark("#enter-email-address-value", ""),
            mark("#check-your-email-value", ""),
        ],
    },
];

impl CardSpec {
    /// The card asks for the trip (the flight, the stay): every card but the UK ETA, an authorisation with no trip
    /// section (`forms::uk_eta` fills the passport, the home address and the occupation).
    pub fn needs_trip(&self) -> bool {
        self.skill != "uk-eta"
    }

    /// The card requires the flight that leaves the country: Taiwan's itinerary step (its "Expected Mode of Departure"
    /// by air needs the departure flight's code and number, mapped from the live site 2026-11-02) — never invented, so
    /// asked before the form opens (`needs_input`) when the user did not give it.
    pub fn needs_departure_flight(&self) -> bool {
        self.skill == "tw-arrival-card"
    }

    /// The card requires the stay's phone number: Cambodia's travel step ("Hotel contact number", "Private residential
    /// contact number": required, walked on the live site 2026-11-02) — never invented, so asked before the form opens
    /// (`needs_input`) when the user did not give it; the user's own home takes their own phone.
    pub fn needs_stay_phone(&self) -> bool {
        self.skill == "kh-arrival-card"
    }
}

// ---------------------------------------------------------------- the workflow tool

/// The id prefix of the machine's code calls.
pub const PREFIX: &str = "drv_";
pub const ARRIVAL_CARD_FILL: &str = "arrival_card_fill";
/// The workflow tools this machine runs.
pub const TOOLS: [&str; 1] = [ARRIVAL_CARD_FILL];
/// The result that has the model call the tool again: the bookings were fetched and are in the transcript for it to
/// read (`workflow::CALL_AGAIN`).
pub const MAIL_READ: &str = "mail_read";

/// The card of a country (the table's name, any case).
pub fn card_of(country: &str) -> Option<&'static CardSpec> {
    let c = country.trim();
    CARDS.iter().find(|s| s.country.eq_ignore_ascii_case(c))
}

/// The catalog entry of the workflow tool.
pub fn tool_schemas() -> Vec<Value> {
    let date = |what: &str| json!({"type": "string", "description": format!("YYYY-MM-DD: {what} (resolve relative dates from today in the live state)")});
    let countries: Vec<&str> = CARDS.iter().map(|c| c.country).collect();
    vec![fn_(
        ARRIVAL_CARD_FILL,
        "An electronic arrival card or travel authorisation — Singapore SG Arrival Card, Malaysia MDAC, Korea \
e-Arrival Card, Indonesia All Indonesia, Taiwan TWAC, Thailand TDAC, China online arrival card, Japan Visit Japan Web, \
Cambodia e-Arrival Card, Philippines eTravel, UK ETA. By default (file false) code answers what to file, when, what it \
needs and the official link — nothing is opened. file true ONLY when the user explicitly asks you to fill it in or \
apply for them: then pass the country and every trip detail the user gave or a booking mail shows. When the trip is not given and a mailbox \
is connected, code fetches the bookings (status mail_read: read the flight and the stay in the mails just fetched and \
call again with every field). Code checks the filing window (a reminder when it is too early), fills the official form \
from these facts, the profile and the Vault, and hands the page to the user to review and submit. The JSON result has \
the reference (status submitted) or says what is left. status needs_input: ask the user for the missing fields, then \
call again with every field; needs_choice: the user picks on the card shown, then call again with own_address; \
invalid: fix the arguments and call again. Write the reply from the result, using its hints (code appends the facts in show to the reply).",
        json!({
            "country": {"type": "string", "enum": countries, "description": "the country whose card or authorisation to fill"},
            "file": {"type": "boolean", "description": "true when the user asks you to do it — fill in, do or apply for the card for them (even when they will review or submit it themselves); false for a question about what to file or how"},
            "flight_no": {"type": "string", "description": "the flight that ARRIVES in the country: airline code and number (\"SQ 231\")"},
            "arrival_date": date("the day that flight arrives"),
            "origin_city": {"type": "string", "description": "the city the flight comes from (\"Sydney\")"},
            "origin_country": {"type": "string", "description": "the country the flight comes from"},
            "arrival_airport": {"type": "string", "description": "the arrival airport's IATA code (\"SIN\")"},
            "airline": {"type": "string", "description": "the airline's name"},
            "stay_name": {"type": "string", "description": "the hotel or host the user stays with"},
            "stay_address": {"type": "string", "description": "the stay's address"},
            "stay_kind": {"type": "string", "enum": ["hotel", "own", "friends", "other"]},
            "own_address": {"type": "string", "description": "the label of the user's own address they chose on the choice card (\"I choose: …\")"},
            "purpose": {"type": "string", "enum": ["holiday", "business", "family", "study", "transit"]},
            "departure_date": date("the day the user leaves the country, when a return or onward flight or the user gives it"),
            "next_city": {"type": "string", "description": "the next city after the country, when an onward flight or the user gives it"},
            "departure_flight_no": {"type": "string", "description": "the flight that LEAVES the country (the return or onward flight), when the user or a booking gives it: airline code and number (\"CI 51\")"},
            "stay_phone": {"type": "string", "description": "the stay's phone number (the hotel's, or the host's), when the user or a booking gives it, with its country code (\"+855 23 000 000\")"},
            "occupation": {"type": "string", "description": "the user's job, only when they gave it — optional: a card that asks for it leaves the box for the user's review, so it is never asked for (needs_input lists everything that is)"}
        }),
        &["country"],
    )]
}

/// `ask_user` in an arrival card's run before `arrival_card_fill` was called: refused, as data — what only the user
/// can give is the tool's `needs_input`, and the defaults are code's. `events` are the task's.
pub fn ask_before_tool(events: &[Value]) -> Option<Value> {
    let skill = events
        .iter()
        .filter(|e| get_str(e, "type") == "tool_call")
        .filter(|e| get_str(&e["data"], "name") == "load_skill")
        .map(|e| get_str(&e["data"]["args"], "name"))
        .filter_map(|n| CARDS.iter().find(|c| c.skill == n))
        .next_back()?;
    let called = events.iter().any(|e| {
        get_str(e, "type") == "workflow_call" && get_str(&e["data"], "tool") == ARRIVAL_CARD_FILL
    });
    (!called).then(|| {
        json!({"refused": "call_the_tool_first", "tool": ARRIVAL_CARD_FILL,
               "why": format!("the {} skill asks through {ARRIVAL_CARD_FILL}: call it with the country and what the user gave; its needs_input lists what only the user can give, and code fills the defaults", skill.skill)})
    })
}

// ---------------------------------------------------------------- validation (code, on the JSON)

/// What validation reads besides the arguments.
pub struct Ctx<'a> {
    pub profile: &'a Value,
    pub today: NaiveDate,
    /// a mailbox is connected
    pub mail_ready: bool,
    /// the chat's transcript: the earlier calls' arguments and whether the bookings were already fetched (structured
    /// results only)
    pub transcript: &'a [Value],
}

/// The trip facts of the tool's arguments (the keys `forms::Facts` reads, and the user's choice).
const FACT_KEYS: &[&str] = &[
    "flight_no",
    "arrival_date",
    "origin_city",
    "origin_country",
    "arrival_airport",
    "airline",
    "stay_name",
    "stay_address",
    "stay_kind",
    "own_address",
    "purpose",
    "departure_date",
    "next_city",
    "occupation",
    "departure_flight_no",
    "stay_phone",
];

static FLIGHT_NO: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^([A-Z0-9]{2})\s*-?\s*(\d{1,4})[A-Z]?$").unwrap());
static IATA: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[A-Z]{3}$").unwrap());

fn s_arg(a: &Value, k: &str) -> String {
    match a.get(k) {
        Some(Value::String(s)) => s.trim().to_string(),
        Some(Value::Number(n)) => n.to_string(),
        _ => String::new(),
    }
}

/// "sq231" / "SQ-231" → "SQ 231"; `None` for anything that is not an airline code and a number (an all-digit code
/// is no airline).
pub fn norm_flight(v: &str) -> Option<String> {
    let up = v.trim().to_uppercase();
    let c = FLIGHT_NO.captures(&up)?;
    let code = &c[1];
    (code.chars().any(|ch| ch.is_ascii_alphabetic())).then(|| format!("{code} {}", &c[2]))
}

fn date_of(v: &str) -> Option<NaiveDate> {
    NaiveDate::parse_from_str(v.trim(), "%Y-%m-%d").ok()
}

fn iso(d: NaiveDate) -> String {
    d.format("%Y-%m-%d").to_string()
}

fn long(d: NaiveDate) -> String {
    d.format("%-d %B %Y").to_string()
}

/// The arguments of the earlier calls for the same country in this chat, merged (a later value wins), under `args`: a
/// call again after `mail_read`, `needs_input` or the choice card need not repeat every field.
fn with_earlier(args: &Value, country: &str, transcript: &[Value]) -> Value {
    let mut merged = Map::new();
    for tc in transcript.iter().flat_map(tool_calls) {
        let (name, a) = stored_call(tc);
        if name != ARRIVAL_CARD_FILL || !s_arg(&a, "country").eq_ignore_ascii_case(country) {
            continue;
        }
        for k in FACT_KEYS {
            let v = s_arg(&a, k);
            if !v.is_empty() {
                merged.insert((*k).into(), json!(v));
            }
        }
    }
    for k in FACT_KEYS {
        let v = s_arg(args, k);
        if !v.is_empty() {
            merged.insert((*k).into(), json!(v));
        }
    }
    Value::Object(merged)
}

/// Whether this chat's bookings were already fetched by the machine (a `mail_read` result).
fn mail_fetched(transcript: &[Value]) -> bool {
    let want = format!("{ARRIVAL_CARD_FILL}{}", workflow::RESULT_SUFFIX);
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

/// Validates the tool's arguments: the arguments to run with (normalised, the defaults filled), the problems the model
/// can fix, or the fields only the user can give — before any page opens.
pub fn check(tool: &str, args: &Value, cx: &Ctx) -> Checked {
    debug_assert_eq!(tool, ARRIVAL_CARD_FILL);
    let Some(card) = card_of(&s_arg(args, "country")) else {
        let countries: Vec<&str> = CARDS.iter().map(|c| c.country).collect();
        return Checked::Invalid(vec![format!("country: one of {}", countries.join(", "))]);
    };
    // the information path (the user asked what to file, not to file it): the country and, when given, the arrival
    // day — no trip detail is asked for, no page opened
    if !truthy(args.get("file").unwrap_or(&Value::Null)) {
        let mut out = json!({"country": card.country, "file": false});
        if let Ok(d) = NaiveDate::parse_from_str(&s_arg(args, "arrival_date"), "%Y-%m-%d") {
            out["arrival_date"] = json!(d.format("%Y-%m-%d").to_string());
        }
        return Checked::Valid(out);
    }
    let a = with_earlier(args, card.country, cx.transcript);
    let mut problems: Vec<String> = Vec::new();
    let mut out = Map::new();
    out.insert("country".into(), json!(card.country));
    out.insert("file".into(), json!(true));
    let mut put = |k: &str, v: String| {
        if !v.is_empty() {
            out.insert(k.into(), json!(v));
        }
    };
    let flight = s_arg(&a, "flight_no");
    let flight = if flight.is_empty() {
        String::new()
    } else {
        match norm_flight(&flight) {
            Some(f) => f,
            None => {
                problems.push(format!(
                    "flight_no: {flight:?} is not an airline code and a number (\"SQ 231\")"
                ));
                String::new()
            }
        }
    };
    put("flight_no", flight.clone());
    let leave = s_arg(&a, "departure_flight_no");
    let leave = if leave.is_empty() {
        String::new()
    } else {
        match norm_flight(&leave) {
            Some(f) => f,
            None => {
                problems.push(format!(
                    "departure_flight_no: {leave:?} is not an airline code and a number (\"CI 51\")"
                ));
                String::new()
            }
        }
    };
    put("departure_flight_no", leave.clone());
    let arrival = match s_arg(&a, "arrival_date") {
        v if v.is_empty() => None,
        v => match date_of(&v) {
            Some(d) if d < cx.today => {
                problems.push(format!("arrival_date: {v} is in the past"));
                None
            }
            Some(d) => Some(d),
            None => {
                problems.push(format!("arrival_date: {v:?} is not a YYYY-MM-DD date"));
                None
            }
        },
    };
    put("arrival_date", arrival.map(iso).unwrap_or_default());
    // the departure: a date after the arrival, else the default (arrival + 7 days, `forms::Inputs::departure`)
    let departure = date_of(&s_arg(&a, "departure_date"))
        .filter(|d| arrival.is_none_or(|ar| *d >= ar))
        .map(iso)
        .unwrap_or_default();
    put("departure_date", departure);
    let origin_city = s_arg(&a, "origin_city");
    let origin_country = match s_arg(&a, "origin_country") {
        c if c.is_empty() => country_of_city(&origin_city).to_string(),
        c => c,
    };
    put("origin_city", origin_city);
    put("origin_country", origin_country);
    let airport = s_arg(&a, "arrival_airport").to_uppercase();
    put(
        "arrival_airport",
        if IATA.is_match(&airport) {
            airport
        } else {
            String::new()
        },
    );
    for k in ["airline", "next_city", "occupation"] {
        put(k, s_arg(&a, k));
    }
    // the stay's phone: digits with an optional +, spaces, dashes, dots or brackets, at least six digits
    let stay_phone = s_arg(&a, "stay_phone");
    let stay_phone = if stay_phone.is_empty() {
        String::new()
    } else if stay_phone
        .chars()
        .all(|c| c.is_ascii_digit() || " +-().".contains(c))
        && stay_phone.chars().filter(char::is_ascii_digit).count() >= 6
    {
        stay_phone
    } else {
        problems.push(format!(
            "stay_phone: {stay_phone:?} is not a phone number (\"+855 23 000 000\")"
        ));
        String::new()
    };
    put("stay_phone", stay_phone.clone());
    // the stay: what the arguments say, else the user's own address they chose, else the profile's one address there
    let own = own_addresses(cx.profile, card.country);
    let mut stay_name = s_arg(&a, "stay_name");
    let mut stay_address = s_arg(&a, "stay_address");
    let mut stay_kind = s_arg(&a, "stay_kind").to_lowercase();
    let label = s_arg(args, "own_address");
    let label = if label.is_empty() && stay_name.is_empty() && stay_address.is_empty() {
        s_arg(&a, "own_address")
    } else {
        label
    };
    if !label.is_empty() {
        let l = label.to_lowercase();
        match own
            .iter()
            .find(|(k, v)| k.to_lowercase() == l || v.to_lowercase() == l)
        {
            Some((_, addr)) => {
                stay_name = String::new();
                stay_address = addr.clone();
                stay_kind = "own".into();
            }
            None if own.is_empty() => problems.push(format!(
                "own_address: the profile has no address in {}",
                card.country
            )),
            None => problems.push(format!(
                "own_address: one of {}",
                own.iter()
                    .map(|(l, _)| format!("{l:?}"))
                    .collect::<Vec<_>>()
                    .join(", ")
            )),
        }
    }
    if !problems.is_empty() {
        return Checked::Invalid(problems);
    }
    let stay_known = !stay_name.is_empty() || !stay_address.is_empty();
    if stay_known && !["hotel", "own", "friends", "other"].contains(&stay_kind.as_str()) {
        stay_kind = if stay_name.is_empty() {
            "other"
        } else {
            "hotel"
        }
        .into();
    }
    let purpose = s_arg(&a, "purpose").to_lowercase();
    if !card.needs_trip() {
        put("purpose", purpose);
        return Checked::Valid(Value::Object(out));
    }
    let flight_known = !flight.is_empty() && arrival.is_some();
    // the trip not known yet and a mailbox connected: the bookings first (the model reads them and calls again)
    if (!flight_known || !stay_known) && cx.mail_ready && !mail_fetched(cx.transcript) {
        out.insert("read_mail".into(), json!(true));
        return Checked::Valid(Value::Object(out));
    }
    let mut choose = false;
    if !stay_known {
        match own.len() {
            0 => {}
            1 => {
                stay_address = own[0].1.clone();
                stay_kind = "own".into();
            }
            _ => choose = true,
        }
    }
    let mut missing: Vec<String> = Vec::new();
    if flight.is_empty() {
        missing.push(format!(
            "flight_no (the flight arriving in {})",
            card.country
        ));
    }
    if arrival.is_none() {
        missing.push("arrival_date".into());
    }
    if stay_address.is_empty() && stay_name.is_empty() && !choose {
        missing.push(format!(
            "stay (the hotel's name, or the address in {})",
            card.country
        ));
    }
    if stay_phone.is_empty() && card.needs_stay_phone() && stay_kind != "own" {
        missing.push(format!(
            "stay_phone (the phone number of the {} you stay at in {}: the card requires it)",
            if stay_kind == "hotel" {
                "hotel"
            } else {
                "place"
            },
            card.country
        ));
    }
    if leave.is_empty() && card.needs_departure_flight() {
        missing.push(format!(
            "departure_flight_no (the flight leaving {} and its date: the card requires it)",
            card.country
        ));
    }
    if !missing.is_empty() {
        if purpose.is_empty() {
            missing.push("purpose (holiday, business, family, study or transit)".into());
        }
        return Checked::NeedsInput(missing);
    }
    out.insert("stay_name".into(), json!(stay_name));
    out.insert("stay_address".into(), json!(stay_address));
    out.insert("stay_kind".into(), json!(stay_kind));
    if purpose.is_empty() {
        out.insert("purpose".into(), json!("holiday"));
        out.insert("purpose_assumed".into(), json!(true));
    } else {
        out.insert("purpose".into(), json!(purpose));
    }
    if choose {
        out.insert("choose_address".into(), json!(true));
    }
    Checked::Valid(Value::Object(out))
}

// ---------------------------------------------------------------- the machine

/// The trip facts the form function gets (`crate::forms::Facts` keys).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Trip {
    pub flight_no: String,
    pub arrival_date: String,
    pub origin_city: String,
    pub origin_country: String,
    pub arrival_airport: String,
    pub airline: String,
    pub stay_name: String,
    pub stay_address: String,
    pub stay_kind: String,
    pub purpose: String,
    pub departure_date: String,
    pub next_city: String,
    pub occupation: String,
    pub departure_flight_no: String,
    pub stay_phone: String,
}

impl Trip {
    /// The trip of the validated arguments.
    pub fn from_args(a: &Value) -> Trip {
        let g = |k: &str| s_arg(a, k);
        Trip {
            flight_no: g("flight_no"),
            arrival_date: g("arrival_date"),
            origin_city: g("origin_city"),
            origin_country: g("origin_country"),
            arrival_airport: g("arrival_airport"),
            airline: g("airline"),
            stay_name: g("stay_name"),
            stay_address: g("stay_address"),
            stay_kind: g("stay_kind"),
            purpose: g("purpose"),
            departure_date: g("departure_date"),
            next_city: g("next_city"),
            occupation: g("occupation"),
            departure_flight_no: g("departure_flight_no"),
            stay_phone: g("stay_phone"),
        }
    }

    /// The `form_fill` arguments.
    pub fn args(&self) -> Value {
        json!({
            "flight_no": self.flight_no, "arrival_date": self.arrival_date, "origin_city": self.origin_city,
            "origin_country": self.origin_country, "arrival_airport": self.arrival_airport, "airline": self.airline,
            "stay_name": self.stay_name, "stay_address": self.stay_address, "stay_kind": self.stay_kind,
            "purpose": self.purpose, "departure_date": self.departure_date, "next_city": self.next_city,
            "occupation": self.occupation, "departure_flight_no": self.departure_flight_no,
            "stay_phone": self.stay_phone,
        })
    }

    fn arrival(&self) -> Option<NaiveDate> {
        date_of(&self.arrival_date)
    }

    /// The stay as the result names it.
    fn stay(&self) -> String {
        if !self.stay_name.is_empty() {
            self.stay_name.clone()
        } else {
            self.stay_address.clone()
        }
    }

    /// The trip as the result carries it.
    fn json(&self) -> Value {
        json!({"flight_no": self.flight_no, "arrival_date": self.arrival_date, "origin_city": self.origin_city,
               "stay": self.stay(), "stay_kind": self.stay_kind, "purpose": self.purpose})
    }
}

/// What the machine reads besides its arguments and the transcript.
pub struct Machine<'a> {
    pub profile: &'a Value,
    pub mail_ready: bool,
    pub today: NaiveDate,
    /// the machine's code calls in the transcript before this run's tool call: the ids go on from there
    pub first: usize,
}

/// The defaults the form function applied that the user should check (the takeover reason and the result).
fn assumptions(trip: &Trip, purpose_assumed: bool) -> Vec<String> {
    let mut parts = vec!["every health and declaration question No".to_string()];
    if trip.departure_date.is_empty() {
        if let Some(a) = trip.arrival() {
            parts.push(format!(
                "departure {} (arrival + 7 days)",
                long(a + chrono::Duration::days(7))
            ));
        }
    }
    if trip.next_city.is_empty() && !trip.origin_city.is_empty() {
        parts.push(format!("next city {}", trip.origin_city));
    }
    if purpose_assumed {
        parts.push("purpose holiday".into());
    }
    parts
}

const WHICH_MARK: &str = "Which of these addresses";

/// The next step of a run: the validated arguments, the transcript after the tool call.
/// What to file for a country and how (the user asked about it, not to file it — the user, 2026-11-04: requirements
/// and the official link; filling in is for when they ask): the card, the official site, the filing window as dates
/// when the arrival day is known, and what the form asks for. Nothing is opened.
pub fn card_info(card: &CardSpec, arrival: &str) -> Value {
    let arrival = NaiveDate::parse_from_str(arrival, "%Y-%m-%d").ok();
    let when = match (card.window_days, arrival) {
        (0, _) => "any time before you arrive".to_string(),
        (w, Some(a)) => format!(
            "from {} to your arrival on {} (it opens {w} days before arrival)",
            (a - chrono::Duration::days(w)).format("%Y-%m-%d"),
            a.format("%Y-%m-%d")
        ),
        (w, None) => format!("within {w} days before you arrive"),
    };
    let show = vec![
        S::fact("What to file", "要填的", &format!("{} ({})", card.name, card.country)),
        S::fact("When", "何时", &when),
        S::fact(
            "You need",
            "需要",
            "your passport, the flight that arrives and its date, where you stay (the hotel's name and address) and \
             the day you leave",
        ),
        S::link("Official site", "官方网站", card.name, card.start_url),
        S::fact(
            "Filling it in",
            "代填",
            "it is free on the official site; ask me to fill it in and I prepare it there for you to check and submit",
        ),
    ];
    json!({"status": "info", "card": card.name, "country": card.country, "official_site": card.start_url,
           "when": when, "show": show,
           "hints": ["the user asked what to file: the requirements and the official link — nothing was filled in"]})
}

pub fn next(tool: &str, args: &Value, t: &[Value], m: &Machine) -> Step {
    debug_assert_eq!(tool, ARRIVAL_CARD_FILL);
    let Some(card) = card_of(&s_arg(args, "country")) else {
        return Step::Done(
            json!({"status": "error", "why": "unknown country", "hints": ["no arrival card for that country"]}),
        );
    };
    if !truthy(args.get("file").unwrap_or(&Value::Null)) {
        return Step::Done(card_info(card, &s_arg(args, "arrival_date")));
    }
    let n = m.first
        + t.iter()
            .flat_map(tool_calls)
            .filter(|tc| get_str(tc, "id").starts_with(PREFIX))
            .count()
        + 1;
    let call = |k: usize, name: &str, a: Value| workflow::code_call(PREFIX, n + k, name, a);
    let trip = Trip::from_args(args);
    // 1. the bookings: the two fixed searches, the first hits of each; then the model reads them and calls again
    if truthy(args.get("read_mail").unwrap_or(&Value::Null)) && m.mail_ready {
        let searches = results_of(t, "gmail_search");
        if searches.is_empty() {
            let q = |what: &str| json!({"query": format!("{what} {} newer_than:60d", card.mail_words), "max_results": 10});
            return Step::Calls(vec![
                call(0, "gmail_search", q("(flight OR e-ticket OR itinerary)")),
                call(1, "gmail_search", q("(hotel OR booking OR reservation)")),
            ]);
        }
        let fetched = results_of(t, "gmail_get_message");
        let fetched_ids: Vec<String> = fetched.iter().map(|(a, _)| sget(a, "message_id")).collect();
        let mut want: Vec<String> = Vec::new();
        for (_, msg) in &searches {
            let mut hits = Vec::new();
            walk_messages(&json_of(msg), &mut hits);
            for h in hits.iter().take(2) {
                let id = get_str(h, "id");
                if !id.is_empty() && !fetched_ids.contains(&id) && !want.contains(&id) {
                    want.push(id);
                }
            }
        }
        if !want.is_empty() {
            return Step::Calls(
                want.iter()
                    .enumerate()
                    .map(|(i, id)| call(i, "gmail_get_message", json!({"message_id": id})))
                    .collect(),
            );
        }
        // what the mails may give: the stay only when the profile has no address in the country (code uses the
        // user's own address then — arrival_address_real 2026-11-01: "stay" listed here, the model asked the user for
        // a hotel instead of calling again)
        let own = own_addresses(m.profile, card.country);
        let missing: Vec<&str> = [
            (trip.flight_no.is_empty(), "flight_no"),
            (trip.arrival_date.is_empty(), "arrival_date"),
            (
                trip.stay_name.is_empty() && trip.stay_address.is_empty() && own.is_empty(),
                "stay",
            ),
        ]
        .into_iter()
        .filter(|(m, _)| *m)
        .map(|(_, k)| k)
        .collect();
        return Step::Done(
            json!({"status": MAIL_READ, "call_again": ARRIVAL_CARD_FILL, "card": card.name,
            "country": card.country, "mails": fetched_ids, "facts_from_mail": missing,
            "own_address_on_profile": !own.is_empty(), "hints": []}),
        );
    }
    // 2. several of the user's own addresses in the country: the choice card, the user picks
    if truthy(args.get("choose_address").unwrap_or(&Value::Null)) {
        let own = own_addresses(m.profile, card.country);
        let question = format!(
            "{WHICH_MARK} in {} are you staying at this time?",
            card.country
        );
        if results_of(t, "present_choices").is_empty() {
            let options: Vec<Value> = own
                .iter()
                .map(|(l, a)| json!({"label": l, "details": [a]}))
                .collect();
            return Step::Calls(vec![call(
                0,
                "present_choices",
                json!({"question": question, "kind": "clarify", "options": options}),
            )]);
        }
        return Step::Done(
            json!({"status": "needs_choice", "card": card.name, "question": question,
            "options": own.iter().map(|(l, _)| l.clone()).collect::<Vec<_>>(), "hints": []}),
        );
    }
    // 3. the filing window: too early → a reminder for the morning it opens
    if let (Some(arr), true) = (trip.arrival(), card.window_days > 0) {
        let opens = arr - chrono::Duration::days(card.window_days);
        if m.today < opens {
            if results_of(t, "schedule_create").is_empty() {
                return Step::Calls(vec![call(
                    0,
                    "schedule_create",
                    json!({"name": format!("{}: {}", card.name, trip.flight_no),
                           "goal": format!("Fill in my {} for {} arriving on {}", card.name, trip.flight_no, trip.arrival_date),
                           "kind": "cron", "spec": format!("0 8 {} {} *", opens.day(), opens.month())}),
                )]);
            }
            return Step::Done(
                json!({"status": "scheduled", "card": card.name, "country": card.country,
                "window_days": card.window_days, "opens": iso(opens), "trip": trip.json(),
                "show": [S::fact(&format!("{} can be filed from", card.name), &format!("{} 可提交日期", card.name),
                                 &format!("{} (within the {} days before arrival)", long(opens), card.window_days)),
                         S::fact("Reminder", "提醒", &format!("the morning of {}", long(opens)))],
                "hints": ["nothing is filed yet"]}),
            );
        }
    }
    // 4. the official page
    let form_open = results_of(t, "browser_navigate")
        .iter()
        .any(|(a, _)| sget(a, "url").contains(card.host));
    if !form_open {
        return Step::Calls(vec![call(
            0,
            "browser_navigate",
            json!({"url": card.start_url}),
        )]);
    }
    // 5. the fill, the user's own steps, the review hand-over, the confirmation
    let fills = results_of(t, "form_fill");
    let takeovers = results_of(t, "browser_request_takeover");
    let last_fill = fills.last().map(|(_, msg)| pos(t, msg));
    let last_back = takeovers
        .iter()
        .filter(|(_, msg)| handed_back(msg))
        .map(|(_, msg)| pos(t, msg))
        .max();
    let last_take = takeovers.last().map(|(_, msg)| pos(t, msg));
    let fill_after_back = matches!((last_fill, last_back), (Some(f), Some(b)) if f > b)
        || (last_fill.is_some() && last_back.is_none());
    if last_fill.is_none()
        || (!fill_after_back && fills.last().is_some_and(|(_, msg)| stopped(msg).is_some()))
    {
        // the first fill, or the fill again after the user's own step
        return Step::Calls(vec![call(0, "form_fill", trip.args())]);
    }
    let fill = fills.last().map(|(_, msg)| *msg).unwrap_or(&Value::Null);
    let fill_pos = last_fill.unwrap_or(0);
    let take_after_fill = last_take.is_some_and(|tk| tk > fill_pos);
    // the site's email verification step (agent/email_code.rs), once per run: after the fill that stopped at it, code
    // types the email (and with a mailbox, sends the code, reads it from the mail and types it); a typed code → the fill
    // goes on, anything else → the hand-over below
    let codes = results_of(t, super::email_code::EMAIL_CODE);
    let code_status = codes
        .last()
        .filter(|(_, msg)| pos(t, msg) > fill_pos)
        .map(|(_, msg)| get_str(&json_of(msg), "status"));
    if let Some(spec) = card.email_code.as_ref() {
        if codes.is_empty() && !take_after_fill && stopped(fill).is_some() {
            return Step::Calls(vec![call(
                0,
                super::email_code::EMAIL_CODE,
                spec.args(&user_email(m.profile)),
            )]);
        }
        if code_status.as_deref() == Some("typed") {
            return Step::Calls(vec![call(0, "form_fill", trip.args())]);
        }
    }
    let assumed = assumptions(
        &trip,
        truthy(args.get("purpose_assumed").unwrap_or(&Value::Null)),
    );
    let user_step = |why: &str| {
        let step = why.split(" is the user's").next().unwrap_or(why).trim();
        step.strip_prefix("the ").unwrap_or(step).to_string()
    };
    // the same user's step handed over twice and still not done: the run ends with where it stands (never a loop)
    let user_steps_back = takeovers
        .iter()
        .filter(|(a, msg)| sget(a, "reason").starts_with("This step is yours") && handed_back(msg))
        .count();
    if !take_after_fill && user_steps_back >= 2 {
        if let Some(why) = stopped(fill) {
            let step = user_step(&why);
            if let Some(r) = code_not_read(card, &codes) {
                return Step::Done(r);
            }
            return Step::Done(
                json!({"status": "waiting_on_user", "card": card.name, "step": step,
                "trip": trip.json(),
                "show": [S::fact("Waiting on your step", "等你完成的步骤", &step)],
                "hints": ["finish it in the browser tab, then ask to continue", "nothing has been submitted"]}),
            );
        }
    }
    // the page check before the hand-over: the card's declared elements, read by their fixed selectors — the final
    // page's Submit, the markers of the user's own steps (never a click: the Submit is the user's)
    let page = page_check(t, fill_pos);
    let declared = card.final_page.is_some() || !card.user_step_at.is_empty();
    if !take_after_fill && page.is_none() && declared {
        return Step::Calls(vec![call(
            0,
            "browser_read_elements",
            page_check_args(card),
        )]);
    }
    let page = page.map(|m| PageSeen::of(card, m)).unwrap_or_default();
    // a filled form handed over before its final page: the fill stopped early (the result and the hand-over say so)
    let early = stopped(fill).is_none() && card.final_page.is_some() && !page.on_final;
    if !take_after_fill {
        let reason = match stopped(fill) {
            Some(why) => format!(
                "This step is yours: {}.{} Do it on the page, then hand the browser back and I will fill in the rest of the form.",
                user_step(&why),
                match code_status.as_deref() {
                    Some("no_mailbox") | Some("no_send_button") => " Your email is filled in: press the button that sends the code and type the code from the email.",
                    Some("timeout") | Some("no_code") | Some("code_not_typed") => " Your email is filled in and the code was requested: type the code from the email.",
                    _ => "",
                }
            ),
            None if early => format!(
                "I filled in what I could, but the {} stopped before its last page ({}). Fill in what is still missing on this page and go on to the end, then {}; nothing has been submitted. Hand the browser back when the confirmation shows.",
                card.name,
                if page.url.is_empty() { "the page in the browser tab".to_string() } else { page.url.clone() },
                card.user_steps
            ),
            None => {
                let left: Vec<String> = json_of(fill)
                    .get("skipped")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .map(|s| get_str(s, "field").replace('_', " "))
                    .filter(|f| !f.is_empty())
                    .collect();
                format!(
                    "The {} is filled in from your trip details, profile and passport (Vault). I assumed {} — change any that are not right. Review it{}, then {}; hand the browser back when the confirmation shows.",
                    card.name,
                    assumed.join(", "),
                    if left.is_empty() { String::new() } else { format!(" and fill in {}", left.join(", ")) },
                    card.user_steps
                )
            }
        };
        return Step::Calls(vec![call(
            0,
            "browser_request_takeover",
            json!({"reason": reason}),
        )]);
    }
    let back_after_fill = last_back.is_some_and(|b| b > fill_pos);
    if !back_after_fill {
        // the page is still the user's (the takeover ended without the hand-back)
        if early {
            return Step::Done(stopped_early(card, &trip, &page));
        }
        if stopped(fill).is_some() {
            if let Some(r) = code_not_read(card, &codes) {
                return Step::Done(r);
            }
        }
        return Step::Done(
            json!({"status": "handed_over", "card": card.name, "trip": trip.json(),
            "user_steps": card.user_steps, "on_final_page": page.on_final, "at_user_step": page.at_step,
            "show": [S::fact("Status", "状态", &format!("the {} is filled in and waiting in the browser tab; nothing has been submitted yet", card.name)),
                     S::fact("Your step", "你的步骤", card.user_steps)],
            "hints": []}),
        );
    }
    // the confirmation's reference: the card's fixed element on the page, its value in the card's format
    let reads = results_of(t, "browser_read_elements");
    let read_after_back = reads
        .last()
        .filter(|(_, msg)| last_back.is_some_and(|b| pos(t, msg) > b));
    let Some((_, read)) = read_after_back else {
        return Step::Calls(vec![call(
            0,
            "browser_read_elements",
            super::confirm::read_args(card.ref_sel),
        )]);
    };
    let reference = crate::outcome::reference_in(
        &super::confirm::element_values(read, super::confirm::REFERENCE),
        card.ref_re,
    );
    Step::Done(match reference {
        Some(r) => {
            json!({"status": "submitted", "card": card.name, "country": card.country, "reference": r,
            "trip": trip.json(), "assumed": assumed,
            "identity": "name, birthday, nationality and passport from the profile and the Vault (the passport number is not shown)",
            "show": [S::reference(&format!("{} confirmation number", card.name), &format!("{} 确认号", card.name), &r)],
            "hints": [format!("{} submitted", card.name)]})
        }
        None if early => stopped_early(card, &trip, &page),
        None => {
            json!({"status": "not_confirmed", "card": card.name, "country": card.country, "trip": trip.json(),
            "on_final_page": page.on_final, "at_user_step": page.at_step,
            "show": [S::fact("Not confirmed", "未确认", &format!("no reference showed after the hand-back: check the browser tab whether the {} was submitted", card.name))],
            "hints": []})
        }
    })
}

/// The name prefix of the page check's reads (`page_check_args`).
const PAGE_FINAL: &str = "page_final";
const PAGE_STEP: &str = "page_step";

/// The `browser_read_elements` arguments of the page check: the final page's Submit (visible), each user step's
/// marker (hidden ones too where the card says so).
pub fn page_check_args(card: &CardSpec) -> Value {
    let mut read: Vec<Value> = Vec::new();
    if let Some(m) = card.final_page {
        read.push(json!({"name": PAGE_FINAL, "sel": m.sel, "max": 5}));
    }
    for (i, m) in card.user_step_at.iter().enumerate() {
        read.push(
            json!({"name": format!("{PAGE_STEP}{i}"), "sel": m.sel, "hidden": m.hidden, "max": 5}),
        );
    }
    json!({"read": read})
}

/// The page check after the last fill (`at` = its position), when it was read.
fn page_check(t: &[Value], at: usize) -> Option<&Value> {
    results_of(t, "browser_read_elements")
        .into_iter()
        .filter(|(a, msg)| {
            pos(t, msg) > at
                && a["read"]
                    .as_array()
                    .is_some_and(|r| r.iter().any(|x| sget(x, "name").starts_with("page_")))
        })
        .map(|(_, msg)| msg)
        .next_back()
}

/// What the page check found.
#[derive(Debug, Clone, Default, PartialEq)]
struct PageSeen {
    url: String,
    /// the final page's Submit is on the page (with its declared text)
    on_final: bool,
    /// a user's step's marker is on the page
    at_step: bool,
}

impl PageSeen {
    fn of(card: &CardSpec, m: &Value) -> Self {
        let present = |name: &str, text: &str| {
            let vals = super::confirm::element_values(m, name);
            !vals.is_empty()
                && (text.is_empty() || vals.iter().any(|v| v.trim().eq_ignore_ascii_case(text)))
        };
        PageSeen {
            url: get_str(&json_of(m), "url"),
            on_final: card.final_page.is_some_and(|f| present(PAGE_FINAL, f.text)),
            at_step: card
                .user_step_at
                .iter()
                .enumerate()
                .any(|(i, s)| present(&format!("{PAGE_STEP}{i}"), s.text)),
        }
    }
}

/// The result of a run handed over at the email step because code could not read the verification code (no mailbox,
/// no mail before the timeout, no code in it): it says so, and why — the user enters the code on the page. `None` when
/// no `email_code` step ran or it typed the code.
fn code_not_read(card: &CardSpec, codes: &[(Value, &Value)]) -> Option<Value> {
    let (_, msg) = codes.last()?;
    let status = get_str(&json_of(msg), "status");
    if status == "typed" {
        return None;
    }
    let from = card.email_code.map(|e| e.from).unwrap_or("");
    let why = match status.as_str() {
        "no_mailbox" => "no mailbox is connected".to_string(),
        "timeout" => format!("no mail from {from} came in time"),
        "no_code" => "the mail held no code I could read".to_string(),
        "code_not_typed" => "the code box did not take the code".to_string(),
        "no_send_button" => "the page had no button that sends the code".to_string(),
        "no_email_field" | "no_email" => "the page had no email box".to_string(),
        "cancelled" => "the task was cancelled".to_string(),
        other => other.replace('_', " "),
    };
    Some(
        json!({"status": EMAIL_CODE_NOT_READ, "card": card.name, "country": card.country, "reason": status,
    "show": [S::fact("Email code", "邮件验证码", &format!(
        "I could not read the verification code ({why}) — enter the code from the email on the page; nothing has been submitted"))],
    "hints": ["say the code could not be read and that the user enters it on the page"]}),
    )
}

/// The status of a run handed over at the email step because the code could not be read.
pub const EMAIL_CODE_NOT_READ: &str = "email_code_not_read";

/// The result of a fill that stopped before the form's final page: where it stopped; nothing was submitted.
fn stopped_early(card: &CardSpec, trip: &Trip, page: &PageSeen) -> Value {
    json!({"status": "stopped_early", "card": card.name, "country": card.country, "trip": trip.json(),
    "page": page.url, "on_final_page": false,
    "show": [S::fact("Stopped early", "未填到最后一页", &format!(
        "the {} was filled only up to {} — not its last page; finish the rest in the browser tab. Nothing has been submitted",
        card.name, if page.url.is_empty() { "the page in the browser tab" } else { page.url.as_str() }))],
    "hints": ["say which page the form stopped on and that nothing was submitted"]})
}

// ---------------------------------------------------------------- shared helpers (the transcript, read back)

/// The user's own messages of the transcript, without the system notes appended to them.
pub(crate) fn user_words(transcript: &[Value]) -> Vec<String> {
    transcript
        .iter()
        .filter(|m| get_str(m, "role") == "user")
        .map(|m| {
            let c = content_of(m);
            match c.find("\n\n(System)").or_else(|| c.find("\n\n（系统）")) {
                Some(i) => c[..i].to_string(),
                None => c,
            }
        })
        .filter(|c| !c.starts_with("(System)") && !c.starts_with("（系统）"))
        .collect()
}

/// What a code machine decides for the next turn.
#[derive(Debug, Clone, PartialEq)]
pub enum Step {
    /// run these calls (one turn)
    Calls(Vec<ToolCall>),
    /// the machine is done: its JSON result goes back as the workflow tool's `T_result` code exchange and the model
    /// writes the reply from it (`agent/workflow.rs`, docs/json-tools.md)
    Done(Value),
}

/// The email a site's verification step gets: the profile's personal email, else its email, else the connected
/// mailbox's address (`Runtime::machine_profile` puts it in as `mailbox`).
fn user_email(profile: &Value) -> String {
    ["email_personal", "email", "mailbox"]
        .iter()
        .map(|k| get_str(profile, k).trim().to_string())
        .find(|e| !e.is_empty())
        .unwrap_or_default()
}

/// The country of a city in the table ("Sydney" → "Australia"); "" when it is not there.
pub(crate) fn country_of_city(city: &str) -> &'static str {
    match city.to_lowercase().as_str() {
        "sydney" | "melbourne" | "brisbane" | "perth" | "adelaide" => "Australia",
        "singapore" => "Singapore",
        "kuala lumpur" | "penang" => "Malaysia",
        "bangkok" | "phuket" | "chiang mai" => "Thailand",
        "tokyo" | "osaka" | "nagoya" | "fukuoka" => "Japan",
        "seoul" | "busan" | "incheon" => "Korea",
        "hong kong" => "Hong Kong",
        "taipei" | "kaohsiung" => "Taiwan",
        "london" | "manchester" | "edinburgh" => "United Kingdom",
        "new york" | "los angeles" | "san francisco" | "chicago" => "United States",
        "auckland" | "wellington" => "New Zealand",
        "jakarta" | "denpasar" | "bali" => "Indonesia",
        "manila" | "cebu" => "Philippines",
        "shanghai" | "beijing" | "shenzhen" | "guangzhou" => "China",
        _ => "",
    }
}

/// The profile's addresses in `country` (a card's country, by its English name): (label, address) — by each address's
/// structured ISO country field (`home_country`, `address_country:<label>`), never by the address's words.
pub fn own_addresses(profile: &Value, country: &str) -> Vec<(String, String)> {
    let iso = crate::places::iso_of(country);
    let mut out = Vec::new();
    if iso.is_empty() {
        return out;
    }
    if let Some(o) = profile.as_object() {
        for (k, v) in o {
            let ck = crate::places::country_key(k);
            if ck.is_empty() || !get_str(profile, &ck).eq_ignore_ascii_case(iso) {
                continue;
            }
            let addr = v.as_str().unwrap_or("").trim();
            if addr.is_empty() {
                continue;
            }
            let label = match k.strip_prefix("address:") {
                Some(l) => l.to_string(),
                None => k.trim_start_matches("address_").to_string(),
            };
            out.push((label, addr.to_string()));
        }
    }
    out
}

// ---------------------------------------------------------------- the transcript, read back

pub(crate) fn results_of<'a>(transcript: &'a [Value], tool: &str) -> Vec<(Value, &'a Value)> {
    // (the call's args, the tool message), in order
    let mut out = Vec::new();
    for tc in transcript.iter().flat_map(tool_calls) {
        let (name, args) = stored_call(tc);
        if name != tool {
            continue;
        }
        let id = get_str(tc, "id");
        if let Some(m) = transcript
            .iter()
            .find(|m| get_str(m, "role") == "tool" && get_str(m, "tool_call_id") == id)
        {
            out.push((args, m));
        }
    }
    out
}

pub(crate) fn json_of(m: &Value) -> Value {
    let c = content_of(m);
    let (Some(i), Some(j)) = (c.find('{'), c.rfind('}')) else {
        return Value::Null;
    };
    serde_json::from_str(&c[i..=j]).unwrap_or(Value::Null)
}

pub(crate) fn walk_messages(v: &Value, out: &mut Vec<Value>) {
    match v {
        Value::Object(m) => {
            if m.contains_key("id") && m.contains_key("subject") {
                out.push(v.clone());
            }
            m.values().for_each(|x| walk_messages(x, out));
        }
        Value::Array(a) => a.iter().for_each(|x| walk_messages(x, out)),
        _ => {}
    }
}

/// The position in the transcript of a tool message (for "after the last …" questions).
pub(crate) fn pos(transcript: &[Value], m: &Value) -> usize {
    transcript
        .iter()
        .position(|x| std::ptr::eq(x, m))
        .unwrap_or(0)
}

pub(crate) fn handed_back(m: &Value) -> bool {
    content_of(m).contains(super::guards::TAKEOVER_DONE_EN)
}

pub(crate) fn stopped(fill: &Value) -> Option<String> {
    let v = json_of(fill);
    v.get("filled")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .find(|f| truthy(f.get("stop").unwrap_or(&Value::Null)))
        .map(|f| get_str(f, "why"))
}

/// The questions a driven run put to the user after its form opened (`ask_user`, a `present_choices` with no pick —
/// a comparison that holds its pick is shown, not asked): the drivers' tests assert there are none. A page only read
/// (opened, then its elements read — a shop's results page before its choice card) is no form opening.
#[cfg(test)]
pub(crate) fn questions_after_navigate(transcript: &[Value]) -> Vec<String> {
    let calls: Vec<(String, Value)> = transcript
        .iter()
        .flat_map(tool_calls)
        .map(stored_call)
        .collect();
    let Some(nav) = calls.iter().enumerate().position(|(i, (n, _))| {
        n == "browser_navigate"
            && calls
                .get(i + 1)
                .is_none_or(|(next, _)| next != "browser_read_elements")
    }) else {
        return Vec::new();
    };
    calls[nav + 1..]
        .iter()
        .filter(|(n, a)| {
            n == "ask_user" || (n == "present_choices" && sget(a, "picked").trim().is_empty())
        })
        .map(|(n, a)| format!("{n} {a}"))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn today() -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 11, 1).unwrap()
    }

    fn card(skill: &str) -> &'static CardSpec {
        CARDS.iter().find(|c| c.skill == skill).unwrap()
    }

    fn step(id: &str, name: &str, args: Value, result: &str) -> Vec<Value> {
        vec![
            json!({"role": "assistant", "content": "", "tool_calls": [{"id": id, "type": "function",
                   "function": {"name": name, "arguments": args.to_string()}}]}),
            json!({"role": "tool", "tool_call_id": id, "content": result}),
        ]
    }

    /// The check of a call that FILLS the card (`file: true` unless the test says otherwise: the user asked for it).
    fn checked(mut args: Value, profile: &Value, mail_ready: bool, t: &[Value]) -> Checked {
        if args.get("file").is_none() {
            args["file"] = json!(true);
        }
        check(
            ARRIVAL_CARD_FILL,
            &args,
            &Ctx {
                profile,
                today: today(),
                mail_ready,
                transcript: t,
            },
        )
    }

    fn valid(args: Value, profile: &Value, mail_ready: bool, t: &[Value]) -> Value {
        match checked(args, profile, mail_ready, t) {
            Checked::Valid(a) => a,
            other => panic!("not valid: {other:?}"),
        }
    }

    /// The transcript as the run loop leaves it after a valid call: the user's message, the call, `running`.
    fn started(user: &str, args: &Value) -> Vec<Value> {
        let mut t = vec![json!({"role": "user", "content": user})];
        t.extend(step(
            "call_w",
            ARRIVAL_CARD_FILL,
            args.clone(),
            &json!({"status": "running", "args": args}).to_string(),
        ));
        t
    }

    fn sg_facts() -> Value {
        json!({"country": "Singapore", "flight_no": "SQ 231", "arrival_date": "2026-11-03", "origin_city": "Sydney",
               "arrival_airport": "SIN", "airline": "Singapore Airlines", "stay_name": "Marina Bayfront Hotel",
               "stay_address": "10 Bayfront Avenue, Singapore 018956", "stay_kind": "hotel", "purpose": "holiday"})
    }

    #[test]
    fn a_question_about_a_card_gets_what_to_file_and_the_official_link_and_nothing_is_filled() {
        // the user, 2026-11-04: "what do I need to file?" gets the requirements and the official link; filling in is
        // for when the user asks for it (file true)
        let p = json!({});
        let a = valid(
            json!({"country": "Singapore", "file": false, "arrival_date": "2026-11-10", "flight_no": "SQ 231"}),
            &p,
            false,
            &[],
        );
        assert_eq!(
            a,
            json!({"country": "Singapore", "file": false, "arrival_date": "2026-11-10"})
        );
        let m = Machine {
            profile: &p,
            mail_ready: true,
            today: today(),
            first: 0,
        };
        let Step::Done(res) = next(ARRIVAL_CARD_FILL, &a, &started("x", &a)[1..], &m) else {
            panic!("the information path opens nothing")
        };
        assert_eq!(res["status"], "info");
        let lines = super::super::show::lines(&res, false).join("\n");
        assert!(
            lines.contains("SG Arrival Card")
                && lines.contains("from 2026-11-07 to your arrival on 2026-11-10")
                && lines.contains(card("sg-arrival-card").start_url),
            "{lines}"
        );
        // no arrival day: the window in days
        let a = valid(
            json!({"country": "Thailand", "file": false}),
            &p,
            false,
            &[],
        );
        let Step::Done(res) = next(ARRIVAL_CARD_FILL, &a, &started("x", &a)[1..], &m) else {
            panic!()
        };
        assert!(
            get_str(&res, "when").contains("days before you arrive"),
            "{res}"
        );
    }

    const BACK: &str = "\"Takeover finished: the user handed browser control back.\"";

    /// Runs the machine from a started transcript, answering each code call with `answer(name, args)`, until it is
    /// done; panics on a question to the user after the form opened and on a code-written answer.
    fn run(
        args: &Value,
        t: Vec<Value>,
        m: &Machine,
        answer: impl Fn(&str, &Value) -> String,
    ) -> (Vec<Value>, Value) {
        run_page(args, t, m, answer, true)
    }

    /// The page check's answer: every declared element present (`on_final`), or only the user steps' markers.
    fn page_answer(a: &Value, on_final: bool) -> String {
        let mut els = Map::new();
        for r in a["read"].as_array().into_iter().flatten() {
            let name = sget(r, "name");
            let present = on_final || name.starts_with(PAGE_STEP);
            // the declared text of the card's element with this selector (the page shows what the card declares)
            let sel = sget(r, "sel");
            let text = CARDS
                .iter()
                .filter_map(|c| c.final_page)
                .find(|f| f.sel == sel && name == PAGE_FINAL)
                .map(|f| f.text)
                .unwrap_or("");
            els.insert(name, if present { json!([text]) } else { json!([]) });
        }
        format!(
            "<untrusted_content source=\"web page https://x.test/form\">{}</untrusted_content>",
            json!({"url": "https://x.test/form", "elements": els})
        )
    }

    /// `run` with the page check answered by `page_answer(on_final)` (the other calls by `answer`).
    fn run_page(
        args: &Value,
        mut t: Vec<Value>,
        m: &Machine,
        answer: impl Fn(&str, &Value) -> String,
        on_final: bool,
    ) -> (Vec<Value>, Value) {
        // a run FILLS the card (the user asked for it) unless the test says otherwise
        let mut filing = args.clone();
        if filing.get("file").is_none() {
            filing["file"] = json!(true);
        }
        let args = &filing;
        let from = t
            .iter()
            .rposition(|x| tool_calls(x).iter().any(|tc| get_str(tc, "id") == "call_w"))
            .unwrap_or(0);
        for _ in 0..40 {
            match next(ARRIVAL_CARD_FILL, args, &t[from..], m) {
                Step::Calls(c) => {
                    for call in &c {
                        assert!(call.id.starts_with(PREFIX), "{}", call.id);
                        assert!(
                            !t.iter()
                                .flat_map(tool_calls)
                                .any(|tc| get_str(tc, "id") == call.id),
                            "the id {} was used before",
                            call.id
                        );
                        let is_check = call.name == "browser_read_elements"
                            && call.args["read"].as_array().is_some_and(|r| {
                                r.iter().any(|x| sget(x, "name").starts_with("page_"))
                            });
                        let content = if is_check {
                            page_answer(&call.args, on_final)
                        } else {
                            answer(&call.name, &call.args)
                        };
                        t.extend(step(&call.id, &call.name, call.args.clone(), &content));
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

    /// The lines code appends to the reply for this result (agent/show.rs).
    fn shown(v: &Value) -> String {
        super::super::show::lines(v, false).join(" | ")
    }

    fn eticket() -> Value {
        json!({"from": "Singapore Airlines <noreply@singaporeairlines.example>", "subject": "Your e-ticket: SQ 231 SYD-SIN 2026-11-03",
               "body": "SQ 231 departs Sydney (SYD) 2026-11-03 09:10, arrives Singapore (SIN) 2026-11-03 18:40."})
    }

    #[test]
    fn the_schema_is_in_the_catalog_and_names_every_country() {
        let s = tool_schemas();
        assert_eq!(super::super::tools::tool_name(&s[0]), ARRIVAL_CARD_FILL);
        let countries = &s[0]["function"]["parameters"]["properties"]["country"]["enum"];
        assert_eq!(countries.as_array().unwrap().len(), CARDS.len());
        assert!(super::super::tools::is_local(ARRIVAL_CARD_FILL));
        assert_eq!(
            card_of("singapore").map(|c| c.skill),
            Some("sg-arrival-card")
        );
        assert_eq!(card_of("United Kingdom").map(|c| c.skill), Some("uk-eta"));
        assert!(card_of("France").is_none());
    }

    #[test]
    fn the_arguments_are_validated_and_normalised() {
        let p = json!({});
        // a flight number in another spelling, the origin's country from the city table, the purpose defaulted
        let mut a = sg_facts();
        a["flight_no"] = json!("sq231");
        a["purpose"] = json!("");
        a["arrival_airport"] = json!("Changi");
        let v = valid(a, &p, false, &[]);
        assert_eq!(v["flight_no"], "SQ 231");
        assert_eq!(v["origin_country"], "Australia");
        assert_eq!(
            (v["purpose"].as_str(), v["purpose_assumed"].as_bool()),
            (Some("holiday"), Some(true))
        );
        assert!(
            v.get("arrival_airport").is_none(),
            "not an IATA code: dropped"
        );
        assert!(v.get("read_mail").is_none());
        // what the model can fix
        for (k, bad, why) in [
            ("country", json!("France"), "country: one of"),
            ("flight_no", json!("12345"), "flight_no"),
            ("arrival_date", json!("2026-10-20"), "in the past"),
            ("arrival_date", json!("next week"), "not a YYYY-MM-DD"),
        ] {
            let mut a = sg_facts();
            a[k] = bad;
            match checked(a, &p, false, &[]) {
                Checked::Invalid(pr) => assert!(pr.iter().any(|x| x.contains(why)), "{k}: {pr:?}"),
                other => panic!("{k}: {other:?}"),
            }
        }
        // a departure before the arrival is dropped (the default applies)
        let mut a = sg_facts();
        a["departure_date"] = json!("2026-11-01");
        assert!(valid(a, &p, false, &[]).get("departure_date").is_none());
        // the UK ETA has no trip section: the country alone runs
        let v = valid(
            json!({"country": "United Kingdom", "occupation": "Software engineer"}),
            &p,
            true,
            &[],
        );
        assert_eq!(v["occupation"], "Software engineer");
        assert!(v.get("read_mail").is_none());
    }

    #[test]
    fn the_bookings_are_fetched_before_anything_is_asked_and_only_the_user_is_asked_the_rest() {
        let p = json!({});
        let bare = json!({"country": "Thailand", "purpose": "holiday"});
        // a mailbox: the bookings first
        assert_eq!(valid(bare.clone(), &p, true, &[])["read_mail"], true);
        // no mailbox: what only the user can give, before any page opens
        match checked(json!({"country": "Thailand"}), &p, false, &[]) {
            Checked::NeedsInput(m) => {
                let m = m.join(" | ");
                assert!(
                    m.contains("flight_no")
                        && m.contains("arrival_date")
                        && m.contains("stay")
                        && m.contains("purpose"),
                    "{m}"
                );
            }
            other => panic!("{other:?}"),
        }
        // the bookings were fetched (a mail_read result in the chat): asked, not fetched again
        let mut t = started("Fill in my TDAC", &bare);
        t.extend(workflow::result_exchange(
            &t,
            ARRIVAL_CARD_FILL,
            &json!({"status": MAIL_READ, "mails": [], "hints": []}),
        ));
        match checked(bare.clone(), &p, true, &t) {
            Checked::NeedsInput(m) => assert!(!m.join(" ").contains("purpose"), "{m:?}"),
            other => panic!("{other:?}"),
        }
        // the call again carries the fields of the earlier call (the facts read in the mail, then the stay alone)
        let mut first = sg_facts();
        first
            .as_object_mut()
            .unwrap()
            .retain(|k, _| !k.starts_with("stay"));
        let t = started("Fill in my SG Arrival Card", &first);
        let profile = json!({"address:Singapore flat": "10 Bayfront Avenue, Singapore 018956",
                             "address_country:Singapore flat": "SG",
                             "address:Parents in Singapore": "5 Jalan Kayu, Singapore 799487",
                             "address_country:Parents in Singapore": "SG"});
        let v = valid(
            json!({"country": "Singapore", "own_address": "parents in singapore"}),
            &profile,
            false,
            &t,
        );
        assert_eq!(
            (
                v["flight_no"].as_str(),
                v["stay_address"].as_str(),
                v["stay_kind"].as_str()
            ),
            (
                Some("SQ 231"),
                Some("5 Jalan Kayu, Singapore 799487"),
                Some("own")
            )
        );
        match checked(
            json!({"country": "Singapore", "own_address": "Office"}),
            &profile,
            false,
            &t,
        ) {
            Checked::Invalid(pr) => assert!(pr[0].contains("\"Singapore flat\""), "{pr:?}"),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn the_users_own_address_is_the_stay_and_several_are_a_choice() {
        let one = json!({"address_home": "12 Harbour St, Sydney NSW 2000, Australia", "home_country": "AU",
                         "address:Singapore flat": "10 Bayfront Avenue, Singapore 018956",
                         "address_country:Singapore flat": "SG"});
        let mut a = sg_facts();
        for k in ["stay_name", "stay_address", "stay_kind"] {
            a.as_object_mut().unwrap().remove(k);
        }
        let v = valid(a.clone(), &one, false, &[]);
        assert_eq!(
            (v["stay_address"].as_str(), v["stay_kind"].as_str()),
            (Some("10 Bayfront Avenue, Singapore 018956"), Some("own"))
        );
        let two = json!({"address:Singapore flat": "10 Bayfront Avenue, Singapore 018956",
                         "address_country:Singapore flat": "SG",
                         "address:Parents in Singapore": "5 Jalan Kayu, Singapore 799487",
                         "address_country:Parents in Singapore": "SG"});
        let v = valid(a.clone(), &two, false, &[]);
        assert_eq!(v["choose_address"], true);
        // the choice card, then the turn ends with the question as data
        let t = started("Please fill in my SG Arrival Card", &v);
        let m = Machine {
            profile: &two,
            mail_ready: false,
            today: today(),
            first: 0,
        };
        let (t, res) = run(&v, t, &m, |_, _| "{\"shown\": 2, \"picked\": \"\"}".into());
        let cards = calls_of(&t, "present_choices");
        assert_eq!(cards.len(), 1);
        let labels: Vec<String> = cards[0]["options"]
            .as_array()
            .unwrap()
            .iter()
            .map(|o| sget(o, "label"))
            .collect();
        assert_eq!(labels, ["Singapore flat", "Parents in Singapore"]);
        assert!(cards[0].get("picked").is_none());
        assert_eq!(res["status"], "needs_choice");
        assert!(calls_of(&t, "browser_navigate").is_empty());
        assert_eq!(own_addresses(&two, "Malaysia").len(), 0);
    }

    #[test]
    fn the_bookings_are_fetched_by_code_then_the_card_is_filled_and_handed_over() {
        let p = json!({"name_en": "JORDAN LU", "nationality": "Australia"});
        let m = Machine {
            profile: &p,
            mail_ready: true,
            today: today(),
            first: 0,
        };
        // 1. the bare call: the two fixed searches, the hits, then mail_read for the model to read them
        let bare = valid(
            json!({"country": "Singapore", "purpose": "holiday"}),
            &p,
            true,
            &[],
        );
        let t = started(
            "Please fill in my SG Arrival Card for my trip to Singapore — it's a holiday.",
            &bare,
        );
        let (t, res) = run(&bare, t, &m, |name, _| {
            match name {
            "gmail_search" => "{\"count\": 1, \"messages\": [{\"id\": \"m1\", \"subject\": \"Your e-ticket: SQ 231\"}]}".into(),
            "gmail_get_message" => eticket().to_string(),
            other => panic!("{other}"),
        }
        });
        let q = calls_of(&t, "gmail_search");
        assert_eq!(q.len(), 2);
        assert!(sget(&q[0], "query")
            .contains("(flight OR e-ticket OR itinerary) (Singapore OR SIN OR Changi)"));
        assert_eq!(calls_of(&t, "gmail_get_message").len(), 1, "m1 once");
        assert_eq!(res["status"], MAIL_READ);
        assert_eq!(res["mails"], json!(["m1"]));
        assert!(calls_of(&t, "browser_navigate").is_empty());
        // 2. the model calls again with what the mail says: the page, the fill, the hand-over, the reference
        let mut t = t;
        t.extend(workflow::result_exchange(&t, ARRIVAL_CARD_FILL, &res));
        let args = valid(sg_facts(), &p, true, &t);
        assert!(args.get("read_mail").is_none());
        t.extend(step(
            "call_w",
            ARRIVAL_CARD_FILL,
            sg_facts(),
            &json!({"status": "running", "args": args}).to_string(),
        ));
        let first = t
            .iter()
            .flat_map(tool_calls)
            .filter(|tc| get_str(tc, "id").starts_with(PREFIX))
            .count();
        let m = Machine { first, ..m };
        let (t, res) = run(&args, t, &m, |name, _| {
            match name {
            "browser_navigate" => "<untrusted_content source=\"web page https://eservices.ica.gov.sg/arrivalcard/sgac\">{}</untrusted_content>".into(),
            "form_fill" => "{\"url\": \"x\", \"filled\": [{\"op\": \"text\", \"field\": \"flight\", \"ok\": true}], \"skipped\": [{\"field\": \"phone\", \"why\": \"no value\"}]}".into(),
            "browser_request_takeover" => BACK.into(),
            "browser_read_elements" => "<untrusted_content source=\"web page https://eservices.ica.gov.sg/sgac_done\">{\"elements\": {\"reference\": [\"DE202611020123\"]}}</untrusted_content>".into(),
            other => panic!("{other}"),
        }
        });
        assert_eq!(
            calls_of(&t, "browser_navigate")[0]["url"],
            card("sg-arrival-card").start_url
        );
        let fill = &calls_of(&t, "form_fill")[0];
        assert_eq!(
            (sget(fill, "flight_no"), sget(fill, "stay_name")),
            ("SQ 231".into(), "Marina Bayfront Hotel".into())
        );
        let reason = sget(&calls_of(&t, "browser_request_takeover")[0], "reason");
        assert!(
            reason.contains("tick the declaration and press Submit")
                && reason.contains("fill in phone")
                && reason.contains("arrival + 7 days")
                && reason.contains("next city Sydney"),
            "{reason}"
        );
        // (the reference read from the stand-in's confirmation page is not checked: a real confirmation page has
        // none of the stand-in's element ids)
        assert_eq!(res["trip"]["stay"], "Marina Bayfront Hotel");
        // the ids went on from the first run's
        let ids: Vec<String> = t
            .iter()
            .flat_map(tool_calls)
            .map(|tc| get_str(tc, "id"))
            .filter(|i| i.starts_with(PREFIX))
            .collect();
        let mut uniq = ids.clone();
        uniq.dedup();
        assert_eq!(ids.len(), uniq.len(), "{ids:?}");
    }

    #[test]
    fn the_mail_read_result_leaves_the_stay_to_the_users_own_address() {
        // arrival_address_real 2026-11-01: the result listed the stay as missing, and the model asked the user for a
        // hotel instead of calling again (the profile's one Singapore address is the stay)
        let p = json!({"address:Singapore flat": "10 Bayfront Avenue, Singapore 018956",
                       "address_country:Singapore flat": "SG"});
        let m = Machine {
            profile: &p,
            mail_ready: true,
            today: today(),
            first: 0,
        };
        let bare = valid(json!({"country": "Singapore"}), &p, true, &[]);
        let (_, res) = run(
            &bare,
            started("Fill in my SG Arrival Card", &bare),
            &m,
            |name, _| match name {
                "gmail_search" => "{\"count\": 1, \"messages\": [{\"id\": \"m1\"}]}".into(),
                _ => eticket().to_string(),
            },
        );
        assert_eq!(res["status"], MAIL_READ);
        assert_eq!(res["call_again"], ARRIVAL_CARD_FILL);
        assert_eq!(res["facts_from_mail"], json!(["flight_no", "arrival_date"]));
        assert_eq!(res["own_address_on_profile"], true);
        // no address on the profile: the stay is one of the facts the mails may give
        let none = json!({});
        let m = Machine {
            profile: &none,
            ..m
        };
        let (_, res) = run(
            &bare,
            started("Fill in my SG Arrival Card", &bare),
            &m,
            |name, _| match name {
                "gmail_search" => "{\"count\": 0, \"messages\": []}".into(),
                _ => eticket().to_string(),
            },
        );
        assert_eq!(
            res["facts_from_mail"],
            json!(["flight_no", "arrival_date", "stay"])
        );
    }

    #[test]
    fn no_reference_after_the_hand_back_is_not_confirmed() {
        let p = json!({});
        let args = valid(sg_facts(), &p, false, &[]);
        let m = Machine {
            profile: &p,
            mail_ready: false,
            today: today(),
            first: 0,
        };
        let (_, res) = run(&args, started("x", &args), &m, |name, _| {
            match name {
            "browser_request_takeover" => BACK.into(),
            "form_fill" => "{\"url\": \"x\", \"filled\": []}".into(),
            _ => "<untrusted_content source=\"web page https://eservices.ica.gov.sg/sgarrivalcard/\">Review your details</untrusted_content>".into(),
        }
        });
        assert_eq!(res["status"], "not_confirmed");
        assert!(
            shown(&res).contains("- Not confirmed: no reference showed"),
            "{res}"
        );
        // a takeover that ended without the hand-back: the page is the user's
        let (_, res) = run(&args, started("x", &args), &m, |name, _| match name {
            "browser_request_takeover" => "\"The user did not hand the browser back.\"".into(),
            _ => "{\"url\": \"x\", \"filled\": []}".into(),
        });
        assert_eq!(res["status"], "handed_over");
        assert!(
            shown(&res).contains("- Your step: ") && shown(&res).contains("tick the declaration"),
            "{res}"
        );
    }

    #[test]
    fn a_users_own_step_is_handed_over_then_the_fill_goes_on_and_twice_undone_ends_the_run() {
        let p = json!({});
        let args = valid(
            json!({"country": "China", "flight_no": "MU 562", "arrival_date": "2026-11-02", "origin_city": "Sydney",
                   "arrival_airport": "PVG", "stay_name": "Bund Riverside Hotel",
                   "stay_address": "99 Zhongshan East 1st Road", "purpose": "holiday"}),
            &p,
            false,
            &[],
        );
        let m = Machine {
            profile: &p,
            mail_ready: false,
            today: today(),
            first: 0,
        };
        let stop = "{\"url\": \"https://s.nia.gov.cn/x\", \"filled\": [{\"op\": \"require\", \"field\": \"photo\", \"ok\": false, \"stop\": true, \"why\": \"the passport photo upload (step 1) is the user's: hand the page over for it, then call form_fill again\"}]}";
        let (t, res) = run(
            &args,
            started("Fill in my China arrival card", &args),
            &m,
            |name, _| {
                match name {
            "browser_navigate" => "<untrusted_content source=\"web page https://s.nia.gov.cn/ArrivalCardFillingPC/\">{}</untrusted_content>".into(),
            "form_fill" => stop.into(),
            "browser_request_takeover" => BACK.into(),
            other => panic!("{other}"),
        }
            },
        );
        let names: Vec<String> = t
            .iter()
            .flat_map(tool_calls)
            .map(|tc| stored_call(tc).0)
            .collect();
        assert_eq!(
            names,
            [
                ARRIVAL_CARD_FILL,
                "browser_navigate",
                "form_fill",
                "browser_read_elements",
                "browser_request_takeover",
                "form_fill",
                "browser_read_elements",
                "browser_request_takeover",
                "form_fill"
            ]
        );
        let reason = sget(&calls_of(&t, "browser_request_takeover")[0], "reason");
        assert!(
            reason.starts_with("This step is yours: passport photo upload"),
            "{reason}"
        );
        assert_eq!(calls_of(&t, "form_fill")[1]["flight_no"], "MU 562");
        assert_eq!(res["status"], "waiting_on_user");
        assert!(
            shown(&res).contains("- Waiting on your step: passport photo upload"),
            "{res}"
        );
    }

    #[test]
    fn an_early_trip_gets_a_reminder_for_the_morning_the_window_opens() {
        let p = json!({});
        let mut a = sg_facts();
        a["arrival_date"] = json!("2026-12-20");
        let args = valid(a, &p, false, &[]);
        let m = Machine {
            profile: &p,
            mail_ready: false,
            today: today(),
            first: 0,
        };
        let (t, res) = run(
            &args,
            started("Fill in my SG Arrival Card", &args),
            &m,
            |_, _| "created".into(),
        );
        let s = calls_of(&t, "schedule_create");
        assert_eq!(sget(&s[0], "spec"), "0 8 17 12 *");
        assert!(sget(&s[0], "goal").contains("SQ 231"));
        assert!(calls_of(&t, "browser_navigate").is_empty());
        assert_eq!(
            (res["status"].as_str(), res["opens"].as_str()),
            (Some("scheduled"), Some("2026-12-17"))
        );
        assert!(
            shown(&res).contains("can be filed from: 17 December 2026")
                && shown(&res).contains("- Reminder: the morning of 17 December 2026"),
            "{res}"
        );
        // Visit Japan Web has no window: the page at once
        let jp = valid(
            json!({"country": "Japan", "flight_no": "NH 880", "arrival_date": "2026-12-20", "stay_name": "Shinjuku Granbell Inn"}),
            &p,
            false,
            &[],
        );
        let Step::Calls(c) = next(ARRIVAL_CARD_FILL, &jp, &started("x", &jp)[1..], &m) else {
            panic!()
        };
        assert_eq!(c[0].name, "browser_navigate");
    }

    /// Cambodia's travel step requires the stay's phone ("Hotel contact number", walked on the live site 2026-11-02:
    /// its Next stays blocked without it): never invented — asked before the form opens, passed on to the form
    /// function once given (a malformed one is the model's to fix); the user's own home takes their own phone.
    #[test]
    fn cambodia_asks_for_the_hotels_phone_before_the_form_opens() {
        let p = json!({});
        let base = json!({"country": "Cambodia", "flight_no": "SQ 166", "arrival_date": "2026-11-02",
                          "arrival_airport": "PNH", "stay_name": "Riverside Palace Hotel", "purpose": "holiday"});
        match checked(base.clone(), &p, false, &[]) {
            Checked::NeedsInput(m) => {
                assert!(m.iter().any(|x| x.starts_with("stay_phone")), "{m:?}")
            }
            other => panic!("not needs_input: {other:?}"),
        }
        let mut with = base.clone();
        with["stay_phone"] = json!("+855 23 000 000");
        let a = valid(with, &p, false, &[]);
        assert_eq!(a["stay_phone"], "+855 23 000 000");
        assert_eq!(Trip::from_args(&a).args()["stay_phone"], "+855 23 000 000");
        let mut bad = base.clone();
        bad["stay_phone"] = json!("ask the front desk");
        assert!(matches!(checked(bad, &p, false, &[]), Checked::Invalid(_)));
        // the user's own home: no question (their own phone is the contact)
        let mut own = base.clone();
        own["stay_kind"] = json!("own");
        own["stay_name"] = json!("");
        own["stay_address"] = json!("12 Street 240, Phnom Penh");
        assert!(matches!(checked(own, &p, false, &[]), Checked::Valid(_)));
        // a card that does not need it: no question
        let mut id = base;
        id["country"] = json!("Indonesia");
        assert!(matches!(checked(id, &p, false, &[]), Checked::Valid(_)));
    }

    /// Taiwan's itinerary requires the flight that leaves the country (arrival_cards2_real 2026-11-02: "Expected Mode
    /// of Departure: Please choose an option." on step 3): never invented — asked before the form opens, and passed on
    /// to the form function once given (a malformed one is the model's to fix).
    #[test]
    fn taiwan_asks_for_the_departure_flight_before_the_form_opens() {
        let p = json!({});
        let base = json!({"country": "Taiwan", "flight_no": "CI 52", "arrival_date": "2026-11-02",
                          "stay_name": "Xinyi Park Hotel", "purpose": "holiday"});
        match checked(base.clone(), &p, false, &[]) {
            Checked::NeedsInput(m) => assert!(
                m.iter().any(|x| x.starts_with("departure_flight_no")),
                "{m:?}"
            ),
            other => panic!("not needs_input: {other:?}"),
        }
        let mut with = base.clone();
        with["departure_flight_no"] = json!("ci51");
        let a = valid(with, &p, false, &[]);
        assert_eq!(a["departure_flight_no"], "CI 51");
        assert_eq!(Trip::from_args(&a).args()["departure_flight_no"], "CI 51");
        let mut bad = base;
        bad["departure_flight_no"] = json!("tomorrow");
        assert!(matches!(checked(bad, &p, false, &[]), Checked::Invalid(_)));
        // a card that does not need it: no question
        let sg = json!({"country": "Singapore", "flight_no": "SQ 231", "arrival_date": "2026-11-02",
                        "stay_name": "Marina Bay Sands", "purpose": "holiday"});
        assert!(matches!(checked(sg, &p, false, &[]), Checked::Valid(_)));
    }

    #[test]
    fn every_card_fills_from_its_arguments_and_asks_nothing_after_the_page_opens() {
        let p = json!({});
        for c in CARDS {
            let args = valid(
                json!({"country": c.country, "flight_no": "XX 100", "arrival_date": "2026-11-02",
                       "stay_name": "Test Hotel", "purpose": "holiday", "occupation": "Engineer",
                       "departure_flight_no": "XX 101", "stay_phone": "+855 23 000 000"}),
                &p,
                false,
                &[],
            );
            let m = Machine {
                profile: &p,
                mail_ready: false,
                today: today(),
                first: 0,
            };
            let (t, res) = run(&args, started("x", &args), &m, |name, _| match name {
                "form_fill" => "{\"url\": \"x\", \"filled\": []}".into(),
                "browser_request_takeover" => BACK.into(),
                _ => "Submitted.".into(),
            });
            assert_eq!(
                calls_of(&t, "browser_navigate")[0]["url"],
                c.start_url,
                "{}",
                c.skill
            );
            assert!(questions_after_navigate(&t).is_empty(), "{}", c.skill);
            let fill = &calls_of(&t, "form_fill")[0];
            if c.needs_trip() {
                assert_eq!(sget(fill, "flight_no"), "XX 100", "{}", c.skill);
            }
            assert_eq!(sget(fill, "occupation"), "Engineer");
            assert!(
                ["submitted", "not_confirmed"].contains(&res["status"].as_str().unwrap_or("")),
                "{}: {res}",
                c.skill
            );
        }
    }

    #[test]
    fn ask_user_before_the_tool_is_refused() {
        let ev = |ty: &str, data: Value| json!({"type": ty, "data": data});
        let loaded = vec![ev(
            "tool_call",
            json!({"name": "load_skill", "args": {"name": "th-arrival-card"}}),
        )];
        let r = ask_before_tool(&loaded).unwrap();
        assert_eq!(r["tool"], ARRIVAL_CARD_FILL);
        let mut called = loaded.clone();
        called.push(ev(
            "workflow_call",
            json!({"tool": ARRIVAL_CARD_FILL, "result": {"status": "needs_input"}}),
        ));
        assert!(ask_before_tool(&called).is_none());
        assert!(ask_before_tool(&[ev(
            "tool_call",
            json!({"name": "load_skill", "args": {"name": "tokyo"}})
        )])
        .is_none());
    }

    #[test]
    fn the_email_step_is_codes_once_then_the_fill_goes_on_or_the_page_is_handed_over() {
        let p = json!({"email_personal": "michael@michaelyuan.com"});
        let args = valid(
            json!({"country": "Taiwan", "flight_no": "CI 52", "arrival_date": "2026-11-02", "origin_city": "Sydney",
                   "stay_name": "Xinyi Park Hotel", "purpose": "holiday", "departure_flight_no": "CI 51"}),
            &p,
            false,
            &[],
        );
        let m = Machine {
            profile: &p,
            mail_ready: true,
            today: today(),
            first: 0,
        };
        let stop = "{\"url\": \"https://twac.immigration.gov.tw/submit\", \"filled\": [{\"op\": \"require\", \"field\": \"traveler_form\", \"ok\": false, \"stop\": true, \"why\": \"the email verification (the code Taiwan emails) is the user's: hand the page over for it, then call form_fill again\"}]}";
        let names = |t: &[Value]| -> Vec<String> {
            t.iter()
                .flat_map(tool_calls)
                .map(|tc| stored_call(tc).0)
                .collect()
        };
        // a typed code: the fill again (the traveller's form now shows), then the review hand-over
        let fills = std::cell::Cell::new(0);
        let (t, _) = run(
            &args,
            started("Fill in my TWAC", &args),
            &m,
            |name, _| match name {
                "form_fill" => {
                    fills.set(fills.get() + 1);
                    if fills.get() == 1 {
                        stop.into()
                    } else {
                        "{\"url\": \"x\", \"filled\": []}".into()
                    }
                }
                "email_code" => {
                    "{\"status\": \"typed\", \"email_filled\": true, \"sent\": true}".into()
                }
                "browser_request_takeover" => BACK.into(),
                _ => "{}".into(),
            },
        );
        assert_eq!(
            names(&t)[..7],
            [
                ARRIVAL_CARD_FILL,
                "browser_navigate",
                "form_fill",
                "email_code",
                "form_fill",
                "browser_read_elements",
                "browser_request_takeover"
            ]
        );
        let code = &calls_of(&t, "email_code")[0];
        assert_eq!(code["email"], "michael@michaelyuan.com");
        assert_eq!(code["email_sel"], "input[name=email]");
        assert_eq!(code["from"], "immigration.gov.tw");
        assert!(sget(&calls_of(&t, "browser_request_takeover")[0], "reason")
            .contains("The Taiwan Arrival Card (TWAC) is filled in"));
        // no mailbox (or no code): the hand-over at the step, saying the email is in; one email step per run, also
        // after the hand-back
        let (t, res) = run(
            &args,
            started("Fill in my TWAC", &args),
            &m,
            |name, _| match name {
                "form_fill" => stop.into(),
                "email_code" => {
                    "{\"status\": \"no_mailbox\", \"email_filled\": true, \"sent\": false}".into()
                }
                "browser_request_takeover" => BACK.into(),
                _ => "{}".into(),
            },
        );
        assert_eq!(calls_of(&t, "email_code").len(), 1);
        let reason = sget(&calls_of(&t, "browser_request_takeover")[0], "reason");
        assert!(
            reason.starts_with("This step is yours: email verification"),
            "{reason}"
        );
        assert!(reason.contains("Your email is filled in"), "{reason}");
        // the result says the code could not be read, and why, as a line code appends to the reply
        assert_eq!(res["status"], EMAIL_CODE_NOT_READ);
        assert_eq!(res["reason"], "no_mailbox");
        assert!(
            shown(&res).contains(
                "- Email code: I could not read the verification code (no mailbox is connected)"
            ),
            "{res}"
        );
        // a card without an email step never calls it
        let sg = valid(sg_facts(), &p, false, &[]);
        let (t, _) = run(&sg, started("x", &sg), &m, |name, _| match name {
            "form_fill" => stop.into(),
            "browser_request_takeover" => BACK.into(),
            _ => "{}".into(),
        });
        assert!(calls_of(&t, "email_code").is_empty());
    }

    #[test]
    fn a_form_handed_over_before_its_final_page_says_it_stopped_early() {
        let p = json!({});
        let args = valid(sg_facts(), &p, false, &[]);
        let m = Machine {
            profile: &p,
            mail_ready: false,
            today: today(),
            first: 0,
        };
        let answer = |name: &str, _: &Value| match name {
            "form_fill" => "{\"url\": \"x\", \"filled\": []}".to_string(),
            "browser_request_takeover" => BACK.to_string(),
            _ => {
                "<untrusted_content source=\"web page https://x.test/form\">{}</untrusted_content>"
                    .to_string()
            }
        };
        // the page check reads the card's declared Submit (nothing is clicked)
        let (t, res) = run_page(&args, started("x", &args), &m, answer, false);
        let check = &calls_of(&t, "browser_read_elements")[0];
        assert_eq!(check["read"][0]["name"], PAGE_FINAL);
        assert_eq!(check["read"][0]["sel"], "button.general-button-container");
        let reason = sget(&calls_of(&t, "browser_request_takeover")[0], "reason");
        assert!(
            reason.contains("stopped before its last page (https://x.test/form)"),
            "{reason}"
        );
        assert_eq!(res["status"], "stopped_early");
        assert_eq!(res["page"], "https://x.test/form");
        assert!(shown(&res).contains("Nothing has been submitted"), "{res}");
        // on the final page: the usual review hand-over
        let (t, res) = run_page(&args, started("x", &args), &m, answer, true);
        let reason = sget(&calls_of(&t, "browser_request_takeover")[0], "reason");
        assert!(
            reason.starts_with("The SG Arrival Card is filled in"),
            "{reason}"
        );
        assert_eq!(res["status"], "not_confirmed");
        assert_eq!(res["on_final_page"], true);
        // every card declares where its hand-over is: the final page's Submit, or a user's step before the form
        for c in CARDS {
            assert!(
                c.final_page.is_some() || !c.user_step_at.is_empty(),
                "{}",
                c.skill
            );
        }
    }

    #[test]
    fn a_flight_number_is_an_airline_code_and_digits() {
        assert_eq!(norm_flight("sq-231"), Some("SQ 231".into()));
        assert_eq!(norm_flight("3K 685"), Some("3K 685".into()));
        assert_eq!(norm_flight("123 45"), None);
    }
}
