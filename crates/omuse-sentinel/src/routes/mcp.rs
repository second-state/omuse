//! MCP servers (hub management) and the OAuth 2.1 sign-in for MCP servers (DialMCP, MCP hub), with the callback page.

use std::collections::HashMap;

use axum::body::Bytes;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use omuse_common::api::ApiError;
use omuse_common::util::{get_str, py_str, take_chars, truthy};
use omuse_mcp::mcp_hub::{self, AddServer, HubError};
use omuse_mcp::mcp_oauth;
use omuse_phone::dialmcp;
use omuse_sentinel_core::store::Audit;
use omuse_web::templates::py_html_escape;
use serde_json::{json, Map, Value};

use super::{
    body_obj, forwarded_host_raw, g, internal as ierr, obj, s_or, App, JsonResult, OAUTH_CALLBACK,
};

fn audit_user(resource: &str, result: &str, detail: Option<Value>) -> Audit {
    Audit {
        resource: resource.into(),
        result: result.into(),
        detail,
        ..Default::default()
    }
}

/// A hub failure: `HubError` → 400 with its text, anything else → 500.
fn hub_err(e: &anyhow::Error) -> ApiError {
    match e.downcast_ref::<HubError>() {
        Some(h) => ApiError::new(400, h.0.clone()),
        None => ierr(format!("{e:#}")),
    }
}

/// `_mcp_view(srv)`: a server record for the UI (no secrets).
pub fn mcp_view(ctx: &App, srv: &Value) -> Value {
    let keys = [
        "id",
        "name",
        "url",
        "transport",
        "enabled",
        "data_class",
        "auth",
        "header_name",
        "server_info",
        "protocol",
        "instructions",
        "last_sync",
        "last_error",
    ];
    let mut v = obj(keys.iter().map(|k| (*k, g(srv, k))).collect());
    let sid = get_str(srv, "id");
    v["has_credential"] = json!(ctx
        .store
        .has_secret(&mcp_hub::handle(&sid))
        .unwrap_or(false));
    let tkeys = [
        "name",
        "title",
        "description",
        "kind",
        "guessed",
        "mode",
        "status",
        "flags",
        "annotations",
        "previous_description",
    ];
    let tools: Vec<Value> = srv
        .get("tools")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|t| {
            let mut m = obj(tkeys.iter().map(|k| (*k, g(t, k))).collect());
            m["llm_name"] = json!(mcp_hub::tool_name(&sid, &get_str(t, "name")));
            m
        })
        .collect();
    v["tools"] = json!(tools);
    v
}

/// `GET /sentinel/api/mcp/servers`.
pub async fn list(State(ctx): State<App>) -> JsonResult {
    let servers = mcp_hub::servers(&ctx.store).map_err(ierr)?;
    Ok(Json(
        json!({"servers": servers.iter().map(|s| mcp_view(&ctx, s)).collect::<Vec<_>>()}),
    ))
}

/// `POST /sentinel/api/mcp/servers`: add a server (connects, lists and pins its tools).
pub async fn add(State(ctx): State<App>, body: Bytes) -> JsonResult {
    let b = body_obj(&body)?;
    let a = AddServer {
        name: s_or(&b, "name", ""),
        url: s_or(&b, "url", ""),
        auth_type: s_or(&b, "auth_type", "none"),
        token: s_or(&b, "token", ""),
        header_name: s_or(&b, "header_name", ""),
        data_class: s_or(&b, "data_class", "CONFIDENTIAL"),
        oauth: None,
    };
    let srv = match mcp_hub::add_server(&ctx.store, a).await {
        Ok(s) => s,
        Err(e) => {
            if let Some(h) = e.downcast_ref::<HubError>() {
                ctx.audit(
                    "user",
                    "mcp.add",
                    audit_user(
                        take_chars(&s_or(&b, "url", ""), 200),
                        "failed",
                        Some(json!({"error": take_chars(&h.0, 300)})),
                    ),
                );
            }
            return Err(hub_err(&e));
        }
    };
    let tools = srv["tools"].as_array().cloned().unwrap_or_default();
    let names: Vec<Value> = tools.iter().map(|t| t["name"].clone()).collect();
    let flagged: Vec<Value> = tools
        .iter()
        .filter(|t| truthy(&t["flags"]))
        .map(|t| t["name"].clone())
        .collect();
    ctx.audit(
        "user",
        "mcp.add",
        audit_user(
            &get_str(&srv, "id"),
            "success",
            Some(json!({"url": srv["url"], "transport": srv["transport"], "tools": names, "flagged": flagged})),
        ),
    );
    Ok(Json(mcp_view(&ctx, &srv)))
}

