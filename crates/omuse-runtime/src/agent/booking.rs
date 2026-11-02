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
//! `travel_book` — the booking link opened, then `purchase_confirm` on that page when money is charged. No booking or
//! checkout site has a form function: the page is handed to the `browser-use` skill (`status: browser_use`), which
//! fills it and hands the final Pay / Confirm / Place order to the user. `shop_order` opens the shop's page and hands
//! it over the same way, with the ranking criteria.
//!
//! The machine ends with [`Step::Done`]: a JSON result (status, options, the pick and its rule, the booking, links,
//! the mail sent, `hints` — the facts the reply must carry). The model writes the reply from it; no answer text is
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
use super::driver::{json_of, pos, Step};
use super::show as S;
use super::tools::fn_;
use super::util::{content_of, sget, stored_call, tool_calls};
use super::workflow::{self, Checked};

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
/// The model's pick over a flight search's flights (`status: pick_needed`): the offer ids, as JSON.
pub const TRAVEL_PICK: &str = "travel_pick";
/// The workflow tools this machine runs.
pub const TOOLS: [&str; 4] = [TRAVEL_SEARCH, TRAVEL_BOOK, SHOP_ORDER, TRAVEL_PICK];
/// The status of a flight search's result that lists every flight found for the model to pick from.
pub const PICK_NEEDED: &str = "pick_needed";

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
        "named": {"type": "array", "items": {"type": "string"}, "description": "options the user named, each as its exact name: a hotel's or restaurant's full name, an airline, a flight number (\"SQ 638\")"},
        "contact": {"type": "object", "description": "contact details the user gave in the chat", "properties": {"name": s.clone(), "email": s.clone(), "phone": s.clone()}},
        "mail_results_to": {"type": "string", "description": "an address the user asked the results to be mailed to"},
        "use_connector": {"type": "boolean", "description": "the user named their travel agency / connector to search and book with"},
        "card": {"type": "string", "description": "the Vault card the user named to pay with: its id from vault_list (id=…), its card_id, or its label exactly as listed"},
        "next": {"type": "array", "items": {"type": "string", "enum": ["flight", "hotel", "car", "restaurant"]}, "description": "the other searches the same request asks for (offered after this one)"}
    });
    let search = fn_(
        TRAVEL_SEARCH,
        "Search and compare flights, hotels, MICHELIN restaurants or restaurants. Code runs ONE search with these arguments. \
Flights: the result (status pick_needed) lists EVERY flight found for the day with its offer_id, departure and arrival \
(airport, local time), price and stops — call travel_pick with the offer_ids of exactly the flights the user asked for. \
Hotels and restaurants: code picks by rule (the cheapest, or the user's criteria), shows the options as choice cards and \
returns the result as JSON (options, the pick and its rule, links, show, hints). Nothing is booked. Write the reply from \
the result, using its hints (code appends the facts in show to the reply). status needs_input: ask the user for the \
missing fields, then call again with every field; invalid: fix the arguments and call again.",
        travel.clone(),
        &["kind"],
    );
    travel["pick"] = json!({"type": "string", "description": "the option the user chose: its label exactly as the choice card listed it (the text after \"I choose: \") or a flight's offer_id; empty = the pick by rule (a flight: the search's flights come back for travel_pick)"});
    let book = fn_(
        TRAVEL_BOOK,
        "Book a flight, a hotel room or a table — only when the user asks to book. Same arguments as travel_search, plus \
pick. Code searches, takes the pick (or picks by rule), asks the user to approve the payment, opens the booking page, \
fills it from the profile and the Vault and hands the final Pay / Confirm click to the user; the JSON result has the \
booking reference (status booked) or says why not. Write the reply from it, using its hints (code appends the facts in show to the reply).",
        travel,
        &["kind"],
    );
    let shop = fn_(
        SHOP_ORDER,
        "Compare, add to the cart or buy on a shop whose link the user gave. Code opens the shop's page and hands it to \
the browser-use skill (status browser_use) with the criteria: list the page's products as present_choices options \
(price, stars, ratings) with these criteria as its ranking, and code ranks them; cart and checkout are filled there, \
Place order is the user's. Write the reply from the JSON result, using its hints (code appends the facts in show to the reply).",
        json!({
            "url": {"type": "string", "description": "the shop's link the user gave in this chat (a results or product page) — the same link again when a later message asks to add to the cart or buy"},
            "action": {"type": "string", "enum": ["compare", "cart", "buy"]},
            "by": {"type": "string", "enum": ["rating", "price", "ratings"], "description": "best-rated, cheapest or most-reviewed first"},
            "max_price": {"type": "number"}, "min_price": {"type": "number"}, "min_stars": {"type": "number"},
            "min_ratings": {"type": "integer"}, "count": {"type": "integer", "description": "how many to show"},
            "title_has": {"type": "array", "items": s.clone(), "description": "words every chosen name has, e.g. [\"case\", \"iPhone 17 Pro Max\"]"},
            "title_not": strs,
            "pick": {"type": "string", "description": "the listing the user chose: its title exactly as the choice card listed it (the text after \"I choose: \")"}
        }),
        &["url", "action"],
    );
    let pick = fn_(
        TRAVEL_PICK,
        "Pick from the flights a travel_search / travel_book result listed (status pick_needed). Read every flight against \
the user's request and give the offer_ids of exactly the flights that meet it, in the order to show them: ONE id when \
the request names one flight (the last, the first, the earliest, the latest, the cheapest, the shortest …), several only \
when the user asked for options or a comparison. Stick to what the user asked: never add a cheaper or \"better\" flight \
they did not ask for, and default to the cheapest only when the user gave no criterion at all. Times are local at each \
airport; arrives_days_later 0 = it lands on the departure day (before midnight), 1 = the next day. When no flight meets \
the request, do not call this: say so. Code shows the flights picked (and books the first after travel_book) and returns \
the result as JSON; write the reply from it (code appends the facts in show to the reply).",
        json!({
            "offer_ids": {"type": "array", "items": {"type": "string"}, "description": "offer_id of each flight that meets the request, as the result listed it, in the order to show"}
        }),
        &["offer_ids"],
    );
    vec![search, book, shop, pick]
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

