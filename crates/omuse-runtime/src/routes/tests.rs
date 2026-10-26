//! Route-level tests: the axum router driven with `tower::ServiceExt::oneshot`.

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::Router;
use http_body_util::BodyExt;
use omuse_common::Config;
use serde_json::{json, Value};
use tower::ServiceExt;

use super::files::relpath;
use super::*;

struct Env {
    _dir: tempfile::TempDir,
    state: Arc<State>,
    app: Router,
}

fn env() -> Env {
    let dir = tempfile::tempdir().unwrap();
    let toml = format!(
        "[data]\ndir = \"{}\"\n[model]\nurl = \"http://127.0.0.1:9/v1\"\nname = \"fake\"\n",
        dir.path().display()
    );
    let cfg = Arc::new(Config::parse(&toml).unwrap());
    let state = State::new(cfg).unwrap();
    // audits and other Sentinel calls answer at once
    state
        .rt
        .set_sentinel_hook(Some(Arc::new(|_, _, _| Box::pin(async { Ok(json!({})) }))));
    let app = router(state.clone());
    Env {
        _dir: dir,
        state,
        app,
    }
}

async fn call(
    app: &Router,
    method: &str,
    uri: &str,
    body: Option<Value>,
    headers: &[(&str, &str)],
) -> (StatusCode, Value) {
    let (st, _, bytes) = raw(app, method, uri, body, headers).await;
    let v = serde_json::from_slice(&bytes)
        .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&bytes).into()));
    (st, v)
}

async fn raw(
    app: &Router,
    method: &str,
    uri: &str,
    body: Option<Value>,
    headers: &[(&str, &str)],
) -> (StatusCode, axum::http::HeaderMap, Vec<u8>) {
    let mut req = Request::builder().method(method).uri(uri);
    for (k, v) in headers {
        req = req.header(*k, *v);
    }
    let req = match body {
        Some(b) => req
            .header("content-type", "application/json")
            .body(Body::from(b.to_string()))
            .unwrap(),
        None => req.body(Body::empty()).unwrap(),
    };
    let resp = app.clone().oneshot(req).await.unwrap();
    let st = resp.status();
    let h = resp.headers().clone();
    let bytes = resp
        .into_body()
        .collect()
        .await
        .unwrap()
        .to_bytes()
        .to_vec();
    (st, h, bytes)
}

#[tokio::test]
async fn health_and_fastapi_style_errors() {
    let e = env();
    let (st, v) = call(&e.app, "GET", "/api/health", None, &[]).await;
    assert_eq!(st, 200);
    assert_eq!(v["ok"], true);
    let (st, v) = call(&e.app, "GET", "/api/nope", None, &[]).await;
    assert_eq!((st.as_u16(), v), (404, json!({"detail": "Not Found"})));
    let (st, v) = call(&e.app, "GET", "/api/tasks/task_x", None, &[]).await;
    assert_eq!((st.as_u16(), v), (404, json!({"detail": "Not Found"})));
    let (st, v) = call(&e.app, "POST", "/api/outcome/task_x", None, &[]).await;
    assert_eq!((st.as_u16(), v), (404, json!({"detail": "no such task"})));
    let (st, v) = call(&e.app, "GET", "/api/tasks?limit=abc", None, &[]).await;
    assert_eq!(st, 422);
    assert_eq!(v["detail"][0]["loc"], json!(["query", "limit"]));
    let (st, v) = call(
        &e.app,
        "POST",
        "/api/chat",
        Some(json!({"message": "  "})),
        &[],
    )
    .await;
    assert_eq!(
        (st.as_u16(), v),
        (400, json!({"detail": "message is empty"}))
    );
    let (st, v) = call(
        &e.app,
        "POST",
        "/api/chat",
        Some(json!({"message": "x", "attachments": ["uploads/../../etc/passwd"]})),
        &[],
    )
    .await;
    assert_eq!(st, 400);
    assert!(v["detail"]
        .as_str()
        .unwrap()
        .starts_with("附件不存在 attachment not found: "));
    let (st, v) = call(&e.app, "PUT", "/api/schedules/sch_x", Some(json!({})), &[]).await;
    assert_eq!((st.as_u16(), v), (404, json!({"detail": "Not Found"})));
    let (st, v) = call(
        &e.app,
        "POST",
        "/api/schedules",
        Some(json!({"goal": "g", "spec": "0 8 * * *"})),
        &[],
    )
    .await;
    assert_eq!((st.as_u16(), v), (400, json!({"detail": "'name'"})));
    let (st, _) = call(&e.app, "DELETE", "/api/skills/import", None, &[]).await;
    assert_eq!(st, 404);
}

