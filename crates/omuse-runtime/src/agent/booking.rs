//! The code driver of the booking and checkout skills (`flight-booking`, `hotel-booking`, `restaurant-booking`,
//! `michelin-dining`, `online-shopping`): once the skill is loaded and the user's own words ask for the work, code
//! decides every step — the search parameters read from the request (regexes and the relative-date rules; one
//! extraction call only as the last resort, and one question before the search when a parameter is still missing),
//! ONE search call, the pick by rule from the structured result (cheapest, or the user's stated criteria), the choice
//! card with the pick shown, and — only when the user asked to book or buy — `purchase_confirm`, the booking link, the
//! page filled by a form function with FIXED input ids, the card from the Vault by the site's fixed labels, the
//! hand-over for the final Pay / Confirm / Place order (the user's), the reference read from the confirmation and the
//! answer.
//!
//! Every fill is code with fixed ids: a booking or checkout page is filled only when a form function for that site
//! exists here (the stand-in sites of the e2e suites: TravelTest's booking pages, Kaya Mobile, Stride Sports). On any
//! other site code opens the booking link and passes the page to the `browser-use` skill (`load_skill`), after which
//! [`active`] is `None` and the run continues on the model path for that page — no code guesses fields from labels.
//!
//! As in [`super::driver`], the next step is a function of the transcript alone ([`next`]), so a run suspended for a
//! takeover carries on where it was.

use std::sync::LazyLock;

use chrono::{Datelike, Duration, NaiveDate, Weekday};
use omuse_common::util::{get_str, take_chars, truthy};
use regex::Regex;
use serde_json::{json, Map, Value};

use super::claims::{money, pick_card, vault_cards, VaultCard};
use super::driver::{
    asked_before, dates_in, handed_back, json_of, pos, snapshot_text, user_asks, user_words, Step,
    World,
};
use super::util::{content_of, sget, stored_call, tool_calls};
use crate::forms::{self, Op};
use crate::llm::ToolCall;

/// The skill the machine hands an unknown booking or checkout page to (a model-driven fill of that one page).
pub const BROWSER_USE: &str = "browser-use";

/// What a spec drives.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Flight,
    Hotel,
    Restaurant,
    Michelin,
    Shop,
}

/// One booking skill: what its text used to tell the model, as data.
pub struct BookingSpec {
    pub skill: &'static str,
    pub kind: Kind,
    /// what is booked, in the answer
    pub what: &'static str,
    /// the search tool by its native name (a travel MCP connector's `mcp_<server>__<name>` is read too)
    pub search: &'static str,
    /// the data tool for the pick (a hotel's rooms, a restaurant's times); "" when none
    pub detail: &'static str,
    /// the final button — always the user's
    pub button: &'static str,
}

pub const SPECS: &[BookingSpec] = &[
    BookingSpec {
        skill: "flight-booking",
        kind: Kind::Flight,
        what: "flight",
        search: "flights_search",
        detail: "",
        button: "Pay now",
    },
    BookingSpec {
        skill: "hotel-booking",
        kind: Kind::Hotel,
        what: "hotel stay",
        search: "hotels_search",
        detail: "hotel_rooms",
        button: "Confirm booking",
    },
    BookingSpec {
        skill: "restaurant-booking",
        kind: Kind::Restaurant,
        what: "table",
        search: "places_search",
        detail: "",
        button: "Confirm booking",
    },
    BookingSpec {
        skill: "michelin-dining",
        kind: Kind::Michelin,
        what: "table",
        search: "michelin_search",
        detail: "restaurant_availability",
        button: "Confirm booking",
    },
    BookingSpec {
        skill: "online-shopping",
        kind: Kind::Shop,
        what: "order",
        search: "browser_read",
        detail: "",
        button: "Place order",
    },
];

// ---------------------------------------------------------------- the user's words

/// A request for the work: search, compare, book, buy, add to the cart — not a question about it.
static WORK: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\b(book|reserve|find|search|look(ing)? for|cheapest|best|compare|show me|options?|buy|order|purchase|get me|recommend|cart|a table|flights? (from|to)|hotels? (in|near))\b|订|预订|找|查|买|购物车")
        .unwrap()
});
/// A request to book or buy (the booking stage), not only the comparison.
static BOOK: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\b(book|reserve|buy|order|purchase|pay for|check ?out|go ahead)\b|帮我订|预订|订一|下单|买")
        .unwrap()
});
static CART: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)\bcart\b|购物车").unwrap());
/// The user wants to pick from the options themselves.
static CHOOSE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\b(options?|choose|let me pick|i'?ll pick|pick one|choices)\b|让我选|我来选")
        .unwrap()
});
/// The pick a choice card sends as the user's message.
static PICKED: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?im)^\s*(?:I choose|我选择?)[:：]\s*(.+?)\s*$").unwrap());
static URL: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"https?://[^\s"'<>)\]]+"#).unwrap());

/// The spec whose skill this run loaded last, when the user's own words ask for the work (`WORK`) — `None` once the
/// machine passed an unknown booking page to `browser-use` (a `load_skill` after the spec's), and for a shopping
/// request with no shop link in the user's words (the model path keeps it).
pub fn active(transcript: &[Value]) -> Option<&'static BookingSpec> {
    let loads: Vec<String> = transcript
        .iter()
        .flat_map(tool_calls)
        .filter_map(|tc| {
            let (name, args) = stored_call(tc);
            (name == "load_skill" && sget(&args, "file").trim().is_empty())
                .then(|| sget(&args, "name"))
        })
        .collect();
    let Some((i, spec)) = loads
        .iter()
        .enumerate()
        .filter_map(|(i, n)| SPECS.iter().find(|s| s.skill == n).map(|s| (i, s)))
        .next_back()
    else {
        return follow_up(transcript).or_else(|| known_shop_request(transcript));
    };
    if loads[i + 1..].iter().any(|n| n == BROWSER_USE) {
        return None;
    }
    let texts = user_words(transcript);
    if !user_asks(&texts, &WORK) {
        return None;
    }
    if spec.kind == Kind::Shop && shop_url(&texts.join("\n")).is_none() {
        return None;
    }
    Some(spec)
}

/// A follow-up message that accepts the next travel search the previous answer offered ("Shall I search the hotel
/// near Ginza now?" — "yes, the hotel please"): that search's spec. The offer is code's own text (`NEXT_MARK`) in the
/// last answer before the user's latest message; the reply must say yes or name the item, and not say no.
fn follow_up(transcript: &[Value]) -> Option<&'static BookingSpec> {
    static YES: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?i)\b(yes|yeah|yep|sure|ok(ay)?|please|go ahead|do it|search it)\b|好的?|是的|可以|请").unwrap()
    });
    static NO: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?i)\b(no|nope|not now|don'?t|later|skip)\b|不用|不要|算了").unwrap()
    });
    let last_user = transcript
        .iter()
        .rposition(|m| get_str(m, "role") == "user")?;
    let offer = transcript[..last_user]
        .iter()
        .rev()
        .find(|m| get_str(m, "role") == "assistant" && !content_of(m).trim().is_empty())
        .map(content_of)?;
    let at = offer.rfind(NEXT_MARK)?;
    let item = offer[at + NEXT_MARK.len()..].to_lowercase();
    let latest = user_words(&transcript[last_user..]).join("\n");
    let lc = latest.to_lowercase();
    let (kind, word) = if item.starts_with("hotel") {
        (Kind::Hotel, "hotel")
    } else if item.starts_with("flight") {
        (Kind::Flight, "flight")
    } else {
        return None; // a rental car: no machine for it
    };
    if NO.is_match(&latest) || !(YES.is_match(&latest) || lc.contains(word)) {
        return None;
    }
    SPECS.iter().find(|s| s.kind == kind)
}

/// How the user books in general ("I book through my travel agency"), not a request to book now.
static HABIT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\b(i|we)\s+(usually\s+|always\s+|normally\s+)?(book|order|buy)\s+(through|with|via|on|from|at)\b")
        .unwrap()
});

/// A request to mail the results ("email the options to mei@example.com"): the address.
fn mail_to(texts: &[String]) -> Option<String> {
    static MAIL_TO: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?i)\b(e-?mail|mail|send)\b[^.?!\n]*?([\w.+-]+@[\w-]+(?:\.[\w-]+)+)|发[^。？！\n]*?([\w.+-]+@[\w-]+(?:\.[\w-]+)+)")
            .unwrap()
    });
    texts.iter().rev().find_map(|t| {
        MAIL_TO
            .captures(t)
            .and_then(|c| c.get(2).or_else(|| c.get(3)))
            .map(|m| m.as_str().trim_end_matches('.').to_string())
    })
}

/// A shop with its own form functions here (Kaya Mobile, Stride Sports): its link in the user's words and a request
/// for the work runs the shopping machine from the start, skill or no skill (shopping_real 2026-11-01: the model
/// opened the results page itself and built the choice cards from the raw snapshot, twice).
fn known_shop_request(transcript: &[Value]) -> Option<&'static BookingSpec> {
    let texts = user_words(transcript);
    let url = shop_url(&texts.join("\n"))?;
    if !known_shop(&url) || !user_asks(&texts, &WORK) {
        return None;
    }
    // once the shop's page went to browser-use, the model has it
    let handed = transcript.iter().flat_map(tool_calls).any(|tc| {
        let (name, args) = stored_call(tc);
        name == "load_skill" && sget(&args, "name") == BROWSER_USE
    });
    (!handed)
        .then(|| SPECS.iter().find(|s| s.kind == Kind::Shop))
        .flatten()
}

/// A flight, hotel or MICHELIN-restaurant search in the user's latest message runs that machine from the start, skill
/// or no skill: the first of them the message names (one travel search per request; the others are offered next).
/// A rental car first, a plain restaurant, a question about a place, or a page already handed to browser-use: `None`
/// (travel_brave_real 2026-11-01: the model searched without the skill and wrote "couldn't verify", twice).
/// Only for a specialised bot's run (never Main, which hands travel to its bot), and only for a new travel search
/// ([`super::guards::new_travel_search`]) with its date in the words and no web address (a page the user gave is read
/// by the model); anything less stays with the model (gate 28: claims, card benefits and emergencies were taken).
pub fn search_request(transcript: &[Value]) -> Option<&'static BookingSpec> {
    static FLIGHT_W: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?i)\b(flights?|fly|flying|airfares?|plane tickets?)\b|机票|航班").unwrap()
    });
    static HOTEL_W: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?i)\b(hotels?|accommodation|place to stay|ryokan)\b|酒店|住宿").unwrap()
    });
    static MICHELIN_W: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"(?i)\bmichelin\b|米其林").unwrap());
    static CAR_W: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?i)\b(rental car|car rental|car hire|hire a car|rent a car)\b|租车").unwrap()
    });
    let last = transcript
        .iter()
        .rposition(|m| get_str(m, "role") == "user")?;
    let latest = user_words(&transcript[last..]);
    if !user_asks(&latest, &WORK) {
        return None;
    }
    let joined = latest.join("\n");
    if !super::guards::new_travel_search(&joined) || URL.is_match(&joined) || !has_date(&joined) {
        return None;
    }
    // a skill the model chose (another one than these machines') keeps the run
    if transcript
        .iter()
        .flat_map(tool_calls)
        .any(|tc| stored_call(tc).0 == "load_skill")
    {
        return None;
    }
    let text = latest.join("\n");
    // "I'm flying to Tokyo … find me a hotel": the trip's flight is context, the thing asked for is searched — the
    // words right after the request verb decide, else the first item named
    let mut named: Vec<(usize, Kind)> = [
        (&*MICHELIN_W, Kind::Michelin),
        (&*FLIGHT_W, Kind::Flight),
        (&*HOTEL_W, Kind::Hotel),
        (&*CAR_W, Kind::Restaurant), // a stand-in for "a car": no machine
    ]
    .into_iter()
    .flat_map(|(re, k)| {
        re.find_iter(&text)
            .map(move |m| (m.start(), k))
            .collect::<Vec<_>>()
    })
    .collect();
    named.sort_by_key(|(p, _)| *p);
    let first_asked = WORK
        .find_iter(&text)
        .find_map(|w| named.iter().find(|(p, _)| *p >= w.start()).copied())
        .or_else(|| named.first().copied())?;
    match first_asked.1 {
        Kind::Restaurant => None,
        k => SPECS.iter().find(|s| s.kind == k),
    }
}

/// A date in the words: ISO, "November 22", "22 Nov", "Dec 1", "next Friday", "tomorrow".
fn has_date(text: &str) -> bool {
    static DATE: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?i)\b\d{4}-\d{2}-\d{2}\b|\b(jan|feb|mar|apr|may|jun|jul|aug|sep|sept|oct|nov|dec)[a-z]*\.?\s+\d{1,2}\b|\b\d{1,2}\s+(jan|feb|mar|apr|may|jun|jul|aug|sep|sept|oct|nov|dec)[a-z]*\b|\b(today|tomorrow|tonight|next\s+(mon|tue|wed|thu|fri|sat|sun)[a-z]*|this\s+(weekend|mon|tue|wed|thu|fri|sat|sun)[a-z]*)\b|\d{1,2}月\d{1,2}日").unwrap()
    });
    DATE.is_match(text)
}

/// The shops the machine fills by fixed ids.
fn known_shop(url: &str) -> bool {
    let path = url::Url::parse(url)
        .map(|u| u.path().to_lowercase())
        .unwrap_or_default();
    host_of(url) == "shop.test" && (path.starts_with("/kaya_") || path.starts_with("/stride_"))
}

fn wants_booking(texts: &[String]) -> bool {
    let texts: Vec<String> = texts
        .iter()
        .map(|t| HABIT.replace_all(t, "").into_owned())
        .collect();
    user_asks(&texts, &BOOK)
}

/// The label the user picked on a choice card ("I choose: …"), the latest.
fn picked_label(texts: &[String]) -> Option<String> {
    texts
        .iter()
        .rev()
        .find_map(|t| PICKED.captures(t).map(|c| c[1].trim().to_string()))
}

fn shop_url(text: &str) -> Option<String> {
    URL.find(text)
        .map(|m| m.as_str().trim_end_matches(['.', ',', ';']).to_string())
}

// ---------------------------------------------------------------- cities

struct City {
    code: &'static str,
    name: &'static str,
    words: &'static [&'static str],
}

#[rustfmt::skip]
const CITIES: &[City] = &[
    City { code: "TYO", name: "Tokyo", words: &["tokyo", "东京", "東京", "haneda", "narita"] },
    City { code: "OSA", name: "Osaka", words: &["osaka", "大阪", "kansai"] },
    City { code: "SEL", name: "Seoul", words: &["seoul", "首尔", "首爾", "incheon"] },
    City { code: "SIN", name: "Singapore", words: &["singapore", "新加坡", "changi"] },
    City { code: "HKG", name: "Hong Kong", words: &["hong kong", "hongkong", "香港"] },
    City { code: "BKK", name: "Bangkok", words: &["bangkok", "曼谷"] },
    City { code: "TPE", name: "Taipei", words: &["taipei", "台北", "臺北"] },
    City { code: "SHA", name: "Shanghai", words: &["shanghai", "上海"] },
    City { code: "BJS", name: "Beijing", words: &["beijing", "北京"] },
    City { code: "CTU", name: "Chengdu", words: &["chengdu", "成都"] },
    City { code: "SZX", name: "Shenzhen", words: &["shenzhen", "深圳"] },
    City { code: "CAN", name: "Guangzhou", words: &["guangzhou", "广州"] },
    City { code: "KUL", name: "Kuala Lumpur", words: &["kuala lumpur", "吉隆坡"] },
    City { code: "DPS", name: "Bali", words: &["bali", "denpasar", "巴厘岛"] },
    City { code: "JKT", name: "Jakarta", words: &["jakarta", "雅加达"] },
    City { code: "MNL", name: "Manila", words: &["manila", "马尼拉"] },
    City { code: "HAN", name: "Hanoi", words: &["hanoi", "河内"] },
    City { code: "SGN", name: "Ho Chi Minh City", words: &["ho chi minh", "saigon", "胡志明"] },
    City { code: "FUK", name: "Fukuoka", words: &["fukuoka", "福冈"] },
    City { code: "CTS", name: "Sapporo", words: &["sapporo", "札幌"] },
    City { code: "SYD", name: "Sydney", words: &["sydney", "悉尼"] },
    City { code: "MEL", name: "Melbourne", words: &["melbourne", "墨尔本"] },
    City { code: "LON", name: "London", words: &["london", "伦敦"] },
    City { code: "PAR", name: "Paris", words: &["paris", "巴黎"] },
    City { code: "NYC", name: "New York", words: &["new york", "纽约"] },
    City { code: "QSF", name: "San Francisco", words: &["san francisco", "旧金山"] },
    City { code: "QLA", name: "Los Angeles", words: &["los angeles", "洛杉矶"] },
    City { code: "DXB", name: "Dubai", words: &["dubai", "迪拜"] },
];

