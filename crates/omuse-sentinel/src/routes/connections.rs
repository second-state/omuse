//! User API: connections & credentials (mail, Telegram, Notion, Slack, Google Calendar / iCal, phone + DialMCP).

use std::collections::HashMap;
use std::sync::LazyLock;
use std::time::Duration;

use axum::body::Bytes;
use axum::extract::{Path, Query, State};
use axum::http::HeaderMap;
use axum::response::{IntoResponse, Response};
use axum::Json;
use base64::Engine;
use omuse_apps::gcal::{self, GCal};
use omuse_apps::ical::{self, ICalFeed};
use omuse_apps::notion::Notion;
use omuse_apps::slack::Slack;
use omuse_common::api::ApiError;
use omuse_common::util::{get_str, now_ts, py_str, take_chars, token_urlsafe, truthy};
use omuse_mail::{mailboxes, mailproviders};
use omuse_phone::dialmcp;
use omuse_phone::phone::{normalize, MAX_MINUTES_CAP};
use omuse_sentinel_core::catalog;
use omuse_sentinel_core::store::Audit;
use omuse_web::templates::py_html_escape;
use regex::Regex;
use serde_json::{json, Map, Value};
use sha2::Digest;

use super::deps::{self, BotExt};
use super::mcp::oauth_redirect;
use super::{
    body_obj, g, internal as ierr, py_float, py_int, req_origin, s_opt, s_or, App, Ctx, JsonResult,
    CAL_CALLBACK, CONNECTORS,
};

fn audit_user(resource: &str, result: &str, detail: Option<Value>) -> Audit {
    Audit {
        resource: resource.into(),
        result: result.into(),
        detail,
        ..Default::default()
    }
}

fn re(p: &str) -> Regex {
    Regex::new(&format!("^(?:{p})$")).expect("static regex")
}

static EMAIL_RE: LazyLock<Regex> =
    LazyLock::new(|| re(r"[^@\s<>,;]+@[^@\s<>,;]+\.[A-Za-z0-9-]{2,}"));
static CHAT_RE: LazyLock<Regex> = LazyLock::new(|| re(r"-?\d{3,20}"));
static NOTION_RE: LazyLock<Regex> = LazyLock::new(|| re(r"(secret_|ntn_)[A-Za-z0-9]{20,80}"));
static SLACK_RE: LazyLock<Regex> = LazyLock::new(|| re(r"xox[bp]-[A-Za-z0-9-]{10,200}"));
static GCID_RE: LazyLock<Regex> =
    LazyLock::new(|| re(r"[0-9]+-[A-Za-z0-9_]+\.apps\.googleusercontent\.com"));
static GSECRET_RE: LazyLock<Regex> = LazyLock::new(|| re(r"[A-Za-z0-9_\-]{10,100}"));
static ORIGIN_RE: LazyLock<Regex> = LazyLock::new(|| re(r"https?://[^/\s]+"));

// ================================================================== connection views

/// `_conn_view(name)`: one connection for the Connections page.
pub fn conn_view(ctx: &Ctx, name: &str) -> Result<Value, ApiError> {
    let store = &ctx.store;
    let mut c = store.connection(name).map_err(ierr)?;
    let has = store.has_secret(&format!("cred_{name}_1")).map_err(ierr)?;
    c["has_credential"] = json!(has);
    c["credential_handle"] = json!(if has {
        format!("cred_{name}_1")
    } else {
        String::new()
    });
    if name == "gmail" {
        let accs: Vec<Value> = mailboxes::accounts(store)
            .map_err(ierr)?
            .iter()
            .map(|a| {
                let mut m = super::obj(
                    ["id", "email", "display_name", "ready", "provider", "auth"]
                        .iter()
                        .map(|k| (*k, g(a, k)))
                        .collect(),
                );
                m["provider_label"] = json!(mailproviders::label(&get_str(a, "provider")));
                m
            })
            .collect();
        c["accounts"] = json!(accs);
        c["providers"] = json!(mailproviders::public_presets());
        c["default"] = json!(mailboxes::default_id(store).map_err(ierr)?);
        c["has_credential"] = json!(ctx.gmail_ready());
    }
    if name == "calendar" {
        let sec = store
            .get_secret("cred_calendar_1")
            .map_err(ierr)?
            .unwrap_or(json!({}));
        let mode = if truthy(&g(&sec, "ical_url")) {
            "ical"
        } else if truthy(&g(&sec, "managed")) {
            "google"
        } else if truthy(&sec) {
            "google_own"
        } else {
            ""
        };
        c["mode"] = json!(mode);
        let m = gcal::managed(&ctx.google, Some(store));
        c["managed_available"] = json!(m.is_some());
        c["managed_source"] = json!(m.as_ref().map(|m| m.source.clone()).unwrap_or_default());
        c["relay_url"] = json!(m
            .map(|m| m.relay)
            .filter(|r| !r.is_empty())
            .unwrap_or_else(|| ctx.google.relay_url.clone()));
    }
    if name == "phone" {
        let d = &ctx.phone.dial;
        c["dialmcp"] = json!({"connected": d.ready(), "account": d.account(), "url": d.url()});
        c["telnyx_ready"] = json!(ctx.phone.telnyx_ready());
        c["ready"] = json!(ctx.phone.ready());
    }
    Ok(c)
}

/// `GET /sentinel/api/connections`.
pub async fn connections(State(ctx): State<App>) -> JsonResult {
    let mut list = Vec::new();
    for n in CONNECTORS {
        list.push(conn_view(&ctx, n)?);
    }
    let mut tools = Map::new();
    for (k, v) in catalog::snapshot() {
        tools.insert(
            k,
            json!({"connector": v["connector"], "capability": v["capability"], "risk": v["risk"]}),
        );
    }
    Ok(Json(json!({"connections": list, "tools": tools})))
}

/// `PUT /sentinel/api/connections/{name}`: settings, permissions, enabled.
pub async fn save_conn(
    State(ctx): State<App>,
    Path(name): Path<String>,
    body: Bytes,
) -> JsonResult {
    if !CONNECTORS.contains(&name.as_str()) {
        return Err(ApiError::new(404, "Not Found"));
    }
    let b = body_obj(&body)?;
    let mut cfg: Option<Value> = None;
    if let Some(Value::Object(raw)) = b.get("config") {
        let drop = [
            "app_password",
            "bot_token",
            "password",
            "token",
            "workspace",
            "bot_id",
            "team",
            "user",
            "user_id",
            "token_type",
        ];
        let mut m: Map<String, Value> = raw
            .iter()
            .filter(|(k, _)| !drop.contains(&k.as_str()))
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        if name == "calendar" {
            m.retain(|k, _| k == "time_zone");
        }
        if name == "phone" {
            m = phone_cfg(&ctx, &Value::Object(m))?
                .as_object()
                .cloned()
                .unwrap_or_default();
        }
        if name == "browser" {
            for k in ["blocked_domains", "allowed_domains"] {
                if let Some(Value::String(s)) = m.get(k).cloned() {
                    let list: Vec<String> = s
                        .replace('\n', ",")
                        .split(',')
                        .map(|x| x.trim().to_lowercase())
                        .filter(|x| !x.is_empty())
                        .collect();
                    m.insert(k.into(), json!(list));
                }
            }
        }
        cfg = Some(Value::Object(m));
    }
    let perms = b.get("permissions").filter(|v| !v.is_null());
    let enabled = b.get("enabled").filter(|v| !v.is_null()).map(truthy);
    let c = ctx
        .store
        .save_connection(&name, cfg.as_ref(), perms, enabled)
        .map_err(ierr)?;
    ctx.audit(
        "user",
        "connection.update",
        audit_user(
            &name,
            "success",
            Some(json!({"permissions": c["permissions"], "enabled": c["enabled"]})),
        ),
    );
    Ok(Json(conn_view(&ctx, &name)?))
}

