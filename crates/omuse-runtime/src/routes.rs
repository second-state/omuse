//! Agent Runtime HTTP API (reached only through Sentinel's reverse proxy) — port of `app/runtime/main.py`.
//!
//! [`serve`] is the FastAPI lifespan plus uvicorn: stall watch, runtime, skills seed, scheduler, health watch, golden
//! runs, task recovery, then the server on `[server] runtime_host:runtime_port`. [`router`] builds the routes (also
//! used by the unit tests). Handlers live in `routes/*.rs`, grouped like the sections of the Python file.

use std::collections::HashMap;
use std::sync::Arc;

use axum::body::Bytes;
use axum::extract::DefaultBodyLimit;
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post, put};
use axum::{Json, Router};
use omuse_common::api::ApiError;
use omuse_common::util::{py_str, truthy, VERSION};
use omuse_common::Config;
use serde_json::{json, Map, Value};

use crate::state::State;

mod auto;
mod chat;
mod files;
mod internal;
mod knowledge;
mod settings;
mod trust;

#[cfg(test)]
mod tests;

pub use settings::check_model_override;

/// Shared handler state.
pub type St = axum::extract::State<Arc<State>>;

/// A handler failure: an `HTTPException` (`{"detail": ...}`) or an unhandled exception (plain-text 500, like
/// Starlette's `ServerErrorMiddleware`).
#[derive(Debug)]
pub enum Fail {
    Api(ApiError),
    Internal(anyhow::Error),
}

impl From<ApiError> for Fail {
    fn from(e: ApiError) -> Self {
        Fail::Api(e)
    }
}

impl From<anyhow::Error> for Fail {
    fn from(e: anyhow::Error) -> Self {
        match e.downcast::<ApiError>() {
            Ok(api) => Fail::Api(api),
            Err(e) => Fail::Internal(e),
        }
    }
}

impl IntoResponse for Fail {
    fn into_response(self) -> Response {
        match self {
            Fail::Api(e) => e.into_response(),
            Fail::Internal(e) => {
                println!("[runtime] unhandled error: {:#}", e);
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    [(header::CONTENT_TYPE, "text/plain; charset=utf-8")],
                    "Internal Server Error",
                )
                    .into_response()
            }
        }
    }
}

/// Handler result.
pub type R<T = Json<Value>> = Result<T, Fail>;

/// `HTTPException(404)` (detail = the status phrase).
pub(crate) fn not_found() -> Fail {
    Fail::Api(ApiError::new(404, "Not Found"))
}

/// `HTTPException(status, detail)`.
pub(crate) fn http(status: u16, detail: impl Into<String>) -> Fail {
    Fail::Api(ApiError::new(status, detail))
}

/// `await req.json()` where the handler then calls `b.get(...)`: anything but a JSON object is a 500, as in Python.
pub(crate) fn body_obj(bytes: &Bytes) -> Result<Value, Fail> {
    let v: Value = serde_json::from_slice(bytes)
        .map_err(|e| Fail::Internal(anyhow::anyhow!("JSONDecodeError: {}", e)))?;
    if v.is_object() {
        Ok(v)
    } else {
        Err(Fail::Internal(anyhow::anyhow!(
            "AttributeError: request body is not an object"
        )))
    }
}