/// Every city named in `text` with where it starts: its words (lower case, on word boundaries) and its code written
/// in capitals ("SIN to NRT" counts only for the city codes of the table).
fn cities_in(text: &str) -> Vec<(usize, &'static City)> {
    let lc = text.to_lowercase();
    let mut out = Vec::new();
    for c in CITIES {
        for w in c.words {
            let mut from = 0;
            while let Some(i) = lc[from..].find(w) {
                let at = from + i;
                let end = at + w.len();
                let before = lc[..at].chars().next_back();
                let after = lc[end..].chars().next();
                let ascii = w.is_ascii();
                if !ascii
                    || (!before.is_some_and(char::is_alphanumeric)
                        && !after.is_some_and(char::is_alphanumeric))
                {
                    out.push((at, c));
                }
                from = end;
            }
        }
        let code = Regex::new(&format!(r"\b{}\b", c.code)).unwrap();
        out.extend(code.find_iter(text).map(|m| (m.start(), c)));
    }
    out.sort_by_key(|(i, _)| *i);
    out.dedup_by(|a, b| a.1.code == b.1.code);
    out
}

/// The currency of a city of the table, by its code.
fn city_currency(code: &str) -> &'static str {
    match code {
        "TYO" | "OSA" | "FUK" | "CTS" => "JPY",
        "SEL" => "KRW",
        "SIN" => "SGD",
        "HKG" => "HKD",
        "BKK" => "THB",
        "TPE" => "TWD",
        "SHA" | "BJS" | "CTU" | "SZX" | "CAN" => "CNY",
        "KUL" => "MYR",
        "DPS" | "JKT" => "IDR",
        "MNL" => "PHP",
        "HAN" | "SGN" => "VND",
        "SYD" | "MEL" => "AUD",
        "LON" => "GBP",
        "PAR" => "EUR",
        "NYC" | "QSF" | "QLA" => "USD",
        "DXB" => "AED",
        _ => "",
    }
}

/// The user's home currency: the country of the profile's home address (a city of the table or a country's name),
/// else the currency where the flight starts (`origin`, a city code), else "".
pub fn home_currency(profile: &Value, origin: &str) -> String {
    static COUNTRY: &[(&str, &str)] = &[
        ("singapore", "SGD"),
        ("japan", "JPY"),
        ("malaysia", "MYR"),
        ("australia", "AUD"),
        ("united kingdom", "GBP"),
        ("england", "GBP"),
        ("united states", "USD"),
        ("usa", "USD"),
        ("china", "CNY"),
        ("korea", "KRW"),
        ("thailand", "THB"),
        ("taiwan", "TWD"),
        ("hong kong", "HKD"),
        ("indonesia", "IDR"),
        ("philippines", "PHP"),
        ("vietnam", "VND"),
        ("france", "EUR"),
        ("germany", "EUR"),
    ];
    let home = get_str(profile, "address_home");
    if let Some((_, c)) = cities_in(&home).first() {
        return city_currency(c.code).to_string();
    }
    let lc = home.to_lowercase();
    if let Some((_, cur)) = COUNTRY.iter().find(|(w, _)| has_word(&lc, w)) {
        return cur.to_string();
    }
    city_currency(origin).to_string()
}

/// The airline alliances named in the user's words, as the flight search takes them.
fn alliances_in(text: &str) -> Vec<String> {
    static ALLIANCE: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"(?i)\b(star alliance|oneworld|skyteam)\b").unwrap());
    ALLIANCE
        .captures_iter(text)
        .map(|c| match c[1].to_lowercase().as_str() {
            "star alliance" => "Star Alliance".to_string(),
            "oneworld" => "oneworld".to_string(),
            _ => "SkyTeam".to_string(),
        })
        .collect()
}

/// The area of a hotel or a restaurant in the user's words ("near Ginza", "around Shibuya", "in Orchard"): a
/// capitalised name that is not a city of the table, a month or a weekday.
fn area_in(text: &str) -> String {
    static AREA: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(
            r"\b(?:[Nn]ear|[Aa]round|[Ii]n|[Cc]lose to)\s+((?:[A-Z][\w'-]+)(?:\s+[A-Z][\w'-]+)?)",
        )
        .unwrap()
    });
    let city_word = |w: &str| {
        let lc = w.to_lowercase();
        CITIES.iter().any(|c| c.words.contains(&lc.as_str())) || CITIES.iter().any(|c| c.code == w)
    };
    let calendar_word = |w: &str| {
        let lc = w.to_lowercase();
        let lc = lc.trim_end_matches('.');
        [
            "january",
            "february",
            "march",
            "april",
            "may",
            "june",
            "july",
            "august",
            "september",
            "october",
            "november",
            "december",
            "jan",
            "feb",
            "mar",
            "apr",
            "jun",
            "jul",
            "aug",
            "sep",
            "sept",
            "oct",
            "nov",
            "dec",
        ]
        .contains(&lc)
            || WEEKDAYS.iter().any(|(d, _)| *d == lc || d[..3] == *lc)
    };
    AREA.captures_iter(text)
        .filter_map(|c| {
            let words: Vec<&str> = c[1]
                .split_whitespace()
                .take_while(|w| !city_word(w))
                .collect();
            (!words.is_empty() && !words.iter().any(|w| calendar_word(w))).then(|| words.join(" "))
        })
        .next()
        .unwrap_or_default()
}

// ---------------------------------------------------------------- dates

const WEEKDAYS: [(&str, Weekday); 7] = [
    ("monday", Weekday::Mon),
    ("tuesday", Weekday::Tue),
    ("wednesday", Weekday::Wed),
    ("thursday", Weekday::Thu),
    ("friday", Weekday::Fri),
    ("saturday", Weekday::Sat),
    ("sunday", Weekday::Sun),
];

fn month_no(m: &str) -> Option<u32> {
    let m = m.to_lowercase();
    let i = [
        "jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec",
    ]
    .iter()
    .position(|p| m.starts_with(p))?;
    Some(i as u32 + 1)
}

/// The day a month and day with no year means: this year's, or next year's when it has passed.
fn upcoming(month: u32, day: u32, today: NaiveDate) -> Option<NaiveDate> {
    let d = NaiveDate::from_ymd_opt(today.year(), month, day)?;
    if d < today {
        NaiveDate::from_ymd_opt(today.year() + 1, month, day)
    } else {
        Some(d)
    }
}

/// The coming `wd` ("next Friday", "on Friday"): this week's when it is still ahead, else a week on — the rule of
/// `guards::trip_dates_from_request`.
fn coming(wd: Weekday, today: NaiveDate) -> NaiveDate {
    let k = (wd.num_days_from_monday() as i64 - today.weekday().num_days_from_monday() as i64)
        .rem_euclid(7);
    today + Duration::days(if k == 0 { 7 } else { k })
}

/// Every date the user's words give, in order: ISO and long dates with a year (`driver::dates_in`), a day and month
/// without a year (the next one), and the relative days — today / tonight, tomorrow, the day after tomorrow, in N
/// days, next week (its Monday), this weekend (Saturday), (next / this / on) a weekday. `today` is the user's.
pub fn request_dates(text: &str, today: NaiveDate) -> Vec<NaiveDate> {
    static MONTH: &str = r"(jan(?:uary)?|feb(?:ruary)?|mar(?:ch)?|apr(?:il)?|may|june?|july?|aug(?:ust)?|sep(?:t(?:ember)?)?|oct(?:ober)?|nov(?:ember)?|dec(?:ember)?)";
    static DAY_MONTH: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(&format!(
            r"(?i)\b(\d{{1,2}})(?:st|nd|rd|th)?\s+(?:of\s+)?{MONTH}\b\.?(,?\s+\d{{4}})?"
        ))
        .unwrap()
    });
    static MONTH_DAY: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(&format!(
            r"(?i)\b{MONTH}\.?\s+(\d{{1,2}})(?:st|nd|rd|th)?\b(,?\s+\d{{4}})?"
        ))
        .unwrap()
    });
    static REL: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?i)\b(day after tomorrow|today|tonight|tomorrow|next week|this weekend|in (\d{1,2}) days|(?:next|this|on|coming)\s+(monday|tuesday|wednesday|thursday|friday|saturday|sunday))\b|后天|明天|今天|今晚|下周")
            .unwrap()
    });
    let mut out: Vec<(usize, NaiveDate)> = dates_in(text);
    for c in DAY_MONTH.captures_iter(text) {
        if c.get(3).is_some() {
            continue; // with a year: dates_in has it
        }
        if let (Ok(d), Some(m)) = (c[1].parse::<u32>(), month_no(&c[2])) {
            if let Some(date) = upcoming(m, d, today) {
                out.push((c.get(0).unwrap().start(), date));
            }
        }
    }
    for c in MONTH_DAY.captures_iter(text) {
        if c.get(3).is_some() {
            continue;
        }
        if let (Some(m), Ok(d)) = (month_no(&c[1]), c[2].parse::<u32>()) {
            if let Some(date) = upcoming(m, d, today) {
                out.push((c.get(0).unwrap().start(), date));
            }
        }
    }
    for c in REL.captures_iter(text) {
        let w = c[0].to_lowercase();
        let date = if w == "day after tomorrow" || w == "后天" {
            today + Duration::days(2)
        } else if w == "today" || w == "tonight" || w == "今天" || w == "今晚" {
            today
        } else if w == "tomorrow" || w == "明天" {
            today + Duration::days(1)
        } else if w == "next week" || w == "下周" {
            today + Duration::days(7 - today.weekday().num_days_from_monday() as i64)
        } else if w == "this weekend" {
            let k = (5 - today.weekday().num_days_from_monday() as i64).rem_euclid(7);
            today + Duration::days(k)
        } else if let Some(k) = c.get(2) {
            today + Duration::days(k.as_str().parse::<i64>().unwrap_or(0))
        } else if let Some(d) = c.get(3) {
            let d = d.as_str().to_lowercase();
            match WEEKDAYS.iter().find(|(n, _)| *n == d) {
                Some((_, wd)) => coming(*wd, today),
                None => continue,
            }
        } else {
            continue;
        };
        out.push((c.get(0).unwrap().start(), date));
    }
    out.sort_by_key(|(i, _)| *i);
    let mut dates: Vec<NaiveDate> = Vec::new();
    for (_, d) in out {
        if dates.last() != Some(&d) {
            dates.push(d);
        }
    }
    dates
}

fn iso(d: NaiveDate) -> String {
    d.format("%Y-%m-%d").to_string()
}

fn long(d: NaiveDate) -> String {
    d.format("%A %-d %B %Y").to_string()
}

// ---------------------------------------------------------------- the request

/// The search parameters read from the user's words (and, as the last resort, the extraction call's JSON).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Req {
    /// city or airport codes for a flight
    pub origin: String,
    pub destination: String,
    /// the city's name for a hotel or a restaurant
    pub city: String,
    pub date: Option<NaiveDate>,
    pub return_date: Option<NaiveDate>,
    pub check_out: Option<NaiveDate>,
    /// adults / guests / party size when the user gave it
    pub party: Option<u32>,
    pub cabin: &'static str,
    pub nonstop: bool,
    pub max_price: Option<f64>,
    pub min_stars: Option<u32>,
    pub min_rating: Option<f64>,
    pub free_cancel: bool,
    /// the cheapest at any condition (a non-refundable rate is fine)
    pub any_terms: bool,
    /// HH:MM
    pub time: String,
    /// Michelin: "3 stars" | "2 stars" | "1 star" | "stars" | "bib gourmand" | ""
    pub distinction: String,
    pub cuisine: String,
    /// dietary needs and the occasion, for a restaurant's requests field
    pub occasion: String,
    /// the user wants the best (highest distinction / rating) rather than the cheapest
    pub best: bool,
    /// the airlines the user prefers ("I prefer United", "Star Alliance", else the profile's standing
    /// `preferred_airline`): the flight search is limited to them
    pub airlines: Vec<String>,
    /// the last departure day of a range ("any day between 8 and 12 December"): ONE search over the days
    pub date_to: Option<NaiveDate>,
    /// the area of a hotel or a restaurant ("near Ginza")
    pub area: String,
    /// the user's home currency (the home address's country, else the flight's origin)
    pub currency: String,
}

fn word_no(w: &str) -> Option<u32> {
    w.parse::<u32>().ok().or_else(|| {
        [
            "one", "two", "three", "four", "five", "six", "seven", "eight", "nine", "ten",
        ]
        .iter()
        .position(|x| x.eq_ignore_ascii_case(w))
        .map(|i| i as u32 + 1)
    })
}

const CUISINES: &[&str] = &[
    "omakase",
    "sushi",
    "kaiseki",
    "tempura",
    "yakitori",
    "ramen",
    "soba",
    "dim sum",
    "cantonese",
    "shanghainese",
    "peranakan",
    "french",
    "italian",
    "spanish",
    "chinese",
    "japanese",
    "korean",
    "thai",
    "indian",
    "vietnamese",
    "steak",
    "seafood",
    "vegetarian",
];

/// The request's parameters, from the user's messages (oldest first), the profile (a flight's origin: the home city)
/// and the extraction call's facts when it ran.
/// The airlines named as a preference: "I prefer United", "fly with Singapore Airlines", "only on ANA or JAL".
fn preferred_airlines(text: &str) -> Vec<String> {
    static PREFER: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?i:\b(?:i\s+prefer|prefer(?:ably)?|fly(?:ing)?\s+with|only\s+(?:on|with))\s+)((?:[A-Z][\w&'-]*\s?){1,3}(?:(?:or|and|,)\s+(?:[A-Z][\w&'-]*\s?){1,3})*)")
            .unwrap()
    });
    static SPLIT: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"\s*(?:,|\bor\b|\band\b)\s*").unwrap());
    PREFER
        .captures_iter(text)
        .flat_map(|c| {
            SPLIT
                .split(c[1].trim())
                .map(|a| a.trim().trim_end_matches(['.', ',']).to_string())
                .filter(|a| !a.is_empty())
                .collect::<Vec<_>>()
        })
        .filter(|a| {
            !matches!(
                a.to_lowercase().as_str(),
                "i" | "economy" | "business" | "first"
            )
        })
        .collect()
}

