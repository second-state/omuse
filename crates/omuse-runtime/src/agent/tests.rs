//! Unit tests of the agent (ports of tests/test_027.py and the agent parts of tests/test_units.py).

use std::sync::Arc;

use omuse_common::Config;
use parking_lot::Mutex;
use serde_json::{json, Value};

use super::*;
use crate::llm::ToolCall;

fn tools(names: &[&str]) -> Vec<Value> {
    names
        .iter()
        .map(|n| json!({"type": "function", "function": {"name": n}}))
        .collect()
}

fn nav_turn(i: usize) -> Value {
    json!({"role": "assistant", "content": "", "tool_calls": [{"id": format!("c{}", i), "type": "function",
           "function": {"name": "browser_navigate", "arguments": "{}"}}]})
}

fn names(v: &[Value]) -> std::collections::BTreeSet<String> {
    v.iter().map(|t| tool_name(t).to_string()).collect()
}

type Calls = Arc<Mutex<Vec<(String, Value)>>>;

/// A runtime over a temporary data dir / workspace; Sentinel answered by `answer(path, payload)`.
fn rt_with(
    answer: impl Fn(&str, &Value) -> Value + Send + Sync + 'static,
) -> (Arc<Runtime>, tempfile::TempDir, Calls) {
    let dir = tempfile::tempdir().unwrap();
    let toml = format!("[data]\ndir = \"{}\"\n", dir.path().display());
    let cfg = Arc::new(Config::parse(&toml).unwrap());
    let publish: Publish = Arc::new(|_ev| Box::pin(async {}));
    let rt = Runtime::new(&dir.path().join("runtime"), cfg, publish).unwrap();
    let calls = Arc::new(Mutex::new(Vec::new()));
    let c2 = calls.clone();
    let answer = Arc::new(answer);
    rt.set_sentinel_hook(Some(Arc::new(move |_m, path, payload| {
        let p = payload.unwrap_or(Value::Null);
        c2.lock().push((path.clone(), p.clone()));
        let r = answer(&path, &p);
        Box::pin(async move { Ok(r) })
    })));
    (rt, dir, calls)
}

fn rt() -> (Arc<Runtime>, tempfile::TempDir) {
    let (r, d, _) = rt_with(|_, _| json!({}));
    (r, d)
}

// ---------------------------------------------------------------- step budget (test_027.py)
#[test]
fn budget_note_once_and_finish_tools_only() {
    let mut tr = vec![
        json!({"role": "system", "content": ""}),
        json!({"role": "user", "content": "research X and make a PDF"}),
    ];
    let all = tools(&[
        "browser_navigate",
        "browser_snapshot",
        "make_pdf",
        "send_file",
        "files_write",
        "gmail_send",
    ]);
    let out = Runtime::budget(&mut tr, all.clone(), 20, "zh");
    assert_eq!(out, all);
    assert_eq!(tr.len(), 2);
    let out = Runtime::budget(&mut tr, all.clone(), 5, "zh");
    assert!(tr.last().unwrap()["content"]
        .as_str()
        .unwrap()
        .contains(BUDGET_MARK));
    assert_eq!(out.len(), all.len());
    let n = tr.len();
    Runtime::budget(&mut tr, all.clone(), 4, "zh");
    assert_eq!(tr.len(), n); // said only once
    let out = Runtime::budget(&mut tr, all, 2, "zh");
    assert_eq!(
        names(&out),
        ["files_write", "gmail_send", "make_pdf", "send_file"]
            .iter()
            .map(|s| s.to_string())
            .collect()
    );
    assert!(!FINISH_TOOLS.contains(&"browser_navigate"));
}

#[test]
fn budget_research_nudge_after_many_pages() {
    let mut tr = vec![
        json!({"role": "system", "content": ""}),
        json!({"role": "user", "content": "news brief"}),
    ];
    tr.extend((0..10).map(nav_turn));
    Runtime::budget(&mut tr, tools(&["browser_navigate"]), 25, "zh");
    let last = tr.last().unwrap()["content"].as_str().unwrap().to_string();
    assert!(last.contains(BUDGET_MARK_PAGES) && last.contains("10"));
    let n = tr.len();
    tr.push(nav_turn(11));
    Runtime::budget(&mut tr, tools(&["browser_navigate"]), 24, "zh");
    assert_eq!(tr.len(), n + 1); // nudged once
}