#[tokio::test]
async fn settings_model_override_rules() {
    let e = env();
    let (_, g) = call(&e.app, "GET", "/api/settings", None, &[]).await;
    assert!(g["settings"].get("model_api_key").is_none());
    assert_eq!(g["settings"]["model_api_key_set"], false);
    assert_eq!(g["system_model"]["model_name"], "fake");
    assert!(g["system_model"].get("model_base_url").is_none());
    assert_eq!(
        g["model_user"],
        json!({"model_base_url": "", "model_name": "", "planner_model": "", "vision_model": "", "stt_model": "",
               "image_model": "", "image_base_url": "", "video_model": "", "video_base_url": "",
               "fast_mode": false, "reasoning_effort": "", "api_key_set": false,
               "image_api_key_set": false, "video_api_key_set": false})
    );
    assert_eq!(
        (
            &g["system_model"]["fast_mode"],
            &g["system_model"]["reasoning_effort"]
        ),
        (&json!(false), &json!(""))
    );
    // fast mode and reasoning can't be changed for the system's models: ignored
    let (st, v) = call(
        &e.app,
        "PUT",
        "/api/settings",
        Some(json!({"fast_mode": true, "reasoning_effort": "high"})),
        &[],
    )
    .await;
    assert_eq!(st, 200);
    assert_eq!(
        (
            &v["settings"]["fast_mode"],
            &v["settings"]["reasoning_effort"]
        ),
        (&json!(false), &json!(""))
    );
    assert_eq!(e.state.rt.store.user_model().unwrap()["fast_mode"], false);
    let sys = "http://127.0.0.1:9/v1";
    for (body, want) in [
        (
            json!({"model_override": true}),
            "接口地址必须是以 http:// 或 https:// 开头的网址 (the endpoint must be an http(s) URL)",
        ),
        (
            json!({"model_override": true, "model_base_url": sys, "model_name": "fake"}),
            "必须填写 API key (an API key is required to use your own model)",
        ),
        (
            json!({"model_override": true, "model_base_url": sys, "model_api_key": "k"}),
            "必须填写执行任务的 LLM 模型名 (the LLM model name is required)",
        ),
        (
            json!({"model_override": true, "model_base_url": "router/v1", "model_api_key": "k", "model_name": "fake"}),
            "接口地址必须是以 http:// 或 https:// 开头的网址 (the endpoint must be an http(s) URL)",
        ),
    ] {
        let (st, v) = call(&e.app, "PUT", "/api/settings", Some(body), &[]).await;
        assert_eq!((st.as_u16(), v), (400, json!({"detail": want})));
    }
    let (_, g) = call(&e.app, "GET", "/api/settings", None, &[]).await;
    assert_eq!(g["settings"]["model_override"], false);

    let (st, v) = call(
        &e.app,
        "PUT",
        "/api/settings",
        Some(json!({"model_override": true, "model_base_url": format!("{}/", sys), "model_api_key": " user-key-1 ",
                    "model_name": " missing-model ", "planner_model": " fake "})),
        &[],
    )
    .await;
    assert_eq!(st, 200);
    let s = &v["settings"];
    assert_eq!(
        (s["model_override"].clone(), s["model_name"].clone()),
        (json!(true), json!("missing-model"))
    );
    assert_eq!(
        (s["model_base_url"].clone(), s["planner_model"].clone()),
        (json!(sys), json!("fake"))
    );
    assert_eq!(s["model_api_key_set"], true);
    assert!(s.get("model_api_key").is_none());
    assert_eq!(
        e.state.rt.store.saved_settings().unwrap()["model_api_key"],
        "user-key-1"
    );

    // an empty key field keeps the saved key; fields left out come from the user's saved values
    let (st, v) = call(
        &e.app,
        "PUT",
        "/api/settings",
        Some(json!({"model_override": true, "model_name": "fake", "model_api_key": ""})),
        &[],
    )
    .await;
    assert_eq!(st, 200);
    assert_eq!(v["settings"]["model_api_key_set"], true);
    assert_eq!(
        e.state.rt.store.saved_settings().unwrap()["model_api_key"],
        "user-key-1"
    );
    // with the user's own model they can: saved, normalised, validated
    let (st, v) = call(
        &e.app,
        "PUT",
        "/api/settings",
        Some(json!({"fast_mode": 1, "reasoning_effort": " Medium "})),
        &[],
    )
    .await;
    assert_eq!(st, 200);
    assert_eq!(
        (
            &v["settings"]["fast_mode"],
            &v["settings"]["reasoning_effort"]
        ),
        (&json!(true), &json!("medium"))
    );
    let (st, _) = call(
        &e.app,
        "PUT",
        "/api/settings",
        Some(json!({"reasoning_effort": "turbo"})),
        &[],
    )
    .await;
    assert_eq!(st, 400);
    let (_, g) = call(&e.app, "GET", "/api/settings", None, &[]).await;
    assert_eq!(g["model_user"]["api_key_set"], true);
    assert!(g["model_user"].get("model_api_key").is_none());

    // switched off: the system's values are back, the user's stay saved
    let (_, v) = call(
        &e.app,
        "PUT",
        "/api/settings",
        Some(json!({"model_override": 0})),
        &[],
    )
    .await;
    assert_eq!(v["settings"]["model_override"], false);
    assert_eq!(v["settings"]["model_name"], "fake");
    assert_eq!(v["settings"]["model_api_key_set"], false);
    assert_eq!(
        (
            &v["settings"]["fast_mode"],
            &v["settings"]["reasoning_effort"]
        ),
        (&json!(false), &json!(""))
    );
    assert_eq!(
        e.state.rt.store.user_model().unwrap()["reasoning_effort"],
        "medium"
    );

    let (st, v) = call(
        &e.app,
        "PUT",
        "/api/settings",
        Some(json!({"memory_consolidate_at": "25:00"})),
        &[],
    )
    .await;
    assert_eq!(
        (st.as_u16(), v),
        (
            400,
            json!({"detail": "整理时间格式应为 HH:MM (time must be HH:MM)"})
        )
    );
    let (st, _) = call(
        &e.app,
        "PUT",
        "/api/settings",
        Some(json!({"memory_consolidate_at": " 4:05 "})),
        &[],
    )
    .await;
    assert_eq!(st, 200);
}