pub fn parse_request(
    kind: Kind,
    texts: &[String],
    profile: &Value,
    today: NaiveDate,
    facts: Option<&Value>,
) -> Req {
    static PARTY: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?i)\b(\d{1,2}|one|two|three|four|five|six|seven|eight|nine|ten)\s+(?:adults?|people|persons|guests|travell?ers|passengers|pax|diners|of us)\b|\b(?:table|party|room|booking|reservation|dinner|lunch)\s+for\s+(\d{1,2}|one|two|three|four|five|six|seven|eight|nine|ten)\b|(\d{1,2})\s*(?:位|人|个人)")
            .unwrap()
    });
    static COUPLE: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?i)\b(my (wife|husband|partner|girlfriend|boyfriend) and (i|me)|the two of us|both of us|a couple)\b").unwrap()
    });
    static BUSINESS: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?i)\bbusiness[- ]class\b|\bc class\b|\blie-?flat\b|\bin business\b|商务舱")
            .unwrap()
    });
    static FIRST: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"(?i)\bfirst[- ]class\b|头等舱").unwrap());
    static PREMIUM: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"(?i)\bpremium economy\b|超级经济舱").unwrap());
    static NONSTOP: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"(?i)\b(direct|non-?stop)\b|直飞").unwrap());
    static MAX_PRICE: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?i)\b(?:under|below|less than|cheaper than|max(?:imum)?|up to|at most|no more than|within|budget(?: of| is)?)\s*(?:S\$|SGD|US\$|USD|HK\$|HKD|A\$|AUD|\$|¥|JPY|€|EUR|£|GBP|RM|MYR|฿|THB)?\s*(\d[\d,]*(?:\.\d+)?)")
            .unwrap()
    });
    static STARS: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?i)\b(\d|one|two|three|four|five)[- ]?(?:michelin[- ])?stars?\b|(\d)\s*星")
            .unwrap()
    });
    static STARRED: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"(?i)\b(michelin[- ])?starred\b|星级").unwrap());
    static BIB: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"(?i)\bbib( gourmand)?\b|必比登").unwrap());
    static RATING: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?i)\brat(?:ed|ing)\s*(?:of\s*)?(?:at least|above|over|>=?)?\s*(\d(?:\.\d)?)\b|\b(\d\.\d)\+?\s*(?:stars?\s*)?(?:or (?:more|higher|above|better))")
            .unwrap()
    });
    static FREE: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(
            r"(?i)\bfree cancell?ation\b|\b(?:fully )?refundable\b|\bcancel for free\b|免费取消",
        )
        .unwrap()
    });
    static ANY_TERMS: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?i)\bnon-?refundable (is )?(fine|ok|okay)\b|\bcheapest (rate )?(at any|whatever|regardless)\b|\bwhatever the (conditions|terms)\b")
            .unwrap()
    });
    static TIME: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?i)\b(\d{1,2}):(\d{2})\s*(am|pm)?\b|\b(\d{1,2})\s*(am|pm)\b").unwrap()
    });
    static MEAL: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"(?i)\b(dinner|lunch|brunch)\b|晚餐|午餐").unwrap());
    static OCCASION: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?i)\b(birthday|anniversary|vegetarian|vegan|halal|gluten[- ]free|no pork|no shellfish|(?:allergic|allergy) to [a-z ]{3,20}?(?:[.,;]|$))")
            .unwrap()
    });
    static BEST: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?i)\b(best|top|finest|highest[- ]rated|best[- ]rated)\b").unwrap()
    });
    static RETURN: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?i)\b(return(ing)?|back on|round[- ]trip|coming back)\b|返回|回程|往返")
            .unwrap()
    });
    static RANGE: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(
            r"(?i)\b(between|any ?day|any ?time|whichever day|flexible|sometime)\b|之间|任何一天",
        )
        .unwrap()
    });
    static NIGHTS: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?i)\b(\d{1,2}|one|two|three|four|five|six|seven|eight|nine|ten)[\s-]+nights?\b|(\d{1,2})\s*(?:晚|夜)").unwrap()
    });

    let text = texts.join("\n");
    let lc = text.to_lowercase();
    let mut r = Req {
        cabin: "economy",
        ..Req::default()
    };
    // the places
    let cities = cities_in(&text);
    match kind {
        Kind::Flight => {
            let mut origin: Option<&City> = None;
            let mut dest: Option<&City> = None;
            for (at, c) in &cities {
                let before = lc[..*at].trim_end();
                if before.ends_with("from") || before.ends_with("从") {
                    origin.get_or_insert(*c);
                } else if before.ends_with("to")
                    || before.ends_with("for")
                    || before.ends_with("到")
                    || before.ends_with("去")
                {
                    dest.get_or_insert(*c);
                }
            }
            let rest: Vec<&City> = cities
                .iter()
                .map(|(_, c)| *c)
                .filter(|c| {
                    origin.is_none_or(|o| o.code != c.code) && dest.is_none_or(|d| d.code != c.code)
                })
                .collect();
            let mut rest = rest.into_iter();
            if origin.is_none() && dest.is_none() {
                match (rest.next(), rest.next()) {
                    (Some(a), Some(b)) => {
                        origin = Some(a);
                        dest = Some(b);
                    }
                    (Some(a), None) => dest = Some(a),
                    _ => {}
                }
            } else if dest.is_none() {
                dest = rest.next();
            } else if origin.is_none() {
                origin = rest.next();
            }
            if origin.is_none() {
                // the user's home city (the profile's home address)
                let home = get_str(profile, "address_home");
                origin = cities_in(&home)
                    .first()
                    .map(|(_, c)| *c)
                    .filter(|c| dest.is_none_or(|d| d.code != c.code));
            }
            r.origin = origin.map(|c| c.code.to_string()).unwrap_or_default();
            r.destination = dest.map(|c| c.code.to_string()).unwrap_or_default();
        }
        _ => {
            // the city the stay or the table is in: the one after "in / near / to / at / around" (a flight and a
            // hotel in one message: the hotel is at the destination), else the first named
            let placed = cities.iter().find(|(at, _)| {
                let before = lc[..*at].trim_end();
                ["in", "near", "to", "at", "around", "在", "到", "去"]
                    .iter()
                    .any(|w| before.ends_with(w))
            });
            r.city = placed
                .or(cities.first())
                .map(|(_, c)| c.name.to_string())
                .unwrap_or_default();
            r.area = area_in(&text);
        }
    }
    // the dates
    let dates = request_dates(&text, today);
    r.date = dates.first().copied();
    match kind {
        Kind::Flight => {
            let later = dates
                .iter()
                .copied()
                .find(|d| r.date.is_some_and(|f| *d > f));
            if RETURN.is_match(&text) {
                r.return_date = later;
            } else if RANGE.is_match(&text) {
                r.date_to = later;
            }
        }
        Kind::Hotel => {
            r.check_out = dates
                .iter()
                .copied()
                .find(|d| r.date.is_some_and(|f| *d > f));
            if r.check_out.is_none() {
                let nights = NIGHTS.captures(&text).and_then(|c| {
                    c.get(1)
                        .or_else(|| c.get(2))
                        .and_then(|m| word_no(m.as_str()))
                });
                match (r.date, nights) {
                    (Some(d), Some(n)) if (1..=30).contains(&n) => {
                        r.check_out = Some(d + Duration::days(n as i64))
                    }
                    _ => {
                        if let Some(s) = super::guards::trip_dates_from_request(&text, today) {
                            let mut it = s.split(" to ").filter_map(|x| {
                                NaiveDate::parse_from_str(x.trim(), "%Y-%m-%d").ok()
                            });
                            r.date = it.next().or(r.date);
                            r.check_out = it.next();
                        }
                    }
                }
            }
        }
        _ => {}
    }
    // the party
    r.party = PARTY
        .captures(&text)
        .and_then(|c| {
            c.get(1)
                .or_else(|| c.get(2))
                .or_else(|| c.get(3))
                .and_then(|m| word_no(m.as_str()))
        })
        .filter(|n| (1..=20).contains(n))
        .or_else(|| COUPLE.is_match(&text).then_some(2));
    // the cabin, the stops, the price, the stars, the rating
    if kind == Kind::Flight {
        r.cabin = if FIRST.is_match(&text) {
            "first"
        } else if PREMIUM.is_match(&text) {
            "premium_economy"
        } else if BUSINESS.is_match(&text) {
            "business"
        } else {
            "economy"
        };
        r.nonstop = NONSTOP.is_match(&text);
        // the airlines and alliances in the user's words, else the profile's standing preference
        let mut airlines = preferred_airlines(&text);
        for a in alliances_in(&text) {
            if !airlines.iter().any(|x| x.eq_ignore_ascii_case(&a)) {
                airlines.push(a);
            }
        }
        if airlines.is_empty() {
            airlines = get_str(profile, "preferred_airline")
                .split([',', ';'])
                .map(|a| a.trim().to_string())
                .filter(|a| !a.is_empty())
                .collect();
        }
        r.airlines = airlines;
    }
    r.max_price = MAX_PRICE
        .captures(&text)
        .and_then(|c| c[1].replace(',', "").parse::<f64>().ok())
        .filter(|p| *p > 0.0);
    if let Some(c) = STARS.captures(&text) {
        let n = c
            .get(1)
            .or_else(|| c.get(2))
            .and_then(|m| word_no(m.as_str()));
        match kind {
            Kind::Hotel => r.min_stars = n.filter(|n| (2..=5).contains(n)),
            Kind::Michelin => {
                r.distinction = match n {
                    Some(1) => "1 star".into(),
                    Some(k) if (2..=3).contains(&k) => format!("{k} stars"),
                    _ => String::new(),
                }
            }
            _ => {}
        }
    }
    if kind == Kind::Michelin && r.distinction.is_empty() {
        if BIB.is_match(&text) {
            r.distinction = "bib gourmand".into();
        } else if STARRED.is_match(&text) {
            r.distinction = "stars".into();
        }
    }
    r.min_rating = RATING
        .captures(&text)
        .and_then(|c| c.get(1).or_else(|| c.get(2)))
        .and_then(|m| m.as_str().parse::<f64>().ok());
    r.free_cancel = FREE.is_match(&text);
    r.any_terms = ANY_TERMS.is_match(&text);
    r.best = BEST.is_match(&text);
    // the time, the cuisine, the occasion
    if let Some(c) = TIME.captures(&text) {
        let (h, m, ap) = match (c.get(1), c.get(2), c.get(4)) {
            (Some(h), Some(m), _) => (h.as_str(), m.as_str(), c.get(3)),
            (_, _, Some(h)) => (h.as_str(), "00", c.get(5)),
            _ => ("", "", None),
        };
        if let (Ok(mut h), Ok(m)) = (h.parse::<u32>(), m.parse::<u32>()) {
            match ap.map(|a| a.as_str().to_lowercase()) {
                Some(a) if a == "pm" && h < 12 => h += 12,
                Some(a) if a == "am" && h == 12 => h = 0,
                _ => {}
            }
            if h < 24 && m < 60 {
                r.time = format!("{h:02}:{m:02}");
            }
        }
    }
    if r.time.is_empty() && matches!(kind, Kind::Restaurant | Kind::Michelin) {
        r.time = match MEAL.captures(&text).map(|c| c[0].to_lowercase()) {
            Some(m) if m == "lunch" || m == "brunch" || m == "午餐" => "12:30".into(),
            _ => "19:00".into(),
        };
    }
    r.cuisine = CUISINES
        .iter()
        .find(|c| {
            Regex::new(&format!(r"(?i)\b{}\b", regex::escape(c)))
                .unwrap()
                .is_match(&text)
        })
        .map(|c| c.to_string())
        .unwrap_or_default();
    r.occasion = OCCASION
        .captures_iter(&text)
        .map(|c| c[1].trim_end_matches(['.', ',', ';']).trim().to_lowercase())
        .collect::<Vec<_>>()
        .join(", ");
    // the extraction call's facts fill what the regexes did not find
    if let Some(f) = facts {
        let d = |k: &str| NaiveDate::parse_from_str(get_str(f, k).trim(), "%Y-%m-%d").ok();
        let code = |k: &str| {
            let v = get_str(f, k).trim().to_string();
            cities_in(&v)
                .first()
                .map(|(_, c)| c.code.to_string())
                .unwrap_or_else(|| v.to_uppercase())
        };
        if r.origin.is_empty() && kind == Kind::Flight {
            r.origin = code("origin");
        }
        if r.destination.is_empty() && kind == Kind::Flight {
            r.destination = code("destination");
        }
        if r.city.is_empty() && kind != Kind::Flight {
            let v = get_str(f, "city").trim().to_string();
            r.city = cities_in(&v)
                .first()
                .map(|(_, c)| c.name.to_string())
                .unwrap_or(v);
        }
        if r.date.is_none() {
            r.date = d("date").or_else(|| d("check_in"));
        }
        if r.return_date.is_none() && kind == Kind::Flight {
            r.return_date = d("return_date");
        }
        if r.check_out.is_none() && kind == Kind::Hotel {
            r.check_out = d("check_out");
        }
        if r.party.is_none() {
            r.party = get_str(f, "guests")
                .trim()
                .parse::<u32>()
                .ok()
                .filter(|n| (1..=20).contains(n));
        }
        if r.cuisine.is_empty() {
            r.cuisine = get_str(f, "cuisine").trim().to_lowercase();
        }
    }
    r.currency = home_currency(profile, &r.origin);
    r
}

impl Req {
    /// What the search cannot run without.
    pub fn missing(&self, kind: Kind) -> Vec<&'static str> {
        let mut m = Vec::new();
        match kind {
            Kind::Flight => {
                if self.origin.is_empty() {
                    m.push("where you fly from");
                }
                if self.destination.is_empty() {
                    m.push("where you fly to");
                }
                if self.date.is_none() {
                    m.push("the departure date");
                }
            }
            Kind::Hotel => {
                if self.city.is_empty() {
                    m.push("the city (and area)");
                }
                if self.date.is_none() || self.check_out.is_none() {
                    m.push("the check-in and check-out dates (or the number of nights)");
                }
            }
            Kind::Michelin => {
                if self.city.is_empty() {
                    m.push("the city");
                }
                if self.date.is_none() {
                    m.push("the date");
                }
            }
            Kind::Restaurant => {
                if self.city.is_empty() {
                    m.push("the city (and area)");
                }
            }
            Kind::Shop => {}
        }
        m
    }

    fn party_or(&self, kind: Kind) -> u32 {
        self.party.unwrap_or(match kind {
            Kind::Restaurant | Kind::Michelin => 2,
            _ => 1,
        })
    }

    /// The search call's arguments.
    fn search_args(&self, kind: Kind) -> Value {
        match kind {
            Kind::Flight => {
                let mut a = json!({"origin": self.origin, "destination": self.destination,
                    "date": self.date.map(iso).unwrap_or_default(), "adults": self.party_or(kind),
                    "cabin": self.cabin, "stops": if self.nonstop { "nonstop" } else { "any" }, "sort": "price"});
                if let Some(d) = self.date_to {
                    a["date_to"] = json!(iso(d));
                }
                if let Some(d) = self.return_date {
                    a["return_date"] = json!(iso(d));
                }
                if let Some(p) = self.max_price {
                    a["max_price"] = json!(p);
                }
                if !self.currency.is_empty() {
                    a["currency"] = json!(self.currency);
                }
                if !self.airlines.is_empty() {
                    a["airlines"] = json!(self.airlines);
                }
                a
            }
            Kind::Hotel => {
                let mut a = json!({"city": self.city, "check_in": self.date.map(iso).unwrap_or_default(),
                    "check_out": self.check_out.map(iso).unwrap_or_default(), "guests": self.party_or(kind)});
                if !self.area.is_empty() {
                    a["area"] = json!(self.area);
                }
                if !self.currency.is_empty() {
                    a["currency"] = json!(self.currency);
                }
                if let Some(s) = self.min_stars {
                    a["min_stars"] = json!(s);
                }
                if let Some(p) = self.max_price {
                    a["max_price_per_night"] = json!(p);
                }
                if self.free_cancel {
                    a["free_cancellation"] = json!(true);
                }
                a
            }
            Kind::Michelin => {
                let mut a = json!({"city": self.city});
                if !self.distinction.is_empty() {
                    a["distinction"] = json!(self.distinction);
                }
                if !self.cuisine.is_empty() {
                    a["cuisine"] = json!(self.cuisine);
                }
                a
            }
            Kind::Restaurant => {
                let mut a = json!({"query": if self.cuisine.is_empty() { "restaurant".to_string() } else { format!("{} restaurant", self.cuisine) },
                                   "city": self.city});
                if !self.area.is_empty() {
                    a["area"] = json!(self.area);
                }
                a
            }
            Kind::Shop => json!({}),
        }
    }
}

// ---------------------------------------------------------------- the structured results

/// One option of a search (a flight, a hotel, a room, a restaurant, a place), as the result gives it.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Offer {
    pub label: String,
    /// hotel_id / restaurant_id / offer_id / place_id
    pub id: String,
    /// the comparison price (per adult, per night, per person's lower bound)
    pub price: Option<f64>,
    /// the amount charged for the whole booking, when the result gives it
    pub total: Option<f64>,
    pub price_text: String,
    pub currency: String,
    pub url: String,
    pub stops: Option<u64>,
    /// out of 5
    pub rating: Option<f64>,
    pub stars: Option<u64>,
    pub airline: String,
    pub area: String,
    pub cancellation: String,
    pub free_cancel: bool,
    pub payment: String,
    pub distinction: String,
    pub closed: String,
    pub reviews: Option<u64>,
    /// exact strings from the result for the choice card
    pub details: Vec<String>,
    /// a flight's airline sites to book on besides the booking link (Kiwi.com's `book_at`)
    pub book_at: Vec<String>,
    /// on the user's preferred airline or brand (the search flagged it)
    pub preferred: bool,
    /// whose data it is ("Kiwi.com")
    pub data: String,
    /// the day of a range search the offer flies
    pub date: String,
}

static NUM: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\d[\d,]*(?:\.\d+)?").unwrap());
static CUR: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"\b(SGD|USD|HKD|JPY|KRW|THB|TWD|CNY|MYR|EUR|GBP|AUD)\b|(S\$|HK\$|NT\$|US\$|A\$|฿|₩|€|£)",
    )
    .unwrap()
});

fn num_of(v: Option<&Value>) -> Option<f64> {
    match v? {
        Value::Number(n) => n.as_f64(),
        Value::String(s) => NUM
            .find(s)
            .and_then(|m| m.as_str().replace(',', "").parse().ok()),
        Value::Object(o) => num_of(o.get("extracted_lowest").or_else(|| o.get("lowest"))),
        _ => None,
    }
}

