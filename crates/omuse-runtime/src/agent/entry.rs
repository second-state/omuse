//! The entry-requirements machine behind the workflow tool `entry_requirements` (docs/json-tools.md): what a trip's
//! destinations ask of the traveller before they arrive, looked up by code every time — each destination's visa-free
//! stay from the official table with its official page (`facts::visa_rules`), and the destination's electronic arrival
//! card from the card table (`driver::CARDS`: its official site and its filing window, counted back from the arrival
//! date the model passes as JSON). The result's `show` carries those facts, so code appends them to the reply whatever
//! the reply says (travel_info_real 2026-11-02: the model sometimes never called `visa_rules`, and the block had no
//! visa-free days).
//!
//! The machine has no code calls: its result is ready as soon as the arguments are valid. The skills that ask through
//! it (`entry-requirements`, `arrival-cards`) require the call before a final answer (`agent/required.rs`).

use chrono::{Duration, NaiveDate};
use omuse_common::util::get_str;
use serde_json::{json, Map, Value};

use super::driver::{CardSpec, Step, CARDS};
use super::judgements::{names_place, VisaPage, VISA_PAGES};
use super::show as S;
use super::tools::fn_;
use super::util::{arr, sget};
use super::workflow::Checked;

/// The workflow tool.
pub const ENTRY_REQUIREMENTS: &str = "entry_requirements";
/// The machine's tools.
pub const TOOLS: [&str; 1] = [ENTRY_REQUIREMENTS];
/// The skills whose runs ask through it.
pub const SKILLS: [&str; 2] = ["entry-requirements", "arrival-cards"];
/// How many destinations one call looks up.
const MAX_DESTINATIONS: usize = 6;
/// The arrival card that is a paid authorisation, not a free card.
const PAID_CARD: &str = "uk-eta";

/// The catalog entry of the workflow tool.
pub fn tool_schemas() -> Vec<Value> {
    let date = |what: &str| json!({"type": "string", "description": format!("YYYY-MM-DD, {what} (resolve relative dates from today in the live state)")});
    vec![fn_(
        ENTRY_REQUIREMENTS,
        "What a trip's destinations ask of the traveller before arrival, looked up by code: each destination's visa-free \
stay from the official table with its official page (ETIAS's status for Schengen), and its electronic arrival card — \
the official site and the day the filing window opens, counted from the arrival date. Call it for any question about \
entering a country (visas, arrival cards, what to file before a trip), with every destination of the trip in order. \
Write the reply from the JSON result (code appends the facts in show to the reply); its reminders are the days to file \
each card, for schedule_create when the user wants reminders.",
        json!({
            "destinations": {"type": "array", "description": "the trip's destinations in the order the user travels",
                "items": {"type": "object", "properties": {
                    "place": {"type": "string", "description": "the country or city (\"Bangkok\", \"Singapore\")"},
                    "arrive": date("the day the user arrives there"),
                    "leave": date("the day the user leaves, when known")
                }, "required": ["place"]}},
            "nationality": {"type": "string", "description": "the passport's nationality when the user said; empty = the profile's"}
        }),
        &["destinations"],
    )]
}

fn date_of(v: &str) -> Option<NaiveDate> {
    NaiveDate::parse_from_str(v.trim(), "%Y-%m-%d").ok()
}

/// The arguments checked: at least one destination with a place; the dates ISO, not in the past, a departure not
/// before its arrival; the nationality the call's, else the profile's.
pub fn check(tool: &str, a: &Value, profile: &Value, today: NaiveDate) -> Checked {
    if tool != ENTRY_REQUIREMENTS {
        return Checked::Invalid(vec![format!("unknown tool {tool}")]);
    }
    let mut problems = Vec::new();
    let mut dests = Vec::new();
    for (i, d) in arr(a, "destinations")
        .iter()
        .take(MAX_DESTINATIONS)
        .enumerate()
    {
        let place = sget(d, "place").trim().to_string();
        if place.is_empty() {
            continue;
        }
        let mut out = Map::new();
        out.insert("place".into(), json!(place));
        let arrive = sget(d, "arrive");
        let arrive = if arrive.trim().is_empty() {
            None
        } else {
            match date_of(&arrive) {
                Some(x) if x < today => {
                    problems.push(format!(
                        "destinations[{i}].arrive: {x} is in the past (today is {today})"
                    ));
                    None
                }
                Some(x) => Some(x),
                None => {
                    problems.push(format!(
                        "destinations[{i}].arrive: \"{arrive}\" is not a date: give YYYY-MM-DD"
                    ));
                    None
                }
            }
        };
        if let Some(x) = arrive {
            out.insert("arrive".into(), json!(x.format("%Y-%m-%d").to_string()));
        }
        let leave = sget(d, "leave");
        if !leave.trim().is_empty() {
            match date_of(&leave) {
                Some(l) if arrive.is_some_and(|x| l < x) => problems.push(format!(
                    "destinations[{i}].leave: {l} is before the arrival"
                )),
                Some(l) => {
                    out.insert("leave".into(), json!(l.format("%Y-%m-%d").to_string()));
                }
                // an optional field that cannot be read is dropped
                None => {}
            }
        }
        dests.push(Value::Object(out));
    }
    if !problems.is_empty() {
        return Checked::Invalid(problems);
    }
    if dests.is_empty() {
        return Checked::NeedsInput(vec![
            "destinations: the countries or cities of the trip".into()
        ]);
    }
    let nationality = Some(sget(a, "nationality"))
        .filter(|n| !n.trim().is_empty())
        .unwrap_or_else(|| get_str(profile, "nationality"));
    Checked::Valid(json!({"destinations": dests, "nationality": nationality.trim()}))
}

