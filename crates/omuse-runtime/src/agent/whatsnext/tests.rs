use super::*;
use chrono::NaiveDate;

fn call(id: &str, name: &str, args: Value) -> Value {
    json!({"role": "assistant", "content": "", "tool_calls": [{"id": id, "type": "function",
           "function": {"name": name, "arguments": args.to_string()}}]})
}
fn tool(id: &str, content: &str) -> Value {
    json!({"role": "tool", "tool_call_id": id, "content": content})
}
fn day() -> NaiveDate {
    NaiveDate::from_ymd_opt(2026, 11, 4).unwrap()
}
fn profile() -> Value {
    json!({"name_en": "LUCAS LU", "email_work": "lucas@acme.example", "mailbox": "lucas@gmail.com",
           "home_country": "SG", "home_city": "Singapore"})
}
fn machine(p: &Value, scheduled: bool) -> Machine<'_> {
    Machine {
        profile: p,
        today: day(),
        scheduled,
        first: 0,
        offer: None,
    }
}
/// The calls of a step (name, args).
fn calls(s: &Step) -> Vec<(String, Value)> {
    match s {
        Step::Calls(c) => c.iter().map(|c| (c.name.clone(), c.args.clone())).collect(),
        other => panic!("not calls: {other:?}"),
    }
}
fn done(s: Step) -> Value {
    match s {
        Step::Done(v) => v,
        other => panic!("not done: {other:?}"),
    }
}
/// Run the step's calls with canned results (by tool name), appending them to the transcript.
fn answer(t: &mut Vec<Value>, s: &Step, result: impl Fn(&str, &Value) -> String) {
    let Step::Calls(cs) = s else {
        panic!("not calls")
    };
    for c in cs {
        t.push(call(&c.id, &c.name, c.args.clone()));
        t.push(tool(&c.id, &result(&c.name, &c.args)));
    }
}

fn plan() -> Value {
    json!({"location": {"city": "Tokyo", "country": "Japan"}, "items": [
        {"kind": "flight", "title": "SQ 637 Tokyo → Singapore", "date": "2026-11-04", "start": "16:30", "end": "22:50",
         "city": "Singapore", "country": "Singapore", "flight_no": "sq 637", "origin": "hnd", "destination": "SIN",
         "origin_country": "Japan", "reference": "ABC123"},
        {"kind": "meeting", "title": "Q4 review with Globex", "date": "2026-11-04", "start": "10:00", "end": "11:00",
         "place": "Roppongi Hills Mori Tower", "city": "Tokyo", "country": "Japan",
         "people": [{"name": "Aiko Tanaka", "email": "aiko@globex.example", "organization": "Globex KK"},
                    {"name": "Ben Ng", "email": "ben@acme.example"}]},
        {"kind": "hotel", "title": "Park Hyatt check-out", "date": "2026-11-04", "start": "08:00", "city": "Tokyo"},
        {"kind": "meeting", "title": "Board call", "date": "2026-11-06", "start": "09:00"},
        {"kind": "event", "title": "Too late", "date": "2026-11-09", "start": "09:00"}
    ]})
}

#[test]
fn the_items_are_cleaned_numbered_in_time_order_and_kept_to_three_days() {
    let items = items_of(&plan(), day());
    let titles: Vec<String> = items.iter().map(|i| get_str(i, "title")).collect();
    assert_eq!(
        titles,
        [
            "Park Hyatt check-out",
            "Q4 review with Globex",
            "SQ 637 Tokyo → Singapore",
            "Board call"
        ]
    );
    assert_eq!(items[2]["item"], 3);
    assert_eq!(
        (
            get_str(&items[2], "origin"),
            get_str(&items[2], "flight_no")
        ),
        ("HND".into(), "SQ 637".into())
    );
}