/// `ask_user` while a page code handed to `browser-use` is being filled (the runtime's `browser_use_handoff` event in
/// this task): refused, as data. A form workflow starts with every value known; the page is filled from the run's data
/// (the profile, the request, the search result), a field without a value is left blank and the page is handed over
/// — nothing is asked in the middle of the form (travel_book_real 2026-11-02: at Kiwi.com's contact step the model
/// asked whether to use the profile's email and phone; the run ended on the question, no hand-over). `events` are the
/// task's.
pub fn ask_in_browser_use(events: &[Value]) -> Option<Value> {
    events
        .iter()
        .any(|e| get_str(e, "type") == "browser_use_handoff")
        .then(|| {
            json!({"refused": "fill_then_hand_over",
                   "why": "this page is filled from what the run has (the profile, the request, the search result) without asking: \
use the profile's values, leave a field with no value blank, then call browser_request_takeover naming what is left \
for the user"})
        })
}

// ---------------------------------------------------------------- validation (code, on the JSON)

/// What validation reads besides the arguments.
pub struct Ctx<'a> {
    /// the profile, with the connected mailbox's address as `mailbox` when one is connected
    pub profile: &'a Value,
    pub today: NaiveDate,
    /// the user's messages of this chat — only for the exact-substring check of an address the results are mailed to
    pub user_texts: &'a [String],
    /// the run's transcript: `travel_pick` reads the flight search it picks from (the machine's own calls)
    pub transcript: &'a [Value],
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
        TRAVEL_PICK => check_pick(args, cx.transcript),
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
    for k in ["title_has", "title_not"] {
        let v = list_arg(a, k);
        if !v.is_empty() {
            out.insert(k.into(), json!(v));
        }
    }
    let pick = s_arg(a, "pick");
    if !pick.is_empty() {
        out.insert("pick".into(), json!(pick));
    }
    if !problems.is_empty() {
        return Checked::Invalid(problems);
    }
    Checked::Valid(Value::Object(out))
}

// ---------------------------------------------------------------- the model's pick over every flight found

/// The flights of a search the model picks from: every offer that meets the request's structured limits (the flights
/// the user named, direct, the cabin searched, the budget — fields of the arguments, compared with the offer's own
/// fields), in departure order (no price order: the pick is the model's, against the user's words). An offer without
/// an id gets `opt<n>` so each can be named.
pub fn flight_candidates(req: &Req, offers: &[Offer]) -> Vec<Offer> {
    let mut keep: Vec<Offer> = offers.to_vec();
    let named: Vec<Offer> = keep
        .iter()
        .filter(|o| mentioned(Kind::Flight, o, &req.named))
        .cloned()
        .collect();
    if !named.is_empty() {
        keep = named;
    }
    if req.nonstop {
        keep.retain(|o| o.stops == Some(0));
    }
    keep.retain(|o| {
        !o.details
            .iter()
            .any(|d| d.contains("not the cabin searched"))
    });
    if let Some(m) = req.max_price {
        keep.retain(|o| o.price.is_some_and(|p| p <= m));
    }
    // departure order; an offer without times after the others (stable: the search's own order among equals)
    keep.sort_by(|a, b| {
        (a.depart.is_empty(), &a.depart, &a.arrive).cmp(&(
            b.depart.is_empty(),
            &b.depart,
            &b.arrive,
        ))
    });
    for (i, o) in keep.iter_mut().enumerate() {
        if o.id.is_empty() {
            o.id = format!("opt{}", i + 1);
        }
    }
    keep
}

/// The days between a flight's departure and arrival dates (local at each airport): 0 = it lands on the day it
/// leaves. `None` without both times.
fn days_later(o: &Offer) -> Option<i64> {
    let d = |s: &str| NaiveDate::parse_from_str(s.get(..10)?, "%Y-%m-%d").ok();
    Some((d(&o.arrive)? - d(&o.depart)?).num_days())
}

/// One flight as the model reads it to pick: its id, name, times, stops and price — structured fields only.
fn flight_json(o: &Offer) -> Value {
    let mut m = Map::new();
    m.insert("offer_id".into(), json!(o.id));
    m.insert("flight".into(), json!(o.label));
    if !o.airline.is_empty() {
        m.insert("airline".into(), json!(o.airline));
    }
    if !o.depart.is_empty() {
        m.insert(
            "departure".into(),
            json!({"airport": o.dep_airport, "time": o.depart}),
        );
    }
    if !o.arrive.is_empty() {
        m.insert(
            "arrival".into(),
            json!({"airport": o.arr_airport, "time": o.arrive}),
        );
    }
    if let Some(n) = days_later(o) {
        m.insert("arrives_days_later".into(), json!(n));
    }
    if let Some(d) = o.duration_min {
        m.insert("duration_min".into(), json!(d));
    }
    if let Some(s) = o.stops {
        m.insert("stops".into(), json!(s));
    }
    if let Some(p) = o.price {
        m.insert("price".into(), json!(p));
        m.insert("currency".into(), json!(o.currency));
    }
    if !o.date.is_empty() {
        m.insert("date".into(), json!(o.date));
    }
    if o.preferred {
        m.insert("preferred_airline".into(), json!(true));
    }
    Value::Object(m)
}

