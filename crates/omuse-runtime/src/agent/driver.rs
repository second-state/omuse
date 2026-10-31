//! The code driver of a known-form skill (the arrival cards with a form function in `crate::forms`): once the skill
//! is loaded, code decides every step instead of the model — the fixed mail searches, the facts read from the
//! bookings (regexes; an extraction call only when they find nothing), the user's own address, the filing window,
//! the official page, `form_fill`, the hand-over for the user's own steps and for the review, the reference on the
//! confirmation and the answer. The next step is a function of the transcript alone (`next`), so a run suspended for
//! a takeover carries on where it was.

use std::sync::LazyLock;

use chrono::{Datelike, NaiveDate};
use omuse_common::util::{get_str, take_chars, truthy};
use regex::Regex;
use serde_json::{json, Map, Value};

use super::util::{content_of, sget, stored_call, tool_calls};
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
    /// the confirmation's reference, on the page after the user's Submit
    pub ref_re: &'static str,
}

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
        ref_re: r"\bDE\d{9,}\b",
    },
    CardSpec {
        skill: "my-arrival-card",
        name: "Malaysia Digital Arrival Card (MDAC)",
        country: "Malaysia",
        mail_words: "(Malaysia OR \"Kuala Lumpur\" OR KUL OR Penang OR PEN OR Johor OR \"Kota Kinabalu\")",
        start_url: "https://imigresen-online.imi.gov.my/mdac/main",
        host: "imigresen-online.imi.gov.my",
        window_days: 3,
        user_steps: "solve the slider puzzle and press Submit",
        ref_re: r"\bMDAC-[A-Z0-9-]{6,}\b",
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
        ref_re: r"\bKR-EA-\d{6,}\b",
    },
    CardSpec {
        skill: "id-arrival-card",
        name: "All Indonesia arrival card",
        country: "Indonesia",
        mail_words: "(Indonesia OR Bali OR Denpasar OR DPS OR Jakarta OR CGK OR Surabaya OR Batam)",
        start_url: "https://allindonesia.imigrasi.go.id/",
        host: "allindonesia.imigrasi.go.id",
        window_days: 3,
        user_steps: "tick the certification and press Submit",
        ref_re: r"\b[A-Z]{2,4}-?\d{8,}\b",
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
        ref_re: r"\bTWAC-?[A-Z0-9]{6,}\b",
    },
    CardSpec {
        skill: "th-arrival-card",
        name: "Thailand Digital Arrival Card (TDAC)",
        country: "Thailand",
        mail_words: "(Thailand OR Bangkok OR BKK OR DMK OR Phuket OR HKT OR \"Chiang Mai\")",
        start_url: "https://tdac.immigration.go.th/",
        host: "tdac.immigration.go.th",
        window_days: 3,
        user_steps: "type the CAPTCHA and press Submit",
        ref_re: r"\bTH[A-Z0-9]{7,}\b",
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
        ref_re: r"\bCN-ARC-[A-Z0-9-]{6,}\b",
    },
];

/// The card whose skill this run loaded last, when it is one the driver knows.
pub fn active(transcript: &[Value]) -> Option<&'static CardSpec> {
    transcript
        .iter()
        .flat_map(tool_calls)
        .filter_map(|tc| {
            let (name, args) = stored_call(tc);
            (name == "load_skill" && sget(&args, "file").trim().is_empty())
                .then(|| sget(&args, "name"))
        })
        .filter_map(|n| CARDS.iter().find(|c| c.skill == n))
        .next_back()
}

/// What the driver decides for the next turn.
#[derive(Debug, Clone, PartialEq)]
pub enum Step {
    /// run these calls (one turn)
    Calls(Vec<ToolCall>),
    /// the run ends with this answer
    Final(String),
    /// the regexes found no flight in these texts: one extraction call (the last resort), its JSON then recorded as
    /// an `extract_facts` result
    Extract(String),
}

/// The world the driver reads besides the transcript.
pub struct World<'a> {
    /// the user's messages of this chat, oldest first (the request last)
    pub user_texts: &'a [String],
    pub profile: &'a Value,
    pub today: NaiveDate,
    pub mail_ready: bool,
    pub zh: bool,
}

/// The trip facts the form function gets (`crate::forms::Facts` keys) plus where each came from.
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
}

impl Trip {
    pub fn args(&self) -> Value {
        json!({
            "flight_no": self.flight_no, "arrival_date": self.arrival_date, "origin_city": self.origin_city,
            "origin_country": self.origin_country, "arrival_airport": self.arrival_airport, "airline": self.airline,
            "stay_name": self.stay_name, "stay_address": self.stay_address, "stay_kind": self.stay_kind,
            "purpose": self.purpose, "departure_date": self.departure_date, "next_city": self.next_city,
        })
    }

    fn arrival(&self) -> Option<NaiveDate> {
        NaiveDate::parse_from_str(&self.arrival_date, "%Y-%m-%d").ok()
    }
}

