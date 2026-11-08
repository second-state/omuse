//! "What's next" (skill `whats-next`, the Main bot; a long task: agent/longtask.rs): today's plan from every connected
//! mailbox and calendar, researched, as one HTML report. ONE workflow tool, `whats_next` (docs/json-tools.md; the
//! protocol is [`super::workflow`]); code runs every step:
//!
//! 1. Code gathers: the calendars' events from today to the day after tomorrow and two fixed mail searches (bookings;
//!    invitations), then reads the first booking emails in full.
//! 2. A subagent ([`ITEMS`], JSON in and out) turns those emails and events into JSON items (a flight with its number,
//!    airports and countries; a meeting with its people; a hotel, a reservation, a place to be); code cleans and
//!    numbers them. Asked in the chat, code shows today's items as a table and asks once whether anything is missing
//!    (`status: confirm`) — before the research and the notes' subagent; the reply owes `whats_next` again with
//!    `confirmed: true` and the user's additions, which code puts in the default calendar. A scheduled run asks nothing.
//! 3. Code researches in one turn: each of today's flights (a web search of its status; the same route's later flights
//!    from the flight search), the local news of today's city, the external people of today's meetings (a web search
//!    each — participants outside the user's own domains), the earlier emails with them, and, when the evening is
//!    free, the city's events and films. The arrival card (on arriving in a country) and the tax refund (on leaving
//!    one) come from code's own tables; the maps are Google Maps links.
//! 4. A second subagent ([`NOTES`]) writes the words as JSON (the day's summary, a note per item, each flight's status
//!    as read from the search, each person's profile); code lays the report out (`whats_next_report`,
//!    crate::whatsnext_report), makes the daily schedule asked for with the task's JSON, mails it to the user's own
//!    address on a scheduled run, and ends with the JSON result (and, in a chat, the schedule offer).

use chrono::{Duration, NaiveDate, NaiveTime};
use omuse_common::util::{get_str, take_chars, truthy};
use serde_json::{json, Value};

use super::driver::{json_of, results_of, walk_messages, Step};
use super::longtask::{self, SubSpec};
use super::show as S;
use super::tools::fn_;
use super::util::{content_of, sget, tool_calls};
use super::workflow::{self, Checked};
use crate::whatsnext_report::{Entry, Link, Person, Report};

/// The skill whose run asks through these tools.
pub const SKILL: &str = "whats-next";
/// The id prefix of the machine's code calls.
pub const PREFIX: &str = "wnx_";
pub const WHATS_NEXT: &str = "whats_next";
/// The workflow tools this machine runs.
pub const TOOLS: [&str; 1] = [WHATS_NEXT];
/// The subagent that turns the emails and events into items.
pub const ITEMS: &str = "whats_next_items";
/// The subagent that writes the report's words.
pub const NOTES: &str = "whats_next_notes";
/// The layout tool the machine publishes with (code's own call; never offered to the model).
pub const REPORT_TOOL: &str = "whats_next_report";
/// Days covered: today and the next two (the user's decision: today in full, the next two in brief).
pub const DAYS: i64 = 3;
/// The booking emails read in full, and the invitations.
const BOOKING_READS: usize = 8;
const INVITE_READS: usize = 4;
/// At most this many people searched, meetings' mail searched.
const PEOPLE: usize = 6;
const MEETINGS: usize = 4;
/// How much of an email or a page the model gets.
const TEXT_CHARS: usize = 1500;

const KINDS: [&str; 8] = [
    "flight",
    "hotel",
    "meeting",
    "reservation",
    "event",
    "train",
    "appointment",
    "other",
];
const STATUSES: [&str; 4] = ["on_time", "delayed", "cancelled", "unknown"];
/// Mail domains anyone can have: a participant there is never "the user's own organization".
const PUBLIC_MAIL: [&str; 14] = [
    "gmail.com",
    "googlemail.com",
    "outlook.com",
    "hotmail.com",
    "live.com",
    "yahoo.com",
    "icloud.com",
    "me.com",
    "qq.com",
    "163.com",
    "126.com",
    "proton.me",
    "protonmail.com",
    "aol.com",
];

fn item_schema() -> Value {
    let s = |d: &str| json!({"type": "string", "description": d});
    let person = json!({"type": "object", "properties": {
        "name": s("the person's full name, as the invitation or email gives it"),
        "email": s("their email address"),
        "organization": s("their company or organization, when known")}});
    json!({"type": "object", "properties": {
        "kind": {"type": "string", "enum": KINDS},
        "title": s("what it is, short: \"SQ 637 Tokyo → Singapore\", \"Q4 review with Acme\", \"Park Hyatt check-in\""),
        "date": s("YYYY-MM-DD (local date where it happens)"),
        "start": s("HH:MM local; empty when all day"),
        "end": s("HH:MM local"),
        "place": s("venue or address (a meeting's room or address, a hotel's address, a restaurant)"),
        "city": s("the city it happens in (a flight: the arrival city)"),
        "country": s("the country it happens in (a flight: the arrival country)"),
        "flight_no": s("flight: e.g. SQ 637"),
        "origin": s("flight: departure airport IATA code"),
        "destination": s("flight: arrival airport IATA code"),
        "origin_country": s("flight: the country it departs from"),
        "reference": s("booking or confirmation reference"),
        "people": {"type": "array", "items": person, "description": "meeting: the participants other than the user"},
        "source": {"type": "string", "enum": ["calendar", "email", "user"], "description": "where it comes from"}}})
}

/// The catalog entry of the workflow tool.
pub fn tool_schemas() -> Vec<Value> {
    let s = |d: &str| json!({"type": "string", "description": d});
    vec![fn_(
        WHATS_NEXT,
        "What's next: today's plan (and the next two days) from every connected mailbox and calendar, researched — \
flight status and backups, arrival cards, tax refunds, local news, maps, the people the user meets, the meetings' \
background, evening events — as one HTML report. Code does all of it. Asked in the chat, the result first shows \
today's list and asks whether anything is missing (status confirm): the user's reply — whatever it is — calls \
whats_next again with confirmed: true and anything they added as additions (code puts those in the calendar).",
        json!({
            "every_day_at": s("HH:MM — only when the user asked for this report every day (a daily schedule)"),
            "confirmed": {"type": "boolean", "description": "true on the reply to the question about today's list (whether or not the user added anything)"},
            "additions": {"type": "array", "items": item_schema(), "description": "what the user added to the plan in that reply, one item each"}
        }),
        &[],
    )]
}