/// `DELETE /sentinel/api/connections/{name}/credential`.
pub async fn delete_cred(State(ctx): State<App>, Path(name): Path<String>) -> JsonResult {
    delete_cred_named(&ctx, &name).await
}

/// The same for the static `/connections/<name>/credential` routes, whose POST handlers shadow the path pattern.
pub async fn delete_cred_named(ctx: &App, name: &str) -> JsonResult {
    let store = &ctx.store;
    if name == "gmail" {
        for a in mailboxes::accounts(store).map_err(ierr)? {
            mailboxes::remove_account(store, &get_str(&a, "id")).map_err(ierr)?;
        }
    }
    store
        .delete_secret(&format!("cred_{name}_1"))
        .map_err(ierr)?;
    if name == "calendar" {
        store.delete_secret("cred_calendar_access").map_err(ierr)?;
        store.delete_secret("cred_calendar_client").map_err(ierr)?;
    }
    if matches!(name, "notion" | "slack" | "calendar" | "phone") {
        store
            .save_connection(name, None, None, Some(false))
            .map_err(ierr)?;
    }
    ctx.audit(
        "user",
        &format!("credential.{name}.delete"),
        audit_user(name, "success", None),
    );
    Ok(Json(json!({"ok": true})))
}

macro_rules! delete_cred_for {
    ($fn:ident, $name:literal) => {
        /// `DELETE /sentinel/api/connections/<name>/credential`.
        pub async fn $fn(State(ctx): State<App>) -> JsonResult {
            delete_cred_named(&ctx, $name).await
        }
    };
}
delete_cred_for!(delete_cred_gmail, "gmail");
delete_cred_for!(delete_cred_phone, "phone");
delete_cred_for!(delete_cred_telegram, "telegram");
delete_cred_for!(delete_cred_notion, "notion");
delete_cred_for!(delete_cred_slack, "slack");

// ================================================================== phone (Telnyx + OpenAI Realtime, DialMCP)

/// `_phone_cfg(b)`: the phone settings from a form.
fn phone_cfg(ctx: &Ctx, b: &Value) -> Result<Value, ApiError> {
    let mut out = Map::new();
    for k in [
        "from_number",
        "connection_id",
        "owner_name",
        "voice",
        "model",
    ] {
        if let Some(v) = b.get(k).filter(|v| !v.is_null()) {
            out.insert(k.into(), json!(take_chars(py_str(v).trim(), 200)));
        }
    }
    if let Some(p @ ("auto" | "telnyx" | "dialmcp")) = b.get("provider").and_then(Value::as_str) {
        out.insert("provider".into(), json!(p));
    }
    if let Some(n) = out.get("from_number").map(py_str) {
        out.insert("from_number".into(), json!(normalize(&n)));
    }
    if let Some(v) = b.get("public_url").filter(|v| !v.is_null()) {
        let u = py_str(v).trim().trim_end_matches('/').to_string();
        if !u.is_empty() && !ctx.phone.public_ok(&u) {
            return Err(ApiError::new(
                400,
                "公开地址必须以 https:// 开头 (public URL must start with https://)",
            ));
        }
        out.insert(
            "public_url".into(),
            json!(u.split("/voice").next().unwrap_or("")),
        );
    }
    if let Some(v) = b.get("allowed_prefixes").filter(|v| !v.is_null()) {
        let items: Vec<String> = match v {
            Value::Array(a) => a.iter().map(py_str).collect(),
            other => py_str(other)
                .replace('\n', ",")
                .split(',')
                .map(String::from)
                .collect(),
        };
        let list: Vec<String> = items
            .iter()
            .filter_map(|x| {
                let d = x.trim().trim_start_matches('+');
                (!d.is_empty() && d.chars().all(|c| c.is_ascii_digit())).then(|| format!("+{d}"))
            })
            .take(20)
            .collect();
        out.insert("allowed_prefixes".into(), json!(list));
    }
    for (k, lo, hi) in [("max_minutes", 1, MAX_MINUTES_CAP), ("daily_limit", 1, 100)] {
        match b.get(k) {
            None | Some(Value::Null) => {}
            Some(Value::String(s)) if s.is_empty() => {}
            Some(v) => {
                let n = py_int(v).ok_or_else(|| {
                    ApiError::new(400, format!("{k} 必须是数字 (must be a number)"))
                })?;
                out.insert(k.into(), json!(n.min(hi).max(lo)));
            }
        }
    }
    Ok(Value::Object(out))
}

/// `POST /sentinel/api/connections/phone/credential`: Telnyx + OpenAI keys and the phone settings.
pub async fn phone_cred(State(ctx): State<App>, body: Bytes) -> JsonResult {
    let b = body_obj(&body)?;
    let store = &ctx.store;
    let mut sec = store
        .get_secret("cred_phone_1")
        .map_err(ierr)?
        .filter(Value::is_object)
        .unwrap_or(json!({}));
    for k in ["telnyx_api_key", "openai_api_key", "telnyx_public_key"] {
        let v = s_opt(&b, k).trim().to_string();
        if !v.is_empty() {
            sec[k] = json!(v);
        }
    }
    if !(truthy(&g(&sec, "telnyx_api_key")) && truthy(&g(&sec, "openai_api_key"))) {
        return Err(ApiError::new(
            400,
            "需要 Telnyx API Key 和 OpenAI API Key (both keys are required)",
        ));
    }
    store.put_secret("phone", &sec, None).map_err(ierr)?;
    let pc = phone_cfg(&ctx, &b)?;
    store
        .save_connection("phone", Some(&pc), Some(&json!({"call": true})), Some(true))
        .map_err(ierr)?;
    let checks = ctx.phone.test_setup().await;
    ctx.audit(
        "user",
        "credential.phone.set",
        audit_user("phone", "success", Some(json!({"checks": checks}))),
    );
    Ok(Json(
        json!({"ok": true, "checks": checks, "ready": ctx.phone.ready(), "connection": conn_view(&ctx, "phone")?}),
    ))
}

/// `POST /sentinel/api/connections/phone/test`.
pub async fn phone_test(State(ctx): State<App>) -> JsonResult {
    Ok(Json(
        json!({"checks": ctx.phone.test_setup().await, "ready": ctx.phone.ready()}),
    ))
}

