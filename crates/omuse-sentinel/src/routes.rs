//! Sentinel: the front door and the sole permission authority (port of `app/sentinel/main.py`).
//!
//! * Serves the web UI and reverse-proxies `/api/*` to the Agent Runtime.
//! * `/internal/*` is for the runtime only (runtime token): tool catalog, act, audit, notify.
//! * `/sentinel/api/*` is for the user (via the Olares entrance / the login gate): approvals, connections,
//!   credentials, audit, browser live view / takeover.
//!
//! Layout: this file holds the shared handler state ([`Ctx`]), the auth guards, small request helpers, the router
//! and [`serve`] (the FastAPI lifespan). The handlers live in `routes/*.rs`; everything this module needs from
//! the sibling modules that are ported in parallel (actions, telegram_bot, watchers, state) goes through
//! `routes/deps.rs`, the only place that names them.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use axum::body::Bytes;
use axum::extract::{Request, State};
use axum::http::{HeaderMap, Method};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{any, delete, get, post, put};
use axum::Router;
use omuse_apps::market::Market;
use omuse_common::api::ApiError;
use omuse_common::util::{py_str, take_chars, token_ok, truthy, VERSION};
use omuse_common::Config;

use crate::state::AppState;
use omuse_phone::{Phone, PhoneConfig};
use omuse_sentinel_core::catalog;
use omuse_sentinel_core::errors::ActionError;
use omuse_sentinel_core::passwd::Passwd;
use omuse_sentinel_core::policy::{self, Hooks};
use omuse_sentinel_core::store::{Audit, Store};
use omuse_web::Gate;
use parking_lot::Mutex;
use serde_json::{json, Map, Value};

mod act;
mod connections;
pub mod deps;
mod internal;
mod mcp;
mod proxy;
mod user;

#[cfg(test)]
mod tests;

pub use act::resolve_core;

/// Connectors shown on the Connections page and in the runtime catalog (Python `CONNECTORS`).
pub const CONNECTORS: [&str; 7] = [
    "gmail", "browser", "telegram", "notion", "slack", "calendar", "phone",
];
/// MCP OAuth callback path.
pub const OAUTH_CALLBACK: &str = "/sentinel/api/oauth/callback";
/// Google Calendar OAuth callback path.
pub const CAL_CALLBACK: &str = "/sentinel/api/connections/calendar/callback";
/// Telegram Bot API upload limit.
pub const TG_FILE_MAX: usize = 50 * 1024 * 1024;

/// Shared handler state: the sentinel [`AppState`] (reachable through `Deref`) plus what only the routes use.
pub struct Ctx {
    pub st: Arc<AppState>,
    pub passwd: Passwd,
    /// `proxy_client` for `/api/*`: a 600 s *read* timeout (not a total one, so a long-lived SSE stream is not cut),
    /// 10 s connect, 3 s keep-alive, no decompression (bytes pass through untouched).
    pub proxy: reqwest::Client,
    /// `SHOTS`: approval screenshots.
    pub shots: PathBuf,
    pub market: Market,
    /// `_MS_FLOWS`: Outlook device-code flows.
    ms_flows: Mutex<HashMap<String, Value>>,
}

impl std::ops::Deref for Ctx {
    type Target = AppState;
    fn deref(&self) -> &AppState {
        &self.st
    }
}

/// Handler state.
pub type App = Arc<Ctx>;

impl Ctx {
    /// Build the state for `cfg` (opens the store). Does not start anything.
    pub fn new(cfg: Arc<Config>) -> anyhow::Result<Ctx> {
        let store = Arc::new(Store::new(cfg.sentinel_data())?);
        Self::with_state(Arc::new(AppState::new(cfg, store)))
    }

