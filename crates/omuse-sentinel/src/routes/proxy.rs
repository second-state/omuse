//! The front door: `/api/{path}` reverse proxy to the Agent Runtime (streaming both ways, SSE unbuffered),
//! `/` (the page shell) and `/static/{name}` (the UI's files from memory).

use std::collections::HashMap;
use std::time::Duration;

use axum::body::Body;
use axum::extract::{Path, Query, Request, State};
use axum::http::{HeaderMap, HeaderName, HeaderValue, Method, StatusCode};
use axum::response::{IntoResponse, Response};
use futures_util::TryStreamExt;
use omuse_common::api::ApiError;
use omuse_common::util::take_chars;
use serde_json::{json, Value};

use super::{header, req_host, req_proto, App};

/// Hop-by-hop headers (and the runtime token) never forwarded either way (Python `HOP`).
pub const HOP: [&str; 11] = [
    "host",
    "content-length",
    "connection",
    "keep-alive",
    "transfer-encoding",
    "te",
    "trailer",
    "upgrade",
    "proxy-authorization",
    "proxy-authenticate",
    "x-persona-runtime",
];

fn unavailable() -> Response {
    (
        StatusCode::SERVICE_UNAVAILABLE,
        axum::Json(json!({"detail": "Agent Runtime 暂不可用 (runtime unavailable)"})),
    )
        .into_response()
}

fn err_name(e: &reqwest::Error) -> &'static str {
    if e.is_connect() {
        "ConnectError"
    } else if e.is_timeout() {
        "TimeoutException"
    } else if e.is_body() || e.is_decode() {
        "ReadError"
    } else if e.is_request() {
        "RemoteProtocolError"
    } else {
        "HTTPError"
    }
}