/// `GET /sentinel/api/phone/calls?limit=`.
pub async fn phone_calls(
    State(ctx): State<App>,
    Query(q): Query<HashMap<String, String>>,
) -> JsonResult {
    let limit = super::internal::q_int(&q, "limit", 20)?;
    Ok(Json(
        json!({"calls": ctx.phone.list_calls(limit.clamp(1, 100))}),
    ))
}

/// `POST /sentinel/api/phone/test_call`: a short test call the user starts (no approval card).
pub async fn phone_test_call(State(ctx): State<App>, body: Bytes) -> JsonResult {
    let b = body_obj(&body)?;
    let lang = s_opt(&b, "language").trim().to_string();
    let args = json!({
        "to": super::g_or(&b, "to", json!("")), "max_minutes": 2, "language": lang,
        "purpose": "This is a short test call of OMuse's phone feature to its owner. Greet them, say \
                    you are OMuse testing the phone line, ask them to say a sentence, repeat back \
                    what you heard, then say goodbye and end the call with outcome done.",
    });
    let r = ctx
        .phone
        .start_call(&args, "")
        .await
        .map_err(|e| ApiError::new(400, e.to_string()))?;
    ctx.audit(
        "user",
        "phone.test_call",
        audit_user(
            &get_str(&r, "to"),
            "success",
            Some(json!({"call_id": g(&r, "call_id")})),
        ),
    );
    Ok(Json(r))
}

/// `POST /sentinel/api/connections/phone/dialmcp/start`: OAuth sign-in to DialMCP.
pub async fn dialmcp_start(State(ctx): State<App>, headers: HeaderMap, body: Bytes) -> JsonResult {
    let b = body_obj(&body)?;
    let redirect = oauth_redirect(&headers, &b)?;
    match omuse_mcp::mcp_oauth::begin(&ctx.phone.dial.url(), &redirect, "phone", None).await {
        Ok(r) => {
            ctx.audit(
                "user",
                "credential.dialmcp.start",
                audit_user("phone", "success", None),
            );
            Ok(Json(json!({"auth_url": r.auth_url})))
        }
        Err(e) => {
            ctx.audit(
                "user",
                "credential.dialmcp.start",
                audit_user(
                    "phone",
                    "failed",
                    Some(json!({"error": take_chars(&e.0, 300)})),
                ),
            );
            Err(ApiError::new(
                400,
                format!("无法开始 DialMCP 登录 (cannot start sign-in): {}", e.0),
            ))
        }
    }
}

/// `DELETE /sentinel/api/connections/phone/dialmcp`.
pub async fn dialmcp_disconnect(State(ctx): State<App>) -> JsonResult {
    ctx.store.delete_secret(dialmcp::HANDLE).map_err(ierr)?;
    ctx.phone.dial.drop_session().await;
    ctx.audit(
        "user",
        "credential.dialmcp.delete",
        audit_user("phone", "success", None),
    );
    Ok(Json(conn_view(&ctx, "phone")?))
}

// ================================================================== mailboxes

/// `_mail_form(b)` → `(email, provider, display name, server settings)`.
fn mail_form(b: &Value) -> Result<(String, String, String, Value), ApiError> {
    let email = s_or(b, "email", "").trim().to_string();
    if !EMAIL_RE.is_match(&email) {
        return Err(ApiError::new(
            400,
            "请填写正确的邮箱地址 (valid email address required)",
        ));
    }
    let mut provider = s_opt(b, "provider").trim().to_string();
    if provider.is_empty() {
        provider = mailproviders::guess_provider(&email);
    }
    if provider.is_empty() {
        provider = "custom".into();
    }
    if mailproviders::provider(&provider).is_none() {
        return Err(ApiError::new(400, format!("unknown provider {provider}")));
    }
    let servers = mailproviders::server_settings(&provider, &email, Some(b))
        .map_err(|e| ApiError::new(400, e.0.clone()))?;
    let dname = take_chars(s_or(b, "display_name", "").trim(), 120).to_string();
    Ok((email, provider, dname, servers))
}

/// `[endpoints] outlook_imap` / `outlook_smtp` (`host:port:security`) in place of Microsoft's servers.
fn outlook_overrides(
    ep: &omuse_common::config::EndpointsConfig,
    servers: &mut Value,
) -> Result<(), ApiError> {
    for (kind, spec) in [("imap", &ep.outlook_imap), ("smtp", &ep.outlook_smtp)] {
        if spec.is_empty() {
            continue;
        }
        let bad = || {
            ApiError::new(
                500,
                format!("[endpoints] outlook_{kind}: expected host:port:security"),
            )
        };
        let mut parts = spec.rsplitn(3, ':');
        let (Some(security), Some(port), Some(host)) = (parts.next(), parts.next(), parts.next())
        else {
            return Err(bad());
        };
        let port: u16 = port.parse().map_err(|_| bad())?;
        if host.is_empty() || !["ssl", "starttls", "none"].contains(&security) {
            return Err(bad());
        }
        servers[format!("{kind}_host")] = json!(host);
        servers[format!("{kind}_port")] = json!(port);
        servers[format!("{kind}_security")] = json!(security);
    }
    Ok(())
}

/// `_mail_connect`: test the mailbox with the given secret, then store it. Nothing is saved when the test fails.
async fn mail_connect(
    ctx: &App,
    email: &str,
    provider: &str,
    dname: &str,
    servers: &Value,
    mut secret: Value,
) -> JsonResult {
    let mut acc =
        json!({"id": "test", "email": email, "display_name": dname, "provider": provider});
    if let (Some(dst), Some(src)) = (acc.as_object_mut(), servers.as_object()) {
        for (k, v) in src {
            dst.insert(k.clone(), v.clone());
        }
    }
    // a fresh access token was just issued; use it for the test instead of refreshing
    let tok = secret
        .get_mut("oauth")
        .and_then(Value::as_object_mut)
        .and_then(|o| o.remove("_access_token"))
        .map(|v| py_str(&v))
        .unwrap_or_default();
    let info = match deps::mail_connect_test(ctx, &acc, &secret, &tok).await {
        Ok(i) => i,
        Err(e) => {
            ctx.audit(
                "user",
                "credential.gmail.set",
                audit_user(
                    "gmail",
                    "failed",
                    Some(json!({"error": take_chars(&e, 300), "provider": provider})),
                ),
            );
            return Err(ApiError::new(400, e));
        }
    };
    let oauth = secret.get("oauth").filter(|o| truthy(o));
    let saved = mailboxes::save_account(
        &ctx.store,
        email,
        &s_or(&secret, "app_password", ""),
        dname,
        provider,
        Some(servers),
        oauth,
    )
    .map_err(ierr)?;
    let aid = get_str(&saved, "id");
    deps::forget_ms_token(&aid).await;
    ctx.audit(
        "user",
        "credential.gmail.set",
        audit_user(
            "gmail",
            "success",
            Some(json!({"email": email, "account": aid, "provider": provider})),
        ),
    );
    Ok(Json(
        json!({"ok": true, "test": info, "account": aid, "connection": conn_view(ctx, "gmail")?}),
    ))
}