#[test]
fn budget_nudges_steer_to_delegate_not_quota() {
    let mut tr = vec![json!({"role": "user", "content": "找 3 个实习岗位"})];
    Runtime::budget(&mut tr, vec![], BUDGET_RESERVE, "zh");
    let m = tr.last().unwrap()["content"].as_str().unwrap();
    assert!(m.contains("delegate") && m.contains("不要向用户提") && m.contains("预算"));
    let mut tr2 = vec![json!({"role": "user", "content": "find 3 internships"})];
    Runtime::budget(&mut tr2, vec![], BUDGET_RESERVE, "en");
    let m2 = tr2.last().unwrap()["content"]
        .as_str()
        .unwrap()
        .to_lowercase();
    assert!(m2.contains("delegate") && m2.contains("do not mention") && m2.contains("budget"));
}

#[test]
fn local_tools_declared() {
    assert!(LOCAL_TOOLS.iter().any(|t| tool_name(t) == "make_xlsx"));
    assert!(
        is_local("present_choices") && is_local("watch_create") && !is_local("browser_navigate")
    );
    // every finish tool that is local is declared
    for n in FINISH_TOOLS {
        if !n.starts_with("gmail_")
            && !n.starts_with("slack_")
            && !n.starts_with("notion_")
            && !n.starts_with("calendar_")
        {
            assert!(is_local(n), "{n}");
        }
    }
}

// ---------------------------------------------------------------- blocked sites (test_027.py)
#[test]
fn site_brand_cases() {
    for (url, brand) in [
        ("https://www.opentable.com/s?x=1", "opentable"),
        ("https://m.opentable.sg/r/abc", "opentable"),
        ("opentable.sg", "opentable"),
        ("https://www.bbc.co.uk/news", "bbc"),
        ("https://shop.test:8099/a", "shop"),
        ("https://www.google.com.sg/maps", "google"),
    ] {
        assert_eq!(site_brand(url), brand, "{url}");
    }
}

#[test]
fn site_blocked_until_takeover() {
    let mut tr = vec![
        json!({"role": "tool", "content": "[SITE BLOCKED site=opentable] wall\n<untrusted_content>…"}),
    ];
    assert_eq!(
        site_blocked(&tr, "https://www.opentable.sg/singapore"),
        "opentable"
    );
    assert_eq!(site_blocked(&tr, "https://www.chope.co/"), "");
    tr.push(json!({"role": "tool", "content": "用户已完成接管并交还浏览器控制权。"}));
    assert_eq!(site_blocked(&tr, "https://www.opentable.com/"), "");
}

// ---------------------------------------------------------------- hints and duplicate schedules (test_units.py)
#[test]
fn deep_link_hint_and_duplicate_schedules() {
    let h = deep_link_hint(
        "https://aswbe.ana.co.jp/webapps/servicing/booking-search?CONNECTION_KIND=SGP&LANG=en",
        "https://aswbe.ana.co.jp/webapps/servicing/common/system-error",
        "Information",
    );
    assert!(h.contains("home page") && h.contains("NOT mean"));
    assert_eq!(
        deep_link_hint("https://shop.test/p/1", "https://shop.test/p/1", "Error"),
        ""
    );
    assert_eq!(
        deep_link_hint("https://a.test/x", "https://a.test/y", "Welcome"),
        ""
    );
    assert!(!deep_link_hint(
        "https://a.test/x",
        "https://a.test/login?session_expired=1",
        "Sign in"
    )
    .is_empty());
    let rows = vec![
        json!({"id": "s1", "enabled": 1, "name": "ANA 选座重试", "goal": "为 ANA 预订 DERKAI 的三位乘客选座。背景：……"}),
        json!({"id": "s2", "enabled": 0, "name": "old", "goal": "something else entirely, long enough"}),
    ];
    assert_eq!(
        same_schedule(
            &rows,
            "ANA选座 第2次",
            "为 ANA 预订 DERKAI 的三位乘客选座 背景……"
        )
        .unwrap()["id"],
        "s1"
    );
    assert_eq!(
        same_schedule(&rows, "ANA 选座重试", "different goal text here").unwrap()["id"],
        "s1"
    );
    assert!(same_schedule(&rows, "old", "something else entirely, long enough").is_none());
    assert!(same_schedule(&rows, "new", "每天早上 9 点把新邮件摘要发给我").is_none());
}