#[tokio::test]
async fn settings_instance_name() {
    let e = env();
    for (given, want) in [
        (
            json!("  Jarvis   at\n home  "),
            "Jarvis at home".to_string(),
        ),
        (json!("x".repeat(60)), "x".repeat(40)),
        (json!("   "), "OMuse".to_string()),
        (Value::Null, "OMuse".to_string()),
        (json!("名字".repeat(30)), "名字".repeat(20)),
    ] {
        let (st, v) = call(
            &e.app,
            "PUT",
            "/api/settings",
            Some(json!({"instance_name": given})),
            &[],
        )
        .await;
        assert_eq!(st, 200);
        assert_eq!(v["settings"]["instance_name"], json!(want));
    }
}

#[test]
fn check_model_override_normalises() {
    let mut b = json!({"model_base_url": " https://x.example/v1// ", "model_name": " m ", "model_api_key": "",
                       "vision_model": null, "stt_model": " w "})
    .as_object()
    .cloned()
    .unwrap();
    check_model_override(&mut b, &json!({"model_api_key": "saved"})).unwrap();
    assert_eq!(b["model_base_url"], "https://x.example/v1");
    assert_eq!(b["model_name"], "m");
    assert!(!b.contains_key("model_api_key"));
    assert_eq!(
        (b["vision_model"].clone(), b["stt_model"].clone()),
        (json!(""), json!("w"))
    );
    let mut b = json!({}).as_object().cloned().unwrap();
    let saved = json!({"model_base_url": "http://a/v1", "model_name": "n", "model_api_key": ""});
    assert_eq!(
        check_model_override(&mut b, &saved).unwrap_err().status,
        400
    );
}