/// `POST /sentinel/api/connections/gmail/credential`: a mailbox with an app password (every provider but Outlook).
pub async fn gmail_cred(State(ctx): State<App>, body: Bytes) -> JsonResult {
    let b = body_obj(&body)?;
    let (email, provider, dname, servers) = mail_form(&b)?;
    if mailproviders::provider(&provider).is_some_and(|p| p.auth == "oauth") {
        return Err(ApiError::new(
            400,
            "Outlook 需要用微软账号登录授权 (use Sign in with Microsoft)",
        ));
    }
    let mut pw = s_or(&b, "app_password", "").trim().to_string();
    if matches!(provider.as_str(), "gmail" | "yahoo" | "aol" | "icloud") {
        pw = pw.replace(' ', "");
    }
    if provider == "gmail" && pw.chars().count() < 12 {
        return Err(ApiError::new(
            400,
            "请填写邮箱地址和 16 位应用专用密码 (App Password)",
        ));
    }
    if pw.is_empty() && get_str(&servers, "smtp_security") != "none" {
        return Err(ApiError::new(
            400,
            "请填写密码 / 授权码 (password required)",
        ));
    }
    mail_connect(
        &ctx,
        &email,
        &provider,
        &dname,
        &servers,
        json!({"app_password": pw}),
    )
    .await
}

/// `POST /sentinel/api/connections/gmail/oauth/start`: Outlook device-code sign-in with the user's own client ID.
pub async fn gmail_oauth_start(State(ctx): State<App>, body: Bytes) -> JsonResult {
    let mut b = body_obj(&body)?;
    b["provider"] = json!("outlook");
    let (email, _provider, dname, mut servers) = mail_form(&b)?;
    outlook_overrides(&ctx.cfg.endpoints, &mut servers)?;
    let client_id = s_or(&b, "client_id", "").trim().to_string();
    let tenant = mailproviders::ms_tenant(&email);
    let d = ctx
        .ms_login
        .device_start(&client_id, &tenant)
        .await
        .map_err(|e| ApiError::new(400, e.0.clone()))?;
    let flow = token_urlsafe(12);
    {
        let mut flows = ctx.ms_flows.lock();
        let now = now_ts();
        flows.retain(|_, v| py_float(&g(v, "expires_at")).unwrap_or(0.0) >= now);
        let mut rec = d.clone();
        for (k, v) in [
            ("client_id", json!(client_id)),
            ("tenant", json!(tenant)),
            ("email", json!(email)),
            ("display_name", json!(dname)),
            ("servers", servers),
            ("next_poll", json!(0.0)),
        ] {
            rec[k] = v;
        }
        flows.insert(flow.clone(), rec);
    }
    ctx.audit(
        "user",
        "credential.gmail.oauth_start",
        audit_user("gmail", "success", Some(json!({"email": email}))),
    );
    let expires_in = (py_float(&g(&d, "expires_at")).unwrap_or(0.0) - now_ts()) as i64;
    Ok(Json(json!({
        "flow": flow, "user_code": g(&d, "user_code"), "verification_uri": g(&d, "verification_uri"),
        "expires_in": expires_in, "interval": g(&d, "interval"),
    })))
}

/// `GET /sentinel/api/connections/gmail/oauth/{flow}`: poll the device-code sign-in.
pub async fn gmail_oauth_poll(State(ctx): State<App>, Path(flow): Path<String>) -> JsonResult {
    poll_flow(&ctx, &flow).await
}

/// `GET /sentinel/api/connections/gmail/oauth/start` (the static POST route shadows the `{flow}` pattern).
pub async fn gmail_oauth_poll_start(State(ctx): State<App>) -> JsonResult {
    poll_flow(&ctx, "start").await
}

async fn poll_flow(ctx: &App, flow: &str) -> JsonResult {
    let f = {
        let mut flows = ctx.ms_flows.lock();
        let Some(f) = flows.get_mut(flow) else {
            return Err(ApiError::new(
                404,
                "登录流程不存在或已结束，请重新开始 (flow not found)",
            ));
        };
        let now = now_ts();
        if py_float(&g(f, "expires_at")).unwrap_or(0.0) < now {
            flows.remove(flow);
            return Ok(Json(
                json!({"status": "expired", "error": "登录码已过期，请重新开始 (code expired)"}),
            ));
        }
        if now < py_float(&g(f, "next_poll")).unwrap_or(0.0) {
            return Ok(Json(json!({"status": "pending"})));
        }
        f["next_poll"] = json!(now + py_float(&g(f, "interval")).unwrap_or(5.0));
        f.clone()
    };
    let tok = match ctx
        .ms_login
        .device_poll(
            &get_str(&f, "client_id"),
            &get_str(&f, "tenant"),
            &get_str(&f, "device_code"),
        )
        .await
    {
        Err(e) => {
            ctx.ms_flows.lock().remove(flow);
            return Ok(Json(json!({"status": "error", "error": e.0})));
        }
        Ok(None) => return Ok(Json(json!({"status": "pending"}))),
        Ok(Some(t)) => t,
    };
    ctx.ms_flows.lock().remove(flow);
    if !truthy(&g(&tok, "refresh_token")) {
        return Ok(Json(
            json!({"status": "error", "error": "微软没有返回 refresh token：请确认权限里包含 offline_access (no refresh token)"}),
        ));
    }
    let secret = json!({"oauth": {"client_id": f["client_id"], "tenant": f["tenant"], "refresh_token": tok["refresh_token"],
                                  "_access_token": super::g_or(&tok, "access_token", json!(""))}});
    match mail_connect(
        ctx,
        &get_str(&f, "email"),
        "outlook",
        &get_str(&f, "display_name"),
        &f["servers"],
        secret,
    )
    .await
    {
        Ok(Json(res)) => {
            let mut m = Map::new();
            m.insert("status".into(), json!("done"));
            if let Value::Object(o) = res {
                m.extend(o);
            }
            Ok(Json(Value::Object(m)))
        }
        Err(e) => Ok(Json(json!({"status": "error", "error": e.detail}))),
    }
}

/// `POST /sentinel/api/connections/gmail/test`.
pub async fn gmail_test(State(ctx): State<App>, body: Bytes) -> JsonResult {
    let b: Value = serde_json::from_slice(&body).unwrap_or(json!({}));
    let account = b.get("account").filter(|v| !v.is_null()).map(py_str);
    match deps::gmail_test(&ctx, account.as_deref(), None).await {
        Ok((email, info)) => Ok(Json(json!({"ok": true, "account": email, "test": info}))),
        Err(e) => Ok(Json(json!({"ok": false, "error": e.msg}))),
    }
}