    /// Build the routes' state around an existing [`AppState`].
    pub fn with_state(st: Arc<AppState>) -> anyhow::Result<Ctx> {
        let cfg = st.cfg.clone();
        let proxy = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(10))
            .read_timeout(Duration::from_secs(600))
            // keep-alive shorter than the runtime's: reusing a connection the runtime is closing gave sporadic 503s
            .pool_idle_timeout(Duration::from_secs(3))
            .no_proxy()
            .no_gzip()
            .redirect(reqwest::redirect::Policy::none())
            .build()?;
        Ok(Ctx {
            passwd: Passwd::from_config(&cfg),
            market: Market::new(omuse_common::http::client(30.0), cfg.market_data_hosts()),
            shots: cfg.sentinel_data().join("shots"),
            proxy,
            ms_flows: Mutex::new(HashMap::new()),
            st,
        })
    }

    /// The Telegram bot (always present; started only when `[telegram] enabled`).
    pub fn bot(&self) -> Option<Arc<deps::Bot>> {
        Some(self.st.bot.clone())
    }

    /// `store.audit(...)`, never failing the request (a broken audit write is logged).
    pub fn audit(&self, actor: &str, action: &str, a: Audit) -> Value {
        match self.store.audit(actor, action, a) {
            Ok(v) => v,
            Err(e) => {
                println!("[sentinel] audit write failed ({action}): {e:#}");
                json!({"seq": Value::Null})
            }
        }
    }

    /// `gmail_ready()`: at least one mailbox is connected.
    pub fn gmail_ready(&self) -> bool {
        omuse_mail::mailboxes::ready_accounts(&self.store)
            .map(|a| !a.is_empty())
            .unwrap_or(false)
    }

    /// `kv_get(k, "") or ""` as text.
    pub fn kv_str(&self, k: &str) -> String {
        match self.store.kv_get(k) {
            Ok(Some(v)) if truthy(&v) => py_str(&v),
            _ => String::new(),
        }
    }
}

// ================================================================== policy hooks (phone, MCP hub)

/// The policy engine's view of the phone line and the MCP hub.
struct SentinelHooks {
    phone: Arc<Phone>,
}

impl Hooks for SentinelHooks {
    fn mcp_tool_info(&self, store: &Store, tool: &str) -> Option<(Value, Value)> {
        omuse_mcp::mcp_hub::tool_info(store, tool).ok().flatten()
    }
    fn phone_ready(&self, _store: &Store) -> bool {
        self.phone.ready()
    }
    fn phone_config(&self, _store: &Store) -> Value {
        self.phone.config().to_value()
    }
    fn phone_check_call(
        &self,
        _store: &Store,
        number: &str,
        cfg: &Value,
    ) -> (String, String, String) {
        self.phone
            .check_call(number, Some(&PhoneConfig::from_value(cfg)))
    }
    fn phone_calls_today(&self, _store: &Store) -> i64 {
        self.phone.calls_today()
    }
}

// ================================================================== auth helpers

/// `runtime_auth`: the runtime's token in `X-Persona-Runtime`.
async fn runtime_auth(State(ctx): State<App>, req: Request, next: Next) -> Response {
    let given = req
        .headers()
        .get("x-persona-runtime")
        .and_then(|v| v.to_str().ok());
    if !token_ok(given, &ctx.cfg.runtime_token()) {
        return ApiError::new(401, "runtime token required").into_response();
    }
    next.run(req).await
}

/// `ui_auth` as a check: user endpoints must come through the web UI (custom header => no cross-site form posts).
pub fn ui_check(method: &Method, headers: &HeaderMap) -> Result<(), ApiError> {
    let safe = method == Method::GET || method == Method::HEAD;
    if !safe && header(headers, "x-persona-ui").as_deref() != Some("1") {
        return Err(ApiError::new(403, "UI header required"));
    }
    if let Some(origin) = header(headers, "origin").filter(|o| !o.is_empty()) {
        if !safe {
            let host = forwarded_host_raw(headers);
            let o = origin.split_once("://").map(|(_, r)| r).unwrap_or(&origin);
            let o = o.split('/').next().unwrap_or("");
            if o != host.split(',').next().unwrap_or("").trim() {
                return Err(ApiError::new(403, "cross-origin request rejected"));
            }
        }
    }
    Ok(())
}