fn currency_in(s: &str) -> String {
    CUR.captures(s)
        .map(|c| {
            match c.get(1) {
                Some(m) => m.as_str(),
                None => match &c[2] {
                    "S$" => "SGD",
                    "HK$" => "HKD",
                    "NT$" => "TWD",
                    "US$" => "USD",
                    "A$" => "AUD",
                    "฿" => "THB",
                    "₩" => "KRW",
                    "€" => "EUR",
                    _ => "GBP",
                },
            }
            .to_string()
        })
        .unwrap_or_default()
}

fn first_str(o: &Value, keys: &[&str]) -> String {
    keys.iter()
        .map(|k| match o.get(*k) {
            Some(Value::String(s)) => s.trim().to_string(),
            Some(Value::Number(n)) => n.to_string(),
            _ => String::new(),
        })
        .find(|s| !s.is_empty())
        .unwrap_or_default()
}

fn first_num(o: &Value, keys: &[&str]) -> Option<f64> {
    keys.iter().find_map(|k| num_of(o.get(*k)))
}

/// The option list of a result: the first non-empty array of objects under the usual keys.
fn items_of(v: &Value) -> Vec<Value> {
    for k in [
        "offers",
        "hotels",
        "restaurants",
        "places",
        "rooms",
        "results",
    ] {
        if let Some(a) = v.get(k).and_then(Value::as_array) {
            let objs: Vec<Value> = a.iter().filter(|x| x.is_object()).cloned().collect();
            if !objs.is_empty() {
                return objs;
            }
        }
    }
    Vec::new()
}

/// One option read from a result object.
pub fn offer_of(kind: Kind, o: &Value, default_currency: &str) -> Offer {
    let label = match kind {
        Kind::Flight => {
            let t = first_str(o, &["title"]);
            if t.is_empty() {
                let a = first_str(o, &["airlines", "airline"]);
                let mut f = first_str(o, &["flight_no"]);
                if f.is_empty() {
                    // Kiwi.com and Google Flights: the flight numbers of the legs
                    let mut nos: Vec<String> = Vec::new();
                    for l in o
                        .get("legs")
                        .and_then(Value::as_array)
                        .into_iter()
                        .flatten()
                    {
                        let no = get_str(l, "flight_number").trim().to_string();
                        if !no.is_empty() && !nos.contains(&no) {
                            nos.push(no);
                        }
                    }
                    f = nos.join(", ");
                }
                format!("{a} {f}").trim().to_string()
            } else {
                t
            }
        }
        _ => first_str(o, &["name", "room_type", "title", "source"]),
    };
    let price = match kind {
        Kind::Flight => first_num(o, &["fare_amount", "price", "total_amount"]),
        Kind::Hotel => first_num(
            o,
            &[
                "price_per_night",
                "from_price_per_night",
                "rate_per_night",
                "total_amount",
                "total",
            ],
        ),
        Kind::Michelin => first_num(o, &["price_range"]),
        _ => first_num(o, &["price"]),
    };
    let total = first_num(o, &["total_amount", "fare_amount", "total", "from_total"])
        .or(if kind == Kind::Flight { price } else { None });
    let price_text = first_str(
        o,
        &[
            "fare",
            "price_per_night",
            "from_price_per_night",
            "price_range",
            "price",
            "total",
        ],
    );
    let mut currency = first_str(o, &["currency"]);
    if currency.is_empty() {
        currency = currency_in(&price_text);
    }
    if currency.is_empty() {
        currency = default_currency.to_string();
    }
    let rating_text = first_str(o, &["rating"]);
    let rating = num_of(o.get("rating")).map(|r| {
        if rating_text.contains("/10") || r > 5.0 {
            r / 2.0
        } else {
            r
        }
    });
    let cancellation = first_str(o, &["cancellation", "refund_rules"]);
    let free_cancel = truthy(o.get("free_cancellation").unwrap_or(&Value::Null))
        || cancellation.to_lowercase().starts_with("free")
        || cancellation.to_lowercase().contains("free cancellation");
    let detail_keys: &[&str] = match kind {
        Kind::Flight => &["schedule", "duration", "stops_text", "fare", "baggage"],
        Kind::Hotel => &[
            "area",
            "rating",
            "price_per_night",
            "from_price_per_night",
            "total",
            "from_total",
            "cancellation",
            "payment",
        ],
        Kind::Michelin => &["distinction", "cuisine", "price_range", "deposit_policy"],
        Kind::Restaurant => &["rating", "price", "address", "open_state"],
        Kind::Shop => &[],
    };
    // a bare figure gets its meaning and currency ("131" → "USD 131/night"); a text the result wrote stays as it is
    let labelled = |k: &str, v: String| -> String {
        let bare = v.trim().replace(',', "").parse::<f64>().is_ok();
        if !bare {
            return v;
        }
        let amt = |v: &str| format!("{} {}", currency, v.trim()).trim().to_string();
        match k {
            "rating" => format!("rating {v}"),
            "price_per_night" | "price" if kind == Kind::Hotel => format!("{}/night", amt(&v)),
            "from_price_per_night" => format!("from {}/night", amt(&v)),
            "total" => format!("{} total", amt(&v)),
            "from_total" => format!("from {} total", amt(&v)),
            _ => v,
        }
    };
    let mut details: Vec<String> = detail_keys
        .iter()
        .map(|k| (k, first_str(o, &[k])))
        .filter(|(_, s)| !s.is_empty())
        .map(|(k, s)| labelled(k, s))
        .collect();
    if kind == Kind::Flight && details.is_empty() {
        details = flight_details(o, &currency);
    }
    Offer {
        label,
        id: first_str(
            o,
            &[
                "hotel_id",
                "restaurant_id",
                "room_offer_id",
                "offer_id",
                "place_id",
            ],
        ),
        price,
        total,
        price_text,
        currency,
        url: first_str(o, &["booking_url", "link", "reserve_url", "website"]),
        stops: o.get("stops").and_then(Value::as_u64),
        rating,
        stars: o.get("stars").and_then(Value::as_u64),
        airline: first_str(o, &["airline", "airlines"]),
        area: first_str(o, &["area", "address"]),
        cancellation,
        free_cancel,
        payment: first_str(o, &["payment"]),
        distinction: first_str(o, &["distinction"]),
        closed: first_str(o, &["closed"]),
        reviews: o.get("reviews").and_then(Value::as_u64),
        details,
        book_at: o
            .get("book_at")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .map(str::to_string)
            .collect(),
        preferred: truthy(o.get("preferred").unwrap_or(&Value::Null)),
        data: first_str(o, &["data"]),
        date: first_str(o, &["date"]),
    }
}

/// A Kiwi.com or Google Flights offer's lines for the card and the answer (they carry no `schedule` / `fare`): the
/// times, the stops, the price, a mixed cabin and the baggage, as the result gives them.
fn flight_details(o: &Value, currency: &str) -> Vec<String> {
    let mut out = Vec::new();
    let dep = get_str(&o["departure"], "time");
    let arr = get_str(&o["arrival"], "time");
    if !dep.is_empty() && !arr.is_empty() {
        out.push(format!(
            "{} {dep} → {} {arr}",
            get_str(&o["departure"], "airport"),
            get_str(&o["arrival"], "airport")
        ));
    } else {
        let route = get_str(o, "route");
        if !route.is_empty() {
            out.push(route);
        }
    }
    match o.get("stops").and_then(Value::as_u64) {
        Some(0) => out.push("direct".into()),
        Some(k) => {
            let via: Vec<String> = o
                .get("layovers")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .map(|l| l.split_whitespace().next().unwrap_or(l).to_string())
                .collect();
            out.push(format!(
                "{k} stop{}{}",
                if k == 1 { "" } else { "s" },
                if via.is_empty() {
                    String::new()
                } else {
                    format!(" via {}", via.join(", "))
                }
            ))
        }
        None => {}
    }
    if let Some(p) = num_of(o.get("price")) {
        out.push(
            format!("{currency} {}", money(p).trim_end_matches(".00"))
                .trim()
                .to_string(),
        );
    }
    let cabin = get_str(o, "cabin");
    if cabin.contains("not the cabin searched") {
        out.push(cabin);
    }
    let mix = get_str(o, "cabin_mix");
    if !mix.is_empty() {
        out.push(format!("cabins: {mix}"));
    }
    if let Some(bags) = o["baggage"].get("checkedBag").and_then(Value::as_u64) {
        out.push(format!(
            "{bags} checked bag{}",
            if bags == 1 { "" } else { "s" }
        ));
    }
    let date = get_str(o, "date");
    if !date.is_empty() && dep.is_empty() {
        out.push(date);
    }
    out
}

pub fn offers_in(kind: Kind, result: &Value) -> Vec<Offer> {
    let cur = get_str(result, "currency");
    items_of(result)
        .iter()
        .map(|o| offer_of(kind, o, &cur))
        .filter(|o| !o.label.is_empty())
        // a flight or a stay without a price is a web page about it (a search's web fallback), not an offer
        .filter(|o| {
            !matches!(kind, Kind::Flight | Kind::Hotel) || o.price.is_some() || o.total.is_some()
        })
        .collect()
}

// ---------------------------------------------------------------- the pick, by rule

const GENERIC: &[&str] = &[
    "hotel",
    "hotels",
    "grand",
    "resort",
    "suites",
    "suite",
    "house",
    "restaurant",
    "airlines",
    "airways",
    "station",
    "tower",
    "central",
    "city",
    "garden",
    "palace",
    "plaza",
    "park",
    "royal",
    "kitchen",
    "dining",
    "room",
    "rooms",
    "double",
    "twin",
    "standard",
    "deluxe",
    "superior",
    "with",
    "breakfast",
];

fn has_word(hay_lc: &str, w: &str) -> bool {
    Regex::new(&format!(r"\b{}\b", regex::escape(&w.to_lowercase())))
        .map(|r| r.is_match(hay_lc))
        .unwrap_or(false)
}

/// Whether the user's words name this option: its whole name, its airline (or the airline's code as a capital word),
/// or a distinctive word of its name or area (never a city, never a generic word like "Hotel").
fn mentioned(kind: Kind, o: &Offer, text: &str) -> bool {
    let lc = text.to_lowercase();
    if !o.label.is_empty() && lc.contains(&o.label.to_lowercase()) {
        return true;
    }
    if kind == Kind::Flight {
        if !o.airline.is_empty() && lc.contains(&o.airline.to_lowercase()) {
            return true;
        }
        static CODE: LazyLock<Regex> =
            LazyLock::new(|| Regex::new(r"\b([A-Z][A-Z0-9])\s?\d{2,4}\b").unwrap());
        return CODE.captures(&o.label).is_some_and(|c| {
            Regex::new(&format!(r"\b{}\b", &c[1]))
                .unwrap()
                .is_match(text)
        });
    }
    let city_word = |w: &str| {
        let wl = w.to_lowercase();
        CITIES.iter().any(|c| c.words.contains(&wl.as_str()))
    };
    format!("{} {}", o.label, o.area)
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| {
            let wl = w.to_lowercase();
            w.chars().count() >= 4
                && !GENERIC.contains(&wl.as_str())
                && !CUISINES.contains(&wl.as_str())
                && !city_word(w)
        })
        .any(|w| has_word(&lc, w))
}

/// The options that meet the user's criteria, best first, with the rule that ordered them (the first is the pick).
pub fn ranked(kind: Kind, req: &Req, offers: &[Offer], text: &str) -> (Vec<Offer>, String) {
    let mut keep: Vec<Offer> = offers.to_vec();
    let mut rule: Vec<String> = Vec::new();
    let named: Vec<Offer> = keep
        .iter()
        .filter(|o| mentioned(kind, o, text))
        .cloned()
        .collect();
    if !named.is_empty() {
        keep = named;
        rule.push("the one you named".into());
    }
    match kind {
        Kind::Flight => {
            if req.nonstop {
                keep.retain(|o| o.stops == Some(0));
                rule.push("direct".into());
            }
            keep.retain(|o| {
                !o.details
                    .iter()
                    .any(|d| d.contains("not the cabin searched"))
            });
        }
        Kind::Hotel => {
            if let Some(s) = req.min_stars {
                keep.retain(|o| o.stars.is_none_or(|x| x >= s as u64));
                rule.push(format!("{s} stars or more"));
            }
            if req.free_cancel {
                let free: Vec<Offer> = keep.iter().filter(|o| o.free_cancel).cloned().collect();
                if !free.is_empty() {
                    keep = free;
                }
                rule.push("free cancellation".into());
            }
        }
        Kind::Michelin => {
            if !req.distinction.is_empty() {
                let d = req.distinction.to_lowercase();
                keep.retain(|o| {
                    let od = o.distinction.to_lowercase();
                    match d.as_str() {
                        "stars" => od.contains("star") && !od.contains("bib"),
                        "bib gourmand" => od.contains("bib"),
                        _ => od.starts_with(&d[..1]) && !od.contains("bib"),
                    }
                });
                rule.push(req.distinction.clone());
            }
            if let Some(date) = req.date {
                let day = date.format("%A").to_string().to_lowercase();
                keep.retain(|o| !o.closed.to_lowercase().contains(&day));
                rule.push(format!("open on {}", date.format("%A")));
            }
        }
        Kind::Restaurant => {
            let min = req.min_rating.unwrap_or(4.3);
            let good: Vec<Offer> = keep
                .iter()
                .filter(|o| o.rating.is_some_and(|r| r >= min))
                .cloned()
                .collect();
            if !good.is_empty() {
                keep = good;
            }
            rule.push(format!("rated {min} or more"));
        }
        Kind::Shop => {}
    }
    if let Some(m) = req.max_price {
        keep.retain(|o| o.price.is_some_and(|p| p <= m));
        rule.push(format!("under {}", money(m).trim_end_matches(".00")));
    }
    if let (Some(m), true) = (req.min_rating, kind != Kind::Restaurant) {
        let m5 = if m > 5.0 { m / 2.0 } else { m };
        keep.retain(|o| o.rating.is_none_or(|r| r >= m5));
        rule.push(format!("rated {m} or more"));
    }
    let price = |o: &Offer| o.price.unwrap_or(f64::MAX);
    let dist_rank = |o: &Offer| {
        let d = o.distinction.to_lowercase();
        if d.contains("bib") {
            0
        } else {
            d.chars().next().and_then(|c| c.to_digit(10)).unwrap_or(0)
        }
    };
    match kind {
        Kind::Restaurant => {
            keep.sort_by(|a, b| {
                b.rating
                    .unwrap_or(0.0)
                    .total_cmp(&a.rating.unwrap_or(0.0))
                    .then(b.reviews.unwrap_or(0).cmp(&a.reviews.unwrap_or(0)))
            });
            rule.push("the best-rated".into());
        }
        Kind::Michelin if req.best => {
            keep.sort_by(|a, b| {
                dist_rank(b)
                    .cmp(&dist_rank(a))
                    .then(price(a).total_cmp(&price(b)))
            });
            rule.push("the highest distinction".into());
        }
        _ => {
            keep.sort_by(|a, b| price(a).total_cmp(&price(b)));
            rule.push("the cheapest".into());
        }
    }
    (keep, rule.join(", "))
}

/// The option the user picked on a choice card, by its label.
fn by_label<'a>(offers: &'a [Offer], label: &str) -> Option<&'a Offer> {
    let l = label.to_lowercase();
    offers
        .iter()
        .find(|o| o.label.to_lowercase() == l)
        .or_else(|| {
            offers.iter().find(|o| {
                let ol = o.label.to_lowercase();
                l.contains(&ol) || ol.contains(&l)
            })
        })
}

/// The room to book: the cheapest with free cancellation (the skill's preference), unless the user wants the cheapest
/// at any condition; the cheapest when none is free to cancel.
pub fn pick_room(rooms: &[Offer], req: &Req) -> Option<(usize, &'static str)> {
    let cheapest = |it: &mut dyn Iterator<Item = (usize, &Offer)>| {
        it.min_by(|a, b| {
            a.1.total
                .or(a.1.price)
                .unwrap_or(f64::MAX)
                .total_cmp(&b.1.total.or(b.1.price).unwrap_or(f64::MAX))
        })
        .map(|(i, _)| i)
    };
    if !req.any_terms {
        if let Some(i) = cheapest(&mut rooms.iter().enumerate().filter(|(_, r)| r.free_cancel)) {
            return Some((i, "the cheapest room with free cancellation"));
        }
    }
    cheapest(&mut rooms.iter().enumerate()).map(|i| (i, "the cheapest room"))
}

/// The slot nearest the wanted time (HH:MM).
fn nearest_slot(slots: &[(String, String)], want: &str) -> Option<usize> {
    let mins = |t: &str| -> i64 {
        let mut p = t.split(':');
        let h: i64 = p.next().and_then(|x| x.trim().parse().ok()).unwrap_or(0);
        let m: i64 = p.next().and_then(|x| x.trim().parse().ok()).unwrap_or(0);
        h * 60 + m
    };
    let w = mins(want);
    slots
        .iter()
        .enumerate()
        .min_by_key(|(_, (t, _))| (mins(t) - w).abs())
        .map(|(i, _)| i)
}