#[tokio::test]
async fn internal_routes_need_the_runtime_token() {
    let e = env();
    let body = json!({"title": "Hello", "body": "World"});
    let (st, v) = call(
        &e.app,
        "POST",
        "/internal/notify_user",
        Some(body.clone()),
        &[],
    )
    .await;
    assert_eq!((st.as_u16(), v), (401, json!({"detail": "unauthorized"})));
    let (st, _) = call(
        &e.app,
        "POST",
        "/internal/notify_user",
        Some(body.clone()),
        &[("X-Persona-Runtime", "wrong")],
    )
    .await;
    assert_eq!(st, 401);
    for p in ["/internal/approval_resolved", "/internal/takeover_ended"] {
        let (st, _) = call(&e.app, "POST", p, Some(json!({})), &[]).await;
        assert_eq!(st, 401);
    }
    let tok = e.state.cfg.runtime_token();
    let (st, v) = call(
        &e.app,
        "POST",
        "/internal/notify_user",
        Some(body),
        &[("X-Persona-Runtime", &tok)],
    )
    .await;
    assert_eq!((st.as_u16(), v), (200, json!({"ok": true})));
    let (_, v) = call(&e.app, "GET", "/api/notifications", None, &[]).await;
    assert_eq!(v["notifications"][0]["title"], "Hello");
    assert_eq!(v["notifications"][0]["level"], "info");
    let (st, _) = call(
        &e.app,
        "POST",
        "/internal/approval_resolved",
        Some(json!({"task_id": "none"})),
        &[("X-Persona-Runtime", &tok)],
    )
    .await;
    assert_eq!(st, 200);
}