/// A flight search's result for the model to pick from (`status: pick_needed`): every flight that meets the request's
/// structured limits, as JSON; nothing is shown yet. None meets them: `nothing_found`, with the limits unmet.
fn pick_needed(req: &Req, searched: &Value, offers: &[Offer]) -> Value {
    let flights = flight_candidates(req, offers);
    if flights.is_empty() {
        let mut unmet: Vec<String> = Vec::new();
        if req.nonstop {
            unmet.push("direct".into());
        }
        if let Some(m) = req.max_price {
            unmet.push(format!("under {}", money(m).trim_end_matches(".00")));
        }
        if unmet.is_empty() {
            unmet.push(format!("the {} searched", req.cabin.replace('_', " ")));
        }
        let unmet = unmet.join(", ");
        return json!({"status": "nothing_found", "searched": searched, "unmet": unmet,
            "results": offers.len(),
            "show": [S::fact(&format!("None of the {} results meets", offers.len()), &format!("{} 个结果都不符合", offers.len()), &unmet)],
            "hints": []});
    }
    let mut out = json!({"status": PICK_NEEDED, "searched": searched, "count": flights.len(),
        "flights": flights.iter().map(flight_json).collect::<Vec<_>>(),
        "pick_with": TRAVEL_PICK, "show": [], "hints": []});
    if !req.next.is_empty() {
        out["next_items"] = json!(req.next);
    }
    out
}

/// The flight search a `travel_pick` picks from: the validated arguments of the `travel_search` / `travel_book` call
/// whose result (the latest of those two) is `pick_needed`, the flights it listed — rebuilt from the machine's own
/// search call (never compressed) exactly as the result listed them — and the search result's own facts.
struct PickFrom {
    args: Value,
    flights: Vec<Offer>,
    result: Value,
}

fn pick_source(t: &[Value]) -> Option<PickFrom> {
    let names = [TRAVEL_SEARCH, TRAVEL_BOOK].map(|n| format!("{n}{}", workflow::RESULT_SUFFIX));
    let (ri, id) = t
        .iter()
        .enumerate()
        .flat_map(|(i, m)| {
            tool_calls(m)
                .iter()
                .map(move |tc| (i, tc))
                .collect::<Vec<_>>()
        })
        .filter(|(_, tc)| {
            get_str(tc, "id").starts_with(workflow::RESULT_PREFIX)
                && names.contains(&stored_call(tc).0)
        })
        .map(|(i, tc)| (i, get_str(tc, "id")))
        .next_back()?;
    let res = t
        .iter()
        .find(|m| get_str(m, "role") == "tool" && get_str(m, "tool_call_id") == id)
        .map(json_of)?;
    if get_str(&res, "status") != PICK_NEEDED {
        return None;
    }
    let (ci, _, args) = workflow::active(&t[..ri], &[TRAVEL_SEARCH, TRAVEL_BOOK])?;
    let req = Req::from_args(&args);
    let (_, msg) = calls_of(&t[ci..ri], spec_of(Kind::Flight).search)
        .into_iter()
        .next_back()?;
    let data = data_of(msg);
    let flights = flight_candidates(&req, &offers_in(Kind::Flight, &data));
    let mut result = json!({"searched": data.get("searched").cloned().unwrap_or(Value::Null)});
    for k in ["by_date", "compare_url", "data"] {
        if let Some(v) = data.get(k) {
            result[k] = v.clone();
        }
    }
    Some(PickFrom {
        args,
        flights,
        result,
    })
}

/// `travel_pick`: the offer ids must be ids of the flights the latest flight search listed; the validated arguments
/// carry those flights (in the model's order), the search's arguments and its facts, so the run needs nothing else.
fn check_pick(a: &Value, t: &[Value]) -> Checked {
    let Some(src) = pick_source(t) else {
        return Checked::Invalid(vec![
            "no flight search to pick from: search with travel_search first (kind flight)".into(),
        ]);
    };
    let mut ids: Vec<String> = Vec::new();
    for id in list_arg(a, "offer_ids") {
        if !ids.contains(&id) {
            ids.push(id);
        }
    }
    if ids.is_empty() {
        return Checked::Invalid(vec![
            "offer_ids: give the offer_id of each flight that meets the request, from the search result's flights (when none meets it, say so instead)".into(),
        ]);
    }
    let mut picked = Vec::new();
    let mut problems = Vec::new();
    for id in &ids {
        match src.flights.iter().find(|o| o.id == *id) {
            Some(o) => picked.push(o.clone()),
            None => problems.push(format!(
                "offer_ids: \"{id}\" is not an offer_id of the search's flights"
            )),
        }
    }
    if !problems.is_empty() {
        return Checked::Invalid(problems);
    }
    Checked::Valid(json!({"offer_ids": ids, "request": src.args,
        "offers": serde_json::to_value(&picked).unwrap_or(Value::Null), "result": src.result}))
}

/// Whether `name` is offered on the model's next turn: `travel_pick` only on the reply to a flight search's
/// `pick_needed` result (and then it is the one workflow tool offered); every other tool as before.
pub fn offered(transcript: &[Value], name: &str) -> bool {
    if name != TRAVEL_PICK {
        return true;
    }
    workflow::reply_turn(transcript)
        && workflow::latest_result(transcript).is_some_and(|r| get_str(&r, "status") == PICK_NEEDED)
}

/// The run of a `travel_pick`: exactly the flights the model picked, shown (and the first booked after a
/// `travel_book`).
fn next_pick(args: &Value, t: &[Value], m: &Machine, ids: &Ids) -> Step {
    let req = Req::from_args(&args["request"]);
    let picked: Vec<Offer> = serde_json::from_value(args["offers"].clone()).unwrap_or_default();
    let Some(first) = picked.first().cloned() else {
        return Step::Done(
            json!({"status": "nothing_found", "searched": req.searched(Kind::Flight),
            "show": [S::fact("Nothing found", "没有找到结果", &searched_line(Kind::Flight, &req))], "hints": []}),
        );
    };
    let shown = Shown {
        kind: Kind::Flight,
        spec: spec_of(Kind::Flight),
        req: &req,
        offers: &picked,
        list: picked.clone(),
        pick: first,
        rule: "picked for your request".into(),
        slot: None,
        result: &args["result"],
        search_pos: 0,
        connector: req.use_connector,
        exact: true,
    };
    present(shown, t, m, ids)
}

// ---------------------------------------------------------------- cities

use crate::places::{City, CITIES};

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

/// The user's home city: the profile's structured `home_city` (a code of the table).
fn home_city(profile: &Value) -> Option<&'static City> {
    crate::places::home_city(profile)
}