// ---------------------------------------------------------------- the transcript, read back

fn is_tool(name: &str, base: &str) -> bool {
    name == base
        || name
            .strip_suffix(base)
            .is_some_and(|p| p.starts_with("mcp_") && p.ends_with("__"))
}

/// (args, the tool message) of every call of `base` (or a travel MCP connector's same tool), in order.
fn calls_of<'a>(transcript: &'a [Value], base: &str) -> Vec<(Value, &'a Value)> {
    let mut out = Vec::new();
    for tc in transcript.iter().flat_map(tool_calls) {
        let (name, args) = stored_call(tc);
        if !is_tool(&name, base) {
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

/// The tool name of the call a tool message answers.
fn search_name(transcript: &[Value], m: &Value) -> String {
    let id = get_str(m, "tool_call_id");
    transcript
        .iter()
        .flat_map(tool_calls)
        .find(|tc| get_str(tc, "id") == id)
        .map(|tc| stored_call(tc).0)
        .unwrap_or_default()
}

fn last_after<'a>(transcript: &'a [Value], base: &str, after: usize) -> Option<(Value, &'a Value)> {
    calls_of(transcript, base)
        .into_iter()
        .rfind(|(_, m)| pos(transcript, m) > after)
}

fn last_user_pos(transcript: &[Value]) -> usize {
    transcript
        .iter()
        .rposition(|m| get_str(m, "role") == "user")
        .unwrap_or(0)
}

/// The tool to call for `base`: the native one when the catalog has it, else a travel MCP connector's
/// (`mcp_<server>__<base>`), else the native name (`michelin_search` and `restaurant_availability` only exist on a
/// connector: `mcp_travel__…` by default).
pub fn tool_for(base: &str, available: &[String], connector: bool) -> String {
    let mcp = available.iter().find(|n| *n != base && is_tool(n, base));
    if let (true, Some(n)) = (connector, mcp) {
        return n.clone();
    }
    if available.iter().any(|n| n == base) {
        return base.into();
    }
    if let Some(n) = mcp {
        return n.clone();
    }
    match base {
        "michelin_search" | "restaurant_availability" => format!("mcp_travel__{base}"),
        _ => base.into(),
    }
}

/// The user's words name their travel connector (agency) as the place to search and book.
static CONNECTOR: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\b(connector|travel agency|my agency|travel agent)\b|旅行社|代理商|连接器")
        .unwrap()
});

/// A tool result's data: the `content` of the untrusted envelope (`{"trust", "source", …, "content"}`) a search or
/// data tool's result arrives in, or the JSON as it is.
fn data_of(m: &Value) -> Value {
    let v = json_of(m);
    match v.get("content") {
        Some(c @ Value::Object(_)) if v.get("trust").is_some() => c.clone(),
        Some(Value::String(s)) if v.get("trust").is_some() => serde_json::from_str(s).unwrap_or(v),
        _ => v,
    }
}

/// The text of the pages a `browser_read` result carries (`{"trust", "source", "pages": [{"url", "text"}]}`), or the
/// message as it is.
fn pages_text(m: &Value) -> String {
    let texts: Vec<String> = json_of(m)
        .get("pages")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .map(|p| get_str(p, "text"))
        .filter(|t| !t.is_empty())
        .collect();
    if texts.is_empty() {
        content_of(m)
    } else {
        texts.join("\n\n")
    }
}

/// A search engine's ad or redirect link (DuckDuckGo's `y.js?ad_domain=…`, Bing's `aclick`, Google's `aclk`): no page
/// about the trip, and it often does not open (travel_brave_real 2026-11-01).
fn ad_link(url: &str) -> bool {
    let host = host_of(url);
    let lc = url.to_lowercase();
    (host.ends_with("duckduckgo.com") && (lc.contains("/y.js") || lc.contains("ad_domain")))
        || (host.ends_with("bing.com") && lc.contains("/aclick"))
        || (host.ends_with("google.com") && lc.contains("/aclk"))
        || lc.contains("ad_provider=")
}

/// A web fallback's results (`{"results": [{"title", "url", "snippet"}]}`) as answer lines, the first four.
fn web_pages(result: &Value) -> Vec<String> {
    static PRICE: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?:S\$|SGD|US\$|USD|HK\$|A\$|¥|JPY|€|EUR|£|GBP|\$)\s?\d[\d,]*(?:\.\d+)?")
            .unwrap()
    });
    // the text of the pages the fallback read, by url
    let page_text = |url: &str| {
        result
            .get("pages")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .find(|p| get_str(p, "url") == url)
            .map(|p| get_str(p, "text"))
            .unwrap_or_default()
    };
    result
        .get("results")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|r| get_str(r, "url").starts_with("http") && !ad_link(&get_str(r, "url")))
        .take(4)
        .map(|r| {
            let url = get_str(r, "url");
            let snippet = take_chars(get_str(r, "snippet").trim(), 160).to_string();
            // a price as the page or its snippet shows it ("from" prices, not quotes)
            let price = if PRICE.is_match(&snippet) {
                String::new()
            } else {
                PRICE
                    .find(&page_text(&url))
                    .map(|m| format!(" (the page shows prices from {})", m.as_str()))
                    .unwrap_or_default()
            };
            format!(
                "- [{}]({url}){}{price}",
                get_str(r, "title"),
                if snippet.is_empty() {
                    String::new()
                } else {
                    format!(" — {snippet}")
                }
            )
        })
        .collect()
}

fn denied(m: &Value) -> bool {
    let c = content_of(m);
    c.starts_with("DENIED") || c.to_lowercase().contains("\"status\": \"denied\"")
}

fn step_count(transcript: &[Value]) -> usize {
    transcript
        .iter()
        .flat_map(tool_calls)
        .filter(|tc| get_str(tc, "id").starts_with("bkg_"))
        .count()
        + 1
}

fn call(n: usize, name: &str, args: Value) -> ToolCall {
    ToolCall {
        id: format!("bkg_{n}"),
        name: name.into(),
        args,
    }
}

fn calls(n: usize, name: &str, args: Value) -> Step {
    Step::Calls(vec![call(n, name, args)])
}

/// The ref of the textbox whose accessible name starts with `label` in a snapshot (a known site's fixed label).
fn ref_of(snapshot: &str, label: &str) -> Option<String> {
    let r = Regex::new(&format!(r#"\[(e\d+)\] textbox "{}"#, regex::escape(label))).ok()?;
    r.captures(snapshot).map(|c| c[1].to_string())
}

/// The page URL a navigate landed on (the envelope's `url`), else the URL asked for.
fn landed(args: &Value, m: &Value) -> String {
    let u = get_str(&json_of(m), "url");
    if u.is_empty() {
        sget(args, "url")
    } else {
        u
    }
}

fn host_of(u: &str) -> String {
    url::Url::parse(u)
        .ok()
        .and_then(|x| x.host_str().map(str::to_lowercase))
        .unwrap_or_default()
}

fn origin_of(u: &str) -> String {
    url::Url::parse(u)
        .ok()
        .map(|x| x.origin().ascii_serialization())
        .unwrap_or_default()
}

// ---------------------------------------------------------------- the known sites: form functions with fixed ids

/// The values a known site's form function gets.
pub struct Guest<'a> {
    pub profile: &'a Value,
    /// dietary needs / the occasion for a restaurant's requests field
    pub requests: &'a str,
}

impl Guest<'_> {
    fn p(&self, k: &str) -> String {
        get_str(self.profile, k).trim().to_string()
    }
    pub fn name(&self) -> String {
        let n = self.p("name_en");
        if n.is_empty() {
            self.p("name")
        } else {
            n
        }
    }
    pub fn email(&self) -> String {
        let e = self.p("email_personal");
        if e.is_empty() {
            self.p("email")
        } else {
            e
        }
    }
    pub fn phone(&self) -> String {
        self.p("phone")
    }
    pub fn address(&self) -> String {
        self.p("address_home")
    }
}

/// The profile with the contact details the user gave in the chat ("I'm Lucas Lu, lucas@example.com, +65 8000 0000")
/// where the profile has none: name, email, phone (the latest message that names one).
pub fn profile_with_words(profile: &Value, texts: &[String]) -> Value {
    static EMAIL: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"[\w.+-]+@[\w-]+(?:\.[\w-]+)+").unwrap());
    static PHONE: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"\+\d{1,3}[\s-]?\d[\d\s-]{5,14}\d").unwrap());
    static NAME: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?:\bI'?m|\bI am|\bmy name is|(?i:\bname)\s*:)\s+([A-Z][a-z]+(?:\s+[A-Z][a-z]+){1,3})|(?:^|[.!?]\s+|\n)([A-Z][a-z]+(?:\s+[A-Z][a-z]+){1,3}),\s*[\w.+-]+@")
            .unwrap()
    });
    let mut p = profile.clone();
    if !p.is_object() {
        p = json!({});
    }
    let mut fill = |keys: &[&str], key: &str, found: Option<String>| {
        if keys.iter().all(|k| get_str(&p, k).trim().is_empty()) {
            if let Some(v) = found {
                p[key] = json!(v);
            }
        }
    };
    let latest = |re: &Regex, group: usize| {
        texts.iter().rev().find_map(|t| {
            re.captures(t)
                .and_then(|c| c.get(group))
                .map(|m| m.as_str().trim().trim_end_matches('.').to_string())
        })
    };
    // the address the results are mailed to ("email the options to mei@…") is not the user's own
    let mailed = mail_to(texts);
    let own_email = texts.iter().rev().find_map(|t| {
        EMAIL
            .find_iter(t)
            .map(|m| m.as_str().trim_end_matches('.').to_string())
            .find(|e| Some(e) != mailed.as_ref())
    });
    let name = texts.iter().rev().find_map(|t| {
        NAME.captures(t)
            .and_then(|c| c.get(1).or_else(|| c.get(2)))
            .map(|m| m.as_str().trim().to_string())
    });
    fill(&["name_en", "name"], "name_en", name);
    fill(&["email_personal", "email"], "email_personal", own_email);
    fill(&["phone"], "phone", latest(&PHONE, 0));
    p
}

type SiteFn = fn(&Guest) -> Vec<Op>;

/// A booking or checkout page code fills: its host and path, the plan with fixed ids, the card boxes by their fixed
/// labels (Vault field, label), the confirmation's reference.
pub struct Site {
    pub form: &'static str,
    pub host: &'static str,
    pub path: &'static str,
    pub fill: SiteFn,
    pub card: &'static [(&'static str, &'static str)],
    pub ref_re: &'static str,
}

const TRAVELTEST_CARD: &[(&str, &str)] = &[
    ("number", "Card number"),
    ("expiry", "Expiry (MM/YY)"),
    ("cvc", "CVC"),
    ("holder", "Name on card"),
];

/// TravelTest's flight booking page (`/book/flight/<offer_id>`): the traveller as in the passport, contact, the
/// passport number from the Vault (Sentinel puts it in). The passport expiry is optional and stays empty; the card is
/// filled by `browser_fill_secret` after `purchase_confirm`; "Pay now" is the user's.
fn traveltest_flight(g: &Guest) -> Vec<Op> {
    vec![
        forms::text("full_name", "#full_name", &g.name().to_uppercase()),
        forms::text("email", "#email", &g.email()),
        forms::text("phone", "#phone", &g.phone()),
        forms::vault_as("passport", "text", "#passport", "number", ""),
    ]
}

/// TravelTest's hotel room page (`/book/hotel/<room_offer_id>`): the guest and contact; the arrival time and the
/// special requests stay empty (none); the card only for a prepaid rate (after `purchase_confirm`).
fn traveltest_hotel(g: &Guest) -> Vec<Op> {
    vec![
        forms::text("full_name", "#full_name", &g.name()),
        forms::text("email", "#email", &g.email()),
        forms::text("phone", "#phone", &g.phone()),
    ]
}

/// TravelTest's restaurant slot page (`/book/restaurant/<slot_id>`): the guest, contact, and the dietary needs /
/// occasion from the request.
fn traveltest_restaurant(g: &Guest) -> Vec<Op> {
    vec![
        forms::text("full_name", "#full_name", &g.name()),
        forms::text("email", "#email", &g.email()),
        forms::text("phone", "#phone", &g.phone()),
        forms::text("requests", "#requests", g.requests),
    ]
}

/// Kaya Mobile's checkout (`/kaya_checkout.html`): the delivery name, mobile and address.
fn kaya_checkout(g: &Guest) -> Vec<Op> {
    vec![
        forms::text("name", "#name", &g.name()),
        forms::text("phone", "#phone", &g.phone()),
        forms::text("address", "#address", &g.address()),
    ]
}

/// Stride Sports' product page with its checkout (`/stride_*.html`): the delivery name, mobile and address.
fn stride_checkout(g: &Guest) -> Vec<Op> {
    vec![
        forms::text("name", "#name", &g.name()),
        forms::text("phone", "#phone", &g.phone()),
        forms::text("address", "#address", &g.address()),
    ]
}

pub const SITES: &[Site] = &[
    Site {
        form: "traveltest-flight",
        host: "shop.test",
        path: "/book/flight/",
        fill: traveltest_flight,
        card: TRAVELTEST_CARD,
        ref_re: r"\bTRV-[A-Z][A-Z0-9]{6}\b",
    },
    Site {
        form: "traveltest-hotel",
        host: "shop.test",
        path: "/book/hotel/",
        fill: traveltest_hotel,
        card: TRAVELTEST_CARD,
        ref_re: r"\bTRV-[A-Z][A-Z0-9]{6}\b",
    },
    Site {
        form: "traveltest-restaurant",
        host: "shop.test",
        path: "/book/restaurant/",
        fill: traveltest_restaurant,
        card: TRAVELTEST_CARD,
        ref_re: r"\bTRV-[A-Z][A-Z0-9]{6}\b",
    },
    Site {
        form: "kaya-checkout",
        host: "shop.test",
        path: "/kaya_checkout",
        fill: kaya_checkout,
        card: &[
            ("number", "Card number"),
            ("expiry", "Expiry (MM/YY)"),
            ("cvc", "Security code (CVC)"),
        ],
        ref_re: r"\bKM-\d{4,}\b",
    },
    Site {
        form: "stride-checkout",
        host: "shop.test",
        path: "/stride_",
        fill: stride_checkout,
        card: &[
            ("number", "Card number"),
            ("expiry", "Expiry"),
            ("cvc", "CVC"),
        ],
        ref_re: r"\bSR-\d{4,}\b",
    },
];

/// The known site of a page, by its host and the start of its path: `None` for any other page (handed to
/// `browser-use`).
pub fn site_for(u: &str) -> Option<&'static Site> {
    let x = url::Url::parse(u).ok()?;
    let host = x.host_str()?.to_lowercase();
    let path = x.path().to_lowercase();
    SITES
        .iter()
        .find(|s| host == s.host && path.starts_with(s.path))
}

/// The plan for `site` as `browser_fill_form` takes it (an empty value is skipped and left for the user).
pub fn fill_args(site: &Site, g: &Guest) -> Value {
    let fills: Vec<Value> = (site.fill)(g).iter().filter_map(Op::to_json).collect();
    json!({"form": site.form, "fills": fills})
}

/// The fields of the plan that had no value (named in the hand-over).
fn blanks(site: &Site, g: &Guest) -> Vec<String> {
    (site.fill)(g)
        .iter()
        .filter(|op| op.to_json().is_none() && op.field != "requests")
        .map(|op| op.field.replace('_', " "))
        .collect()
}

// ---------------------------------------------------------------- the booking stage

/// What is booked, as the hand-over and the answer name it.
#[derive(Debug, Clone, Default)]
pub struct Item {
    /// the line of `purchase_confirm` and the answer ("SQ 638 SIN-NRT 2026-12-04, 1 adult")
    pub name: String,
    pub url: String,
    pub total: Option<f64>,
    pub currency: String,
    /// money is charged now (a fare, a prepaid rate, a deposit, an order): `purchase_confirm` and the card
    pub charged: bool,
    pub delivery: String,
    pub note: String,
    /// extra lines for the answer (schedule, baggage, cancellation, payment …)
    pub lines: Vec<String>,
}

const ASK_MARK: &str = "To search, I still need";
const HANDOVER_MARK: &str = "filled in";