// ---------------------------------------------------------------- loop guards
fn call(id: &str, name: &str, args: Value) -> ToolCall {
    ToolCall {
        id: id.into(),
        name: name.into(),
        args,
    }
}

fn turn(calls: &[(&str, &str, Value)]) -> Value {
    json!({"role": "assistant", "content": "", "tool_calls": calls.iter().map(|(id, n, a)| json!({"id": id, "type": "function",
        "function": {"name": n, "arguments": omuse_common::pyjson::dumps(a)}})).collect::<Vec<_>>()})
}

#[test]
fn repeat_guard_blocks_streaks_and_plan_loops() {
    let a = json!({"url": "http://x.test/feed.xml"});
    let tr = vec![
        turn(&[("c1", "browser_navigate", a.clone())]),
        turn(&[("c2", "browser_navigate", a.clone())]),
    ];
    let r = repeat_guard(&tr, &call("c3", "browser_navigate", a)).unwrap();
    assert!(r.contains("重复调用已拦截") && r.contains("3x"));
    let tr: Vec<Value> = (0..3)
        .map(|i| turn(&[(&format!("p{i}"), "update_plan", json!({"steps": []}))]))
        .collect();
    assert!(repeat_guard(&tr, &call("p9", "update_plan", json!({})))
        .unwrap()
        .contains("update_plan"));
    // scrolling repeatedly is normal
    let s = json!({"direction": "down"});
    let tr = vec![
        turn(&[("s1", "browser_scroll", s.clone())]),
        turn(&[("s2", "browser_scroll", s.clone())]),
    ];
    assert!(repeat_guard(&tr, &call("s3", "browser_scroll", s)).is_none());
}

#[test]
fn reread_points_back_and_invented_ids_are_refused() {
    let a = json!({"path": "notes/a.md"});
    let tr = vec![
        turn(&[("r1", "files_read", a.clone())]),
        json!({"role": "tool", "tool_call_id": "r1", "content": "hello"}),
    ];
    let r = reread_guard(&tr, &call("r2", "files_read", a)).unwrap();
    assert!(r.starts_with("已读取过") && r.contains("r1"));
    let tr = vec![json!({"role": "tool", "content": "id g2:abc123456"})];
    assert!(invented_id_guard(
        &tr,
        &call(
            "x",
            "gmail_get_message",
            json!({"message_id": "g2:abc123456"})
        ),
        None
    )
    .is_none());
    assert!(invented_id_guard(
        &tr,
        &call(
            "x",
            "gmail_get_message",
            json!({"message_id": "1878038389"})
        ),
        None
    )
    .unwrap()
    .contains("Never invent"));
}

#[test]
fn host_guard_after_three_failures() {
    let mut ev = Vec::new();
    for i in 0..3 {
        ev.push(json!({"type": "tool_call", "data": {"call_id": format!("c{i}"), "args": {"url": format!("http://www.nosuch.test/q{i}")}}}));
        ev.push(json!({"type": "tool_result", "data": {"call_id": format!("c{i}"), "ok": false}}));
    }
    let f = source_failures(&ev);
    assert_eq!(f.get("nosuch.test"), Some(&3));
    assert!(dead_ends_text(&f, "en").starts_with("Dead ends in this task"));
    let c = call(
        "c9",
        "browser_navigate",
        json!({"url": "http://nosuch.test/other"}),
    );
    assert!(host_guard(|| ev.clone(), &c)
        .unwrap()
        .contains("nosuch.test"));
}

