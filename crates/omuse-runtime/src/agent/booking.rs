//! The booking and checkout machine behind the workflow tools `travel_search`, `travel_book` and `shop_order`
//! (docs/json-tools.md; the protocol is [`super::workflow`]).
//!
//! The model reads the user's words and calls a tool with JSON arguments (the kind, the places, the dates resolved to
//! YYYY-MM-DD, the party, the criteria, the pick from a choice card, the contact details the user gave …). [`check`]
//! validates them in code — a date parses and is not past, a city code is in the table or IATA-shaped, a party is
//! 1–9, an address the results are mailed to is one the user wrote — and returns the arguments to run with, the
//! problems the model can fix, or the fields only the user can give (asked before any page opens). Nothing here
//! reads the user's wording.
//!
//! Then every step is code ([`next`]): ONE search call (on the user's travel connector when the arguments say so, the
//! connector once more when the native search had nothing priced), the pick by rule from the structured result (the
//! cheapest, or the criteria in the arguments, or the user's pick), the choice card with the pick shown, and — for
//! `travel_book` / `shop_order` buy — `purchase_confirm`, the booking link, the page filled by a form function with
//! FIXED input ids, the card from the Vault by the site's fixed labels, the hand-over for the final Pay / Confirm /
//! Place order (the user's) and the reference read from the confirmation page's fixed pattern. A booking or checkout
//! page with no form function here is opened and handed to the `browser-use` skill (`status: browser_use`).
//!
//! The machine ends with [`Step::Done`]: a JSON result (status, options, the pick and its rule, the booking, links,
//! the mail sent, `must_say` — the facts the reply must carry). The model writes the reply from it; no answer text is
//! written here.
//!
//! As in [`super::driver`], the next step is a function of the validated arguments and the transcript after the tool
//! call alone, so a run suspended for a takeover carries on where it was.

use std::sync::LazyLock;

use chrono::NaiveDate;
use omuse_common::util::{get_str, take_chars, truthy};
use regex::Regex;
use serde_json::{json, Map, Value};

use super::claims::{money, pick_card, vault_cards, VaultCard};
use super::driver::{handed_back, json_of, pos, snapshot_text, Step};
use super::tools::fn_;
use super::util::{content_of, sget, stored_call, tool_calls};
use super::workflow::{self, Checked};
use crate::forms::{self, Op};

/// The skill the machine hands an unknown booking or checkout page to (a model-driven fill of that one page).
pub const BROWSER_USE: &str = "browser-use";

/// The id prefix of the machine's code calls.
pub const PREFIX: &str = "bkg_";

/// What a spec drives.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Flight,
    Hotel,
    Restaurant,
    Michelin,
    Shop,
}

impl Kind {
    fn parse(s: &str) -> Option<Kind> {
        match s.trim().to_lowercase().as_str() {
            "flight" | "flights" => Some(Kind::Flight),
            "hotel" | "hotels" => Some(Kind::Hotel),
            "restaurant" | "restaurants" => Some(Kind::Restaurant),
            "michelin" => Some(Kind::Michelin),
            _ => None,
        }
    }

    fn name(self) -> &'static str {
        match self {
            Kind::Flight => "flight",
            Kind::Hotel => "hotel",
            Kind::Restaurant => "restaurant",
            Kind::Michelin => "michelin",
            Kind::Shop => "shop",
        }
    }
}

/// One booking skill: what its text used to tell the model, as data.
pub struct BookingSpec {
    pub skill: &'static str,
    pub kind: Kind,
    /// what is booked
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

pub fn spec_of(kind: Kind) -> &'static BookingSpec {
    SPECS.iter().find(|s| s.kind == kind).unwrap_or(&SPECS[0])
}

// ---------------------------------------------------------------- the workflow tools

pub const TRAVEL_SEARCH: &str = "travel_search";
pub const TRAVEL_BOOK: &str = "travel_book";
pub const SHOP_ORDER: &str = "shop_order";
/// The workflow tools this machine runs.
pub const TOOLS: [&str; 3] = [TRAVEL_SEARCH, TRAVEL_BOOK, SHOP_ORDER];

/// The catalog entries of the workflow tools (JSON schemas; the descriptions say what the tool does).
pub fn tool_schemas() -> Vec<Value> {
    let s = json!({"type": "string"});
    let date = json!({"type": "string", "description": "YYYY-MM-DD (resolve relative dates from today in the live state)"});
    let strs = json!({"type": "array", "items": {"type": "string"}});
    let mut travel = json!({
        "kind": {"type": "string", "enum": ["flight", "hotel", "michelin", "restaurant"]},
        "origin": {"type": "string", "description": "flight: city or airport code (TYO, SIN, NRT) or a city name; empty = the user's home city"},
        "destination": {"type": "string", "description": "flight: city or airport code or a city name"},
        "city": {"type": "string", "description": "hotel / restaurant: the city"},
        "area": {"type": "string", "description": "hotel / restaurant: the area or landmark the user named (\"Ginza\")"},
        "date": {"type": "string", "description": "YYYY-MM-DD: the departure day, the check-in day or the restaurant's day"},
        "date_to": {"type": "string", "description": "flight: the last departure day when any day of a range will do (one search covers it)"},
        "return_date": date.clone(),
        "check_out": date,
        "party": {"type": "integer", "description": "adults / guests / diners when the user said"},
        "cabin": {"type": "string", "enum": ["economy", "premium_economy", "business", "first"]},
        "nonstop": {"type": "boolean"},
        "max_price": {"type": "number", "description": "the user's budget: per adult (flight), per night (hotel)"},
        "min_stars": {"type": "integer", "description": "hotel stars"},
        "min_rating": {"type": "number"},
        "free_cancellation": {"type": "boolean"},
        "any_terms": {"type": "boolean", "description": "the cheapest rate whatever its conditions (a non-refundable rate is fine)"},
        "time": {"type": "string", "description": "restaurant: HH:MM"},
        "distinction": {"type": "string", "enum": ["3 stars", "2 stars", "1 star", "stars", "bib gourmand"]},
        "cuisine": s.clone(),
        "requests": {"type": "string", "description": "restaurant: dietary needs and the occasion, for the booking's requests field"},
        "best": {"type": "boolean", "description": "the best (highest distinction / rating) rather than the cheapest"},
        "airlines": {"type": "array", "items": {"type": "string"}, "description": "airlines or alliances the user prefers (names, 2-letter codes, Star Alliance …)"},
        "brands": {"type": "array", "items": {"type": "string"}, "description": "hotel brands or groups the user prefers (Holiday Inn, IHG …)"},
        "all_airlines": {"type": "boolean", "description": "the user asked for every airline (no standing preference)"},
        "named": {"type": "array", "items": {"type": "string"}, "description": "options the user named (a hotel, an airline, a flight number)"},
        "contact": {"type": "object", "description": "contact details the user gave in the chat", "properties": {"name": s.clone(), "email": s.clone(), "phone": s.clone()}},
        "mail_results_to": {"type": "string", "description": "an address the user asked the results to be mailed to"},
        "use_connector": {"type": "boolean", "description": "the user named their travel agency / connector to search and book with"},
        "card": {"type": "string", "description": "the Vault card the user named to pay with"},
        "next": {"type": "array", "items": {"type": "string", "enum": ["flight", "hotel", "car", "restaurant"]}, "description": "the other searches the same request asks for (offered after this one)"}
    });
    let search = fn_(
        TRAVEL_SEARCH,
        "Search and compare flights, hotels, MICHELIN restaurants or restaurants. Code runs ONE search with these arguments, \
picks by rule (the cheapest, or the user's criteria), shows the options as choice cards and returns the result as JSON \
(options, the pick and its rule, links, must_say). Nothing is booked. Write the reply from the result, with every fact in \
its must_say. status needs_input: ask the user for the missing fields, then call again with every field; invalid: fix \
the arguments and call again.",
        travel.clone(),
        &["kind"],
    );
    travel["pick"] = json!({"type": "string", "description": "the label of the option the user chose (a choice card's \"I choose: …\"); empty = the pick by rule"});
    let book = fn_(
        TRAVEL_BOOK,
        "Book a flight, a hotel room or a table — only when the user asks to book. Same arguments as travel_search, plus \
pick. Code searches, takes the pick (or picks by rule), asks the user to approve the payment, opens the booking page, \
fills it from the profile and the Vault and hands the final Pay / Confirm click to the user; the JSON result has the \
booking reference (status booked) or says why not. Write the reply from it, with every fact in its must_say.",
        travel,
        &["kind"],
    );
    let shop = fn_(
        SHOP_ORDER,
        "Compare, add to the cart or buy on a shop whose link the user gave. Code reads the shop's results page, ranks the \
listings by the criteria, shows them as choice cards, and for cart / buy adds the item and fills the checkout from the \
profile and the Vault (Place order is the user's). A shop code has no form for is opened and handed to the browser-use \
skill (status browser_use). Write the reply from the JSON result, with every fact in its must_say.",
        json!({
            "url": {"type": "string", "description": "the shop's link the user gave (a results or product page)"},
            "action": {"type": "string", "enum": ["compare", "cart", "buy"]},
            "by": {"type": "string", "enum": ["rating", "price", "ratings"], "description": "best-rated, cheapest or most-reviewed first"},
            "max_price": {"type": "number"}, "min_price": {"type": "number"}, "min_stars": {"type": "number"},
            "min_ratings": {"type": "integer"}, "count": {"type": "integer", "description": "how many to show"},
            "title_has": {"type": "array", "items": s.clone(), "description": "words every chosen name has, e.g. [\"case\", \"iPhone 17 Pro Max\"]"},
            "title_not": strs,
            "pick": {"type": "string", "description": "the listing the user chose (a choice card's \"I choose: …\")"},
            "let_user_choose": {"type": "boolean", "description": "the user wants to pick from the options themselves"},
            "colour": s.clone(),
            "quantity": {"type": "integer"},
            "contact": {"type": "object", "properties": {"name": s.clone(), "phone": s.clone(), "address": s.clone()}},
            "card": s
        }),
        &["url", "action"],
    );
    vec![search, book, shop]
}

