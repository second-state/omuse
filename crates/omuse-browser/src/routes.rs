//! HTTP routes and the server entry point of the broker (`app/browser/main.py`: the FastAPI app).

use std::sync::Arc;
use std::time::Duration;

use axum::body::Bytes;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use omuse_common::api::{ApiError, ApiResult};
use omuse_common::util::{get_str, now_ts, token_ok, truthy};
use omuse_common::Config;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::broker::Broker;

/// Shared handler state.
pub type App = Arc<Broker>;

fn auth(b: &Broker, headers: &HeaderMap) -> Result<(), ApiError> {
    let given = headers.get("x-browser-token").and_then(|v| v.to_str().ok());
    if !token_ok(given, &b.cfg.browser_token()) {
        return Err(ApiError::new(401, "unauthorized"));
    }
    Ok(())
}

fn body_json(bytes: &Bytes) -> Result<Value, ApiError> {
    if bytes.is_empty() {
        return Ok(json!({}));
    }
    serde_json::from_slice(bytes)
        .map_err(|_| ApiError::new(400, "There was an error parsing the body"))
}

async fn health(State(b): State<App>) -> Json<Value> {
    Json(json!({"ok": true, "mode": b.mode(), "headless": b.headless(), "started": b.started()}))
}

async fn make_pdf(State(b): State<App>, headers: HeaderMap, body: Bytes) -> ApiResult {
    auth(&b, &headers)?;
    let v = body_json(&body)?;
    b.ensure_pw().await;
    b.touch();
    Ok(Json(b.render.make_pdf(&v).await?))
}

async fn make_png(State(b): State<App>, headers: HeaderMap, body: Bytes) -> ApiResult {
    auth(&b, &headers)?;
    let v = body_json(&body)?;
    b.ensure_pw().await;
    b.touch();
    Ok(Json(b.render.make_png(&v).await?))
}

async fn compose(State(b): State<App>, headers: HeaderMap, body: Bytes) -> ApiResult {
    auth(&b, &headers)?;
    let v = body_json(&body)?;
    b.ensure_pw().await;
    b.touch();
    Ok(Json(b.render.compose(&v).await?))
}

async fn agent_action(
    State(b): State<App>,
    Path(action): Path<String>,
    headers: HeaderMap,
    body: Bytes,
) -> ApiResult {
    auth(&b, &headers)?;
    let v = body_json(&body)?;
    Ok(Json(b.agent_action(&action, &v).await?))
}

async fn downloads(State(b): State<App>, headers: HeaderMap) -> ApiResult {
    auth(&b, &headers)?;
    Ok(Json(b.downloads()))
}

/// `GET /state`.
pub async fn state_json(b: &Broker) -> Value {
    let p = b.view_page();
    let title = match &p {
        Some(p) => p.title().await,
        None => String::new(),
    };
    let (popup, opener_url) = match &p {
        Some(p) => {
            let st = b.st.lock();
            match st.parent.get(p.id()) {
                Some(par) => (true, par.url()),
                None => (false, String::new()),
            }
        }
        None => (false, String::new()),
    };
    let tasks = b.task_list().await;
    let requests = b.open_requests();
    let requested = requests.first().cloned().unwrap_or(Value::Null);
    let st = b.st.lock();
    json!({"mode": st.mode, "url": p.as_ref().map(|p| p.url()).unwrap_or_default(), "title": title, "view_task": st.view_task,
        "popup": popup, "popup_opener_url": opener_url, "headless": st.headless,
        "requested": requested, "requests": requests, "takeover_task": st.takeover_task,
        "tasks": tasks, "viewport": {"width": crate::textfmt::VIEWPORT_W, "height": crate::textfmt::VIEWPORT_H}})
}

async fn state(State(b): State<App>, headers: HeaderMap) -> ApiResult {
    auth(&b, &headers)?;
    Ok(Json(state_json(&b).await))
}

#[derive(Deserialize)]
struct ShotQuery {
    task_id: Option<String>,
}

async fn screenshot(
    State(b): State<App>,
    headers: HeaderMap,
    Query(q): Query<ShotQuery>,
) -> Result<Response, ApiError> {
    auth(&b, &headers)?;
    let mut p = match q.task_id.as_deref().filter(|t| !t.is_empty()) {
        Some(t) => b.st.lock().pages.get(t).cloned(),
        None => b.view_page(),
    };
    if p.as_ref().map(|p| p.is_closed()).unwrap_or(true) {
        p = b.view_page();
    }
    let Some(p) = p else {
        return Ok(StatusCode::NO_CONTENT.into_response());
    };
    match p
        .screenshot(true, Some(60), None, Duration::from_secs(8))
        .await
    {
        Ok(img) if !img.is_empty() => Ok((
            [
                ("content-type", "image/jpeg"),
                ("cache-control", "no-store"),
            ],
            img,
        )
            .into_response()),
        _ => Ok(StatusCode::NO_CONTENT.into_response()),
    }
}

