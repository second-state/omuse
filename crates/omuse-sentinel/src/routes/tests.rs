//! Route-level tests (axum Router + tower oneshot): auth guards, the CSRF header, the origin check, the proxy
//! pass-through (incl. SSE streaming and 503), health, approvals.

use std::sync::Arc;
use std::time::Duration;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::routing::{get, post, put};
use axum::Router;
use futures_util::StreamExt;
use http_body_util::BodyExt;
use omuse_common::Config;
use serde_json::{json, Value};
use tower::ServiceExt;

use super::{app, router, App, Ctx};

const TOKEN: &str = "rt-test-token";

/// A fake runtime on a random port: echo, chat, SSE.
async fn fake_runtime() -> String {
    let r = Router::new()
        .route(
            "/api/echo",
            get(|req: axum::extract::Request| async move {
                let h = req.headers();
                let rt = h.get("x-persona-runtime").is_some();
                axum::Json(json!({"query": req.uri().query(), "ui": h.get("x-persona-ui").and_then(|v| v.to_str().ok()), "runtime_header": rt}))
            })
            .post(|body: axum::body::Bytes| async move { ([("x-custom", "1")], body) }),
        )
        .route("/api/chat", post(|| async { axum::Json(json!({"task_id": "t_chat_1"})) }))
        .route(
            "/api/run_pending_tasks",
            get(|| async { "{\"event\": \"pending\", \"count\": 0}\n{\"event\": \"done\"}\n" }),
        )
        .route(
            "/api/length",
            put(|h: axum::http::HeaderMap, body: axum::body::Bytes| async move {
                let cl = h.get("content-length").and_then(|v| v.to_str().ok()).map(str::to_string);
                axum::Json(json!({"content_length": cl, "body": String::from_utf8_lossy(&body)}))
            }),
        )
        .route(
            "/api/stream",
            get(|| async {
                let s = async_stream::stream! {
                    yield Ok::<_, std::io::Error>(bytes::Bytes::from_static(b"data: one\n\n"));
                    tokio::time::sleep(Duration::from_millis(1500)).await;
                    yield Ok(bytes::Bytes::from_static(b"data: two\n\n"));
                };
                ([("content-type", "text/event-stream")], Body::from_stream(s))
            }),
        );
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(l, r).await.unwrap() });
    format!("http://{addr}")
}

fn ctx_with(runtime_url: &str, auth: bool) -> (App, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let toml = format!(
        "[data]\ndir = \"{}\"\n[internal]\nruntime_url = \"{runtime_url}\"\nruntime_token = \"{TOKEN}\"\nbrowser_token = \"bt\"\n\
         browser_url = \"http://127.0.0.1:9\"\n[auth]\nenabled = {auth}\npassword = \"secret-pass\"\n[telegram]\nenabled = false\n",
        dir.path().display()
    );
    let cfg = Arc::new(Config::parse(&toml).unwrap());
    std::fs::create_dir_all(cfg.sentinel_data().join("shots")).unwrap();
    (Arc::new(Ctx::new(cfg).unwrap()), dir)
}