// ---------------------------------------------------------------- answer checks
#[test]
fn ungrounded_numbers_numcheck() {
    let tr = vec![
        json!({"role": "user", "content": "NUMCHECK how much do I save"}),
        turn(&[(
            "k1",
            "calculate",
            json!({"expressions": ["6.5*365 - 6.5*3*52"]}),
        )]),
        json!({"role": "tool", "tool_call_id": "k1", "content": "6.5*365 - 6.5*3*52 = 1,358.50\ninvest(...) = final balance 16,310.30"}),
    ];
    assert_eq!(
        ungrounded_numbers(
            &tr,
            "Saves 1,358.50 a year; after 10 years about 17,016.64",
            "NUMCHECK"
        ),
        vec!["17,016.64"]
    );
    assert!(ungrounded_numbers(
        &tr,
        "FIXED: saves 1,358.50 a year; after 10 years 16,310.30",
        "NUMCHECK"
    )
    .is_empty());
    assert!(self_corrects("……10.5 超过 10？不对，等一下，这里需要修正"));
    let tr2 = vec![json!({"role": "tool", "content": "SQ 638 departs 09:00"})];
    assert_eq!(
        ungrounded_codes(&tr2, "Your flight SQ 637 departs at 11:30", ""),
        vec!["SQ 637"]
    );
}

#[test]
fn cut_off_and_asks_user_and_stranded() {
    assert!(cut_off_final("Sheng Siong 搜索\"gard", None, 0));
    assert!(!cut_off_final("完成了。", None, 2));
    assert!(cut_off_final("anything", Some("length"), 0));
    assert!(asks_user("我没找到预约记录。请告诉我是哪家诊所？"));
    assert!(!asks_user("任务失败了。"));
    let table = "x".repeat(320);
    let tr = vec![
        json!({"role": "user", "content": "bills"}),
        json!({"role": "assistant", "content": table.clone(), "tool_calls": [{"id": "u"}]}),
    ];
    assert!(
        merge_stranded_answer(&tr, "The summary table above covers all bills.").starts_with(&table)
    );
}

#[test]
fn compress_shrinks_old_results_first() {
    let mut tr = vec![json!({"role": "system", "content": ""})];
    for i in 0..6 {
        tr.push(
            json!({"role": "tool", "tool_call_id": format!("t{i}"), "content": "a".repeat(20000)}),
        );
    }
    let out = runtime::compress_with(tr, None, 0);
    let first = out[1]["content"].as_str().unwrap();
    assert!(first.contains(COMPRESSED_MARK) && first.chars().count() < 600);
    assert!(!out[6]["content"]
        .as_str()
        .unwrap()
        .contains(COMPRESSED_MARK));
}

#[test]
fn search_key_ignores_order_and_case() {
    assert_eq!(
        search_key(&json!("Singapore Weather october")),
        search_key(&json!("october weather singapore"))
    );
}

// ---------------------------------------------------------------- runtime-level (test_units.py)
#[tokio::test]
async fn grounded_choices() {
    let (rt, _d) = rt();
    rt.remember("t1", "browser_navigate", &json!({"url": "https://shop.test/s?k=case", "snapshot": "Aurora Case\n  4.4 out of 5 stars\n  S$21.90\n[e9] link \"Kick\" → https://shop.test/p/7"}));
    let ok = vec![
        json!({"label": "Aurora Case", "details": ["S$21.90"], "source_url": "https://www.shop.test/s"}),
    ];
    assert!(rt.check_choices("t1", &ok).is_empty());
    assert!(!rt.check_choices("t1", &[json!({"label": "Aurora Case", "details": ["S$9.90"], "source_url": "https://shop.test/s"})]).is_empty());
    assert!(!rt.check_choices("t1", &[json!({"label": "X", "details": ["S$21.90"], "source_url": "https://elsewhere.test/"})]).is_empty());
    // a product link seen on a page that was read counts, checked against that page's text
    assert!(rt.check_choices("t1", &[json!({"label": "Aurora Case", "details": ["4.4 out of 5 stars"], "source_url": "https://shop.test/p/7"})]).is_empty());
    // a long product name the page shortened with "…": its first 60 characters, exactly, are enough
    let longname = "OtterBox Defender Series Pro XT Clear MagSafe Case for iPhone 17 Pro Max, Shockproof, Drop proof";
    let shown: String = longname.chars().take(88).collect();
    rt.remember("t1", "browser_navigate", &json!({"url": "https://shop.test/s?k=otter", "snapshot": format!("[e3] link \"{}…\"\n  S$50.15", shown)}));
    assert!(rt.check_choices("t1", &[json!({"label": longname, "details": ["S$50.15"], "source_url": "https://shop.test/s"})]).is_empty());
    assert!(!rt.check_choices("t1", &[json!({"label": "OtterBox Commuter Series", "details": ["S$50.15"], "source_url": "https://shop.test/s"})]).is_empty());
}

