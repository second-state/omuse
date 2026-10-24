//! User API: approvals list, vault, grants, grant suggestions, ledger, audit, browser view & takeover, health,
//! login password, subscription.

use std::collections::HashMap;
use std::time::Duration;

use axum::body::{Body, Bytes};
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use omuse_common::api::ApiError;
use omuse_common::guard;
use omuse_common::util::{get_str, take_chars, truthy, VERSION};
use omuse_sentinel_core::store::Audit;
use omuse_sentinel_core::vault::{self, VaultError};
use omuse_sentinel_core::{ledger, suggest};
use serde_json::{json, Map, Value};

use super::deps::{self, BrokerOut};
use super::internal::{q_int, run_reconcile};
use super::{
    action_error, body_obj, g, internal as ierr, notify_runtime, obj, py_float, req_host,
    req_proto, s_or, App, JsonResult,
};

fn audit_user(resource: &str, result: &str, detail: Option<Value>) -> Audit {
    Audit {
        resource: resource.into(),
        result: result.into(),
        detail,
        ..Default::default()
    }
}

// ------------------------------------------------------------------ grant suggestions, ledger

/// `GET /sentinel/api/grant_suggestions`.
pub async fn grant_suggestions(State(ctx): State<App>) -> JsonResult {
    Ok(Json(
        json!({"suggestions": suggest::suggestions(&ctx.store, None).map_err(ierr)?}),
    ))
}

/// `POST /sentinel/api/grant_suggestions`: accept (turn into a grant) or dismiss a suggestion.
pub async fn grant_suggestion_decide(State(ctx): State<App>, body: Bytes) -> JsonResult {
    let b = body_obj(&body)?;
    let k = super::s_opt(&b, "key");
    if !k.contains('|') {
        return Err(ApiError::new(400, "bad key"));
    }
    if b.get("accept").is_some_and(truthy) {
        let days = g(&b, "days");
        let d = if truthy(&days) {
            Some(py_float(&days).ok_or_else(|| ierr("could not convert to float"))?)
        } else {
            None
        };
        let gid = suggest::accept(&ctx.store, &k, d).map_err(ierr)?;
        ctx.audit(
            "user",
            "grant.from_suggestion",
            audit_user(
                k.split('|').next().unwrap_or(""),
                "success",
                Some(json!({"key": k, "grant_id": gid, "days": days})),
            ),
        );
        return Ok(Json(json!({"ok": true, "grant_id": gid})));
    }
    suggest::dismiss(&ctx.store, &k).map_err(ierr)?;
    Ok(Json(json!({"ok": true})))
}

/// `ledger.LABEL`.
fn ledger_labels() -> Value {
    let mut m = Map::new();
    for s in [
        "approved",
        "placed",
        "unconfirmed",
        "shipped",
        "delivered",
        "cancel_requested",
        "return_requested",
        "cancelled",
        "refunded",
    ] {
        m.insert(s.into(), json!(ledger::label(s)));
    }
    Value::Object(m)
}

/// `GET /sentinel/api/ledger?limit=`.
pub async fn ledger(
    State(ctx): State<App>,
    Query(q): Query<HashMap<String, String>>,
) -> JsonResult {
    let limit = q_int(&q, "limit", 100)?;
    let orders = ledger::entries(&ctx.store, limit.min(300), None).map_err(ierr)?;
    Ok(Json(json!({"orders": orders, "labels": ledger_labels()})))
}

/// `POST /sentinel/api/ledger/reconcile`.
pub async fn ledger_reconcile(State(ctx): State<App>) -> JsonResult {
    Ok(Json(run_reconcile(&ctx).await?))
}

// ------------------------------------------------------------------ approvals

/// `GET /sentinel/api/approvals?status=&limit=`.
pub async fn list_approvals(
    State(ctx): State<App>,
    Query(q): Query<HashMap<String, String>>,
) -> JsonResult {
    let limit = q_int(&q, "limit", 50)?;
    let aps = ctx
        .store
        .approvals(q.get("status").map(String::as_str), limit)
        .map_err(ierr)?;
    Ok(Json(json!({"approvals": aps})))
}

