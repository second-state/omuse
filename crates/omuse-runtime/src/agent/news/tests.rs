use super::*;

const NEWS: [(&str, u32, &str, &str, &str); 6] = [
    ("lumora-orkney-array", 1, "Reuters", "Lumora switches on 40 MW tidal array off Orkney",
     "Lumora Energy switched on its first commercial tidal array, 40 megawatts across 16 turbines in the Pentland Firth, on Monday. The company said the array will power about 30,000 homes and that output in the first 48 hours was 12% above forecast."),
    ("lumora-eu-grant", 2, "Financial Times", "EU awards Lumora €120m to build second tidal site in Brittany",
     "The European Commission awarded Lumora a €120 million Innovation Fund grant for a 60 MW array near Paimpol in Brittany. Construction is due to start in March 2027."),
    ("lumora-kestrel-deal", 3, "Bloomberg", "Turbine maker Kestrel signs 200-unit supply deal with Lumora",
     "Kestrel Marine will supply 200 of its K4 tidal turbines to Lumora over five years, the largest tidal turbine order to date."),
    ("lumora-grid-link", 4, "BBC News", "Regulator approves Lumora's Orkney grid link",
     "Ofgem approved a £310 million subsea cable linking Orkney's tidal arrays to the mainland grid, ending years of curtailment."),
    ("lumora-fishermen", 5, "The Guardian", "Orkney fishermen raise concerns over Lumora turbines",
     "The Orkney Fisheries Association said the new array closes a crab ground used by eleven boats and asked for compensation."),
    ("lumora-shares", 6, "CNBC", "Lumora shares jump 14% after first output figures",
     "Lumora's shares rose 14% in London after it published its first output figures."),
];

fn url(sl: &str) -> String {
    format!("https://news.example/news/{sl}")
}

fn day(ago: u32) -> String {
    format!("{} October 2026", 32 - ago)
}

/// The same day as an engine's timestamp ("2026-10-31T09:00:00.0000000").
fn stamp(ago: u32) -> String {
    format!("2026-10-{:02}T09:00:00.0000000", 32 - ago)
}

fn today() -> NaiveDate {
    NaiveDate::from_ymd_opt(2026, 11, 1).unwrap()
}

/// A tool result as the run loop stores it: the sentinel's untrusted envelope inside the `<untrusted_content>` tag.
fn enveloped(source: &str, body: Value) -> String {
    let mut env = json!({"trust": "untrusted", "source": source});
    for (k, v) in body.as_object().unwrap() {
        env[k] = v.clone();
    }
    format!("<untrusted_content source=\"{source}\">\n{env}\n</untrusted_content>")
}

fn search_result(q: &str) -> String {
    let rows: Vec<Value> = NEWS
        .iter()
        .map(|(sl, ago, src, title, body)| {
            // DuckDuckGo's row: the timestamp as its own field, no publication (it comes from the article's page)
            let _ = src;
            json!({"title": title, "url": url(sl), "snippet": format!("{}…", take_chars(body, 140)), "date": stamp(*ago)})
        })
        .collect();
    enveloped(
        &format!("web search (duckduckgo): {q}"),
        json!({"results": rows, "next": "browser_read the 2–4 most relevant URLs in one call."}),
    )
}

fn read_result(urls: &[String]) -> String {
    let pages: Vec<Value> = urls
        .iter()
        .map(|u| {
            let (_, ago, src, title, body) = NEWS.iter().find(|n| url(n.0) == *u).unwrap();
            let text = format!("{title}\n{src} · {}\n{body}", day(*ago));
            json!({"url": u, "title": format!("{title} | {src}"), "text": text, "chars": text.len(),
                   "lead": body, "site_name": src, "published": stamp(*ago)})
        })
        .collect();
    enveloped(
        &format!("web pages {}", urls.join(", ")),
        json!({"pages": pages}),
    )
}

fn published(subject: &str) -> String {
    let s = crate::newsreport::slug(subject);
    json!({"published": true, "title": "The Tidal Current", "stories": 6,
           "html_url": format!("/artifacts/news/{s}/2026-11-01-0800.html"), "html_path": format!("artifacts/news/{s}/2026-11-01-0800.html"),
           "pdf_url": format!("/artifacts/news/{s}/2026-11-01-0800.pdf"), "pdf_path": format!("artifacts/news/{s}/2026-11-01-0800.pdf")})
    .to_string()
}