#[tokio::test]
async fn learned_facts_wait_for_the_user() {
    let (rt, _d) = rt();
    let t = json!({"id": "t1", "goal": "在 decathlon.sg 买一双袜子", "conv_id": ""});
    let out = rt.local(&t, "memory_remember", &json!({"fact": "decathlon.sg asks for the phone as 8 digits without +65.", "domain": "site"})).await.unwrap();
    let f = rt
        .store
        .search_facts("decathlon phone", 5, Some("long"), true)
        .unwrap()[0]
        .clone();
    assert_eq!(f["status"], "pending");
    assert_eq!(f["domain"], "site");
    assert!(out.contains("确认"));
    let t2 = json!({"id": "t2", "goal": "记住：我订酒店只要安静的精品酒店", "conv_id": ""});
    rt.local(
        &t2,
        "memory_remember",
        &json!({"fact": "The user wants quiet boutique hotels."}),
    )
    .await
    .unwrap();
    let g = rt
        .store
        .facts(500, Some("long"))
        .unwrap()
        .into_iter()
        .find(|x| x["fact"].as_str().unwrap().contains("boutique"))
        .unwrap();
    assert_eq!(g["status"], "active");
}

#[tokio::test]
async fn site_habits_show_up_when_the_site_opens() {
    let (rt, _d) = rt();
    rt.store
        .add_fact(
            "decathlon.sg 支持访客结账；电话填 8 位，不要 +65。",
            "other",
            "",
            &Default::default(),
        )
        .unwrap();
    rt.store
        .add_fact(
            "The user likes jazz.",
            "preference",
            "",
            &Default::default(),
        )
        .unwrap();
    let note = rt
        .site_note("t1", "https://www.decathlon.sg/checkout")
        .await;
    assert!(note.contains("8 位") && !note.contains("jazz"));
    assert_eq!(
        rt.site_note("t1", "https://www.decathlon.sg/cart").await,
        ""
    ); // once per task
    assert_eq!(rt.site_note("t1", "https://www.ikea.com/sg").await, "");
}

#[tokio::test]
async fn block_is_learned_as_a_site_habit() {
    let (rt, _d) = rt();
    rt.learn_site_block("t1", "decathlon", "Cloudflare bot check")
        .await;
    let f = rt
        .store
        .search_facts("decathlon 反机器人 bot", 5, Some("long"), true)
        .unwrap();
    assert!(
        !f.is_empty()
            && f[0]["domain"] == "site"
            && f[0]["status"] == "pending"
            && f[0]["fact"].as_str().unwrap().contains("拦截")
    );
    let n = rt.store.facts(500, Some("long")).unwrap().len();
    rt.learn_site_block("t2", "decathlon", "Cloudflare bot check")
        .await; // once per site
    assert_eq!(rt.store.facts(500, Some("long")).unwrap().len(), n);
}

fn png(path: &std::path::Path, w: u32, h: u32) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    image::RgbImage::from_pixel(w, h, image::Rgb([30, 120, 200]))
        .save(path)
        .unwrap();
}

#[tokio::test]
async fn caption_image_calls_compose_with_exact_text() {
    // the fake compose writes the output file, as the browser service would
    let ws: Arc<Mutex<Option<std::path::PathBuf>>> = Arc::new(Mutex::new(None));
    let ws2 = ws.clone();
    let (rt, _d, calls) = rt_with(move |path, body| {
        if path == "/internal/compose_image" {
            if let Some(w) = ws2.lock().clone() {
                png(&w.join(body["output"].as_str().unwrap()), 64, 48);
            }
            json!({"path": body["output"], "size": 1234, "width": 64, "height": 48, "font_px": 20})
        } else {
            json!({})
        }
    });
    *ws.lock() = Some(rt.workspace.clone());
    let conv = rt.store.create_conv("c", "chat", None).unwrap();
    png(&rt.path("images/poster.png").unwrap(), 64, 48);
    rt.inner
        .lock()
        .last_image
        .insert("t1".into(), "images/poster.png".into());
    let t = json!({"id": "t1", "conv_id": conv});
    let r = rt.caption_image(&t, &json!({"text": "双十一 大促 50% OFF", "position": "top", "band": true, "color": "gold"})).await;
    assert!(r.contains("letter-perfect"), "{r}");
    let (_, b) = calls
        .lock()
        .iter()
        .find(|(p, _)| p == "/internal/compose_image")
        .cloned()
        .unwrap();
    assert_eq!(b["image"], "images/poster.png");
    assert_eq!(b["text"], "双十一 大促 50% OFF");
    assert_eq!(b["position"], "top");
    assert_eq!(b["band"], true);
    assert_eq!(b["output"], "images/poster_text.png");
    assert_eq!(rt.inner.lock().last_image["t1"], "images/poster_text.png");
    assert!(rt
        .caption_image(&t, &json!({"text": ""}))
        .await
        .starts_with("ERROR"));
}

