//! Live test against a local Chromium and the real web. Ignored by default: run with
//! `cargo test -p omuse-browser --test live -- --ignored --nocapture`. It skips itself when no Chromium is found.

use std::sync::Arc;

use omuse_common::Config;
use serde_json::{json, Value};

use omuse_browser::broker::Broker;
use omuse_browser::launch;

fn config(dir: &std::path::Path) -> Arc<Config> {
    let toml = format!(
        "[browser]\nheadless = \"1\"\nidle_shutdown = 0\n[data]\ndir = \"{}\"\n[internal]\nbrowser_token = \"bt-test\"\n[server]\nbrowser_port = 18082\n",
        dir.display()
    );
    Arc::new(Config::parse(&toml).unwrap())
}

async fn act(b: &Broker, action: &str, body: Value) -> Value {
    b.agent_action(action, &body)
        .await
        .unwrap_or_else(|e| panic!("{action} failed: {e}"))
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
        .eval(None, "async () => { try { await fetch('http://127.0.0.1:9/page.html', {mode: 'no-cors'}); return 'loaded'; } catch (e) { return 'blocked'; } }", vec![])
        .await
        .unwrap();
    assert_eq!(r, "blocked");
    b.stop().await;
    let _ = std::fs::remove_dir_all(&dir);
}

/// A secret's box shows only whether it holds a value: a filled card number reads `value=[hidden]` (never the
/// digits), an empty one `value=""` (shop_real 2026-11-02: Shopify's empty card boxes read as filled at the hand-over).
/// Needs internet access; skipped without Chromium or network.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore]
async fn an_empty_secret_box_reads_empty_and_a_filled_one_hidden() {
    let dir = std::env::temp_dir().join(format!("omuse-browser-secret-{}", std::process::id()));
    std::fs::create_dir_all(dir.join("workspace")).unwrap();
    let cfg = config(&dir);
    if launch::find_executable(&cfg).is_none()
        || reqwest::get("https://example.com/").await.is_err()
    {
        eprintln!("no chromium or no network: skipping");
        return;
    }
    let b = Broker::new(cfg);
    act(
        &b,
        "navigate",
        json!({"task_id": "s1", "url": "example.com"}),
    )
    .await;
    let page = b.page("s1").await.unwrap();
    page.eval(
        None,
        "() => { document.body.innerHTML = '<label>Card number <input autocomplete=\"cc-number\" id=\"a\"></label>' + \
         '<label>Security code <input autocomplete=\"cc-csc\" id=\"b\" value=\"123\"></label>'; return 1; }",
        vec![],
    )
    .await
    .unwrap();
    let snap = act(&b, "snapshot", json!({"task_id": "s1"})).await;
    let text = snap["snapshot"].as_str().unwrap_or_default();
    let line = |name: &str| {
        text.lines()
            .find(|l| l.trim_start().starts_with('[') && l.contains(name))
            .unwrap_or_default()
            .to_string()
    };
    assert!(line("Card number").contains("value=\"\""), "{text}");
    assert!(line("Security code").contains("value=[hidden]"), "{text}");
    assert!(!line("Security code").contains("123"), "{text}");
    b.stop().await;
    let _ = std::fs::remove_dir_all(&dir);
}
