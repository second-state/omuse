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
fn dry_run_side_effects() {
    use super::act::dry_run_side_effect;
    assert!(dry_run_side_effect("purchase_confirm", &json!({})));
    assert!(!dry_run_side_effect(
        "phone_hangup",
        &json!({"capability": "call"})
    ));
    assert!(dry_run_side_effect(
        "mcp_x__y",
        &json!({"connector": "mcp:x", "capability": "write"})
    ));
    assert!(!dry_run_side_effect(
        "mcp_x__z",
        &json!({"connector": "mcp:x", "capability": "read"})
    ));
    assert!(dry_run_side_effect(
        "gmail_send",
        &json!({"connector": "gmail", "capability": "send"})
    ));
    assert!(!dry_run_side_effect(
        "gmail_search",
        &json!({"connector": "gmail", "capability": "read"})
    ));
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