#[tokio::test]
async fn workspace_file_path_safety() {
    let e = env();
    let ws = e.state.rt.workspace.clone();
    std::fs::create_dir_all(ws.join("reports")).unwrap();
    std::fs::write(ws.join("reports/a b.txt"), "hello world").unwrap();
    std::fs::write(ws.join("page.html"), "<script>alert(1)</script>").unwrap();
    std::fs::write(ws.join(".hidden"), "x").unwrap();
    std::fs::create_dir_all(ws.join("x/.quarantine")).unwrap();
    std::fs::write(ws.join("x/.quarantine/bad.pdf"), "x").unwrap();
    std::fs::write(ws.parent().unwrap().join("secret.txt"), "secret").unwrap();

    let (st, v) = call(&e.app, "GET", "/api/files", None, &[]).await;
    assert_eq!(st, 200);
    assert_eq!(v["path"], ".");
    let names: Vec<&str> = v["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["page.html", "reports", "x"]);
    let (_, v) = call(&e.app, "GET", "/api/files?path=/reports", None, &[]).await;
    assert_eq!(v["items"][0]["path"], "reports/a b.txt");
    assert_eq!(v["items"][0]["size"], 11);
    let (st, v) = call(&e.app, "GET", "/api/files?path=../..", None, &[]).await;
    assert_eq!((st.as_u16(), v), (400, json!({"detail": "Bad Request"})));
    let (st, _) = call(&e.app, "GET", "/api/files?path=nothing", None, &[]).await;
    assert_eq!(st, 404);

    for p in [
        "../secret.txt",
        "/../secret.txt",
        "reports/../../secret.txt",
        "x/.quarantine/bad.pdf",
        "reports",
        "missing.txt",
    ] {
        let (st, v) = call(
            &e.app,
            "GET",
            &format!("/api/files/raw?path={}", p),
            None,
            &[],
        )
        .await;
        assert_eq!(
            (st.as_u16(), v),
            (404, json!({"detail": "Not Found"})),
            "{}",
            p
        );
    }
    let (st, _) = call(&e.app, "GET", "/api/files/raw", None, &[]).await;
    assert_eq!(st, 422);

    let (st, h, body) = raw(
        &e.app,
        "GET",
        "/api/files/raw?path=reports/a%20b.txt",
        None,
        &[],
    )
    .await;
    assert_eq!(st, 200);
    assert_eq!(body, b"hello world");
    assert_eq!(h["content-type"], "text/plain; charset=utf-8");
    assert_eq!(
        h["content-disposition"],
        "inline; filename*=utf-8''a%20b.txt"
    );
    assert_eq!(h["x-content-type-options"], "nosniff");
    let (_, h, _) = raw(
        &e.app,
        "GET",
        "/api/files/raw?path=reports/a%20b.txt&download=1",
        None,
        &[],
    )
    .await;
    assert!(h["content-disposition"]
        .to_str()
        .unwrap()
        .starts_with("attachment;"));
    let (_, h, _) = raw(&e.app, "GET", "/api/files/raw?path=page.html", None, &[]).await;
    assert_eq!(h["content-type"], "application/octet-stream");
    assert_eq!(
        h["content-disposition"],
        "attachment; filename=\"page.html\""
    );
    assert_eq!(h["content-security-policy"], "sandbox; default-src 'none'");
    let (st, h, body) = raw(
        &e.app,
        "GET",
        "/api/files/raw?path=reports/a%20b.txt",
        None,
        &[("Range", "bytes=6-")],
    )
    .await;
    assert_eq!(st, 206);
    assert_eq!(body, b"world");
    assert_eq!(h["content-range"], "bytes 6-10/11");
    let (st, _, _) = raw(
        &e.app,
        "GET",
        "/api/files/raw?path=reports/a%20b.txt",
        None,
        &[("Range", "bytes=50-")],
    )
    .await;
    assert_eq!(st, 416);
}

#[tokio::test]
async fn upload_and_chat_attachment() {
    let e = env();
    let req = Request::builder()
        .method("PUT")
        .uri("/api/upload?name=note.txt")
        .body(Body::from("hi there"))
        .unwrap();
    let resp = e.app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), 200);
    let info: Value =
        serde_json::from_slice(&resp.into_body().collect().await.unwrap().to_bytes()).unwrap();
    assert!(info["path"].as_str().unwrap().starts_with("uploads/"));
    assert_eq!(info["size"], 8);
    let req = Request::builder()
        .method("PUT")
        .uri("/api/upload?name=big.txt")
        .header(
            "content-length",
            (crate::attachments::UPLOAD_MAX + 1).to_string(),
        )
        .body(Body::empty())
        .unwrap();
    let resp = e.app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), 413);
    let v: Value =
        serde_json::from_slice(&resp.into_body().collect().await.unwrap().to_bytes()).unwrap();
    assert_eq!(v, json!({"detail": "文件超过 50 MB (file too large)"}));
    let (st, v) = call(&e.app, "PUT", "/api/upload?name=empty.txt", None, &[]).await;
    assert_eq!(
        (st.as_u16(), v),
        (400, json!({"detail": "文件是空的 (empty file)"}))
    );
}

#[tokio::test]
async fn sse_stream_frames() {
    let e = env();
    let req = Request::builder()
        .uri("/api/stream")
        .body(Body::empty())
        .unwrap();
    let resp = e.app.clone().oneshot(req).await.unwrap();
    assert_eq!(
        resp.headers()["content-type"],
        "text/event-stream; charset=utf-8"
    );
    assert_eq!(resp.headers()["x-accel-buffering"], "no");
    assert_eq!(resp.headers()["cache-control"], "no-cache");
    let mut body = resp.into_body();
    let first = body.frame().await.unwrap().unwrap().into_data().unwrap();
    assert_eq!(&first[..], b"retry: 3000\n\n");
    assert_eq!(e.state.hub.len(), 1);
    e.state
        .publish(json!({"kind": "conv_update", "conv_id": "c1", "ts": 1.5}));
    let ev = body.frame().await.unwrap().unwrap().into_data().unwrap();
    assert_eq!(
        std::str::from_utf8(&ev).unwrap(),
        "data: {\"kind\": \"conv_update\", \"conv_id\": \"c1\", \"ts\": 1.5}\n\n"
    );
    drop(body);
    assert_eq!(e.state.hub.len(), 0);
}