/// The booking stage, after the pick: the Vault's list, `purchase_confirm`, the booking link, then a known site's
/// form function (fixed ids), the card by the site's labels, the hand-over and the confirmation — or, on any other
/// site, `load_skill(browser-use)` for that page.
fn book(spec: &BookingSpec, t: &[Value], w: &World, n: usize, item: &Item, req: &Req) -> Step {
    let card_needed = item.charged;
    let vault = calls_of(t, "vault_list").last().map(|(_, m)| content_of(m));
    if (card_needed || spec.kind == Kind::Flight) && vault.is_none() {
        return calls(n, "vault_list", json!({}));
    }
    let cards: Vec<VaultCard> = vault.as_deref().map(vault_cards).unwrap_or_default();
    let card = pick_card(&cards, w.user_texts);
    let card_ok = card.is_some_and(|c| !c.name_only);
    let mut approved_at = 0usize;
    if card_needed {
        match calls_of(t, "purchase_confirm").last() {
            None => {
                let mut a = json!({"site": host_of(&item.url), "items": [{"name": item.name, "qty": 1, "price": item.total.unwrap_or(0.0)}],
                                   "shipping": 0, "total": item.total.unwrap_or(0.0), "currency": item.currency,
                                   "delivery": item.delivery, "note": item.note});
                if let (Some(c), true) = (card, card_ok) {
                    a["card_item_id"] = json!(c.id);
                }
                return calls(n, "purchase_confirm", a);
            }
            Some((_, m)) if denied(m) => {
                return Step::Final(if w.zh {
                    format!("你没有批准这笔付款，所以没有预订：{}。", item.name)
                } else {
                    format!(
                        "The purchase was not approved, so nothing was booked: {}.",
                        item.name
                    )
                });
            }
            Some((_, m)) => approved_at = pos(t, m),
        }
    }
    // the booking link (once)
    let nav = calls_of(t, "browser_navigate")
        .into_iter()
        .rfind(|(a, _)| sget(a, "url") == item.url);
    let Some((nav_args, nav_msg)) = nav else {
        return calls(n, "browser_navigate", json!({"url": item.url}));
    };
    let page_url = landed(&nav_args, nav_msg);
    let Some(site) = site_for(&page_url).or_else(|| site_for(&item.url)) else {
        // no form function knows this page: the model fills it with the browser-use skill
        return calls(n, "load_skill", json!({"name": BROWSER_USE}));
    };
    let nav_pos = pos(t, nav_msg);
    let profile = profile_with_words(w.profile, w.user_texts);
    let guest = Guest {
        profile: &profile,
        requests: &req.occasion,
    };
    // the plan with fixed ids
    let Some((_, fill_msg)) = calls_of(t, "browser_fill_form")
        .into_iter()
        .rfind(|(a, m)| sget(a, "form") == site.form && pos(t, m) > nav_pos)
    else {
        return calls(n, "browser_fill_form", fill_args(site, &guest));
    };
    let fill_pos = pos(t, fill_msg);
    // the card: one secret fill per box, by the site's fixed labels, after the approval
    let mut done_pos = fill_pos;
    if let (Some(c), true, true) = (card, card_ok, card_needed) {
        let after = fill_pos.max(approved_at);
        let Some((_, snap)) = calls_of(t, "browser_snapshot")
            .into_iter()
            .find(|(_, m)| pos(t, m) > after)
        else {
            return calls(n, "browser_snapshot", json!({"max_chars": 6000}));
        };
        let page = snapshot_text(snap);
        let filled: Vec<String> = calls_of(t, "browser_fill_secret")
            .iter()
            .filter(|(_, m)| pos(t, m) > after)
            .map(|(a, _)| sget(a, "field"))
            .collect();
        for (field, label) in site.card {
            if filled.iter().any(|f| f == field) {
                continue;
            }
            if let Some(r) = ref_of(&page, label) {
                return calls(
                    n,
                    "browser_fill_secret",
                    json!({"ref": r, "item_id": c.id, "field": field}),
                );
            }
        }
        done_pos = calls_of(t, "browser_fill_secret")
            .last()
            .map(|(_, m)| pos(t, m))
            .unwrap_or(fill_pos)
            .max(fill_pos);
    }
    // the hand-over: the final click is the user's
    let takeovers: Vec<(Value, &Value)> = calls_of(t, "browser_request_takeover")
        .into_iter()
        .filter(|(_, m)| pos(t, m) > done_pos)
        .collect();
    let back = match takeovers.last() {
        None => {
            let left = blanks(site, &guest);
            let card_part = match (card_needed, card, card_ok) {
                (false, _, _) => String::new(),
                (true, Some(c), true) => format!(" and your {} (Vault)", c.label),
                (true, Some(c), false) => format!(". Your {} is in the Vault by its name only (no number, expiry or CVC): type the card yourself", c.label),
                (true, None, _) => ". There is no card in your Vault: type the card yourself".into(),
            };
            let total = item
                .total
                .filter(|_| item.charged)
                .map(|x| format!(", total {} {}", item.currency, money(x)))
                .unwrap_or_default();
            let reason = format!(
                "The {} page is {HANDOVER_MARK} from your profile{}{card_part}: {}{total}. Check it{}, press {} yourself and hand the browser back.",
                spec.what,
                if spec.kind == Kind::Flight { " and passport (Vault)" } else { "" },
                item.name,
                if left.is_empty() { String::new() } else { format!(", fill in {}", left.join(", ")) },
                spec.button
            );
            return calls(n, "browser_request_takeover", json!({"reason": reason}));
        }
        Some((_, m)) if handed_back(m) => pos(t, m),
        Some(_) => return Step::Final(String::new()),
    };
    let Some((_, snap)) = last_after(t, "browser_snapshot", back) else {
        return calls(n, "browser_snapshot", json!({"max_chars": 6000}));
    };
    let page = snapshot_text(snap);
    let reference = Regex::new(site.ref_re)
        .ok()
        .and_then(|r| r.find(&page).map(|m| m.as_str().to_string()))
        .or_else(|| crate::outcome::refs_in(&page).into_iter().next());
    Step::Final(answer_booked(
        spec,
        item,
        reference.as_deref(),
        &guest,
        card.filter(|_| card_ok && card_needed),
        w.zh,
    ))
}

fn answer_booked(
    spec: &BookingSpec,
    item: &Item,
    reference: Option<&str>,
    g: &Guest,
    card: Option<&VaultCard>,
    zh: bool,
) -> String {
    let total = item
        .total
        .map(|x| format!("{} {}", item.currency, money(x)))
        .unwrap_or_default();
    let mut lines: Vec<String> = vec![format!("- {}", item.name)];
    if !total.is_empty() {
        lines.push(if item.charged {
            format!(
                "- {}: {total}{}",
                if spec.kind == Kind::Shop {
                    "Order total"
                } else {
                    "Total"
                },
                card.map(|c| format!(" (your {})", c.label))
                    .unwrap_or_default()
            )
        } else {
            format!("- Total: {total} (nothing charged now)")
        });
    }
    lines.extend(item.lines.iter().map(|l| format!("- {l}")));
    if !g.email().is_empty() && spec.kind != Kind::Shop {
        lines.push(format!("- Confirmation to {}", g.email()));
    }
    match (reference, zh) {
        (Some(r), false) => format!(
            "Your {} is **{}**: reference **{r}**.\n\n{}",
            spec.what,
            if spec.kind == Kind::Shop {
                "placed"
            } else {
                "booked"
            },
            lines.join("\n")
        ),
        (Some(r), true) => format!(
            "已完成{}：编号 **{r}**。\n\n{}",
            if spec.kind == Kind::Shop {
                "下单"
            } else {
                "预订"
            },
            lines.join("\n")
        ),
        (None, false) => format!(
            "Your {} is filled in but **not confirmed**: no reference showed after the hand-back, so it is not booked. The page stays open in the browser tab for you.\n\n{}",
            spec.what,
            lines.join("\n")
        ),
        (None, true) => format!(
            "已填好，但**还没有确认**：交还后页面上没有编号，所以还没有订上。页面仍在浏览器标签页里。\n\n{}",
            lines.join("\n")
        ),
    }
}

// ---------------------------------------------------------------- the machine

/// The extraction call's facts (the last resort), if it ran: (position, JSON).
fn facts(transcript: &[Value]) -> Option<(usize, Value)> {
    calls_of(transcript, "booking_facts")
        .last()
        .map(|(_, m)| (pos(transcript, m), json_of(m)))
}

/// The next step of the run, with the catalog's default tool names (see [`next_in`]).
pub fn next(spec: &BookingSpec, transcript: &[Value], w: &World) -> Step {
    next_in(spec, transcript, w, &[])
}