async fn send(r: &Router, req: Request<Body>) -> (StatusCode, Value) {
    let resp = r.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

fn req(method: &str, uri: &str) -> axum::http::request::Builder {
    Request::builder().method(method).uri(uri)
}

#[tokio::test]
async fn health_is_open() {
    let (ctx, _d) = ctx_with("http://127.0.0.1:9", false);
    let (s, v) = send(
        &router(ctx),
        req("GET", "/sentinel/api/health")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(v["ok"], json!(true));
    assert_eq!(v["version"], json!(omuse_common::util::VERSION.as_str()));
}

#[tokio::test]
async fn internal_needs_runtime_token() {
    let (ctx, _d) = ctx_with("http://127.0.0.1:9", false);
    let r = router(ctx);
    let (s, v) = send(
        &r,
        req("GET", "/internal/public_base")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);
    assert_eq!(v["detail"], json!("runtime token required"));
    let (s, _) = send(
        &r,
        req("GET", "/internal/public_base")
            .header("X-Persona-Runtime", "wrong")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);
    let (s, v) = send(
        &r,
        req("GET", "/internal/public_base")
            .header("X-Persona-Runtime", TOKEN)
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(v, json!({"base": ""}));
}

#[tokio::test]
async fn ui_header_required_for_writes() {
    let (ctx, _d) = ctx_with("http://127.0.0.1:9", false);
    let r = router(ctx);
    let (s, v) = send(
        &r,
        req("POST", "/sentinel/api/approvals/x/resolve")
            .header("content-type", "application/json")
            .body(Body::from("{}"))
            .unwrap(),
    )
    .await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    assert_eq!(v["detail"], json!("UI header required"));
    // reads need no header
    let (s, v) = send(
        &r,
        req("GET", "/sentinel/api/approvals?status=pending")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(v, json!({"approvals": []}));
    // with the header: the approval does not exist
    let (s, v) = send(
        &r,
        req("POST", "/sentinel/api/approvals/x/resolve")
            .header("X-Persona-UI", "1")
            .body(Body::from(r#"{"decision":"approve"}"#))
            .unwrap(),
    )
    .await;
    assert_eq!(s, StatusCode::NOT_FOUND);
    assert_eq!(v["detail"], json!("approval not found"));
}

#[tokio::test]
async fn origin_must_match_host() {
    let (ctx, _d) = ctx_with("http://127.0.0.1:9", false);
    let r = router(ctx);
    let mk = |origin: &str, host: &str| {
        req("DELETE", "/sentinel/api/grants/g1")
            .header("X-Persona-UI", "1")
            .header("Origin", origin)
            .header("Host", host)
            .body(Body::empty())
            .unwrap()
    };
    let (s, v) = send(&r, mk("https://evil.example", "omuse.local:8080")).await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    assert_eq!(v["detail"], json!("cross-origin request rejected"));
    let (s, v) = send(&r, mk("http://omuse.local:8080", "omuse.local:8080")).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(v, json!({"ok": true}));
    // X-Forwarded-Host wins, first value only
    let fwd = req("DELETE", "/sentinel/api/grants/g1")
        .header("X-Persona-UI", "1")
        .header("Origin", "https://box.example.com")
        .header("Host", "127.0.0.1:8080")
        .header("X-Forwarded-Host", "box.example.com, proxy.internal")
        .body(Body::empty())
        .unwrap();
    assert_eq!(send(&r, fwd).await.0, StatusCode::OK);
}

#[tokio::test]
async fn act_unknown_tool_is_denied() {
    let (ctx, _d) = ctx_with("http://127.0.0.1:9", false);
    let r = router(ctx.clone());
    let (s, v) = send(
        &r,
        req("POST", "/internal/act")
            .header("X-Persona-Runtime", TOKEN)
            .body(Body::from(r#"{"tool":"nope","task_id":"t1"}"#))
            .unwrap(),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(
        v,
        json!({"status": "denied", "reason": "未知工具 unknown tool nope"})
    );
    let ev = ctx.store.audit_list(Some("t1"), 10, None, None).unwrap();
    assert_eq!(ev[0]["action"], json!("nope"));
    assert_eq!(ev[0]["result"], json!("denied"));
}

#[tokio::test]
async fn proxy_passes_through() {
    let rt = fake_runtime().await;
    let (ctx, _d) = ctx_with(&rt, false);
    let r = router(ctx.clone());
    let (s, v) = send(
        &r,
        req("GET", "/api/echo?a=1&b=x")
            .header("X-Persona-UI", "1")
            .header("X-Persona-Runtime", "leak")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(v["query"], json!("a=1&b=x"));
    assert_eq!(v["ui"], json!("1"));
    assert_eq!(
        v["runtime_header"],
        json!(false),
        "the runtime token header is never forwarded"
    );
    // writes need the UI header
    let (s, v) = send(&r, req("POST", "/api/echo").body(Body::from("x")).unwrap()).await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    assert_eq!(v["detail"], json!("UI header required"));
    // streamed upload, response headers kept
    let resp = r
        .clone()
        .oneshot(
            req("POST", "/api/echo")
                .header("X-Persona-UI", "1")
                .body(Body::from("hello body"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.headers().get("x-custom").unwrap(), "1");
    assert_eq!(
        &resp.into_body().collect().await.unwrap().to_bytes()[..],
        b"hello body"
    );
    // a streamed body keeps its length: the runtime reads a body only when Content-Length says there is one
    let (s, v) = send(
        &r,
        req("PUT", "/api/length")
            .header("X-Persona-UI", "1")
            .header("content-length", "15")
            .body(Body::from(r#"{"only":["G1"]}"#))
            .unwrap(),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(
        v,
        json!({"content_length": "15", "body": r#"{"only":["G1"]}"#})
    );
    // chat records the user's words for the task
    let (s, v) = send(
        &r,
        req("POST", "/api/chat")
            .header("X-Persona-UI", "1")
            .body(Body::from(r#"{"message":"buy the blue one"}"#))
            .unwrap(),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(v["task_id"], json!("t_chat_1"));
    assert_eq!(
        ctx.store.user_request("t_chat_1").unwrap(),
        "buy the blue one"
    );
}

#[tokio::test]
async fn run_pending_tasks_reaches_the_runtime() {
    let rt = fake_runtime().await;
    let (ctx, _d) = ctx_with(&rt, false);
    let resp = router(ctx)
        .oneshot(
            req("GET", "/run_pending_tasks")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    assert!(String::from_utf8_lossy(&body).ends_with("{\"event\": \"done\"}\n"));
}

#[tokio::test]
async fn proxy_streams_sse_without_buffering() {
    let rt = fake_runtime().await;
    let (ctx, _d) = ctx_with(&rt, false);
    let resp = router(ctx)
        .oneshot(req("GET", "/api/stream").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(
        resp.headers().get("content-type").unwrap(),
        "text/event-stream"
    );
    let mut s = resp.into_body().into_data_stream();
    let t0 = std::time::Instant::now();
    let first = s.next().await.unwrap().unwrap();
    assert_eq!(&first[..], b"data: one\n\n");
    assert!(
        t0.elapsed() < Duration::from_millis(1000),
        "first event must not wait for the stream to end"
    );
    let second = s.next().await.unwrap().unwrap();
    assert_eq!(&second[..], b"data: two\n\n");
}

#[tokio::test]
async fn proxy_runtime_down_is_503() {
    let (ctx, _d) = ctx_with("http://127.0.0.1:9", false);
    let (s, v) = send(
        &router(ctx),
        req("GET", "/api/tasks").body(Body::empty()).unwrap(),
    )
    .await;
    assert_eq!(s, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(
        v["detail"],
        json!("Agent Runtime 暂不可用 (runtime unavailable)")
    );
}

#[tokio::test]
async fn index_learns_public_base_and_static_serves() {
    let (ctx, _d) = ctx_with("http://127.0.0.1:9", false);
    let r = router(ctx.clone());
    let resp = r
        .clone()
        .oneshot(
            req("GET", "/")
                .header("Host", "box.example.com")
                .header("X-Forwarded-Proto", "https")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(resp.headers().get("cache-control").unwrap(), "no-cache");
    assert_eq!(ctx.kv_str("public_base"), "https://box.example.com");
    // loopback hosts are not remembered
    let _ = r
        .clone()
        .oneshot(
            req("GET", "/")
                .header("Host", "127.0.0.1:8080")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(ctx.kv_str("public_base"), "https://box.example.com");
    let name = ctx
        .assets
        .names()
        .into_iter()
        .next()
        .expect("at least one asset");
    let resp = r
        .oneshot(
            req("GET", &format!("/static/{name}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
}

#[tokio::test]
async fn delete_credential_on_shadowed_route() {
    let (ctx, _d) = ctx_with("http://127.0.0.1:9", false);
    let r = router(ctx);
    for name in ["notion", "gmail", "calendar"] {
        let (s, v) = send(
            &r,
            req(
                "DELETE",
                &format!("/sentinel/api/connections/{name}/credential"),
            )
            .header("X-Persona-UI", "1")
            .body(Body::empty())
            .unwrap(),
        )
        .await;
        assert_eq!(s, StatusCode::OK, "{name}");
        assert_eq!(v, json!({"ok": true}));
    }
}

#[tokio::test]
async fn each_google_account_is_listed_made_default_and_disconnected_on_its_own() {
    let (ctx, _d) = ctx_with("http://127.0.0.1:9", false);
    for (email, tok) in [("me@example.com", "r1"), ("work@example.com", "r2")] {
        omuse_apps::gaccounts::save_account(
            &ctx.store,
            "calendar",
            email,
            &json!({"refresh_token": tok}),
            &json!({"mode": "google", "time_zone": "Asia/Singapore"}),
        )
        .unwrap();
    }
    let r = router(ctx.clone());
    let ui = |m: &str, u: &str, body: Value| {
        req(m, u)
            .header("X-Persona-UI", "1")
            .header("content-type", "application/json")
            .body(Body::from(body.to_string()))
            .unwrap()
    };
    let (s, v) = send(
        &r,
        ui(
            "POST",
            "/sentinel/api/connections/calendar/default",
            json!({"account": "c2"}),
        ),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(v["default"], "c2");
    let emails: Vec<&str> = v["accounts"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|a| a["email"].as_str())
        .collect();
    assert_eq!(emails, ["me@example.com", "work@example.com"]);
    let (s, v) = send(
        &r,
        ui(
            "DELETE",
            "/sentinel/api/connections/calendar/accounts/c2",
            json!({}),
        ),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(
        (
            v["accounts"].as_array().unwrap().len(),
            v["default"].clone()
        ),
        (1, json!("c1"))
    );
    assert!(ctx.store.get_secret("cred_calendar_2").unwrap().is_none());
    // an unknown service or account is a 404, gmail keeps its own routes
    let (s, _) = send(
        &r,
        ui(
            "POST",
            "/sentinel/api/connections/notion/default",
            json!({"account": "c1"}),
        ),
    )
    .await;
    assert_eq!(s, StatusCode::NOT_FOUND);
    let (s, _) = send(
        &r,
        ui(
            "POST",
            "/sentinel/api/connections/calendar/default",
            json!({"account": "c9"}),
        ),
    )
    .await;
    assert_eq!(s, StatusCode::NOT_FOUND);
    // the picker is not configured in this build and no drive is connected: said, not an error
    let (s, v) = send(
        &r,
        req("GET", "/sentinel/api/connections/drive/picker")
            .header("X-Persona-UI", "1")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!((s, v["ok"].clone()), (StatusCode::OK, json!(false)), "{v}");
}

#[tokio::test]
async fn login_gate_wraps_ui_but_not_health_or_internal() {
    let (ctx, _d) = ctx_with("http://127.0.0.1:9", true);
    let r = app(ctx).unwrap();
    let (s, _) = send(
        &r,
        req("GET", "/sentinel/api/health")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    let (s, _) = send(
        &r,
        req("GET", "/internal/public_base")
            .header("X-Persona-Runtime", TOKEN)
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    let resp = r
        .clone()
        .oneshot(
            req("GET", "/sentinel/api/approvals")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    // HTTP Basic with the configured password passes
    use base64::Engine;
    let basic = base64::engine::general_purpose::STANDARD.encode("omuse:secret-pass");
    let (s, _) = send(
        &r,
        req("GET", "/sentinel/api/approvals")
            .header("Authorization", format!("Basic {basic}"))
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
}

#[tokio::test]
async fn password_status_and_change() {
    let (ctx, _d) = ctx_with("http://127.0.0.1:9", true);
    let r = router(ctx);
    let (s, v) = send(
        &r,
        req("GET", "/sentinel/api/password")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(
        v,
        json!({"enabled": true, "default": true, "user": "omuse", "min_length": 8})
    );
    let (s, v) = send(
        &r,
        req("POST", "/sentinel/api/password")
            .header("X-Persona-UI", "1")
            .body(Body::from(r#"{"current":"secret-pass","new":"short"}"#))
            .unwrap(),
    )
    .await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "{v}");
    let (s, _) = send(
        &r,
        req("POST", "/sentinel/api/password")
            .header("X-Persona-UI", "1")
            .body(Body::from(
                r#"{"current":"secret-pass","new":"a-much-longer-one"}"#,
            ))
            .unwrap(),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    let (_, v) = send(
        &r,
        req("GET", "/sentinel/api/password")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(v["default"], json!(false));
}

#[tokio::test]
async fn unknown_paths_are_404() {
    let (ctx, _d) = ctx_with("http://127.0.0.1:9", false);
    let (s, v) = send(
        &router(ctx),
        req("GET", "/nothing/here").body(Body::empty()).unwrap(),
    )
    .await;
    assert_eq!(s, StatusCode::NOT_FOUND);
    assert_eq!(v["detail"], json!("Not Found"));
}

#[test]
fn action_error_status_mapping() {
    use omuse_sentinel_core::errors::ActionError;
    assert_eq!(
        super::action_error(&ActionError::with_status("x", "paused")).status,
        423
    );
    assert_eq!(
        super::action_error(&ActionError::new("该网站已被拦截")).status,
        403
    );
    assert_eq!(
        super::action_error(&ActionError::new("domain blocked")).status,
        403
    );
    assert_eq!(super::action_error(&ActionError::new("other")).status, 400);
}

#[test]
fn ledger_summary_warns_without_hits() {
    let mut s = json!({"fields": [], "warning": ""});
    super::act::ledger_summary(&mut s, &[], Some(&json!({"name": "Cancel order"})));
    assert_eq!(s["ledger_click"], json!("Cancel order"));
    assert!(s["warning"]
        .as_str()
        .unwrap()
        .starts_with("这个页面上的订单号不在 OMuse 的交易账本里"));
}

#[test]
fn page_id_gives_sub_agent_lanes_their_own_browser_page() {
    use super::act::page_id;
    assert_eq!(page_id("task_1", ""), "task_1");
    assert_eq!(page_id("task_1", "sub2"), "task_1~sub2");
    assert_eq!(page_id("task_1", "../x y!"), "task_1~xy");
    assert_eq!(
        page_id("t", &"a".repeat(40)),
        format!("t~{}", "a".repeat(16))
    );
}

#[tokio::test]
async fn a_keyless_places_search_in_a_tainted_task_asks_like_the_map_page_load() {
    // browser_auto_real 2026-10-31: without a SerpApi key places_search read the Google Maps page with the address
    // from the user's mail, past the egress check a browser_navigate to the same page stops at
    let (ctx, _d) = ctx_with("http://127.0.0.1:9", false);
    let r = router(ctx.clone());
    ctx.store
        .update_task_ctx("t1", Some("CONFIDENTIAL"), None, None)
        .unwrap();
    let body = json!({"tool": "places_search", "task_id": "t1", "call_id": "c1",
                      "args": {"query": "Hotel Ginza Grand", "area": "Ginza", "city": "Tokyo"}});
    let (s, v) = send(
        &r,
        req("POST", "/internal/act")
            .header("X-Persona-Runtime", TOKEN)
            .body(Body::from(body.to_string()))
            .unwrap(),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    // auto-approve is on by default: the page load is approved at once, shown as "Open page · auto-approved"
    assert_eq!(v["auto_approved"], json!(true), "{v}");
    assert!(
        v["summary"]["title"]
            .as_str()
            .unwrap_or("")
            .contains("Open page"),
        "{v}"
    );
    let ap = ctx
        .store
        .approval(v["approval_id"].as_str().unwrap())
        .unwrap()
        .unwrap();
    assert_eq!(ap["tool"], json!("browser_navigate"), "{ap}");
    assert!(ap.to_string().contains("google.com/maps"), "{ap}");
}

/// A fake browser broker for one checkout page: what a ref is, the page text with its total, and the typing.
async fn fake_checkout_broker() -> String {
    const URL: &str = "https://shop.example/book/restaurant/R-TYO-2";
    let r = Router::new()
        .route(
            "/agent/describe",
            post(|| async {
                axum::Json(json!({"tag": "input", "name": "Card number", "input_type": "text",
                                  "page_url": URL, "page_title": "Kappo Yamabuki — booking"}))
            }),
        )
        .route("/state", get(|| async { axum::Json(json!({"url": URL, "tasks": []})) }))
        .route(
            "/agent/page_text",
            post(|| async { axum::Json(json!({"text": "Kappo Yamabuki\nDeposit\nTotal: JPY 28,000\nConfirm booking"})) }),
        )
        .route(
            "/agent/type",
            post(|| async { axum::Json(json!({"url": URL, "title": "Kappo Yamabuki — booking", "snapshot": "[e5] textbox \"Card number\""})) }),
        );
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(l, r).await.unwrap() });
    format!("http://{addr}")
}

#[tokio::test]
async fn a_checkout_pages_text_never_sets_the_amount_it_is_shown_next_to_the_models() {
    // the card fill before a purchase was confirmed used to make the confirmation in code with the page text's total
    // (`page_total` over an arbitrary checkout's prose): now the fill is refused until purchase_confirm, whose total
    // is the model's, and the page's own figure is shown beside it on the approval card
    let broker = fake_checkout_broker().await;
    let dir = tempfile::tempdir().unwrap();
    let toml = format!(
        "[data]\ndir = \"{}\"\n[internal]\nruntime_url = \"http://127.0.0.1:9\"\nruntime_token = \"{TOKEN}\"\nbrowser_token = \"bt\"\n\
         browser_url = \"{broker}\"\n[auth]\nenabled = false\npassword = \"secret-pass\"\n[telegram]\nenabled = false\n",
        dir.path().display()
    );
    let cfg = Arc::new(Config::parse(&toml).unwrap());
    std::fs::create_dir_all(cfg.sentinel_data().join("shots")).unwrap();
    let ctx: App = Arc::new(Ctx::new(cfg).unwrap());
    let card = omuse_sentinel_core::vault::save_item(
        &ctx.store,
        &json!({"kind": "card", "label": "Visa", "domains": "",
                "values": {"number": "4111 1111 1111 1111", "expiry": "12/29", "cvc": "123", "holder": "LUCAS LU"}}),
        None,
    )
    .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();
    let act = |body: Value| {
        let ctx = ctx.clone();
        async move {
            send(
                &router(ctx),
                req("POST", "/internal/act")
                    .header("X-Persona-Runtime", TOKEN)
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
        }
    };
    let (s, v) = act(
        json!({"tool": "browser_fill_secret", "task_id": "t1", "call_id": "c1",
                             "args": {"ref": "e5", "item_id": card, "field": "number"}}),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(v["status"], "denied", "{v}");
    assert!(v.to_string().contains("purchase_confirm"), "{v}");
    assert!(ctx
        .store
        .active_purchase("t1", "shop.example")
        .unwrap()
        .is_none());
    // the model's purchase_confirm (it read JPY 25,000; the page text says 28,000; a `_page_shows` it sends is dropped)
    let (_, v) = act(json!({"tool": "purchase_confirm", "task_id": "t1", "call_id": "c2",
                             "args": {"site": "shop.example", "items": [{"name": "Deposit", "qty": 1, "price": 25000}],
                                      "total": 25000, "currency": "JPY", "delivery": "Table for 2", "card_item_id": card,
                                      "_page_shows": 1}}))
    .await;
    assert_eq!(v["status"], "approval_required", "{v}");
    let summ = v["summary"].to_string();
    assert!(summ.contains("JPY 25000.00"), "{summ}");
    assert!(
        summ.contains("Page shows") && summ.contains("JPY 28000.00"),
        "{summ}"
    );
    assert!(
        v["summary"]["warning"]
            .as_str()
            .unwrap_or("")
            .contains("JPY 28000.00"),
        "{summ}"
    );
    let ap = ctx
        .store
        .approval(v["approval_id"].as_str().unwrap())
        .unwrap()
        .unwrap();
    assert_eq!(ap["args"]["total"], json!(25000), "{ap}");
    // the same total as the page shows: the auto-approval setting applies as before
    let (_, v) = act(json!({"tool": "purchase_confirm", "task_id": "t1", "call_id": "c3",
                             "args": {"site": "shop.example", "items": [{"name": "Deposit", "qty": 1, "price": 28000}],
                                      "total": 28000, "currency": "JPY", "delivery": "Table for 2", "card_item_id": card}}))
    .await;
    assert_eq!(v["auto_approved"], true, "{v}");
    let p = ctx
        .store
        .active_purchase("t1", "shop.example")
        .unwrap()
        .expect("a confirmed purchase");
    assert!(p.to_string().contains("28000"), "{p}");
}

#[tokio::test]
async fn artifacts_are_served_to_the_signed_in_user_only() {
    let rt = fake_runtime().await;
    let (ctx, _d) = ctx_with(&rt, true);
    let dir = ctx.cfg.workspace().join("artifacts/news/batteries");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("2026-11-02-0800.html"),
        "<h1>The Battery Weekly</h1>",
    )
    .unwrap();
    std::fs::write(dir.join("2026-11-02-0800.pdf"), b"%PDF-1.7 x").unwrap();
    let r = router(ctx.clone());
    let get = |uri: &str| req("GET", uri).body(Body::empty()).unwrap();
    let resp = r
        .clone()
        .oneshot(get("/artifacts/news/batteries/2026-11-02-0800.html"))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(resp.headers()["content-type"], "text/html; charset=utf-8");
    assert!(resp.headers()["content-security-policy"]
        .to_str()
        .unwrap()
        .contains("sandbox"));
    let resp = r
        .clone()
        .oneshot(get("/artifacts/news/batteries/2026-11-02-0800.pdf"))
        .await
        .unwrap();
    assert_eq!(resp.headers()["content-type"], "application/pdf");
    let resp = r.clone().oneshot(get("/artifacts/")).await.unwrap();
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    assert!(
        String::from_utf8_lossy(&body).contains("/artifacts/news/batteries/2026-11-02-0800.html")
    );
    let resp = r
        .clone()
        .oneshot(get("/artifacts/../config.toml"))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    // behind the login gate: without a session or Basic credentials nothing is served
    let gated = app(ctx.clone()).unwrap();
    let resp = gated
        .clone()
        .oneshot(get("/artifacts/news/batteries/2026-11-02-0800.html"))
        .await
        .unwrap();
    assert_ne!(resp.status(), StatusCode::OK);
    let basic = format!(
        "Basic {}",
        base64::Engine::encode(
            &base64::engine::general_purpose::STANDARD,
            "omuse:secret-pass"
        )
    );
    let resp = gated
        .oneshot(
            req("GET", "/artifacts/news/batteries/2026-11-02-0800.html")
                .header("authorization", basic)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
}

/// The shop_order machine's `shop_search` is in the runtime's catalog without any connection (shopping_real
/// 2026-11-02: left out, the machine's call ran as "unknown tool" and every search said "nothing in stock").
#[tokio::test]
async fn catalog_has_the_shopping_search() {
    let (ctx, _d) = ctx_with("http://127.0.0.1:9", false);
    let (s, v) = send(
        &router(ctx),
        req("GET", "/internal/catalog")
            .header("X-Persona-Runtime", TOKEN)
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    let names: Vec<&str> = v["tools"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|t| t["function"]["name"].as_str())
        .collect();
    assert!(names.contains(&"shop_search"), "{names:?}");
}