// ---------------------------------------------------------------- checks

/// What the checks read besides the arguments.
pub struct Ctx<'a> {
    pub today: NaiveDate,
    pub transcript: &'a [Value],
}

pub fn check(_tool: &str, args: &Value, cx: &Ctx) -> Checked {
    let at = sget(args, "every_day_at");
    let at = if NaiveTime::parse_from_str(at.trim(), "%H:%M").is_ok() {
        at.trim().to_string()
    } else {
        every_day_at_in(cx.transcript)
    };
    // the reply to the question about today's list is decided in code: the list was shown and the user wrote since
    // (whats_next_real 2026-11-05: the model answered its own question — `confirmed: true` with nothing asked)
    let (asked, answered) = question_state(cx.transcript);
    let said = args.get("confirmed").is_some_and(truthy);
    if said && asked && !answered {
        return Checked::NeedsInput(vec![
            "the user's answer to the question about today's list: is anything missing (it goes into the calendar too)?".into(),
        ]);
    }
    let mut out = json!({});
    if !at.is_empty() {
        out["every_day_at"] = json!(at);
    }
    if let (true, Some(q)) = (asked && answered, latest_confirm(cx.transcript)) {
        // the items shown with the question, and what the user added (source: user)
        let mut all: Vec<Value> = q["items"].as_array().cloned().unwrap_or_default();
        all.extend(
            args.get("additions")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .map(|i| {
                    let mut i = i.clone();
                    i["source"] = json!("user");
                    i
                }),
        );
        out["items"] = json!(items_of(&json!({"items": all}), cx.today));
        out["location"] = q.get("location").cloned().unwrap_or_else(|| json!({}));
        out["confirmed"] = json!(true);
    }
    Checked::Valid(out)
}

/// The latest `confirm` result (today's list and its question) in the transcript.
pub fn latest_confirm(transcript: &[Value]) -> Option<Value> {
    transcript
        .iter()
        .rev()
        .filter(|m| get_str(m, "role") == "tool")
        .filter_map(|m| serde_json::from_str::<Value>(&content_of(m)).ok())
        .find(|v| get_str(v, "status") == "confirm" && v.get("question").is_some())
}

fn hhmm(v: &str) -> String {
    NaiveTime::parse_from_str(v.trim(), "%H:%M")
        .map(|t| t.format("%H:%M").to_string())
        .unwrap_or_default()
}

fn iata(v: &str) -> String {
    let v = v.trim().to_uppercase();
    if v.len() == 3 && v.bytes().all(|b| b.is_ascii_uppercase()) {
        v
    } else {
        String::new()
    }
}

/// The items in the window (today … today + 2), cleaned and in time order (code numbers them 1…).
pub fn items_of(args: &Value, today: NaiveDate) -> Vec<Value> {
    let last = today + Duration::days(DAYS - 1);
    let mut out: Vec<Value> = args
        .get("items")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|i| {
            let date = NaiveDate::parse_from_str(sget(i, "date").trim(), "%Y-%m-%d").ok()?;
            if date < today || date > last || sget(i, "title").trim().is_empty() {
                return None;
            }
            let kind = sget(i, "kind").to_lowercase();
            let people: Vec<Value> = i
                .get("people")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .map(|p| json!({"name": sget(p, "name").trim(), "email": sget(p, "email").trim().to_lowercase(),
                                "organization": sget(p, "organization").trim()}))
                .filter(|p| !get_str(p, "name").is_empty() || !get_str(p, "email").is_empty())
                .take(12)
                .collect();
            let source = match sget(i, "source").as_str() {
                s @ ("calendar" | "email" | "user") => s.to_string(),
                _ => "email".to_string(),
            };
            Some(json!({
                "kind": if KINDS.contains(&kind.as_str()) { kind } else { "other".to_string() },
                "title": take_chars(sget(i, "title").trim(), 120), "date": date.format("%Y-%m-%d").to_string(),
                "start": hhmm(&sget(i, "start")), "end": hhmm(&sget(i, "end")),
                "place": take_chars(sget(i, "place").trim(), 200), "city": sget(i, "city").trim(),
                "country": sget(i, "country").trim(), "flight_no": sget(i, "flight_no").trim().to_uppercase(),
                "origin": iata(&sget(i, "origin")), "destination": iata(&sget(i, "destination")),
                "origin_country": sget(i, "origin_country").trim(), "reference": sget(i, "reference").trim(),
                "people": people, "source": source,
            }))
        })
        .take(30)
        .collect();
    out.sort_by(|a, b| {
        (get_str(a, "date"), get_str(a, "start")).cmp(&(get_str(b, "date"), get_str(b, "start")))
    });
    for (n, i) in out.iter_mut().enumerate() {
        i["item"] = json!(n + 1);
    }
    out
}

/// The notes' subagent's words (`args`) merged into the researched data `r`: the report, as `whats_next_report`
/// takes it.
fn report_of(args: &Value, r: &Value) -> Report {
    let by_item = |key: &str| -> Vec<(i64, Value)> {
        args.get(key)
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|x| {
                x.get("item")
                    .and_then(Value::as_i64)
                    .map(|n| (n, x.clone()))
            })
            .collect()
    };
    let notes = by_item("notes");
    let flights = by_item("flights");
    let profiles: Vec<Value> = args
        .get("people")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let entry = |i: &Value, full: bool| -> Entry {
        let n = i["item"].as_i64().unwrap_or(0);
        let mut e: Entry = serde_json::from_value(i["entry"].clone()).unwrap_or_default();
        if full {
            if let Some((_, x)) = notes.iter().find(|(k, _)| *k == n) {
                e.notes = take_chars(sget(x, "text").trim(), 600).to_string();
            }
            if let Some((_, f)) = flights.iter().find(|(k, _)| *k == n) {
                let status = match sget(f, "status").as_str() {
                    s if STATUSES.contains(&s) => s.replace('_', " "),
                    _ => "unknown".into(),
                };
                let detail = take_chars(sget(f, "detail").trim(), 160).to_string();
                e.facts.insert(
                    0,
                    (
                        "Status".into(),
                        if detail.is_empty() {
                            status.clone()
                        } else {
                            format!("{status} — {detail}")
                        },
                    ),
                );
                // the later flights only when this one is late or cancelled
                if !matches!(status.as_str(), "delayed" | "cancelled") {
                    e.backups.clear();
                }
            } else {
                e.backups.clear();
            }
            for p in e.people.iter_mut() {
                if let Some(x) = profiles
                    .iter()
                    .find(|x| sget(x, "name").trim().eq_ignore_ascii_case(p.name.trim()))
                {
                    p.profile = take_chars(sget(x, "profile").trim(), 240).to_string();
                }
            }
        }
        e
    };
    let rows = |k: &str| -> Vec<Value> { r[k].as_array().cloned().unwrap_or_default() };
    Report {
        title: format!("What's next — {}", get_str(r, "day")),
        date: get_str(r, "today"),
        location: get_str(r, "location"),
        summary: take_chars(sget(args, "summary").trim(), 600).to_string(),
        today: rows("today_rows").iter().map(|i| entry(i, true)).collect(),
        later: rows("later").iter().map(|i| entry(i, false)).collect(),
        news: serde_json::from_value(r["news"].clone()).unwrap_or_default(),
        news_city: get_str(r, "news_city"),
        evening: serde_json::from_value(r["evening"].clone()).unwrap_or_default(),
    }
}