static FLIGHT: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\b([A-Z][A-Z0-9])\s?(\d{2,4})\b").unwrap());
static ISO: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\b(\d{4}-\d{2}-\d{2})\b").unwrap());
static LONG_DATE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\b(\d{1,2})(?:st|nd|rd|th)?\s+(January|February|March|April|May|June|July|August|September|October|November|December|Jan|Feb|Mar|Apr|Jun|Jul|Aug|Sep|Sept|Oct|Nov|Dec)\.?,?\s+(\d{4})\b|\b(January|February|March|April|May|June|July|August|September|October|November|December|Jan|Feb|Mar|Apr|Jun|Jul|Aug|Sep|Sept|Oct|Nov|Dec)\.?\s+(\d{1,2})(?:st|nd|rd|th)?,?\s+(\d{4})\b").unwrap()
});
static ARRIVES: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)\barriv(?:es|ing|al)\b[^.\n]{0,40}?\b([A-Z][A-Za-z .'-]{2,30}?)\s*\(([A-Z]{3})\)",
    )
    .unwrap()
});
static DEPARTS: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\b(?:departs?|departing|from)\b\s*([A-Z][A-Za-z .'-]{2,30}?)\s*\(([A-Z]{3})\)")
        .unwrap()
});
static HOTEL_SUBJECT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)(?:booking|reservation)\s+(?:confirm(?:ed|ation))?:?\s*(?:for\s+|at\s+)?([A-Z][^\n:,]{3,60}?)(?:\s+\d{1,2}\s|\s*[-–]\s*|\s*$|\n)").unwrap()
});
static STAYING: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\bstay(?:ing)?\s+(?:at|in)\s+(?:the\s+)?([A-Z][^.,;\n]{3,60}?)(?:\s*[,.;\n]|\s+(?:on|from|for|at)\b|$)").unwrap()
});
static ADDRESS_WORDS: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\b(road|rd\.?|street|st\.?|avenue|ave\.?|jalan|lane|drive|boulevard|blvd|soi|lorong|district|floor|#\d|\d{4,6})\b").unwrap()
});

fn month_no(m: &str) -> u32 {
    match m.to_lowercase().as_str() {
        "january" | "jan" => 1,
        "february" | "feb" => 2,
        "march" | "mar" => 3,
        "april" | "apr" => 4,
        "may" => 5,
        "june" | "jun" => 6,
        "july" | "jul" => 7,
        "august" | "aug" => 8,
        "september" | "sep" | "sept" => 9,
        "october" | "oct" => 10,
        "november" | "nov" => 11,
        _ => 12,
    }
}

/// Every date written in `text`, in order (ISO and "3 November 2026" / "November 3, 2026").
pub fn dates_in(text: &str) -> Vec<(usize, NaiveDate)> {
    let mut out: Vec<(usize, NaiveDate)> = ISO
        .captures_iter(text)
        .filter_map(|c| {
            let m = c.get(1)?;
            NaiveDate::parse_from_str(m.as_str(), "%Y-%m-%d")
                .ok()
                .map(|d| (m.start(), d))
        })
        .collect();
    for c in LONG_DATE.captures_iter(text) {
        let (d, m, y) = match (c.get(1), c.get(2), c.get(3), c.get(4), c.get(5), c.get(6)) {
            (Some(d), Some(m), Some(y), _, _, _) => (d, m, y),
            (_, _, _, Some(m), Some(d), Some(y)) => (d, m, y),
            _ => continue,
        };
        if let (Ok(day), Ok(year)) = (d.as_str().parse::<u32>(), y.as_str().parse::<i32>()) {
            if let Some(date) = NaiveDate::from_ymd_opt(year, month_no(m.as_str()), day) {
                out.push((c.get(0).unwrap().start(), date));
            }
        }
    }
    out.sort_by_key(|(i, _)| *i);
    out
}

fn is_flight_text(t: &str) -> bool {
    static F: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?i)\b(flight|e-?ticket|itinerary|departs?|arrives?|boarding)\b").unwrap()
    });
    F.is_match(t)
}

fn country_of_city(city: &str) -> &'static str {
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