/// `/api/{path}` (GET, POST, PUT, PATCH, DELETE) → `<runtime>/api/{path}`.
pub async fn proxy(State(ctx): State<App>, Path(path): Path<String>, req: Request) -> Response {
    let method = req.method().clone();
    if !matches!(
        method,
        Method::GET | Method::HEAD | Method::POST | Method::PUT | Method::PATCH | Method::DELETE
    ) {
        return (
            StatusCode::METHOD_NOT_ALLOWED,
            axum::Json(json!({"detail": "Method Not Allowed"})),
        )
            .into_response();
    }
    let safe = method == Method::GET || method == Method::HEAD;
    if !safe && header(req.headers(), "x-persona-ui").as_deref() != Some("1") {
        return ApiError::new(403, "UI header required").into_response();
    }
    let mut url = format!("{}/api/{}", ctx.cfg.runtime_url(), path);
    if let Some(q) = req.uri().query().filter(|q| !q.is_empty()) {
        url.push('?');
        url.push_str(q);
    }
    let mut headers = reqwest::header::HeaderMap::new();
    for (k, v) in req.headers() {
        if !HOP.contains(&k.as_str()) {
            headers.append(k.clone(), v.clone());
        }
    }
    let rmethod =
        reqwest::Method::from_bytes(method.as_str().as_bytes()).unwrap_or(reqwest::Method::GET);
    let is_chat = path == "chat" && method == Method::POST;
    // Reads (retried once) and the chat call (its body is recorded) are buffered; other uploads stream through.
    let buffered = safe || is_chat;
    // The Python proxy read the whole body and sent it with its length. A streamed body goes out chunked unless the
    // length is passed on, and the runtime reads a request without Content-Length as having no body.
    let content_length = req.headers().get("content-length").cloned();
    let body = req.into_body();
    let (sent, req_body) = if buffered {
        let bytes = match axum::body::to_bytes(body, usize::MAX).await {
            Ok(b) => b,
            Err(_) => {
                return ApiError::new(400, "There was an error parsing the body").into_response()
            }
        };
        let build = || {
            ctx.proxy
                .request(rmethod.clone(), &url)
                .headers(headers.clone())
                .body(bytes.clone())
        };
        let mut r = build().send().await;
        if let Err(e) = &r {
            if safe && !e.is_timeout() {
                // a pooled connection closed under us (or the runtime is restarting): one retry for reads
                println!("[proxy] retry {} /api/{}: {}", method, path, err_name(e));
                tokio::time::sleep(Duration::from_millis(300)).await;
                r = build().send().await;
            }
        }
        (r, Some(bytes))
    } else {
        let stream = body.into_data_stream();
        if let Some(cl) = content_length {
            headers.insert(reqwest::header::CONTENT_LENGTH, cl);
        }
        let r = ctx
            .proxy
            .request(rmethod, &url)
            .headers(headers)
            .body(reqwest::Body::wrap_stream(stream))
            .send()
            .await;
        (r, None)
    };
    let r = match sent {
        Ok(r) => r,
        Err(e) => {
            println!(
                "[proxy] 503 {} /api/{}: {}: {}",
                method,
                path,
                err_name(&e),
                take_chars(&e.to_string(), 120)
            );
            return unavailable();
        }
    };
    let status = r.status();
    let mut out_headers = HeaderMap::new();
    for (k, v) in r.headers() {
        let name = k.as_str();
        if HOP.contains(&name) || name == "content-encoding" {
            continue;
        }
        if let (Ok(n), Ok(val)) = (
            HeaderName::from_bytes(name.as_bytes()),
            HeaderValue::from_bytes(v.as_bytes()),
        ) {
            out_headers.append(n, val);
        }
    }
    if is_chat && status.as_u16() == 200 {
        // remember the user's own words for the task they start (policy uses them for buttons the user named)
        let data = r.bytes().await.unwrap_or_default();
        let tid = serde_json::from_slice::<Value>(if data.is_empty() { b"{}" } else { &data })
            .ok()
            .and_then(|v| v.get("task_id").cloned())
            .filter(omuse_common::util::truthy);
        let msg = req_body
            .as_ref()
            .and_then(|b| {
                serde_json::from_slice::<Value>(if b.is_empty() { b"{}" } else { b }).ok()
            })
            .and_then(|v| v.get("message").cloned());
        if let (Some(tid), Some(Value::String(msg))) = (tid, msg) {
            let tid = omuse_common::util::py_str(&tid);
            let _ = ctx.store.set_user_request(&tid, &msg);
        }
        let mut resp = Response::new(Body::from(data));
        *resp.status_mut() = StatusCode::from_u16(status.as_u16()).unwrap_or(StatusCode::OK);
        *resp.headers_mut() = out_headers;
        return resp;
    }
    let stream = r.bytes_stream().map_err(std::io::Error::other);
    let mut resp = Response::new(Body::from_stream(stream));
    *resp.status_mut() = StatusCode::from_u16(status.as_u16()).unwrap_or(StatusCode::OK);
    *resp.headers_mut() = out_headers;
    resp
}

/// `GET /`: the page shell; also learns the public address users reach this OMuse at.
pub async fn index(State(ctx): State<App>, headers: HeaderMap) -> Response {
    let host = req_host(&headers);
    if !host.is_empty()
        && !(host.starts_with("127.")
            || host.starts_with("localhost")
            || host.starts_with("0.0.0.0"))
    {
        let base = format!("{}://{}", req_proto(&headers), host);
        if ctx.store.kv_get("public_base").ok().flatten() != Some(json!(base)) {
            let _ = ctx.store.kv_set("public_base", &json!(base));
        }
    }
    let v = ctx.assets.version();
    // `[web] dir` hot patch: its index.html wins over the built-in template
    let html = ctx
        .assets
        .dir()
        .and_then(|d| std::fs::read_to_string(d.join("index.html")).ok())
        .map(|h| omuse_web::assets::stamp(&h, &v))
        .unwrap_or_else(|| ctx.templates.render_index(&v));
    (
        [
            ("content-type", "text/html; charset=utf-8"),
            ("cache-control", "no-cache"),
        ],
        html,
    )
        .into_response()
}

/// `GET /static/{name}`: stamped URLs immutable, others revalidated, text pre-gzipped.
pub async fn static_file(
    State(ctx): State<App>,
    Path(name): Path<String>,
    method: Method,
    headers: HeaderMap,
    Query(q): Query<HashMap<String, String>>,
) -> Response {
    ctx.assets
        .response(&name, &method, &headers, q.get("v").map(String::as_str))
}