async fn ui_auth(req: Request, next: Next) -> Response {
    if let Err(e) = ui_check(req.method(), req.headers()) {
        return e.into_response();
    }
    next.run(req).await
}

// ================================================================== request helpers

/// A header as text (`None` when missing or not UTF-8).
pub fn header(h: &HeaderMap, name: &str) -> Option<String> {
    h.get(name)
        .map(|v| String::from_utf8_lossy(v.as_bytes()).into_owned())
}

/// `headers.get("x-forwarded-host") or headers.get("host", "")` (not split).
pub fn forwarded_host_raw(h: &HeaderMap) -> String {
    match header(h, "x-forwarded-host").filter(|s| !s.is_empty()) {
        Some(s) => s,
        None => header(h, "host").unwrap_or_default(),
    }
}

/// The first forwarded host, trimmed.
pub fn req_host(h: &HeaderMap) -> String {
    forwarded_host_raw(h)
        .split(',')
        .next()
        .unwrap_or("")
        .trim()
        .to_string()
}

/// `(x-forwarded-proto or request.url.scheme or "https")`, first value: the server itself speaks plain HTTP.
pub fn req_proto(h: &HeaderMap) -> String {
    let p = header(h, "x-forwarded-proto")
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "http".into());
    p.split(',').next().unwrap_or("").trim().to_string()
}

/// `_req_origin(req)`: `proto://host` as the browser sees this server.
pub fn req_origin(h: &HeaderMap) -> String {
    format!("{}://{}", req_proto(h), req_host(h))
}

/// `await req.json()` for handlers that use the body as a dict.
pub fn body_obj(bytes: &Bytes) -> Result<Value, ApiError> {
    let v: Value = serde_json::from_slice(bytes)
        .map_err(|_| ApiError::new(400, "There was an error parsing the body"))?;
    if v.is_object() {
        Ok(v)
    } else {
        Err(ApiError::new(400, "There was an error parsing the body"))
    }
}

/// `str(b.get(k, default))`.
pub fn s_or(b: &Value, k: &str, default: &str) -> String {
    match b.get(k) {
        None => default.to_string(),
        Some(v) => py_str(v),
    }
}

/// `str(b.get(k) or "")`.
pub fn s_opt(b: &Value, k: &str) -> String {
    match b.get(k) {
        Some(v) if truthy(v) => py_str(v),
        _ => String::new(),
    }
}

/// `str(b.get(k, default))[:n]`.
pub fn s_cut(b: &Value, k: &str, default: &str, n: usize) -> String {
    take_chars(&s_or(b, k, default), n).to_string()
}

/// `b.get(k)` (missing → `null`).
pub fn g(b: &Value, k: &str) -> Value {
    b.get(k).cloned().unwrap_or(Value::Null)
}

/// `b.get(k, default)` as a value.
pub fn g_or(b: &Value, k: &str, default: Value) -> Value {
    b.get(k).cloned().unwrap_or(default)
}

/// `b.get(k) or default` as a value.
pub fn g_truthy_or(b: &Value, k: &str, default: Value) -> Value {
    match b.get(k) {
        Some(v) if truthy(v) => v.clone(),
        _ => default,
    }
}

/// Python `float(v)` (`None` where Python would raise).
pub fn py_float(v: &Value) -> Option<f64> {
    match v {
        Value::Number(n) => n.as_f64(),
        Value::Bool(b) => Some(if *b { 1.0 } else { 0.0 }),
        Value::String(s) => {
            let t = s.trim().to_lowercase();
            match t.as_str() {
                "inf" | "+inf" | "infinity" | "+infinity" => Some(f64::INFINITY),
                "-inf" | "-infinity" => Some(f64::NEG_INFINITY),
                "nan" | "+nan" | "-nan" => Some(f64::NAN),
                _ => t.replace('_', "").parse::<f64>().ok(),
            }
        }
        _ => None,
    }
}