/// `DELETE /sentinel/api/connections/gmail/accounts/{aid}`.
pub async fn gmail_remove(State(ctx): State<App>, Path(aid): Path<String>) -> JsonResult {
    let acc = mailboxes::find(&ctx.store, Some(&aid)).map_err(ierr)?;
    mailboxes::remove_account(&ctx.store, &aid).map_err(ierr)?;
    let email = acc.map(|a| a["email"].clone()).unwrap_or(json!(""));
    ctx.audit(
        "user",
        "credential.gmail.delete",
        audit_user(
            "gmail",
            "success",
            Some(json!({"account": aid, "email": email})),
        ),
    );
    Ok(Json(conn_view(&ctx, "gmail")?))
}

/// `POST /sentinel/api/connections/gmail/default`.
pub async fn gmail_default(State(ctx): State<App>, body: Bytes) -> JsonResult {
    let b = body_obj(&body)?;
    if !mailboxes::set_default(&ctx.store, &s_or(&b, "account", "")).map_err(ierr)? {
        return Err(ApiError::new(404, "account not found"));
    }
    ctx.audit(
        "user",
        "connection.gmail.default",
        audit_user(
            "gmail",
            "success",
            Some(json!({"account": g(&b, "account")})),
        ),
    );
    Ok(Json(conn_view(&ctx, "gmail")?))
}

// ================================================================== Telegram

fn saved_bot_token(ctx: &Ctx) -> String {
    ctx.store
        .get_secret("cred_telegram_1")
        .ok()
        .flatten()
        .map(|s| get_str(&s, "bot_token"))
        .unwrap_or_default()
}

/// `POST /sentinel/api/connections/telegram/credential`.
pub async fn telegram_cred(State(ctx): State<App>, body: Bytes) -> JsonResult {
    let b = body_obj(&body)?;
    let mut tok = s_or(&b, "bot_token", "").trim().to_string();
    let chat = s_or(&b, "chat_id", "").trim().to_string();
    let old = saved_bot_token(&ctx);
    if tok.is_empty() {
        tok = old.clone();
    }
    if tok.is_empty() || chat.is_empty() {
        return Err(ApiError::new(400, "需要 bot token 和 chat id"));
    }
    if !CHAT_RE.is_match(&chat) {
        return Err(ApiError::new(
            400,
            "chat id 应该是一串数字（可点「检测 Chat ID」自动获取）",
        ));
    }
    let store = &ctx.store;
    store
        .put_secret("telegram", &json!({"bot_token": tok}), None)
        .map_err(ierr)?;
    store
        .save_connection(
            "telegram",
            Some(&json!({"chat_id": chat})),
            Some(&json!({"notify": true, "control": true})),
            Some(true),
        )
        .map_err(ierr)?;
    if old != tok {
        // new bot: forget the old update offset / cached username
        store.kv_set("tg_offset", &json!(0)).map_err(ierr)?;
        if let Some(bot) = ctx.bot() {
            bot.clear_username();
        }
    }
    if let Err(e) = deps::telegram_send(
        &ctx,
        "✅ OMuse 已连接 Telegram。直接给我发消息就能布置任务，发 /help 查看用法。\n\
         Connected — message me to give OMuse a task, /help for commands.",
    )
    .await
    {
        ctx.audit(
            "user",
            "credential.telegram.set",
            audit_user("telegram", "failed", Some(json!({"error": e.to_string()}))),
        );
        return Err(ApiError::new(400, e.to_string()));
    }
    ctx.audit(
        "user",
        "credential.telegram.set",
        audit_user("telegram", "success", None),
    );
    Ok(Json(
        json!({"ok": true, "connection": conn_view(&ctx, "telegram")?}),
    ))
}

/// `GET /sentinel/api/telegram/status`.
pub async fn telegram_status(State(ctx): State<App>) -> JsonResult {
    let bot = ctx.bot();
    let mut st = bot
        .as_ref()
        .map(|b| b.status_value())
        .filter(Value::is_object)
        .unwrap_or(json!({}));
    st["configured"] = json!(bot.as_ref().is_some_and(|b| b.configured()));
    st["tracking"] = json!(bot.as_ref().map(|b| b.tracked_count()).unwrap_or(0));
    Ok(Json(st))
}

/// `POST /sentinel/api/connections/telegram/detect`: find the chat id of whoever sent /start to the bot.
pub async fn telegram_detect(State(ctx): State<App>, body: Bytes) -> JsonResult {
    let b = body_obj(&body)?;
    let saved = saved_bot_token(&ctx);
    let mut tok = s_or(&b, "bot_token", "").trim().to_string();
    if tok.is_empty() {
        tok = saved.clone();
    }
    if tok.is_empty() {
        return Err(ApiError::new(400, "请先填写 bot token"));
    }
    let mut chats: Vec<(String, Value)> = Vec::new();
    let mut put = |id: String, name: Value| match chats.iter_mut().find(|(k, _)| *k == id) {
        Some(e) => e.1 = name,
        None => chats.push((id, name)),
    };
    if ctx.bot().is_some_and(|bt| bt.running()) && tok == saved {
        // our own poll loop consumes the updates: use the private chats it saw and rejected
        let mut seen = ctx
            .store
            .kv_get("tg_seen_chats")
            .ok()
            .flatten()
            .and_then(|v| v.as_array().cloned())
            .unwrap_or_default();
        seen.sort_by(|a, b| {
            let (x, y) = (
                py_float(&g(a, "ts")).unwrap_or(0.0),
                py_float(&g(b, "ts")).unwrap_or(0.0),
            );
            x.partial_cmp(&y).unwrap_or(std::cmp::Ordering::Equal)
        });
        for x in seen {
            put(
                py_str(&g(&x, "chat_id")),
                super::g_or(&x, "name", json!("")),
            );
        }
    } else {
        let r = ctx
            .http
            .get(format!("{}/bot{}/getUpdates", ctx.cfg.telegram.api, tok))
            .query(&[("timeout", "0")])
            .timeout(Duration::from_secs(20))
            .send()
            .await
            .map_err(ierr)?;
        let code = r.status().as_u16();
        let bytes = r.bytes().await.map_err(ierr)?;
        let data: Value = if bytes.is_empty() {
            json!({})
        } else {
            serde_json::from_slice(&bytes).map_err(ierr)?
        };
        if !data.get("ok").is_some_and(truthy) {
            let why = match data.get("description").filter(|d| truthy(d)) {
                Some(d) => py_str(d),
                None => code.to_string(),
            };
            return Err(ApiError::new(400, format!("Telegram: {why}")));
        }
        for up in data
            .get("result")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
        {
            let ch = up
                .get("message")
                .and_then(|m| m.get("chat"))
                .cloned()
                .unwrap_or(json!({}));
            if ch.get("type").and_then(Value::as_str) == Some("private") {
                let parts: Vec<String> = ["first_name", "last_name"]
                    .iter()
                    .filter_map(|k| ch.get(*k).filter(|v| truthy(v)).map(py_str))
                    .collect();
                let name = if parts.is_empty() {
                    super::g_or(&ch, "username", json!(""))
                } else {
                    json!(parts.join(" "))
                };
                put(py_str(&g(&ch, "id")), name);
            }
        }
    }
    let list: Vec<Value> = chats
        .into_iter()
        .map(|(k, v)| json!({"chat_id": k, "name": v}))
        .collect();
    Ok(Json(json!({"chats": list})))
}

