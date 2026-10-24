//! The ONLY public surface of OMuse: Telnyx's media stream and webhooks for phone calls.
//!
//! Served by Sentinel on its own port (`[voice] port`, default 8083) behind the public "phone" entrance, so nothing
//! else of Sentinel/OMuse becomes reachable without login. Media streams need a one-time token that exists only while
//! a call you approved is being dialled; webhooks are only trusted when they carry a valid Telnyx signature.
//!
//! Port of `app/sentinel/voice_app.py`: [`router`] is the FastAPI `app`; the sentinel assembly serves it on the voice port.

use std::sync::Arc;

use axum::body::Bytes;
use axum::extract::ws::WebSocketUpgrade;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use omuse_common::util::get_str;
use omuse_sentinel_core::store::Audit;
use serde_json::{json, Value};

use crate::phone::{verify_webhook, Phone};

/// The voice app: `GET /voice/health`, `WS /voice/stream/{token}`, `POST /voice/webhook`; everything else is 404.
pub fn router(phone: Arc<Phone>) -> Router {
    Router::new()
        .route("/voice/health", get(health))
        .route("/voice/stream/{token}", get(stream))
        .route("/voice/webhook", post(webhook))
        .fallback(nothing_else)
        .with_state(phone)
}

async fn health() -> &'static str {
    "omuse-voice ok"
}

async fn stream(
    State(phone): State<Arc<Phone>>,
    Path(token): Path<String>,
    ws: WebSocketUpgrade,
) -> Response {
    // Unknown, expired or already used token: refuse before the upgrade (Starlette's close-before-accept = HTTP 403).
    match phone.claim_stream_token(&token) {
        None => StatusCode::FORBIDDEN.into_response(),
        Some(call) => ws.on_upgrade(move |socket| phone.bridge(socket, call)),
    }
}

async fn webhook(State(phone): State<Arc<Phone>>, headers: HeaderMap, body: Bytes) -> Response {
    let pk = get_str(&phone.keys(), "telnyx_public_key");
    if pk.is_empty() {
        return Json(json!({"ok": true, "ignored": "no public key configured"})).into_response();
    }
    let hdr = |n: &str, d: &str| {
        headers
            .get(n)
            .and_then(|v| v.to_str().ok())
            .unwrap_or(d)
            .to_string()
    };
    if !verify_webhook(
        &pk,
        &hdr("telnyx-signature-ed25519", ""),
        &hdr("telnyx-timestamp", "0"),
        &body,
        300,
    ) {
        let _ = phone.store().audit(
            "sentinel",
            "phone.webhook_rejected",
            Audit {
                result: "denied".into(),
                detail: Some(json!({"len": body.len()})),
                ..Default::default()
            },
        );
        return (StatusCode::UNAUTHORIZED, Json(json!({"ok": false}))).into_response();
    }
    let raw: &[u8] = if body.is_empty() { b"{}" } else { &body };
    let payload: Value = match std::str::from_utf8(raw)
        .ok()
        .and_then(|s| serde_json::from_str(s).ok())
    {
        Some(v) => v,
        None => return (StatusCode::BAD_REQUEST, Json(json!({"ok": false}))).into_response(),
    };
    phone.on_webhook(&payload);
    Json(json!({"ok": true})).into_response()
}