/// `POST /sentinel/api/mcp/servers/{sid}/refresh`.
pub async fn refresh(State(ctx): State<App>, Path(sid): Path<String>) -> JsonResult {
    let r = mcp_hub::refresh(&ctx.store, &sid)
        .await
        .map_err(|e| hub_err(&e))?;
    ctx.audit(
        "user",
        "mcp.refresh",
        audit_user(&sid, "success", Some(json!({"diff": r.diff}))),
    );
    let mut v = mcp_view(&ctx, &r.server);
    v["diff"] = r.diff;
    Ok(Json(v))
}

/// `POST /sentinel/api/mcp/servers/{sid}/test`.
pub async fn test(State(ctx): State<App>, Path(sid): Path<String>) -> JsonResult {
    Ok(Json(
        mcp_hub::test_server(&ctx.store, &sid)
            .await
            .map_err(|e| hub_err(&e))?,
    ))
}

/// `PUT /sentinel/api/mcp/servers/{sid}`: credentials, enabled, data class, name.
pub async fn update(State(ctx): State<App>, Path(sid): Path<String>, body: Bytes) -> JsonResult {
    let b = body_obj(&body)?;
    let store = &ctx.store;
    let has = |k: &str| b.get(k).is_some_and(|v| !v.is_null());
    if has("token") || has("auth_type") {
        let srv = mcp_hub::server(store, &sid)
            .map_err(ierr)?
            .unwrap_or(json!({}));
        let at = match b.get("auth_type") {
            Some(v) => py_str(v),
            None => s_or(&srv, "auth", "none"),
        };
        let header_name = match b.get("header_name") {
            Some(v) => py_str(v),
            None => s_or(&srv, "header_name", ""),
        };
        let headers = mcp_hub::build_headers(&at, &s_or(&b, "token", ""), &header_name)
            .map_err(|e| ApiError::new(400, e.0.clone()))?;
        let have_headers = !headers.is_empty();
        if have_headers {
            store
                .put_secret(
                    "mcp",
                    &json!({"headers": mcp_hub::headers_to_value(&headers)}),
                    Some(&mcp_hub::handle(&sid)),
                )
                .map_err(ierr)?;
        } else if at == "none" {
            store.delete_secret(&mcp_hub::handle(&sid)).map_err(ierr)?;
        }
        let b2 = b.clone();
        mcp_hub::update_with(store, &sid, move |x| {
            if have_headers || at == "none" {
                x.insert("auth".into(), json!(at));
            } else {
                let cur = x.get("auth").cloned().unwrap_or(Value::Null);
                x.insert("auth".into(), cur);
            }
            let hn = match b2.get("header_name") {
                Some(v) => py_str(v),
                None => x.get("header_name").map(py_str).unwrap_or_default(),
            };
            x.insert("header_name".into(), json!(hn));
        })
        .map_err(|e| hub_err(&e))?;
        mcp_hub::drop_session(&sid).await;
    }
    let enabled = b.get("enabled").filter(|v| !v.is_null()).map(truthy);
    let data_class = b.get("data_class").filter(|v| !v.is_null()).map(py_str);
    let name = b.get("name").filter(|v| !v.is_null()).map(py_str);
    let srv = mcp_hub::set_server(store, &sid, enabled, data_class.as_deref(), name.as_deref())
        .map_err(|e| hub_err(&e))?;
    let mut detail = Map::new();
    for k in ["enabled", "data_class", "name", "auth_type"] {
        if let Some(v) = b.get(k) {
            detail.insert(k.into(), v.clone());
        }
    }
    ctx.audit(
        "user",
        "mcp.update",
        audit_user(&sid, "success", Some(Value::Object(detail))),
    );
    Ok(Json(mcp_view(&ctx, &srv)))
}