// ================================================================== Notion / Slack

/// `POST /sentinel/api/connections/notion/credential`.
pub async fn notion_cred(State(ctx): State<App>, body: Bytes) -> JsonResult {
    let b = body_obj(&body)?;
    let token = s_or(&b, "token", "").trim().to_string();
    if !NOTION_RE.is_match(&token) {
        return Err(ApiError::new(
            400,
            "请粘贴 Notion 集成令牌（以 ntn_ 或 secret_ 开头）(integration token)",
        ));
    }
    let n = Notion::new(ctx.http.clone(), &ctx.cfg.endpoints.notion, &token);
    let r = async {
        Ok::<_, omuse_apps::notion::NotionError>((n.me().await?, n.search("", "", 5).await?))
    }
    .await;
    let (me, found) = match r {
        Ok(x) => x,
        Err(e) => {
            ctx.audit(
                "user",
                "credential.notion.set",
                audit_user(
                    "notion",
                    "failed",
                    Some(json!({"error": take_chars(&e.to_string(), 300)})),
                ),
            );
            return Err(ApiError::new(400, e.to_string()));
        }
    };
    let ws = me
        .get("bot")
        .and_then(|x| x.get("workspace_name"))
        .filter(|v| truthy(v))
        .map(py_str)
        .unwrap_or_default();
    let store = &ctx.store;
    store
        .put_secret("notion", &json!({"token": token}), None)
        .map_err(ierr)?;
    store
        .save_connection(
            "notion",
            Some(&json!({"workspace": ws, "bot_id": super::g_or(&me, "id", json!(""))})),
            None,
            Some(true),
        )
        .map_err(ierr)?;
    ctx.audit(
        "user",
        "credential.notion.set",
        audit_user("notion", "success", Some(json!({"workspace": ws}))),
    );
    let visible: Vec<Value> = found.iter().map(|f| f["title"].clone()).collect();
    Ok(Json(
        json!({"ok": true, "workspace": ws, "bot": super::g_or(&me, "name", json!("")), "visible": visible}),
    ))
}

/// `POST /sentinel/api/connections/slack/credential`.
pub async fn slack_cred(State(ctx): State<App>, body: Bytes) -> JsonResult {
    let b = body_obj(&body)?;
    let token = s_or(&b, "token", "").trim().to_string();
    if !SLACK_RE.is_match(&token) {
        return Err(ApiError::new(
            400,
            "请粘贴 Slack 令牌：机器人令牌 xoxb-… 或用户令牌 xoxp-… (bot or user token)",
        ));
    }
    let sl = Slack::new(ctx.http.clone(), &ctx.cfg.endpoints.slack, &token);
    let r = async {
        Ok::<_, omuse_apps::slack::SlackError>((sl.auth_test().await?, sl.channels(50).await?))
    }
    .await;
    let (info, chans) = match r {
        Ok(x) => x,
        Err(e) => {
            ctx.audit(
                "user",
                "credential.slack.set",
                audit_user(
                    "slack",
                    "failed",
                    Some(json!({"error": take_chars(&e.to_string(), 300)})),
                ),
            );
            return Err(ApiError::new(400, e.to_string()));
        }
    };
    let store = &ctx.store;
    store
        .put_secret("slack", &json!({"token": token}), None)
        .map_err(ierr)?;
    let conf = super::obj(
        ["team", "user", "user_id", "bot_id", "token_type"]
            .iter()
            .map(|k| (*k, super::g_or(&info, k, json!(""))))
            .collect(),
    );
    store
        .save_connection("slack", Some(&conf), None, Some(true))
        .map_err(ierr)?;
    ctx.audit(
        "user",
        "credential.slack.set",
        audit_user(
            "slack",
            "success",
            Some(json!({"team": g(&info, "team"), "type": g(&info, "token_type")})),
        ),
    );
    let mut m = Map::new();
    m.insert("ok".into(), json!(true));
    if let Value::Object(o) = info {
        m.extend(o);
    }
    m.insert("channels".into(), json!(chans.len()));
    let member_of: Vec<Value> = chans
        .iter()
        .filter(|c| truthy(&c["is_member"]))
        .take(20)
        .map(|c| c["name"].clone())
        .collect();
    m.insert("member_of".into(), json!(member_of));
    Ok(Json(Value::Object(m)))
}

// ================================================================== Google Calendar / iCal

/// The address the user's browser really uses when it matches the server's view (`given` from the form).
fn browser_origin(headers: &HeaderMap, b: &Value) -> String {
    let origin = req_origin(headers);
    let given = s_or(b, "origin", "")
        .trim()
        .trim_end_matches('/')
        .to_string();
    let host_of = |s: &str| {
        s.split_once("://")
            .map(|(_, r)| r.to_string())
            .unwrap_or_default()
    };
    if ORIGIN_RE.is_match(&given) && host_of(&given) == host_of(&origin) {
        return given; // scheme as seen behind the proxy
    }
    origin
}

fn client_inputs(b: &Value, old: &Value) -> Result<(String, String), ApiError> {
    let cid = s_or(b, "client_id", "").trim().to_string();
    let mut secret = s_or(b, "client_secret", "").trim().to_string();
    if secret.is_empty() && get_str(old, "client_id") == cid {
        secret = get_str(old, "client_secret");
    }
    if !GCID_RE.is_match(&cid) {
        return Err(ApiError::new(
            400,
            "请粘贴 OAuth 客户端 ID（形如 1234-abc.apps.googleusercontent.com）(Client ID)",
        ));
    }
    if !GSECRET_RE.is_match(&secret) {
        return Err(ApiError::new(
            400,
            "请粘贴客户端密钥 Client secret（GOCSPX- 开头）",
        ));
    }
    Ok((cid, secret))
}

/// `POST /sentinel/api/connections/calendar/start`: save the user's OAuth client and return Google's consent URL.
pub async fn calendar_start(State(ctx): State<App>, headers: HeaderMap, body: Bytes) -> JsonResult {
    let b = body_obj(&body)?;
    let old = ctx
        .store
        .get_secret("cred_calendar_client")
        .map_err(ierr)?
        .unwrap_or(json!({}));
    let (cid, secret) = client_inputs(&b, &old)?;
    let redirect = browser_origin(&headers, &b) + CAL_CALLBACK;
    let state = token_urlsafe(24);
    let store = &ctx.store;
    store
        .put_secret(
            "calendar",
            &json!({"client_id": cid, "client_secret": secret}),
            Some("cred_calendar_client"),
        )
        .map_err(ierr)?;
    store
        .kv_set(
            "calendar_oauth",
            &json!({"state": state, "ts": now_ts(), "redirect": redirect}),
        )
        .map_err(ierr)?;
    ctx.audit(
        "user",
        "credential.calendar.start",
        audit_user(
            "calendar",
            "success",
            Some(json!({"client_id": take_chars(&cid, 24)})),
        ),
    );
    Ok(Json(
        json!({"auth_url": gcal::auth_url(&ctx.google, &cid, &redirect, &state, ""), "redirect_uri": redirect}),
    ))
}

