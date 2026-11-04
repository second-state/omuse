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

/// A masked input sets its template and moves the caret on its own timer after the focus (FWD Hong Kong's HKID
/// "A______(_)" and mobile, insurance_hk_real 2026-11-03: the HKID came out empty and the mobile "1234 5679"): `keys`
/// waits for it and starts at its first slot. Skipped without Chromium or network.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore]
async fn keys_fill_a_masked_input_from_its_first_slot() {
    let dir = std::env::temp_dir().join(format!("omuse-browser-mask-{}", std::process::id()));
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
        json!({"task_id": "m1", "url": "example.com"}),
    )
    .await;
    let page = b.page("m1").await.unwrap();
    // the mask: on focus, 100 ms later, the template and the caret at its end; each digit fills the next slot
    page.eval(
        None,
        "() => { document.body.innerHTML = '<input id=\"m\">'; const el = document.getElementById('m');
           el.addEventListener('focus', () => setTimeout(() => { if (!el.value) el.value = '________';
             el.setSelectionRange(el.value.length, el.value.length); }, 100));
           el.addEventListener('keydown', (e) => { if (!/^[0-9]$/.test(e.key)) return; e.preventDefault();
             const i = el.value.indexOf('_', el.selectionStart); if (i < 0) return;
             el.value = el.value.slice(0, i) + e.key + el.value.slice(i + 1); el.setSelectionRange(i + 1, i + 1); });
           return 1; }",
        vec![],
    )
    .await
    .unwrap();
    let r = act(
        &b,
        "fill_form",
        json!({"task_id": "m1", "fills": [{"op": "keys", "sel": "#m", "value": "91234567", "field": "mobile"}]}),
    )
    .await;
    let v = page
        .eval(None, "() => document.getElementById('m').value", vec![])
        .await
        .unwrap();
    assert_eq!(v, "91234567", "{r}");
    b.stop().await;
    let _ = std::fs::remove_dir_all(&dir);
}

/// `only_if_failed`: a second try runs only when the op it names did not work (TIP Insure's Next takes a second press
/// right after its address lists, insurance_th_real 2026-11-03). Skipped without Chromium or network.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore]
async fn a_second_try_runs_only_after_its_op_failed() {
    let dir = std::env::temp_dir().join(format!("omuse-browser-again-{}", std::process::id()));
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
        json!({"task_id": "a1", "url": "example.com"}),
    )
    .await;
    let page = b.page("a1").await.unwrap();
    page.eval(
        None,
        "() => { window.presses = 0; document.body.innerHTML = '<button id=\"next\" onclick=\"window.presses++\">Next</button>'; return 1; }",
        vec![],
    )
    .await
    .unwrap();
    let r = act(
        &b,
        "fill_form",
        json!({"task_id": "a1", "fills": [
            {"op": "button", "sel": "button", "text": "Next", "field": "next"},
            // never there: this wait fails, so the second press runs
            {"op": "await", "sel": "#summary", "wait_ms": 500, "field": "summary"},
            {"op": "button", "sel": "button", "text": "Next", "field": "again", "only_if_failed": "summary"},
            // there: this wait works, so the third press is skipped
            {"op": "await", "sel": "#next", "wait_ms": 500, "field": "shown"},
            {"op": "button", "sel": "button", "text": "Next", "field": "third", "only_if_failed": "shown"}]}),
    )
    .await;
    let presses = page
        .eval(None, "() => window.presses", vec![])
        .await
        .unwrap();
    assert_eq!(presses, json!(2), "{r}");
    b.stop().await;
    let _ = std::fs::remove_dir_all(&dir);
}