async fn nothing_else() -> Response {
    (StatusCode::NOT_FOUND, "not found").into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use http_body_util::BodyExt;
    use omuse_common::config::VoiceConfig;
    use omuse_sentinel_core::store::Store;
    use std::future::IntoFuture;
    use tower::ServiceExt;

    fn app() -> (Router, Arc<Store>) {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(Store::new(dir.keep()).unwrap());
        let phone = Arc::new(Phone::new(store.clone(), VoiceConfig::default()));
        (router(phone), store)
    }

    async fn call(app: &Router, req: Request<Body>) -> (u16, String) {
        let r = app.clone().oneshot(req).await.unwrap();
        let st = r.status().as_u16();
        let b = r.into_body().collect().await.unwrap().to_bytes();
        (st, String::from_utf8_lossy(&b).into_owned())
    }

    #[tokio::test]
    async fn public_surface() {
        let (app, store) = app();
        let (st, body) = call(
            &app,
            Request::get("/voice/health").body(Body::empty()).unwrap(),
        )
        .await;
        assert_eq!((st, body.as_str()), (200, "omuse-voice ok"));
        for p in ["/sentinel/api/connections", "/api/tasks", "/"] {
            let (st, body) = call(&app, Request::get(p).body(Body::empty()).unwrap()).await;
            assert_eq!((st, body.as_str()), (404, "not found"), "{p}");
        }
        // the WebSocket upgrade needs a real connection: an unknown token is refused with 403 before the upgrade
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(axum::serve(listener, app.clone()).into_future());
        match tokio_tungstenite::connect_async(format!("ws://{addr}/voice/stream/not-a-token"))
            .await
        {
            Err(tokio_tungstenite::tungstenite::Error::Http(resp)) => {
                assert_eq!(resp.status().as_u16(), 403)
            }
            other => panic!("expected an HTTP 403 refusal, got {:?}", other.map(|_| ())),
        }
        // webhooks are ignored until a public key is set
        let (st, body) = call(
            &app,
            Request::post("/voice/webhook")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"data": {"event_type": "call.hangup"}}"#))
                .unwrap(),
        )
        .await;
        assert_eq!(st, 200);
        assert!(body.contains("\"ignored\""));
        // with a key: forged -> 401, signed -> 200, bad JSON -> 400
        use base64::Engine;
        use ed25519_dalek::{Signer, SigningKey};
        let sk = SigningKey::from_bytes(&[3u8; 32]);
        let pk = base64::engine::general_purpose::STANDARD.encode(sk.verifying_key().to_bytes());
        store
            .put_secret("phone", &json!({"telnyx_public_key": pk}), None)
            .unwrap();
        let ts = format!("{}", omuse_common::util::now_ts() as i64);
        let body_bytes =
            br#"{"data": {"event_type": "call.answered", "payload": {"client_state": ""}}}"#;
        let sign = |k: &SigningKey, b: &[u8]| {
            let mut m = format!("{ts}|").into_bytes();
            m.extend_from_slice(b);
            base64::engine::general_purpose::STANDARD.encode(k.sign(&m).to_bytes())
        };
        let bad = sign(&SigningKey::from_bytes(&[4u8; 32]), body_bytes);
        let (st, body) = call(
            &app,
            Request::post("/voice/webhook")
                .header("telnyx-signature-ed25519", bad)
                .header("telnyx-timestamp", &ts)
                .body(Body::from(&body_bytes[..]))
                .unwrap(),
        )
        .await;
        assert_eq!((st, body.as_str()), (401, r#"{"ok":false}"#));
        let good = sign(&sk, body_bytes);
        let (st, body) = call(
            &app,
            Request::post("/voice/webhook")
                .header("telnyx-signature-ed25519", &good)
                .header("telnyx-timestamp", &ts)
                .body(Body::from(&body_bytes[..]))
                .unwrap(),
        )
        .await;
        assert_eq!((st, body.as_str()), (200, r#"{"ok":true}"#));
        let junk = b"not json";
        let good = sign(&sk, junk);
        let (st, _) = call(
            &app,
            Request::post("/voice/webhook")
                .header("telnyx-signature-ed25519", good)
                .header("telnyx-timestamp", &ts)
                .body(Body::from(&junk[..]))
                .unwrap(),
        )
        .await;
        assert_eq!(st, 400);
        let (st, _) = call(
            &app,
            Request::post("/voice/webhook")
                .body(Body::from(&body_bytes[..]))
                .unwrap(),
        )
        .await;
        assert_eq!(st, 401, "no signature at all");
    }
}