/// The next step of the run; `available` are the tool names of the catalog (to call a travel MCP connector's
/// search when the native one is not there).
pub fn next_in(spec: &BookingSpec, t: &[Value], w: &World, available: &[String]) -> Step {
    let n = step_count(t);
    if spec.kind == Kind::Shop {
        return next_shop(spec, t, w, n);
    }
    let text = w.user_texts.join("\n");
    let fact = facts(t);
    let req = parse_request(
        spec.kind,
        w.user_texts,
        w.profile,
        w.today,
        fact.as_ref().map(|(_, v)| v),
    );
    // 1. one search, on the user's connector when they name it (the data tools follow the search)
    let searches = calls_of(t, spec.search);
    let connector = user_asks(w.user_texts, &CONNECTOR)
        || searches
            .last()
            .is_some_and(|(_, m)| search_name(t, m).starts_with("mcp_"));
    let Some((_, search_msg)) = searches.last() else {
        let missing = req.missing(spec.kind);
        if missing.is_empty() {
            return calls(
                n,
                &tool_for(spec.search, available, connector),
                req.search_args(spec.kind),
            );
        }
        let fresh = fact.as_ref().is_none_or(|(p, _)| last_user_pos(t) > *p);
        if fresh {
            return Step::Extract(
                take_chars(
                    &format!(
                        "Today: {}\n\n{}",
                        iso(w.today),
                        w.user_texts
                            .iter()
                            .map(|u| format!("User: {u}"))
                            .collect::<Vec<_>>()
                            .join("\n\n")
                    ),
                    8000,
                )
                .to_string(),
            );
        }
        if !asked_before(t, ASK_MARK) {
            return Step::Final(if w.zh {
                format!("{ASK_MARK}（还需要）：\n- {}", missing.join("\n- "))
            } else {
                format!("{ASK_MARK}:\n- {}", missing.join("\n- "))
            });
        }
        return Step::Final(format!(
            "Without {} I cannot search; tell me and ask again.",
            missing.join(" and ")
        ));
    };
    // the booking stage under way and the search result compressed away (a long run: snapshots, the vault): the item
    // from the machine's own calls, whose arguments compression never touches
    if content_of(search_msg).contains(super::guards::COMPRESSED_MARK) {
        if let Some(item) = resumed_item(t, pos(t, search_msg)) {
            return book(spec, t, w, n, &item, &req);
        }
    }
    let result = data_of(search_msg);
    let offers = offers_in(spec.kind, &result);
    let next_ask = next_search(spec.kind, w.user_texts, &req);
    let then_next = |s: String| match &next_ask {
        Some(q) => format!("{s}\n\n{q}"),
        None => s,
    };
    if offers.is_empty() {
        // the native search found nothing priced: the user's travel connector next, once
        let mcp = tool_for(spec.search, available, true);
        if mcp.starts_with("mcp_") && !search_name(t, search_msg).starts_with("mcp_") {
            return calls(n, &mcp, req.search_args(spec.kind));
        }
        // the search was limited to the preferred airlines and none flies it: say so, ask before searching all
        if spec.kind == Kind::Flight && !req.airlines.is_empty() && web_pages(&result).is_empty() {
            return Step::Final(no_preferred(&req, &[]));
        }
        // the flight / hotel APIs failed and the search fell back to the web: the pages it read, with their "from"
        // prices as the pages show them
        // (the pages of whichever search fell back to the web: a connector tried after it may have failed)
        let pages = searches
            .iter()
            .rev()
            .map(|(_, m)| web_pages(&data_of(m)))
            .find(|p| !p.is_empty())
            .unwrap_or_default();
        if !pages.is_empty() {
            let compare = get_str(&result, "compare_url");
            return Step::Final(then_next(format!(
                "No {} search answered for {}, so these are web pages about it (their prices are \"from\" prices, not quotes for your dates):\n{}{}",
                spec.what,
                searched_line(spec.kind, &req),
                pages.join("\n"),
                if compare.is_empty() { String::new() } else { format!("\n\nCompare prices for your dates on Kayak: {compare}") }
            )));
        }
        return Step::Final(format!(
            "The search found nothing for {}. Tell me what to change (dates, area, budget …) and I will search again.",
            searched_line(spec.kind, &req)
        ));
    }
    // a search limited to the preferred airlines that found none of them (the API had no filter for them): the
    // answer says so and asks before searching all airlines — no second search now
    if spec.kind == Kind::Flight
        && !req.airlines.is_empty()
        && result["searched"]["preferred_found"].as_u64() == Some(0)
    {
        return Step::Final(no_preferred(&req, &offers));
    }
    let (list, rule) = ranked(spec.kind, &req, &offers, &text);
    let chosen = picked_label(w.user_texts).and_then(|l| by_label(&offers, &l).cloned());
    let (pick, rule) = match chosen {
        Some(o) => (o, "your pick".to_string()),
        None => match list.first() {
            Some(o) => (o.clone(), rule),
            None => {
                return Step::Final(format!(
                    "None of the {} results for {} meets {} — the options:\n{}\n\nTell me what to relax and I will look again.",
                    offers.len(),
                    searched_line(spec.kind, &req),
                    rule.trim_end_matches(", the cheapest"),
                    offers.iter().take(4).map(|o| format!("- {}", option_line(spec.kind, o))).collect::<Vec<_>>().join("\n")
                ))
            }
        },
    };
    let book_it = wants_booking(w.user_texts);
    let search_pos = pos(t, search_msg);
    // 2. the pick's data: a restaurant's times (the next candidate when it is full), a hotel's rooms when booking
    let mut slot: Option<(String, String, Value)> = None; // (time, url, the availability result)
    let mut pick = pick;
    if spec.kind == Kind::Michelin {
        let avail = calls_of(t, spec.detail);
        let mut tried = 0;
        let mut found = None;
        let candidates: Vec<Offer> = std::iter::once(pick.clone())
            .chain(list.iter().filter(|o| o.label != pick.label).cloned())
            .take(4)
            .collect();
        for c in &candidates {
            let got = avail
                .iter()
                .rfind(|(a, _)| sget(a, "restaurant_id") == c.id);
            let Some((_, m)) = got else {
                return calls(
                    n,
                    &tool_for(spec.detail, available, connector),
                    json!({"restaurant_id": c.id, "date": req.date.map(iso).unwrap_or_default(), "party_size": req.party_or(spec.kind)}),
                );
            };
            tried += 1;
            let v = data_of(m);
            let slots: Vec<(String, String)> = v
                .get("slots")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .map(|s| (get_str(s, "time"), get_str(s, "booking_url")))
                .filter(|(t, _)| !t.is_empty())
                .collect();
            if let Some(i) = nearest_slot(&slots, &req.time) {
                found = Some((c.clone(), slots[i].0.clone(), slots[i].1.clone(), v));
                break;
            }
        }
        match found {
            Some((c, time, url, v)) => {
                pick = c;
                slot = Some((time, url, v));
            }
            None => {
                return Step::Final(format!(
                    "No table on {} for {} at the {tried} restaurant(s) I checked ({}). Tell me another date or time and I will look again.",
                    req.date.map(long).unwrap_or_default(),
                    req.party_or(spec.kind),
                    candidates.iter().take(tried).map(|c| c.label.as_str()).collect::<Vec<_>>().join(", ")
                ));
            }
        }
    }
    // 3. the choice card with the pick shown (once)
    if calls_of(t, "present_choices")
        .iter()
        .all(|(_, m)| pos(t, m) < search_pos)
    {
        let mut shown: Vec<Offer> = vec![pick.clone()];
        shown.extend(
            list.iter()
                .filter(|o| o.label != pick.label)
                .take(3)
                .cloned(),
        );
        let options: Vec<Value> = shown
            .iter()
            .map(|o| {
                let mut details = o.details.clone();
                let mut url = o.url.clone();
                if o.label == pick.label {
                    if let Some((time, surl, _)) = &slot {
                        details.push(format!("{time} available"));
                        url = surl.clone();
                    }
                }
                json!({"label": o.label, "details": details, "source_url": url,
                       "note": if o.label == pick.label { format!("picked: {rule}") } else { String::new() }})
            })
            .collect();
        return calls(
            n,
            "present_choices",
            json!({"question": format!("{} for {}", choice_title(spec.kind), searched_line(spec.kind, &req)),
                   "kind": "comparison", "options": options, "picked": pick.label}),
        );
    }
    // 3b. the pick's own data: a Google Flights offer's booking options (who sells it, at what price) and, for the
    // comparison, a hotel's rooms (the booking stage reads the same call)
    let mut extra: Vec<String> = Vec::new();
    if spec.kind == Kind::Flight && pick.id.starts_with("fl_") {
        let got = calls_of(t, "flight_offer")
            .into_iter()
            .rfind(|(a, m)| sget(a, "offer_id") == pick.id && pos(t, m) > search_pos);
        let Some((_, m)) = got else {
            let cur = if pick.currency.is_empty() {
                &req.currency
            } else {
                &pick.currency
            };
            return calls(
                n,
                &tool_for("flight_offer", available, false),
                json!({"offer_id": pick.id, "currency": cur}),
            );
        };
        extra.extend(booking_options(&data_of(m)));
    }
    let rooms_args = json!({"hotel_id": pick.id, "check_in": req.date.map(iso).unwrap_or_default(),
                            "check_out": req.check_out.map(iso).unwrap_or_default(), "guests": req.party_or(spec.kind)});
    if spec.kind == Kind::Hotel && !book_it && !pick.id.is_empty() {
        let rooms_call = calls_of(t, spec.detail)
            .into_iter()
            .rfind(|(a, _)| sget(a, "hotel_id") == pick.id);
        let Some((_, m)) = rooms_call else {
            return calls(n, &tool_for(spec.detail, available, connector), rooms_args);
        };
        let rooms = offers_in(spec.kind, &data_of(m));
        if let Some((i, why)) = pick_room(&rooms, &req) {
            let r = &rooms[i];
            let mut parts = vec![format!("Room at {}: **{}**", pick.label, r.label)];
            parts.extend(
                r.details
                    .iter()
                    .filter(|d| !pick.details.contains(d))
                    .cloned(),
            );
            if !r.url.is_empty() {
                parts.push(r.url.clone());
            }
            extra.push(format!("{} ({why})", parts.join(" · ")));
        }
    }
    if !book_it {
        let answer = answer_compare(
            spec,
            &req,
            &pick,
            &rule,
            &list,
            &result,
            slot.as_ref().map(|(t, _, _)| t.as_str()),
            &extra,
        );
        // "… and email the options to mei@example.com": the comparison mailed to the address in the user's words
        let Some(to) = mail_to(w.user_texts) else {
            return Step::Final(then_next(answer));
        };
        let Some((_, sent)) = last_after(t, "gmail_send", search_pos) else {
            return calls(
                n,
                "gmail_send",
                json!({"to": to, "subject": format!("{} for {}", choice_title(spec.kind), searched_line(spec.kind, &req)),
                       "body": answer}),
            );
        };
        let c = content_of(sent);
        let done = if c.starts_with("ERROR") || denied(sent) {
            format!("Not sent to {to}: {}", take_chars(&c, 200))
        } else {
            format!("Sent to {to}.")
        };
        return Step::Final(then_next(format!("{answer}\n\n{done}")));
    }
    // 4. the item to book
    let item = match spec.kind {
        Kind::Flight => Item {
            name: format!(
                "{} {}-{} {}, {} adult{}{}",
                pick.label,
                req.origin,
                req.destination,
                req.date.map(iso).unwrap_or_default(),
                req.party_or(spec.kind),
                if req.party_or(spec.kind) > 1 { "s" } else { "" },
                match req.cabin {
                    "economy" => String::new(),
                    c => format!(", {}", c.replace('_', " ")),
                }
            ),
            url: pick.url.clone(),
            total: pick.price.map(|p| p * req.party_or(spec.kind) as f64),
            currency: pick.currency.clone(),
            charged: true,
            delivery: format!(
                "E-ticket to {}",
                Guest {
                    profile: &profile_with_words(w.profile, w.user_texts),
                    requests: ""
                }
                .email()
            ),
            note: pick.cancellation.clone(),
            lines: pick.details.clone(),
        },
        Kind::Hotel => {
            // the rooms of the picked hotel, then the room by rule
            let rooms_call = calls_of(t, spec.detail)
                .into_iter()
                .rfind(|(a, _)| sget(a, "hotel_id") == pick.id);
            if rooms_call.is_none() && !pick.id.is_empty() {
                return calls(n, &tool_for(spec.detail, available, connector), rooms_args);
            }
            let rooms: Vec<Offer> = rooms_call
                .map(|(_, m)| offers_in(spec.kind, &data_of(m)))
                .unwrap_or_default()
                .into_iter()
                .filter(|r| !r.url.is_empty())
                .collect();
            let nights = match (req.date, req.check_out) {
                (Some(a), Some(b)) => (b - a).num_days(),
                _ => 0,
            };
            let dates = format!(
                "{} to {} ({nights} night{})",
                req.date.map(iso).unwrap_or_default(),
                req.check_out.map(iso).unwrap_or_default(),
                if nights == 1 { "" } else { "s" }
            );
            match pick_room(&rooms, &req) {
                Some((i, _)) => {
                    let r = &rooms[i];
                    let pay = r.payment.to_lowercase();
                    let prepaid = !pay.contains("no prepayment")
                        && (pay.starts_with("prepay")
                            || pay.contains("prepaid")
                            || pay.contains("pay now"));
                    let mut lines = Vec::new();
                    if !r.cancellation.is_empty() {
                        lines.push(format!(
                            "Cancellation: {} (hotel local time)",
                            r.cancellation
                        ));
                    }
                    if !r.payment.is_empty() {
                        lines.push(format!("Payment: {}", r.payment));
                    }
                    Item {
                        name: format!(
                            "{} – {}, {dates}, {} guest{}",
                            pick.label,
                            r.label,
                            req.party_or(spec.kind),
                            if req.party_or(spec.kind) > 1 { "s" } else { "" }
                        ),
                        url: r.url.clone(),
                        total: r.total.or(r.price.map(|p| p * nights.max(1) as f64)),
                        currency: if r.currency.is_empty() {
                            pick.currency.clone()
                        } else {
                            r.currency.clone()
                        },
                        charged: prepaid,
                        delivery: format!(
                            "Hotel stay, {}",
                            if prepaid { "prepaid" } else { "pay at hotel" }
                        ),
                        note: r.cancellation.clone(),
                        lines,
                    }
                }
                None => Item {
                    name: format!("{}, {dates}", pick.label),
                    url: pick.url.clone(),
                    total: pick.total,
                    currency: pick.currency.clone(),
                    charged: false,
                    delivery: "Hotel stay".into(),
                    note: pick.cancellation.clone(),
                    lines: Vec::new(),
                },
            }
        }
        Kind::Michelin => {
            let (time, url, v) = slot.clone().unwrap_or_default();
            let deposit = get_str(&v, "deposit_due_now");
            let dep = num_of(v.get("deposit_due_now")).filter(|d| *d > 0.0);
            let mut lines = vec![format!(
                "{} · {}",
                pick.distinction,
                req.date.map(long).unwrap_or_default()
            )];
            let policy = get_str(&v["restaurant"], "deposit_policy");
            if !policy.is_empty() {
                lines.push(format!("Deposit: {policy}"));
            }
            let seating = get_str(&v, "seating_limit");
            if !seating.is_empty() {
                lines.push(format!("Seating limit: {seating}"));
            }
            let dress = get_str(&v["restaurant"], "dress_code");
            if !dress.is_empty() {
                lines.push(format!("Dress code: {dress}"));
            }
            Item {
                name: format!(
                    "{} {} {time}, {} guests{}",
                    pick.label,
                    req.date.map(iso).unwrap_or_default(),
                    req.party_or(spec.kind),
                    if dep.is_some() { " – deposit" } else { "" }
                ),
                url,
                total: dep,
                currency: {
                    let c = currency_in(&deposit);
                    if c.is_empty() {
                        currency_in(&pick.price_text)
                    } else {
                        c
                    }
                },
                charged: dep.is_some(),
                delivery: "Table reservation".into(),
                note: policy,
                lines,
            }
        }
        Kind::Restaurant | Kind::Shop => Item {
            name: format!(
                "{}, {} at {}, {} guests",
                pick.label,
                req.date
                    .map(iso)
                    .unwrap_or_else(|| "the date you gave".into()),
                req.time,
                req.party_or(spec.kind)
            ),
            url: pick.url.clone(),
            total: None,
            currency: String::new(),
            charged: false,
            delivery: "Table reservation".into(),
            note: String::new(),
            lines: Vec::new(),
        },
    };
    if item.url.is_empty() {
        return Step::Final(format!(
            "{}\n\n{} has no online booking link in the data: book it by phone or on its own site.",
            answer_compare(
                spec,
                &req,
                &pick,
                &rule,
                &list,
                &result,
                slot.as_ref().map(|(t, _, _)| t.as_str()),
                &extra,
            ),
            pick.label
        ));
    }
    book(spec, t, w, n, &item, &req)
}

/// The item of a booking stage already under way, from the machine's own calls after the search: the booking link it
/// opened, and the approved `purchase_confirm` (what, the total, the currency, the delivery) or else the pick shown on
/// the choice card. `None` before the booking link was opened.
fn resumed_item(t: &[Value], search_pos: usize) -> Option<Item> {
    let (nav, _) = calls_of(t, "browser_navigate")
        .into_iter()
        .find(|(_, m)| pos(t, m) > search_pos && get_str(m, "tool_call_id").starts_with("bkg_"))?;
    let url = sget(&nav, "url");
    let confirm = calls_of(t, "purchase_confirm")
        .into_iter()
        .rfind(|(_, m)| pos(t, m) > search_pos)
        .map(|(a, _)| a);
    let picked = calls_of(t, "present_choices")
        .into_iter()
        .rfind(|(_, m)| pos(t, m) > search_pos)
        .map(|(a, _)| sget(&a, "picked"))
        .unwrap_or_default();
    Some(match confirm {
        Some(a) => Item {
            name: a["items"][0]
                .get("name")
                .and_then(Value::as_str)
                .map(str::to_string)
                .unwrap_or(picked),
            url,
            total: a.get("total").and_then(Value::as_f64),
            currency: sget(&a, "currency"),
            charged: true,
            delivery: sget(&a, "delivery"),
            note: sget(&a, "note"),
            lines: Vec::new(),
        },
        None => Item {
            name: picked,
            url,
            ..Item::default()
        },
    })
}

fn choice_title(kind: Kind) -> &'static str {
    match kind {
        Kind::Flight => "Flights",
        Kind::Hotel => "Hotels",
        Kind::Michelin => "MICHELIN restaurants",
        Kind::Restaurant => "Restaurants",
        Kind::Shop => "Products",
    }
}

fn searched_line(kind: Kind, r: &Req) -> String {
    match kind {
        Kind::Flight => format!(
            "{} to {} {}{}, {}, {} adult{}{}, {}",
            r.origin,
            r.destination,
            match r.date_to {
                Some(to) => format!(
                    "any day {} to {}",
                    r.date.map(iso).unwrap_or_default(),
                    iso(to)
                ),
                None => format!("on {}", r.date.map(iso).unwrap_or_default()),
            },
            r.return_date
                .map(|d| format!(" (return {})", iso(d)))
                .unwrap_or_default(),
            r.cabin.replace('_', " "),
            r.party_or(kind),
            if r.party_or(kind) > 1 { "s" } else { "" },
            if r.nonstop { ", direct" } else { "" },
            match r.max_price {
                Some(p) => format!("under {} {}", r.currency, money(p).trim_end_matches(".00"))
                    .replace("under  ", "under "),
                None => "any price".to_string(),
            }
        ),
        Kind::Hotel => format!(
            "{}, {} to {}, {} guest{}",
            r.city,
            r.date.map(iso).unwrap_or_default(),
            r.check_out.map(iso).unwrap_or_default(),
            r.party_or(kind),
            if r.party_or(kind) > 1 { "s" } else { "" }
        ),
        Kind::Michelin | Kind::Restaurant => format!(
            "{}{}, {}{}, {} guests",
            r.city,
            if r.cuisine.is_empty() {
                String::new()
            } else {
                format!(" ({})", r.cuisine)
            },
            r.date.map(long).unwrap_or_else(|| "any date".into()),
            if r.time.is_empty() {
                String::new()
            } else {
                format!(" around {}", r.time)
            },
            r.party_or(kind)
        ),
        Kind::Shop => String::new(),
    }
}

fn option_line(kind: Kind, o: &Offer) -> String {
    let mut parts = vec![format!("**{}**", o.label)];
    parts.extend(o.details.iter().cloned());
    if o.preferred {
        parts.push(match kind {
            Kind::Flight => "your preferred airline".into(),
            _ => "your preferred brand".to_string(),
        });
    }
    let line = parts.join(" · ");
    if o.url.is_empty() {
        return line;
    }
    // where to book it: the offer's booking link (Kiwi.com's page for it), and the airline's own site
    let mut book = format!("Book it at {}", o.url);
    if o.data == "Kiwi.com" {
        book.push_str(" (Kiwi.com)");
    }
    if !o.book_at.is_empty() {
        book.push_str(&format!(" or on {}", o.book_at.join(" or ")));
    }
    format!("{line} — {book}")
}

/// A Google Flights offer's booking options (`flight_offer`) as an answer line: who sells it, at what price.
fn booking_options(v: &Value) -> Vec<String> {
    let sellers: Vec<String> = v
        .get("options")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|o| !get_str(o, "book_with").is_empty())
        .map(|o| {
            let price = num_of(o.get("price"))
                .map(|p| {
                    format!(
                        " {} {}",
                        get_str(o, "currency"),
                        money(p).trim_end_matches(".00")
                    )
                })
                .unwrap_or_default();
            let title = get_str(o, "title");
            format!(
                "{}{price}{}",
                get_str(o, "book_with"),
                if title.is_empty() {
                    String::new()
                } else {
                    format!(" ({title})")
                }
            )
        })
        .collect();
    if sellers.is_empty() {
        Vec::new()
    } else {
        vec![format!("Sold by: {}", sellers.join(", "))]
    }
}

/// The answer when a search limited to the preferred airlines found none of them: no second search now — the
/// question whether to search all airlines (and what else came back, when the API searched them all).
fn no_preferred(req: &Req, others: &[Offer]) -> String {
    let mut out = format!(
        "There is no {} flight for {} (the search was limited to your preferred airline{}).",
        req.airlines.join(" or "),
        searched_line(Kind::Flight, req),
        if req.airlines.len() > 1 { "s" } else { "" }
    );
    if !others.is_empty() {
        out.push_str("\nThe search also returned:\n");
        for o in others.iter().take(4) {
            out.push_str(&format!("- {}\n", option_line(Kind::Flight, o)));
        }
    }
    out.push_str("\n\nShall I search all airlines?");
    out
}

/// The next travel search the request asks for, offered after this one (ONE travel search per request): a flight's
/// answer offers the hotel, a hotel's the rental car — only when the user's words ask for it.
pub const NEXT_MARK: &str = "Shall I search the ";

fn next_search(kind: Kind, texts: &[String], req: &Req) -> Option<String> {
    static HOTEL_W: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?i)\b(hotels?|accommodation|place to stay|ryokan)\b|酒店|住宿").unwrap()
    });
    static CAR_W: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?i)\b(rental car|car rental|car hire|hire a car|rent a car|a car)\b|租车")
            .unwrap()
    });
    let text = texts.join("\n");
    let wants_hotel = HOTEL_W.is_match(&text);
    let wants_car = CAR_W.is_match(&text);
    match kind {
        Kind::Flight if wants_hotel => {
            let area = area_in(&text);
            Some(format!(
                "{NEXT_MARK}hotel{} now?",
                if area.is_empty() {
                    if req.destination.is_empty() {
                        String::new()
                    } else {
                        CITIES
                            .iter()
                            .find(|c| c.code == req.destination)
                            .map(|c| format!(" in {}", c.name))
                            .unwrap_or_default()
                    }
                } else {
                    format!(" near {area}")
                }
            ))
        }
        Kind::Flight | Kind::Hotel if wants_car => Some(format!("{NEXT_MARK}rental car now?")),
        _ => None,
    }
}