fn tools(name: &str, args: &Value) -> String {
    match name {
        "browser_news_search" => search_result(&sget(args, "query")),
        "browser_read" => {
            let urls: Vec<String> = args["urls"]
                .as_array()
                .unwrap()
                .iter()
                .map(|u| u.as_str().unwrap().to_string())
                .collect();
            read_result(&urls)
        }
        "news_report" => published(&sget(args, "subject")),
        "schedule_create" => {
            "已创建定时任务 schedule created: id=sch_1, next run 2026-11-02 08:00".into()
        }
        "gmail_send" => json!({"sent": true, "id": "m1"}).to_string(),
        // the edition's subagent: one story for the first article (code adds the rest)
        "subagent_json" => sub_result(
            args,
            json!({"title": "The Tidal Current", "stories": [
                {"article": 1, "headline": "Orkney array goes live", "summary": "Lumora switched on a 40 MW array of 16 turbines."}],
                "insights": ["Tidal power is leaving the pilot stage."]}),
        ),
        "market_quotes" => quotes_of(&json!({"results": [
            {"symbol": "BTC-USD", "name": "Bitcoin USD", "exchange": "CCC", "currency": "USD", "range": "5d",
             "last": 68123.45, "last_time": "2026-11-01 07:58", "day_change_pct": 1.2, "series": []}],
            "errors": []}))
        .to_string(),
        other => panic!("unexpected call {other}"),
    }
}

/// The subagent's result as `Runtime::subagent_json` returns it: the answer through the spec's check, else its default.
fn sub_result(args: &Value, answer: Value) -> String {
    assert_eq!(sget(args, "kind"), EDITION);
    let input = &args["input"];
    let (answer, defaulted) = match edition_check(input, &answer) {
        Ok(v) => (v, false),
        Err(_) => (edition_fallback(input), true),
    };
    json!({"kind": EDITION, "answer": answer, "retried": false, "defaulted": defaulted}).to_string()
}

fn valid(c: Checked) -> Value {
    match c {
        Checked::Valid(v) => v,
        other => panic!("not valid: {other:?}"),
    }
}

/// The transcript the run loop leaves for a valid call of `tool`: the request, the call, its `running` result.
fn started(request: &str, tool: &str, args: &Value) -> Vec<Value> {
    vec![
        json!({"role": "user", "content": request}),
        json!({"role": "assistant", "content": "", "tool_calls": [{"id": "c1", "type": "function",
               "function": {"name": tool, "arguments": args.to_string()}}]}),
        json!({"role": "tool", "tool_call_id": "c1", "content": Checked::Valid(args.clone()).to_result().to_string()}),
    ]
}

