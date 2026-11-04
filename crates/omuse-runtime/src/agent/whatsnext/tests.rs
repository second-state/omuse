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

#[test]
fn gathering_reads_every_calendar_and_two_fixed_mail_searches_then_the_first_hits() {
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
    answer(&mut t, &s, |name, _| {
        match name {
        "calendar_list_events" => json!({"trust": "untrusted", "content": {"events": [
            {"id": "e1", "title": "Q4 review", "start": "2026-11-04 10:00", "attendees": ["aiko@globex.example"]}]}}).to_string(),
        _ => json!({"messages": [{"id": "m1", "subject": "Your SQ booking"}, {"id": "m2", "subject": "Hotel"}]}).to_string(),
    }
    });
    let s = next(WHATS_NEXT, &json!({}), &t, &machine(&p, false));
    let c = calls(&s);
    assert!(
        c.iter().all(|x| x.0 == "gmail_get_message") && c.len() == 2,
        "{c:?}"
    );
    answer(&mut t, &s, |_, a| {
        json!({"subject": "Your SQ booking", "from": "sq@singaporeair.example",
        "body": format!("Booking ABC123 for {}", sget(a, "message_id"))})
        .to_string()
    });
    let r = done(next(WHATS_NEXT, &json!({}), &t, &machine(&p, false)));
    assert_eq!(r["status"], "gathered");
    assert_eq!(r["events"][0]["title"], "Q4 review");
    assert!(get_str(&r["emails"][0], "text").contains("ABC123"));
    assert_eq!(r["next_tool"], WHATS_NEXT_ITEMS);
}