/// `GET /sentinel/api/approvals/{aid}/screenshot`.
pub async fn approval_shot(
    State(ctx): State<App>,
    Path(aid): Path<String>,
) -> Result<Response, ApiError> {
    let base = aid.rsplit('/').next().unwrap_or("");
    let p = ctx.shots.join(format!("{base}.jpg"));
    let data = std::fs::read(&p).map_err(|_| ApiError::new(404, "Not Found"))?;
    Ok(([("content-type", "image/jpeg")], data).into_response())
}

// ------------------------------------------------------------------ vault (values never leave Sentinel)

/// `GET /internal/vault`: item metadata for the runtime.
pub async fn internal_vault(State(ctx): State<App>) -> JsonResult {
    let items: Vec<Value> = vault::list_items(&ctx.store)
        .map_err(ierr)?
        .iter()
        .map(|it| {
            obj(["id", "label", "kind", "masked", "fields", "domains"]
                .iter()
                .map(|k| (*k, g(it, k)))
                .collect())
        })
        .collect();
    Ok(Json(json!({"items": items})))
}

/// `GET /sentinel/api/vault`.
pub async fn vault_list(State(ctx): State<App>) -> JsonResult {
    let mut labels = Map::new();
    for (k, v) in vault::FIELD_LABELS {
        labels.insert((*k).into(), json!(v));
    }
    Ok(Json(
        json!({"items": vault::list_items(&ctx.store).map_err(ierr)?, "kinds": vault::kinds(), "field_labels": labels}),
    ))
}

fn vault_save(ctx: &App, b: &Value, iid: Option<&str>) -> JsonResult {
    let it = match vault::save_item(&ctx.store, b, iid) {
        Ok(it) => it,
        Err(VaultError::Key(_)) => return Err(ApiError::new(404, "not found")),
        Err(VaultError::Value(e)) => return Err(ApiError::new(400, e)),
        Err(VaultError::Other(e)) => return Err(ierr(e)),
    };
    ctx.audit(
        "user",
        if iid.is_some() { "vault.update" } else { "vault.add" },
        audit_user(
            &get_str(&it, "id"),
            "",
            Some(json!({"label": it["label"], "kind": it["kind"], "fields": it["fields"], "domains": it["domains"]})),
        ),
    );
    Ok(Json(json!({"item": it})))
}

/// `POST /sentinel/api/vault`.
pub async fn vault_add(State(ctx): State<App>, body: Bytes) -> JsonResult {
    let b = body_obj(&body)?;
    vault_save(&ctx, &b, None)
}

/// `PUT /sentinel/api/vault/{iid}`.
pub async fn vault_update(
    State(ctx): State<App>,
    Path(iid): Path<String>,
    body: Bytes,
) -> JsonResult {
    let b = body_obj(&body)?;
    vault_save(&ctx, &b, Some(&iid))
}

/// `DELETE /sentinel/api/vault/{iid}`.
pub async fn vault_delete(State(ctx): State<App>, Path(iid): Path<String>) -> JsonResult {
    let it = vault::item(&ctx.store, &iid).map_err(ierr)?;
    if !vault::delete_item(&ctx.store, &iid).map_err(ierr)? {
        return Err(ApiError::new(404, "not found"));
    }
    let label = it.map(|i| i["label"].clone()).unwrap_or(Value::Null);
    ctx.audit(
        "user",
        "vault.delete",
        audit_user(&iid, "", Some(json!({"label": label}))),
    );
    Ok(Json(json!({"ok": true})))
}

// ------------------------------------------------------------------ grants

/// `GET /sentinel/api/grants`.
pub async fn grants(State(ctx): State<App>) -> JsonResult {
    Ok(Json(
        json!({"grants": ctx.store.active_grants().map_err(ierr)?}),
    ))
}

/// `DELETE /sentinel/api/grants/{gid}`.
pub async fn revoke(State(ctx): State<App>, Path(gid): Path<String>) -> JsonResult {
    ctx.store.revoke_grant(&gid).map_err(ierr)?;
    ctx.audit(
        "user",
        "grant.revoke",
        Audit {
            resource: gid,
            decision: "REVOKE".into(),
            result: "success".into(),
            ..Default::default()
        },
    );
    Ok(Json(json!({"ok": true})))
}