// ---------------------------------------------------------------- the machine

/// What the machine reads besides the validated arguments and the transcript.
pub struct Machine<'a> {
    /// the profile, with the connected mailbox's address as `mailbox` when one is connected
    pub profile: &'a Value,
    pub today: NaiveDate,
    /// this run is a schedule's: no questions, the report goes out by email
    pub scheduled: bool,
    /// the machine's code calls before this run's tool call: the ids go on from there
    pub first: usize,
    /// the schedule offer a chat run ends with (agent/longtask.rs)
    pub offer: Option<Value>,
}

fn next_id(t: &[Value], first: usize) -> usize {
    first
        + t.iter()
            .flat_map(tool_calls)
            .filter(|tc| get_str(tc, "id").starts_with(PREFIX))
            .count()
        + 1
}

/// The machine's own calls of `tool` in this run: (args, tool message).
fn own<'a>(t: &'a [Value], tool: &str) -> Vec<(Value, &'a Value)> {
    results_of(t, tool)
}

fn failed(m: &Value) -> bool {
    content_of(m).trim_start().starts_with("ERROR")
}

/// A tool result's data: the untrusted envelope's content, or the JSON itself.
fn data_of(m: &Value) -> Value {
    let v = json_of(m);
    match v.get("content") {
        Some(c) if c.is_object() || c.is_array() => c.clone(),
        _ => v,
    }
}

/// The next step of a run of workflow tool `tool` with its validated arguments; `t` is the transcript from the call on.
/// The next step of a run of `whats_next` with its validated arguments; `t` is the transcript from the call on. Each
/// stage is a step function whose `Done` is its data for the next stage: the gathered mail and calendar, the items
/// (the question, asked in a chat), the research, the notes, the report.
pub fn next(_tool: &str, args: &Value, t: &[Value], m: &Machine) -> Step {
    let n = next_id(t, m.first);
    let every = sget(args, "every_day_at");
    // 1–2. the items: the ones shown with the question and the user's additions (the reply's run), else gathered and
    // made by the items' subagent
    let plan = if args.get("items").is_some_and(Value::is_array) {
        json!({"items": args["items"], "location": args["location"], "every_day_at": every, "confirmed": true})
    } else {
        let gathered = match next_gather(args, t, m, n) {
            Step::Done(g) => g,
            calls => return calls,
        };
        let Some(made) = longtask::sub_answer(t, ITEMS) else {
            let input = json!({"today": gathered["today"], "window": gathered["window"], "home": gathered["home"],
                               "events": gathered["events"], "emails": gathered["emails"]});
            return Step::Calls(vec![longtask::sub_call(PREFIX, n, ITEMS, input)]);
        };
        let items = items_of(&made, m.today);
        let location = json!({"city": sget(&made["location"], "city"), "country": sget(&made["location"], "country")});
        if !m.scheduled {
            return confirm(&items, &location, &every, m.today);
        }
        json!({"items": items, "location": location, "every_day_at": every})
    };
    // 3. the additions into the calendar, then the research
    let r = match next_items(&plan, t, m, n) {
        Step::Done(r) => r,
        calls => return calls,
    };
    // 4. the notes' subagent writes the words; code lays out the report and delivers it
    let Some(notes) = longtask::sub_answer(t, NOTES) else {
        return Step::Calls(vec![longtask::sub_call(PREFIX, n, NOTES, notes_input(&r))]);
    };
    let report = report_of(&notes, &r);
    next_publish(&json!({"report": report, "every_day_at": every}), t, m, n)
}