/// `POST /sentinel/api/connections/calendar/google/start`: one-click sign-in through OMuse's client and the relay page.
pub async fn calendar_google_start(
    State(ctx): State<App>,
    headers: HeaderMap,
    body: Bytes,
) -> JsonResult {
    let Some(m) = gcal::managed(&ctx.google, Some(&ctx.store)) else {
        return Err(ApiError::new(
            400,
            "这个版本还没有内置 OMuse 的 Google 登录，请用下面的「自己的 Google Cloud 客户端」或 iCal 地址 (not available in this build)",
        ));
    };
    let b = body_obj(&body)?;
    let back = browser_origin(&headers, &b) + CAL_CALLBACK;
    let verifier = token_urlsafe(48);
    let b64 = base64::engine::general_purpose::URL_SAFE_NO_PAD;
    let challenge = b64.encode(sha2::Sha256::digest(verifier.as_bytes()));
    let st_json =
        serde_json::to_string(&json!({"r": back, "n": token_urlsafe(18)})).map_err(ierr)?;
    let state = b64.encode(st_json.as_bytes());
    ctx.store
        .kv_set(
            "calendar_oauth",
            &json!({"state": state, "ts": now_ts(), "redirect": m.relay, "mode": "managed", "verifier": verifier}),
        )
        .map_err(ierr)?;
    ctx.audit(
        "user",
        "credential.calendar.start",
        audit_user("calendar", "success", Some(json!({"mode": "managed"}))),
    );
    Ok(Json(
        json!({"auth_url": gcal::auth_url(&ctx.google, &m.client_id, &m.relay, &state, &challenge)}),
    ))
}

/// `POST /sentinel/api/connections/calendar/google/client`: the user's own Google client for one-click sign-in.
pub async fn calendar_google_client(State(ctx): State<App>, body: Bytes) -> JsonResult {
    let b = body_obj(&body)?;
    let store = &ctx.store;
    let old = match store
        .get_secret(gcal::BOX_CLIENT)
        .map_err(ierr)?
        .filter(truthy)
    {
        Some(v) => v,
        None => store
            .get_secret("cred_calendar_client")
            .map_err(ierr)?
            .unwrap_or(json!({})),
    };
    let (cid, secret) = client_inputs(&b, &old)?;
    let relay = ctx.google.relay_url.clone();
    store
        .put_secret(
            "calendar",
            &json!({"client_id": cid, "client_secret": secret, "relay": relay}),
            Some(gcal::BOX_CLIENT),
        )
        .map_err(ierr)?;
    ctx.audit(
        "user",
        "credential.calendar.box_client",
        audit_user(
            "calendar",
            "success",
            Some(json!({"client_id": take_chars(&cid, 24)})),
        ),
    );
    Ok(Json(
        json!({"ok": true, "relay": relay, "connection": conn_view(&ctx, "calendar")?}),
    ))
}

/// `DELETE /sentinel/api/connections/calendar/google/client`.
pub async fn calendar_google_client_delete(State(ctx): State<App>) -> JsonResult {
    ctx.store.delete_secret(gcal::BOX_CLIENT).map_err(ierr)?;
    ctx.audit(
        "user",
        "credential.calendar.box_client_delete",
        audit_user("calendar", "success", None),
    );
    Ok(Json(
        json!({"ok": true, "connection": conn_view(&ctx, "calendar")?}),
    ))
}

/// `POST /sentinel/api/connections/calendar/ical`: a read-only calendar by its private iCal address.
pub async fn calendar_ical(State(ctx): State<App>, body: Bytes) -> JsonResult {
    let b = body_obj(&body)?;
    let tz_hint = s_opt(&b, "time_zone").trim().to_string();
    let r = async {
        let url = ical::normalize_url(&s_or(&b, "url", ""), ctx.cfg.endpoints.ical_allow_http)?;
        let feed = ICalFeed::new(
            ctx.http.clone(),
            &url,
            if tz_hint.is_empty() { "UTC" } else { &tz_hint },
        );
        ical::fetch(&ctx.http, &url, true).await?;
        let info = feed.info().await?;
        Ok::<_, gcal::GCalError>((url, info))
    }
    .await;
    let (url, info) = r.map_err(|e| ApiError::new(400, e.to_string()))?;
    let tz = [get_str(&info, "time_zone"), tz_hint.clone(), "UTC".into()]
        .into_iter()
        .find(|s| !s.is_empty())
        .unwrap_or_default();
    let store = &ctx.store;
    store.delete_secret("cred_calendar_access").map_err(ierr)?;
    store.delete_secret("cred_calendar_client").map_err(ierr)?;
    store
        .put_secret("calendar", &json!({"ical_url": url}), None)
        .map_err(ierr)?;
    store
        .save_connection(
            "calendar",
            Some(&json!({"email": ical::label(&url), "time_zone": tz, "client_id": "", "mode": "ical", "calendar_name": info["name"]})),
            None,
            Some(true),
        )
        .map_err(ierr)?;
    let host = url.split('/').nth(2).unwrap_or("").to_string();
    ctx.audit(
        "user",
        "credential.calendar.set",
        audit_user(
            "calendar",
            "success",
            Some(json!({"mode": "ical", "host": host, "events": info["events"]})),
        ),
    );
    Ok(Json(
        json!({"ok": true, "name": info["name"], "time_zone": tz, "events": info["events"], "connection": conn_view(&ctx, "calendar")?}),
    ))
}

/// `_cal_page(ok, msg)`.
fn cal_page(ok: bool, msg: &str) -> Response {
    let color = if ok { "#1F6F5C" } else { "#B42318" };
    let body = format!(
        "<!doctype html><meta charset=utf-8><meta name=viewport content='width=device-width'><title>OMuse</title>\
         <body style='font-family:system-ui;padding:40px;max-width:560px;margin:auto'><h2 style='color:{color}'>\
         {} Google Calendar</h2><p>{}</p><p><a href='../../../../#connections'>\
         返回 OMuse 连接页 Back to Connections</a></p>{}</body>",
        if ok { "✓" } else { "✗" },
        py_html_escape(msg),
        if ok { "<script>setTimeout(()=>location.href='../../../../#connections',1500)</script>" } else { "" },
    );
    (
        [
            ("content-type", "text/html; charset=utf-8"),
            ("cache-control", "no-store"),
            ("referrer-policy", "no-referrer"),
        ],
        body,
    )
        .into_response()
}

const ACCESS_DENIED: &str = "Google 拒绝了授权 (access_denied)。最常见的原因：OMuse 的 Google 应用还是「测试 Testing」发布状态，\
只有被加入测试用户名单的 Google 账号才能连接。请让管理员在 Google Cloud Console →「OAuth 同意屏幕」里\
把应用发布为「正式版 In production」，或把你的 Google 账号加入「测试用户 Test users」；\
如果你是在同意页点了「取消」，重新点一次「连接 Google 日历」即可。\
 Google denied authorization. Most often OMuse's Google app is still in 'Testing' mode, so only accounts on \
the developer's test-user list can connect — ask the admin to publish the OAuth consent screen to Production, \
or add your Google account under Test users. If you clicked Cancel, just start the connect flow again.";