// ------------------------------------------------------------------ audit

/// `GET /sentinel/api/audit?task_id=&limit=&before=&actor=`.
pub async fn audit_list(
    State(ctx): State<App>,
    Query(q): Query<HashMap<String, String>>,
) -> JsonResult {
    let limit = q_int(&q, "limit", 200)?;
    let before = match q.get("before") {
        Some(_) => Some(q_int(&q, "before", 0)?),
        None => None,
    };
    let events = ctx
        .store
        .audit_list(
            q.get("task_id").map(String::as_str),
            limit.min(1000),
            before,
            q.get("actor").map(String::as_str),
        )
        .map_err(ierr)?;
    Ok(Json(json!({"events": events})))
}

/// `GET /sentinel/api/audit/verify`.
pub async fn audit_verify(State(ctx): State<App>) -> JsonResult {
    Ok(Json(ctx.store.audit_verify().map_err(ierr)?))
}

// ------------------------------------------------------------------ browser view & takeover

/// `GET /sentinel/api/browser/state`.
pub async fn b_state(State(ctx): State<App>) -> JsonResult {
    match deps::broker(&ctx, "GET", "/state", None, 10.0).await {
        Ok(r) => Ok(Json(r.json())),
        Err(e) => Ok(Json(json!({"mode": "offline", "error": e.to_string()}))),
    }
}

/// `GET /sentinel/api/browser/screenshot?task_id=`: JPEG, or 204 when there is none.
pub async fn b_shot(State(ctx): State<App>, Query(q): Query<HashMap<String, String>>) -> Response {
    let path = match q.get("task_id").filter(|t| !t.is_empty()) {
        Some(t) => format!("/screenshot?task_id={t}"),
        None => "/screenshot".to_string(),
    };
    match deps::broker(&ctx, "GET", &path, None, 15.0).await {
        Ok(BrokerOut::Bytes(img)) => (
            [
                ("content-type", "image/jpeg"),
                ("cache-control", "no-store"),
            ],
            img,
        )
            .into_response(),
        _ => Response::builder()
            .status(StatusCode::NO_CONTENT)
            .body(Body::empty())
            .expect("204"),
    }
}

/// `POST /sentinel/api/browser/takeover`.
pub async fn b_takeover(State(ctx): State<App>, body: Bytes) -> JsonResult {
    let b = body_obj(&body)?;
    let st = deps::broker(
        &ctx,
        "POST",
        "/user/takeover",
        Some(&json!({"task_id": g(&b, "task_id")})),
        200.0,
    )
    .await
    .map_err(|e| action_error(&e))?
    .json();
    ctx.audit(
        "user",
        "browser.takeover",
        Audit {
            task_id: get_str(&st, "takeover_task"),
            resource: get_str(&st, "url"),
            result: "success".into(),
            ..Default::default()
        },
    );
    Ok(Json(st))
}

/// `POST /sentinel/api/browser/release`.
pub async fn b_release(State(ctx): State<App>) -> JsonResult {
    let r = deps::broker(&ctx, "POST", "/user/release", Some(&json!({})), 20.0)
        .await
        .map_err(|e| action_error(&e))?
        .json();
    let rel = match r.get("released") {
        Some(v) if truthy(v) => v.clone(),
        _ => json!({}),
    };
    let tid = super::s_opt(&rel, "task_id");
    ctx.audit(
        "user",
        "browser.release",
        Audit {
            task_id: tid.clone(),
            result: "success".into(),
            ..Default::default()
        },
    );
    let requested = match rel.get("requested") {
        Some(v) if truthy(v) => super::g_or(v, "task_id", json!("")),
        _ => json!(""),
    };
    notify_runtime(
        &ctx,
        "/internal/takeover_ended",
        json!({"task_id": tid, "requested_task": requested}),
    )
    .await;
    Ok(Json(r))
}

/// `POST /sentinel/api/browser/view`.
pub async fn b_view(State(ctx): State<App>, body: Bytes) -> JsonResult {
    let b: Value = serde_json::from_slice(&body)
        .map_err(|_| ApiError::new(400, "There was an error parsing the body"))?;
    Ok(Json(
        deps::broker(&ctx, "POST", "/user/view", Some(&b), 10.0)
            .await
            .map_err(|e| action_error(&e))?
            .json(),
    ))
}