async fn user_view(State(b): State<App>, headers: HeaderMap, body: Bytes) -> ApiResult {
    auth(&b, &headers)?;
    let v = body_json(&body)?;
    let tid = get_str(&v, "task_id");
    {
        let mut st = b.st.lock();
        let known = !tid.is_empty() && st.pages.contains_key(&tid);
        if known {
            st.view_task = tid.clone();
        }
        // the user chose what to watch: keep it for 10 minutes ("pin": false hands the view back to the agents)
        let pin = v.get("pin").map(truthy).unwrap_or(true);
        st.view_pinned_until = if pin && known { now_ts() + 600.0 } else { 0.0 };
    }
    Ok(Json(state_json(&b).await))
}

async fn user_takeover(State(b): State<App>, headers: HeaderMap, body: Bytes) -> ApiResult {
    auth(&b, &headers)?;
    let v = body_json(&body)?;
    let mut tid = get_str(&v, "task_id");
    if tid.is_empty() {
        tid = b
            .requested()
            .map(|r| get_str(&r, "task_id"))
            .unwrap_or_default();
    }
    if tid.is_empty() {
        tid = b.st.lock().view_task.clone();
    }
    if tid.is_empty() {
        tid = "default".into();
    }
    {
        let st = b.st.lock();
        if st.mode == "user" && !st.takeover_task.is_empty() && st.takeover_task != tid {
            // one takeover at a time: switching silently would resume the first task while the user is still in it
            return Err(ApiError::new(
                409,
                format!(
                    "你正在接管另一个任务（{}），请先交还它 (hand back the current task first)",
                    st.takeover_task
                ),
            ));
        }
    }
    // Pause the task first (its next action gets 423), then wait briefly for an action already in flight.
    {
        let mut st = b.st.lock();
        st.mode = "user".into();
        st.takeover_task = tid.clone();
    }
    let lock = b.task_lock(&tid);
    let guard = match tokio::time::timeout(Duration::from_secs(8), lock.lock()).await {
        Ok(g) => Some(g),
        Err(_) => {
            println!(
                "[browser] takeover of {}: an agent action is still running; taking over anyway",
                tid
            );
            None
        }
    };
    let res = b.page(&tid).await; // the page the user drives is always a registered task page
    b.st.lock().view_task = tid.clone();
    drop(guard);
    res?;
    Ok(Json(state_json(&b).await))
}

async fn user_release(State(b): State<App>, headers: HeaderMap) -> ApiResult {
    auth(&b, &headers)?;
    // only the request this takeover answered is cleared; other tasks' requests stay open (they still need the user)
    let requested = b.requested();
    let mut st = b.st.lock();
    let tid = st.takeover_task.clone();
    let mut req = st.requests.shift_remove(&tid);
    if req.is_none() && (tid.is_empty() || tid == "default") && !st.requests.is_empty() {
        if let Some(r) = requested {
            req = st.requests.shift_remove(&get_str(&r, "task_id")); // untargeted takeover: it answered the latest request
        }
    }
    let released = json!({"task_id": tid, "requested": req});
    st.mode = "agent".into();
    st.takeover_task.clear();
    Ok(Json(json!({"ok": true, "released": released})))
}

async fn user_input(State(b): State<App>, headers: HeaderMap, body: Bytes) -> ApiResult {
    auth(&b, &headers)?;
    if b.mode() != "user" {
        return Err(ApiError::new(409, "请先点击「接管」(take over first)"));
    }
    let v = body_json(&body)?;
    let _g = b.input_lock.lock().await; // user events are applied strictly in arrival order
    Ok(Json(b.user_input(&v).await?))
}

/// The axum router over a broker.
pub fn router(b: App) -> Router {
    Router::new()
        .route("/health", get(health))
        .route("/pdf", post(make_pdf))
        .route("/png", post(make_png))
        .route("/compose", post(compose))
        .route("/agent/downloads", get(downloads))
        .route("/agent/{action}", post(agent_action))
        .route("/state", get(state))
        .route("/screenshot", get(screenshot))
        .route("/user/view", post(user_view))
        .route("/user/takeover", post(user_takeover))
        .route("/user/release", post(user_release))
        .route("/user/input", post(user_input))
        .with_state(b)
}

/// Shut the persistent Chromium down after it has been idle (`_idle_reaper`).
pub fn spawn_idle_reaper(b: App) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_secs(60)).await;
            if b.idle_ok() {
                b.teardown().await;
            }
        }
    })
}

/// Serve until the process ends.
pub async fn serve(cfg: Arc<Config>) -> anyhow::Result<()> {
    let b = Broker::new(cfg.clone());
    let reaper = if cfg.browser.idle_shutdown > 0.0 {
        Some(spawn_idle_reaper(b.clone()))
    } else {
        None
    };
    let addr = format!("{}:{}", cfg.server.browser_host, cfg.server.browser_port);
    let listener = tokio::net::TcpListener::bind(&addr).await?;
    println!("[browser] listening on {}", addr);
    let res = axum::serve(listener, router(b.clone())).await;
    if let Some(r) = reaper {
        r.abort();
    }
    b.stop().await;
    res?;
    Ok(())
}