/// `PUT /sentinel/api/mcp/servers/{sid}/tools/{tname}`: a tool's mode (`auto` / `ask` / `off`) and/or accept a change.
pub async fn tool_mode(
    State(ctx): State<App>,
    Path((sid, tname)): Path<(String, String)>,
    body: Bytes,
) -> JsonResult {
    let b = body_obj(&body)?;
    let mode = b.get("mode").filter(|v| !v.is_null()).map(py_str);
    let srv = mcp_hub::set_tool(
        &ctx.store,
        &sid,
        &tname,
        mode.as_deref(),
        b.get("accept").is_some_and(truthy),
    )
    .map_err(|e| hub_err(&e))?;
    let mut detail = Map::new();
    for k in ["mode", "accept"] {
        if let Some(v) = b.get(k) {
            detail.insert(k.into(), v.clone());
        }
    }
    ctx.audit(
        "user",
        "mcp.tool",
        audit_user(
            &format!("{sid}/{tname}"),
            "success",
            Some(Value::Object(detail)),
        ),
    );
    Ok(Json(mcp_view(&ctx, &srv)))
}

/// `DELETE /sentinel/api/mcp/servers/{sid}`.
pub async fn remove(State(ctx): State<App>, Path(sid): Path<String>) -> JsonResult {
    mcp_hub::remove_server(&ctx.store, &sid)
        .await
        .map_err(|e| hub_err(&e))?;
    ctx.audit("user", "mcp.remove", audit_user(&sid, "success", None));
    Ok(Json(json!({"ok": true})))
}

// ------------------------------------------------------------------ OAuth 2.1 sign-in for MCP servers

/// `_oauth_redirect(req, b)`: the callback must be this OMuse's own address.
pub fn oauth_redirect(headers: &HeaderMap, b: &Value) -> Result<String, ApiError> {
    mcp_oauth::check_redirect(
        &s_or(b, "redirect_uri", ""),
        &forwarded_host_raw(headers),
        OAUTH_CALLBACK,
    )
    .map_err(|e| ApiError::new(400, e.0.clone()))
}

/// `POST /sentinel/api/mcp/oauth/start`.
pub async fn oauth_start(State(_ctx): State<App>, headers: HeaderMap, body: Bytes) -> JsonResult {
    let b = body_obj(&body)?;
    let redirect = oauth_redirect(&headers, &b)?;
    let name = take_chars(s_or(&b, "name", "").trim(), 40).to_string();
    if name.is_empty() {
        return Err(ApiError::new(400, "请给这个服务器起个名字 (name required)"));
    }
    let start = |e: String| {
        ApiError::new(
            400,
            format!("无法开始 OAuth 登录 (cannot start sign-in): {e}"),
        )
    };
    let url =
        omuse_mcp::check_server_url(&s_or(&b, "url", "")).map_err(|e| start(e.msg.clone()))?;
    let extra =
        json!({"name": name, "url": url, "data_class": s_or(&b, "data_class", "CONFIDENTIAL")});
    let r = mcp_oauth::begin(&url, &redirect, "mcp", Some(extra))
        .await
        .map_err(|e| start(e.0.clone()))?;
    Ok(Json(json!({"auth_url": r.auth_url})))
}

/// `_oauth_page(ok, msg)`: the page the provider's login returns to.
pub fn oauth_page(ok: bool, msg: &str) -> Response {
    let color = if ok { "#1a7f37" } else { "#c62828" };
    let body = format!(
        "<!doctype html><meta charset=utf-8><meta name=viewport content='width=device-width,initial-scale=1'>\
         <title>OMuse</title><body style='font-family:system-ui;padding:32px;max-width:560px;margin:auto'>\
         <h2 style='color:{color}'>{} {}</h2>\
         <p>{}</p>\
         <p><a href='../../../#connections'>OMuse →</a></p>\
         <script>try{{window.opener&&window.opener.postMessage({{omuseOAuth:{}}},location.origin)}}catch(e){{}}\
         {}</script></body>",
        if ok { "✅" } else { "⚠️" },
        py_html_escape(msg),
        if ok { "可以关闭这个页面，回到 OMuse。You can close this tab and return to OMuse." } else { "请回到 OMuse 重试。Go back to OMuse and try again." },
        if ok { "true" } else { "false" },
        if ok { "setTimeout(()=>window.close(),1500)" } else { "" },
    );
    let status = if ok {
        StatusCode::OK
    } else {
        StatusCode::BAD_REQUEST
    };
    (status, [("content-type", "text/html; charset=utf-8")], body).into_response()
}