/// `ask_user` in a booking skill's run before its workflow tool was called: refused, as data. What the user must give is
/// the tool's `needs_input`; what has a default (a table for 2 at 19:00, 1 adult, economy) is code's — so the tool is
/// called first. `events` are the task's.
pub fn ask_before_tool(events: &[Value]) -> Option<Value> {
    let skill = events
        .iter()
        .filter(|e| get_str(e, "type") == "tool_call")
        .filter(|e| get_str(&e["data"], "name") == "load_skill")
        .map(|e| get_str(&e["data"]["args"], "name"))
        .filter_map(|n| SPECS.iter().find(|s| s.skill == n))
        .next_back()?;
    let called = events.iter().any(|e| get_str(e, "type") == "workflow_call");
    if called {
        return None;
    }
    let tool = if skill.kind == Kind::Shop {
        SHOP_ORDER
    } else {
        TRAVEL_SEARCH
    };
    Some(json!({"refused": "call_the_tool_first", "tool": tool,
                "why": format!("the {} skill asks through {tool}: call it with what the user gave; its needs_input lists what only the user can give, and code fills the defaults", skill.skill)}))
}

// ---------------------------------------------------------------- validation (code, on the JSON)

/// What validation reads besides the arguments.
pub struct Ctx<'a> {
    /// the profile, with the connected mailbox's address as `mailbox` when one is connected
    pub profile: &'a Value,
    pub today: NaiveDate,
    /// the user's messages of this chat — only for the exact-substring check of an address the results are mailed to
    pub user_texts: &'a [String],
}

static EMAIL: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[\w.+-]+@[\w-]+(?:\.[\w-]+)+$").unwrap());
static CODES: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[A-Z]{3}(?:,[A-Z]{3})*$").unwrap());
static HHMM: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^([01]?\d|2[0-3]):([0-5]\d)$").unwrap());

/// Validates a workflow tool's arguments: the arguments to run with (normalised), the problems the model can fix, or
/// the fields only the user can give.
pub fn check(tool: &str, args: &Value, cx: &Ctx) -> Checked {
    match tool {
        SHOP_ORDER => check_shop(args),
        TRAVEL_BOOK => check_travel(args, cx, true),
        _ => check_travel(args, cx, false),
    }
}

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

fn b_arg(a: &Value, k: &str) -> bool {
    a.get(k).is_some_and(truthy)
}

fn list_arg(a: &Value, k: &str) -> Vec<String> {
    match a.get(k) {
        Some(Value::Array(v)) => v
            .iter()
            .filter_map(Value::as_str)
            .map(|x| x.trim().to_string())
            .filter(|x| !x.is_empty())
            .collect(),
        Some(Value::String(s)) if !s.trim().is_empty() => s
            .split(',')
            .map(|x| x.trim().to_string())
            .filter(|x| !x.is_empty())
            .collect(),
        _ => Vec::new(),
    }
}

fn check_travel(a: &Value, cx: &Ctx, book: bool) -> Checked {
    let Some(kind) = Kind::parse(&s_arg(a, "kind")) else {
        return Checked::Invalid(vec![
            "kind: one of flight, hotel, michelin, restaurant".into()
        ]);
    };
    let mut problems: Vec<String> = Vec::new();
    let mut missing: Vec<String> = Vec::new();
    let mut out = Map::new();
    out.insert("kind".into(), json!(kind.name()));
    let date = |k: &str, problems: &mut Vec<String>| -> Option<NaiveDate> {
        let v = s_arg(a, k);
        if v.is_empty() {
            return None;
        }
        match NaiveDate::parse_from_str(&v, "%Y-%m-%d") {
            Ok(d) if d < cx.today => {
                problems.push(format!(
                    "{k}: {v} is in the past (today is {})",
                    iso(cx.today)
                ));
                None
            }
            Ok(d) => Some(d),
            Err(_) => {
                problems.push(format!(
                    "{k}: \"{v}\" is not a date: give YYYY-MM-DD (resolve relative dates from today, {})",
                    iso(cx.today)
                ));
                None
            }
        }
    };
    let d = date("date", &mut problems);
    let date_to = date("date_to", &mut problems);
    let ret = date("return_date", &mut problems);
    let out_d = date("check_out", &mut problems);
    let after = |k: &str, x: Option<NaiveDate>, problems: &mut Vec<String>| match (d, x) {
        (Some(a), Some(b)) if b <= a => {
            problems.push(format!("{k}: {} is not after date {}", iso(b), iso(a)));
            None
        }
        (_, x) => x,
    };
    match kind {
        Kind::Flight => {
            let place = |k: &str, problems: &mut Vec<String>| -> Option<String> {
                let v = s_arg(a, k);
                if v.is_empty() {
                    return None;
                }
                let code = city_code(&v);
                if code.is_none() {
                    problems.push(format!(
                        "{k}: \"{v}\" is no city of the table and no IATA code: give the city or airport code (e.g. TYO, SIN, NRT)"
                    ));
                }
                code
            };
            let dest = place("destination", &mut problems);
            let origin = place("origin", &mut problems).or_else(|| {
                // the user's home city (the profile's home address), when it is not the destination
                home_city(cx.profile)
                    .map(|c| c.code.to_string())
                    .filter(|c| dest.as_deref() != Some(c.as_str()))
            });
            match origin {
                Some(o) => {
                    out.insert("origin".into(), json!(o));
                }
                None if s_arg(a, "origin").is_empty() => missing.push("origin".into()),
                None => {}
            }
            match dest {
                Some(x) => {
                    out.insert("destination".into(), json!(x));
                }
                None if s_arg(a, "destination").is_empty() => missing.push("destination".into()),
                None => {}
            }
            if d.is_none() && s_arg(a, "date").is_empty() {
                missing.push("date".into());
            }
            if let Some(x) = after("return_date", ret, &mut problems) {
                out.insert("return_date".into(), json!(iso(x)));
            }
            if let Some(x) = after("date_to", date_to, &mut problems) {
                out.insert("date_to".into(), json!(iso(x)));
            }
            let cabin = s_arg(a, "cabin").to_lowercase().replace([' ', '-'], "_");
            match cabin.as_str() {
                "" => {
                    out.insert("cabin".into(), json!("economy"));
                }
                "economy" | "premium_economy" | "business" | "first" => {
                    out.insert("cabin".into(), json!(cabin));
                }
                _ => problems.push(format!(
                    "cabin: \"{cabin}\" is not one of economy, premium_economy, business, first"
                )),
            }
            out.insert("nonstop".into(), json!(b_arg(a, "nonstop")));
            // the airlines in the request, else the profile's standing preference (unless every airline is asked for)
            let mut airlines = list_arg(a, "airlines");
            if airlines.is_empty() && !b_arg(a, "all_airlines") {
                airlines = get_str(cx.profile, "preferred_airline")
                    .split([',', ';'])
                    .map(|x| x.trim().to_string())
                    .filter(|x| !x.is_empty())
                    .collect();
            }
            if !airlines.is_empty() {
                out.insert("airlines".into(), json!(airlines));
            }
            let origin = get_str(&Value::Object(out.clone()), "origin");
            out.insert("currency".into(), json!(home_currency(cx.profile, &origin)));
        }
        _ => {
            let city = s_arg(a, "city");
            if city.is_empty() {
                missing.push("city".into());
            } else {
                let name = city_code(&city)
                    .and_then(|code| CITIES.iter().find(|c| c.code == code))
                    .map(|c| c.name.to_string())
                    .unwrap_or(city);
                out.insert("city".into(), json!(name));
            }
            let area = s_arg(a, "area");
            if !area.is_empty() {
                out.insert("area".into(), json!(area));
            }
            match kind {
                Kind::Hotel => {
                    if d.is_none() && s_arg(a, "date").is_empty() {
                        missing.push("date (check-in)".into());
                    }
                    match after("check_out", out_d, &mut problems) {
                        Some(x) => {
                            out.insert("check_out".into(), json!(iso(x)));
                        }
                        None if s_arg(a, "check_out").is_empty() => {
                            missing.push("check_out".into())
                        }
                        None => {}
                    }
                }
                Kind::Michelin if d.is_none() && s_arg(a, "date").is_empty() => {
                    missing.push("date".into());
                }
                _ => {}
            }
            out.insert("currency".into(), json!(home_currency(cx.profile, "")));
        }
    }
    if let Some(x) = d {
        out.insert("date".into(), json!(iso(x)));
    }
    // the party: 1–9 (a 0 the model sent was already made 1 where tool calls are parsed)
    if let Some(p) = n_arg(a, "party") {
        if p.fract() != 0.0 || !(1.0..=9.0).contains(&p) {
            problems.push(format!("party: {p} — give 1 to 9 people"));
        } else {
            out.insert("party".into(), json!(p as u32));
        }
    }
    if let Some(p) = n_arg(a, "max_price").filter(|p| *p >= 1.0) {
        out.insert("max_price".into(), json!(p));
    }
    if let Some(s) = n_arg(a, "min_stars").filter(|s| (1.0..=5.0).contains(s)) {
        out.insert("min_stars".into(), json!(s as u32));
    }
    if let Some(r) = n_arg(a, "min_rating").filter(|r| *r > 0.0 && *r <= 10.0) {
        out.insert("min_rating".into(), json!(r));
    }
    for k in ["free_cancellation", "any_terms", "best", "use_connector"] {
        if b_arg(a, k) {
            out.insert(k.into(), json!(true));
        }
    }
    let table = matches!(kind, Kind::Restaurant | Kind::Michelin);
    let time = if table {
        s_arg(a, "time")
    } else {
        String::new()
    };
    if !time.is_empty() {
        match HHMM.captures(&time) {
            Some(c) => {
                out.insert(
                    "time".into(),
                    json!(format!("{:02}:{}", c[1].parse::<u32>().unwrap_or(0), &c[2])),
                );
            }
            None => problems.push(format!("time: \"{time}\" — give HH:MM (24-hour)")),
        }
    } else if table {
        out.insert("time".into(), json!("19:00"));
    }
    let dist = if kind == Kind::Michelin {
        s_arg(a, "distinction").to_lowercase()
    } else {
        String::new()
    };
    match dist.as_str() {
        "" => {}
        "3 stars" | "2 stars" | "1 star" | "stars" | "bib gourmand" => {
            out.insert("distinction".into(), json!(dist));
        }
        _ => problems.push(format!(
            "distinction: \"{dist}\" is not one of 3 stars, 2 stars, 1 star, stars, bib gourmand"
        )),
    }
    for k in ["cuisine", "requests", "card", "pick"] {
        let v = s_arg(a, k);
        if !v.is_empty() && (k != "pick" || book) {
            out.insert(
                k.into(),
                json!(if k == "cuisine" { v.to_lowercase() } else { v }),
            );
        }
    }
    for k in ["named", "brands"] {
        let v = list_arg(a, k);
        if !v.is_empty() && (k != "brands" || kind == Kind::Hotel) {
            out.insert(k.into(), json!(v));
        }
    }
    let next: Vec<String> = list_arg(a, "next")
        .into_iter()
        .map(|x| x.to_lowercase())
        .filter(|x| {
            ["flight", "hotel", "car", "restaurant"].contains(&x.as_str()) && x != kind.name()
        })
        .collect();
    if !next.is_empty() {
        out.insert("next".into(), json!(next));
    }
    // contact details the user gave: an email that is no address is dropped
    if let Some(c) = a.get("contact").filter(|c| c.is_object()) {
        let mut cm = Map::new();
        for k in ["name", "email", "phone"] {
            let v = s_arg(c, k);
            if !v.is_empty() && (k != "email" || EMAIL.is_match(&v)) {
                cm.insert(k.into(), json!(v));
            }
        }
        if !cm.is_empty() {
            out.insert("contact".into(), Value::Object(cm));
        }
    }
    // an address the results are mailed to must be one the user wrote (an exact substring check, not parsing)
    let to = s_arg(a, "mail_results_to");
    if !to.is_empty() {
        let said = cx
            .user_texts
            .iter()
            .any(|t| t.to_lowercase().contains(&to.to_lowercase()));
        if !EMAIL.is_match(&to) {
            problems.push(format!("mail_results_to: \"{to}\" is not an email address"));
        } else if !said {
            problems.push(format!(
                "mail_results_to: {to} is not in the user's messages — only an address the user wrote"
            ));
        } else {
            out.insert("mail_results_to".into(), json!(to));
        }
    }
    // fields of another kind (a model that fills every field of the schema) are dropped
    let drop: &[&str] = match kind {
        Kind::Flight => &[
            "min_stars",
            "free_cancellation",
            "any_terms",
            "time",
            "distinction",
            "cuisine",
            "requests",
            "brands",
            "city",
            "area",
        ],
        Kind::Hotel => &[
            "time",
            "distinction",
            "cuisine",
            "requests",
            "airlines",
            "date_to",
            "return_date",
        ],
        Kind::Michelin | Kind::Restaurant => &[
            "min_stars",
            "free_cancellation",
            "any_terms",
            "airlines",
            "date_to",
            "return_date",
            "check_out",
            "brands",
        ],
        Kind::Shop => &[],
    };
    for k in drop {
        out.remove(*k);
    }
    if kind != Kind::Michelin {
        out.remove("distinction");
    }
    if !problems.is_empty() {
        return Checked::Invalid(problems);
    }
    if !missing.is_empty() {
        return Checked::NeedsInput(missing);
    }
    out.insert("book".into(), json!(book));
    Checked::Valid(Value::Object(out))
}