/// Runs the machine from a started transcript to its Done JSON: each code call answered by `answer`. Returns the
/// transcript (with the calls) and the result; asserts the ids go on and every step is calls or Done.
fn run(
    tool: &str,
    args: &Value,
    mut t: Vec<Value>,
    m: &Machine,
    answer: impl Fn(&str, &Value) -> String,
) -> (Vec<Value>, Value) {
    for _ in 0..12 {
        match next(tool, args, &t[1..], m) {
            Step::Calls(c) => {
                assert!(!c.is_empty());
                for call in &c {
                    assert!(call.id.starts_with(PREFIX), "{}", call.id);
                }
                // one turn: every call of the step, then their results
                t.push(json!({"role": "assistant", "content": "", "tool_calls": c.iter().map(|call| json!({"id": call.id, "type": "function",
                    "function": {"name": call.name, "arguments": call.args.to_string()}})).collect::<Vec<_>>()}));
                for call in &c {
                    t.push(json!({"role": "tool", "tool_call_id": call.id, "content": answer(&call.name, &call.args)}));
                }
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

fn machine(profile: &Value, scheduled: bool) -> Machine<'_> {
    Machine {
        profile,
        today: today(),
        scheduled,
        zh: false,
        first: 0,
        offer: None,
    }
}

const REQUEST: &str = "Research the latest news on Lumora tidal power and make me a newspaper-style report. Send me a new edition every week.";

fn gargs(a: Value) -> Value {
    let p = json!({});
    valid(check(NEWS_GATHER, &a, &Ctx { profile: &p }))
}

/// One whole `news_gather` run with the edition's subagent answering `edition`.
fn run_with(args: &Value, m: &Machine, edition: Value) -> (Vec<Value>, Value) {
    run(
        NEWS_GATHER,
        args,
        started(REQUEST, NEWS_GATHER, args),
        m,
        |n, a| match n {
            "subagent_json" => sub_result(a, edition.clone()),
            _ => tools(n, a),
        },
    )
}

#[test]
fn gather_needs_a_subject_and_normalises_the_whole_request() {
    let p = json!({});
    let cx = Ctx { profile: &p };
    assert_eq!(
        check(NEWS_GATHER, &json!({"window": "week"}), &cx),
        Checked::NeedsInput(vec!["subject".into()])
    );
    assert_eq!(
        valid(check(
            NEWS_GATHER,
            &json!({"subject": " \"Lumora\" ", "window": "Daily", "schedule": "every day", "hour": 30}),
            &cx
        )),
        json!({"subject": "Lumora", "window": "day", "schedule": "once", "weekday": 1, "hour": 8, "email": false})
    );
    let a = valid(check(
        NEWS_GATHER,
        &json!({"subject": "AI", "schedule": "weekly", "weekday": 3, "email": true}),
        &cx,
    ));
    assert_eq!(
        (
            a["window"].as_str(),
            a["schedule"].as_str(),
            a["weekday"].as_i64(),
            a["email"].as_bool()
        ),
        (Some("week"), Some("weekly"), Some(3), Some(true))
    );
    assert_eq!(period(7, today()), "26 October 2026 – 1 November 2026");
    assert_eq!(period(1, today()), "1 November 2026");
}

/// news_report_real (g31, r32): the model ran six searches of its own and called the articles "unverified". Code makes
/// ONE search from the call's subject, reads the six articles in one turn, gives them to the edition's subagent as
/// JSON (without their links) and publishes — one call of the model's.
#[test]
fn one_call_runs_one_search_one_read_turn_the_subagent_and_publishes() {
    let p = json!({});
    let args = gargs(json!({"subject": "Lumora tidal power", "window": "week"}));
    let (t, res) = run(
        NEWS_GATHER,
        &args,
        started(REQUEST, NEWS_GATHER, &args),
        &machine(&p, false),
        tools,
    );
    assert_eq!(
        calls_of(&t, "browser_news_search"),
        [json!({"query": "Lumora tidal power", "days": 7, "max_results": 10})]
    );
    let reads = calls_of(&t, "browser_read");
    assert_eq!(reads.len(), 2);
    assert_eq!(reads[0]["urls"].as_array().unwrap().len(), 4);
    // both reads in the same turn
    let turn = t
        .iter()
        .find(|m| tool_calls(m).len() == 2)
        .expect("one turn with both reads");
    assert!(tool_calls(turn)
        .iter()
        .all(|tc| stored_call(tc).0 == "browser_read"));
    // the subagent's input: the articles as JSON, numbered, with their publication and date — no links
    let sub = &calls_of(&t, "subagent_json")[0];
    assert_eq!(sub["kind"], EDITION);
    let arts = sub["input"]["articles"].as_array().unwrap();
    assert_eq!(arts.len(), 6);
    assert_eq!(
        (arts[0]["article"].as_i64(), arts[0]["publication"].as_str()),
        (Some(1), Some("Reuters"))
    );
    assert_eq!(arts[0]["date"], day(1));
    assert!(get_str(&arts[3], "text").contains("Ofgem"));
    assert!(!sub.to_string().contains("news.example"));
    assert_eq!(sub["input"]["period"], "26 October 2026 – 1 November 2026");
    assert_eq!(res["status"], "published");
    let names: Vec<String> = t
        .iter()
        .flat_map(tool_calls)
        .map(|tc| stored_call(tc).0)
        .collect();
    assert_eq!(
        names,
        [
            "news_gather",
            "browser_news_search",
            "browser_read",
            "browser_read",
            "subagent_json",
            "news_report"
        ]
    );
}

#[test]
fn no_news_ends_with_nothing_found_after_one_wider_search() {
    let p = json!({});
    let args = gargs(json!({"subject": "Lumora", "window": "day"}));
    let empty = |name: &str, args: &Value| match name {
        "browser_news_search" => enveloped(
            &format!("web search (none): {}", sget(args, "query")),
            json!({"results": [], "note": "the browser's news search found nothing"}),
        ),
        other => panic!("unexpected call {other}"),
    };
    let (t, res) = run(
        NEWS_GATHER,
        &args,
        started("x", NEWS_GATHER, &args),
        &machine(&p, false),
        empty,
    );
    let s = calls_of(&t, "browser_news_search");
    assert_eq!(s.len(), 2);
    assert_eq!(s[0]["days"], 1);
    assert_eq!(
        (sget(&s[1], "query").as_str(), s[1]["days"].as_i64()),
        ("Lumora news", Some(30))
    );
    assert_eq!(res["status"], "nothing_found");
    assert_eq!(
        super::super::show::lines(&res, false),
        vec!["- No edition published: the news search found no recent articles on Lumora"]
    );
}

/// Links, publications and dates come from the gathered articles, never from the subagent: a story naming an
/// article that does not exist is dropped, an article named twice is kept once, an article left out is added with
/// its own headline and opening.
#[test]
fn the_edition_keeps_only_gathered_articles_and_takes_their_links() {
    let p = json!({});
    let args = gargs(json!({"subject": "Lumora tidal power", "window": "week"}));
    let (t, _) = run_with(
        &args,
        &machine(&p, false),
        json!({"title": "The Tidal Current", "stories": [
        {"article": 2, "headline": "Brussels backs Brittany site", "summary": "The EU awarded Lumora €120 million for a 60 MW array near Paimpol."},
        {"article": 1, "headline": "Orkney array goes live", "summary": "Lumora switched on a 40 MW array of 16 turbines.", "url": "https://evil.example/x"},
        {"article": 1, "headline": "again", "summary": "dup"},
        {"article": 9, "headline": "no such article", "summary": "x"}],
        "insights": ["Tidal power is leaving the pilot stage.", ""]}),
    );
    let rep = &calls_of(&t, "news_report")[0];
    let stories = rep["stories"].as_array().unwrap();
    assert_eq!(stories.len(), 6, "{rep}");
    assert_eq!(stories[0]["headline"], "Brussels backs Brittany site");
    assert_eq!(stories[0]["url"], url("lumora-eu-grant"));
    assert_eq!(stories[0]["source"], "Financial Times");
    assert_eq!(stories[0]["date"], day(2));
    assert_eq!(stories[1]["url"], url("lumora-orkney-array"));
    assert!(!rep.to_string().contains("evil.example"));
    // the omitted articles: their own headline and opening
    assert_eq!(stories[2]["headline"], NEWS[2].3);
    assert!(get_str(&stories[2], "summary").starts_with("Kestrel Marine will supply 200"));
    assert!(stories
        .iter()
        .any(|s| get_str(s, "summary").contains("Ofgem")));
    assert_eq!(
        rep["insights"],
        json!(["Tidal power is leaving the pilot stage."])
    );
    assert_eq!(rep["period"], "26 October 2026 – 1 November 2026");
}

/// A subagent that names no article (twice): code's default edition, every article with its own headline and opening.
#[test]
fn an_edition_that_names_no_article_is_codes_default() {
    let input =
        json!({"articles": [{"article": 1, "title": "T", "lead": "The lead.", "text": "x"}]});
    assert!(edition_check(
        &input,
        &json!({"stories": [{"article": 9, "summary": "s"}]})
    )
    .is_err());
    assert!(edition_check(&input, &json!("prose")).is_err());
    assert_eq!(
        edition_fallback(&input)["stories"],
        json!([{"article": 1, "headline": "T", "summary": "The lead."}])
    );
    let p = json!({});
    let args = gargs(json!({"subject": "Lumora tidal power"}));
    let (t, res) = run_with(
        &args,
        &machine(&p, false),
        json!({"stories": [{"article": 9, "summary": "x"}]}),
    );
    assert_eq!(res["status"], "published");
    let rep = &calls_of(&t, "news_report")[0];
    assert_eq!(rep["stories"].as_array().unwrap().len(), 6);
    assert_eq!(rep["stories"][0]["headline"], NEWS[0].3);
}

#[test]
fn every_week_lays_out_schedules_the_tasks_json_and_mails_the_edition() {
    let p = json!({"mailbox": "me@example.com"});
    let args = gargs(json!({"subject": "Lumora tidal power", "schedule": "weekly", "email": true}));
    let m = Machine {
        offer: longtask::offer(NEWS_GATHER, &args),
        ..machine(&p, false)
    };
    let (t, res) = run(
        NEWS_GATHER,
        &args,
        started(REQUEST, NEWS_GATHER, &args),
        &m,
        tools,
    );
    let rep = &calls_of(&t, "news_report")[0];
    assert_eq!(rep["stories"].as_array().unwrap().len(), 6);
    assert_eq!(rep["subject"], "Lumora tidal power");
    let sch = &calls_of(&t, "schedule_create")[0];
    assert_eq!(sch["spec"], "0 8 * * 1");
    assert_eq!(sch["name"], "News report: Lumora tidal power");
    // the schedule stores the task's JSON: its run calls news_gather with it, no model in between
    assert_eq!(
        sch["task"],
        json!({"tool": "news_gather", "args": {"subject": "Lumora tidal power", "window": "week", "email": true}})
    );
    let mail = &calls_of(&t, "gmail_send")[0];
    assert_eq!(mail["to"], "me@example.com");
    assert_eq!(
        mail["attachments"],
        json!([
            "artifacts/news/lumora-tidal-power/2026-11-01-0800.pdf",
            "artifacts/news/lumora-tidal-power/2026-11-01-0800.html"
        ])
    );
    assert_eq!(res["status"], "published");
    // the lines code appends to the reply (gate 40 news_report_real: the editions' links left out of the prose)
    let title = res["title"].as_str().unwrap().to_string();
    assert_eq!(
        super::super::show::lines(&res, false),
        vec![
            format!("- HTML edition: [{title}](/artifacts/news/lumora-tidal-power/2026-11-01-0800.html)"),
            format!("- PDF edition: [{title}](/artifacts/news/lumora-tidal-power/2026-11-01-0800.pdf)"),
            "- New edition: every Monday at 08:00 — next: 2026-11-02 08:00".to_string(),
            "- Emailed to: me@example.com (PDF attached)".to_string(),
        ]
    );
    // a schedule asked for: none offered
    assert!(res.get(longtask::OFFER).is_none() && res.get("next_question").is_none());
}

#[test]
fn a_scheduled_run_makes_no_schedule_and_a_chat_run_offers_one() {
    let p = json!({"mailbox": "me@example.com"});
    // the schedule's run (the stored task's arguments): mailed, no schedule
    let stored = gargs(json!({"subject": "Lumora tidal power", "window": "week", "email": true}));
    let (t, res) = run(
        NEWS_GATHER,
        &stored,
        started("⏰", NEWS_GATHER, &stored),
        &machine(&p, true),
        tools,
    );
    assert!(calls_of(&t, "schedule_create").is_empty());
    assert_eq!(calls_of(&t, "gmail_send").len(), 1);
    assert!(res["schedule"].is_null() && res.get(longtask::OFFER).is_none());
    // a one-off edition in a chat: no schedule, no mail, the schedule offered (agent/longtask.rs)
    let once = gargs(json!({"subject": "Lumora tidal power"}));
    let m = Machine {
        offer: longtask::offer(NEWS_GATHER, &once),
        ..machine(&p, false)
    };
    let (t, res) = run(
        NEWS_GATHER,
        &once,
        started("x", NEWS_GATHER, &once),
        &m,
        tools,
    );
    assert!(calls_of(&t, "schedule_create").is_empty() && calls_of(&t, "gmail_send").is_empty());
    assert_eq!(res[longtask::OFFER]["every"], "every Monday at 08:00");
    assert_eq!(
        res[longtask::OFFER]["task"],
        json!({"tool": "news_gather", "args": {"subject": "Lumora tidal power", "window": "week"}})
    );
    assert!(sget(&res, "next_question").contains("every Monday at 08:00"));
    // offered already (or scheduled): the email question instead
    let (_, res) = run(
        NEWS_GATHER,
        &once,
        started("x", NEWS_GATHER, &once),
        &machine(&p, false),
        tools,
    );
    assert!(sget(&res, "next_question").contains("me@example.com"));
    // the mail asked for with no mailbox connected: delivered in the chat, nothing sent
    let none = json!({});
    let (t, res) = run(
        NEWS_GATHER,
        &stored,
        started("x", NEWS_GATHER, &stored),
        &machine(&none, true),
        tools,
    );
    assert!(calls_of(&t, "gmail_send").is_empty());
    assert!(super::super::show::lines(&res, false)
        .iter()
        .any(|m| m.starts_with("- Delivered: here in this chat")));
    assert_eq!(res["delivery"], json!({"chat": true, "email": null}));
}

#[test]
fn a_failed_layout_ends_with_the_error() {
    let p = json!({});
    let args = gargs(json!({"subject": "Lumora tidal power"}));
    let (_, res) = run(
        NEWS_GATHER,
        &args,
        started("x", NEWS_GATHER, &args),
        &machine(&p, false),
        |n, a| match n {
            "news_report" => "ERROR: the template is missing".into(),
            _ => tools(n, a),
        },
    );
    assert_eq!(res["status"], "error");
    assert!(super::super::show::lines(&res, false)[0].contains("the template is missing"));
}

#[test]
fn the_published_edition_is_the_newspaper_with_every_link() {
    let p = json!({});
    let args = gargs(json!({"subject": "Lumora tidal power"}));
    let (t, _) = run_with(
        &args,
        &machine(&p, false),
        json!({"stories": [
        {"article": 3, "headline": "Kestrel order", "summary": "Kestrel Marine will supply 200 K4 turbines."}]}),
    );
    let r = crate::newsreport::Report::from_args(&calls_of(&t, "news_report")[0]).unwrap();
    let html = crate::newsreport::render(
        include_str!("../../../../../skills/news-research/template.html"),
        &r,
        chrono::Local::now(),
    );
    assert!(
        html.contains("class=\"masthead\"")
            && html.contains("class=\"story\"")
            && html.contains("Sources")
    );
    assert_eq!(html.matches("Read the article").count(), 6);
    for f in [
        "40 megawatts",
        "€120 million",
        "Kestrel",
        "Ofgem",
        "Brittany",
    ] {
        assert!(html.contains(f), "{f}");
    }
}

#[test]
fn email_it_mails_the_edition_the_model_names_to_the_users_own_address() {
    let p = json!({"mailbox": "me@example.com"});
    let cx = Ctx { profile: &p };
    let args = valid(check(
        NEWS_MAIL,
        &json!({"files": ["http://localhost:8080/artifacts/news/lumora/2026-11-01-0800.html",
                          "/artifacts/news/lumora/2026-11-01-0800.pdf", "https://evil.example/x.pdf"],
                "title": "The Tidal Current"}),
        &cx,
    ));
    assert_eq!(
        args["files"],
        json!([
            "artifacts/news/lumora/2026-11-01-0800.pdf",
            "artifacts/news/lumora/2026-11-01-0800.html"
        ])
    );
    let (t, res) = run(
        NEWS_MAIL,
        &args,
        started("email it", NEWS_MAIL, &args),
        &machine(&p, false),
        tools,
    );
    let mail = &calls_of(&t, "gmail_send")[0];
    assert_eq!(
        (mail["to"].as_str(), mail["subject"].as_str()),
        (Some("me@example.com"), Some("The Tidal Current"))
    );
    assert_eq!(res["status"], "mailed");
    assert_eq!(
        super::super::show::lines(&res, false),
        vec!["- Emailed to: me@example.com (PDF attached)"]
    );
    // a link that is no edition: invalid; no mailbox: the user connects one
    assert!(matches!(
        check(NEWS_MAIL, &json!({"files": ["/etc/passwd"]}), &cx),
        Checked::Invalid(_)
    ));
    assert!(matches!(
        check(
            NEWS_MAIL,
            &json!({"files": ["/artifacts/news/a/b.pdf"]}),
            &Ctx {
                profile: &json!({})
            }
        ),
        Checked::NeedsInput(_)
    ));
    assert_eq!(edition_path("/artifacts/news/../../x.pdf"), None);
}

#[test]
fn the_model_never_gets_the_layout_tool_and_no_news_tool_on_the_reply() {
    let mut t = vec![json!({"role": "user", "content": REQUEST})];
    assert!(!offered(&t, REPORT_TOOL) && !offered(&t, MARKET_TOOL));
    assert!(offered(&t, NEWS_GATHER) && offered(&t, "web_search"));
    t.extend(workflow::result_exchange(
        &t,
        NEWS_GATHER,
        &json!({"status": "published"}),
    ));
    assert!(TOOLS.iter().all(|n| !offered(&t, n)));
}

#[test]
fn ask_user_before_news_gather_is_refused_in_the_skills_run() {
    let load =
        json!({"type": "tool_call", "data": {"name": "load_skill", "args": {"name": SKILL}}});
    let r = ask_before_tool(std::slice::from_ref(&load)).unwrap();
    assert_eq!(r["tool"], NEWS_GATHER);
    assert!(ask_before_tool(&[load, json!({"type": "workflow_call", "data": {}})]).is_none());
    assert!(ask_before_tool(&[]).is_none());
}

#[test]
fn an_articles_date_and_publication_come_from_structured_fields() {
    // an engine's timestamp or a page's published time is shown as a date; any other text stays as it is
    assert_eq!(shown_date("2026-10-31T09:00:00.0000000"), "31 October 2026");
    assert_eq!(shown_date("2026-10-30"), "30 October 2026");
    assert_eq!(shown_date("2 days ago"), "2 days ago");
    // no byline is looked for in the text: the opening is the article's lead paragraph, else its text
    let art = json!({"title": "T", "lead": "", "text": "T\nReuters · 31 October 2026\nThe body."});
    assert_eq!(opening(&art), "Reuters · 31 October 2026 The body.");
    let art = json!({"title": "T", "lead": "The lead paragraph.", "text": "T\nwhatever"});
    assert_eq!(opening(&art), "The lead paragraph.");
}

// ---------------------------------------------------------------- monitoring a price with the news ("Monitor BTC price
// and news. Send me an update with research report every day at 8am.": Main's first example, btc_monitor_real)

#[test]
fn market_symbols_are_normalised_in_code() {
    assert_eq!(
        market_symbols(Some(&json!([
            "btc",
            " ETH-USD ",
            "BTC-USD",
            "AAPL",
            "not a symbol!",
            "^GSPC"
        ]))),
        ["BTC-USD", "ETH-USD", "AAPL", "^GSPC"]
    );
    assert_eq!(
        market_symbols(Some(&json!("BTC, gc=f"))),
        ["BTC-USD", "GC=F"]
    );
    assert!(market_symbols(None).is_empty());
    let a = gargs(json!({"subject": "Bitcoin", "window": "day", "markets": ["BTC"]}));
    assert_eq!(a["markets"], json!(["BTC-USD"]));
    // no markets asked: none in the arguments
    assert!(gargs(json!({"subject": "Bitcoin"}))
        .get("markets")
        .is_none());
}

#[test]
fn market_quotes_keep_only_structured_prices() {
    let q = quotes_of(&json!({"results": [
        {"symbol": "BTC-USD", "name": "Bitcoin USD", "currency": "USD", "last": 68123.45, "last_time": "2026-11-01 07:58",
         "day_change_pct": -0.5, "series": [[1, 2]]},
        {"symbol": "X", "last": null}], "errors": ["NOPE: no data"]}));
    assert_eq!(
        q,
        json!({"quotes": [{"symbol": "BTC-USD", "name": "Bitcoin USD", "currency": "USD", "last": 68123.45,
                           "last_time": "2026-11-01 07:58", "day_change_pct": -0.5}], "errors": ["NOPE: no data"]})
    );
    assert_eq!(
        price_line(&q["quotes"][0]).unwrap(),
        "68,123.45 USD (-0.50% on the day; at 2026-11-01 07:58)"
    );
    assert_eq!(price_label(&q["quotes"][0]), "Bitcoin USD (BTC-USD)");
    assert_eq!(price_text(0.123456), "0.1235");
    assert_eq!(quotes_of(&json!({"error": "down"}))["quotes"], json!([]));
}

/// The daily BTC update in ONE call: the prices are fetched in the same turn as the first news search, carried into
/// the edition (the HTML's markets line) and appended to the reply; the schedule is daily at 08:00 and its task keeps
/// the symbols.
fn btc_run(profile: &Value) -> (Vec<Value>, Value) {
    let request =
        "Monitor BTC price and news. Send me an update with research report every day at 8am.";
    let args = gargs(
        json!({"subject": "Bitcoin", "window": "day", "markets": ["BTC-USD"], "schedule": "daily", "hour": 8, "email": true}),
    );
    run(
        NEWS_GATHER,
        &args,
        started(request, NEWS_GATHER, &args),
        &machine(profile, false),
        tools,
    )
}

#[test]
fn a_price_monitor_fetches_the_prices_with_the_news_and_shows_them() {
    let none = json!({});
    let (t, res) = btc_run(&none);
    // one turn: the market prices and the first news search
    let first = t
        .iter()
        .find(|m| !tool_calls(m).is_empty() && get_str(&tool_calls(m)[0], "id").starts_with(PREFIX))
        .unwrap();
    let names: Vec<String> = tool_calls(first)
        .iter()
        .map(|tc| stored_call(tc).0)
        .collect();
    assert_eq!(names, [MARKET_TOOL, "browser_news_search"]);
    assert_eq!(calls_of(&t, MARKET_TOOL)[0]["symbols"], json!(["BTC-USD"]));
    // the edition carries the price (code's line), the schedule is daily at 08:00 and its task keeps the symbols
    let rep = &calls_of(&t, "news_report")[0];
    assert_eq!(
        rep["markets"],
        json!([{"label": "Bitcoin USD (BTC-USD)", "price": "68,123.45 USD (+1.20% on the day; at 2026-11-01 07:58)"}])
    );
    let sch = &calls_of(&t, "schedule_create")[0];
    assert_eq!(sch["spec"], "0 8 * * *");
    assert_eq!(sch["task"]["args"]["markets"], json!(["BTC-USD"]));
    let shown = super::super::show::lines(&res, false);
    assert_eq!(
        shown[0],
        "- Bitcoin USD (BTC-USD): 68,123.45 USD (+1.20% on the day; at 2026-11-01 07:58)"
    );
    assert!(
        shown
            .iter()
            .any(|l| l.starts_with("- New edition: every day at 08:00")),
        "{shown:?}"
    );
}

/// The delivery is code's, from the connection: no mailbox — the update is in the chat only, nothing is sent, and the
/// schedule's delivery is the chat; a mailbox — the update is also mailed to the user's own address and the schedule
/// records email as a channel.
#[test]
fn send_me_delivers_in_the_chat_and_by_email_only_when_a_mailbox_is_connected() {
    let none = json!({});
    let (t, res) = btc_run(&none);
    assert!(calls_of(&t, "gmail_send").is_empty());
    assert_eq!(res["delivery"], json!({"chat": true, "email": null}));
    assert_eq!(res["schedule"]["delivery"], json!(["chat"]));
    assert!(super::super::show::lines(&res, false)
        .iter()
        .any(|l| l.starts_with("- Delivered: here in this chat, each new edition too")));

    let mail = json!({"mailbox": "me@example.com"});
    let (t, res) = btc_run(&mail);
    assert_eq!(calls_of(&t, "gmail_send")[0]["to"], "me@example.com");
    assert_eq!(
        res["delivery"],
        json!({"chat": true, "email": "me@example.com"})
    );
    assert_eq!(res["schedule"]["delivery"], json!(["chat", "email"]));

    // the decision itself: asked by the model's `email`, by a schedule, or in a schedule's own run; sent by email only
    // with a mailbox
    let m = |p| machine(p, false);
    assert_eq!(delivery(&json!({"email": true}), &m(&none)), (true, false));
    assert_eq!(delivery(&json!({"email": true}), &m(&mail)), (true, true));
    assert_eq!(
        delivery(&json!({"subject": "x", "schedule": "daily"}), &m(&mail)),
        (true, true)
    );
    assert_eq!(delivery(&json!({}), &m(&mail)), (false, false));
    assert_eq!(delivery(&json!({}), &machine(&mail, true)), (true, true));
    assert_eq!(delivery(&json!({}), &machine(&none, true)), (true, false));
}