/// What the machine reads besides its arguments.
pub struct Machine {
    pub zh: bool,
    pub today: NaiveDate,
}

/// The machine's one step: the result, from the tables (no page, no model).
pub fn next(_tool: &str, args: &Value, _t: &[Value], m: &Machine) -> Step {
    Step::Done(result(args, m.zh, m.today))
}

/// The official visa page of a destination (lower case): its code, its name or a city in it.
fn visa_page(low: &str) -> Option<&'static VisaPage> {
    VISA_PAGES.iter().find(|p| {
        p.code.eq_ignore_ascii_case(low)
            || p.country.eq_ignore_ascii_case(low)
            || names_place(low, p.words)
    })
}

/// The United Kingdom's names (it has no page in `VISA_PAGES`; its card is the ETA).
const UK_WORDS: [&str; 8] = [
    "united kingdom",
    "uk",
    "britain",
    "great britain",
    "england",
    "scotland",
    "london",
    "英国",
];

/// The arrival card of a destination (lower case), by the country of its visa page or by the card's own country.
fn card_for(low: &str) -> Option<&'static CardSpec> {
    let country = visa_page(low).map(|p| p.country.to_lowercase());
    CARDS.iter().find(|c| {
        let cc = c.country.to_lowercase();
        match &country {
            Some(pc) => *pc == cc || pc.ends_with(&format!(" {cc}")),
            None => {
                low == cc
                    || names_place(low, &[cc.as_str()])
                    || (c.skill == PAID_CARD && names_place(low, &UK_WORDS))
            }
        }
    })
}

/// When to file a card: (the day the window opens — today when it is already open —, the line shown).
fn filing(
    c: &CardSpec,
    arrive: Option<NaiveDate>,
    today: NaiveDate,
    zh: bool,
) -> (Option<NaiveDate>, String) {
    let paid = c.skill == PAID_CARD;
    let cost = match (paid, zh) {
        (true, false) => "paid (the ETA fee), on the official site only",
        (true, true) => "需付费（ETA 费用），只在官方网站办理",
        (false, false) => "free, on the official site only",
        (false, true) => "免费，只在官方网站填写",
    };
    if c.window_days <= 0 {
        let when = match (paid, zh) {
            (true, false) => "apply a few days before you travel",
            (true, true) => "出发前几天申请",
            (false, false) => "before you arrive (no filing window)",
            (false, true) => "抵达前填写（没有填写时间窗口）",
        };
        return (None, format!("{when} — {cost}"));
    }
    let w = c.window_days;
    match arrive {
        Some(a) => {
            let opens = a - Duration::days(w);
            let from = opens.max(today);
            let line = match (from > opens, zh) {
                (false, false) => format!(
                    "file from {opens} (the window opens {w} days before you arrive on {a}) — {cost}"
                ),
                (true, false) => format!(
                    "the window is open now: file before you arrive on {a} — {cost}"
                ),
                (false, true) => {
                    format!("{opens} 起填写（抵达 {a} 前 {w} 天开放）— {cost}")
                }
                (true, true) => format!("现在已可填写：请在 {a} 抵达前完成 — {cost}"),
            };
            (Some(from), line)
        }
        None => (
            None,
            if zh {
                format!("抵达前 {w} 天内填写 — {cost}")
            } else {
                format!("file within {w} days before you arrive — {cost}")
            },
        ),
    }
}

/// The result: the visa rules of every destination (the official table's days and page) and each destination's
/// arrival card (its official site and the day to file it), with `show` (appended to the reply by code), the
/// `reminders` to file (a day per card) and `hints`.
pub fn result(args: &Value, zh: bool, today: NaiveDate) -> Value {
    let nationality = sget(args, "nationality");
    let dests = arr(args, "destinations");
    let places: Vec<String> = dests.iter().map(|d| sget(d, "place")).collect();
    let visa = super::facts::visa_rules(
        &json!({"destinations": places, "nationality": nationality}),
        &nationality,
        zh,
    );
    let mut show: Vec<Value> = S::items(&visa);
    let mut hints: Vec<String> = super::workflow::hints(&visa);
    let mut rows = Vec::new();
    let mut reminders = Vec::new();
    for d in dests {
        let place = sget(d, "place");
        let low = place.trim().to_lowercase();
        let arrive = date_of(&sget(d, "arrive"));
        let mut row = json!({"place": place, "arrive": d.get("arrive"), "leave": d.get("leave"), "arrival_card": null});
        if let Some(c) = card_for(&low) {
            let (from, line) = filing(c, arrive, today, zh);
            row["arrival_card"] = json!({"name": c.name, "country": c.country, "official_site": c.start_url,
                "window_days": c.window_days, "file_from": from.map(|x| x.format("%Y-%m-%d").to_string()),
                "paid": c.skill == PAID_CARD});
            if !show.iter().any(|s| get_str(s, "label") == c.name) {
                show.push(S::fact(c.name, c.name, &line));
                show.push(S::link(
                    &format!("{} (official site)", c.name),
                    &format!("{}（官方网站）", c.name),
                    "",
                    c.start_url,
                ));
                hints.push(format!("{}: {line} ({})", c.name, c.start_url));
                if let Some(f) = from {
                    reminders.push(json!({"what": format!("File the {}", c.name),
                                          "on": f.format("%Y-%m-%d").to_string(), "url": c.start_url}));
                }
            }
        }
        rows.push(row);
    }
    json!({"status": "shown", "nationality": nationality, "destinations": rows,
           "visa": visa.get("destinations"), "unknown": visa.get("unknown"),
           "reminders": reminders, "show": show, "hints": hints})
}