fn check_shop(a: &Value) -> Checked {
    let mut problems = Vec::new();
    let mut out = Map::new();
    let url = s_arg(a, "url");
    if url.is_empty() {
        return Checked::NeedsInput(vec!["url (the shop's link)".into()]);
    }
    match url::Url::parse(&url) {
        Ok(u) if u.scheme() == "http" || u.scheme() == "https" => {
            out.insert("url".into(), json!(url));
        }
        _ => problems.push(format!("url: \"{url}\" is not a web address")),
    }
    let action = s_arg(a, "action").to_lowercase();
    match action.as_str() {
        "compare" | "cart" | "buy" => {
            out.insert("action".into(), json!(action));
        }
        "" => {
            out.insert("action".into(), json!("compare"));
        }
        _ => problems.push(format!(
            "action: \"{action}\" is not one of compare, cart, buy"
        )),
    }
    let by = s_arg(a, "by").to_lowercase();
    match by.as_str() {
        "" => {
            out.insert("by".into(), json!("rating"));
        }
        "rating" | "price" | "ratings" => {
            out.insert("by".into(), json!(by));
        }
        _ => problems.push(format!("by: \"{by}\" is not one of rating, price, ratings")),
    }
    for k in ["max_price", "min_price", "min_stars"] {
        if let Some(x) = n_arg(a, k).filter(|x| *x > 0.0) {
            out.insert(k.into(), json!(x));
        }
    }
    for k in ["min_ratings", "count"] {
        if let Some(x) = n_arg(a, k).filter(|x| *x >= 1.0) {
            out.insert(k.into(), json!(x as u64));
        }
    }
    match n_arg(a, "quantity") {
        None => {
            out.insert("quantity".into(), json!(1));
        }
        Some(q) if q.fract() == 0.0 && (1.0..=10.0).contains(&q) => {
            out.insert("quantity".into(), json!(q as u32));
        }
        Some(q) => problems.push(format!("quantity: {q} — give 1 to 10")),
    }
    for k in ["title_has", "title_not"] {
        let v = list_arg(a, k);
        if !v.is_empty() {
            out.insert(k.into(), json!(v));
        }
    }
    for k in ["pick", "colour", "card"] {
        let v = s_arg(a, k);
        if !v.is_empty() {
            out.insert(k.into(), json!(v));
        }
    }
    if b_arg(a, "let_user_choose") {
        out.insert("let_user_choose".into(), json!(true));
    }
    if let Some(c) = a.get("contact").filter(|c| c.is_object()) {
        let mut cm = Map::new();
        for k in ["name", "phone", "address"] {
            let v = s_arg(c, k);
            if !v.is_empty() {
                cm.insert(k.into(), json!(v));
            }
        }
        if !cm.is_empty() {
            out.insert("contact".into(), Value::Object(cm));
        }
    }
    if !problems.is_empty() {
        return Checked::Invalid(problems);
    }
    Checked::Valid(Value::Object(out))
}

// ---------------------------------------------------------------- cities

struct City {
    code: &'static str,
    name: &'static str,
    words: &'static [&'static str],
}

#[rustfmt::skip]
const CITIES: &[City] = &[
    City { code: "TYO", name: "Tokyo", words: &["tokyo", "东京", "東京"] },
    City { code: "OSA", name: "Osaka", words: &["osaka", "大阪"] },
    City { code: "SEL", name: "Seoul", words: &["seoul", "首尔", "首爾"] },
    City { code: "SIN", name: "Singapore", words: &["singapore", "新加坡"] },
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
    City { code: "SGN", name: "Ho Chi Minh City", words: &["ho chi minh city", "ho chi minh", "saigon", "胡志明"] },
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

/// A flight's place as the search takes it: an IATA-shaped code (or a comma list of them) as it is, a city of the
/// table by its whole name (any case) as its code; anything else `None`. The value is the model's structured argument —
/// it is compared whole, never searched in prose.
fn city_code(v: &str) -> Option<String> {
    let t = v.trim();
    let up = t.to_uppercase().replace(' ', "");
    if CODES.is_match(&up) {
        return Some(up);
    }
    let lc = t.to_lowercase();
    CITIES
        .iter()
        .find(|c| c.name.to_lowercase() == lc || c.words.contains(&lc.as_str()))
        .map(|c| c.code.to_string())
}

/// Every city of the table named in a profile address (a structured field of the user's profile, not chat words).
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
                if !w.is_ascii()
                    || (!before.is_some_and(char::is_alphanumeric)
                        && !after.is_some_and(char::is_alphanumeric))
                {
                    out.push((at, c));
                }
                from = end;
            }
        }
    }
    out.sort_by_key(|(i, _)| *i);
    out.dedup_by(|a, b| a.1.code == b.1.code);
    out
}

/// The user's home city: the first city of the table in the profile's home address.
fn home_city(profile: &Value) -> Option<&'static City> {
    cities_in(&get_str(profile, "address_home"))
        .first()
        .map(|(_, c)| *c)
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
    if let Some(c) = home_city(profile) {
        return city_currency(c.code).to_string();
    }
    let lc = get_str(profile, "address_home").to_lowercase();
    if let Some((_, cur)) = COUNTRY.iter().find(|(w, _)| has_word(&lc, w)) {
        return cur.to_string();
    }
    city_currency(origin).to_string()
}

fn iso(d: NaiveDate) -> String {
    d.format("%Y-%m-%d").to_string()
}

fn long(d: NaiveDate) -> String {
    d.format("%A %-d %B %Y").to_string()
}

// ---------------------------------------------------------------- the request, from the validated arguments

