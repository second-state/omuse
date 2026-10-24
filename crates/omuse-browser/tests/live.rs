//! Live test against a local Chromium and the fixture pages of `tests/pages` served on 127.0.0.1:8099
//! (`cd tests/pages && python3 -m http.server 8099`). Ignored by default: run with
//! `cargo test -p omuse-browser --test live -- --ignored --nocapture`. It skips itself when no Chromium is found.
//!
//! The pages live on a loopback address, which the egress guard blocks, so navigation goes through the broker's
//! page API directly (the route-level `navigate` is exercised for its 403) with the egress filter off.

use std::sync::Arc;
use std::time::Duration;

use omuse_common::util::get_str;
use omuse_common::Config;
use regex::Regex;
use serde_json::{json, Value};

use omuse_browser::broker::Broker;
use omuse_browser::launch;
use omuse_browser::routes;

const BASE: &str = "http://127.0.0.1:8099";

fn config(dir: &std::path::Path) -> Arc<Config> {
    let toml = format!(
        "[browser]\nheadless = \"1\"\nidle_shutdown = 0\n[data]\ndir = \"{}\"\n[internal]\nbrowser_token = \"bt-test\"\n[server]\nbrowser_port = 18082\n",
        dir.display()
    );
    Arc::new(Config::parse(&toml).unwrap())
}

fn re(pat: &str, text: &str) -> String {
    Regex::new(pat)
        .unwrap()
        .captures(text)
        .map(|c| c[1].to_string())
        .unwrap_or_else(|| panic!("no match for {pat} in:\n{text}"))
}

async fn open(b: &Broker, task: &str, path: &str) -> Value {
    let page = b.page(task).await.unwrap();
    page.navigate(&format!("{}/{}", BASE, path), Duration::from_secs(20))
        .await
        .unwrap();
    b.settle(&page, 300).await;
    b.agent_action("snapshot", &json!({"task_id": task}))
        .await
        .unwrap()
}

async fn act(b: &Broker, action: &str, body: Value) -> Value {
    b.agent_action(action, &body)
        .await
        .unwrap_or_else(|e| panic!("{action} failed: {e}"))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore]