/// The currency of a city of the table, by its code.
fn city_currency(code: &str) -> &'static str {
    crate::places::city(code)
        .and_then(|c| crate::places::country(c.country))
        .map(|c| c.currency)
        .unwrap_or("")
}

/// The user's home currency: the profile's home country (`home_country`, else the home city's), else the currency
/// where the flight starts (`origin`, a city code), else "".
pub fn home_currency(profile: &Value, origin: &str) -> String {
    if let Some(c) = crate::places::country(&crate::places::home_country(profile)) {
        return c.currency.to_string();
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
                    "cabin": self.cabin, "stops": if self.nonstop { "nonstop" } else { "any" }});
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
        }
    }
}

// ---------------------------------------------------------------- the structured results

/// One option of a search (a flight, a hotel, a room, a restaurant, a place), as the result gives it. Serialized into
/// `travel_pick`'s validated arguments (the flights the model picked), so its run needs nothing but them.
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
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
    /// the provider's `free_cancellation` flag
    pub free_cancel: bool,
    pub payment: String,
    /// the provider's `prepaid` flag: the rate is charged at booking (`None`: the provider did not say)
    pub prepaid: Option<bool>,
    pub distinction: String,
    /// a Michelin Bib Gourmand (the provider's `bib_gourmand` flag)
    pub bib: bool,
    pub closed: String,
    /// the days of the week it is closed ("Monday"), the provider's `closed_days`
    pub closed_days: Vec<String>,
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
    /// a flight's departure: the airport and its local time ("2026-11-03 18:50"), from the API's own fields
    pub dep_airport: String,
    pub depart: String,
    /// a flight's arrival: the airport and its local time
    pub arr_airport: String,
    pub arrive: String,
    /// a flight's total time, minutes
    pub duration_min: Option<u64>,
}

/// A local date-time of an API's field as "YYYY-MM-DD HH:MM" ("2026-11-03T18:50:00" and "2026-11-03 18:50" alike);
/// "" for anything else.
fn local_time(v: &str) -> String {
    let t = v.trim().replace('T', " ");
    let head: String = t.chars().take(16).collect();
    match chrono::NaiveDateTime::parse_from_str(&head, "%Y-%m-%d %H:%M") {
        Ok(d) => d.format("%Y-%m-%d %H:%M").to_string(),
        Err(_) => String::new(),
    }
}

/// A flight offer's departure and arrival (airport, local time) from the result's structured fields: the offer's own
/// `departure` / `arrival` objects (Kiwi.com, a connector), else its first leg's departure and last leg's arrival
/// (Google Flights' `legs`: `from` / `depart`, `to` / `arrive`). Never read from a prose line.
fn flight_ends(o: &Value) -> (String, String, String, String) {
    let (mut da, mut dt) = (
        get_str(&o["departure"], "airport"),
        local_time(&get_str(&o["departure"], "time")),
    );
    let (mut aa, mut at) = (
        get_str(&o["arrival"], "airport"),
        local_time(&get_str(&o["arrival"], "time")),
    );
    let legs: &[Value] = o
        .get("legs")
        .and_then(Value::as_array)
        .map_or(&[], Vec::as_slice);
    if dt.is_empty() {
        if let Some(l) = legs.first() {
            da = get_str(l, "from");
            dt = local_time(&get_str(l, "depart"));
        }
    }
    if at.is_empty() {
        if let Some(l) = legs.last() {
            aa = get_str(l, "to");
            at = local_time(&get_str(l, "arrive"));
        }
    }
    (da, dt, aa, at)
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
    // a bare figure ("562.0": Kiwi.com's round-trip price) gets its currency, as every amount the reply carries
    // (travel_brave_real 2026-11-01: "— 562.0" in the answer, no currency)
    let price_text = match price_text.trim().replace(',', "").parse::<f64>() {
        Ok(p) if !currency.is_empty() => {
            format!("{currency} {}", money(p).trim_end_matches(".00"))
        }
        _ => price_text,
    };
    let rating_text = first_str(o, &["rating"]);
    let rating = num_of(o.get("rating")).map(|r| {
        if rating_text.contains("/10") || r > 5.0 {
            r / 2.0
        } else {
            r
        }
    });
    let cancellation = first_str(o, &["cancellation", "refund_rules"]);
    // the provider's flag; the policy's text is shown, never searched for "free"
    let free_cancel = truthy(o.get("free_cancellation").unwrap_or(&Value::Null));
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
    let (dep_airport, depart, arr_airport, arrive) = if kind == Kind::Flight {
        flight_ends(o)
    } else {
        Default::default()
    };
    Offer {
        dep_airport,
        depart,
        arr_airport,
        arrive,
        duration_min: o
            .get("duration_min")
            .and_then(Value::as_f64)
            .filter(|m| *m > 0.0)
            .map(|m| m.round() as u64),
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
        prepaid: o.get("prepaid").and_then(Value::as_bool),
        distinction: first_str(o, &["distinction"]),
        bib: truthy(o.get("bib_gourmand").unwrap_or(&Value::Null)),
        closed: first_str(o, &["closed"]),
        closed_days: o
            .get("closed_days")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .map(|d| d.trim().to_string())
            .collect(),
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

fn has_word(hay_lc: &str, w: &str) -> bool {
    Regex::new(&format!(r"\b{}\b", regex::escape(&w.to_lowercase())))
        .map(|r| r.is_match(hay_lc))
        .unwrap_or(false)
}

/// Whether one of the names the user named (the model's `named` argument) is this option, compared whole with the
/// option's structured fields: its label or id (any case, spaces aside), and for a flight its airline or one of its
/// flight codes ("SQ 638" = "SQ638") — never a word searched in a name.
fn mentioned(kind: Kind, o: &Offer, names: &[String]) -> bool {
    let norm = |s: &str| s.split_whitespace().collect::<String>().to_lowercase();
    let names: Vec<String> = names
        .iter()
        .map(|n| norm(n))
        .filter(|n| !n.is_empty())
        .collect();
    if names.is_empty() {
        return false;
    }
    let mut own = vec![norm(&o.label), norm(&o.id)];
    if kind == Kind::Flight {
        static CODE: LazyLock<Regex> =
            LazyLock::new(|| Regex::new(r"\b[A-Z][A-Z0-9]\s?\d{2,4}\b").unwrap());
        own.push(norm(&o.airline));
        own.extend(CODE.find_iter(&o.label).map(|m| norm(m.as_str())));
    }
    own.retain(|x| !x.is_empty());
    names.iter().any(|n| own.contains(n))
}

/// A Michelin restaurant's level, from the provider's structured fields: its stars (`stars`, 0–3) and whether it is a
/// Bib Gourmand (`bib_gourmand`); without them, its `distinction` taken whole as one of the guide's values ("3 Stars",
/// "2 Stars", "1 Star", "Bib Gourmand", any case, a note in brackets after it aside) — never a word searched in it.
fn michelin_level(o: &Offer) -> (u32, bool) {
    if o.bib {
        return (0, true);
    }
    if let Some(n) = o.stars.filter(|n| *n <= 3) {
        return (n as u32, false);
    }
    let d = o
        .distinction
        .split(" (")
        .next()
        .unwrap_or("")
        .trim()
        .to_lowercase();
    match d.as_str() {
        "3 stars" | "three stars" => (3, false),
        "2 stars" | "two stars" => (2, false),
        "1 star" | "one star" => (1, false),
        "bib gourmand" => (0, true),
        _ => (0, false),
    }
}

/// Whether a restaurant is closed on `day` ("Monday"): one of the provider's `closed_days`, or — without them — one of
/// the items of its `closed` list ("Monday, Tuesday"), each compared whole.
fn closed_on(o: &Offer, day: &str) -> bool {
    if !o.closed_days.is_empty() {
        return o.closed_days.iter().any(|d| d.eq_ignore_ascii_case(day));
    }
    o.closed
        .split(',')
        .any(|d| d.trim().eq_ignore_ascii_case(day))
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
                    let (stars, bib) = michelin_level(o);
                    match d.as_str() {
                        "stars" => stars > 0,
                        "bib gourmand" => bib,
                        "3 stars" => stars == 3,
                        "2 stars" => stars == 2,
                        "1 star" => stars == 1,
                        _ => true,
                    }
                });
                rule.push(req.distinction.clone());
            }
            if let Some(date) = req.date {
                let day = date.format("%A").to_string();
                keep.retain(|o| !closed_on(o, &day));
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
    let dist_rank = |o: &Offer| michelin_level(o).0;
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

/// The option the user picked on a choice card (the model's `pick` argument): its label or id exactly as the card
/// listed it (any case, spaces aside).
fn by_label<'a>(offers: &'a [Offer], label: &str) -> Option<&'a Offer> {
    let norm = |s: &str| {
        s.split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .to_lowercase()
    };
    let l = norm(label);
    if l.is_empty() {
        return None;
    }
    offers
        .iter()
        .find(|o| norm(&o.label) == l || (!o.id.is_empty() && norm(&o.id) == l))
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