/// The validated arguments of `travel_search` / `travel_book`, typed.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Req {
    pub kind: Option<Kind>,
    /// city or airport codes for a flight
    pub origin: String,
    pub destination: String,
    /// the city's name for a hotel or a restaurant
    pub city: String,
    pub area: String,
    pub date: Option<NaiveDate>,
    /// the last departure day of a range: ONE search over the days
    pub date_to: Option<NaiveDate>,
    pub return_date: Option<NaiveDate>,
    pub check_out: Option<NaiveDate>,
    pub party: Option<u32>,
    pub cabin: String,
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
    pub requests: String,
    pub best: bool,
    /// the airlines the search is limited to (the request's, else the profile's standing preference)
    pub airlines: Vec<String>,
    /// options the user named
    pub named: Vec<String>,
    /// hotel brands the search is limited to
    pub brands: Vec<String>,
    /// the user's home currency
    pub currency: String,
    pub contact: Value,
    pub mail_to: String,
    pub use_connector: bool,
    pub card: String,
    /// the label the user picked on a choice card
    pub pick: String,
    /// the other searches the request asks for
    pub next: Vec<String>,
    /// `travel_book`: book the pick
    pub book: bool,
}

impl Req {
    /// The request from `check`'s validated arguments.
    pub fn from_args(a: &Value) -> Req {
        let d = |k: &str| NaiveDate::parse_from_str(&s_arg(a, k), "%Y-%m-%d").ok();
        Req {
            kind: Kind::parse(&s_arg(a, "kind")),
            origin: s_arg(a, "origin"),
            destination: s_arg(a, "destination"),
            city: s_arg(a, "city"),
            area: s_arg(a, "area"),
            date: d("date"),
            date_to: d("date_to"),
            return_date: d("return_date"),
            check_out: d("check_out"),
            party: n_arg(a, "party").map(|p| p as u32),
            cabin: {
                let c = s_arg(a, "cabin");
                if c.is_empty() {
                    "economy".into()
                } else {
                    c
                }
            },
            nonstop: b_arg(a, "nonstop"),
            max_price: n_arg(a, "max_price"),
            min_stars: n_arg(a, "min_stars").map(|s| s as u32),
            min_rating: n_arg(a, "min_rating"),
            free_cancel: b_arg(a, "free_cancellation"),
            any_terms: b_arg(a, "any_terms"),
            time: s_arg(a, "time"),
            distinction: s_arg(a, "distinction"),
            cuisine: s_arg(a, "cuisine"),
            requests: s_arg(a, "requests"),
            best: b_arg(a, "best"),
            airlines: list_arg(a, "airlines"),
            named: list_arg(a, "named"),
            brands: list_arg(a, "brands"),
            currency: s_arg(a, "currency"),
            contact: a.get("contact").cloned().unwrap_or(Value::Null),
            mail_to: s_arg(a, "mail_results_to"),
            use_connector: b_arg(a, "use_connector"),
            card: s_arg(a, "card"),
            pick: s_arg(a, "pick"),
            next: list_arg(a, "next"),
            book: b_arg(a, "book"),
        }
    }

    fn kind(&self) -> Kind {
        self.kind.unwrap_or(Kind::Flight)
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
                if !self.brands.is_empty() {
                    a["brands"] = json!(self.brands);
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

    /// What was searched, as the result reports it.
    fn searched(&self, kind: Kind) -> Value {
        let mut o = Map::new();
        let mut put = |k: &str, v: Value| {
            let empty = matches!(&v, Value::Null) || v.as_str().is_some_and(str::is_empty);
            if !empty {
                o.insert(k.into(), v);
            }
        };
        put("kind", json!(kind.name()));
        match kind {
            Kind::Flight => {
                put("origin", json!(self.origin));
                put("destination", json!(self.destination));
                put("date", json!(self.date.map(iso)));
                put("date_to", json!(self.date_to.map(iso)));
                put("return_date", json!(self.return_date.map(iso)));
                put("cabin", json!(self.cabin));
                put("adults", json!(self.party_or(kind)));
                put("nonstop", json!(self.nonstop));
                put("max_price", json!(self.max_price));
                put("currency", json!(self.currency));
                if !self.airlines.is_empty() {
                    put("airlines", json!(self.airlines));
                }
            }
            Kind::Hotel => {
                put("city", json!(self.city));
                put("area", json!(self.area));
                put("check_in", json!(self.date.map(iso)));
                put("check_out", json!(self.check_out.map(iso)));
                if let (Some(a), Some(b)) = (self.date, self.check_out) {
                    put("nights", json!((b - a).num_days()));
                }
                put("guests", json!(self.party_or(kind)));
                put("max_price_per_night", json!(self.max_price));
            }
            _ => {
                put("city", json!(self.city));
                put("area", json!(self.area));
                put("cuisine", json!(self.cuisine));
                put("distinction", json!(self.distinction));
                put("date", json!(self.date.map(iso)));
                put("time", json!(self.time));
                put("party", json!(self.party_or(kind)));
            }
        }
        Value::Object(o)
    }
}

/// The validated arguments of `shop_order`, typed.
#[derive(Debug, Clone, Default)]
pub struct ShopReq {
    pub url: String,
    /// "compare" | "cart" | "buy"
    pub action: String,
    pub ranking: crate::listings::Ranking,
    pub pick: String,
    pub let_user_choose: bool,
    pub colour: String,
    pub quantity: u32,
    pub contact: Value,
    pub card: String,
}

impl ShopReq {
    pub fn from_args(a: &Value) -> ShopReq {
        ShopReq {
            url: s_arg(a, "url"),
            action: s_arg(a, "action"),
            ranking: crate::listings::Ranking {
                by: {
                    let b = s_arg(a, "by");
                    if b.is_empty() {
                        "rating".into()
                    } else {
                        b
                    }
                },
                max_price: n_arg(a, "max_price"),
                min_price: n_arg(a, "min_price"),
                min_stars: n_arg(a, "min_stars"),
                min_ratings: n_arg(a, "min_ratings").map(|x| x as u64),
                title_has: list_arg(a, "title_has"),
                title_not: list_arg(a, "title_not"),
                count: n_arg(a, "count").map(|x| x as usize),
            },
            pick: s_arg(a, "pick"),
            let_user_choose: b_arg(a, "let_user_choose"),
            colour: s_arg(a, "colour"),
            quantity: n_arg(a, "quantity").map(|q| q as u32).unwrap_or(1).max(1),
            contact: a.get("contact").cloned().unwrap_or(Value::Null),
            card: s_arg(a, "card"),
        }
    }

    fn buy(&self) -> bool {
        self.action == "buy"
    }

    fn cart(&self) -> bool {
        self.action == "cart" || self.buy()
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

/// Whether one of the names the user named (the model's `named` argument) is this option: its whole name, its airline
/// (or the airline's code as a capital word), or a distinctive word of its name or area (never a city, never a generic
/// word like "Hotel").
fn mentioned(kind: Kind, o: &Offer, names: &[String]) -> bool {
    let text = names.join("\n");
    if text.trim().is_empty() {
        return false;
    }
    let lc = text.to_lowercase();
    if !o.label.is_empty() && lc.contains(&o.label.to_lowercase()) {
        return true;
    }
    if names
        .iter()
        .any(|n| !n.trim().is_empty() && o.label.to_lowercase().contains(&n.trim().to_lowercase()))
    {
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
                .is_match(&text)
        });
    }
    let city_word = |w: &str| {
        let wl = w.to_lowercase();
        CITIES
            .iter()
            .any(|c| c.words.contains(&wl.as_str()) || c.name.to_lowercase() == wl)
    };
    format!("{} {}", o.label, o.area)
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| {
            let wl = w.to_lowercase();
            w.chars().count() >= 4 && !GENERIC.contains(&wl.as_str()) && !city_word(w)
        })
        .any(|w| has_word(&lc, w))
}

/// The options that meet the request's criteria, best first, with the rule that ordered them (the first is the pick).
pub fn ranked(kind: Kind, req: &Req, offers: &[Offer]) -> (Vec<Offer>, String) {
    let mut keep: Vec<Offer> = offers.to_vec();
    let mut rule: Vec<String> = Vec::new();
    let named: Vec<Offer> = keep
        .iter()
        .filter(|o| mentioned(kind, o, &req.named))
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

/// The option the user picked on a choice card, by its label (the model's `pick` argument).
fn by_label<'a>(offers: &'a [Offer], label: &str) -> Option<&'a Offer> {
    let l = label.trim().to_lowercase();
    if l.is_empty() {
        return None;
    }
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

// ---------------------------------------------------------------- the result JSON

/// One option as the result lists it.
fn option_json(o: &Offer) -> Value {
    let mut m = Map::new();
    m.insert("label".into(), json!(o.label));
    if !o.details.is_empty() {
        m.insert("details".into(), json!(o.details));
    }
    if let Some(p) = o.price {
        m.insert("price".into(), json!(p));
    }
    if let Some(t) = o.total.filter(|t| Some(*t) != o.price) {
        m.insert("total".into(), json!(t));
    }
    let strs = [
        ("price_text", &o.price_text),
        ("currency", &o.currency),
        ("booking_url", &o.url),
        ("airline", &o.airline),
        ("area", &o.area),
        ("cancellation", &o.cancellation),
        ("distinction", &o.distinction),
        ("date", &o.date),
        ("data", &o.data),
    ];
    for (k, v) in strs {
        if !v.is_empty() {
            m.insert(k.into(), json!(v));
        }
    }
    if !o.book_at.is_empty() {
        m.insert("book_at".into(), json!(o.book_at));
    }
    if let Some(s) = o.stops {
        m.insert("stops".into(), json!(s));
    }
    if let Some(r) = o.rating {
        m.insert("rating".into(), json!(r));
    }
    if let Some(s) = o.stars {
        m.insert("stars".into(), json!(s));
    }
    if o.preferred {
        m.insert("preferred".into(), json!(true));
    }
    Value::Object(m)
}

/// The fact the reply must carry about one option: its name, its price as the result wrote it, its booking link.
fn offer_fact(o: &Offer) -> String {
    let price = if o.price_text.is_empty() {
        o.price
            .map(|p| format!("{} {}", o.currency, money(p).trim_end_matches(".00")))
            .unwrap_or_default()
    } else {
        o.price_text.clone()
    };
    let mut s = o.label.clone();
    if !price.trim().is_empty() {
        s = format!("{s} — {}", price.trim());
    }
    if !o.url.is_empty() {
        s = format!("{s} — book at {}", o.url);
    }
    if !o.book_at.is_empty() {
        s = format!("{s} (or on {})", o.book_at.join(" or "));
    }
    s
}

/// A web fallback's results (`{"results": [{"title", "url", "snippet"}]}`), the first four, with a price a page shows
/// ("from" prices, not quotes).
fn web_pages(result: &Value) -> Vec<Value> {
    static PRICE: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?:S\$|SGD|US\$|USD|HK\$|A\$|¥|JPY|€|EUR|£|GBP|\$)\s?\d[\d,]*(?:\.\d+)?")
            .unwrap()
    });
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
            let mut o = json!({"title": get_str(r, "title"), "url": url});
            if !snippet.is_empty() {
                o["snippet"] = json!(snippet);
            }
            let page = page_text(&url);
            if let Some(m) = PRICE.find(&snippet).or_else(|| PRICE.find(&page)) {
                o["price_from"] = json!(m.as_str());
            }
            o
        })
        .collect()
}