fn gathered_answer(name: &str, a: &Value) -> String {
    match name {
        "calendar_list_events" => json!({"trust": "untrusted", "content": {"events": [
            {"id": "e1", "title": "Q4 review", "start": "2026-11-04 10:00", "end": "2026-11-04 11:00",
             "attendees": ["aiko@globex.example"]}]}}).to_string(),
        "gmail_search" => json!({"messages": [{"id": "m1", "subject": "Your SQ booking"}, {"id": "m2", "subject": "Hotel"}]}).to_string(),
        "gmail_get_message" => json!({"subject": "Your SQ booking", "from": "sq@singaporeair.example",
            "body": format!("Booking ABC123 for {}", sget(a, "message_id"))}).to_string(),
        "subagent_json" => sub_result(a, json!({"location": {"city": "Tokyo", "country": "Japan"}, "items": plan()["items"]})),
        _ => "{}".to_string(),
    }
}

/// The subagent's result as `Runtime::subagent_json` returns it: the answer through the spec's check, else its default.
fn sub_result(a: &Value, answer: Value) -> String {
    let spec = longtask::spec(&sget(a, "kind")).unwrap();
    let (answer, defaulted) = match (spec.check)(&a["input"], &answer) {
        Ok(v) => (v, false),
        Err(_) => ((spec.fallback)(&a["input"]), true),
    };
    json!({"kind": sget(a, "kind"), "answer": answer, "defaulted": defaulted}).to_string()
}