/// An offer as items to show (agent/show.rs): `- {label}: {offer} — {price}{extra}` and its booking link.
fn offer_show(label: &str, label_zh: &str, o: &Offer, extra: &str) -> Vec<Value> {
    let price = if o.price_text.is_empty() {
        o.price
            .map(|p| format!("{} {}", o.currency, money(p).trim_end_matches(".00")))
            .unwrap_or_default()
    } else {
        o.price_text.clone()
    };
    let mut v = o.label.clone();
    if !price.trim().is_empty() {
        v = format!("{v} — {}", price.trim());
    }
    if !o.book_at.is_empty() {
        v = format!("{v} (or on {})", o.book_at.join(" or "));
    }
    let mut out = vec![S::fact(label, label_zh, &format!("{v}{extra}"))];
    if !o.url.is_empty() {
        out.push(S::link(
            &format!("Book {}", o.label),
            &format!("预订 {}", o.label),
            "",
            &o.url,
        ));
    }
    out
}

/// A flight offer's departure → arrival line from the result's own details (the first one with a clock time), seconds
/// dropped ("SIN 2026-12-02 08:00 → HND 2026-12-02 20:00").
fn schedule_of(o: &Offer) -> Option<String> {
    static TIME: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\b\d{1,2}:\d{2}\b").unwrap());
    static SECS: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"\b(\d{1,2}:\d{2}):\d{2}\b").unwrap());
    o.details
        .iter()
        .find(|d| TIME.is_match(d))
        .map(|d| SECS.replace_all(d, "$1").trim().to_string())
}

/// A flight's departure → arrival line from its structured times ("HKG 2026-11-03 18:50 → SIN 2026-11-03 22:50"), else
/// the result's own detail line with a clock time.
fn times_of(o: &Offer) -> Option<String> {
    if !o.depart.is_empty() && !o.arrive.is_empty() {
        return Some(
            format!(
                "{} {} → {} {}",
                o.dep_airport, o.depart, o.arr_airport, o.arrive
            )
            .trim()
            .to_string(),
        );
    }
    schedule_of(o)
}

/// Whether a flight offer is on one of the user's preferred airlines: the search flagged it, or its airline or label
/// names one of them (a structured field of the result against the profile's value).
fn on_airline(o: &Offer, airlines: &[String]) -> bool {
    o.preferred
        || airlines.iter().any(|a| {
            let a = a.trim().to_lowercase();
            !a.is_empty()
                && (o.airline.to_lowercase().contains(&a) || has_word(&o.label.to_lowercase(), &a))
        })
}