/// A Google Flights offer's booking options (`flight_offer`): who sells it, at what price.
fn sellers(v: &Value) -> Vec<Value> {
    v.get("options")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|o| !get_str(o, "book_with").is_empty())
        .map(|o| {
            let mut s = json!({"book_with": get_str(o, "book_with")});
            if let Some(p) = num_of(o.get("price")) {
                s["price"] = json!(p);
                s["currency"] = json!(get_str(o, "currency"));
            }
            let title = get_str(o, "title");
            if !title.is_empty() {
                s["title"] = json!(title);
            }
            s
        })
        .collect()
}

/// The card question of a search (the choice card's title).
fn choice_title(kind: Kind) -> &'static str {
    match kind {
        Kind::Flight => "Flights",
        Kind::Hotel => "Hotels",
        Kind::Michelin => "MICHELIN restaurants",
        Kind::Restaurant => "Restaurants",
        Kind::Shop => "Products",
    }
}

/// What was searched, in one line (the choice card's title and the mailed list's subject).
fn searched_line(kind: Kind, r: &Req) -> String {
    match kind {
        Kind::Flight => format!(
            "{} to {} {}{}, {}, {} adult{}{}",
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
        ),
        Kind::Hotel => format!(
            "{}{}, {} to {}, {} guest{}",
            r.city,
            if r.area.is_empty() {
                String::new()
            } else {
                format!(" ({})", r.area)
            },
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

/// The options as plain lines for a mail (data, one option per line).
fn listing_lines(options: &[&Offer]) -> String {
    options
        .iter()
        .map(|o| {
            let mut parts = vec![o.label.clone()];
            parts.extend(o.details.iter().cloned());
            let mut line = format!("- {}", parts.join(" · "));
            if !o.url.is_empty() {
                line.push_str(&format!("\n  {}", o.url));
            }
            line
        })
        .collect::<Vec<_>>()
        .join("\n")
}

// ---------------------------------------------------------------- the transcript, read back

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

// ---------------------------------------------------------------- the known sites: form functions with fixed ids

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

fn denied(m: &Value) -> bool {
    let c = content_of(m);
    c.starts_with("DENIED") || c.to_lowercase().contains("\"status\": \"denied\"")
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

// ---------------------------------------------------------------- the machine

/// What the machine reads besides the validated arguments and the transcript.
pub struct Machine<'a> {
    /// the profile, with the connected mailbox's address as `mailbox` when one is connected
    pub profile: &'a Value,
    /// a mail account is connected (results are mailed only then)
    pub mail_ready: bool,
    /// the catalog's tool names (a travel MCP connector's search is called when the native one is missing)
    pub available: &'a [String],
    /// the machine's code calls in the transcript before this run's tool call: the ids go on from there
    pub first: usize,
}

/// The ids of the machine's code calls go on from the ones already in the transcript.
struct Ids(usize);

impl Ids {
    fn new(t: &[Value], first: usize) -> Ids {
        Ids(first
            + t.iter()
                .flat_map(tool_calls)
                .filter(|tc| get_str(tc, "id").starts_with(PREFIX))
                .count()
            + 1)
    }
    fn calls(&self, name: &str, args: Value) -> Step {
        Step::Calls(vec![workflow::code_call(PREFIX, self.0, name, args)])
    }
}

/// The next step of a run of workflow tool `tool` with its validated arguments `args`; `t` is the transcript from the
/// tool call on (the machine reads only its own calls after it).
pub fn next(tool: &str, args: &Value, t: &[Value], m: &Machine) -> Step {
    let ids = Ids::new(t, m.first);
    if tool == SHOP_ORDER {
        return next_shop(&ShopReq::from_args(args), t, m, &ids);
    }
    next_travel(&Req::from_args(args), t, m, &ids)
}

fn next_travel(req: &Req, t: &[Value], m: &Machine, ids: &Ids) -> Step {
    let kind = req.kind();
    let spec = spec_of(kind);
    let searched = req.searched(kind);
    // 1. one search, on the user's connector when the arguments say so (the data tools follow the search)
    let searches = calls_of(t, spec.search);
    let connector = req.use_connector
        || searches
            .last()
            .is_some_and(|(_, msg)| search_name(t, msg).starts_with("mcp_"));
    let Some((_, search_msg)) = searches.last() else {
        return ids.calls(
            &tool_for(spec.search, m.available, connector),
            req.search_args(kind),
        );
    };
    // the booking stage under way and the search result compressed away (a long run: snapshots, the vault): the item
    // from the machine's own calls, whose arguments compression never touches
    if content_of(search_msg).contains(super::guards::COMPRESSED_MARK) {
        if let Some(item) = resumed_item(t, pos(t, search_msg)) {
            return book(spec, req, t, m, ids, &item);
        }
    }
    let result = data_of(search_msg);
    let offers = offers_in(kind, &result);
    let next_items = json!(req.next);
    let compare = get_str(&result, "compare_url");
    let source = get_str(&result, "data");
    if offers.is_empty() {
        // the native search found nothing priced: the user's travel connector next, once
        let mcp = tool_for(spec.search, m.available, true);
        if mcp.starts_with("mcp_") && !search_name(t, search_msg).starts_with("mcp_") {
            return ids.calls(&mcp, req.search_args(kind));
        }
        // the flight / hotel APIs failed and a search fell back to the web: the pages it read
        let pages = searches
            .iter()
            .rev()
            .map(|(_, msg)| web_pages(&data_of(msg)))
            .find(|p| !p.is_empty())
            .unwrap_or_default();
        // the search was limited to the preferred airlines and none flies it: no second search now
        if kind == Kind::Flight && !req.airlines.is_empty() && pages.is_empty() {
            return Step::Done(no_preferred(req, &searched, &[]));
        }
        if !pages.is_empty() {
            let mut must: Vec<String> = pages
                .iter()
                .map(|p| {
                    let price = get_str(p, "price_from");
                    format!(
                        "{} — {}{}",
                        get_str(p, "title"),
                        get_str(p, "url"),
                        if price.is_empty() {
                            String::new()
                        } else {
                            format!(" (the page shows prices from {price})")
                        }
                    )
                })
                .collect();
            must.push(
                "these are web pages about it, their prices are \"from\" prices, not quotes for the dates".into(),
            );
            if !compare.is_empty() {
                must.push(format!("compare prices for the dates on Kayak: {compare}"));
            }
            let mut out = json!({"status": "shown", "source": "web", "searched": searched,
                                 "options": [], "web_pages": pages, "must_say": must});
            if !compare.is_empty() {
                out["links"] = json!({"compare_url": compare});
            }
            if !req.next.is_empty() {
                out["next_items"] = next_items;
            }
            return Step::Done(out);
        }
        return Step::Done(json!({"status": "nothing_found", "searched": searched,
            "must_say": [format!("the search found nothing for {}", searched_line(kind, req))]}));
    }
    // a search limited to the preferred airlines that found none of them: say so, ask before searching all airlines
    if kind == Kind::Flight
        && !req.airlines.is_empty()
        && result["searched"]["preferred_found"].as_u64() == Some(0)
    {
        return Step::Done(no_preferred(req, &searched, &offers));
    }
    let (list, rule) = ranked(kind, req, &offers);
    let chosen = by_label(&offers, &req.pick).cloned();
    let (pick, rule) = match chosen {
        Some(o) => (o, "your pick".to_string()),
        None => match list.first() {
            Some(o) => (o.clone(), rule),
            None => {
                return Step::Done(json!({"status": "nothing_found", "searched": searched,
                    "unmet": rule.trim_end_matches(", the cheapest"),
                    "results": offers.len(),
                    "options": offers.iter().take(4).map(option_json).collect::<Vec<_>>(),
                    "must_say": [format!("none of the {} results meets {}", offers.len(), rule.trim_end_matches(", the cheapest"))]}));
            }
        },
    };
    let search_pos = pos(t, search_msg);
    // 2. the pick's data: a restaurant's times (the next candidate when it is full)
    let mut slot: Option<(String, String, Value)> = None; // (time, url, the availability result)
    let mut pick = pick;
    if kind == Kind::Michelin {
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
            let Some((_, msg)) = got else {
                return ids.calls(
                    &tool_for(spec.detail, m.available, connector),
                    json!({"restaurant_id": c.id, "date": req.date.map(iso).unwrap_or_default(), "party_size": req.party_or(kind)}),
                );
            };
            tried += 1;
            let v = data_of(msg);
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
                let checked: Vec<&str> = candidates
                    .iter()
                    .take(tried)
                    .map(|c| c.label.as_str())
                    .collect();
                return Step::Done(json!({"status": "nothing_found", "searched": searched,
                    "checked": checked,
                    "must_say": [format!("no table on {} for {} at {}", req.date.map(long).unwrap_or_default(),
                                         req.party_or(kind), checked.join(", "))]}));
            }
        }
    }
    // 3. the choice card with the pick shown (once)
    let mut shown: Vec<Offer> = vec![pick.clone()];
    shown.extend(
        list.iter()
            .filter(|o| o.label != pick.label)
            .take(3)
            .cloned(),
    );
    if calls_of(t, "present_choices")
        .iter()
        .all(|(_, msg)| pos(t, msg) < search_pos)
    {
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
        return ids.calls(
            "present_choices",
            json!({"question": format!("{} for {}", choice_title(kind), searched_line(kind, req)),
                   "kind": "comparison", "options": options, "picked": pick.label}),
        );
    }
    // 3b. the pick's own data: a Google Flights offer's sellers and, for the comparison, a hotel's rooms (the booking
    // stage reads the same call)
    let mut out = json!({"status": "shown", "searched": searched,
                         "options": shown.iter().map(option_json).collect::<Vec<_>>(),
                         "pick": {"label": pick.label, "rule": rule}});
    let mut must: Vec<String> = vec![format!("picked: {} ({rule})", offer_fact(&pick))];
    if !source.is_empty() {
        out["source"] = json!(source);
    }
    if kind == Kind::Flight && req.cabin != "economy" {
        let as_searched = get_str(&result["searched"], "cabin");
        let cabin = if as_searched.is_empty() {
            req.cabin.replace('_', " ")
        } else {
            as_searched
        };
        out["cabin_searched"] = json!(cabin);
        must.push(format!("searched as {cabin}"));
    }
    if let Some((time, _, _)) = &slot {
        out["pick"]["time"] = json!(time);
        must.push(format!("{time} available at {}", pick.label));
    }
    if pick.preferred {
        must.push(format!("{} is on your preferred airline", pick.label));
    }
    if kind == Kind::Flight && pick.id.starts_with("fl_") {
        let got = calls_of(t, "flight_offer")
            .into_iter()
            .rfind(|(a, msg)| sget(a, "offer_id") == pick.id && pos(t, msg) > search_pos);
        let Some((_, msg)) = got else {
            let cur = if pick.currency.is_empty() {
                &req.currency
            } else {
                &pick.currency
            };
            return ids.calls(
                &tool_for("flight_offer", m.available, false),
                json!({"offer_id": pick.id, "currency": cur}),
            );
        };
        let s = sellers(&data_of(msg));
        if !s.is_empty() {
            must.push(format!(
                "sold by: {}",
                s.iter()
                    .map(|x| get_str(x, "book_with"))
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
            out["sellers"] = json!(s);
        }
    }
    let rooms_args = json!({"hotel_id": pick.id, "check_in": req.date.map(iso).unwrap_or_default(),
                            "check_out": req.check_out.map(iso).unwrap_or_default(), "guests": req.party_or(kind)});
    if kind == Kind::Hotel && !req.book && !pick.id.is_empty() {
        let rooms_call = calls_of(t, spec.detail)
            .into_iter()
            .rfind(|(a, _)| sget(a, "hotel_id") == pick.id);
        let Some((_, msg)) = rooms_call else {
            return ids.calls(&tool_for(spec.detail, m.available, connector), rooms_args);
        };
        let rooms = offers_in(kind, &data_of(msg));
        if let Some((i, why)) = pick_room(&rooms, req) {
            let r = &rooms[i];
            let mut room = option_json(r);
            room["rule"] = json!(why);
            out["pick"]["room"] = room;
            must.push(format!("room at {}: {} ({why})", pick.label, offer_fact(r)));
        }
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
        out["days_searched"] = json!({"count": days.len(), "from": days[0], "to": days[days.len() - 1], "pick_date": pick.date});
        must.push(format!(
            "{} days searched in one call ({} to {}){}",
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
    if !compare.is_empty() {
        out["links"] = json!({"compare_url": compare});
        must.push(format!("compare prices on Kayak: {compare}"));
    }
    if !req.next.is_empty() {
        out["next_items"] = next_items;
    }
    if !req.book {
        // the comparison mailed to the address the user gave (only with a connected mail account)
        if !req.mail_to.is_empty() {
            let to = req.mail_to.clone();
            if !m.mail_ready {
                out["mailed"] =
                    json!({"to": to, "ok": false, "reason": "no mail account is connected"});
                must.push(format!(
                    "not sent to {to}: no mail account is connected (connect Gmail in Settings)"
                ));
            } else {
                let Some((_, sent)) = last_after(t, "gmail_send", search_pos) else {
                    let shown_refs: Vec<&Offer> = shown.iter().collect();
                    return ids.calls(
                        "gmail_send",
                        json!({"to": to, "subject": format!("{} for {}", choice_title(kind), searched_line(kind, req)),
                               "body": format!("{} for {}\n\n{}\n\nPicked: {} ({rule}){}",
                                   choice_title(kind), searched_line(kind, req), listing_lines(&shown_refs), pick.label,
                                   if compare.is_empty() { String::new() } else { format!("\nCompare prices on Kayak: {compare}") })}),
                    );
                };
                let c = content_of(sent);
                let ok = !(c.starts_with("ERROR") || denied(sent));
                out["mailed"] = json!({"to": to, "ok": ok});
                if ok {
                    must.push(format!("sent to {to}"));
                } else {
                    out["mailed"]["error"] = json!(take_chars(&c, 200));
                    must.push(format!("not sent to {to}: {}", take_chars(&c, 120)));
                }
            }
        }
        out["must_say"] = json!(must);
        return Step::Done(out);
    }
    // 4. the item to book
    let item = match item_for(spec, req, t, m, ids, &pick, slot.as_ref(), &rooms_args) {
        Ok(i) => i,
        Err(step) => return step,
    };
    if item.url.is_empty() {
        out["booking_link"] = Value::Null;
        must.push(format!(
            "{} has no online booking link in the data: book it by phone or on its own site",
            pick.label
        ));
        out["must_say"] = json!(must);
        return Step::Done(out);
    }
    book(spec, req, t, m, ids, &item)
}

/// The answer when a search limited to the preferred airlines found none of them: no second search now — the question
/// whether to search all airlines (and what else came back, when the API searched them all).
fn no_preferred(req: &Req, searched: &Value, others: &[Offer]) -> Value {
    let mut must = vec![format!(
        "there is no {} flight for {}",
        req.airlines.join(" or "),
        searched_line(Kind::Flight, req)
    )];
    must.extend(others.iter().take(4).map(offer_fact));
    json!({"status": "nothing_found", "searched": searched, "preferred_found": 0,
           "options": others.iter().take(4).map(option_json).collect::<Vec<_>>(),
           "next_question": "search_all_airlines", "must_say": must})
}

/// What is booked, as the hand-over and the result name it.
#[derive(Debug, Clone, Default)]
pub struct Item {
    /// the line of `purchase_confirm` ("SQ 638 SIN-NRT 2026-12-04, 1 adult")
    pub name: String,
    pub url: String,
    pub total: Option<f64>,
    pub currency: String,
    /// money is charged now (a fare, a prepaid rate, a deposit, an order): `purchase_confirm` and the card
    pub charged: bool,
    pub delivery: String,
    pub note: String,
    /// details of the booking (schedule, baggage, cancellation, payment …)
    pub lines: Vec<String>,
}

impl Item {
    fn to_json(&self) -> Value {
        let mut o = json!({"what": self.name, "charged": self.charged});
        if let Some(t) = self.total {
            o["amount"] = json!(t);
            o["currency"] = json!(self.currency);
        }
        if !self.delivery.is_empty() {
            o["delivery"] = json!(self.delivery);
        }
        if !self.lines.is_empty() {
            o["details"] = json!(self.lines);
        }
        if !self.url.is_empty() {
            o["url"] = json!(self.url);
        }
        o
    }
}

/// The item to book for the pick: a flight's fare, a hotel's room by rule (its rooms read first), a table's slot.
#[allow(clippy::too_many_arguments)]
fn item_for(
    spec: &BookingSpec,
    req: &Req,
    t: &[Value],
    m: &Machine,
    ids: &Ids,
    pick: &Offer,
    slot: Option<&(String, String, Value)>,
    rooms_args: &Value,
) -> Result<Item, Step> {
    let kind = spec.kind;
    Ok(match kind {
        Kind::Flight => Item {
            name: format!(
                "{} {}-{} {}, {} adult{}{}",
                pick.label,
                req.origin,
                req.destination,
                req.date.map(iso).unwrap_or_default(),
                req.party_or(kind),
                if req.party_or(kind) > 1 { "s" } else { "" },
                match req.cabin.as_str() {
                    "economy" => String::new(),
                    c => format!(", {}", c.replace('_', " ")),
                }
            ),
            url: pick.url.clone(),
            total: pick.price.map(|p| p * req.party_or(kind) as f64),
            currency: pick.currency.clone(),
            charged: true,
            delivery: format!("E-ticket to {}", guest_profile(m.profile, &req.contact).1),
            note: pick.cancellation.clone(),
            lines: pick.details.clone(),
        },
        Kind::Hotel => {
            // the rooms of the picked hotel, then the room by rule
            let connector = req.use_connector;
            let rooms_call = calls_of(t, spec.detail)
                .into_iter()
                .rfind(|(a, _)| sget(a, "hotel_id") == pick.id);
            if rooms_call.is_none() && !pick.id.is_empty() {
                return Err(ids.calls(
                    &tool_for(spec.detail, m.available, connector),
                    rooms_args.clone(),
                ));
            }
            let rooms: Vec<Offer> = rooms_call
                .map(|(_, msg)| offers_in(kind, &data_of(msg)))
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
            match pick_room(&rooms, req) {
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
                            req.party_or(kind),
                            if req.party_or(kind) > 1 { "s" } else { "" }
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
            let (time, url, v) = slot.cloned().unwrap_or_default();
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
                    req.party_or(kind),
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
                req.party_or(kind)
            ),
            url: pick.url.clone(),
            total: None,
            currency: String::new(),
            charged: false,
            delivery: "Table reservation".into(),
            note: String::new(),
            lines: Vec::new(),
        },
    })
}

/// The item of a booking stage already under way, from the machine's own calls after the search: the booking link it
/// opened, and the approved `purchase_confirm` (what, the total, the currency, the delivery) or else the pick shown on
/// the choice card. `None` before the booking link was opened.
fn resumed_item(t: &[Value], search_pos: usize) -> Option<Item> {
    let (nav, _) = calls_of(t, "browser_navigate")
        .into_iter()
        .find(|(_, msg)| {
            pos(t, msg) > search_pos && get_str(msg, "tool_call_id").starts_with(PREFIX)
        })?;
    let url = sget(&nav, "url");
    let confirm = calls_of(t, "purchase_confirm")
        .into_iter()
        .rfind(|(_, msg)| pos(t, msg) > search_pos)
        .map(|(a, _)| a);
    let picked = calls_of(t, "present_choices")
        .into_iter()
        .rfind(|(_, msg)| pos(t, msg) > search_pos)
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

/// The profile the form functions read, with the contact details filled where the profile has none: the profile's
/// own, else the connected mailbox's address (`profile["mailbox"]`), else the ones the user gave (the `contact`
/// argument). Returns the profile and the email that results.
fn guest_profile(profile: &Value, contact: &Value) -> (Value, String) {
    let mut p = if profile.is_object() {
        profile.clone()
    } else {
        json!({})
    };
    let blank = |p: &Value, keys: &[&str]| keys.iter().all(|k| get_str(p, k).trim().is_empty());
    let given = |k: &str| get_str(contact, k).trim().to_string();
    if blank(&p, &["name_en", "name"]) && !given("name").is_empty() {
        p["name_en"] = json!(given("name"));
    }
    if blank(&p, &["email_personal", "email"]) {
        let mailbox = get_str(profile, "mailbox");
        let email = if mailbox.trim().is_empty() {
            given("email")
        } else {
            mailbox
        };
        if !email.is_empty() {
            p["email_personal"] = json!(email);
        }
    }
    if blank(&p, &["phone"]) && !given("phone").is_empty() {
        p["phone"] = json!(given("phone"));
    }
    if blank(&p, &["address_home"]) && !given("address").is_empty() {
        p["address_home"] = json!(given("address"));
    }
    let email = Guest {
        profile: &p,
        requests: "",
    }
    .email();
    (p, email)
}

/// The booking stage, after the pick: the Vault's list, `purchase_confirm`, the booking link, then a known site's
/// form function (fixed ids), the card by the site's labels, the hand-over and the confirmation — or, on any other
/// site, the page handed to `browser-use` (`status: browser_use`).
fn book(spec: &BookingSpec, req: &Req, t: &[Value], m: &Machine, ids: &Ids, item: &Item) -> Step {
    book_with(
        spec,
        t,
        m,
        ids,
        item,
        &req.contact,
        &req.card,
        &req.requests,
    )
}

#[allow(clippy::too_many_arguments)]
fn book_with(
    spec: &BookingSpec,
    t: &[Value],
    m: &Machine,
    ids: &Ids,
    item: &Item,
    contact: &Value,
    card_name: &str,
    requests: &str,
) -> Step {
    let card_needed = item.charged;
    let vault = calls_of(t, "vault_list")
        .last()
        .map(|(_, msg)| content_of(msg));
    if (card_needed || spec.kind == Kind::Flight) && vault.is_none() {
        return ids.calls("vault_list", json!({}));
    }
    let cards: Vec<VaultCard> = vault.as_deref().map(vault_cards).unwrap_or_default();
    // the card the user named (the `card` argument), else the first with its details
    let card = pick_card(&cards, &[card_name.to_string()]);
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
                return ids.calls("purchase_confirm", a);
            }
            Some((_, msg)) if denied(msg) => {
                return Step::Done(
                    json!({"status": "not_confirmed", "reason": "purchase_not_approved",
                    "booking": item.to_json(),
                    "must_say": [format!("nothing was booked: the purchase of {} was not approved", item.name)]}),
                );
            }
            Some((_, msg)) => approved_at = pos(t, msg),
        }
    }
    // the booking link (once)
    let nav = calls_of(t, "browser_navigate")
        .into_iter()
        .rfind(|(a, _)| sget(a, "url") == item.url);
    let Some((nav_args, nav_msg)) = nav else {
        return ids.calls("browser_navigate", json!({"url": item.url}));
    };
    let page_url = landed(&nav_args, nav_msg);
    let Some(site) = site_for(&page_url).or_else(|| site_for(&item.url)) else {
        // no form function knows this page: the model fills it with the browser-use skill
        return Step::Done(
            json!({"status": "browser_use", "url": page_url, "booking": item.to_json(),
            "must_say": [format!("the booking page {page_url} is open in the browser")]}),
        );
    };
    let nav_pos = pos(t, nav_msg);
    let (profile, email) = guest_profile(m.profile, contact);
    let guest = Guest {
        profile: &profile,
        requests,
    };
    // the plan with fixed ids
    let Some((_, fill_msg)) = calls_of(t, "browser_fill_form")
        .into_iter()
        .rfind(|(a, msg)| sget(a, "form") == site.form && pos(t, msg) > nav_pos)
    else {
        return ids.calls("browser_fill_form", fill_args(site, &guest));
    };
    let fill_pos = pos(t, fill_msg);
    // the card: one secret fill per box, by the site's fixed labels, after the approval
    let mut done_pos = fill_pos;
    if let (Some(c), true, true) = (card, card_ok, card_needed) {
        let after = fill_pos.max(approved_at);
        let Some((_, snap)) = calls_of(t, "browser_snapshot")
            .into_iter()
            .find(|(_, msg)| pos(t, msg) > after)
        else {
            return ids.calls("browser_snapshot", json!({"max_chars": 6000}));
        };
        let page = snapshot_text(snap);
        let filled: Vec<String> = calls_of(t, "browser_fill_secret")
            .iter()
            .filter(|(_, msg)| pos(t, msg) > after)
            .map(|(a, _)| sget(a, "field"))
            .collect();
        for (field, label) in site.card {
            if filled.iter().any(|f| f == field) {
                continue;
            }
            if let Some(r) = ref_of(&page, label) {
                return ids.calls(
                    "browser_fill_secret",
                    json!({"ref": r, "item_id": c.id, "field": field}),
                );
            }
        }
        done_pos = calls_of(t, "browser_fill_secret")
            .last()
            .map(|(_, msg)| pos(t, msg))
            .unwrap_or(fill_pos)
            .max(fill_pos);
    }
    // the hand-over: the final click is the user's
    let takeovers: Vec<(Value, &Value)> = calls_of(t, "browser_request_takeover")
        .into_iter()
        .filter(|(_, msg)| pos(t, msg) > done_pos)
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
                "The {} page is filled in from your profile{}{card_part}: {}{total}. Check it{}, press {} yourself and hand the browser back.",
                spec.what,
                if spec.kind == Kind::Flight { " and passport (Vault)" } else { "" },
                item.name,
                if left.is_empty() { String::new() } else { format!(", fill in {}", left.join(", ")) },
                spec.button
            );
            return ids.calls("browser_request_takeover", json!({"reason": reason}));
        }
        Some((_, msg)) if handed_back(msg) => pos(t, msg),
        Some(_) => {
            // the user has the page and did not hand it back
            return Step::Done(
                json!({"status": "handed_over", "url": page_url, "booking": item.to_json(),
                "must_say": [format!("the filled {} page is with you: press {} yourself", spec.what, spec.button)]}),
            );
        }
    };
    let Some((_, snap)) = last_after(t, "browser_snapshot", back) else {
        return ids.calls("browser_snapshot", json!({"max_chars": 6000}));
    };
    let page = snapshot_text(snap);
    let reference = Regex::new(site.ref_re)
        .ok()
        .and_then(|r| r.find(&page).map(|x| x.as_str().to_string()))
        .or_else(|| crate::outcome::refs_in(&page).into_iter().next());
    let mut booking = item.to_json();
    let paid_with = card
        .filter(|_| card_ok && card_needed)
        .map(|c| c.label.clone());
    if let Some(c) = &paid_with {
        booking["card"] = json!(c);
    }
    if !email.is_empty() && spec.kind != Kind::Shop {
        booking["confirmation_to"] = json!(email);
    }
    let amount = item
        .total
        .map(|x| format!("{} {}", item.currency, money(x)))
        .unwrap_or_default();
    match reference {
        Some(r) => {
            booking["reference"] = json!(r);
            let mut must = vec![format!("reference {r}"), item.name.clone()];
            if !amount.is_empty() {
                must.push(if item.charged {
                    format!("total {amount}")
                } else {
                    format!("total {amount} (nothing charged now)")
                });
            }
            must.extend(item.lines.iter().cloned());
            if !email.is_empty() && spec.kind != Kind::Shop {
                must.push(format!("confirmation to {email}"));
            }
            Step::Done(json!({"status": "booked", "booking": booking, "must_say": must}))
        }
        None => Step::Done(
            json!({"status": "not_confirmed", "reason": "no_reference", "booking": booking,
            "must_say": [format!("{} is filled in but NOT confirmed: no reference showed after the hand-back, so it is not booked; the page stays open in the browser tab", item.name)]}),
        ),
    }
}

// ---------------------------------------------------------------- shopping

/// The shops the machine fills by fixed ids.
fn known_shop(url: &str) -> bool {
    let path = url::Url::parse(url)
        .map(|u| u.path().to_lowercase())
        .unwrap_or_default();
    host_of(url) == "shop.test" && (path.starts_with("/kaya_") || path.starts_with("/stride_"))
}

/// The checkout of a shopping run on a known shop (Kaya Mobile: results → product → Add to Cart → cart → checkout;
/// Stride Sports: the product page holds the checkout), or the shop's page handed to `browser-use` on any other site.
fn next_shop(sr: &ShopReq, t: &[Value], m: &Machine, ids: &Ids) -> Step {
    let spec = spec_of(Kind::Shop);
    let url = sr.url.clone();
    let lc_path = url::Url::parse(&url)
        .map(|u| u.path().to_lowercase())
        .unwrap_or_default();
    let (profile, _) = guest_profile(m.profile, &sr.contact);
    let address = get_str(&profile, "address_home");
    let delivery = format!("Home delivery to {address}");
    if !known_shop(&url) {
        // any other shop: open it and hand the page to browser-use
        let Some((a, msg)) = calls_of(t, "browser_navigate")
            .into_iter()
            .rfind(|(a, _)| sget(a, "url") == url)
        else {
            return ids.calls("browser_navigate", json!({"url": url}));
        };
        let page = landed(&a, msg);
        return Step::Done(
            json!({"status": "browser_use", "url": page, "action": sr.action,
            "must_say": [format!("the shop's page {page} is open in the browser")]}),
        );
    }
    if lc_path.starts_with("/stride_") {
        // the product page is the checkout: open it, read the item and the price, then book
        let Some((_, msg)) = calls_of(t, "browser_navigate")
            .into_iter()
            .rfind(|(a, _)| sget(a, "url") == url)
        else {
            return ids.calls("browser_navigate", json!({"url": url}));
        };
        // Stride's product page by its fixed parts: the item in the <title> ("<item> — Stride Sports Singapore"),
        // the amount on the "total S$…" line of its checkout
        let page = snapshot_text(msg);
        let title = get_str(&json_of(msg), "title")
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
        if !sr.buy() {
            let price_text = price.map(|p| format!("S${}", money(p))).unwrap_or_default();
            return Step::Done(json!({"status": "shown", "shop": "Stride Sports",
                "options": [{"label": title, "price": price, "currency": "SGD", "price_text": price_text, "url": url}],
                "must_say": [format!("{title}: {price_text} on Stride Sports ({url})")]}));
        }
        let item = Item {
            name: title,
            url: url.clone(),
            total: price.map(|p| p * sr.quantity as f64),
            currency: "SGD".into(),
            charged: true,
            delivery,
            note: String::new(),
            lines: vec![format!("Delivery: home delivery to {address}")],
        };
        return book_with(spec, t, m, ids, &item, &sr.contact, &sr.card, "");
    }
    // Kaya Mobile: the results page read whole, the listings ranked by code
    let origin = origin_of(&url);
    let Some((_, read)) = calls_of(t, "browser_read").into_iter().rfind(|(a, _)| {
        a["urls"]
            .as_array()
            .is_some_and(|u| u.iter().any(|x| x.as_str() == Some(url.as_str())))
    }) else {
        return ids.calls("browser_read", json!({"urls": [url]}));
    };
    let page = pages_text(read);
    let listings = crate::listings::parse(&page);
    let ranked = sr.ranking.apply(&listings);
    let pick_lc = sr.pick.to_lowercase();
    let chosen = (!pick_lc.is_empty())
        .then(|| {
            listings
                .iter()
                .find(|x| {
                    x.title.eq_ignore_ascii_case(&sr.pick)
                        || pick_lc.contains(&x.title.to_lowercase())
                })
                .cloned()
        })
        .flatten();
    let wants_choose = sr.let_user_choose && chosen.is_none();
    let read_pos = pos(t, read);
    let shown = calls_of(t, "present_choices")
        .iter()
        .any(|(_, msg)| pos(t, msg) > read_pos);
    let listing_json = |l: &crate::listings::Listing| json!({"label": l.title, "price": l.price, "price_text": l.price_text, "rating": l.rating_text, "url": url});
    let Some(top) = chosen.clone().or_else(|| ranked.first().cloned()) else {
        return Step::Done(
            json!({"status": "nothing_found", "shop": "Kaya Mobile", "compared": listings.len(),
            "criteria": sr.ranking.to_json(),
            "must_say": [format!("none of the {} listings on Kaya Mobile's page meets the criteria", listings.len())]}),
        );
    };
    if !shown && chosen.is_none() {
        let options: Vec<Value> = ranked
            .iter()
            .map(|l| json!({"label": l.title, "details": [l.price_text, l.rating_text], "source_url": url}))
            .collect();
        let mut rk = sr.ranking.to_json();
        rk["url"] = json!(url);
        let mut a = json!({"question": format!("The {} listings on Kaya Mobile's page that meet your criteria ({} compared)", ranked.len(), listings.len()),
                           "kind": "comparison", "options": options, "ranking": rk});
        if !wants_choose {
            a["picked"] = json!(top.title);
        }
        return ids.calls("present_choices", a);
    }
    let options: Vec<Value> = ranked.iter().map(listing_json).collect();
    if wants_choose {
        return Step::Done(
            json!({"status": "shown", "shop": "Kaya Mobile", "choose": true,
            "compared": listings.len(), "options": options,
            "must_say": [format!("the {} best matches out of {} listings are on the cards: press Choose on one", ranked.len(), listings.len())]}),
        );
    }
    let rule = if chosen.is_some() {
        "your pick".to_string()
    } else {
        format!("by {}", sr.ranking.by)
    };
    let pick_fact = format!("{} — {} ({})", top.title, top.price_text, top.rating_text);
    if !sr.cart() {
        return Step::Done(
            json!({"status": "shown", "shop": "Kaya Mobile", "compared": listings.len(),
            "options": options, "pick": {"label": top.title, "rule": rule}, "url": url,
            "must_say": [format!("best match on Kaya Mobile: {pick_fact}")]}),
        );
    }
    // the cart first: an item a previous turn put there is not added twice
    let cart_url = format!("{origin}/kaya_cart.html");
    let Some((_, cart_msg)) = calls_of(t, "browser_navigate")
        .into_iter()
        .rfind(|(a, _)| sget(a, "url") == cart_url)
    else {
        return ids.calls("browser_navigate", json!({"url": cart_url}));
    };
    let cart_pos = pos(t, cart_msg);
    let in_cart = snapshot_text(cart_msg).contains(&top.title);
    let mut colour = String::new();
    if !in_cart {
        // the product from the results page, by its title (a fixed selector for Kaya's product links)
        let Some((_, nav)) = calls_of(t, "browser_navigate")
            .into_iter()
            .rfind(|(a, msg)| sget(a, "url") == url && pos(t, msg) > cart_pos)
        else {
            return ids.calls("browser_navigate", json!({"url": url}));
        };
        let nav_pos = pos(t, nav);
        let Some((_, open)) = calls_of(t, "browser_fill_form")
            .into_iter()
            .rfind(|(a, msg)| sget(a, "form") == "kaya-results" && pos(t, msg) > nav_pos)
        else {
            return ids.calls(
                "browser_fill_form",
                json!({"form": "kaya-results", "fills": [{"op": "click", "sel": "a[href*='kaya_item.html']", "text": top.title, "field": "product"}]}),
            );
        };
        let open_pos = pos(t, open);
        let Some((_, added)) = calls_of(t, "browser_fill_form")
            .into_iter()
            .rfind(|(a, msg)| sget(a, "form") == "kaya-item" && pos(t, msg) > open_pos)
        else {
            let mut fills: Vec<Value> = Vec::new();
            if !sr.colour.is_empty() {
                fills.push(
                    json!({"op": "select", "sel": "#colour", "value": sr.colour, "field": "colour"}),
                );
            }
            for _ in 0..sr.quantity {
                fills.push(json!({"op": "click", "sel": "#add", "field": "add_to_cart"}));
            }
            return ids.calls(
                "browser_fill_form",
                json!({"form": "kaya-item", "fills": fills}),
            );
        };
        // the cart count, verified on the page
        let added_pos = pos(t, added);
        let Some((_, snap)) = calls_of(t, "browser_snapshot")
            .into_iter()
            .find(|(_, msg)| pos(t, msg) > added_pos)
        else {
            return ids.calls("browser_snapshot", json!({"max_chars": 4000}));
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
            return Step::Done(
                json!({"status": "error", "reason": "cart_still_empty", "item": top.title,
                "must_say": [format!("Add to Cart was pressed for {} but the cart still shows 0 items; the browser tab is open to check", top.title)]}),
            );
        }
        if !sr.buy() {
            return Step::Done(json!({"status": "in_cart", "shop": "Kaya Mobile",
                "item": {"label": top.title, "colour": colour, "price_text": top.price_text, "rating": top.rating_text, "url": url},
                "cart": {"count": count, "checked_out": false},
                "must_say": [format!("{pick_fact}{} is in your Kaya Mobile cart (cart: {count} item{}; not checked out)",
                    if colour.is_empty() { String::new() } else { format!(", {colour}") }, if count == 1 { "" } else { "s" })]}));
        }
    } else if !sr.buy() {
        return Step::Done(
            json!({"status": "in_cart", "shop": "Kaya Mobile", "already": true,
            "item": {"label": top.title, "price_text": top.price_text, "url": url},
            "cart": {"checked_out": false},
            "must_say": [format!("{} — {} is already in your Kaya Mobile cart (not checked out)", top.title, top.price_text)]}),
        );
    }
    let item = Item {
        name: if colour.is_empty() {
            top.title.clone()
        } else {
            format!("{} ({colour})", top.title)
        },
        url: format!("{origin}/kaya_checkout.html"),
        total: top.price.map(|p| p * sr.quantity as f64),
        currency: currency_in(&top.price_text),
        charged: true,
        delivery,
        note: String::new(),
        lines: vec![format!("Delivery: home delivery to {address}")],
    };
    book_with(spec, t, m, ids, &item, &sr.contact, &sr.card, "")
}

#[cfg(test)]
mod tests;