/// Today's list was shown and the user answered (what `confirmed: true` needs).
fn answered() -> Vec<Value> {
    vec![
        tool(
            "res_0",
            &json!({"status": "confirm", "question": "Is anything missing from today?"})
                .to_string(),
        ),
        json!({"role": "user", "content": "Nothing else, thanks"}),
    ]
}
fn items_call(args: &Value) -> Vec<Value> {
    let a = check(
        WHATS_NEXT_ITEMS,
        args,
        &Ctx {
            today: day(),
            transcript: &answered(),
        },
    );
    let Checked::Valid(v) = a else {
        panic!("{a:?}")
    };
    vec![
        call("c2", WHATS_NEXT_ITEMS, v.clone()),
        tool("c2", &json!({"status": "running", "args": v}).to_string()),
    ]
}
fn valid(args: &Value) -> Value {
    match check(
        WHATS_NEXT_ITEMS,
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

#[test]
fn asked_in_the_chat_it_shows_todays_table_and_asks_once_a_schedule_asks_nothing() {
    let p = profile();
    let a = valid(&plan());
    let t = items_call(&plan());
    let r = done(next(WHATS_NEXT_ITEMS, &a, &t, &machine(&p, false)));
    assert_eq!(r["status"], "confirm");
    let table = get_str(&r["show"][0], "value");
    assert!(
        table.contains("| 08:00 | Park Hyatt check-out |")
            && table.contains("SQ 637")
            && !table.contains("Board call"),
        "{table}"
    );
    // a scheduled run goes straight to the research
    let s = next(WHATS_NEXT_ITEMS, &a, &t, &machine(&p, true));
    assert!(matches!(s, Step::Calls(_)));
}

#[test]
fn the_users_additions_go_into_the_calendar_before_the_research() {
    let p = profile();
    let mut args = plan();
    args["confirmed"] = json!(true);
    args["items"].as_array_mut().unwrap().push(json!({"kind": "reservation", "title": "Dinner at Den", "date": "2026-11-04",
        "start": "19:00", "end": "21:00", "place": "Den, Jingumae", "city": "Tokyo", "source": "user"}));
    let a = valid(&args);
    let t = items_call(&args);
    let c = calls(&next(WHATS_NEXT_ITEMS, &a, &t, &machine(&p, false)));
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
    let mut args = plan();
    args["confirmed"] = json!(true);
    let a = valid(&args);
    let mut t = items_call(&args);
    let s = next(WHATS_NEXT_ITEMS, &a, &t, &machine(&p, false));
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
    let r = done(next(WHATS_NEXT_ITEMS, &a, &t, &machine(&p, false)));
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
    let args = json!({"confirmed": true, "location": {"city": "Sydney"}, "items": [
        {"kind": "flight", "title": "QF 1 Sydney → Singapore", "date": "2026-11-04", "start": "07:00", "end": "13:00",
         "city": "Singapore", "country": "Singapore", "flight_no": "QF 1", "origin": "SYD", "destination": "SIN", "origin_country": "Australia"}]});
    let a = valid(&args);
    let mut t = items_call(&args);
    let s = next(WHATS_NEXT_ITEMS, &a, &t, &machine(&p, true));
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
    let r = done(next(WHATS_NEXT_ITEMS, &a, &t, &machine(&p, true)));
    let facts = r["today_rows"][0]["entry"]["facts"].to_string();
    assert!(
        facts.contains("Arrival card") && facts.contains("SG Arrival Card"),
        "{facts}"
    );
    assert_eq!(r["evening"][0]["url"], "https://www.gv.com.sg/dune");
}

fn researched_transcript() -> Vec<Value> {
    let p = profile();
    let mut args = plan();
    args["confirmed"] = json!(true);
    let a = valid(&args);
    let mut t = items_call(&args);
    let s = next(WHATS_NEXT_ITEMS, &a, &t, &machine(&p, false));
    answer(&mut t, &s, |name, _| {
        match name {
        "flights_search" => json!({"content": {"currency": "SGD", "offers": [{"offer_id": "o2", "airline": "ANA", "legs": [{"flight_number": "NH 843"}], "price": 540, "stops": 0,
            "departure": {"airport": "HND", "time": "2026-11-04 17:25"}, "arrival": {"airport": "SIN", "time": "2026-11-04 23:55"}}]}}).to_string(),
        _ => "{}".to_string(),
    }
    });
    let r = done(next(WHATS_NEXT_ITEMS, &a, &t, &machine(&p, false)));
    t.extend(workflow::result_exchange(&t, WHATS_NEXT_ITEMS, &r));
    t
}

#[test]
fn the_models_notes_are_merged_and_backups_show_only_for_a_late_flight() {
    let t = researched_transcript();
    let pick = |status: &str| -> Report {
        let a = json!({"summary": "A meeting, then home.", "notes": [{"item": 2, "text": "Bring the Q4 deck."}],
            "flights": [{"item": 3, "status": status, "detail": "departs 18:10"}],
            "people": [{"name": "Aiko Tanaka", "profile": "Head of Sales at Globex KK"}]});
        match check(
            WHATS_NEXT_PUBLISH,
            &a,
            &Ctx {
                today: day(),
                transcript: &t,
            },
        ) {
            Checked::Valid(v) => serde_json::from_value(v["report"].clone()).unwrap(),
            other => panic!("{other:?}"),
        }
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
    // nothing researched: the model is told to research first
    assert!(matches!(
        check(
            WHATS_NEXT_PUBLISH,
            &json!({"summary": "x"}),
            &Ctx {
                today: day(),
                transcript: &[]
            }
        ),
        Checked::Invalid(_)
    ));
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
    let mut t = vec![
        call("c3", WHATS_NEXT_PUBLISH, a.clone()),
        tool("c3", &json!({"status": "running", "args": a}).to_string()),
    ];
    let s = next(WHATS_NEXT_PUBLISH, &a, &t, &machine(&p, true));
    assert_eq!(calls(&s)[0].0, REPORT_TOOL);
    answer(&mut t, &s, |_, _| {
        json!({"html_url": "/artifacts/whats-next/2026-11-04-0700.html", "html_path": "artifacts/whats-next/2026-11-04-0700.html"}).to_string()
    });
    let s = next(WHATS_NEXT_PUBLISH, &a, &t, &machine(&p, true));
    let c = calls(&s);
    assert_eq!(c[0].0, "gmail_send");
    assert_eq!(c[0].1["to"], "lucas@gmail.com");
    assert_eq!(
        c[0].1["attachments"],
        json!(["artifacts/whats-next/2026-11-04-0700.html"])
    );
    assert!(sget(&c[0].1, "body").contains("- 16:30–22:50 SQ 637"));
    answer(&mut t, &s, |_, _| r#"{"sent": true}"#.to_string());
    let r = done(next(WHATS_NEXT_PUBLISH, &a, &t, &machine(&p, true)));
    assert_eq!(r["mailed"], "lucas@gmail.com");
    let lines = super::super::show::lines(&r, false).join("\n");
    assert!(
        lines.contains("/artifacts/whats-next/2026-11-04-0700.html")
            && lines.contains("Emailed to"),
        "{lines}"
    );
}

#[test]
fn asked_for_every_day_it_makes_the_daily_schedule_once() {
    let p = profile();
    let a = json!({"report": Report::default(), "every_day_at": "07:30"});
    let mut t = vec![
        call("c3", WHATS_NEXT_PUBLISH, a.clone()),
        tool("c3", &json!({"status": "running", "args": a}).to_string()),
    ];
    let s = next(WHATS_NEXT_PUBLISH, &a, &t, &machine(&p, false));
    answer(&mut t, &s, |_, _| {
        json!({"html_url": "/artifacts/whats-next/x.html", "html_path": "artifacts/whats-next/x.html"}).to_string()
    });
    let c = calls(&next(WHATS_NEXT_PUBLISH, &a, &t, &machine(&p, false)));
    assert_eq!(c[0].0, "schedule_create");
    assert_eq!(c[0].1["spec"], "30 7 * * *");
    assert!(sget(&c[0].1, "goal").contains("whats_next"));
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
    // "every morning at 7:30": whats_next's gathered result carries it; the question about today's list carries it
    // into the reply's task; the items call takes it from there
    let gathered = vec![tool(
        "c1",
        r#"{"status": "gathered", "every_day_at": "07:30"}"#,
    )];
    let Checked::Valid(v) = check(
        WHATS_NEXT_ITEMS,
        &plan(),
        &Ctx {
            today: day(),
            transcript: &gathered,
        },
    ) else {
        panic!()
    };
    assert_eq!(v["every_day_at"], "07:30");
    let p = profile();
    let t = vec![
        call("c2", WHATS_NEXT_ITEMS, v.clone()),
        tool("c2", &json!({"status": "running", "args": v}).to_string()),
    ];
    let r = done(next(WHATS_NEXT_ITEMS, &v, &t, &machine(&p, false)));
    assert_eq!(
        (r["status"].clone(), r["every_day_at"].clone()),
        (json!("confirm"), json!("07:30"))
    );
    // the reply's task sees only the carried question: the time is still found
    let carried = vec![tool("res_1", &r.to_string())];
    let Checked::Valid(v2) = check(
        WHATS_NEXT_ITEMS,
        &plan(),
        &Ctx {
            today: day(),
            transcript: &carried,
        },
    ) else {
        panic!()
    };
    assert_eq!(v2["every_day_at"], "07:30");
}

#[test]
fn an_empty_plan_is_a_valid_answer() {
    let params = tool_schemas()
        .into_iter()
        .find(|s| s["function"]["name"] == WHATS_NEXT_ITEMS)
        .unwrap()["function"]["parameters"]
        .clone();
    let mut a = json!({"location": {"city": "Singapore"}, "items": [], "confirmed": false});
    let c = super::super::json_in::conform(&params, &mut a);
    assert!(!c.required_bad, "{:?}", c.problems);
    let Checked::Valid(v) = check(
        WHATS_NEXT_ITEMS,
        &a,
        &Ctx {
            today: day(),
            transcript: &[],
        },
    ) else {
        panic!()
    };
    assert_eq!(v["items"], json!([]));
}

#[test]
fn confirmed_counts_only_after_the_user_answered_the_question() {
    let mut yes = plan();
    yes["confirmed"] = json!(true);
    let cx = |t: &[Value]| -> Checked {
        check(
            WHATS_NEXT_ITEMS,
            &yes,
            &Ctx {
                today: day(),
                transcript: t,
            },
        )
    };
    // no list shown yet: the machine shows it first
    let Checked::Valid(v) = cx(&[]) else { panic!() };
    assert_eq!(v["confirmed"], false);
    // shown, and the model calls again without the user's answer: the user must be asked
    let confirm = tool(
        "res_1",
        &json!({"status": "confirm", "question": "Is anything missing from today?"}).to_string(),
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
