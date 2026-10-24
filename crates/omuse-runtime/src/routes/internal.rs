//! Internal callbacks from Sentinel (header `X-Persona-Runtime: <runtime token>`).

use axum::body::Bytes;
use axum::http::HeaderMap;
use axum::Json;
use omuse_common::http::H_RUNTIME;
use omuse_common::util::{take_chars, token_ok};
use serde_json::json;

use super::{body_obj, http, pstr_or, Fail, St, R};

/// `Depends(internal_auth)`: 401 unless the runtime token matches (checked before the body is read).
fn internal_auth(st: &St, headers: &HeaderMap) -> Result<(), Fail> {
    let given = headers.get(H_RUNTIME).and_then(|v| v.to_str().ok());
    if token_ok(given, &st.cfg.runtime_token()) {
        Ok(())
    } else {
        Err(http(401, "unauthorized"))
    }
}

pub async fn approval_resolved(st: St, headers: HeaderMap, body: Bytes) -> R {
    internal_auth(&st, &headers)?;
    let b = body_obj(&body)?;
    st.rt.on_approval_resolved(&b).await;
    Ok(Json(json!({"ok": true})))
}

pub async fn notify_user(st: St, headers: HeaderMap, body: Bytes) -> R {
    internal_auth(&st, &headers)?;
    let b = body_obj(&body)?;
    let title = pstr_or(&b, "title", "");
    let text = pstr_or(&b, "body", "");
    let n = st
        .rt
        .store
        .notify(take_chars(&title, 120), take_chars(&text, 1000), "", "info")?;
    st.rt
        .publish(json!({"kind": "notification", "notification": n}))
        .await;
    Ok(Json(json!({"ok": true})))
}

pub async fn takeover_ended(st: St, headers: HeaderMap, body: Bytes) -> R {
    internal_auth(&st, &headers)?;
    let b = body_obj(&body)?;
    st.rt.on_takeover_ended(&b).await;
    Ok(Json(json!({"ok": true})))
}