/// Python `int(v)` (`None` where Python would raise).
pub fn py_int(v: &Value) -> Option<i64> {
    match v {
        Value::Number(n) => n.as_i64().or_else(|| {
            n.as_f64()
                .filter(|f| f.is_finite())
                .map(|f| f.trunc() as i64)
        }),
        Value::Bool(b) => Some(i64::from(*b)),
        Value::String(s) => s.trim().replace('_', "").parse::<i64>().ok(),
        _ => None,
    }
}

/// A JSON object from `(key, value)` pairs, in order.
pub fn obj(pairs: Vec<(&str, Value)>) -> Value {
    let mut m = Map::new();
    for (k, v) in pairs {
        m.insert(k.to_string(), v);
    }
    Value::Object(m)
}

/// Merge `extra` into a copy of `base` (`{**base, **extra}`).
pub fn merged(base: &Value, extra: Vec<(&str, Value)>) -> Value {
    let mut m = base.as_object().cloned().unwrap_or_default();
    for (k, v) in extra {
        m.insert(k.to_string(), v);
    }
    Value::Object(m)
}

/// The global `ActionError` handler: 423 when paused, 403 when blocked, else 400.
pub fn action_error(e: &ActionError) -> ApiError {
    let s = e.to_string();
    let code = if e.status == "paused" {
        423
    } else if s.contains("拦截") || s.contains("禁止") || s.contains("blocked") {
        403
    } else {
        400
    };
    ApiError::new(code, s)
}

/// A store (or other internal) failure: Python would answer 500.
pub fn internal(e: impl std::fmt::Display) -> ApiError {
    ApiError::new(500, format!("Internal Server Error: {e}"))
}

/// Result alias for handlers that answer JSON.
pub type JsonResult = Result<axum::Json<Value>, ApiError>;

// ================================================================== background helpers

/// `notify_runtime(path, payload)`: best-effort POST to the runtime with the runtime token.
pub async fn notify_runtime(ctx: &Ctx, path: &str, payload: Value) {
    let r = ctx
        .http
        .post(format!("{}{}", ctx.cfg.runtime_url(), path))
        .timeout(Duration::from_secs(15))
        .header(omuse_common::http::H_RUNTIME, ctx.cfg.runtime_token())
        .json(&payload)
        .send()
        .await;
    if let Err(e) = r {
        ctx.audit(
            "sentinel",
            "runtime.notify_failed",
            Audit {
                resource: path.into(),
                result: "error".into(),
                detail: Some(json!({"error": take_chars(&e.to_string(), 200)})),
                ..Default::default()
            },
        );
    }
}

/// Fire-and-forget [`notify_runtime`] (`asyncio.create_task(notify_runtime(...))`).
pub fn spawn_notify_runtime(ctx: &App, path: &'static str, payload: Value) {
    let ctx = ctx.clone();
    tokio::spawn(async move { notify_runtime(&ctx, path, payload).await });
}

/// `notify_phone(text)`: best-effort Telegram push for approvals / takeover requests.
pub async fn notify_phone(ctx: &Ctx, text: &str) {
    let Ok(conn) = ctx.store.connection("telegram") else {
        return;
    };
    let notify = conn["config"]
        .get("notify_approvals")
        .map(truthy)
        .unwrap_or(true);
    if truthy(&conn["enabled"])
        && notify
        && ctx.store.has_secret("cred_telegram_1").unwrap_or(false)
    {
        let _ = deps::telegram_send(ctx, text).await;
    }
}

/// `_safe(coro)`: run a Telegram send and audit its failure.
pub async fn safe_tg<F>(ctx: &Ctx, fut: F)
where
    F: std::future::Future<Output = anyhow::Result<()>>,
{
    if let Err(e) = fut.await {
        ctx.audit(
            "sentinel",
            "telegram.send_failed",
            Audit {
                result: "error".into(),
                detail: Some(json!({"error": take_chars(&format!("{e:#}"), 300)})),
                ..Default::default()
            },
        );
    }
}

// ================================================================== router

