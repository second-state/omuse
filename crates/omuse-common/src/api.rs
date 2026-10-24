//! HTTP error responses shaped like FastAPI's: `{"detail": "<message>"}`.

use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::{json, Value};

/// An error that becomes `{"detail": ...}` with a status code (FastAPI `HTTPException`).
#[derive(Debug, Clone)]
pub struct ApiError {
    pub status: u16,
    pub detail: Value,
    pub headers: Vec<(String, String)>,
}

impl ApiError {
    pub fn new(status: u16, detail: impl Into<String>) -> Self {
        Self {
            status,
            detail: Value::String(detail.into()),
            headers: vec![],
        }
    }
    /// `HTTPException(status, detail=<dict>)`.
    pub fn with_json(status: u16, detail: Value) -> Self {
        Self {
            status,
            detail,
            headers: vec![],
        }
    }
    pub fn header(mut self, k: &str, v: &str) -> Self {
        self.headers.push((k.to_string(), v.to_string()));
        self
    }
    pub fn bad_request(d: impl Into<String>) -> Self {
        Self::new(400, d)
    }
    pub fn unauthorized(d: impl Into<String>) -> Self {
        Self::new(401, d)
    }
    pub fn forbidden(d: impl Into<String>) -> Self {
        Self::new(403, d)
    }
    pub fn not_found(d: impl Into<String>) -> Self {
        Self::new(404, d)
    }
    pub fn conflict(d: impl Into<String>) -> Self {
        Self::new(409, d)
    }
    pub fn locked(d: impl Into<String>) -> Self {
        Self::new(423, d)
    }
    pub fn unprocessable(d: impl Into<String>) -> Self {
        Self::new(422, d)
    }
    pub fn internal(d: impl Into<String>) -> Self {
        Self::new(500, d)
    }
    pub fn unavailable(d: impl Into<String>) -> Self {
        Self::new(503, d)
    }
    pub fn detail_text(&self) -> String {
        match &self.detail {
            Value::String(s) => s.clone(),
            v => crate::pyjson::dumps(v),
        }
    }
}

impl std::fmt::Display for ApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.status, self.detail_text())
    }
}
impl std::error::Error for ApiError {}

impl From<anyhow::Error> for ApiError {
    fn from(e: anyhow::Error) -> Self {
        match e.downcast::<ApiError>() {
            Ok(api) => api,
            Err(e) => ApiError::new(500, format!("{:#}", e)),
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let status = StatusCode::from_u16(self.status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
        let mut headers = HeaderMap::new();
        for (k, v) in &self.headers {
            if let (Ok(name), Ok(val)) = (
                k.parse::<axum::http::HeaderName>(),
                v.parse::<axum::http::HeaderValue>(),
            ) {
                headers.insert(name, val);
            }
        }
        (status, headers, axum::Json(json!({"detail": self.detail}))).into_response()
    }
}

/// Handler result type.
pub type ApiResult<T = axum::Json<Value>> = Result<T, ApiError>;

/// `Json(value)` shorthand.
pub fn ok(v: Value) -> ApiResult {
    Ok(axum::Json(v))
}