#[test]
fn gathering_reads_every_calendar_and_two_fixed_mail_searches_then_the_items_subagent() {
    let p = profile();
    let mut t = vec![
        call("c1", WHATS_NEXT, json!({})),
        tool("c1", r#"{"status": "running", "args": {}}"#),
    ];
    let s = next(WHATS_NEXT, &json!({}), &t, &machine(&p, false));
    let c = calls(&s);
    assert_eq!(
        c.iter().map(|x| x.0.as_str()).collect::<Vec<_>>(),
        ["calendar_list_events", "gmail_search", "gmail_search"]
    );
    assert_eq!(c[0].1["time_min"], "2026-11-04 00:00");
    assert_eq!(c[0].1["time_max"], "2026-11-07 00:00");
    answer(&mut t, &s, gathered_answer);
    let s = next(WHATS_NEXT, &json!({}), &t, &machine(&p, false));
    let c = calls(&s);
    assert!(
        c.iter().all(|x| x.0 == "gmail_get_message") && c.len() == 2,
        "{c:?}"
    );
    answer(&mut t, &s, gathered_answer);
    // the items' subagent gets the events and the emails as JSON
    let s = next(WHATS_NEXT, &json!({}), &t, &machine(&p, false));
    let c = calls(&s);
    assert_eq!(
        (c[0].0.as_str(), c[0].1["kind"].as_str()),
        ("subagent_json", Some(ITEMS))
    );
    assert_eq!(c[0].1["input"]["events"][0]["title"], "Q4 review");
    assert!(get_str(&c[0].1["input"]["emails"][0], "text").contains("ABC123"));
    answer(&mut t, &s, gathered_answer);
    // asked in the chat: today's table and one question, before any research
    let r = done(next(WHATS_NEXT, &json!({}), &t, &machine(&p, false)));
    assert_eq!(r["status"], "confirm");
    let table = get_str(&r["show"][0], "value");
    assert!(
        table.contains("| 08:00 | Park Hyatt check-out |")
            && table.contains("SQ 637")
            && !table.contains("Board call"),
        "{table}"
    );
    assert_eq!(r["items"].as_array().unwrap().len(), 4);
    assert_eq!(r["location"]["city"], "Tokyo");
    // a scheduled run asks nothing: straight to the research
    let s = next(WHATS_NEXT, &json!({}), &t, &machine(&p, true));
    assert!(calls(&s).iter().any(|x| x.0 == "flights_search"), "{s:?}");
}

#[test]
fn the_items_default_is_the_calendars_events() {
    let input = json!({"home": {"city": "Singapore"}, "events": [
        {"title": "Q4 review", "start": "2026-11-04 10:00", "end": "2026-11-04 11:00", "attendees": ["aiko@globex.example"], "location": "Mori Tower"},
        {"title": "Gym", "start": "2026-11-04 18:00"}]});
    assert!(items_check(&input, &json!({"location": {}})).is_err());
    assert!(items_check(&input, &json!({"items": []})).is_ok());
    let f = items_fallback(&input);
    let items = items_of(&f, day());
    assert_eq!(items.len(), 2);
    assert_eq!(
        (get_str(&items[0], "kind"), get_str(&items[0], "start")),
        ("meeting".into(), "10:00".into())
    );
    assert_eq!(items[0]["people"][0]["email"], "aiko@globex.example");
    assert_eq!(get_str(&items[1], "kind"), "event");
}

/// Today's list was shown with its items, and the user answered.
fn answered() -> Vec<Value> {
    vec![
        tool(
            "res_0",
            &json!({"status": "confirm", "question": "Is anything missing from today?",
                    "items": items_of(&plan(), day()), "location": {"city": "Tokyo", "country": "Japan"}})
                .to_string(),
        ),
        json!({"role": "user", "content": "Nothing else, thanks"}),
    ]
}
fn valid(args: &Value) -> Value {
    match check(
        WHATS_NEXT,
        args,
        &Ctx {
            today: day(),
            transcript: &answered(),
        },
    ) {
        Checked::Valid(v) => v,
        other => panic!("{other:?}"),
    }
}
/// The reply's run: its validated call (the items carried from the question).
fn items_call(args: &Value) -> Vec<Value> {
    let v = valid(args);
    vec![
        call("c2", WHATS_NEXT, v.clone()),
        tool("c2", &json!({"status": "running", "args": v}).to_string()),
    ]
}

#[test]
fn the_reply_carries_the_items_shown_and_the_users_additions() {
    let v = valid(
        &json!({"confirmed": true, "additions": [{"kind": "reservation", "title": "Dinner at Den",
        "date": "2026-11-04", "start": "19:00", "place": "Den, Jingumae", "source": "email"}]}),
    );
    assert_eq!(v["confirmed"], true);
    let items = v["items"].as_array().unwrap();
    assert_eq!(items.len(), 5);
    let dinner = items
        .iter()
        .find(|i| i["title"] == "Dinner at Den")
        .unwrap();
    assert_eq!(dinner["source"], "user");
    // renumbered in time order
    assert_eq!(
        items
            .iter()
            .map(|i| i["item"].as_i64().unwrap())
            .collect::<Vec<_>>(),
        [1, 2, 3, 4, 5]
    );
    assert_eq!(v["location"]["city"], "Tokyo");
    // even without `confirmed`, the user answered the question: it is the reply's run
    assert_eq!(valid(&json!({}))["confirmed"], true);
}

#[test]
fn the_users_additions_go_into_the_calendar_before_the_research() {
    let p = profile();
    let args = json!({"confirmed": true, "additions": [{"kind": "reservation", "title": "Dinner at Den", "date": "2026-11-04",
        "start": "19:00", "end": "21:00", "place": "Den, Jingumae", "city": "Tokyo"}]});
    let a = valid(&args);
    let t = items_call(&args);
    let c = calls(&next(WHATS_NEXT, &a, &t, &machine(&p, false)));
    assert_eq!(c.len(), 1);
    assert_eq!(c[0].0, "calendar_create_event");
    assert_eq!(
        c[0].1,
        json!({"title": "Dinner at Den", "location": "Den, Jingumae", "start": "2026-11-04 19:00", "end": "2026-11-04 21:00"})
    );
}

#[test]
fn the_research_covers_flights_news_external_people_earlier_mail_and_a_free_evening() {
    let p = profile();
    let args = json!({"confirmed": true});
    let a = valid(&args);
    let mut t = items_call(&args);
    let s = next(WHATS_NEXT, &a, &t, &machine(&p, false));
    let c = calls(&s);
    let q = |name: &str| -> Vec<Value> {
        c.iter()
            .filter(|x| x.0 == name)
            .map(|x| x.1.clone())
            .collect()
    };
    let web: Vec<String> = q("web_search").iter().map(|x| sget(x, "query")).collect();
    assert!(
        web.contains(&"SQ 637 flight status 2026-11-04".to_string()),
        "{web:?}"
    );
    // Aiko is at another company; Ben shares the user's work domain: only Aiko is searched
    assert!(
        web.contains(&"\"Aiko Tanaka\" Globex KK".to_string())
            && !web.iter().any(|w| w.contains("Ben Ng")),
        "{web:?}"
    );
    assert_eq!(
        q("flights_search"),
        vec![json!({"origin": "HND", "destination": "SIN", "date": "2026-11-04"})]
    );
    assert_eq!(
        sget(&q("browser_news_search")[0], "query"),
        "Singapore news"
    );
    assert!(sget(&q("gmail_search")[0], "query").contains("from:aiko@globex.example"));
    // the flight lands at 22:50: the evening is not free, no events
    assert!(q("events_search").is_empty());
    answer(&mut t, &s, |name, _| {
        match name {
        "flights_search" => json!({"trust": "untrusted", "content": {"currency": "SGD", "offers": [
            {"offer_id": "o1", "airline": "Singapore Airlines", "legs": [{"flight_number": "SQ 637"}], "price": 620, "stops": 0,
             "departure": {"airport": "HND", "time": "2026-11-04 16:30"}, "arrival": {"airport": "SIN", "time": "2026-11-04 22:50"}},
            {"offer_id": "o2", "airline": "ANA", "legs": [{"flight_number": "NH 843"}], "price": 540, "stops": 0,
             "departure": {"airport": "HND", "time": "2026-11-04 17:25"}, "arrival": {"airport": "SIN", "time": "2026-11-04 23:55"}},
            {"offer_id": "o3", "airline": "JAL", "legs": [{"flight_number": "JL 37"}], "price": 500, "stops": 0,
             "departure": {"airport": "HND", "time": "2026-11-04 09:00"}, "arrival": {"airport": "SIN", "time": "2026-11-04 15:00"}}]}}).to_string(),
        "browser_news_search" => json!({"results": [{"title": "MRT line opens", "url": "https://news.example/mrt", "source": "Straits Times"}]}).to_string(),
        "web_search" => json!({"results": [{"title": "Aiko Tanaka — Globex", "url": "https://globex.example/team/aiko", "snippet": "Head of Sales"}]}).to_string(),
        "gmail_search" => json!({"messages": [{"id": "m9", "subject": "Agenda for Q4 review", "from": "aiko@globex.example", "date": "2026-10-30"}]}).to_string(),
        _ => "{}".to_string(),
    }
    });
    let r = done(next_items(&a, &t, &machine(&p, false), 1));
    assert_eq!(r["status"], "researched");
    let rows = r["today_rows"].as_array().unwrap();
    let flight = rows.iter().find(|x| x["kind"] == "flight").unwrap();
    // only the later flight that is not the booked one
    assert_eq!(
        flight["later_flights"],
        json!(["ANA NH 843 · 17:25 → 23:55 · nonstop · SGD 540"])
    );
    let facts = flight["entry"]["facts"].to_string();
    // arriving in Singapore (home): no arrival card; leaving Japan: its tax refund
    assert!(
        !facts.contains("Arrival card") && facts.contains("Tax refund on leaving"),
        "{facts}"
    );
    let meeting = rows
        .iter()
        .find(|x| x["title"] == "Q4 review with Globex")
        .unwrap();
    assert_eq!(meeting["people"][0]["name"], "Aiko Tanaka");
    assert!(meeting["entry"]["background"][0]
        .as_str()
        .unwrap()
        .contains("Agenda for Q4 review"));
    assert!(
        get_str(&meeting["entry"], "map_url").starts_with("https://www.google.com/maps/search/")
    );
    assert_eq!(r["news"][0]["url"], "https://news.example/mrt");
}

#[test]
fn arriving_abroad_gives_the_arrival_card_and_a_free_evening_gives_events() {
    let p = json!({"home_country": "AU", "email_personal": "jo@example.com"});
    let a = json!({"confirmed": true, "location": {"city": "Sydney"}, "items": items_of(&json!({"items": [
        {"kind": "flight", "title": "QF 1 Sydney → Singapore", "date": "2026-11-04", "start": "07:00", "end": "13:00",
         "city": "Singapore", "country": "Singapore", "flight_no": "QF 1", "origin": "SYD", "destination": "SIN", "origin_country": "Australia"}]}), day())});
    let mut t = vec![
        call("c2", WHATS_NEXT, a.clone()),
        tool("c2", &json!({"status": "running", "args": a}).to_string()),
    ];
    let s = next(WHATS_NEXT, &a, &t, &machine(&p, true));
    let names: Vec<String> = calls(&s).into_iter().map(|c| c.0).collect();
    assert_eq!(
        names.iter().filter(|n| *n == "events_search").count(),
        2,
        "{names:?}"
    );
    answer(&mut t, &s, |name, a| {
        match name {
        "events_search" if sget(a, "query") == "movies" => json!({"content": {"events": [{"title": "Dune: Part Three", "when": "Wed 19:30", "venue": "GV Plaza",
            "tickets": [{"source": "GV", "url": "https://www.gv.com.sg/dune"}]}]}}).to_string(),
        _ => "{}".to_string(),
    }
    });
    let r = done(next_items(&a, &t, &machine(&p, true), 1));
    let facts = r["today_rows"][0]["entry"]["facts"].to_string();
    assert!(
        facts.contains("Arrival card") && facts.contains("SG Arrival Card"),
        "{facts}"
    );
    assert_eq!(r["evening"][0]["url"], "https://www.gv.com.sg/dune");
}

/// The researched data of the plan (the flight's later flights answered).
fn researched() -> (Vec<Value>, Value) {
    let p = profile();
    let args = json!({"confirmed": true});
    let a = valid(&args);
    let mut t = items_call(&args);
    let s = next(WHATS_NEXT, &a, &t, &machine(&p, false));
    answer(&mut t, &s, |name, _| {
        match name {
        "flights_search" => json!({"content": {"currency": "SGD", "offers": [{"offer_id": "o2", "airline": "ANA", "legs": [{"flight_number": "NH 843"}], "price": 540, "stops": 0,
            "departure": {"airport": "HND", "time": "2026-11-04 17:25"}, "arrival": {"airport": "SIN", "time": "2026-11-04 23:55"}}]}}).to_string(),
        _ => "{}".to_string(),
    }
    });
    let r = done(next_items(&a, &t, &machine(&p, false), 1));
    (t, r)
}

#[test]
fn the_notes_subagents_words_are_merged_and_backups_show_only_for_a_late_flight() {
    let (t, r) = researched();
    // the notes' subagent is the next step: the research without the report's own entries
    let p = profile();
    let a = valid(&json!({"confirmed": true}));
    let c = calls(&next(WHATS_NEXT, &a, &t, &machine(&p, false)));
    assert_eq!(
        (c[0].0.as_str(), c[0].1["kind"].as_str()),
        ("subagent_json", Some(NOTES))
    );
    assert!(c[0].1["input"]["today_rows"][0].get("entry").is_none());
    assert!(c[0].1["input"].to_string().contains("NH 843"));
    let pick = |status: &str| -> Report {
        report_of(
            &json!({"summary": "A meeting, then home.", "notes": [{"item": 2, "text": "Bring the Q4 deck."}],
                "flights": [{"item": 3, "status": status, "detail": "departs 18:10"}],
                "people": [{"name": "Aiko Tanaka", "profile": "Head of Sales at Globex KK"}]}),
            &r,
        )
    };
    let late = pick("delayed");
    let flight = late.today.iter().find(|e| e.kind == "flight").unwrap();
    assert_eq!(
        flight.facts[0],
        ("Status".into(), "delayed — departs 18:10".into())
    );
    assert_eq!(flight.backups.len(), 1);
    let meeting = late
        .today
        .iter()
        .find(|e| e.title == "Q4 review with Globex")
        .unwrap();
    assert_eq!(meeting.notes, "Bring the Q4 deck.");
    assert_eq!(meeting.people[0].profile, "Head of Sales at Globex KK");
    assert_eq!(late.later[0].title, "Board call");
    assert!(late.title.starts_with("What's next — Wednesday 4 November"));
    let on_time = pick("on_time");
    assert!(on_time
        .today
        .iter()
        .find(|e| e.kind == "flight")
        .unwrap()
        .backups
        .is_empty());
    // the notes' default: a summary that counts today's items
    assert!(notes_check(&json!({}), &json!({"notes": []})).is_err());
    assert_eq!(
        notes_fallback(&notes_input(&r))["summary"],
        "3 items on your plan today."
    );
}

/// A scheduled run, end to end: gathered, the items' subagent, no question, the research, the notes' subagent, the
/// report, mailed to the user's own address — and no schedule offer.
#[test]
fn a_scheduled_run_goes_from_the_mail_to_the_mailed_report_without_a_question() {
    let p = profile();
    let mut t = vec![
        call("sch_1", WHATS_NEXT, json!({})),
        tool("sch_1", r#"{"status": "running", "args": {}}"#),
    ];
    let m = Machine {
        offer: longtask::offer(WHATS_NEXT, &json!({})),
        ..machine(&p, true)
    };
    let r = loop {
        match next(WHATS_NEXT, &json!({}), &t, &m) {
            Step::Done(v) => break v,
            s => answer(&mut t, &s, |name, a| {
                match name {
                "subagent_json" if sget(a, "kind") == NOTES => sub_result(a, json!({"summary": "Home tonight."})),
                "whats_next_report" => json!({"html_url": "/artifacts/whats-next/x.html", "html_path": "artifacts/whats-next/x.html"}).to_string(),
                "gmail_send" => r#"{"sent": true}"#.to_string(),
                other => gathered_answer(other, a),
            }
            }),
        }
    };
    let names: Vec<String> = t
        .iter()
        .flat_map(tool_calls)
        .map(|tc| super::super::util::stored_call(tc).0)
        .collect();
    assert_eq!(
        names.iter().filter(|n| *n == "subagent_json").count(),
        2,
        "{names:?}"
    );
    assert!(
        names.contains(&"flights_search".to_string())
            && names.ends_with(&["whats_next_report".into(), "gmail_send".into()])
    );
    assert_eq!(r["status"], "published");
    assert_eq!(r["mailed"], "lucas@gmail.com");
    assert!(r.get(longtask::OFFER).is_none());
}

#[test]
fn a_scheduled_run_lays_out_the_report_and_mails_it_to_the_user() {
    let p = profile();
    let report = Report {
        title: "What's next — Wednesday 4 November".into(),
        date: "2026-11-04".into(),
        summary: "Home tonight.".into(),
        today: vec![Entry {
            kind: "flight".into(),
            title: "SQ 637".into(),
            time: "16:30–22:50".into(),
            ..Default::default()
        }],
        ..Default::default()
    };
    let a = json!({"report": report, "every_day_at": null});
    let mut t = vec![];
    let s = next_publish(&a, &t, &machine(&p, true), 1);
    assert_eq!(calls(&s)[0].0, REPORT_TOOL);
    answer(&mut t, &s, |_, _| {
        json!({"html_url": "/artifacts/whats-next/2026-11-04-0700.html", "html_path": "artifacts/whats-next/2026-11-04-0700.html"}).to_string()
    });
    let s = next_publish(&a, &t, &machine(&p, true), 1);
    let c = calls(&s);
    assert_eq!(c[0].0, "gmail_send");
    assert_eq!(c[0].1["to"], "lucas@gmail.com");
    assert_eq!(
        c[0].1["attachments"],
        json!(["artifacts/whats-next/2026-11-04-0700.html"])
    );
    assert!(sget(&c[0].1, "body").contains("- 16:30–22:50 SQ 637"));
    answer(&mut t, &s, |_, _| r#"{"sent": true}"#.to_string());
    let r = done(next_publish(&a, &t, &machine(&p, true), 1));
    assert_eq!(r["mailed"], "lucas@gmail.com");
    let lines = super::super::show::lines(&r, false).join("\n");
    assert!(
        lines.contains("/artifacts/whats-next/2026-11-04-0700.html")
            && lines.contains("Emailed to"),
        "{lines}"
    );
}

#[test]
fn asked_for_every_day_it_makes_the_daily_schedule_with_the_tasks_json_and_a_chat_run_offers_one() {
    let p = profile();
    let laid = |_: &str, _: &Value| {
        json!({"html_url": "/artifacts/whats-next/x.html", "html_path": "artifacts/whats-next/x.html"}).to_string()
    };
    let a = json!({"report": Report::default(), "every_day_at": "07:30"});
    let offer = longtask::offer(WHATS_NEXT, &json!({}));
    let m = Machine {
        offer: offer.clone(),
        ..machine(&p, false)
    };
    let mut t = vec![];
    let s = next_publish(&a, &t, &m, 1);
    answer(&mut t, &s, laid);
    let s = next_publish(&a, &t, &m, 1);
    let c = calls(&s);
    assert_eq!(c[0].0, "schedule_create");
    assert_eq!(c[0].1["spec"], "30 7 * * *");
    assert_eq!(c[0].1["task"], json!({"tool": "whats_next", "args": {}}));
    answer(&mut t, &s, |_, _| {
        "已创建定时任务 schedule created: id=sch_1, next run 2026-11-05 07:30".into()
    });
    let r = done(next_publish(&a, &t, &m, 1));
    assert!(
        r.get(longtask::OFFER).is_none(),
        "a daily report asked for: none offered"
    );
    // no daily time asked: the report, then the schedule offer
    let a = json!({"report": Report::default(), "every_day_at": ""});
    let mut t = vec![];
    let s = next_publish(&a, &t, &m, 1);
    answer(&mut t, &s, laid);
    let r = done(next_publish(&a, &t, &m, 1));
    assert_eq!(r[longtask::OFFER]["every"], "every day at 07:00");
    assert!(super::super::show::lines(&r, false)
        .iter()
        .any(|l| l.contains("Run it automatically?")));
}

#[test]
fn colleagues_and_the_user_are_not_external_public_mail_is() {
    let p = profile();
    let d = own_domains(&p);
    assert_eq!(d, vec!["acme.example".to_string()]);
    assert!(!external(&json!({"email": "ben@acme.example"}), &p, &d));
    assert!(!external(&json!({"email": "lucas@gmail.com"}), &p, &d));
    assert!(external(&json!({"email": "friend@gmail.com"}), &p, &d));
    assert!(external(&json!({"email": "aiko@globex.example"}), &p, &d));
}

#[test]
fn the_daily_time_asked_for_reaches_the_publish_step_across_the_question() {
    // "every morning at 7:30": the question about today's list carries it into the reply's task
    let q = done(confirm(
        &items_of(&plan(), day()),
        &json!({"city": "Tokyo"}),
        "07:30",
        day(),
    ));
    assert_eq!(q["every_day_at"], "07:30");
    let carried = vec![
        tool("res_1", &q.to_string()),
        json!({"role": "user", "content": "nothing to add"}),
    ];
    let Checked::Valid(v) = check(
        WHATS_NEXT,
        &json!({"confirmed": true}),
        &Ctx {
            today: day(),
            transcript: &carried,
        },
    ) else {
        panic!()
    };
    assert_eq!(
        (v["every_day_at"].as_str(), v["confirmed"].as_bool()),
        (Some("07:30"), Some(true))
    );
    assert_eq!(v["items"].as_array().unwrap().len(), 4);
}

#[test]
fn an_empty_plan_and_no_additions_are_valid() {
    let params = tool_schemas()[0]["function"]["parameters"].clone();
    let mut a = json!({"confirmed": true, "additions": []});
    let c = super::super::json_in::conform(&params, &mut a);
    assert!(!c.required_bad, "{:?}", c.problems);
    let Checked::Valid(v) = check(
        WHATS_NEXT,
        &a,
        &Ctx {
            today: day(),
            transcript: &[],
        },
    ) else {
        panic!()
    };
    // nothing was asked yet: a fresh run
    assert!(v.get("items").is_none());
}

#[test]
fn confirmed_counts_only_after_the_user_answered_the_question() {
    let yes = json!({"confirmed": true});
    let cx = |t: &[Value]| -> Checked {
        check(
            WHATS_NEXT,
            &yes,
            &Ctx {
                today: day(),
                transcript: t,
            },
        )
    };
    // no list shown yet: the machine gathers and shows it first
    let Checked::Valid(v) = cx(&[]) else { panic!() };
    assert!(v.get("confirmed").is_none());
    // shown, and the model calls again without the user's answer: the user must be asked
    let confirm = tool(
        "res_1",
        &json!({"status": "confirm", "question": "Is anything missing from today?", "items": []})
            .to_string(),
    );
    assert!(matches!(
        cx(std::slice::from_ref(&confirm)),
        Checked::NeedsInput(_)
    ));
    // the user answered: confirmed
    let t = vec![
        confirm,
        json!({"role": "user", "content": "Add dinner at Den at 7pm"}),
    ];
    let Checked::Valid(v) = cx(&t) else { panic!() };
    assert_eq!(v["confirmed"], true);
}