#[cfg(test)]
mod tests {
    use super::*;

    fn today() -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 11, 2).unwrap()
    }

    #[test]
    fn check_needs_a_destination_rejects_past_dates_and_takes_the_profiles_nationality() {
        let p = json!({"nationality": "Australia"});
        assert!(matches!(
            check(
                ENTRY_REQUIREMENTS,
                &json!({"destinations": []}),
                &p,
                today()
            ),
            Checked::NeedsInput(_)
        ));
        let Checked::Invalid(pr) = check(
            ENTRY_REQUIREMENTS,
            &json!({"destinations": [{"place": "Bangkok", "arrive": "2026-10-01"}]}),
            &p,
            today(),
        ) else {
            panic!("a past arrival is invalid")
        };
        assert!(pr[0].contains("in the past"), "{pr:?}");
        let Checked::Valid(v) = check(
            ENTRY_REQUIREMENTS,
            &json!({"destinations": [{"place": "Bangkok", "arrive": "2026-11-09"},
                                     {"place": "Singapore", "arrive": "2026-11-13", "leave": "nope"}]}),
            &p,
            today(),
        ) else {
            panic!("valid")
        };
        assert_eq!(v["nationality"], "Australia");
        assert_eq!(
            v["destinations"][1],
            json!({"place": "Singapore", "arrive": "2026-11-13"})
        );
    }

    #[test]
    fn the_result_shows_the_visa_days_and_each_cards_official_site_and_filing_day() {
        // travel_info_real 2026-11-02: Bangkok next week, then Singapore, an Australian passport
        let args = json!({"nationality": "Australia", "destinations": [
            {"place": "Bangkok", "arrive": "2026-11-09"}, {"place": "Singapore", "arrive": "2026-11-13"}]});
        let r = result(&args, false, today());
        let shown = S::lines(&r, false);
        assert!(
            shown
                .iter()
                .any(|l| l == "- Visa-free stay in Thailand (Australia passport): up to 30 days"),
            "{shown:?}"
        );
        assert!(
            shown.iter().any(|l| l
                == "- Thailand Digital Arrival Card (TDAC): file from 2026-11-06 (the window opens 3 days before you arrive on 2026-11-09) — free, on the official site only"),
            "{shown:?}"
        );
        assert!(
            shown
                .iter()
                .any(|l| l.contains("(https://tdac.immigration.go.th/)")),
            "{shown:?}"
        );
        assert!(
            shown
                .iter()
                .any(|l| l.contains("(https://eservices.ica.gov.sg/sgarrivalcard/)")),
            "{shown:?}"
        );
        assert_eq!(r["reminders"][0]["on"], "2026-11-06");
        assert_eq!(r["reminders"][1]["on"], "2026-11-10");
        assert_eq!(r["destinations"][0]["arrival_card"]["window_days"], 3);
    }

    #[test]
    fn an_open_window_a_card_without_one_and_a_place_with_none() {
        // arriving tomorrow: the window is open now; Japan: no window; Hong Kong: no card, nothing invented
        let args = json!({"nationality": "", "destinations": [
            {"place": "Thailand", "arrive": "2026-11-03"}, {"place": "Tokyo"}, {"place": "Hong Kong"}, {"place": "London"}]});
        let r = result(&args, false, today());
        let shown = S::lines(&r, false);
        assert!(
            shown.iter().any(|l| l.starts_with(
                "- Thailand Digital Arrival Card (TDAC): the window is open now: file before you arrive on 2026-11-03"
            )),
            "{shown:?}"
        );
        assert!(
            shown.iter().any(|l| l
                == "- Visit Japan Web registration: before you arrive (no filing window) — free, on the official site only"),
            "{shown:?}"
        );
        assert!(
            shown.iter().any(|l| l
                .starts_with("- UK ETA application: apply a few days before you travel — paid")),
            "{shown:?}"
        );
        assert!(r["destinations"][2]["arrival_card"].is_null());
        assert_eq!(r["reminders"][0]["on"], "2026-11-02");
    }
}