/// `POST /sentinel/api/browser/input` (what the user types during takeover is never logged).
pub async fn b_input(State(ctx): State<App>, body: Bytes) -> JsonResult {
    let b = body_obj(&body)?;
    let r = deps::broker(&ctx, "POST", "/user/input", Some(&b), 60.0)
        .await
        .map_err(|e| action_error(&e))?
        .json();
    if b.get("type").and_then(Value::as_str) == Some("navigate") {
        ctx.audit(
            "user",
            "browser.user_navigate",
            audit_user(&guard::domain_of(&s_or(&b, "url", "")), "success", None),
        );
    }
    Ok(Json(r))
}

// ------------------------------------------------------------------ front door

/// `GET /sentinel/api/health`.
pub async fn health() -> Json<Value> {
    Json(json!({"ok": true, "version": VERSION.as_str()}))
}

/// `GET /sentinel/api/password`: whether this install has its own login and whether it still runs on the default.
pub async fn password_status(State(ctx): State<App>) -> JsonResult {
    let p = &ctx.passwd;
    if !p.enabled() {
        return Ok(Json(json!({"enabled": false})));
    }
    Ok(Json(
        json!({"enabled": true, "default": p.is_default(), "user": p.user(), "min_length": omuse_sentinel_core::passwd::MIN_LEN}),
    ))
}

/// `POST /sentinel/api/password`: change the login password.
pub async fn password_change(State(ctx): State<App>, body: Bytes) -> JsonResult {
    let p = &ctx.passwd;
    if !p.enabled() {
        return Err(ApiError::new(404, "this install has no login of its own"));
    }
    let b = body_obj(&body)?;
    let current = super::s_opt(&b, "current");
    let new = super::s_opt(&b, "new");
    if !p.verify(p.user(), &current) {
        ctx.audit(
            "user",
            "password.change",
            audit_user(
                "login",
                "denied",
                Some(json!({"reason": "current password wrong"})),
            ),
        );
        tokio::time::sleep(Duration::from_secs(1)).await;
        return Err(ApiError::new(
            403,
            "当前密码不对 (current password is wrong)",
        ));
    }
    p.set_password(&new)
        .map_err(|e| ApiError::new(400, e.to_string()))?;
    ctx.audit(
        "user",
        "password.change",
        audit_user("login", "success", None),
    );
    Ok(Json(json!({"ok": true})))
}

/// `GET /sentinel/api/subscription`: whether Settings shows "Manage subscription", plus its state.
pub async fn subscription(State(ctx): State<App>) -> JsonResult {
    let Some(c) = omuse_apps::billing::config(&ctx.cfg) else {
        return Ok(Json(json!({"enabled": false})));
    };
    match omuse_apps::billing::status(&ctx.http, Some(&c)).await {
        Ok(st) => {
            let mut m = Map::new();
            m.insert("enabled".into(), json!(true));
            if let Value::Object(o) = st {
                m.extend(o);
            }
            Ok(Json(Value::Object(m)))
        }
        Err(e) => Ok(Json(json!({"enabled": true, "error": e.to_string()}))),
    }
}

/// `POST /sentinel/api/subscription/portal`: a Stripe billing-portal link.
pub async fn subscription_portal(State(ctx): State<App>, headers: HeaderMap) -> JsonResult {
    let Some(c) = omuse_apps::billing::config(&ctx.cfg) else {
        return Err(ApiError::new(
            404,
            "subscription management is not configured",
        ));
    };
    let url = format!("{}://{}/#settings", req_proto(&headers), req_host(&headers));
    match omuse_apps::billing::portal_url(&ctx.http, Some(&c), &url).await {
        Ok(u) => {
            ctx.audit(
                "user",
                "subscription.portal",
                audit_user("stripe", "success", None),
            );
            Ok(Json(json!({"url": u})))
        }
        Err(e) => {
            ctx.audit(
                "user",
                "subscription.portal",
                audit_user(
                    "stripe",
                    "failed",
                    Some(json!({"error": take_chars(&e.to_string(), 300)})),
                ),
            );
            Err(ApiError::new(502, e.to_string()))
        }
    }
}