/// A web fallback's results (`{"results": [{"title", "url", "snippet"}]}`), the first four, as the search gave them:
/// the snippet is natural language for the model to read (a price in it is the model's to quote, never read by code).
fn web_pages(result: &Value) -> Vec<Value> {
    result
        .get("results")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|r| get_str(r, "url").starts_with("http") && !ad_link(&get_str(r, "url")))
        .take(4)
        .map(|r| {
            let snippet = take_chars(get_str(r, "snippet").trim(), 160).to_string();
            let mut o = json!({"title": get_str(r, "title"), "url": get_str(r, "url")});
            if !snippet.is_empty() {
                o["snippet"] = json!(snippet);
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

// ---------------------------------------------------------------- the booking page

fn denied(m: &Value) -> bool {
    let c = content_of(m);
    c.starts_with("DENIED") || c.to_lowercase().contains("\"status\": \"denied\"")
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
        return next_shop(&ShopReq::from_args(args), t, &ids);
    }
    if tool == TRAVEL_PICK {
        return next_pick(args, t, m, &ids);
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
        if let Some(item) = resumed_item(t, pos(t, search_msg), &req.pick) {
            return book(spec, req, t, ids, &item);
        }
    }
    let result = data_of(search_msg);
    let offers = offers_in(kind, &result);
    let next_items = json!(req.next);
    let compare = get_str(&result, "compare_url");
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
            let mut show: Vec<Value> = pages
                .iter()
                .map(|p| S::link("Web page", "网页", &get_str(p, "title"), &get_str(p, "url")))
                .collect();
            show.push(S::fact(
                "Prices",
                "价格",
                "these are web pages about it: any price on them is a \"from\" price, not a quote for the dates",
            ));
            if !compare.is_empty() {
                show.push(S::link(
                    "Compare prices for the dates on Kayak",
                    "在 Kayak 比较这些日期的价格",
                    "Kayak",
                    &compare,
                ));
            }
            let mut out = json!({"status": "shown", "source": "web", "searched": searched,
                                 "options": [], "web_pages": pages, "show": show, "hints": []});
            if !compare.is_empty() {
                out["links"] = json!({"compare_url": compare});
            }
            if !req.next.is_empty() {
                out["next_items"] = next_items;
            }
            return Step::Done(out);
        }
        return Step::Done(json!({"status": "nothing_found", "searched": searched,
            "show": [S::fact("Nothing found", "没有找到结果", &searched_line(kind, req))], "hints": []}));
    }
    // a search limited to the preferred airlines that found none of them: say so, ask before searching all airlines
    if kind == Kind::Flight
        && !req.airlines.is_empty()
        && result["searched"]["preferred_found"].as_u64() == Some(0)
    {
        return Step::Done(no_preferred(req, &searched, &offers));
    }
    let chosen = by_label(&offers, &req.pick).cloned();
    // a flight: EVERY flight found goes back as JSON and the model picks exactly the ones the user asked for
    // (`travel_pick`) — code never picks a flight by a rule the user did not give ("the last flight that lands before
    // midnight" came back as the cheapest, 2026-11-02)
    if kind == Kind::Flight {
        let Some(o) = chosen else {
            return Step::Done(pick_needed(req, &searched, &offers));
        };
        let exact = Shown {
            kind,
            spec,
            req,
            offers: &offers,
            list: vec![o.clone()],
            pick: o,
            rule: "your pick".into(),
            slot: None,
            result: &result,
            search_pos: pos(t, search_msg),
            connector,
            exact: true,
        };
        return present(exact, t, m, ids);
    }
    let (list, rule) = ranked(kind, req, &offers);
    let (pick, rule) = match chosen {
        Some(o) => (o, "your pick".to_string()),
        None => match list.first() {
            Some(o) => (o.clone(), rule),
            None => {
                return Step::Done(json!({"status": "nothing_found", "searched": searched,
                    "unmet": rule.trim_end_matches(", the cheapest"),
                    "results": offers.len(),
                    "options": offers.iter().take(4).map(option_json).collect::<Vec<_>>(),
                    "show": [S::fact(&format!("None of the {} results meets", offers.len()), &format!("{} 个结果都不符合", offers.len()),
                                     rule.trim_end_matches(", the cheapest"))],
                    "hints": []}));
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
                    "show": [S::fact(&format!("No table on {} for {}", req.date.map(long).unwrap_or_default(), req.party_or(kind)),
                                     &format!("{} {} 人没有空位", req.date.map(long).unwrap_or_default(), req.party_or(kind)),
                                     &checked.join(", "))],
                    "hints": []}));
            }
        }
    }
    let shown = Shown {
        kind,
        spec,
        req,
        offers: &offers,
        list,
        pick,
        rule,
        slot,
        result: &result,
        search_pos,
        connector,
        exact: false,
    };
    present(shown, t, m, ids)
}

/// What a search's result shows, once the pick is made.
struct Shown<'a> {
    kind: Kind,
    spec: &'static BookingSpec,
    req: &'a Req,
    /// every option of the search
    offers: &'a [Offer],
    /// the options that meet the request, in the order to show (`exact`: the ones to show, and only them)
    list: Vec<Offer>,
    pick: Offer,
    rule: String,
    /// a restaurant's time: (time, booking url, the availability result)
    slot: Option<(String, String, Value)>,
    /// the search's result (its `searched`, `by_date`, `compare_url`, `data`)
    result: &'a Value,
    /// where the search's result is in the transcript (the machine's calls after it are this run's)
    search_pos: usize,
    connector: bool,
    /// a flight: exactly the flights picked (the model's pick over every flight found, or the user's own), nothing
    /// added — no other options on the card, no other airline's fare
    exact: bool,
}