async fn broker_end_to_end() {
    let dir = std::env::temp_dir().join(format!("omuse-browser-live-{}", std::process::id()));
    std::fs::create_dir_all(dir.join("workspace")).unwrap();
    let cfg = config(&dir);
    if launch::find_executable(&cfg).is_none() {
        eprintln!("no chromium found: skipping");
        return;
    }
    if reqwest::get(format!("{}/page.html", BASE)).await.is_err() {
        eprintln!("fixture server not running on :8099: skipping");
        return;
    }
    let b = Broker::new(cfg.clone());
    b.egress_filter
        .store(false, std::sync::atomic::Ordering::SeqCst);

    // ---- page.html: snapshot, type, select, click, describe
    let snap = open(&b, "t1", "page.html").await;
    let text = get_str(&snap, "snapshot");
    println!("{text}");
    assert!(text.contains("# Order #12345"));
    assert!(text.contains("(password field — agent must NOT type here)"));
    assert!(text.contains("options=[S | M]"));
    assert_eq!(snap["title"], "Test Shop");
    let search = re(r#"\[(e\d+)\] searchbox "Search orders""#, &text);
    let snap = act(
        &b,
        "type",
        json!({"task_id": "t1", "ref": search, "text": "socks"}),
    )
    .await;
    assert!(get_str(&snap, "snapshot").contains(r#"searchbox "Search orders" value="socks""#));
    let sel = re(r#"\[(e\d+)\] combobox"#, &text);
    let snap = act(
        &b,
        "select",
        json!({"task_id": "t1", "ref": sel, "value": "S"}),
    )
    .await;
    assert!(get_str(&snap, "snapshot").contains(r#"selected="S""#));
    let d = act(
        &b,
        "describe",
        json!({"task_id": "t1", "ref": re(r#"\[(e\d+)\] button "Submit order""#, &text)}),
    )
    .await;
    assert_eq!(d["tag"], "button");
    assert_eq!(d["in_form"], true);
    assert!(get_str(&d, "page_url").ends_with("page.html"));
    let f = act(&b, "focused", json!({"task_id": "t1"})).await;
    assert_eq!(f["tag"], "input");
    let btn = re(r#"\[(e\d+)\] button "Submit order""#, &text);
    let snap = act(&b, "click", json!({"task_id": "t1", "ref": btn})).await;
    assert!(get_str(&snap, "url").contains("submitted.html"), "{snap}");
    let snap = act(&b, "back", json!({"task_id": "t1"})).await;
    assert!(get_str(&snap, "url").ends_with("page.html"));
    // stale / bad refs
    let e = b
        .agent_action("click", &json!({"task_id": "t1", "ref": "zzz"}))
        .await
        .unwrap_err();
    assert_eq!(e.status, 400);
    let e = b
        .agent_action("click", &json!({"task_id": "t1", "ref": "f9e1"}))
        .await
        .unwrap_err();
    assert_eq!(e.status, 409);
    let e = b
        .agent_action("click", &json!({"task_id": "t1", "ref": "e999"}))
        .await
        .unwrap_err();
    assert_eq!(e.status, 409);
    assert!(e.detail_text().starts_with("点击失败 click failed"));
    let e = b
        .agent_action("nonsense", &json!({"task_id": "t1"}))
        .await
        .unwrap_err();
    assert_eq!(e.status, 404);
    // page_text / press / wait / scroll
    let pt = act(&b, "page_text", json!({"task_id": "t1"})).await;
    assert!(get_str(&pt, "text").contains("Order #12345"));
    let s = act(&b, "scroll", json!({"task_id": "t1", "direction": "down"})).await;
    assert!(get_str(&s, "snapshot")
        .starts_with("(showing the part of the page around the current scroll position)"));
    assert_eq!(
        b.agent_action("scroll", &json!({"task_id": "t1", "direction": "sideways"}))
            .await
            .unwrap_err()
            .status,
        400
    );
    assert_eq!(
        b.agent_action("press", &json!({"task_id": "t1", "key": "Enter;rm"}))
            .await
            .unwrap_err()
            .status,
        400
    );
    act(&b, "press", json!({"task_id": "t1", "key": "Tab"})).await;

    // ---- shop.html: find / look / locate
    let snap = open(&b, "t2", "shop.html").await;
    assert!(snap["truncated"].as_bool().is_some());
    let f = act(
        &b,
        "find",
        json!({"task_id": "t2", "query": "Aurora Glitter Case"}),
    )
    .await;
    assert!(f["matches"].as_i64().unwrap() > 0, "{f}");
    assert!(get_str(&f, "snapshot").starts_with("browser_find \"Aurora Glitter Case\":"));
    let look = act(&b, "look", json!({"task_id": "t2"})).await;
    assert!(get_str(&look, "image_b64").len() > 2000);
    assert!(!look["marks"].as_array().unwrap().is_empty());
    assert!(get_str(&look, "snapshot").starts_with("Labelled elements visible on screen:\n[e"));
    let grid = act(&b, "locate", json!({"task_id": "t2"})).await;
    assert_eq!(grid["cols"], 8);
    assert_eq!(grid["labels"], "letters");
    assert_eq!(grid["region"], json!([0.0, 0.0, 1280.0, 800.0]));
    let zoom = act(
        &b,
        "locate",
        json!({"task_id": "t2", "region": [100, 100, 320, 200]}),
    )
    .await;
    assert_eq!(zoom["labels"], "numbers");
    assert!(get_str(&zoom, "image_b64").len() > 500);
    let mark = act(&b, "locate", json!({"task_id": "t2", "mark": [640, 400]})).await;
    assert!(mark["at"].is_object());

    // ---- support.html: open shadow root widget
    let snap = open(&b, "t3", "support.html").await;
    let text = get_str(&snap, "snapshot");
    let launch_ref = re(r#"\[(e\d+)\] button "Open chat widget""#, &text);
    let snap = act(&b, "click", json!({"task_id": "t3", "ref": launch_ref})).await;
    let text = get_str(&snap, "snapshot");
    let ta = re(r#"\[(e\d+)\] textbox "Type your message""#, &text);
    act(
        &b,
        "type",
        json!({"task_id": "t3", "ref": ta, "text": "Is there a free plan?", "submit": true}),
    )
    .await;
    tokio::time::sleep(Duration::from_millis(2000)).await;
    let snap = act(&b, "snapshot", json!({"task_id": "t3"})).await;
    assert!(
        get_str(&snap, "snapshot").contains("Lyra: Yes"),
        "{}",
        get_str(&snap, "snapshot")
    );

    // ---- support_iframe.html: a widget in an iframe (f1 refs)
    let snap = open(&b, "t4", "support_iframe.html").await;
    let text = get_str(&snap, "snapshot");
    assert!(text.contains("--- iframe f1:"), "{text}");
    let msg = re(r#"\[(f1e\d+)\] textbox "Message""#, &text);
    act(&b, "type", json!({"task_id": "t4", "ref": msg, "text": "How long does delivery take?", "submit": true})).await;
    tokio::time::sleep(Duration::from_millis(1800)).await;
    let snap = act(&b, "snapshot", json!({"task_id": "t4"})).await;
    assert!(
        get_str(&snap, "snapshot").contains("Zed: Standard delivery"),
        "{}",
        get_str(&snap, "snapshot")
    );
    let look = act(&b, "look", json!({"task_id": "t4"})).await;
    assert!(get_str(&look, "snapshot").contains("[f1e"));

    // ---- cardframes.html: card boxes in separate iframes, key-by-key typing
    let snap = open(&b, "t5", "cardframes.html").await;
    let text = get_str(&snap, "snapshot");
    let num = re(r#"\[(f\d+e\d+)\][^\n]*Card number"#, &text);
    let exp = re(r#"\[(f\d+e\d+)\][^\n]*Expiry date"#, &text);
    let cvc = re(r#"\[(f\d+e\d+)\][^\n]*Security code"#, &text);
    let pre = |r: &str| r.split('e').next().unwrap().to_string();
    assert_eq!(
        [pre(&num), pre(&exp), pre(&cvc)]
            .iter()
            .collect::<std::collections::HashSet<_>>()
            .len(),
        3
    );
    act(
        &b,
        "type",
        json!({"task_id": "t5", "ref": num, "text": "4111111111111111", "keys": true}),
    )
    .await;
    act(
        &b,
        "type",
        json!({"task_id": "t5", "ref": exp, "text": "12/2028", "keys": true, "expiry": true}),
    )
    .await;
    act(
        &b,
        "type",
        json!({"task_id": "t5", "ref": cvc, "text": "737", "keys": true}),
    )
    .await;
    let snap = act(&b, "snapshot", json!({"task_id": "t5"})).await;
    let pay = re(r#"\[(e\d+)\] button "Pay""#, &get_str(&snap, "snapshot"));
    act(&b, "click", json!({"task_id": "t5", "ref": pay})).await;
    let pt = act(&b, "page_text", json!({"task_id": "t5"})).await;
    assert!(
        get_str(&pt, "text").contains("yes:4111111111111111,yes:12/28,yes:737"),
        "{}",
        get_str(&pt, "text")
    );

    // ---- form_time.html: free text into <input type=time/date>
    let snap = open(&b, "t6", "form_time.html").await;
    let text = get_str(&snap, "snapshot");
    let tm = re(r#"\[(e\d+)\] textbox "Preferred delivery time""#, &text);
    let snap = act(
        &b,
        "type",
        json!({"task_id": "t6", "ref": tm, "text": "12:00 PM"}),
    )
    .await;
    assert!(get_str(&snap, "snapshot").contains(r#"value="12:00""#));
    let dt = re(r#"\[(e\d+)\] textbox "Date""#, &text);
    let snap = act(
        &b,
        "type",
        json!({"task_id": "t6", "ref": dt, "text": "2026/10/3"}),
    )
    .await;
    assert!(get_str(&snap, "snapshot").contains(r#"value="2026-10-03""#));

    // ---- lazylist.html: results that load late
    let t0 = std::time::Instant::now();
    let snap = open(&b, "t7", "lazylist.html").await;
    let text = get_str(&snap, "snapshot");
    assert!(
        text.contains("Kiprun socks") && !text.contains("Loading"),
        "{text}"
    );
    assert!(t0.elapsed() < Duration::from_secs(12));

    // ---- gallery.html: save_media by ref
    let snap = open(&b, "t8", "gallery.html").await;
    let text = get_str(&snap, "snapshot");
    let img = re(r#"\[(e\d+)\] img "Velvet lipstick""#, &text);
    // the picture's URL is on a loopback host: the egress guard refuses it (403), like the Python broker
    let e = b
        .agent_action("save_media", &json!({"task_id": "t8", "ref": img}))
        .await
        .unwrap_err();
    assert_eq!(e.status, 403);
    let tiny = "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNk+M9QDwADhgGAWjR9awAAAABJRU5ErkJggg==";
    let saved = act(
        &b,
        "save_media",
        json!({"task_id": "t8", "url": tiny, "name": "dot"}),
    )
    .await;
    assert_eq!(saved["saved"]["mime"], "image/png");
    assert_eq!(
        saved["saved"]["path"]
            .as_str()
            .unwrap()
            .rsplit('/')
            .next()
            .unwrap(),
        "dot.png"
    );
    assert_eq!(saved["saved"]["source"], "data: URL");
    assert!(get_str(&saved["saved"], "path").starts_with("media/"));
    let e = b
        .agent_action(
            "save_media",
            &json!({"task_id": "t8", "url": "data:text/html;base64,PGI+"}),
        )
        .await
        .unwrap_err();
    assert_eq!(e.status, 422);

    // ---- support_iframe.html again: what is under a point inside an iframe, and a visual click there
    open(&b, "t9", "support_iframe.html").await;
    let at = act(
        &b,
        "describe_at",
        json!({"task_id": "t9", "x": 1050, "y": 762}),
    )
    .await;
    assert_eq!(at["tag"], "input", "{at}");
    assert_eq!(at["name"], "Message");
    assert!(
        at["frames"][0]
            .as_str()
            .unwrap()
            .ends_with("support_widget.html"),
        "{at}"
    );
    let snap = act(&b, "click_at", json!({"task_id": "t9", "x": 1050, "y": 762, "text": "Where is my parcel?", "submit": true})).await;
    tokio::time::sleep(Duration::from_millis(1500)).await;
    let snap2 = act(&b, "snapshot", json!({"task_id": "t9"})).await;
    assert!(
        get_str(&snap2, "snapshot").contains("You: Where is my parcel?"),
        "{}\n{}",
        get_str(&snap, "snapshot"),
        get_str(&snap2, "snapshot")
    );
    // a cross-origin iframe whose host does not resolve here renders out of process: reported as an iframe, no error
    open(&b, "t9b", "bubble.html").await;
    let at = act(
        &b,
        "describe_at",
        json!({"task_id": "t9b", "x": 1220, "y": 740}),
    )
    .await;
    assert!(at["tag"] == "iframe" || at["tag"] == "button", "{at}");

    // ---- popup.html: a window.open popup follows the task; the user drives it; back to the opener when it closes
    let snap = open(&b, "t10", "popup.html").await;
    let g = re(
        r#"\[(e\d+)\] button "Continue with Google""#,
        &get_str(&snap, "snapshot"),
    );
    let snap = act(&b, "click", json!({"task_id": "t10", "ref": g})).await;
    assert!(get_str(&snap, "url").contains("popup_login.html"), "{snap}");
    let st = routes::state_json(&b).await;
    assert!(st["tasks"]
        .as_array()
        .unwrap()
        .iter()
        .any(|t| t["task_id"] == "t10" && get_str(t, "url").contains("popup_login")));
    {
        let mut s = b.st.lock();
        s.mode = "user".into();
        s.takeover_task = "t10".into();
        s.view_task = "t10".into();
    }
    let st = routes::state_json(&b).await;
    assert_eq!(st["popup"], true);
    assert!(get_str(&st, "popup_opener_url").ends_with("popup.html"));
    assert_eq!(
        b.agent_action("snapshot", &json!({"task_id": "t10"}))
            .await
            .unwrap_err()
            .status,
        423
    );
    b.user_input(&json!({"type": "click", "x": 250, "y": 120}))
        .await
        .unwrap();
    b.user_input(&json!({"type": "text", "text": "lucas@example.com"}))
        .await
        .unwrap();
    b.user_input(&json!({"type": "click", "x": 160, "y": 200}))
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(1200)).await;
    let st = routes::state_json(&b).await;
    assert!(
        get_str(&st, "url").ends_with("popup.html") && st["popup"] == false,
        "{st}"
    );
    {
        let mut s = b.st.lock();
        s.mode = "agent".into();
        s.takeover_task.clear();
    }
    let snap = act(&b, "snapshot", json!({"task_id": "t10"})).await;
    assert!(
        get_str(&snap, "snapshot").contains("Signed in as lucas@example.com"),
        "{}",
        get_str(&snap, "snapshot")
    );

    // ---- release / recycle / tabs
    act(&b, "release", json!({"task_id": "t7"})).await;
    assert!(b.st.lock().finished.contains_key("t7"));
    assert!(b.tabs() >= 10);
    let shot = b
        .view_page()
        .unwrap()
        .screenshot(true, Some(60), None, Duration::from_secs(8))
        .await
        .unwrap();
    assert!(shot.starts_with(&[0xFF, 0xD8, 0xFF]));

    // ---- HTTP routes: auth, 403 for a private host, 423 during takeover, downloads list
    let app = routes::router(b.handle());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let c = reqwest::Client::new();
    let base = format!("http://127.0.0.1:{}", port);
    let h = c
        .get(format!("{base}/health"))
        .send()
        .await
        .unwrap()
        .json::<Value>()
        .await
        .unwrap();
    assert_eq!(h["ok"], true);
    assert_eq!(h["started"], true);
    assert_eq!(
        c.get(format!("{base}/state"))
            .send()
            .await
            .unwrap()
            .status(),
        401
    );
    let r = c
        .post(format!("{base}/agent/navigate"))
        .header("X-Browser-Token", "bt-test")
        .json(&json!({"task_id": "t1", "url": "http://127.0.0.1:8099/page.html"}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 403);
    let r = c
        .post(format!("{base}/user/input"))
        .header("X-Browser-Token", "bt-test")
        .json(&json!({"type": "click"}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 409);
    let r = c
        .post(format!("{base}/user/takeover"))
        .header("X-Browser-Token", "bt-test")
        .json(&json!({"task_id": "t1"}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    let st: Value = r.json().await.unwrap();
    assert_eq!(st["mode"], "user");
    assert_eq!(st["takeover_task"], "t1");
    let r = c
        .post(format!("{base}/agent/snapshot"))
        .header("X-Browser-Token", "bt-test")
        .json(&json!({"task_id": "t1"}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 423);
    let r = c
        .post(format!("{base}/agent/snapshot"))
        .header("X-Browser-Token", "bt-test")
        .json(&json!({"task_id": "t2"}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    let r = c
        .post(format!("{base}/user/takeover"))
        .header("X-Browser-Token", "bt-test")
        .json(&json!({"task_id": "t2"}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 409);
    let r = c
        .get(format!("{base}/screenshot?task_id=t1"))
        .header("X-Browser-Token", "bt-test")
        .send()
        .await
        .unwrap();
    assert_eq!(r.headers().get("content-type").unwrap(), "image/jpeg");
    let r = c
        .post(format!("{base}/user/release"))
        .header("X-Browser-Token", "bt-test")
        .json(&json!({}))
        .send()
        .await
        .unwrap();
    assert_eq!(
        r.json::<Value>().await.unwrap()["released"]["task_id"],
        "t1"
    );
    let r = c
        .get(format!("{base}/agent/downloads"))
        .header("X-Browser-Token", "bt-test")
        .send()
        .await
        .unwrap();
    assert!(r.json::<Value>().await.unwrap()["downloads"].is_array());

    // ---- rendering: pdf / png / compose with the second headless Chromium
    let ws = cfg.workspace();
    std::fs::create_dir_all(ws.join("reports")).unwrap();
    std::fs::write(
        ws.join("reports/cn.md"),
        "# AI 报告\n\n**结论** 本地生成。\n\n| a | b |\n|---|---|\n| 1 | 2 |\n",
    )
    .unwrap();
    let r = c
        .post(format!("{base}/pdf"))
        .header("X-Browser-Token", "bt-test")
        .json(&json!({"source": "reports/cn.md"}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200, "{}", r.text().await.unwrap());
    let j: Value = r.json().await.unwrap();
    assert_eq!(j["path"], "reports/cn.pdf");
    assert_eq!(j["title"], "AI 报告");
    assert!(std::fs::read(ws.join("reports/cn.pdf"))
        .unwrap()
        .starts_with(b"%PDF"));
    let r = c
        .post(format!("{base}/pdf"))
        .header("X-Browser-Token", "bt-test")
        .json(&json!({"source": "../../etc/passwd"}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 400);
    let svg = "<svg xmlns='http://www.w3.org/2000/svg' width='320' height='200'><rect width='320' height='200' fill='#222'/></svg>";
    let r = c
        .post(format!("{base}/png"))
        .header("X-Browser-Token", "bt-test")
        .json(&json!({"svg": svg, "output": "images/bg"}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200, "{}", r.text().await.unwrap());
    let png = std::fs::read(ws.join("images/bg.png")).unwrap();
    assert!(png.starts_with(b"\x89PNG"));
    assert_eq!(
        omuse_browser::render::image_size(&ws.join("images/bg.png")).unwrap(),
        (640, 400)
    );
    let r = c.post(format!("{base}/compose")).header("X-Browser-Token", "bt-test")
        .json(&json!({"image": "images/bg.png", "text": "海报标题 Hello", "position": "bottom", "band": true, "color": "#ffffff"})).send().await.unwrap();
    assert_eq!(r.status(), 200, "{}", r.text().await.unwrap());
    let j: Value = r.json().await.unwrap();
    assert_eq!(j["path"], "images/bg_text.png");
    assert_eq!(j["width"], 640);
    assert!(j["font_px"].as_i64().unwrap() >= 14);
    assert_eq!(
        omuse_browser::render::image_size(&ws.join("images/bg_text.png")).unwrap(),
        (640, 400)
    );
    let r = c
        .post(format!("{base}/compose"))
        .header("X-Browser-Token", "bt-test")
        .json(&json!({"image": "images/nope.png", "text": "x"}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 404);

    b.stop().await;
    let _ = std::fs::remove_dir_all(&dir);
}

/// With the egress filter on (the default), a public page loads and a request from it to a loopback host fails
/// with `ERR_BLOCKED_BY_CLIENT`. Needs internet access; skipped without Chromium or network.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore]
async fn egress_filter_blocks_private_hosts() {
    let dir = std::env::temp_dir().join(format!("omuse-browser-egress-{}", std::process::id()));
    std::fs::create_dir_all(dir.join("workspace")).unwrap();
    let cfg = config(&dir);
    if launch::find_executable(&cfg).is_none()
        || reqwest::get("https://example.com/").await.is_err()
    {
        eprintln!("no chromium or no network: skipping");
        return;
    }
    let b = Broker::new(cfg);
    let snap = act(
        &b,
        "navigate",
        json!({"task_id": "e1", "url": "example.com"}),
    )
    .await;
    assert_eq!(snap["title"], "Example Domain", "{snap}");
    let page = b.page("e1").await.unwrap();
    let r = page
        .eval(None, "async () => { try { await fetch('http://127.0.0.1:8099/page.html', {mode: 'no-cors'}); return 'loaded'; } catch (e) { return 'blocked'; } }", vec![])
        .await
        .unwrap();
    assert_eq!(r, "blocked");
    b.stop().await;
    let _ = std::fs::remove_dir_all(&dir);
}