#[tokio::test]
async fn upscale_image_doubles_and_quadruples() {
    let (rt, _d) = rt();
    let conv = rt.store.create_conv("c", "chat", None).unwrap();
    png(&rt.path("images/cat.png").unwrap(), 64, 48);
    rt.inner
        .lock()
        .last_image
        .insert("t1".into(), "images/cat.png".into());
    let t = json!({"id": "t1", "conv_id": conv});
    let r = rt.upscale_image(&t, &json!({})).await;
    assert!(r.contains("upscaled"), "{r}");
    assert_eq!(
        image::image_dimensions(rt.path("images/cat_x2.png").unwrap()).unwrap(),
        (128, 96)
    );
    rt.upscale_image(&t, &json!({"path": "images/cat.png", "factor": 4}))
        .await;
    assert_eq!(
        image::image_dimensions(rt.path("images/cat_x4.png").unwrap()).unwrap(),
        (256, 192)
    );
    assert_eq!(rt.inner.lock().last_image["t1"], "images/cat_x4.png");
    png(&rt.path("images/huge.png").unwrap(), 5000, 100);
    assert!(rt
        .upscale_image(&t, &json!({"path": "images/huge.png", "factor": 2}))
        .await
        .contains("8192"));
    assert!(rt
        .upscale_image(&json!({"id": "t9", "conv_id": conv}), &json!({}))
        .await
        .starts_with("ERROR"));
}

#[tokio::test]
async fn image_budget_per_task() {
    let (rt, _d) = rt();
    rt.image_budget
        .store(0, std::sync::atomic::Ordering::Relaxed);
    let r = rt
        .make_image(&json!({"id": "tb1"}), &json!({"prompt": "a cat"}))
        .await;
    assert!(r.starts_with("ERROR") && r.contains("上限"), "{r}");
    // workspace escapes are refused, never read
    for bad in ["../../etc/passwd", "/etc/hostname"] {
        let r = rt
            .edit_image(
                &json!({"id": "t1"}),
                &json!({"path": bad, "prompt": "x"}),
                false,
            )
            .await;
        assert!(r.starts_with("ERROR"), "{r}");
    }
    assert!(rt
        .edit_image(&json!({"id": "t1"}), &json!({"prompt": "add a hat"}), false)
        .await
        .contains("make_image"));
}

#[tokio::test]
async fn present_choices_clarify_and_files_roundtrip() {
    let (rt, _d) = rt();
    let conv = rt.store.create_conv("c", "chat", None).unwrap();
    let t = json!({"id": "t1", "conv_id": conv, "goal": "x"});
    let r = rt.local(&t, "present_choices", &json!({"question": "Which?", "kind": "clarify", "options": [{"label": "A"}, {"label": "B"}]})).await.unwrap();
    assert!(r.starts_with("已显示 2 张选项卡") && r.contains("1. A"));
    let w = rt
        .local(
            &t,
            "files_write",
            &json!({"path": "notes/a.md", "content": "hello 你好"}),
        )
        .await
        .unwrap();
    assert!(
        w.contains("notes/a.md") && w.contains("2 Chinese characters + 1 words"),
        "{w}"
    );
    let rd = rt
        .local(&t, "files_read", &json!({"path": "notes/a.md"}))
        .await
        .unwrap();
    assert_eq!(
        rd,
        "<untrusted_content source=\"file notes/a.md\">\nhello 你好\n</untrusted_content>"
    );
    assert!(rt
        .local(&t, "files_read", &json!({"path": "../x"}))
        .await
        .is_err());
    let s = rt
        .send_file(&t, &json!({"path": "notes/a.md"}))
        .await
        .unwrap();
    assert!(s.starts_with("已发送到对话"));
    assert!(rt
        .send_file(&t, &json!({"path": "notes/a.md"}))
        .await
        .unwrap()
        .starts_with("已经发过了"));
}