/// The choice card, the pick's own data, the mail and the result — or, to book, the booking stage.
fn present(s: Shown, t: &[Value], m: &Machine, ids: &Ids) -> Step {
    let Shown {
        kind,
        spec,
        req,
        offers,
        list,
        pick,
        rule,
        slot,
        result,
        search_pos,
        connector,
        exact,
    } = s;
    let searched = req.searched(kind);
    let next_items = json!(req.next);
    let compare = get_str(result, "compare_url");
    let source = get_str(result, "data");
    // 3. the choice card with the pick shown (once; exactly the picked flights, and none for a single one)
    let shown: Vec<Offer> = if exact {
        list.clone()
    } else {
        let mut v = vec![pick.clone()];
        v.extend(
            list.iter()
                .filter(|o| o.label != pick.label)
                .take(3)
                .cloned(),
        );
        v
    };
    if (!exact || shown.len() > 1)
        && calls_of(t, "present_choices")
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
                       "note": if o.label == pick.label && !exact { format!("picked: {rule}") } else { String::new() }})
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
    if !pick.id.is_empty() {
        out["pick"]["offer_id"] = json!(pick.id);
    }
    let must: Vec<String> = Vec::new();
    // the facts the user must read: appended to the reply by code (agent/show.rs)
    let mut show: Vec<Value> = vec![S::fact("Searched", "搜索", &searched_line(kind, req))];
    if exact {
        // exactly the flights picked, each with its times and booking link
        let many = shown.len() > 1;
        for (i, o) in shown.iter().enumerate() {
            let (en, zh) = if many {
                (format!("Flight {}", i + 1), format!("航班 {}", i + 1))
            } else {
                ("Flight".to_string(), "航班".to_string())
            };
            show.extend(offer_show(&en, &zh, o, ""));
            if let Some(when) = times_of(o) {
                show.push(S::fact(&o.label, &o.label, &when));
            }
        }
    } else {
        show.extend(offer_show("Picked", "推荐", &pick, &format!(" ({rule})")));
        // a flight's departure and arrival times, from the result (travel_kiwi_real 2026-11-01: the reply gave
        // neither the day nor the times of the pick)
        if let Some(when) = (kind == Kind::Flight).then(|| times_of(&pick)).flatten() {
            show.push(S::fact(&pick.label, &pick.label, &when));
        }
    }
    // the user's preferred airline: its best fare in the results, or that it has none (travel_ask_real 2026-11-01:
    // the cheapest was another airline and ANA went unmentioned) — not beside flights picked exactly
    if kind == Kind::Flight
        && !exact
        && !req.airlines.is_empty()
        && !on_airline(&pick, &req.airlines)
    {
        let names = req.airlines.join(" or ");
        match offers.iter().find(|o| on_airline(o, &req.airlines)) {
            Some(p) => show.extend(offer_show(
                &format!("Your preferred airline {names}"),
                &format!("你偏好的航空公司 {names}"),
                p,
                &schedule_of(p).map(|w| format!(", {w}")).unwrap_or_default(),
            )),
            None => show.push(S::fact(
                &format!("Your preferred airline {names}"),
                &format!("你偏好的航空公司 {names}"),
                &format!("no flight in the results for {}", searched_line(kind, req)),
            )),
        }
    }
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
        show.push(S::fact("Searched as", "搜索舱位", &cabin));
    }
    if let Some((time, _, _)) = &slot {
        out["pick"]["time"] = json!(time);
        show.push(S::fact(
            &format!("Table at {}", pick.label),
            &format!("{} 的空位", pick.label),
            &format!("{time} available"),
        ));
    }
    if pick.preferred {
        show.push(S::fact(
            &pick.label,
            &pick.label,
            "on your preferred airline",
        ));
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
            show.push(S::fact(
                "Sold by",
                "销售方",
                &s.iter()
                    .map(|x| get_str(x, "book_with"))
                    .collect::<Vec<_>>()
                    .join(", "),
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
            show.extend(offer_show(
                &format!("Room at {}", pick.label),
                &format!("{} 的房型", pick.label),
                r,
                &format!(" ({why})"),
            ));
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
        show.push(S::fact(
            "Days searched",
            "搜索日期",
            &format!(
                "{} days in one search ({} to {}){}",
                days.len(),
                days[0],
                days[days.len() - 1],
                if pick.date.is_empty() {
                    String::new()
                } else {
                    format!("; the pick flies on {}", pick.date)
                }
            ),
        ));
    }
    if !compare.is_empty() {
        out["links"] = json!({"compare_url": compare});
        show.push(S::link(
            "Compare prices on Kayak",
            "在 Kayak 比价",
            "Kayak",
            &compare,
        ));
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
                show.push(S::fact(
                    &format!("Not sent to {to}"),
                    &format!("未发送到 {to}"),
                    "no mail account is connected (connect Gmail in Settings)",
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
                    show.push(S::fact("Sent to", "已发送到", &to));
                } else {
                    out["mailed"]["error"] = json!(take_chars(&c, 200));
                    show.push(S::fact(
                        &format!("Not sent to {to}"),
                        &format!("未发送到 {to}"),
                        take_chars(&c, 120),
                    ));
                }
            }
        }
        out["show"] = json!(show);
        out["hints"] = json!(must);
        return Step::Done(out);
    }
    // 4. the item to book
    let item = match item_for(
        spec,
        req,
        t,
        m,
        ids,
        &pick,
        slot.as_ref(),
        &rooms_args,
        connector,
    ) {
        Ok(i) => i,
        Err(step) => return step,
    };
    if item.url.is_empty() {
        out["booking_link"] = Value::Null;
        show.push(S::fact(
            &format!("{}: no online booking link", pick.label),
            &format!("{}：没有在线预订链接", pick.label),
            "book it by phone or on its own site",
        ));
        out["show"] = json!(show);
        out["hints"] = json!(must);
        return Step::Done(out);
    }
    book(spec, req, t, ids, &item)
}