/// The Sentinel app (without the login gate).
pub fn router(ctx: App) -> Router {
    let internal = Router::new()
        .route("/internal/catalog", get(internal::catalog))
        .route("/internal/audit", post(internal::audit))
        .route("/internal/notify", post(internal::notify))
        .route("/internal/render_pdf", post(internal::render_pdf))
        .route("/internal/market_data", post(internal::market_data))
        .route("/internal/fundamentals", post(internal::fundamentals))
        .route("/internal/browser_release", post(internal::browser_release))
        .route("/internal/render_png", post(internal::render_png))
        .route("/internal/compose_image", post(internal::compose_image))
        .route("/internal/send_file", post(internal::send_file))
        .route("/internal/public_base", get(internal::public_base))
        .route(
            "/internal/expire_approvals",
            post(internal::expire_approvals),
        )
        .route("/internal/ledger", get(internal::ledger))
        .route("/internal/ledger/confirm", post(internal::ledger_confirm))
        .route("/internal/ledger/backfill", post(internal::ledger_backfill))
        .route(
            "/internal/ledger/reconcile",
            post(internal::ledger_reconcile),
        )
        .route("/internal/mail_check", get(internal::mail_check))
        .route("/internal/browser_state", get(internal::browser_state))
        .route("/internal/act", post(act::act))
        .route("/internal/user_request", post(internal::user_request))
        .route("/internal/vault", get(user::internal_vault))
        .route("/internal/watch/sources", get(internal::watch_sources))
        .route("/internal/watch", post(internal::watch))
        .route("/internal/taint", post(internal::taint))
        .route_layer(middleware::from_fn_with_state(ctx.clone(), runtime_auth));

    let ui = Router::new()
        .route(
            "/sentinel/api/grant_suggestions",
            get(user::grant_suggestions).post(user::grant_suggestion_decide),
        )
        .route("/sentinel/api/ledger", get(user::ledger))
        .route(
            "/sentinel/api/ledger/reconcile",
            post(user::ledger_reconcile),
        )
        .route("/sentinel/api/approvals", get(user::list_approvals))
        .route(
            "/sentinel/api/approvals/{aid}/screenshot",
            get(user::approval_shot),
        )
        .route("/sentinel/api/approvals/{aid}/resolve", post(act::resolve))
        .route(
            "/sentinel/api/vault",
            get(user::vault_list).post(user::vault_add),
        )
        .route(
            "/sentinel/api/vault/{iid}",
            put(user::vault_update).delete(user::vault_delete),
        )
        .route("/sentinel/api/grants", get(user::grants))
        .route("/sentinel/api/grants/{gid}", delete(user::revoke))
        .route("/sentinel/api/connections", get(connections::connections))
        .route(
            "/sentinel/api/connections/{name}",
            put(connections::save_conn),
        )
        .route(
            "/sentinel/api/connections/phone/credential",
            post(connections::phone_cred).delete(connections::delete_cred_phone),
        )
        .route(
            "/sentinel/api/connections/phone/test",
            post(connections::phone_test),
        )
        .route("/sentinel/api/phone/calls", get(connections::phone_calls))
        .route(
            "/sentinel/api/phone/test_call",
            post(connections::phone_test_call),
        )
        .route(
            "/sentinel/api/connections/phone/dialmcp/start",
            post(connections::dialmcp_start),
        )
        .route(
            "/sentinel/api/connections/phone/dialmcp",
            delete(connections::dialmcp_disconnect),
        )
        .route("/sentinel/api/mcp/oauth/start", post(mcp::oauth_start))
        .route(
            "/sentinel/api/connections/gmail/credential",
            post(connections::gmail_cred).delete(connections::delete_cred_gmail),
        )
        .route(
            "/sentinel/api/connections/gmail/oauth/start",
            post(connections::gmail_oauth_start).get(connections::gmail_oauth_poll_start),
        )
        .route(
            "/sentinel/api/connections/gmail/oauth/{flow}",
            get(connections::gmail_oauth_poll),
        )
        .route(
            "/sentinel/api/connections/gmail/test",
            post(connections::gmail_test),
        )
        .route(
            "/sentinel/api/connections/gmail/accounts/{aid}",
            delete(connections::gmail_remove),
        )
        .route(
            "/sentinel/api/connections/gmail/default",
            post(connections::gmail_default),
        )
        .route(
            "/sentinel/api/connections/telegram/credential",
            post(connections::telegram_cred).delete(connections::delete_cred_telegram),
        )
        .route(
            "/sentinel/api/telegram/status",
            get(connections::telegram_status),
        )
        .route(
            "/sentinel/api/connections/telegram/detect",
            post(connections::telegram_detect),
        )
        .route(
            "/sentinel/api/connections/notion/credential",
            post(connections::notion_cred).delete(connections::delete_cred_notion),
        )
        .route(
            "/sentinel/api/connections/calendar/start",
            post(connections::calendar_start),
        )
        .route(
            "/sentinel/api/connections/calendar/google/start",
            post(connections::calendar_google_start),
        )
        .route(
            "/sentinel/api/connections/calendar/google/client",
            post(connections::calendar_google_client)
                .delete(connections::calendar_google_client_delete),
        )
        .route(
            "/sentinel/api/connections/calendar/ical",
            post(connections::calendar_ical),
        )
        .route(
            "/sentinel/api/connections/calendar/test",
            post(connections::calendar_test),
        )
        .route(
            "/sentinel/api/connections/slack/credential",
            post(connections::slack_cred).delete(connections::delete_cred_slack),
        )
        .route("/sentinel/api/mcp/servers", get(mcp::list).post(mcp::add))
        .route(
            "/sentinel/api/mcp/servers/{sid}/refresh",
            post(mcp::refresh),
        )
        .route("/sentinel/api/mcp/servers/{sid}/test", post(mcp::test))
        .route(
            "/sentinel/api/mcp/servers/{sid}",
            put(mcp::update).delete(mcp::remove),
        )
        .route(
            "/sentinel/api/mcp/servers/{sid}/tools/{tname}",
            put(mcp::tool_mode),
        )
        .route(
            "/sentinel/api/connections/{name}/credential",
            delete(connections::delete_cred),
        )
        .route("/sentinel/api/audit", get(user::audit_list))
        .route("/sentinel/api/audit/verify", get(user::audit_verify))
        .route("/sentinel/api/browser/state", get(user::b_state))
        .route("/sentinel/api/browser/screenshot", get(user::b_shot))
        .route("/sentinel/api/browser/takeover", post(user::b_takeover))
        .route("/sentinel/api/browser/release", post(user::b_release))
        .route("/sentinel/api/browser/view", post(user::b_view))
        .route("/sentinel/api/browser/input", post(user::b_input))
        .route(
            "/sentinel/api/password",
            get(user::password_status).post(user::password_change),
        )
        .route("/sentinel/api/subscription", get(user::subscription))
        .route(
            "/sentinel/api/subscription/portal",
            post(user::subscription_portal),
        )
        .route_layer(middleware::from_fn(ui_auth));

    let open = Router::new()
        .route(OAUTH_CALLBACK, get(mcp::oauth_callback))
        .route(CAL_CALLBACK, get(connections::calendar_callback))
        .route("/sentinel/api/health", get(user::health))
        .route("/api/{*path}", any(proxy::proxy))
        .route("/", get(proxy::index))
        .route("/static/{*name}", get(proxy::static_file));

    Router::new()
        .merge(internal)
        .merge(ui)
        .merge(open)
        .fallback(not_found)
        .method_not_allowed_fallback(method_not_allowed)
        .with_state(ctx)
}