/// `GET /sentinel/api/connections/calendar/callback`: swap Google's one-time code for a refresh token (vault).
pub async fn calendar_callback(
    State(ctx): State<App>,
    Query(q): Query<HashMap<String, String>>,
) -> Response {
    match calendar_callback_inner(&ctx, &q).await {
        Ok(r) => r,
        Err(e) => e.into_response(),
    }
}

async fn calendar_callback_inner(
    ctx: &App,
    q: &HashMap<String, String>,
) -> Result<Response, ApiError> {
    let qs = |k: &str| q.get(k).cloned().unwrap_or_default();
    let (code, state, error) = (qs("code"), qs("state"), qs("error"));
    let store = &ctx.store;
    let pend = store
        .kv_get("calendar_oauth")
        .map_err(ierr)?
        .filter(truthy)
        .unwrap_or(json!({}));
    if !error.is_empty() {
        if error == "access_denied" {
            return Ok(cal_page(false, ACCESS_DENIED));
        }
        return Ok(cal_page(
            false,
            &format!("Google 返回错误 (error): {error}"),
        ));
    }
    let want = s_opt(&pend, "state");
    let fresh = now_ts() - py_float(&g(&pend, "ts")).unwrap_or(0.0) <= 900.0;
    let same = omuse_common::util::token_ok(Some(&state), &want);
    if state.is_empty() || want.is_empty() || !same || !fresh {
        return Ok(cal_page(false, "授权链接已失效或不匹配，请回到连接页重新点「连接 Google 日历」(state mismatch / expired)"));
    }
    store.kv_set("calendar_oauth", &json!({})).map_err(ierr)?;
    let managed = get_str(&pend, "mode") == "managed";
    let (cid, csecret, turl) = if managed {
        let Some(m) = gcal::managed(&ctx.google, Some(store)) else {
            return Ok(cal_page(
                false,
                "这个版本没有内置 OMuse 的 Google 登录 (managed client missing)",
            ));
        };
        let turl = gcal::token_url(&ctx.google, Some(&m));
        let sec = if m.broker.is_empty() {
            m.client_secret.clone()
        } else {
            String::new()
        };
        (m.client_id.clone(), sec, turl)
    } else {
        let cl = store
            .get_secret("cred_calendar_client")
            .map_err(ierr)?
            .unwrap_or(json!({}));
        (
            get_str(&cl, "client_id"),
            get_str(&cl, "client_secret"),
            String::new(),
        )
    };
    let r = async {
        let tok = gcal::exchange_code(
            &ctx.http,
            &ctx.google,
            &cid,
            &csecret,
            &code,
            &get_str(&pend, "redirect"),
            &get_str(&pend, "verifier"),
            &turl,
        )
        .await?;
        let exp = py_int(&g(&tok, "expires_in"))
            .filter(|n| *n != 0)
            .unwrap_or(3600);
        let gc = GCal::new(
            ctx.http.clone(),
            &ctx.google,
            &cid,
            &csecret,
            &get_str(&tok, "refresh_token"),
            "",
        )
        .with_access_token(&get_str(&tok, "access_token"), now_ts() + exp as f64)
        .with_token_url(&turl);
        let who = gc.userinfo().await?;
        let tz = gc.settings_tz().await;
        gc.list_events(&Value::Null, &Value::Null, "", "primary", 1)
            .await?;
        Ok::<_, gcal::GCalError>((tok, who, tz))
    }
    .await;
    let (tok, who, tz) = match r {
        Ok(x) => x,
        Err(e) => {
            ctx.audit(
                "user",
                "credential.calendar.set",
                audit_user(
                    "calendar",
                    "failed",
                    Some(json!({"error": take_chars(&e.to_string(), 300)})),
                ),
            );
            return Ok(cal_page(false, &e.to_string()));
        }
    };
    if managed {
        // the secret is OMuse's (built in or kept by the broker): store only what is ours
        store
            .put_secret(
                "calendar",
                &json!({"client_id": cid, "managed": true, "refresh_token": tok["refresh_token"]}),
                None,
            )
            .map_err(ierr)?;
        store.delete_secret("cred_calendar_client").map_err(ierr)?;
    } else {
        store
            .put_secret("calendar", &json!({"client_id": cid, "client_secret": csecret, "refresh_token": tok["refresh_token"]}), None)
            .map_err(ierr)?;
    }
    store.delete_secret("cred_calendar_access").map_err(ierr)?;
    let email = get_str(&who, "email");
    store
        .save_connection(
            "calendar",
            Some(&json!({"email": email, "time_zone": tz, "client_id": cid,
                         "mode": if managed { "google" } else { "google_own" }, "calendar_name": ""})),
            None,
            Some(true),
        )
        .map_err(ierr)?;
    ctx.audit(
        "user",
        "credential.calendar.set",
        audit_user(
            "calendar",
            "success",
            Some(json!({"email": email, "tz": tz})),
        ),
    );
    Ok(cal_page(
        true,
        &format!("已连接 {email}（时区 {tz}）。Connected."),
    ))
}

/// `POST /sentinel/api/connections/calendar/test`.
pub async fn calendar_test(State(ctx): State<App>) -> JsonResult {
    match deps::calendar_call(&ctx, "calendar_list_events", &json!({}), "").await {
        Err(e) => Ok(Json(json!({"ok": false, "error": e.to_string()}))),
        Ok(r) => {
            let n = r
                .get("content")
                .and_then(|c| c.get("events"))
                .and_then(Value::as_array)
                .map(|a| a.len())
                .unwrap_or(0);
            Ok(Json(json!({"ok": true, "upcoming": n})))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use omuse_common::config::EndpointsConfig;

    #[test]
    fn outlook_servers_overridden_from_config() {
        let mut servers = json!({"imap_host": "outlook.office365.com", "imap_port": 993, "imap_security": "ssl",
                                 "smtp_host": "smtp-mail.outlook.com", "smtp_port": 587, "smtp_security": "starttls"});
        let mut ep = EndpointsConfig::default();
        outlook_overrides(&ep, &mut servers).unwrap();
        assert_eq!(servers["imap_host"], "outlook.office365.com");
        ep.outlook_imap = "127.0.0.1:1143:none".into();
        outlook_overrides(&ep, &mut servers).unwrap();
        assert_eq!(
            (
                &servers["imap_host"],
                &servers["imap_port"],
                &servers["imap_security"]
            ),
            (&json!("127.0.0.1"), &json!(1143), &json!("none"))
        );
        assert_eq!(servers["smtp_host"], "smtp-mail.outlook.com");
        for bad in [
            "127.0.0.1:1143",
            "127.0.0.1:x:none",
            ":25:none",
            "h:25:plain",
        ] {
            ep.outlook_smtp = bad.into();
            assert!(outlook_overrides(&ep, &mut servers).is_err(), "{bad}");
        }
    }
}