/// `GET /sentinel/api/oauth/callback`: back from the provider's login page (protected by the single-use `state`).
pub async fn oauth_callback(
    State(ctx): State<App>,
    Query(q): Query<HashMap<String, String>>,
) -> Response {
    let qs = |k: &str| q.get(k).cloned().unwrap_or_default();
    let err = format!("{} {}", qs("error"), qs("error_description"))
        .trim()
        .to_string();
    let (flow, rec) = match mcp_oauth::complete(&qs("state"), &qs("code"), &err).await {
        Ok(x) => x,
        Err(e) => {
            ctx.audit(
                "user",
                "credential.oauth.callback",
                audit_user(
                    "oauth",
                    "failed",
                    Some(json!({"error": take_chars(&e.0, 300)})),
                ),
            );
            return oauth_page(false, &e.0);
        }
    };
    let store = &ctx.store;
    if flow.purpose == "phone" {
        if let Err(e) = mcp_oauth::save(store, dialmcp::HANDLE, "phone", &rec, None) {
            return ierr(e).into_response();
        }
        ctx.phone.dial.drop_session().await;
        let acct = match ctx.phone.dial.whoami().await {
            Ok(a) => a,
            Err(e) => {
                ctx.audit(
                    "user",
                    "credential.dialmcp.whoami",
                    audit_user(
                        "phone",
                        "failed",
                        Some(json!({"error": take_chars(&e.to_string(), 300)})),
                    ),
                );
                json!({})
            }
        };
        let mut sec = store
            .get_secret(dialmcp::HANDLE)
            .ok()
            .flatten()
            .filter(Value::is_object)
            .unwrap_or(json!({}));
        sec["account"] = acct.clone();
        let r = store
            .put_secret("phone", &sec, Some(dialmcp::HANDLE))
            .and_then(|_| {
                store.save_connection(
                    "phone",
                    Some(&json!({"dialmcp_url": flow.server_url})),
                    Some(&json!({"call": true})),
                    Some(true),
                )
            });
        if let Err(e) = r {
            return ierr(e).into_response();
        }
        let phone = s_or(&acct, "phone", "");
        ctx.audit(
            "user",
            "credential.dialmcp.set",
            audit_user("phone", "success", Some(json!({"phone": phone}))),
        );
        return oauth_page(true, format!("DialMCP 已连接 connected {phone}").trim());
    }
    if flow.purpose == "mcp" {
        let x = &flow.extra;
        let url = s_or(x, "url", "");
        let a = AddServer {
            name: s_or(x, "name", ""),
            url: url.clone(),
            data_class: s_or(x, "data_class", "CONFIDENTIAL"),
            oauth: Some(rec),
            ..Default::default()
        };
        let srv = match mcp_hub::add_server(store, a).await {
            Ok(s) => s,
            Err(e) => {
                let Some(h) = e.downcast_ref::<HubError>() else {
                    return ierr(format!("{e:#}")).into_response();
                };
                ctx.audit(
                    "user",
                    "mcp.add",
                    audit_user(
                        take_chars(&url, 200),
                        "failed",
                        Some(json!({"error": take_chars(&h.0, 300), "auth": "oauth"})),
                    ),
                );
                return oauth_page(false, &h.0);
            }
        };
        let names: Vec<Value> = srv["tools"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .map(|t| t["name"].clone())
            .collect();
        ctx.audit(
            "user",
            "mcp.add",
            audit_user(
                &get_str(&srv, "id"),
                "success",
                Some(json!({"url": srv["url"], "auth": "oauth", "tools": names})),
            ),
        );
        return oauth_page(
            true,
            &format!("MCP「{}」已连接 connected", py_str(&srv["name"])),
        );
    }
    oauth_page(false, "unknown sign-in")
}