async fn not_found() -> ApiError {
    ApiError::new(404, "Not Found")
}

async fn method_not_allowed() -> ApiError {
    ApiError::new(405, "Method Not Allowed")
}

/// The router wrapped in the login gate when `[auth]` is on and `[auth] gate` is not switched off.
pub fn app(ctx: App) -> anyhow::Result<Router> {
    let r = router(ctx.clone());
    if !ctx.cfg.auth_enabled() || !ctx.cfg.auth.gate {
        return Ok(r);
    }
    let pv = ctx.passwd.clone();
    let pver = ctx.passwd.clone();
    let gate = Gate::new(
        Arc::new(move |u: &str, p: &str| pv.verify(u, p)),
        Arc::new(move || pver.version()),
        ctx.passwd.user().to_string(),
        ctx.cfg.sentinel_data().join("session.key"),
        ctx.cfg.auth.session_days,
        omuse_web::gate::FAIL_DELAY_S,
    )?
    .with_proxy_headers(ctx.cfg.server.proxy_headers);
    Ok(r.layer(middleware::from_fn_with_state(Arc::new(gate), Gate::layer)))
}

// ================================================================== lifespan + server

/// Start Sentinel (and the voice app when `[voice] port` is not 0) and serve until shutdown.
pub async fn serve(cfg: Arc<Config>) -> anyhow::Result<()> {
    let sdata = cfg.sentinel_data();
    std::fs::create_dir_all(sdata.join("shots"))?;
    omuse_common::stallwatch::start(&sdata, "sentinel", cfg.server.loop_stall_seconds);
    let ctx: App = Arc::new(Ctx::new(cfg.clone())?);
    policy::set_hooks(Arc::new(SentinelHooks {
        phone: ctx.phone.clone(),
    }));
    ctx.audit(
        "sentinel",
        "sentinel.start",
        Audit {
            detail: Some(json!({"version": VERSION.as_str()})),
            ..Default::default()
        },
    );
    if let Err(e) = omuse_mcp::mcp_hub::sync_catalog(&ctx.store) {
        // never block startup on a bad MCP config
        ctx.audit(
            "sentinel",
            "mcp.sync",
            Audit {
                result: "error".into(),
                detail: Some(json!({"error": take_chars(&format!("{e:?}"), 300)})),
                ..Default::default()
            },
        );
    }
    deps::install_resolver(&ctx);
    if cfg.telegram.enabled {
        ctx.st.bot.start();
    }
    let (stop_tx, stop_rx) = tokio::sync::watch::channel(false);
    if cfg.voice.port != 0 {
        let addr = format!("0.0.0.0:{}", cfg.voice.port);
        let listener = tokio::net::TcpListener::bind(&addr)
            .await
            .map_err(|e| anyhow::anyhow!("bind {addr}: {e}"))?;
        let voice = omuse_phone::voice_app::router(ctx.phone.clone());
        let mut rx = stop_rx.clone();
        tokio::spawn(async move {
            let r = axum::serve(listener, voice)
                .with_graceful_shutdown(async move {
                    let _ = rx.wait_for(|v| *v).await;
                })
                .await;
            if let Err(e) = r {
                println!("[voice] server error: {e}");
            }
        });
    }
    let app = app(ctx.clone())?;
    let addr = format!("{}:{}", cfg.server.host, cfg.server.port);
    let listener = tokio::net::TcpListener::bind(&addr)
        .await
        .map_err(|e| anyhow::anyhow!("bind {addr}: {e}"))?;
    println!("[sentinel] listening on {addr}");
    let res = axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await;
    let _ = stop_tx.send(true);
    ctx.st.bot.stop().await;
    omuse_mcp::mcp_hub::drop_all().await;
    res.map_err(Into::into)
}

async fn shutdown_signal() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{signal, SignalKind};
        match signal(SignalKind::terminate()) {
            Ok(mut term) => {
                tokio::select! {
                    _ = tokio::signal::ctrl_c() => {},
                    _ = term.recv() => {},
                }
            }
            Err(_) => {
                let _ = tokio::signal::ctrl_c().await;
            }
        }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
}

/// The tool catalog entry (`TOOLS[name]`).
pub fn tool(name: &str) -> Option<Value> {
    catalog::get(name)
}