#[tokio::test]
async fn memory_profile_and_library_basics() {
    let e = env();
    let (st, v) = call(
        &e.app,
        "POST",
        "/api/memory",
        Some(json!({"fact": "my password is hunter2"})),
        &[],
    )
    .await;
    assert_eq!(st, 400);
    assert_eq!(
        v["detail"],
        "证件号、卡号和密码请放进保险箱 (ID / card numbers and passwords belong in the vault)"
    );
    let (st, v) = call(
        &e.app,
        "POST",
        "/api/memory",
        Some(json!({"fact": "  "})),
        &[],
    )
    .await;
    assert_eq!((st.as_u16(), v), (400, json!({"detail": "empty fact"})));
    let (st, f) = call(
        &e.app,
        "POST",
        "/api/memory",
        Some(json!({"fact": "Likes window seats", "tier": "recent"})),
        &[],
    )
    .await;
    assert_eq!(st, 200);
    assert_eq!(f["tier"], "recent");
    let (st, v) = call(&e.app, "GET", "/api/memory", None, &[]).await;
    assert_eq!(st, 200);
    assert_eq!(v["recent"][0]["fact"], "Likes window seats");
    assert_eq!(v["profile_fields"][0]["key"], "name_zh");
    assert_eq!(v["job"]["running"], false);
    let (st, v) = call(
        &e.app,
        "PUT",
        "/api/memory/fact_none",
        Some(json!({"fact": "x"})),
        &[],
    )
    .await;
    assert_eq!((st.as_u16(), v), (404, json!({"detail": "not found"})));
    let (st, v) = call(
        &e.app,
        "PUT",
        "/api/profile",
        Some(json!({"key": "shoe", "value": "42"})),
        &[],
    )
    .await;
    assert_eq!((st.as_u16(), v), (400, json!({"detail": "unknown field"})));
    let (st, v) = call(
        &e.app,
        "PUT",
        "/api/profile",
        Some(json!({"key": "phone", "value": "+65 9123 4567"})),
        &[],
    )
    .await;
    assert_eq!(st, 200);
    assert_eq!(v["profile"]["phone"], "+65 9123 4567");
    let (st, v) = call(
        &e.app,
        "POST",
        "/api/profile/pending/pp_x",
        Some(json!({"accept": true})),
        &[],
    )
    .await;
    assert_eq!(
        (st.as_u16(), v),
        (409, json!({"detail": "already resolved"}))
    );
    let (st, _) = call(&e.app, "GET", "/api/memory/runs/abc", None, &[]).await;
    assert_eq!(st, 422);
    let (st, v) = call(&e.app, "GET", "/api/memory/runs/7", None, &[]).await;
    assert_eq!((st.as_u16(), v), (404, json!({"detail": "not found"})));
    let (st, v) = call(
        &e.app,
        "POST",
        "/api/library",
        Some(json!({"title": " "})),
        &[],
    )
    .await;
    assert_eq!((st.as_u16(), v), (400, json!({"detail": "title required"})));
    let (st, v) = call(&e.app, "GET", "/api/library/search?q=", None, &[]).await;
    assert_eq!((st.as_u16(), v), (200, json!({"hits": []})));
    let (st, v) = call(&e.app, "GET", "/api/context/preview?q=seat", None, &[]).await;
    assert_eq!(st, 200);
    assert_eq!(v["mode"], "v1");
}

#[test]
fn relpath_like_python() {
    use std::path::Path;
    assert_eq!(relpath(Path::new("/w"), Path::new("/w")), ".");
    assert_eq!(relpath(Path::new("/w/a/b"), Path::new("/w")), "a/b");
    assert_eq!(relpath(Path::new("/w2/x"), Path::new("/w")), "../w2/x");
}