#[allow(clippy::too_many_arguments)]
fn answer_compare(
    spec: &BookingSpec,
    req: &Req,
    pick: &Offer,
    rule: &str,
    list: &[Offer],
    result: &Value,
    time: Option<&str>,
    extra: &[String],
) -> String {
    let mut out = format!(
        "{} for {}",
        choice_title(spec.kind),
        searched_line(spec.kind, req)
    );
    if spec.kind == Kind::Flight {
        // the cabin searched, as the search reports it (an answer in another cabin is never given as this one)
        if req.cabin != "economy" {
            let searched = get_str(&result["searched"], "cabin");
            out = format!(
                "{} — searched as {}. {out}",
                req.cabin.replace('_', " ").to_uppercase()
                    + if req.cabin == "premium_economy" {
                        ""
                    } else {
                        " CLASS"
                    },
                if searched.is_empty() {
                    req.cabin.replace('_', " ")
                } else {
                    searched
                }
            );
        }
        let data = get_str(result, "data");
        if !data.is_empty() {
            out.push_str(&format!(" ({data} data)"));
        }
    }
    out.push_str(":\n");
    let mut shown = vec![pick];
    shown.extend(list.iter().filter(|o| o.label != pick.label).take(3));
    for o in shown {
        out.push_str(&format!("- {}\n", option_line(spec.kind, o)));
    }
    out.push_str(&format!(
        "\nPicked: **{}**{} — {rule}.",
        pick.label,
        time.map(|t| format!(" at {t}")).unwrap_or_default()
    ));
    for l in extra {
        out.push_str(&format!("\n{l}"));
    }
    // a range of days: ONE search covered them all
    let days: Vec<String> = result
        .get("by_date")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .map(|d| get_str(d, "date"))
        .filter(|d| !d.is_empty())
        .collect();
    if days.len() > 1 {
        out.push_str(&format!(
            "\n{} days searched in one call ({} to {}){}.",
            days.len(),
            days[0],
            days[days.len() - 1],
            if pick.date.is_empty() {
                String::new()
            } else {
                format!("; the pick flies on {}", pick.date)
            }
        ));
    }
    let compare = get_str(result, "compare_url");
    if !compare.is_empty() {
        out.push_str(&format!("\nCompare prices on Kayak: {compare}"));
    }
    out.push_str(&format!(
        "\n\nSay \"book it\" and I will fill in the booking page for you to check and confirm (the {} click stays yours).",
        spec.button
    ));
    out
}

// ---------------------------------------------------------------- shopping

/// The ranking the user's words ask for, read by regex: best-rated / cheapest / most-reviewed, the price cap, the
/// minimum stars and ratings, how many, the product word and the device model every chosen name has.
pub fn shop_ranking(text: &str) -> crate::listings::Ranking {
    static COUNT: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?i)\b(?:the\s+)?(\d|two|three|four|five|six)\s+(?:best|cheapest|top|most)|\btop\s+(\d|two|three|four|five)\b").unwrap()
    });
    static MIN_RATINGS: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?i)\b(?:at least|over|more than|min(?:imum)?)\s+(\d[\d,]*)\s+(?:ratings|reviews)\b|\b(\d[\d,]*)\+\s*(?:ratings|reviews)\b").unwrap()
    });
    static MIN_STARS: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?i)\b(?:at least|over)\s+(\d(?:\.\d)?)\s*stars?\b|\b(\d\.\d)\+?\s*stars?\s+or\s+(?:more|better|higher)\b").unwrap()
    });
    static MODEL: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?i)\b((?:iphone|galaxy|pixel|xiaomi|redmi)\s+[a-z]?\d{1,3}(?:\s+(?:pro\s+max|pro|plus|ultra|max|mini|fe))?)\b").unwrap()
    });
    const PRODUCTS: &[&str] = &[
        "screen protector",
        "power bank",
        "case",
        "cover",
        "charger",
        "cable",
        "socks",
        "shoes",
        "headphones",
        "earbuds",
        "backpack",
        "bag",
        "watch",
        "strap",
    ];
    let lc = text.to_lowercase();
    let mut r = crate::listings::Ranking {
        by: if Regex::new(r"(?i)\b(cheapest|lowest price|least expensive)\b")
            .unwrap()
            .is_match(text)
        {
            "price".into()
        } else if Regex::new(r"(?i)\bmost (reviewed|ratings|popular)\b")
            .unwrap()
            .is_match(text)
        {
            "ratings".into()
        } else {
            "rating".into()
        },
        ..Default::default()
    };
    static MAX: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?i)\b(?:under|below|less than|max(?:imum)?|up to|at most)\s*(?:S\$|SGD|US\$|\$|RM|€|£)?\s*(\d[\d,]*(?:\.\d+)?)").unwrap()
    });
    r.max_price = MAX
        .captures(text)
        .and_then(|c| c[1].replace(',', "").parse().ok());
    r.min_ratings = MIN_RATINGS
        .captures(text)
        .and_then(|c| c.get(1).or_else(|| c.get(2)))
        .and_then(|m| m.as_str().replace(',', "").parse().ok());
    r.min_stars = MIN_STARS
        .captures(text)
        .and_then(|c| c.get(1).or_else(|| c.get(2)))
        .and_then(|m| m.as_str().parse().ok());
    r.count = COUNT
        .captures(text)
        .and_then(|c| c.get(1).or_else(|| c.get(2)))
        .and_then(|m| word_no(m.as_str()))
        .map(|n| n as usize);
    if let Some(p) = PRODUCTS.iter().find(|p| has_word(&lc, p)) {
        r.title_has.push(p.to_string());
    }
    if let Some(m) = MODEL.captures(text) {
        r.title_has.push(m[1].to_string());
    }
    r
}

/// The checkout of a shopping run on a known shop (Kaya Mobile: results → product → Add to Cart → cart → checkout;
/// Stride Sports: the product page holds the checkout), or the shop's page handed to `browser-use` on any other site.
fn next_shop(spec: &BookingSpec, t: &[Value], w: &World, n: usize) -> Step {
    let text = w.user_texts.join("\n");
    let Some(url) = shop_url(&text) else {
        return Step::Final(String::new());
    };
    let lc_path = url::Url::parse(&url)
        .map(|u| u.path().to_lowercase())
        .unwrap_or_default();
    let buy = wants_booking(w.user_texts);
    let cart = buy || CART.is_match(&text);
    let profile = profile_with_words(w.profile, w.user_texts);
    let guest = Guest {
        profile: &profile,
        requests: "",
    };
    let delivery = format!("Home delivery to {}", guest.address());
    if host_of(&url) == "shop.test" && lc_path.starts_with("/stride_") {
        // the product page is the checkout: open it, read the item and the price, then book
        let Some((_, m)) = calls_of(t, "browser_navigate")
            .into_iter()
            .rfind(|(a, _)| sget(a, "url") == url)
        else {
            return calls(n, "browser_navigate", json!({"url": url}));
        };
        // Stride's product page by its fixed parts: the item in the <title> ("<item> — Stride Sports Singapore"),
        // the amount on the "total S$…" line of its checkout
        let page = snapshot_text(m);
        let title = get_str(&json_of(m), "title")
            .split(" — ")
            .next()
            .map(str::trim)
            .filter(|t| !t.is_empty())
            .map(String::from)
            .unwrap_or_else(|| "the item".into());
        static TOTAL: LazyLock<Regex> =
            LazyLock::new(|| Regex::new(r"(?i)\btotal\s+S\$\s*([\d,]+(?:\.\d+)?)").unwrap());
        let price = TOTAL
            .captures(&page)
            .and_then(|c| c[1].replace(',', "").parse::<f64>().ok());
        if !buy {
            return Step::Final(format!(
                "{title}: {} on Stride Sports ({url}). Say \"buy it\" and I will fill in the checkout for you to place the order.",
                price.map(|p| format!("S${}", money(p))).unwrap_or_default()
            ));
        }
        let item = Item {
            name: title,
            url: url.clone(),
            total: price,
            currency: "SGD".into(),
            charged: true,
            delivery,
            note: String::new(),
            lines: vec![format!("Delivery: home delivery to {}", guest.address())],
        };
        return book(spec, t, w, n, &item, &Req::default());
    }
    if !(host_of(&url) == "shop.test" && lc_path.starts_with("/kaya_")) {
        // any other shop: open it and hand the page to browser-use
        if calls_of(t, "browser_navigate")
            .iter()
            .all(|(a, _)| sget(a, "url") != url)
        {
            return calls(n, "browser_navigate", json!({"url": url}));
        }
        return calls(n, "load_skill", json!({"name": BROWSER_USE}));
    }
    // Kaya Mobile: the results page read whole, the listings ranked by code
    let origin = origin_of(&url);
    let Some((_, read)) = calls_of(t, "browser_read").into_iter().rfind(|(a, _)| {
        a["urls"]
            .as_array()
            .is_some_and(|u| u.iter().any(|x| x.as_str() == Some(url.as_str())))
    }) else {
        return calls(n, "browser_read", json!({"urls": [url]}));
    };
    let page = pages_text(read);
    let listings = crate::listings::parse(&page);
    let ranking = shop_ranking(&text);
    let ranked = ranking.apply(&listings);
    let chosen = picked_label(w.user_texts).and_then(|l| {
        listings
            .iter()
            .find(|x| {
                x.title.eq_ignore_ascii_case(&l)
                    || l.to_lowercase().contains(&x.title.to_lowercase())
            })
            .cloned()
    });
    let wants_choose = CHOOSE.is_match(&text) && chosen.is_none();
    let read_pos = pos(t, read);
    let shown = calls_of(t, "present_choices")
        .iter()
        .any(|(_, m)| pos(t, m) > read_pos);
    let Some(top) = chosen.clone().or_else(|| ranked.first().cloned()) else {
        return Step::Final(format!(
            "None of the {} listings on Kaya Mobile's page meets what you asked for; tell me what to relax.",
            listings.len()
        ));
    };
    if !shown && chosen.is_none() {
        let options: Vec<Value> = ranked
            .iter()
            .map(|l| json!({"label": l.title, "details": [l.price_text, l.rating_text], "source_url": url}))
            .collect();
        let mut rk = ranking.to_json();
        rk["url"] = json!(url);
        let mut a = json!({"question": format!("The {} listings on Kaya Mobile's page that meet your criteria ({} compared)", ranked.len(), listings.len()),
                           "kind": "comparison", "options": options, "ranking": rk});
        if !wants_choose {
            a["picked"] = json!(top.title);
        }
        return calls(n, "present_choices", a);
    }
    if wants_choose {
        return Step::Final(format!(
            "Here are the {} best matches out of {} listings on Kaya Mobile's page — press Choose on the one you want and I will add it to your cart.",
            ranked.len(),
            listings.len()
        ));
    }
    if !cart {
        return Step::Final(format!(
            "Best match on Kaya Mobile: **{}** — {} ({}). Say \"add it to my cart\" or \"buy it\".",
            top.title, top.price_text, top.rating_text
        ));
    }
    // the cart first: an item a previous turn put there is not added twice
    let cart_url = format!("{origin}/kaya_cart.html");
    let Some((_, cart_msg)) = calls_of(t, "browser_navigate")
        .into_iter()
        .rfind(|(a, _)| sget(a, "url") == cart_url)
    else {
        return calls(n, "browser_navigate", json!({"url": cart_url}));
    };
    let cart_pos = pos(t, cart_msg);
    let in_cart = snapshot_text(cart_msg).contains(&top.title);
    let mut colour = String::new();
    if !in_cart {
        // the product from the results page, by its title (a fixed selector for Kaya's product links)
        let Some((_, nav)) = calls_of(t, "browser_navigate")
            .into_iter()
            .rfind(|(a, m)| sget(a, "url") == url && pos(t, m) > cart_pos)
        else {
            return calls(n, "browser_navigate", json!({"url": url}));
        };
        let nav_pos = pos(t, nav);
        let Some((_, open)) = calls_of(t, "browser_fill_form")
            .into_iter()
            .rfind(|(a, m)| sget(a, "form") == "kaya-results" && pos(t, m) > nav_pos)
        else {
            return calls(
                n,
                "browser_fill_form",
                json!({"form": "kaya-results", "fills": [{"op": "click", "sel": "a[href*='kaya_item.html']", "text": top.title, "field": "product"}]}),
            );
        };
        let open_pos = pos(t, open);
        let Some((_, added)) = calls_of(t, "browser_fill_form")
            .into_iter()
            .rfind(|(a, m)| sget(a, "form") == "kaya-item" && pos(t, m) > open_pos)
        else {
            let mut fills: Vec<Value> = Vec::new();
            if let Some(c) = colour_in(&text) {
                fills
                    .push(json!({"op": "select", "sel": "#colour", "value": c, "field": "colour"}));
            }
            fills.push(json!({"op": "click", "sel": "#add", "field": "add_to_cart"}));
            return calls(
                n,
                "browser_fill_form",
                json!({"form": "kaya-item", "fills": fills}),
            );
        };
        // the cart count, verified on the page
        let added_pos = pos(t, added);
        let Some((_, snap)) = calls_of(t, "browser_snapshot")
            .into_iter()
            .find(|(_, m)| pos(t, m) > added_pos)
        else {
            return calls(n, "browser_snapshot", json!({"max_chars": 4000}));
        };
        let page = snapshot_text(snap);
        static COUNT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"Cart \((\d+)\)").unwrap());
        static ADDED: LazyLock<Regex> =
            LazyLock::new(|| Regex::new(r"Added to Cart: .*\(([^)]+)\)").unwrap());
        let count: u32 = COUNT
            .captures(&page)
            .and_then(|c| c[1].parse().ok())
            .unwrap_or(0);
        colour = ADDED
            .captures(&page)
            .map(|c| c[1].to_string())
            .unwrap_or_default();
        if count == 0 {
            return Step::Final(format!(
                "I pressed Add to Cart for **{}** but the cart still shows 0 items; open the browser tab to check.",
                top.title
            ));
        }
        if !buy {
            return Step::Final(format!(
                "**{}**{} — {} ({}) is in your Kaya Mobile cart ✓ (cart: {count} item{}; not checked out). Link: {url}",
                top.title,
                if colour.is_empty() { String::new() } else { format!(" ({colour})") },
                top.price_text,
                top.rating_text,
                if count == 1 { "" } else { "s" }
            ));
        }
    } else if !buy {
        return Step::Final(format!(
            "**{}** — {} is already in your Kaya Mobile cart ✓ (not checked out).",
            top.title, top.price_text
        ));
    }
    let item = Item {
        name: if colour.is_empty() {
            top.title.clone()
        } else {
            format!("{} ({colour})", top.title)
        },
        url: format!("{origin}/kaya_checkout.html"),
        total: top.price,
        currency: currency_in(&top.price_text),
        charged: true,
        delivery,
        note: String::new(),
        lines: vec![format!("Delivery: home delivery to {}", guest.address())],
    };
    book(spec, t, w, n, &item, &Req::default())
}

fn colour_in(text: &str) -> Option<String> {
    static COLOUR: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?i)\b(black|white|blue|navy|red|pink|green|olive|brown|tan|gold|silver|grey|gray|clear|beige|purple)\b(?:\s+(?:one|colour|color|version))?")
            .unwrap()
    });
    static ANY: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)\bany colou?r\b").unwrap());
    if ANY.is_match(text) {
        return None;
    }
    COLOUR.captures(text).map(|c| c[1].to_string())
}

// ---------------------------------------------------------------- the extraction call (the last resort)

/// The extraction call's instruction when the regexes did not find a search parameter: JSON only.
pub const EXTRACT_SYSTEM: &str = "Read the user's words below (the first line is today's date) and return JSON only, exactly these keys: \
{\"origin\": \"\", \"destination\": \"\", \"city\": \"\", \"date\": \"YYYY-MM-DD\", \"return_date\": \"YYYY-MM-DD\", \"check_in\": \"YYYY-MM-DD\", \
\"check_out\": \"YYYY-MM-DD\", \"guests\": \"\", \"time\": \"HH:MM\", \"cuisine\": \"\"}. \
origin and destination are the cities of a flight; city where a hotel or restaurant is wanted; date the departure or the restaurant's day, \
return_date a flight's return, check_in / check_out a hotel's nights (relative days computed from today's date); guests the number of people. \
Leave a value empty when the words do not give it. Never invent.";

/// The facts the extraction call returned, as the transcript records them (a `booking_facts` tool exchange).
pub fn extract_exchange(n: usize, text: &str, facts: &Value) -> Vec<Value> {
    let id = format!("bkg_{n}");
    let mut m = Map::new();
    for k in [
        "origin",
        "destination",
        "city",
        "date",
        "return_date",
        "check_in",
        "check_out",
        "guests",
        "time",
        "cuisine",
    ] {
        m.insert(k.into(), json!(get_str(facts, k)));
    }
    vec![
        json!({"role": "assistant", "content": "", "tool_calls": [{"id": id, "type": "function",
               "function": {"name": "booking_facts", "arguments": json!({"chars": text.chars().count()}).to_string()}}]}),
        json!({"role": "tool", "tool_call_id": id, "content": Value::Object(m).to_string()}),
    ]
}

#[cfg(test)]
mod tests;