/// `await req.json() if (req.headers.get("content-length") or "0") not in skip else {}`.
pub(crate) fn body_opt(
    headers: &axum::http::HeaderMap,
    bytes: &Bytes,
    skip: &[&str],
) -> Result<Value, Fail> {
    let cl = headers
        .get(header::CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    let cl = if cl.is_empty() { "0" } else { cl };
    if skip.contains(&cl) {
        Ok(json!({}))
    } else {
        body_obj(bytes)
    }
}

/// Python `str(b.get(k, default))`.
pub(crate) fn pstr(b: &Value, k: &str, default: &str) -> String {
    b.get(k).map(py_str).unwrap_or_else(|| default.to_string())
}

/// Python `str(b.get(k) or default)`.
pub(crate) fn pstr_or(b: &Value, k: &str, default: &str) -> String {
    b.get(k)
        .filter(|v| truthy(v))
        .map(py_str)
        .unwrap_or_else(|| default.to_string())
}

/// `b.get(k)` (null when missing).
pub(crate) fn getv(b: &Value, k: &str) -> Value {
    b.get(k).cloned().unwrap_or(Value::Null)
}

/// The query string as a map (FastAPI query parameters).
pub type Q = axum::extract::Query<HashMap<String, String>>;

/// A FastAPI `int` parameter: 422 with a pydantic-like detail list when it does not parse.
pub(crate) fn int_param(
    q: &HashMap<String, String>,
    loc: &str,
    name: &str,
    default: Option<i64>,
) -> Result<i64, Fail> {
    match q.get(name) {
        None => default.ok_or_else(|| {
            Fail::Api(ApiError::with_json(
                422,
                json!([{"type": "missing", "loc": [loc, name], "msg": "Field required", "input": null}]),
            ))
        }),
        Some(v) => parse_int(v).ok_or_else(|| int_error(loc, name, v)),
    }
}

pub(crate) fn parse_int(v: &str) -> Option<i64> {
    v.trim().parse::<i64>().ok()
}

pub(crate) fn int_error(loc: &str, name: &str, v: &str) -> Fail {
    Fail::Api(ApiError::with_json(
        422,
        json!([{"type": "int_parsing", "loc": [loc, name],
                "msg": "Input should be a valid integer, unable to parse string as an integer", "input": v}]),
    ))
}

/// `{k: v for k, v in obj.items() if keep(k)}`.
pub(crate) fn filter_obj(v: &Value, keep: impl Fn(&str) -> bool) -> Value {
    let mut m = Map::new();
    if let Some(o) = v.as_object() {
        for (k, x) in o {
            if keep(k) {
                m.insert(k.clone(), x.clone());
            }
        }
    }
    Value::Object(m)
}

/// Python `round(x, 2)`.
pub(crate) fn round2(x: f64) -> f64 {
    (x * 100.0).round() / 100.0
}

async fn health() -> Json<Value> {
    Json(json!({"ok": true, "version": VERSION.as_str()}))
}

async fn fallback_404() -> Fail {
    not_found()
}

async fn fallback_405() -> Fail {
    http(405, "Method Not Allowed")
}

/// All routes of the runtime API.
pub fn router(state: Arc<State>) -> Router {
    Router::new()
        .route("/api/health", get(health))
        // chat & conversations
        .route("/api/upload", put(chat::upload))
        .route("/api/chat", post(chat::chat))
        .route("/api/conversations", get(chat::conversations))
        .route("/api/bots", get(chat::bots))
        .route(
            "/api/conversations/{cid}",
            get(chat::conversation).delete(chat::del_conv),
        )
        // tasks
        .route("/api/tasks", get(chat::tasks))
        .route("/api/tasks/{tid}", get(chat::task))
        .route("/api/tasks/{tid}/{action}", post(chat::task_action))
        // live events
        .route("/api/stream", get(chat::stream))
        // schedules & goals
        .route(
            "/api/schedules",
            get(auto::schedules).post(auto::add_schedule),
        )
        .route(
            "/api/schedules/{sid}",
            put(auto::upd_schedule).delete(auto::del_schedule),
        )
        .route("/api/schedules/{sid}/poll", post(auto::poll_schedule))
        .route("/api/schedules/{sid}/run", post(auto::run_schedule))
        .route("/api/goals", get(auto::goals).post(auto::add_goal))
        .route(
            "/api/goals/{gid}",
            put(auto::upd_goal).delete(auto::del_goal),
        )
        .route("/api/goals/{gid}/run", post(auto::run_goal))
        // skills
        .route(
            "/api/skills",
            get(knowledge::skills_list).put(knowledge::skills_select),
        )
        .route(
            "/api/skills/import",
            post(knowledge::skills_import).delete(knowledge::skills_delete_import),
        )
        .route("/api/skills/{name}", delete(knowledge::skills_delete))
        // research library
        .route(
            "/api/library",
            get(knowledge::library_list).post(knowledge::library_create),
        )
        .route(
            "/api/library/search",
            get(knowledge::library_search).delete(knowledge::library_delete_search),
        )
        .route(
            "/api/library/{sid}",
            get(knowledge::library_get).delete(knowledge::library_delete),
        )
        .route(
            "/api/library/{sid}/refresh",
            post(knowledge::library_refresh),
        )
        // memory
        .route(
            "/api/memory",
            get(knowledge::memory).post(knowledge::add_memory),
        )
        .route(
            "/api/memory/consolidate",
            post(knowledge::consolidate)
                .put(knowledge::edit_memory_consolidate)
                .delete(knowledge::del_memory_consolidate),
        )
        .route("/api/memory/runs/{rid}", get(knowledge::memory_run))
        .route(
            "/api/memory/{fid}",
            put(knowledge::edit_memory).delete(knowledge::del_memory),
        )
        .route("/api/context/preview", get(knowledge::context_preview))
        .route(
            "/api/entities/{eid}",
            put(knowledge::edit_entity).delete(knowledge::del_entity),
        )
        .route("/api/profile", put(knowledge::set_profile))
        .route("/api/profile/status", get(knowledge::profile_status))
        .route("/api/profile/ocr", put(knowledge::profile_ocr))
        .route(
            "/api/profile/pending/{pid}",
            post(knowledge::resolve_profile),
        )
        // trust: metrics, health, golden runs, outcomes
        .route("/api/metrics", get(trust::api_metrics))
        .route("/api/health/status", get(trust::api_health))
        .route("/api/health/check", post(trust::api_health_check))
        .route("/api/golden/runs", get(trust::golden_runs))
        .route("/api/golden/run", post(trust::golden_run))
        .route("/api/ledger/backfill", post(trust::ledger_backfill))
        .route("/api/ledger/reconcile", post(trust::ledger_reconcile))
        .route("/api/outcome/{tid}", post(trust::task_outcome))
        // notifications
        .route("/api/notifications", get(trust::notifications))
        .route("/api/notifications/read", post(trust::notifications_read))
        // settings
        .route(
            "/api/settings",
            get(settings::get_settings).put(settings::put_settings),
        )
        .route("/api/settings/test-model", post(settings::test_model))
        .route("/api/settings/test-models", post(settings::test_models))
        // workspace files
        .route("/api/files", get(files::files))
        .route("/api/files/raw", get(files::file_raw))
        // internal callbacks from Sentinel
        .route(
            "/internal/approval_resolved",
            post(internal::approval_resolved),
        )
        .route("/internal/notify_user", post(internal::notify_user))
        .route("/internal/takeover_ended", post(internal::takeover_ended))
        .fallback(fallback_404)
        .method_not_allowed_fallback(fallback_405)
        .layer(DefaultBodyLimit::disable())
        .with_state(state)
}

/// The lifespan + server: serve until the process ends.
pub async fn serve(cfg: Arc<Config>) -> anyhow::Result<()> {
    let data = cfg.runtime_data();
    std::fs::create_dir_all(&data)?;
    omuse_common::stallwatch::start(&data, "runtime", cfg.server.loop_stall_seconds);
    crate::prompts::set_app_id(&cfg.server.app_id);
    let state = State::new(cfg.clone())?;
    let rt = state.rt.clone();
    match rt.skills_dirs.seed(&rt.store, cfg.skills_initial()) {
        Ok(Some(seeded)) => {
            let from_env = cfg.skills_initial().is_some_and(|v| !v.is_empty());
            println!(
                "[skills] initial selection written: {}{}",
                if seeded.is_empty() {
                    "all on".to_string()
                } else {
                    format!("off: {}", seeded.join(", "))
                },
                if from_env { " (from OMUSE_SKILLS)" } else { "" }
            );
        }
        Ok(None) => {}
        Err(e) => println!("[skills] seed failed: {:#}", e),
    }
    state.sched.start();
    rt.health().start();
    state.golden.start();
    rt.recover().await;
    let addr = format!("{}:{}", cfg.server.runtime_host, cfg.server.runtime_port);
    let listener = tokio::net::TcpListener::bind(&addr).await?;
    println!("[runtime] listening on {}", addr);
    axum::serve(listener, router(state)).await?;
    Ok(())
}