/// The trip read from the bookings and the user's words: `texts` are (sender, subject, body) of the fetched mail,
/// `user_texts` the user's messages. Regexes only.
pub fn extract(
    texts: &[(String, String, String)],
    user_texts: &[String],
    today: NaiveDate,
) -> Trip {
    let mut t = Trip::default();
    // the flight: the e-ticket first, then the user's words
    let flight_texts: Vec<String> = texts
        .iter()
        .filter(|(_, s, b)| is_flight_text(&format!("{s}\n{b}")))
        .map(|(f, s, b)| format!("{f}\n{s}\n{b}"))
        .chain(user_texts.iter().cloned())
        .collect();
    for text in &flight_texts {
        if t.flight_no.is_empty() {
            if let Some(c) = FLIGHT.captures_iter(text).find(|c| {
                let code = &c[1];
                code.chars()
                    .all(|ch| ch.is_ascii_uppercase() || ch.is_ascii_digit())
                    && code != "DE"
                    && !code.chars().all(|ch| ch.is_ascii_digit())
            }) {
                t.flight_no = format!("{} {}", &c[1], &c[2]);
            }
        }
        if t.arrival_airport.is_empty() {
            if let Some(c) = ARRIVES.captures(text) {
                t.arrival_airport = c[2].to_string();
                // the arrival date: the first date after "arrives", else the latest date in the text
                let after = &text[c.get(0).unwrap().end()..];
                if let Some((_, d)) = dates_in(after).first() {
                    t.arrival_date = d.format("%Y-%m-%d").to_string();
                }
            }
        }
        if t.origin_city.is_empty() {
            if let Some(c) = DEPARTS.captures(text) {
                t.origin_city = c[1].trim().to_string();
            }
        }
        if t.arrival_date.is_empty() {
            let mut ds: Vec<NaiveDate> = dates_in(text)
                .into_iter()
                .map(|(_, d)| d)
                .filter(|d| *d >= today - chrono::Duration::days(1))
                .collect();
            ds.sort();
            if let Some(d) = ds.last() {
                t.arrival_date = d.format("%Y-%m-%d").to_string();
            }
        }
        if t.airline.is_empty() && !t.flight_no.is_empty() {
            if let Some(first) = text.lines().next() {
                let name = first
                    .split('<')
                    .next()
                    .unwrap_or("")
                    .trim()
                    .trim_matches('"');
                if !name.is_empty()
                    && !name.contains('@')
                    && name.len() <= 40
                    && !first.contains("Dear")
                {
                    t.airline = name.to_string();
                }
            }
        }
    }
    t.origin_country = country_of_city(&t.origin_city).to_string();
    // the stay: a booking's subject and its address line, else the user's words
    for (_, subject, body) in texts {
        if is_flight_text(subject) && !subject.to_lowercase().contains("hotel") {
            continue;
        }
        let Some(c) = HOTEL_SUBJECT.captures(subject) else {
            continue;
        };
        let name = c[1].trim().to_string();
        if name.is_empty() {
            continue;
        }
        t.stay_name = name.clone();
        t.stay_kind = "hotel".into();
        // "<name>, <address>." in the body
        if let Some(i) = body.find(&name) {
            let rest = &body[i + name.len()..];
            let rest = rest.trim_start_matches([',', ' ']);
            let end = rest.find(['.', '\n']).unwrap_or(rest.len());
            let addr = rest[..end].trim();
            if ADDRESS_WORDS.is_match(addr) {
                t.stay_address = addr.to_string();
            }
        }
        break;
    }
    if t.stay_name.is_empty() {
        for u in user_texts {
            if let Some(c) = STAYING.captures(u) {
                let s = c[1].trim().to_string();
                if ADDRESS_WORDS.is_match(&s) {
                    t.stay_address = s;
                    t.stay_kind = "other".into();
                } else {
                    t.stay_name = s;
                    t.stay_kind = "hotel".into();
                }
                break;
            }
        }
    }
    // the purpose, from the user's words only
    static PURPOSE: LazyLock<[(Regex, &str); 5]> = LazyLock::new(|| {
        [
            (
                Regex::new(
                    r"(?i)\b(holiday|vacation|leisure|touris[mt]|sightseeing)\b|度假|旅游|旅遊",
                )
                .unwrap(),
                "holiday",
            ),
            (
                Regex::new(r"(?i)\b(business|work trip|conference|meeting|client)\b|出差|商务")
                    .unwrap(),
                "business",
            ),
            (
                Regex::new(r"(?i)\b(family|relatives|parents|friends?)\b|探亲|家人").unwrap(),
                "family",
            ),
            (
                Regex::new(r"(?i)\b(study|studies|university|school)\b|留学|上学").unwrap(),
                "study",
            ),
            (Regex::new(r"(?i)\btransit\b|转机|過境").unwrap(), "transit"),
        ]
    });
    for u in user_texts.iter().rev() {
        if let Some((_, p)) = PURPOSE.iter().find(|(r, _)| r.is_match(u)) {
            t.purpose = p.to_string();
            break;
        }
    }
    t
}

/// The profile's addresses in `country`: (label, address).
pub fn own_addresses(profile: &Value, country: &str) -> Vec<(String, String)> {
    let needle = country.to_lowercase();
    let mut out = Vec::new();
    if let Some(o) = profile.as_object() {
        for (k, v) in o {
            if !k.starts_with("address") {
                continue;
            }
            let addr = v.as_str().unwrap_or("").trim();
            if addr.to_lowercase().contains(&needle) {
                let label = match k.strip_prefix("address:") {
                    Some(l) => l.to_string(),
                    None => k.trim_start_matches("address_").to_string(),
                };
                out.push((label, addr.to_string()));
            }
        }
    }
    out
}

// ---------------------------------------------------------------- the transcript, read back