/// The answer when a search limited to the preferred airlines found none of them: no second search now — the question
/// whether to search all airlines (and what else came back, when the API searched them all).
fn no_preferred(req: &Req, searched: &Value, others: &[Offer]) -> Value {
    let mut show = vec![S::fact(
        &format!("No {} flight", req.airlines.join(" or ")),
        &format!("没有 {} 的航班", req.airlines.join(" 或 ")),
        &searched_line(Kind::Flight, req),
    )];
    for o in others.iter().take(4) {
        show.extend(offer_show("Other airline", "其他航空公司", o, ""));
    }
    json!({"status": "nothing_found", "searched": searched, "preferred_found": 0,
           "options": others.iter().take(4).map(option_json).collect::<Vec<_>>(),
           "next_question": "search_all_airlines", "show": show, "hints": []})
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
    connector: bool,
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
            delivery: format!("E-ticket to {}", contact_email(m.profile, &req.contact)),
            note: pick.cancellation.clone(),
            lines: pick.details.clone(),
        },
        Kind::Hotel => {
            // the rooms of the picked hotel, then the room by rule — on the connector the search ran on
            // (travel_hotel_real 2026-11-01: the search went to the agency's connector, the rooms to the SerpApi tool
            // without a key; Sentinel refused it and the hotel's page for other dates was booked)
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
                    // the provider's flag; a room it says nothing about is treated as charged (the payment is
                    // approved and the card filled rather than "nothing charged" claimed)
                    let prepaid = r.prepaid.unwrap_or(true);
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
/// the choice card, else the user's own pick (`pick`: a flight's pick shows no card). `None` before the booking link
/// was opened.
fn resumed_item(t: &[Value], search_pos: usize, pick: &str) -> Option<Item> {
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
        .unwrap_or_else(|| pick.to_string());
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

/// The email the booking's confirmation goes to: the profile's own, else the connected mailbox's address
/// (`profile["mailbox"]`), else the one the user gave (the `contact` argument).
fn contact_email(profile: &Value, contact: &Value) -> String {
    ["email_personal", "email", "mailbox"]
        .iter()
        .map(|k| get_str(profile, k).trim().to_string())
        .chain(std::iter::once(
            get_str(contact, "email").trim().to_string(),
        ))
        .find(|e| !e.is_empty())
        .unwrap_or_default()
}

/// The booking stage, after the pick: the Vault's list, the booking link opened, then — when money is charged —
/// `purchase_confirm` on that page, and the page handed to `browser-use` (`status: browser_use`) — no booking site has
/// a form function. The approval comes after the page is open so it names the site the booking lands on: Sentinel
/// binds it to the open page's host and checks the total that page shows (travel_book_real 2026-11-02: approved
/// before the hotel's page opened, the approval named kiwi.com — the flight's page still open — and the Google
/// redirect's host).
fn book(spec: &BookingSpec, req: &Req, t: &[Value], ids: &Ids, item: &Item) -> Step {
    let card_needed = item.charged;
    let vault = calls_of(t, "vault_list")
        .last()
        .map(|(_, msg)| content_of(msg));
    if (card_needed || spec.kind == Kind::Flight) && vault.is_none() {
        return ids.calls("vault_list", json!({}));
    }
    let cards: Vec<VaultCard> = vault.as_deref().map(vault_cards).unwrap_or_default();
    // the card the user named (the `card` argument), else the first with its details
    let card = pick_card(&cards, std::slice::from_ref(&req.card));
    let card_ok = card.is_some_and(|c| !c.name_only);
    // the booking link (once)
    let nav = calls_of(t, "browser_navigate")
        .into_iter()
        .rfind(|(a, _)| sget(a, "url") == item.url);
    let Some((nav_args, nav_msg)) = nav else {
        return ids.calls("browser_navigate", json!({"url": item.url}));
    };
    let page_url = landed(&nav_args, nav_msg);
    if card_needed {
        match calls_of(t, "purchase_confirm").last() {
            None => {
                let mut a = json!({"site": host_of(&page_url), "items": [{"name": item.name, "qty": 1, "price": item.total.unwrap_or(0.0)}],
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
                    "show": [S::fact("Nothing was booked", "没有预订", &format!("the purchase of {} was not approved", item.name))],
                    "hints": []}),
                );
            }
            Some(_) => {}
        }
    }
    // the page is the browser-use skill's from here
    let mut booking = item.to_json();
    if let (Some(c), true, true) = (card, card_ok, card_needed) {
        booking["card"] = json!(c.label);
    }
    Step::Done(
        json!({"status": "browser_use", "url": page_url, "booking": booking,
        "show": handover_show(item, &page_url), "hints": []}),
    )
}

/// The facts the user reads under the reply when a booking page is handed to `browser-use`: the booking page, what is
/// being booked and its price from the search. A charged item's amount is the approved `purchase_confirm`'s ("Amount you
/// approved", `guards::purchase_total_facts`), so it is not repeated here.
fn handover_show(item: &Item, page_url: &str) -> Vec<Value> {
    let mut show = vec![S::link(
        "Booking page (open in the browser)",
        "预订页面（已在浏览器中打开）",
        "",
        page_url,
    )];
    if !item.name.is_empty() {
        show.push(S::fact("To book", "预订", &item.name));
    }
    if let (Some(total), false) = (item.total.filter(|t| *t > 0.0), item.charged) {
        show.push(S::amount(
            "Price in the search",
            "搜索中的价格",
            total,
            &item.currency,
        ));
    }
    show
}

// ---------------------------------------------------------------- shopping

/// A shopping run: the shop's link opened and the page handed to `browser-use` (no shop has a form function). The
/// criteria go with the result: the model lists the page's products as `present_choices` options with their price,
/// stars and ratings, and code ranks them by these criteria (`listings::Ranking`).
fn next_shop(sr: &ShopReq, t: &[Value], ids: &Ids) -> Step {
    let url = sr.url.clone();
    let Some((a, msg)) = calls_of(t, "browser_navigate")
        .into_iter()
        .rfind(|(a, _)| sget(a, "url") == url)
    else {
        return ids.calls("browser_navigate", json!({"url": url}));
    };
    let page = landed(&a, msg);
    let mut out = json!({"status": "browser_use", "url": page, "action": sr.action,
        "criteria": sr.ranking.to_json(),
        "show": [S::link("The shop's page (open in the browser)", "商店页面（已在浏览器中打开）", "", &page)],
        "hints": []});
    if !sr.pick.is_empty() {
        out["pick"] = json!(sr.pick);
    }
    Step::Done(out)
}

#[cfg(test)]
mod tests;