/// Asked in the chat: today's items as a table, and one question (`status: confirm`); the reply owes `whats_next`
/// again (agent/required.rs), which finds the items here (`latest_confirm`).
fn confirm(items: &[Value], location: &Value, every: &str, today: NaiveDate) -> Step {
    let today = today.format("%Y-%m-%d").to_string();
    let mut table = String::from("| Time | What | Where |\n|---|---|---|\n");
    let todays: Vec<&Value> = items
        .iter()
        .filter(|i| get_str(i, "date") == today)
        .collect();
    for i in &todays {
        table.push_str(&format!(
            "| {} | {} | {} |\n",
            md_cell(&if time_of(i).is_empty() {
                "all day".into()
            } else {
                time_of(i)
            }),
            md_cell(&get_str(i, "title")),
            md_cell(
                &[get_str(i, "place"), get_str(i, "city")]
                    .into_iter()
                    .filter(|x| !x.is_empty())
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        ));
    }
    if todays.is_empty() {
        table.push_str("| — | nothing found in your mail or calendar for today | |\n");
    }
    Step::Done(json!({
        "status": "confirm", "today": today, "items": items, "location": location, "every_day_at": every,
        "question": "Is anything missing from today? Anything you add goes into your calendar too.",
        "next_tool": WHATS_NEXT,
        "show": [S::block(&format!("**Today, {today}**\n\n{table}"))],
        "hints": ["ask the user the question in one line; their reply calls whats_next again with confirmed: true and their additions"],
    }))
}

/// The research as the notes' subagent reads it: the rows without the report's own entries.
fn notes_input(r: &Value) -> Value {
    let strip = |k: &str| -> Vec<Value> {
        r[k].as_array()
            .into_iter()
            .flatten()
            .map(|row| {
                let mut row = row.clone();
                if let Some(o) = row.as_object_mut() {
                    o.remove("entry");
                }
                row
            })
            .collect()
    };
    json!({"today": r["today"], "day": r["day"], "location": r["location"], "today_rows": strip("today_rows"),
           "later": strip("later"), "news_city": r["news_city"], "news": r["news"], "evening": r["evening"]})
}

/// The fixed mail searches: bookings (flights, hotels, tables, tickets) and invitations.
pub fn searches() -> [(&'static str, usize); 2] {
    [
        ("(flight OR \"e-ticket\" OR itinerary OR \"boarding pass\" OR hotel OR reservation OR booking OR \"check-in\" OR ticket) newer_than:45d", BOOKING_READS),
        ("(invitation OR invite OR meeting OR agenda OR call) newer_than:14d", INVITE_READS),
    ]
}

fn next_gather(args: &Value, t: &[Value], m: &Machine, n: usize) -> Step {
    let today = m.today;
    let end = today + Duration::days(DAYS);
    // 1. the calendars (all of them) and the mail searches, in one turn
    if own(t, "calendar_list_events").is_empty() && own(t, "gmail_search").is_empty() {
        let mut calls = vec![workflow::code_call(
            PREFIX,
            n,
            "calendar_list_events",
            json!({"time_min": format!("{today} 00:00"), "time_max": format!("{end} 00:00"), "max_results": 100}),
        )];
        for (i, (q, _)) in searches().iter().enumerate() {
            calls.push(workflow::code_call(
                PREFIX,
                n + 1 + i,
                "gmail_search",
                json!({"query": q, "max_results": 20}),
            ));
        }
        return Step::Calls(calls);
    }
    // 2. the first hits of each search, read in full (one turn)
    let reads = own(t, "gmail_get_message");
    if reads.is_empty() {
        let mut want: Vec<String> = Vec::new();
        for (a, msg) in own(t, "gmail_search") {
            if failed(msg) {
                continue;
            }
            let take = searches()
                .iter()
                .find(|(q, _)| *q == sget(&a, "query"))
                .map(|(_, k)| *k)
                .unwrap_or(0);
            let mut hits = Vec::new();
            walk_messages(&json_of(msg), &mut hits);
            for h in hits.iter().take(take) {
                let id = get_str(h, "id");
                if !id.is_empty() && !want.contains(&id) {
                    want.push(id);
                }
            }
        }
        if !want.is_empty() {
            return Step::Calls(
                want.iter()
                    .enumerate()
                    .map(|(i, id)| {
                        workflow::code_call(
                            PREFIX,
                            n + i,
                            "gmail_get_message",
                            json!({"message_id": id}),
                        )
                    })
                    .collect(),
            );
        }
    }
    // 3. the data for the model to make the items from
    let events: Vec<Value> = own(t, "calendar_list_events")
        .iter()
        .filter(|(_, msg)| !failed(msg))
        .flat_map(|(_, msg)| data_of(msg)["events"].as_array().cloned().unwrap_or_default())
        .map(|e| {
            json!({"title": e["title"], "start": e["start"], "end": e["end"], "location": e["location"],
                   "attendees": e["attendees"], "organizer": e["organizer"], "account": e["account"],
                   "description": take_chars(&get_str(&e, "description"), 600)})
        })
        .collect();
    let emails: Vec<Value> = own(t, "gmail_get_message")
        .iter()
        .filter(|(_, msg)| !failed(msg))
        .map(|(a, msg)| {
            let v = data_of(msg);
            let text = [get_str(&v, "body"), get_str(&v, "text"), get_str(&v, "snippet")]
                .into_iter()
                .find(|x| !x.is_empty())
                .unwrap_or_default();
            json!({"message_id": sget(a, "message_id"), "from": v["from"], "subject": v["subject"], "date": v["date"],
                   "text": take_chars(&text, TEXT_CHARS)})
        })
        .collect();
    let calendar_error = own(t, "calendar_list_events")
        .iter()
        .find(|(_, msg)| failed(msg))
        .map(|(_, msg)| take_chars(content_of(msg).trim(), 200).to_string());
    let mut out = json!({
        "status": "gathered", "today": today.format("%Y-%m-%d").to_string(),
        "window": [today.format("%Y-%m-%d").to_string(), (end - Duration::days(1)).format("%Y-%m-%d").to_string()],
        "home": {"city": get_str(m.profile, "home_city"), "country": get_str(m.profile, "home_country")},
        "events": events, "emails": emails,
    });
    if let Some(e) = calendar_error {
        out["calendar"] = json!(format!("not read: {e}"));
    }
    if let Some(at) = args.get("every_day_at").filter(|v| v.is_string()) {
        out["every_day_at"] = at.clone();
    }
    Step::Done(out)
}

/// The user's own mail domains (from the profile's addresses and the mailbox), public mail services left out.
pub fn own_domains(profile: &Value) -> Vec<String> {
    ["email_personal", "email_work", "mailbox", "email"]
        .iter()
        .map(|k| get_str(profile, k).to_lowercase())
        .filter_map(|e| e.split_once('@').map(|(_, d)| d.trim().to_string()))
        .filter(|d| !d.is_empty() && !PUBLIC_MAIL.contains(&d.as_str()))
        .collect()
}

/// Whether a participant is external: not the user, and not at the user's own organization's domain.
pub fn external(p: &Value, profile: &Value, domains: &[String]) -> bool {
    let email = get_str(p, "email").to_lowercase();
    let mine: Vec<String> = ["email_personal", "email_work", "mailbox", "email"]
        .iter()
        .map(|k| get_str(profile, k).to_lowercase())
        .filter(|e| !e.is_empty())
        .collect();
    if !email.is_empty() && mine.contains(&email) {
        return false;
    }
    match email.split_once('@') {
        Some((_, d)) => !domains.iter().any(|o| o == d),
        None => true,
    }
}

/// A country of the tables by name, ISO code or city ("Japan", "JP", "Tokyo").
fn country_name(v: &str) -> String {
    let v = v.trim();
    if let Some(c) = crate::places::country(v) {
        return c.name.to_string();
    }
    if let Some(c) = crate::places::city_named(v).and_then(|c| crate::places::country(c.country)) {
        return c.name.to_string();
    }
    v.to_string()
}

/// The facts and links of a `show` list (the arrival card's, the tax refund's).
fn facts_links(show: &Value, facts: &mut Vec<(String, String)>, links: &mut Vec<Link>) {
    for s in show.as_array().into_iter().flatten() {
        match get_str(s, "kind").as_str() {
            "fact" => facts.push((get_str(s, "label"), get_str(s, "value"))),
            "link" => links.push(Link {
                text: get_str(s, "label"),
                url: get_str(s, "url"),
                note: get_str(s, "text"),
            }),
            _ => {}
        }
    }
}

/// The arrival card on arriving in a country other than home, and the tax refund on leaving one.
fn border_facts(i: &Value, home: &str, facts: &mut Vec<(String, String)>, links: &mut Vec<Link>) {
    if get_str(i, "kind") != "flight" {
        return;
    }
    let to = country_name(&get_str(i, "country"));
    if !to.is_empty() && !to.eq_ignore_ascii_case(home) {
        if let Some(card) = super::driver::card_of(&to) {
            let mut f = Vec::new();
            facts_links(
                &super::driver::card_info(card, &get_str(i, "date"))["show"],
                &mut f,
                links,
            );
            facts.extend(
                f.into_iter()
                    .map(|(k, v)| (format!("Arrival card — {k}"), v)),
            );
        }
    }
    let from = country_name(&get_str(i, "origin_country"));
    if !from.is_empty() && !from.eq_ignore_ascii_case(home) {
        let r = super::tax_refund::tax_refund_rules(&json!({"countries": [from]}));
        if let Some(c) = r["countries"].as_array().and_then(|a| a.first()) {
            if c["running"] == json!(true) {
                facts.push((
                    "Tax refund on leaving".into(),
                    format!("{} — {}", get_str(c, "scheme"), get_str(c, "how")),
                ));
                for l in c["links"].as_array().into_iter().flatten().take(1) {
                    links.push(Link {
                        text: get_str(l, "label"),
                        url: get_str(l, "url"),
                        note: String::new(),
                    });
                }
            }
        }
    }
}

fn map_url(i: &Value) -> String {
    let place = get_str(i, "place");
    if place.is_empty() || get_str(i, "kind") == "flight" {
        return String::new();
    }
    omuse_apps::serpapi::place_map_url(&place, &get_str(i, "city"), "")
}

/// Today's evening is free: no item starts or ends after 18:00.
fn evening_free(today: &[Value]) -> bool {
    !today.iter().any(|i| {
        let (s, e) = (get_str(i, "start"), get_str(i, "end"));
        s.as_str() >= "18:00" || e.as_str() > "18:00"
    })
}

fn time_of(i: &Value) -> String {
    match (get_str(i, "start"), get_str(i, "end")) {
        (s, _) if s.is_empty() => String::new(),
        (s, e) if e.is_empty() => s,
        (s, e) => format!("{s}–{e}"),
    }
}

fn md_cell(s: &str) -> String {
    s.replace('|', "/").replace('\n', " ")
}

fn next_items(args: &Value, t: &[Value], m: &Machine, n: usize) -> Step {
    let today = m.today.format("%Y-%m-%d").to_string();
    let items: Vec<Value> = args["items"].as_array().cloned().unwrap_or_default();
    let todays: Vec<Value> = items
        .iter()
        .filter(|i| get_str(i, "date") == today)
        .cloned()
        .collect();
    // 2. the user's additions go into the default calendar
    let added: Vec<&Value> = items
        .iter()
        .filter(|i| get_str(i, "source") == "user")
        .collect();
    if !added.is_empty() && own(t, "calendar_create_event").is_empty() {
        return Step::Calls(
            added
                .iter()
                .enumerate()
                .map(|(j, i)| {
                    let start = get_str(i, "start");
                    let mut ev =
                        json!({"title": get_str(i, "title"), "location": get_str(i, "place")});
                    if start.is_empty() {
                        ev["start"] = json!(get_str(i, "date"));
                        ev["all_day"] = json!(true);
                    } else {
                        ev["start"] = json!(format!("{} {start}", get_str(i, "date")));
                        if !get_str(i, "end").is_empty() {
                            ev["end"] =
                                json!(format!("{} {}", get_str(i, "date"), get_str(i, "end")));
                        }
                    }
                    workflow::code_call(PREFIX, n + j, "calendar_create_event", ev)
                })
                .collect(),
        );
    }
    // 3. the research, in one turn
    let loc_city = sget(&args["location"], "city");
    let news_city = todays
        .iter()
        .rev()
        .find(|i| get_str(i, "kind") == "flight" && !get_str(i, "city").is_empty())
        .map(|i| get_str(i, "city"))
        .unwrap_or_else(|| loc_city.clone());
    let domains = own_domains(m.profile);
    let people: Vec<(i64, Value)> = todays
        .iter()
        .filter(|i| get_str(i, "kind") == "meeting")
        .flat_map(|i| {
            let k = i["item"].as_i64().unwrap_or(0);
            i["people"]
                .as_array()
                .cloned()
                .unwrap_or_default()
                .into_iter()
                .map(move |p| (k, p))
        })
        .filter(|(_, p)| !get_str(p, "name").is_empty() && external(p, m.profile, &domains))
        .take(PEOPLE)
        .collect();
    // (the gathering's own mail searches, in a scheduled run's transcript, are not research)
    let researching = !own(t, "web_search").is_empty()
        || !own(t, "browser_news_search").is_empty()
        || !own(t, "events_search").is_empty()
        || !own(t, "flights_search").is_empty()
        || research_mail(t).next().is_some();
    if !researching {
        let mut calls = Vec::new();
        let mut call = |name: &str, a: Value| {
            let id = n + calls.len();
            calls.push(workflow::code_call(PREFIX, id, name, a));
        };
        for i in todays.iter().filter(|i| get_str(i, "kind") == "flight") {
            let f = get_str(i, "flight_no");
            if !f.is_empty() {
                call(
                    "web_search",
                    json!({"query": format!("{f} flight status {today}"), "max_results": 5}),
                );
            }
            let (o, d) = (get_str(i, "origin"), get_str(i, "destination"));
            if !o.is_empty() && !d.is_empty() && o != d {
                call(
                    "flights_search",
                    json!({"origin": o, "destination": d, "date": today}),
                );
            }
        }
        if !news_city.is_empty() {
            call(
                "browser_news_search",
                json!({"query": format!("{news_city} news"), "days": 1, "max_results": 8}),
            );
        }
        for (_, p) in &people {
            let org = get_str(p, "organization");
            let q = if org.is_empty() {
                format!("\"{}\"", get_str(p, "name"))
            } else {
                format!("\"{}\" {org}", get_str(p, "name"))
            };
            call("web_search", json!({"query": q, "max_results": 3}));
        }
        for i in todays
            .iter()
            .filter(|i| get_str(i, "kind") == "meeting")
            .take(MEETINGS)
        {
            let who: Vec<String> = i["people"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|p| get_str(p, "email"))
                .filter(|e| e.contains('@'))
                .take(3)
                .flat_map(|e| [format!("from:{e}"), format!("to:{e}")])
                .collect();
            if !who.is_empty() {
                call(
                    "gmail_search",
                    json!({"query": format!("({}) newer_than:90d", who.join(" OR ")), "max_results": 5}),
                );
            }
        }
        if !news_city.is_empty() && evening_free(&todays) {
            call("events_search", json!({"city": news_city, "when": "today"}));
            call(
                "events_search",
                json!({"city": news_city, "query": "movies", "when": "today"}),
            );
        }
        if !calls.is_empty() {
            return Step::Calls(calls);
        }
    }
    // 4. the researched data: what the report is laid out from, and what the model writes its notes from
    let home = country_name(&get_str(m.profile, "home_country"));
    let web = own(t, "web_search");
    let search_rows = |msg: &Value| -> Vec<Value> {
        data_of(msg)["results"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .take(5)
            .map(|r| json!({"title": r["title"], "url": r["url"], "snippet": take_chars(&get_str(&r, "snippet"), 300)}))
            .collect()
    };
    let mut today_rows = Vec::new();
    for i in &todays {
        let k = i["item"].as_i64().unwrap_or(0);
        let mut e = Entry {
            kind: get_str(i, "kind"),
            title: get_str(i, "title"),
            date: get_str(i, "date"),
            time: time_of(i),
            place: [get_str(i, "place"), get_str(i, "city")]
                .into_iter()
                .filter(|x| !x.is_empty())
                .collect::<Vec<_>>()
                .join(", "),
            map_url: map_url(i),
            added: get_str(i, "source") == "user",
            ..Default::default()
        };
        if !get_str(i, "reference").is_empty() {
            e.facts.push(("Reference".into(), get_str(i, "reference")));
        }
        border_facts(i, &home, &mut e.facts, &mut e.links);
        let mut row = json!({"item": k, "kind": get_str(i, "kind"), "title": get_str(i, "title"), "time": time_of(i)});
        if get_str(i, "kind") == "flight" {
            let f = get_str(i, "flight_no");
            let status: Vec<Value> = web
                .iter()
                .filter(|(a, _)| !f.is_empty() && sget(a, "query").starts_with(&f))
                .flat_map(|(_, msg)| search_rows(msg))
                .collect();
            row["status_search"] = json!(status);
            let booked = format!("{} {}", get_str(i, "date"), get_str(i, "start"));
            let mut later: Vec<super::booking::Offer> = own(t, "flights_search")
                .iter()
                .filter(|(a, _)| {
                    sget(a, "origin") == get_str(i, "origin")
                        && sget(a, "destination") == get_str(i, "destination")
                })
                .flat_map(|(_, msg)| {
                    super::booking::offers_in(super::booking::Kind::Flight, &data_of(msg))
                })
                .filter(|o| o.depart.as_str() > booked.as_str() && !o.label.contains(&f))
                .collect();
            later.sort_by(|a, b| a.depart.cmp(&b.depart));
            e.backups = later
                .iter()
                .take(3)
                .map(|o| {
                    let stops = match o.stops {
                        Some(0) => "nonstop".to_string(),
                        Some(s) => format!("{s} stop{}", if s == 1 { "" } else { "s" }),
                        None => String::new(),
                    };
                    let price = o
                        .price
                        .map(|p| format!("{} {p:.0}", o.currency))
                        .unwrap_or_default();
                    [
                        o.label.clone(),
                        format!(
                            "{} → {}",
                            o.depart.get(11..).unwrap_or(&o.depart),
                            o.arrive.get(11..).unwrap_or(&o.arrive)
                        ),
                        stops,
                        price,
                    ]
                    .into_iter()
                    .filter(|x| !x.trim().is_empty())
                    .collect::<Vec<_>>()
                    .join(" · ")
                })
                .collect();
            row["later_flights"] = json!(e.backups);
        }
        if get_str(i, "kind") == "meeting" {
            for (_, p) in people.iter().filter(|(pk, _)| *pk == k) {
                let name = get_str(p, "name");
                let found: Vec<Value> = web
                    .iter()
                    .filter(|(a, _)| sget(a, "query").starts_with(&format!("\"{name}\"")))
                    .flat_map(|(_, msg)| search_rows(msg))
                    .collect();
                e.people.push(Person {
                    name: name.clone(),
                    organization: get_str(p, "organization"),
                    profile: String::new(),
                    links: found
                        .iter()
                        .take(2)
                        .map(|r| Link {
                            text: get_str(r, "title"),
                            url: get_str(r, "url"),
                            note: String::new(),
                        })
                        .collect(),
                });
                row["people"] = json!(row["people"].as_array().cloned().unwrap_or_default().into_iter()
                    .chain([json!({"name": name, "organization": get_str(p, "organization"), "search": found})]).collect::<Vec<_>>());
            }
            let emails: Vec<String> = i["people"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|p| get_str(p, "email"))
                .filter(|e| !e.is_empty())
                .collect();
            let mut mails = Vec::new();
            for (a, msg) in own(t, "gmail_search") {
                if !emails
                    .iter()
                    .any(|e| sget(&a, "query").contains(e.as_str()))
                {
                    continue;
                }
                let mut hits = Vec::new();
                walk_messages(&json_of(msg), &mut hits);
                for h in hits.iter().take(5) {
                    mails.push(format!(
                        "{} — {}, {}",
                        get_str(h, "subject"),
                        get_str(h, "from"),
                        get_str(h, "date")
                    ));
                    row["earlier_emails"] = json!(row["earlier_emails"].as_array().cloned().unwrap_or_default().into_iter()
                        .chain([json!({"subject": h["subject"], "from": h["from"], "date": h["date"], "snippet": take_chars(&get_str(h, "snippet"), 300)})]).collect::<Vec<_>>());
                }
            }
            e.background = mails;
        }
        row["entry"] = serde_json::to_value(&e).unwrap_or_default();
        today_rows.push(row);
    }
    let later_rows: Vec<Value> = items
        .iter()
        .filter(|i| get_str(i, "date") != today)
        .map(|i| {
            let mut e = Entry {
                kind: get_str(i, "kind"),
                title: get_str(i, "title"),
                date: get_str(i, "date"),
                time: time_of(i),
                place: [get_str(i, "place"), get_str(i, "city")].into_iter().filter(|x| !x.is_empty()).collect::<Vec<_>>().join(", "),
                map_url: map_url(i),
                added: get_str(i, "source") == "user",
                ..Default::default()
            };
            border_facts(i, &home, &mut e.facts, &mut e.links);
            json!({"item": i["item"], "title": get_str(i, "title"), "date": get_str(i, "date"), "entry": e})
        })
        .collect();
    let news: Vec<Link> = own(t, "browser_news_search")
        .iter()
        .flat_map(|(_, msg)| {
            let d = data_of(msg);
            d["results"]
                .as_array()
                .or_else(|| d["articles"].as_array())
                .cloned()
                .unwrap_or_default()
        })
        .filter(|r| get_str(r, "url").starts_with("https://"))
        .take(6)
        .map(|r| Link {
            text: get_str(&r, "title"),
            url: get_str(&r, "url"),
            note: get_str(&r, "source"),
        })
        .collect();
    let evening: Vec<Link> = own(t, "events_search")
        .iter()
        .flat_map(|(_, msg)| {
            data_of(msg)["events"]
                .as_array()
                .cloned()
                .unwrap_or_default()
        })
        .take(8)
        .map(|e| {
            let url = e["tickets"]
                .as_array()
                .and_then(|t| t.first())
                .map(|t| get_str(t, "url"))
                .filter(|u| !u.is_empty())
                .unwrap_or_else(|| get_str(&e, "link"));
            Link {
                text: get_str(&e, "title"),
                url,
                note: [get_str(&e, "when"), get_str(&e, "venue")]
                    .into_iter()
                    .filter(|x| !x.is_empty())
                    .collect::<Vec<_>>()
                    .join(" · "),
            }
        })
        .collect();
    let added_ok = own(t, "calendar_create_event")
        .iter()
        .filter(|(_, m)| !failed(m))
        .count();
    let location = [loc_city, sget(&args["location"], "country")]
        .into_iter()
        .filter(|x| !x.is_empty())
        .collect::<Vec<_>>()
        .join(", ");
    Step::Done(json!({
        "status": "researched", "machine": SKILL, "today": today, "day": m.today.format("%A %-d %B").to_string(),
        "location": location, "today_items": today_rows.len(), "today_rows": today_rows,
        "later": later_rows, "news_city": news_city, "news": news, "evening": evening,
        "evening_free": evening_free(&todays), "added_to_calendar": added_ok,
        "every_day_at": sget(args, "every_day_at"),
    }))
}

/// The research's mail searches (the earlier emails with a meeting's people): not the gathering's fixed ones.
fn research_mail(t: &[Value]) -> impl Iterator<Item = (Value, &Value)> {
    own(t, "gmail_search")
        .into_iter()
        .filter(|(a, _)| !searches().iter().any(|(q, _)| *q == sget(a, "query")))
}

/// Whether today's list was shown with its question (a `confirm` result), and whether the user wrote after the latest
/// one.
pub fn question_state(transcript: &[Value]) -> (bool, bool) {
    let at = transcript.iter().rposition(|m| {
        get_str(m, "role") == "tool"
            && serde_json::from_str::<Value>(&content_of(m))
                .is_ok_and(|v| get_str(&v, "status") == "confirm" && v.get("question").is_some())
    });
    match at {
        Some(i) => (
            true,
            transcript[i + 1..]
                .iter()
                .any(|m| get_str(m, "role") == "user"),
        ),
        None => (false, false),
    }
}

/// The daily time the user asked for: the latest result that carries it (the gathered mail and calendar, or the
/// question about today's list — the one carried into the reply's task), "" when none.
pub fn every_day_at_in(transcript: &[Value]) -> String {
    transcript
        .iter()
        .rev()
        .filter(|m| get_str(m, "role") == "tool")
        .filter_map(|m| serde_json::from_str::<Value>(&content_of(m)).ok())
        .find_map(|v| {
            v.get("every_day_at")
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .unwrap_or_default()
}

fn next_publish(args: &Value, t: &[Value], m: &Machine, n: usize) -> Step {
    let report: Report = serde_json::from_value(args["report"].clone()).unwrap_or_default();
    // 1. laid out by code
    let reports = own(t, REPORT_TOOL);
    let Some((_, rm)) = reports.last() else {
        return Step::Calls(vec![workflow::code_call(
            PREFIX,
            n,
            REPORT_TOOL,
            json!({"report": report}),
        )]);
    };
    if failed(rm) {
        let why = take_chars(content_of(rm).trim_start_matches("ERROR:").trim(), 200).to_string();
        return Step::Done(json!({"status": "error", "why": why,
            "show": [S::fact("Report not made", "报告未生成", &why)], "hints": []}));
    }
    let rep = json_of(rm);
    let (html_url, html_path) = (get_str(&rep, "html_url"), get_str(&rep, "html_path"));
    // 2. the daily schedule the user asked for (never from a schedule's own run)
    let at = args
        .get("every_day_at")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let schedules = own(t, "schedule_create");
    if !m.scheduled && !at.is_empty() && schedules.is_empty() {
        let (h, mi) = at.split_once(':').unwrap_or(("7", "0"));
        return Step::Calls(vec![workflow::code_call(
            PREFIX,
            n,
            "schedule_create",
            json!({"name": "What's next", "kind": "cron",
                   "spec": format!("{} {} * * *", mi.trim_start_matches('0').parse::<u32>().unwrap_or(0), h.trim_start_matches('0').parse::<u32>().unwrap_or(0)),
                   "goal": "What's next", "task": longtask::task_call(WHATS_NEXT, &json!({}))}),
        )]);
    }
    // 3. a scheduled run: the report goes to the user's own address
    let mailbox = get_str(m.profile, "mailbox");
    let mails = own(t, "gmail_send");
    if m.scheduled && !mailbox.is_empty() && mails.is_empty() {
        let lines: Vec<String> = report
            .today
            .iter()
            .map(|e| {
                format!(
                    "- {} {}",
                    if e.time.is_empty() {
                        "all day"
                    } else {
                        &e.time
                    },
                    e.title
                )
            })
            .collect();
        let body = format!(
            "{}\n\nToday:\n{}\n\nThe full report is attached (open the HTML file).",
            report.summary,
            if lines.is_empty() {
                "- nothing found in your mail or calendar".to_string()
            } else {
                lines.join("\n")
            }
        );
        return Step::Calls(vec![workflow::code_call(
            PREFIX,
            n,
            "gmail_send",
            json!({"to": mailbox, "subject": report.title, "body": body, "attachments": [html_path]}),
        )]);
    }
    // 4. the result
    let mut show = vec![S::link("Report", "报告", &report.title, &html_url)];
    let mut out = json!({"status": "published", "title": report.title, "html_url": html_url,
        "today": report.today.iter().map(|e| json!({"time": e.time, "title": e.title})).collect::<Vec<_>>()});
    if let Some((_, sm)) = schedules.last() {
        if failed(sm) {
            show.push(S::fact(
                "Daily report not scheduled",
                "每日报告未设定",
                take_chars(content_of(sm).trim(), 200),
            ));
        } else {
            show.push(S::fact(
                "Every day",
                "每天",
                &format!("at {at}, emailed to you when a mailbox is connected"),
            ));
            out["schedule"] = json!({"every_day_at": at});
        }
    }
    match mails.last() {
        Some((a, mm)) if !failed(mm) => {
            show.push(S::fact("Emailed to", "已发送到", &sget(a, "to")));
            out["mailed"] = json!(sget(a, "to"));
        }
        Some((_, mm)) => {
            show.push(S::fact(
                "Not emailed",
                "邮件未发送",
                take_chars(content_of(mm).trim(), 200),
            ));
        }
        None if m.scheduled && mailbox.is_empty() => {
            show.push(S::fact(
                "Delivered",
                "送达",
                "here in this chat (connect a mailbox to get it by email)",
            ));
        }
        None => {}
    }
    // the schedule offer: a chat run that asked for no daily report (agent/longtask.rs)
    if let Some(o) = m.offer.as_ref().filter(|_| at.is_empty() && !m.scheduled) {
        show.push(longtask::offer_item(o));
        out[longtask::OFFER] = o.clone();
        out["next_question"] = o["question"].clone();
    }
    out["show"] = json!(show);
    out["hints"] = json!([]);
    Step::Done(out)
}

/// Whether the model is offered tool `name` on its next turn: `whats_next_report` is code's own call; on the reply
/// to the machine's result `whats_next` is withdrawn.
pub fn offered(transcript: &[Value], name: &str) -> bool {
    if name == REPORT_TOOL {
        return false;
    }
    !TOOLS.contains(&name) || !workflow::reply_turn(transcript)
}

// ---------------------------------------------------------------- the subagents

fn items_output() -> Value {
    json!({"type": "object", "properties": {
        "location": {"type": "object", "properties": {"city": {"type": "string", "description": "where the user is today (the city of the first item, or home)"},
                                                      "country": {"type": "string"}}},
        "items": {"type": "array", "items": item_schema()}},
        "required": ["items"]})
}

fn items_check(_input: &Value, out: &Value) -> Result<Value, Vec<String>> {
    match out.get("items") {
        Some(Value::Array(_)) => Ok(out.clone()),
        _ => Err(vec![
            "items: a JSON array of the plan's items (empty when there is none)".into(),
        ]),
    }
}

/// The default items: the calendar's events as they are (each a meeting when it has attendees, else an event).
fn items_fallback(input: &Value) -> Value {
    let items: Vec<Value> = input["events"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|e| {
            let start = get_str(e, "start");
            let date = start.get(..10)?.to_string();
            let time = |v: &str| v.get(11..16).unwrap_or("").to_string();
            let people: Vec<Value> = e["attendees"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|a| match a {
                    Value::String(x) => json!({"email": x}),
                    other => json!({"name": get_str(other, "name"), "email": get_str(other, "email")}),
                })
                .collect();
            Some(json!({"kind": if people.is_empty() { "event" } else { "meeting" }, "title": get_str(e, "title"),
                        "date": date, "start": time(&start), "end": time(&get_str(e, "end")),
                        "place": get_str(e, "location"), "people": people, "source": "calendar"}))
        })
        .collect();
    json!({"location": input["home"], "items": items})
}

pub fn items_spec() -> SubSpec {
    SubSpec {
        kind: ITEMS,
        instruction: "You make a user's plan from their emails and calendar events (the input). Give every item from \
`today` to the end of `window`, one each: flights (number, IATA airports, the arrival city and country, the departure \
country), hotel check-ins and check-outs, meetings (with every participant other than the user: name, email, \
organization), reservations, events, places to be. Times are local where it happens. A later email about the same \
booking wins; leave out cancelled ones. Never invent an item, a time or a person. location: where the user is today.",
        output: items_output,
        tools: &[],
        check: items_check,
        fallback: items_fallback,
    }
}

fn notes_output() -> Value {
    let s = |d: &str| json!({"type": "string", "description": d});
    json!({"type": "object", "properties": {
        "summary": s("two or three sentences on the day"),
        "notes": {"type": "array", "items": {"type": "object", "properties": {"item": {"type": "integer"}, "text": s("at most 60 words")}}},
        "flights": {"type": "array", "items": {"type": "object", "properties": {"item": {"type": "integer"},
            "status": {"type": "string", "enum": STATUSES}, "detail": s("e.g. \"departs 19:40, 50 min late\"")}}},
        "people": {"type": "array", "items": {"type": "object", "properties": {"name": s("as the input lists it"), "profile": s("one line: role, company, what they are known for")}}}},
        "required": ["summary"]})
}

fn notes_check(_input: &Value, out: &Value) -> Result<Value, Vec<String>> {
    if sget(out, "summary").trim().is_empty() {
        return Err(vec!["summary: two or three sentences on the day".into()]);
    }
    Ok(out.clone())
}

/// The default words: a summary that counts today's items, no notes.
fn notes_fallback(input: &Value) -> Value {
    let n = input["today_rows"].as_array().map_or(0, Vec::len);
    json!({"summary": match n {
        0 => "Nothing on your plan for today.".to_string(),
        1 => "One item on your plan today.".to_string(),
        n => format!("{n} items on your plan today."),
    }, "notes": [], "flights": [], "people": []})
}

pub fn notes_spec() -> SubSpec {
    SubSpec {
        kind: NOTES,
        instruction: "You write the words of a user's day report from the researched plan in the input: a summary of \
the day (two or three sentences), a short note per item by its number (what to know or do — from the emails, the \
earlier emails with the participants, the searches), each of today's flights' status as its status search shows it \
(`unknown` when it does not say), and a one-line public profile of each person from their search results (role and \
company only; nothing private, nothing guessed).",
        output: notes_output,
        tools: &[],
        check: notes_check,
        fallback: notes_fallback,
    }
}

#[cfg(test)]
mod tests;