fn results_of<'a>(transcript: &'a [Value], tool: &str) -> Vec<(Value, &'a Value)> {
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

fn json_of(m: &Value) -> Value {
    let c = content_of(m);
    let (Some(i), Some(j)) = (c.find('{'), c.rfind('}')) else {
        return Value::Null;
    };
    serde_json::from_str(&c[i..=j]).unwrap_or(Value::Null)
}

fn walk_messages(v: &Value, out: &mut Vec<Value>) {
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
fn pos(transcript: &[Value], m: &Value) -> usize {
    transcript
        .iter()
        .position(|x| std::ptr::eq(x, m))
        .unwrap_or(0)
}

fn handed_back(m: &Value) -> bool {
    content_of(m).contains(super::guards::TAKEOVER_DONE_EN)
}

fn stopped(fill: &Value) -> Option<String> {
    let v = json_of(fill);
    v.get("filled")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .find(|f| truthy(f.get("stop").unwrap_or(&Value::Null)))
        .map(|f| get_str(f, "why"))
}

fn call(n: usize, name: &str, args: Value) -> ToolCall {
    ToolCall {
        id: format!("drv_{n}"),
        name: name.into(),
        args,
    }
}

/// Whether the driver already asked the user (the question is an earlier assistant message of this chat).
fn asked_before(transcript: &[Value], marker: &str) -> bool {
    transcript
        .iter()
        .any(|m| get_str(m, "role") == "assistant" && content_of(m).contains(marker))
}

const ASK_MARK: &str = "arrive on (number and date)";
const WHICH_MARK: &str = "Which of these addresses";

/// The next step of the run.
pub fn next(card: &CardSpec, transcript: &[Value], w: &World) -> Step {
    let n = transcript
        .iter()
        .flat_map(tool_calls)
        .filter(|tc| get_str(tc, "id").starts_with("drv_"))
        .count()
        + 1;
    // 1. the bookings: the two fixed searches, then the first hits of each
    let searches = results_of(transcript, "gmail_search");
    if w.mail_ready && searches.is_empty() {
        let q = |what: &str| json!({"query": format!("{what} {} newer_than:60d", card.mail_words), "max_results": 10});
        return Step::Calls(vec![
            call(n, "gmail_search", q("(flight OR e-ticket OR itinerary)")),
            call(
                n + 1,
                "gmail_search",
                q("(hotel OR booking OR reservation)"),
            ),
        ]);
    }
    let fetched = results_of(transcript, "gmail_get_message");
    let fetched_ids: Vec<String> = fetched.iter().map(|(a, _)| sget(a, "message_id")).collect();
    let mut want: Vec<String> = Vec::new();
    for (_, m) in &searches {
        let mut hits = Vec::new();
        walk_messages(&json_of(m), &mut hits);
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
                .map(|(i, id)| call(n + i, "gmail_get_message", json!({"message_id": id})))
                .collect(),
        );
    }
    // 2. the facts
    let texts: Vec<(String, String, String)> = fetched
        .iter()
        .map(|(_, m)| {
            let v = json_of(m);
            (
                get_str(&v, "from"),
                get_str(&v, "subject"),
                get_str(&v, "body"),
            )
        })
        .collect();
    let mut trip = extract(&texts, w.user_texts, w.today);
    if let Some((_, m)) = results_of(transcript, "extract_facts").last() {
        let v = json_of(m);
        let fill = |cur: &mut String, k: &str| {
            if cur.is_empty() {
                *cur = get_str(&v, k).trim().to_string();
            }
        };
        fill(&mut trip.flight_no, "flight_no");
        fill(&mut trip.arrival_date, "arrival_date");
        fill(&mut trip.origin_city, "origin_city");
        fill(&mut trip.stay_name, "stay_name");
        fill(&mut trip.stay_address, "stay_address");
        fill(&mut trip.purpose, "purpose");
        if trip.stay_kind.is_empty() && !(trip.stay_name.is_empty() && trip.stay_address.is_empty())
        {
            trip.stay_kind = "hotel".into();
        }
    }
    let flight_known = !trip.flight_no.is_empty() && trip.arrival().is_some();
    // the regexes found no flight in mail that is there: the extraction call, once
    // (or in the user's own answer to the question: anything beyond the bare request)
    if !flight_known
        && (!texts.is_empty() || w.user_texts.len() > 1)
        && results_of(transcript, "extract_facts").is_empty()
    {
        let joined: String = texts
            .iter()
            .map(|(f, s, b)| format!("From: {f}\nSubject: {s}\n{b}"))
            .chain(w.user_texts.iter().map(|u| format!("User: {u}")))
            .collect::<Vec<_>>()
            .join("\n\n");
        return Step::Extract(take_chars(&joined, 12000).to_string());
    }
    // 3. the stay: a booking wins; else the user's own address in the country
    let own = own_addresses(w.profile, card.country);
    if trip.stay_name.is_empty() && trip.stay_address.is_empty() {
        match own.len() {
            1 => {
                trip.stay_address = own[0].1.clone();
                trip.stay_kind = "own".into();
            }
            k if k > 1 => {
                // the user named one of them?
                let picked = w.user_texts.iter().rev().find_map(|u| {
                    let ul = u.to_lowercase();
                    own.iter()
                        .find(|(l, _)| !l.is_empty() && ul.contains(&l.to_lowercase()))
                });
                match picked {
                    Some((_, addr)) => {
                        trip.stay_address = addr.clone();
                        trip.stay_kind = "own".into();
                    }
                    None if !asked_before(transcript, WHICH_MARK) => {
                        let list: Vec<String> =
                            own.iter().map(|(l, a)| format!("- **{l}**: {a}")).collect();
                        return Step::Final(if w.zh {
                            format!(
                                "你的档案里有几个{}地址。这次住哪一个？\n{}",
                                card.country,
                                list.join("\n")
                            )
                        } else {
                            format!(
                                "{WHICH_MARK} in {} are you staying at this time?\n{}",
                                card.country,
                                list.join("\n")
                            )
                        });
                    }
                    None => {}
                }
            }
            _ => {}
        }
    }
    // 4. missing facts: ask once (the flight, the stay, the purpose), then fill what there is
    let missing: Vec<&str> = [
        (!flight_known, "flight"),
        (
            trip.stay_name.is_empty() && trip.stay_address.is_empty(),
            "stay",
        ),
        (trip.purpose.is_empty(), "purpose"),
    ]
    .iter()
    .filter(|(m, _)| *m)
    .map(|(_, k)| *k)
    .collect();
    if !missing.is_empty()
        && !asked_before(transcript, ASK_MARK)
        && !asked_before(transcript, "哪个航班")
    {
        let mut lines = Vec::new();
        if missing.contains(&"flight") {
            lines.push(format!("- Which flight do you {ASK_MARK}?"));
        }
        if missing.contains(&"stay") {
            lines.push(format!(
                "- Where are you staying in {} (hotel name, or the address)?",
                card.country
            ));
        }
        if missing.contains(&"purpose") {
            lines.push(
                "- Is the trip a holiday, business, visiting family, study or transit?".to_string(),
            );
        }
        let mut q = format!(
            "To fill in your {}, I still need:\n{}\nYou can also upload the e-ticket or booking confirmation.",
            card.name,
            lines.join("\n")
        );
        if !w.mail_ready {
            q.push_str("\n\n(Connect your mailbox in Connections and I can read the flight and the stay from your booking emails myself.)");
        }
        return Step::Final(q);
    }
    if trip.purpose.is_empty() {
        trip.purpose = "holiday".into();
    }
    // 5. the filing window: too early → a reminder, and the answer
    if let (Some(arr), true) = (trip.arrival(), card.window_days > 0) {
        let opens = arr - chrono::Duration::days(card.window_days);
        if w.today < opens {
            let made = results_of(transcript, "schedule_create");
            if made.is_empty() {
                return Step::Calls(vec![call(
                    n,
                    "schedule_create",
                    json!({"name": format!("{}: {}", card.name, trip.flight_no),
                           "goal": format!("Fill in my {} for {} arriving on {}", card.name, trip.flight_no, trip.arrival_date),
                           "kind": "cron", "spec": format!("0 8 {} {} *", opens.day(), opens.month())}),
                )]);
            }
            return Step::Final(format!(
                "Your {} can only be filed within the {} days before arrival: for {} arriving on {} that is from **{}**. \
I've set a reminder for that morning and will fill it in then. Nothing is filed yet.",
                card.name,
                card.window_days,
                trip.flight_no,
                trip.arrival_date,
                opens.format("%-d %B %Y")
            ));
        }
    }
    // 6. the official page
    let opened = results_of(transcript, "browser_navigate")
        .iter()
        .any(|(a, _)| sget(a, "url").contains(card.host));
    if !opened {
        return Step::Calls(vec![call(
            n,
            "browser_navigate",
            json!({"url": card.start_url}),
        )]);
    }
    // 7. the fill, the user's own steps, the review hand-over, the confirmation
    let fills = results_of(transcript, "form_fill");
    let takeovers = results_of(transcript, "browser_request_takeover");
    let last_fill = fills.last().map(|(_, m)| pos(transcript, m));
    let last_back = takeovers
        .iter()
        .filter(|(_, m)| handed_back(m))
        .map(|(_, m)| pos(transcript, m))
        .max();
    let last_take = takeovers.last().map(|(_, m)| pos(transcript, m));
    let fill_after_back = matches!((last_fill, last_back), (Some(f), Some(b)) if f > b)
        || (last_fill.is_some() && last_back.is_none());
    if last_fill.is_none()
        || (!fill_after_back && fills.last().is_some_and(|(_, m)| stopped(m).is_some()))
    {
        // the first fill, or the fill again after the user's own step
        return Step::Calls(vec![call(n, "form_fill", trip.args())]);
    }
    let fill = fills.last().unwrap().1;
    let fill_json = json_of(fill);
    let fill_pos = last_fill.unwrap();
    let take_after_fill = last_take.is_some_and(|t| t > fill_pos);
    if !take_after_fill {
        let reason = match stopped(fill) {
            Some(why) => format!(
                "Please {}; hand the browser back when it is done and I will fill in the rest.",
                why.trim_end_matches('.')
            ),
            None => {
                let left: Vec<String> = fill_json
                    .get("skipped")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .map(|s| get_str(s, "field").replace('_', " "))
                    .filter(|f| !f.is_empty())
                    .collect();
                let assumed = assumptions(&trip);
                format!(
                    "The {} is filled in from your {}profile and passport (Vault). {}Review it{}, then {}; hand the browser back when the confirmation shows.",
                    card.name,
                    if texts.is_empty() { "" } else { "e-ticket, booking, " },
                    assumed,
                    if left.is_empty() { String::new() } else { format!(" and fill in {}", left.join(", ")) },
                    card.user_steps
                )
            }
        };
        return Step::Calls(vec![call(
            n,
            "browser_request_takeover",
            json!({"reason": reason}),
        )]);
    }
    let back_after_fill = last_back.is_some_and(|b| b > fill_pos);
    if !back_after_fill {
        // the takeover is still open (the loop only gets here once it came back)
        return Step::Final(String::new());
    }
    let snaps = results_of(transcript, "browser_snapshot");
    let snap_after_back = snaps
        .last()
        .is_some_and(|(_, m)| pos(transcript, m) > last_back.unwrap());
    if !snap_after_back {
        return Step::Calls(vec![call(
            n,
            "browser_snapshot",
            json!({"max_chars": 6000}),
        )]);
    }
    let page = content_of(snaps.last().unwrap().1);
    let reference = Regex::new(card.ref_re)
        .ok()
        .and_then(|r| r.find(&page).map(|m| m.as_str().to_string()))
        .or_else(|| crate::outcome::refs_in(&page).into_iter().next());
    Step::Final(answer(card, &trip, reference.as_deref(), w.zh))
}

fn assumptions(trip: &Trip) -> String {
    let mut parts = vec!["every health and declaration question No".to_string()];
    if trip.departure_date.is_empty() {
        if let Some(a) = trip.arrival() {
            parts.push(format!(
                "departure {} (arrival + 7 days)",
                (a + chrono::Duration::days(7)).format("%-d %B %Y")
            ));
        }
    }
    if trip.next_city.is_empty() && !trip.origin_city.is_empty() {
        parts.push(format!("next city {}", trip.origin_city));
    }
    format!(
        "I assumed {} — change any that are not right. ",
        parts.join(", ")
    )
}

/// The answer after the user's Submit.
pub fn answer(card: &CardSpec, trip: &Trip, reference: Option<&str>, zh: bool) -> String {
    let stay = if !trip.stay_name.is_empty() {
        trip.stay_name.clone()
    } else if trip.stay_kind == "own" {
        format!("your own address ({})", trip.stay_address)
    } else {
        trip.stay_address.clone()
    };
    let trip_line = format!(
        "{} arriving {}{}, purpose {}{}",
        trip.flight_no,
        trip.arrival_date,
        if trip.origin_city.is_empty() {
            String::new()
        } else {
            format!(" from {}", trip.origin_city)
        },
        trip.purpose,
        if stay.is_empty() {
            String::new()
        } else {
            format!(", staying at {stay}")
        }
    );
    match (reference, zh) {
        (Some(r), false) => format!(
            "Your {} was **submitted**: reference **{}**.\n\n- Trip: {}\n- Name, birthday, nationality and passport from your profile and Vault (the passport number is not shown here).\n\nAt the checkpoint show your passport; the card is on record.",
            card.name, r, trip_line
        ),
        (Some(r), true) => format!(
            "你的{}已提交：编号 **{}**。\n\n- 行程：{}\n- 姓名、生日、国籍和护照来自档案和保险箱（护照号不在这里显示）。\n\n过关时出示护照即可。",
            card.name, r, trip_line
        ),
        (None, false) => format!(
            "Your {} is filled in but **not confirmed yet**: no reference showed after the hand-back. Open the browser tab to check whether it was submitted, or ask me to try again.\n\n- Trip: {}",
            card.name, trip_line
        ),
        (None, true) => format!(
            "你的{}已填好，但**还没有确认**：交还后页面上没有编号。请到浏览器标签页确认是否已提交，或让我再试一次。\n\n- 行程：{}",
            card.name, trip_line
        ),
    }
}

/// The extraction call's instruction (the last resort when the regexes find no flight): JSON only.
pub const EXTRACT_SYSTEM: &str = "Read the booking emails and the user's words below and return JSON only, exactly these keys: \
{\"flight_no\": \"\", \"arrival_date\": \"YYYY-MM-DD\", \"origin_city\": \"\", \"stay_name\": \"\", \"stay_address\": \"\", \"purpose\": \"\"}. \
flight_no is the flight that ARRIVES in the destination country (airline code and number, e.g. \"SQ 231\"); arrival_date its arrival day; \
origin_city where it departs; stay_name the hotel or host, stay_address its address; purpose one of holiday, business, family, study, transit. \
Leave a value empty when the texts do not give it. Never invent.";

/// The facts the extraction call returned, as the transcript records them (an `extract_facts` tool exchange).
pub fn extract_exchange(n: usize, text: &str, facts: &Value) -> Vec<Value> {
    let id = format!("drv_{n}");
    let mut m = Map::new();
    for k in [
        "flight_no",
        "arrival_date",
        "origin_city",
        "stay_name",
        "stay_address",
        "purpose",
    ] {
        m.insert(k.into(), json!(get_str(facts, k)));
    }
    vec![
        json!({"role": "assistant", "content": "", "tool_calls": [{"id": id, "type": "function",
               "function": {"name": "extract_facts", "arguments": json!({"chars": text.chars().count()}).to_string()}}]}),
        json!({"role": "tool", "tool_call_id": id, "content": Value::Object(m).to_string()}),
    ]
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

    fn eticket() -> (String, String, String) {
        ("Singapore Airlines <noreply@singaporeairlines.example>".into(),
         "Your e-ticket: SQ 231 SYD-SIN 2026-11-03".into(),
         "E-ticket receipt. Booking ref 9QW3PL. Passenger: LU/JORDAN. SQ 231 departs Sydney (SYD) 2026-11-03 09:10, arrives Singapore (SIN) 2026-11-03 18:40. One way, economy, 30 kg baggage.".into())
    }

    fn hotel() -> (String, String, String) {
        ("Marina Bayfront Hotel <reservations@stay.example>".into(),
         "Booking confirmed: Marina Bayfront Hotel".into(),
         "Dear Jordan Lu, your reservation is confirmed. Confirmation HB-55120. Arrival 2026-11-03, check-in from 15:00. Marina Bayfront Hotel, 10 Bayfront Avenue, Singapore 018956. We look forward to welcoming you.".into())
    }

    #[test]
    fn the_trip_is_read_from_the_bookings_and_the_request() {
        let today = NaiveDate::from_ymd_opt(2026, 11, 1).unwrap();
        let t = extract(
            &[eticket(), hotel()],
            &[
                "Please fill in my SG Arrival Card for my trip to Singapore — it's a holiday."
                    .into(),
            ],
            today,
        );
        assert_eq!(t.flight_no, "SQ 231");
        assert_eq!(t.arrival_date, "2026-11-03");
        assert_eq!(t.origin_city, "Sydney");
        assert_eq!(t.origin_country, "Australia");
        assert_eq!(t.arrival_airport, "SIN");
        assert_eq!(t.airline, "Singapore Airlines");
        assert_eq!(t.stay_name, "Marina Bayfront Hotel");
        assert_eq!(t.stay_address, "10 Bayfront Avenue, Singapore 018956");
        assert_eq!(
            (t.stay_kind.as_str(), t.purpose.as_str()),
            ("hotel", "holiday")
        );
        // the user's own words, no mail: a long date and "staying at"
        let u = extract(&[], &["I arrive on TG 476 on 3 November 2026, staying at the Siam Riverside Hotel. Business trip.".into()], today);
        assert_eq!(
            (u.flight_no.as_str(), u.arrival_date.as_str()),
            ("TG 476", "2026-11-03")
        );
        assert_eq!(
            (u.stay_name.as_str(), u.purpose.as_str()),
            ("Siam Riverside Hotel", "business")
        );
        assert!(extract(&[], &["Fill in my card please".into()], today)
            .flight_no
            .is_empty());
    }

    #[test]
    fn the_own_address_rule() {
        let p = json!({"address_home": "12 Harbour St, Sydney NSW 2000, Australia", "address:Singapore flat": "10 Bayfront Ave, Singapore 018956",
                       "address:Parents in Singapore": "5 Jalan Kayu, Singapore 799999"});
        let own = own_addresses(&p, "Singapore");
        assert_eq!(own.len(), 2);
        assert_eq!(own[0].0, "Singapore flat");
        assert!(own_addresses(&p, "Malaysia").is_empty());
    }

    #[test]
    fn the_steps_follow_the_transcript() {
        let card = CARDS.iter().find(|c| c.skill == "sg-arrival-card").unwrap();
        let today = NaiveDate::from_ymd_opt(2026, 11, 1).unwrap();
        let user = vec![
            "Please fill in my SG Arrival Card for my trip to Singapore — it's a holiday."
                .to_string(),
        ];
        let profile = json!({"name_en": "JORDAN LU", "nationality": "Australia"});
        let w = World {
            user_texts: &user,
            profile: &profile,
            today,
            mail_ready: true,
            zh: false,
        };
        let mut t = step(
            "k1",
            "load_skill",
            json!({"name": "sg-arrival-card"}),
            "---\nname: sg-arrival-card\n---",
        );
        assert_eq!(active(&t).map(|c| c.skill), Some("sg-arrival-card"));
        // 1. the two searches
        let Step::Calls(c) = next(card, &t, &w) else {
            panic!()
        };
        assert_eq!(c.len(), 2);
        assert!(sget(&c[0].args, "query")
            .contains("(flight OR e-ticket OR itinerary) (Singapore OR SIN OR Changi)"));
        t.extend(step("drv_1", "gmail_search", c[0].args.clone(), "{\"count\": 1, \"messages\": [{\"id\": \"m1\", \"subject\": \"Your e-ticket: SQ 231\"}]}"));
        t.extend(step("drv_2", "gmail_search", c[1].args.clone(), "{\"count\": 1, \"messages\": [{\"id\": \"m2\", \"subject\": \"Booking confirmed: Marina Bayfront Hotel\"}]}"));
        // 2. the messages
        let Step::Calls(c) = next(card, &t, &w) else {
            panic!()
        };
        assert_eq!(
            c.iter()
                .map(|x| sget(&x.args, "message_id"))
                .collect::<Vec<_>>(),
            ["m1", "m2"]
        );
        let (f, s, b) = eticket();
        t.extend(step(
            "drv_3",
            "gmail_get_message",
            json!({"message_id": "m1"}),
            &json!({"from": f, "subject": s, "body": b}).to_string(),
        ));
        let (f, s, b) = hotel();
        t.extend(step(
            "drv_4",
            "gmail_get_message",
            json!({"message_id": "m2"}),
            &json!({"from": f, "subject": s, "body": b}).to_string(),
        ));
        // 3. inside the window: the official page
        let Step::Calls(c) = next(card, &t, &w) else {
            panic!()
        };
        assert_eq!(
            (c[0].name.as_str(), sget(&c[0].args, "url").as_str()),
            ("browser_navigate", card.start_url)
        );
        t.extend(step("drv_5", "browser_navigate", c[0].args.clone(), "<untrusted_content source=\"web page https://eservices.ica.gov.sg/arrivalcard/sgac\">{}</untrusted_content>"));
        // 4. form_fill with the facts
        let Step::Calls(c) = next(card, &t, &w) else {
            panic!()
        };
        assert_eq!(c[0].name, "form_fill");
        assert_eq!(sget(&c[0].args, "flight_no"), "SQ 231");
        assert_eq!(sget(&c[0].args, "stay_name"), "Marina Bayfront Hotel");
        t.extend(step("drv_6", "form_fill", c[0].args.clone(), "{\"url\": \"https://eservices.ica.gov.sg/arrivalcard/sgac\", \"filled\": [{\"op\": \"text\", \"field\": \"flight\", \"ok\": true}], \"skipped\": [{\"field\": \"phone\", \"why\": \"no value\"}]}"));
        // 5. the hand-over, naming the user's steps and what is left
        let Step::Calls(c) = next(card, &t, &w) else {
            panic!()
        };
        assert_eq!(c[0].name, "browser_request_takeover");
        let reason = sget(&c[0].args, "reason");
        assert!(
            reason.contains("tick the declaration and press Submit")
                && reason.contains("fill in phone")
                && reason.contains("arrival + 7 days"),
            "{reason}"
        );
        t.extend(step(
            "drv_7",
            "browser_request_takeover",
            c[0].args.clone(),
            "\"Takeover finished: the user handed browser control back.\"",
        ));
        // 6. the confirmation, then the answer with the reference
        let Step::Calls(c) = next(card, &t, &w) else {
            panic!()
        };
        assert_eq!(c[0].name, "browser_snapshot");
        t.extend(step("drv_8", "browser_snapshot", c[0].args.clone(), "<untrusted_content source=\"web page https://eservices.ica.gov.sg/sgac_done\">Submitted. DE Number: DE202611020123 Thank you.</untrusted_content>"));
        let Step::Final(a) = next(card, &t, &w) else {
            panic!()
        };
        assert!(
            a.contains("**DE202611020123**")
                && a.contains("SQ 231")
                && a.contains("Marina Bayfront Hotel"),
            "{a}"
        );
    }

    #[test]
    fn a_users_own_step_is_handed_over_then_the_fill_goes_on() {
        let card = CARDS.iter().find(|c| c.skill == "cn-arrival-card").unwrap();
        let today = NaiveDate::from_ymd_opt(2026, 11, 1).unwrap();
        let user = vec!["Fill in my China arrival card: MU 562 arriving Shanghai (PVG) 2026-11-02 from Sydney (SYD), staying at Bund Riverside Hotel, 99 Zhongshan East 1st Road. Holiday.".to_string()];
        let profile = json!({});
        let w = World {
            user_texts: &user,
            profile: &profile,
            today,
            mail_ready: false,
            zh: false,
        };
        let mut t = step(
            "k1",
            "load_skill",
            json!({"name": "cn-arrival-card"}),
            "---",
        );
        let Step::Calls(c) = next(card, &t, &w) else {
            panic!()
        };
        assert_eq!(c[0].name, "browser_navigate"); // no mail connected: straight to the page
        t.extend(step("drv_1", "browser_navigate", c[0].args.clone(), "<untrusted_content source=\"web page https://s.nia.gov.cn/ArrivalCardFillingPC/entry-registation-form\">{}</untrusted_content>"));
        let Step::Calls(c) = next(card, &t, &w) else {
            panic!()
        };
        assert_eq!(c[0].name, "form_fill");
        t.extend(step("drv_2", "form_fill", c[0].args.clone(), "{\"url\": \"https://s.nia.gov.cn/x\", \"filled\": [{\"op\": \"require\", \"field\": \"photo\", \"ok\": false, \"stop\": true, \"why\": \"the passport photo upload (step 1) is the user's: hand the page over for it, then call form_fill again\"}]}"));
        let Step::Calls(c) = next(card, &t, &w) else {
            panic!()
        };
        assert_eq!(c[0].name, "browser_request_takeover");
        assert!(sget(&c[0].args, "reason").contains("passport photo upload"));
        t.extend(step(
            "drv_3",
            "browser_request_takeover",
            c[0].args.clone(),
            "\"Takeover finished: the user handed browser control back.\"",
        ));
        let Step::Calls(c) = next(card, &t, &w) else {
            panic!()
        };
        assert_eq!(c[0].name, "form_fill"); // again, after the user's step
    }

    #[test]
    fn missing_facts_are_asked_once_and_an_early_trip_gets_a_reminder() {
        let card = CARDS.iter().find(|c| c.skill == "sg-arrival-card").unwrap();
        let today = NaiveDate::from_ymd_opt(2026, 11, 1).unwrap();
        let profile = json!({});
        let user = vec!["Please fill in my SG Arrival Card.".to_string()];
        let w = World {
            user_texts: &user,
            profile: &profile,
            today,
            mail_ready: false,
            zh: false,
        };
        let t = step(
            "k1",
            "load_skill",
            json!({"name": "sg-arrival-card"}),
            "---",
        );
        let Step::Final(q) = next(card, &t, &w) else {
            panic!()
        };
        assert!(
            q.contains(ASK_MARK)
                && q.contains("Where are you staying")
                && q.contains("holiday, business")
                && q.contains("Connect your mailbox"),
            "{q}"
        );
        // the user answers: on to the page (the chat's history carries the question)
        let answered = vec![
            "Please fill in my SG Arrival Card.".to_string(),
            "SQ 231 on 3 November 2026, staying at Marina Bayfront Hotel, holiday".to_string(),
        ];
        let w2 = World {
            user_texts: &answered,
            profile: &profile,
            today,
            mail_ready: false,
            zh: false,
        };
        let mut t2 = vec![
            json!({"role": "user", "content": "Please fill in my SG Arrival Card."}),
            json!({"role": "assistant", "content": q}),
        ];
        t2.extend(step(
            "k2",
            "load_skill",
            json!({"name": "sg-arrival-card"}),
            "---",
        ));
        let Step::Calls(c) = next(card, &t2, &w2) else {
            panic!()
        };
        assert_eq!(c[0].name, "browser_navigate");
        // too early: the reminder, then the answer
        let far = vec!["Fill in my SG Arrival Card: SQ 231 on 20 December 2026, staying at Marina Bayfront Hotel, holiday".to_string()];
        let w3 = World {
            user_texts: &far,
            profile: &profile,
            today,
            mail_ready: false,
            zh: false,
        };
        let mut t3 = step(
            "k1",
            "load_skill",
            json!({"name": "sg-arrival-card"}),
            "---",
        );
        let Step::Calls(c) = next(card, &t3, &w3) else {
            panic!()
        };
        assert_eq!(c[0].name, "schedule_create");
        assert_eq!(sget(&c[0].args, "spec"), "0 8 17 12 *");
        t3.extend(step(
            "drv_1",
            "schedule_create",
            c[0].args.clone(),
            "created",
        ));
        let Step::Final(a) = next(card, &t3, &w3) else {
            panic!()
        };
        assert!(
            a.contains("17 December 2026") && a.contains("Nothing is filed yet"),
            "{a}"
        );
    }
}