#[test]
fn format_external_shapes() {
    let (rt, _d) = rt();
    let r = rt.format_external(
        "gmail_search",
        &json!({"status": "ok", "result": {"trust": "untrusted", "source": "gmail", "items": [1]}}),
    );
    assert!(r.starts_with("<untrusted_content source=\"gmail\">\n{\"trust\": \"untrusted\""));
    assert!(rt
        .format_external("x", &json!({"status": "denied", "reason": "no"}))
        .starts_with("DENIED by Sentinel: no."));
    assert!(rt
        .format_external("x", &json!({"status": "error", "error": "boom"}))
        .starts_with("ERROR: boom."));
}

// ---------------------------------------------------------------- 429 + Retry-After: cancel stops a sleeping model call
struct Always429(Arc<std::sync::atomic::AtomicUsize>);

impl crate::llm::LlmTransport for Always429 {
    fn send(
        &self,
        _req: crate::llm::HttpRequest,
    ) -> futures::future::BoxFuture<'_, Result<crate::llm::HttpResponse, String>> {
        self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Box::pin(async {
            Ok(crate::llm::HttpResponse {
                status: 429,
                retry_after: Some("300".into()),
                text: "{\"error\": {\"message\": \"rate limited\"}}".into(),
            })
        })
    }
}

#[tokio::test]
async fn cancel_interrupts_a_retry_after_sleep() {
    let dir = tempfile::tempdir().unwrap();
    let cfg =
        Arc::new(Config::parse(&format!("[data]\ndir = \"{}\"\n", dir.path().display())).unwrap());
    let events: Arc<Mutex<Vec<Value>>> = Arc::new(Mutex::new(Vec::new()));
    let ev2 = events.clone();
    let publish: Publish = Arc::new(move |ev| {
        ev2.lock().push(ev);
        Box::pin(async {})
    });
    let hits = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let rt = Runtime::with_llm_transport(
        &dir.path().join("runtime"),
        cfg,
        publish,
        Some(Arc::new(Always429(hits.clone()))),
    )
    .unwrap();
    rt.set_sentinel_hook(Some(Arc::new(|_, _, _| Box::pin(async { Ok(json!({})) }))));
    rt.store.set_settings(&json!({"model_base_url": "http://model.test/v1", "model_name": "fake", "model_api_key": "k"})).unwrap();
    let conv = rt.store.create_conv("c", "chat", None).unwrap();
    let t = rt.submit(&conv, "hello", "chat", "", None).await.unwrap();
    let tid = t["id"].as_str().unwrap().to_string();
    // wait for the first retry notice: the model call is now sleeping 300 s on the Retry-After
    let t0 = std::time::Instant::now();
    loop {
        let seen = events.lock().iter().any(|e| {
            e["kind"] == "task_event" && e["type"] == "llm_retry" && e["task_id"] == tid.as_str()
        });
        if seen {
            break;
        }
        assert!(t0.elapsed().as_secs() < 10, "no llm_retry event");
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    let retry = events
        .lock()
        .iter()
        .find(|e| e["type"] == "llm_retry")
        .cloned()
        .unwrap();
    for k in ["attempt", "of", "wait_s", "reason", "source", "purpose"] {
        assert!(retry["data"].get(k).is_some(), "{k}");
    }
    assert!(rt.is_running(&tid));
    rt.cancel(&tid, "").await;
    let t1 = std::time::Instant::now();
    while rt.is_running(&tid) {
        assert!(
            t1.elapsed().as_secs() < 3,
            "the task kept sleeping after cancel"
        );
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    assert_eq!(rt.store.task(&tid).unwrap().unwrap()["status"], "CANCELLED");
    let n = hits.load(std::sync::atomic::Ordering::SeqCst);
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    assert_eq!(hits.load(std::sync::atomic::Ordering::SeqCst), n); // no more requests after the cancel
}
