//! OpenAI-compatible chat client (Olares Router / llama.cpp / any endpoint). Replaceable by design
//! (`app/runtime/llm.py`).
//!
//! The HTTP layer sits behind [`LlmTransport`] so tests (and the agent's scripted model) can serve canned
//! responses; [`ReqwestTransport`] is the real one. Request bodies, retry rules and every error text the agent
//! surfaces to the user are the Python ones.

use std::collections::HashMap;
use std::sync::{Arc, LazyLock};
use std::time::Duration;

use futures::future::BoxFuture;
use omuse_common::util::{get_str, get_truthy, now_ts, py_str, take_chars, truthy};
use omuse_common::{pyjson, Config};
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use tokio::sync::Semaphore;

static THINK: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?s)<think>.*?</think>").unwrap());
static THINK_CAP: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?s)<think>(.*?)</think>").unwrap());
static TOOLCALL_TAG: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?s)<tool_call>\s*(\{.*?\})\s*</tool_call>").unwrap());
/// Qwen3 "XML" style that llama.cpp sometimes leaves in the text:
/// `<tool_call><function=x><parameter=k>v</parameter></function></tool_call>`
static TOOLCALL_XML: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?s)<tool_call>\s*<function=([\w.\-]+)>(.*?)</function>\s*(?:</tool_call>|$)")
        .unwrap()
});
static XML_PARAM: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?s)<parameter=([\w.\-]+)>\n?(.*?)\n?</parameter>").unwrap());
static ANY_TOOLCALL: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?s)<tool_call>.*?(?:</tool_call>|$)").unwrap());
static TRAILING_COMMA: LazyLock<Regex> = LazyLock::new(|| Regex::new(r",\s*([}\]])").unwrap());

/// The `(name, body)` pairs of `_TOOLCALL_XML.findall(text)`.
pub fn toolcall_xml(text: &str) -> Vec<(String, String)> {
    TOOLCALL_XML
        .captures_iter(text)
        .map(|c| (c[1].to_string(), c[2].to_string()))
        .collect()
}

/// The `(key, value)` pairs of `_XML_PARAM.findall(body)`.
pub fn xml_params(body: &str) -> Vec<(String, String)> {
    XML_PARAM
        .captures_iter(body)
        .map(|c| (c[1].to_string(), c[2].to_string()))
        .collect()
}

/// `_xml_value`: JSON when it parses, else the raw string.
pub fn xml_value(v: &str) -> Value {
    serde_json::from_str(v).unwrap_or_else(|_| Value::String(v.to_string()))
}

/// Remove tool-call markup a model wrote as plain text (it must never reach the user as the answer).
pub fn strip_tool_markup(text: &str) -> String {
    ANY_TOOLCALL.replace_all(text, "").trim().to_string()
}

/// Bearer key for the model endpoint in force: the user's own when Settings → Models overrides the system's, else
/// `[model] api_key` (`store::effective_model` decides; the Olares router needs none). Without settings, the
/// system's key. Returns the header as a `(name, value)` list: empty when there is no key.
pub fn auth_headers(settings: Option<&Value>, cfg: &Config) -> Vec<(String, String)> {
    let key = match settings {
        None => cfg.model.api_key.trim().to_string(),
        Some(s) => s
            .get("model_api_key")
            .filter(|v| truthy(v))
            .map(py_str)
            .unwrap_or_default()
            .trim()
            .to_string(),
    };
    if key.is_empty() {
        vec![]
    } else {
        vec![("Authorization".to_string(), format!("Bearer {}", key))]
    }
}

fn settings_auth(settings: &Value) -> Vec<(String, String)> {
    let key = settings
        .get("model_api_key")
        .filter(|v| truthy(v))
        .map(py_str)
        .unwrap_or_default()
        .trim()
        .to_string();
    if key.is_empty() {
        vec![]
    } else {
        vec![("Authorization".to_string(), format!("Bearer {}", key))]
    }
}

/// A model that can chat: the router's mode says so when it reports one (tts / audio / embedding models are never
/// picked), otherwise judge by the name. Models that are still loading are skipped.
pub fn pick_chat_model(models: &[Value], exclude: &str) -> String {
    for m in models {
        let mid = get_str(m, "id");
        if mid.is_empty() || mid == exclude {
            continue;
        }
        let mode = get_str(m, "mode").to_lowercase();
        if !mode.is_empty() && mode != "chat" {
            continue;
        }
        let readiness = {
            let r = get_str(m, "readiness");
            if r.is_empty() {
                "ready".to_string()
            } else {
                r.to_lowercase()
            }
        };
        if !["ready", "running", "loaded"].contains(&readiness.as_str()) {
            continue;
        }
        let low = mid.to_lowercase();
        if [
            "embed", "rerank", "whisper", "tts", "kokoro", "speech", "audio",
        ]
        .iter()
        .any(|k| low.contains(k))
        {
            continue;
        }
        return mid;
    }
    String::new()
}

// Retries. A Retry-After header is always obeyed (429, 403, 503 ... whatever the status), up to RETRY_AFTER_TRIES
// times and RETRY_AFTER_MAX seconds each. Any other failure (HTTP error, timeout, connection error) is retried
// RETRY_DELAYS times, after 5, 10 and 20 seconds. Only an over-long prompt is not retried: it cannot succeed as is.
pub const RETRY_DELAYS: [f64; 3] = [5.0, 10.0, 20.0];
pub const RETRY_AFTER_TRIES: u32 = 10;
pub const RETRY_AFTER_MAX: f64 = 300.0;

/// Seconds to wait as the server asked (delta-seconds or an HTTP date), capped; `None` when there is no header
/// (or it is unreadable).
pub fn retry_after(header: Option<&str>) -> Option<f64> {
    let v = header?.trim();
    let secs = match v.parse::<f64>() {
        Ok(f) => f,
        Err(_) => chrono::DateTime::parse_from_rfc2822(v).ok()?.timestamp() as f64 - now_ts(),
    };
    Some(secs.clamp(0.0, RETRY_AFTER_MAX))
}

/// `LLMError` / `LLMContextError`: the message is what the agent shows the user.
#[derive(Debug, Clone, thiserror::Error)]
pub enum LlmError {
    /// `LLMError`.
    #[error("{0}")]
    Failed(String),
    /// `LLMContextError`: the prompt is larger than the model server's context window; retrying the same request
    /// cannot help. `chars` is the size of the message contents.
    #[error("{msg}")]
    Context { msg: String, chars: usize },
}

impl LlmError {
    /// Python's `type(e).__name__`.
    pub fn name(&self) -> &'static str {
        match self {
            LlmError::Failed(_) => "LLMError",
            LlmError::Context { .. } => "LLMContextError",
        }
    }
    pub fn is_context(&self) -> bool {
        matches!(self, LlmError::Context { .. })
    }
}

fn context_exceeded(text: &str) -> bool {
    let t = text.to_lowercase();
    (t.contains("context")
        && [
            "exceed",
            "too long",
            "too large",
            "maximum context",
            "context length",
            "context size",
            "n_ctx",
        ]
        .iter()
        .any(|k| t.contains(k)))
        || t.contains("prompt is too long")
}

fn parse_loose(s: &str) -> Option<Value> {
    serde_json::from_str(s).ok()
}

/// Tool-call arguments from whatever the model sent: a dict as is, JSON text (also with trailing commas or
/// wrapped in prose), else `{"_unparsed": ...}`.
pub fn parse_args(raw: &Value) -> Value {
    static OBJ: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?s)\{.*\}").unwrap());
    if raw.is_object() {
        return raw.clone();
    }
    if !truthy(raw) {
        return json!({});
    }
    let s = py_str(raw).trim().to_string();
    if let Some(v) = parse_loose(&s) {
        return if v.is_object() {
            v
        } else {
            json!({"value": v})
        };
    }
    let s2 = TRAILING_COMMA.replace_all(&s, "$1");
    if let Some(v) = parse_loose(&s2) {
        return v;
    }
    if let Some(m) = OBJ.find(&s) {
        if let Some(v) = parse_loose(m.as_str()) {
            return v;
        }
    }
    json!({"_unparsed": take_chars(&s, 500)})
}

/// Pull the first JSON object/array out of a model reply (handles ```json fences).
pub fn extract_json(text: &str) -> Option<Value> {
    static FENCE: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"(?s)```(?:json)?\s*(.*?)```").unwrap());
    if text.is_empty() {
        return None;
    }
    let text = THINK.replace_all(text, "");
    let cand: &str = match FENCE.captures(&text) {
        Some(c) => c.get(1).map(|m| m.as_str()).unwrap_or(""),
        None => &text,
    };
    for (opener, closer) in [('{', '}'), ('[', ']')] {
        let (Some(i), Some(j)) = (cand.find(opener), cand.rfind(closer)) else {
            continue;
        };
        if j <= i {
            continue;
        }
        let piece = &cand[i..=j];
        if let Some(v) = parse_loose(piece) {
            return Some(v);
        }
        if let Some(v) = parse_loose(&TRAILING_COMMA.replace_all(piece, "$1")) {
            return Some(v);
        }
    }
    None
}

// ------------------------------------------------------------------------------------------------ transport

/// One HTTP exchange with the model server, as the client needs it.
#[derive(Debug, Clone)]
pub struct HttpRequest {
    pub method: &'static str,
    pub url: String,
    pub headers: Vec<(String, String)>,
    /// JSON body (`POST .../chat/completions`).
    pub json: Option<Value>,
    /// Multipart body for `/audio/transcriptions`: `(model, filename, bytes)`.
    pub audio: Option<(String, String, Vec<u8>)>,
    pub timeout: f64,
    pub connect_timeout: f64,
}

/// What came back: status, the `Retry-After` header and the body text.
#[derive(Debug, Clone, Default)]
pub struct HttpResponse {
    pub status: u16,
    pub retry_after: Option<String>,
    pub text: String,
}

impl HttpResponse {
    pub fn json(&self) -> Option<Value> {
        serde_json::from_str(&self.text).ok()
    }
}

/// The HTTP layer. `Err(text)` is a transport failure (`"ConnectError: ..."`, `"TimeoutException: ..."`), which is
/// retried like Python's `httpx.TransportError` / `httpx.TimeoutException`.
pub trait LlmTransport: Send + Sync {
    fn send(&self, req: HttpRequest) -> BoxFuture<'_, Result<HttpResponse, String>>;
}

/// The real transport (reqwest, rustls, no proxy). Like the Python client (a new `httpx.AsyncClient` per call), every
/// request opens its own connection: a pooled keep-alive connection can be closed by the server just as it is
/// reused (uvicorn drops idle ones after 5 s — exactly the first retry backoff), which failed the retry.
pub struct ReqwestTransport {
    client: reqwest::Client,
}

impl Default for ReqwestTransport {
    fn default() -> Self {
        Self::new()
    }
}

impl ReqwestTransport {
    pub fn new() -> Self {
        Self {
            client: reqwest::Client::builder()
                .connect_timeout(Duration::from_secs(15))
                .pool_max_idle_per_host(0)
                .no_proxy()
                .build()
                .expect("reqwest client"),
        }
    }
}

impl LlmTransport for ReqwestTransport {
    fn send(&self, req: HttpRequest) -> BoxFuture<'_, Result<HttpResponse, String>> {
        Box::pin(async move {
            let mut b = match req.method {
                "GET" => self.client.get(&req.url),
                _ => self.client.post(&req.url),
            };
            b = b.timeout(Duration::from_secs_f64(req.timeout.max(0.001)));
            for (k, v) in &req.headers {
                b = b.header(k.as_str(), v.as_str());
            }
            if let Some(j) = &req.json {
                b = b.json(j);
            }
            if let Some((model, filename, bytes)) = req.audio {
                let part = reqwest::multipart::Part::bytes(bytes)
                    .file_name(filename)
                    .mime_str("audio/wav")
                    .map_err(|e| format!("ValueError: {}", e))?;
                let form = reqwest::multipart::Form::new()
                    .text("model", model)
                    .text("response_format", "json")
                    .part("file", part);
                b = b.multipart(form);
            }
            let r = b.send().await.map_err(describe_reqwest_error)?;
            let status = r.status().as_u16();
            let retry_after = r
                .headers()
                .get("retry-after")
                .and_then(|v| v.to_str().ok())
                .map(str::to_string);
            let text = r.text().await.map_err(describe_reqwest_error)?;
            Ok(HttpResponse {
                status,
                retry_after,
                text,
            })
        })
    }
}

/// `f"{type(e).__name__}: {e}"` for the httpx exception classes the Python code caught.
fn describe_reqwest_error(e: reqwest::Error) -> String {
    let kind = if e.is_timeout() {
        "TimeoutException"
    } else if e.is_connect() {
        "ConnectError"
    } else if e.is_body() || e.is_decode() {
        "ReadError"
    } else {
        "TransportError"
    };
    format!("{}: {}", kind, e)
}

// ------------------------------------------------------------------------------------------------ client

/// An async callback fed with a JSON dict (`on_call` / `on_retry` in Python).
pub type Hook = Arc<dyn Fn(Value) -> BoxFuture<'static, ()> + Send + Sync>;
/// `get_settings()`: the settings in force (`store.settings()`).
pub type SettingsFn = Arc<dyn Fn() -> Value + Send + Sync>;

/// Per-call options of [`Llm::chat`] (the Python keyword arguments).
#[derive(Debug, Clone)]
pub struct ChatOpts {
    pub temperature: Option<f64>,
    pub max_tokens: Option<i64>,
    pub purpose: String,
    pub task_id: String,
    pub model: Option<String>,
    pub no_think: bool,
}

impl Default for ChatOpts {
    fn default() -> Self {
        Self {
            temperature: None,
            max_tokens: None,
            purpose: "executor".into(),
            task_id: String::new(),
            model: None,
            no_think: false,
        }
    }
}

/// One tool call the model asked for.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub args: Value,
}

/// What [`Llm::chat`] returns (`{"content", "reasoning", "tool_calls", "usage", "finish_reason"}`).
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ChatReply {
    pub content: String,
    pub reasoning: String,
    pub tool_calls: Vec<ToolCall>,
    pub usage: Value,
    pub finish_reason: Value,
}

struct RetryState {
    after: u32,
    backoff: usize,
}

/// The chat client. Shared behind `Arc`; `chat` and `transcribe` take `&self`.
pub struct Llm {
    pub get_settings: SettingsFn,
    pub on_call: Option<Hook>,
    /// `async (info) -> None`: the UI shows "Retrying 1 of 3".
    pub on_retry: Option<Hook>,
    /// Requests in flight at once (Settings → `llm_concurrency`, 1..=8).
    sem: Semaphore,
    /// Speech-to-text runs on its own server; it must not hold an LLM slot.
    stt_sem: Semaphore,
    /// configured model -> (model actually served, until when). Only for a few minutes: while the Olares router
    /// restarts, the configured model is briefly "missing"; a permanent fallback kept every task on whatever model
    /// was listed first — on 2026-10-05 that was a text-to-speech model, and every chat failed with HTTP 422 for a day.
    fallback: parking_lot::Mutex<HashMap<String, (String, f64)>>,
    transport: Arc<dyn LlmTransport>,
    /// Tests: called with the delay instead of sleeping.
    pub sleep_hook: Option<Arc<dyn Fn(f64) + Send + Sync>>,
}

impl Llm {
    pub fn new(get_settings: SettingsFn, on_call: Option<Hook>, on_retry: Option<Hook>) -> Self {
        Self::with_transport(
            get_settings,
            on_call,
            on_retry,
            Arc::new(ReqwestTransport::new()),
        )
    }

    /// Same, with a custom HTTP layer (tests, scripted models).
    pub fn with_transport(
        get_settings: SettingsFn,
        on_call: Option<Hook>,
        on_retry: Option<Hook>,
        transport: Arc<dyn LlmTransport>,
    ) -> Self {
        // llama.cpp serves 2 slots (-np 2) and batches them, so two tasks no longer wait for each other's calls
        let n = get_settings()
            .get("llm_concurrency")
            .and_then(omuse_common::util::as_i64)
            .filter(|n| *n > 0)
            .unwrap_or(2);
        Self {
            get_settings,
            on_call,
            on_retry,
            sem: Semaphore::new(n.clamp(1, 8) as usize),
            stt_sem: Semaphore::new(1),
            fallback: parking_lot::Mutex::new(HashMap::new()),
            transport,
            sleep_hook: None,
        }
    }

    /// The current fallback table (configured model → stand-in, expiry), for diagnostics.
    pub fn fallbacks(&self) -> HashMap<String, (String, f64)> {
        self.fallback.lock().clone()
    }

    async fn sleep(&self, secs: f64) {
        match &self.sleep_hook {
            Some(h) => h(secs),
            None => tokio::time::sleep(Duration::from_secs_f64(secs.max(0.0))).await,
        }
    }

    async fn call_hook(hook: &Option<Hook>, info: Value) {
        if let Some(h) = hook {
            h(info).await;
        }
    }

    /// Decide whether to retry after `err`; sleeps the right time or fails with `LLMError` when retries are used up.
    async fn retry(
        &self,
        state: &mut RetryState,
        err: &str,
        retry_after_header: Option<&str>,
        task_id: &str,
        purpose: &str,
    ) -> Result<(), LlmError> {
        let ra = retry_after(retry_after_header);
        let (attempt, of, delay, source) = match ra {
            Some(ra) if state.after < RETRY_AFTER_TRIES => {
                state.after += 1;
                (
                    state.after as usize,
                    RETRY_AFTER_TRIES as usize,
                    ra,
                    "retry-after",
                )
            }
            _ if state.backoff < RETRY_DELAYS.len() => {
                let delay = RETRY_DELAYS[state.backoff];
                state.backoff += 1;
                (state.backoff, RETRY_DELAYS.len(), delay, "backoff")
            }
            _ => {
                return Err(LlmError::Failed(format!(
                    "模型服务暂时不可用 (model unavailable after retries): {}",
                    err
                )))
            }
        };
        let info = json!({"attempt": attempt, "of": of, "wait_s": (delay * 10.0).round() / 10.0, "reason": take_chars(err, 200),
                          "source": source, "task_id": task_id, "purpose": purpose});
        println!(
            "[llm] retry {}/{} in {:.0}s ({}): {}",
            attempt,
            of,
            delay,
            source,
            take_chars(err, 120)
        );
        Self::call_hook(&self.on_retry, info).await;
        self.sleep(delay).await;
        Ok(())
    }

    fn model_missing(r: &HttpResponse) -> bool {
        let t = r.text.to_lowercase();
        r.status == 404
            || (t.contains("model")
                && [
                    "not found",
                    "does not exist",
                    "unknown",
                    "not exist",
                    "no such",
                ]
                .iter()
                .any(|k| t.contains(k)))
    }

    /// The first chat-capable model the endpoint lists (other than `exclude`); "" when none or unreachable.
    pub async fn first_model(&self, base: &str, exclude: &str) -> String {
        let s = (self.get_settings)();
        let req = HttpRequest {
            method: "GET",
            url: format!("{}/models", base),
            headers: settings_auth(&s),
            json: None,
            audio: None,
            timeout: 15.0,
            connect_timeout: 15.0,
        };
        let Ok(r) = self.transport.send(req).await else {
            return String::new();
        };
        let Some(j) = r.json() else {
            return String::new();
        };
        let models: Vec<Value> = j
            .get("data")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter(|m| m.is_object() && get_truthy(m, "id"))
                    .cloned()
                    .collect()
            })
            .unwrap_or_default();
        pick_chat_model(&models, exclude)
    }

    /// Speech-to-text model: Settings → `stt_model`, else the first whisper-like model the endpoint serves ("" = none).
    pub async fn stt_model(&self) -> String {
        let s = (self.get_settings)();
        if get_truthy(&s, "stt_model") {
            return py_str(&s["stt_model"]);
        }
        let base = get_str(&s, "model_base_url")
            .trim_end_matches('/')
            .to_string();
        let req = HttpRequest {
            method: "GET",
            url: format!("{}/models", base),
            headers: settings_auth(&s),
            json: None,
            audio: None,
            timeout: 15.0,
            connect_timeout: 15.0,
        };
        let Ok(r) = self.transport.send(req).await else {
            return String::new();
        };
        let Some(j) = r.json() else {
            return String::new();
        };
        let ids: Vec<String> = j
            .get("data")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter(|m| get_truthy(m, "id"))
                    .map(|m| py_str(&m["id"]))
                    .collect()
            })
            .unwrap_or_default();
        for i in ids {
            let low = i.to_lowercase();
            if [
                "whisper",
                "sensevoice",
                "paraformer",
                "asr",
                "speech-to-text",
                "stt",
            ]
            .iter()
            .any(|k| low.contains(k))
            {
                return i;
            }
        }
        String::new()
    }

    /// OpenAI-compatible `/audio/transcriptions`. Fails with `LLMError` when no speech model is available.
    pub async fn transcribe(
        &self,
        audio: Vec<u8>,
        filename: &str,
        task_id: &str,
    ) -> Result<String, LlmError> {
        let s = (self.get_settings)();
        let base = get_str(&s, "model_base_url")
            .trim_end_matches('/')
            .to_string();
        let model = self.stt_model().await;
        if model.is_empty() {
            return Err(LlmError::Failed(
                "没有语音转文字模型 (no speech-to-text model on the model endpoint; set Settings → Speech-to-text model)".into(),
            ));
        }
        let t0 = now_ts();
        let mut retries = RetryState {
            after: 0,
            backoff: 0,
        };
        let timeout = s
            .get("llm_timeout")
            .and_then(omuse_common::util::as_f64)
            .filter(|t| *t > 0.0)
            .unwrap_or(600.0);
        let _slot = self
            .stt_sem
            .acquire()
            .await
            .map_err(|e| LlmError::Failed(e.to_string()))?;
        let r = loop {
            let req = HttpRequest {
                method: "POST",
                url: format!("{}/audio/transcriptions", base),
                headers: settings_auth(&s),
                json: None,
                audio: Some((model.clone(), filename.to_string(), audio.clone())),
                timeout,
                connect_timeout: 15.0,
            };
            let r = match self.transport.send(req).await {
                Ok(r) => r,
                Err(e) => {
                    self.retry(&mut retries, &e, None, task_id, "stt").await?;
                    continue;
                }
            };
            if r.status < 400 {
                break r;
            }
            let err = format!(
                "语音转文字失败 transcription failed HTTP {}: {}",
                r.status,
                take_chars(&r.text, 200)
            );
            self.retry(&mut retries, &err, r.retry_after.as_deref(), task_id, "stt")
                .await?;
        };
        let text = match r.json() {
            Some(j) => j
                .get("text")
                .filter(|t| truthy(t))
                .map(py_str)
                .unwrap_or_default(),
            None => r.text.clone(),
        };
        Self::call_hook(
            &self.on_call,
            json!({"purpose": "stt", "task_id": task_id, "model": model, "latency_s": round2(now_ts() - t0),
                   "prompt_tokens": null, "completion_tokens": null, "tool_calls": []}),
        )
        .await;
        Ok(text.trim().to_string())
    }

    /// One chat completion (with tools when given). Retries per the module rules; a prompt over the context window
    /// fails at once with [`LlmError::Context`].
    pub async fn chat(
        &self,
        messages: &[Value],
        tools: Option<&[Value]>,
        o: &ChatOpts,
    ) -> Result<ChatReply, LlmError> {
        let s = (self.get_settings)();
        let base = get_str(&s, "model_base_url")
            .trim_end_matches('/')
            .to_string();
        let want = o
            .model
            .clone()
            .filter(|m| !m.is_empty())
            .unwrap_or_else(|| get_str(&s, "model_name"));
        let fb = {
            let mut fbs = self.fallback.lock();
            match fbs.get(&want).cloned() {
                Some((_, until)) if until < now_ts() => {
                    fbs.remove(&want);
                    None
                }
                other => other,
            }
        };
        let temperature = o
            .temperature
            .or_else(|| omuse_common::util::as_f64(&s["temperature"]))
            .unwrap_or(0.0);
        let max_tokens = o
            .max_tokens
            .filter(|m| *m != 0)
            .or_else(|| omuse_common::util::as_i64(&s["max_tokens"]))
            .unwrap_or(0);
        let mut body = Map::new();
        body.insert(
            "model".into(),
            json!(fb
                .as_ref()
                .map(|f| f.0.clone())
                .unwrap_or_else(|| want.clone())),
        );
        body.insert("messages".into(), Value::Array(messages.to_vec()));
        body.insert("temperature".into(), json!(temperature));
        body.insert("max_tokens".into(), json!(max_tokens));
        body.insert("stream".into(), json!(false));
        let has_tools = tools.is_some_and(|t| !t.is_empty());
        if let Some(t) = tools.filter(|t| !t.is_empty()) {
            body.insert("tools".into(), Value::Array(t.to_vec()));
            body.insert("tool_choice".into(), json!("auto"));
        }
        if get_truthy(&s, "disable_thinking") || o.no_think {
            body.insert(
                "chat_template_kwargs".into(),
                json!({"enable_thinking": false}),
            );
        }
        let timeout = s
            .get("llm_timeout")
            .and_then(omuse_common::util::as_f64)
            .filter(|t| *t > 0.0)
            .unwrap_or(600.0);
        let queued = now_ts();
        let mut retries = RetryState {
            after: 0,
            backoff: 0,
        };
        let mut stripped_kwargs = false;
        let headers = settings_auth(&s);
        let _slot = self
            .sem
            .acquire()
            .await
            .map_err(|e| LlmError::Failed(e.to_string()))?;
        let wait_s = round2(now_ts() - queued);
        let mut t0;
        let data = loop {
            t0 = now_ts();
            let req = HttpRequest {
                method: "POST",
                url: format!("{}/chat/completions", base),
                headers: headers.clone(),
                json: Some(Value::Object(body.clone())),
                audio: None,
                timeout,
                connect_timeout: 15.0,
            };
            let r = match self.transport.send(req).await {
                Ok(r) => r,
                Err(e) => {
                    self.retry(&mut retries, &e, None, &o.task_id, &o.purpose)
                        .await?;
                    continue;
                }
            };
            if [400, 413, 500].contains(&r.status) && context_exceeded(&r.text) {
                let chars = messages
                    .iter()
                    .map(|m| {
                        m.get("content")
                            .filter(|c| truthy(c))
                            .map(py_str)
                            .unwrap_or_default()
                            .chars()
                            .count()
                    })
                    .sum();
                return Err(LlmError::Context {
                    msg: format!(
                        "上下文太长 (prompt exceeds the model's context window): {}",
                        take_chars(&r.text, 200)
                    ),
                    chars,
                });
            }
            if r.status < 400 {
                match r.json() {
                    Some(d) => break d,
                    None => {
                        return Err(LlmError::Failed(format!(
                            "模型返回格式异常 unexpected response: {}",
                            take_chars(&r.text, 300)
                        )))
                    }
                }
            }
            let mut err = format!("HTTP {}: {}", r.status, take_chars(&r.text, 200));
            if ![429, 500, 502, 503, 504].contains(&r.status) {
                // adjustments that are not retries: a stand-in model that fails, a missing model, a server that
                // rejects chat_template_kwargs
                let current = get_str(&Value::Object(body.clone()), "model");
                if current != want && !context_exceeded(&r.text) {
                    self.fallback.lock().remove(&want);
                    body.insert("model".into(), json!(want));
                    continue;
                }
                let known = self.fallback.lock().contains_key(&want);
                if Self::model_missing(&r) && !known {
                    let alt = self.first_model(&base, &current).await;
                    if !alt.is_empty() {
                        self.fallback
                            .lock()
                            .insert(want.clone(), (alt.clone(), now_ts() + 300.0));
                        body.insert("model".into(), json!(alt));
                        continue;
                    }
                    err = format!(
                        "设置里的模型 {} 现在不可用（可能正在加载或已被卸载），也没有别的聊天模型可用 (the configured model is not available right now): {}",
                        want, err
                    );
                } else if body.contains_key("chat_template_kwargs") && !stripped_kwargs {
                    stripped_kwargs = true;
                    body.remove("chat_template_kwargs");
                    continue;
                }
            }
            self.retry(
                &mut retries,
                &err,
                r.retry_after.as_deref(),
                &o.task_id,
                &o.purpose,
            )
            .await?;
        };
        let latency = now_ts() - t0;
        let msg = match data
            .get("choices")
            .and_then(Value::as_array)
            .and_then(|c| c.first())
            .and_then(|c| c.get("message"))
        {
            Some(m) if m.is_object() => m.clone(),
            _ => {
                return Err(LlmError::Failed(format!(
                    "模型返回格式异常 unexpected response: {}",
                    take_chars(&pyjson::py_repr(&data), 300)
                )))
            }
        };
        let mut content = msg
            .get("content")
            .filter(|c| truthy(c))
            .map(py_str)
            .unwrap_or_default();
        let mut reasoning = msg
            .get("reasoning_content")
            .filter(|c| truthy(c))
            .map(py_str)
            .unwrap_or_default();
        if let Some(m) = THINK_CAP.captures(&content) {
            if reasoning.is_empty() {
                reasoning = m[1].to_string();
            }
        }
        content = THINK.replace_all(&content, "").into_owned();
        if content.contains("<think>") && !content.contains("</think>") {
            // truncated thinking
            content = content.split("<think>").next().unwrap_or("").to_string();
        }
        content = content.trim().to_string();
        let mut calls: Vec<ToolCall> = Vec::new();
        if let Some(tcs) = msg.get("tool_calls").and_then(Value::as_array) {
            for tc in tcs {
                let fun = tc
                    .get("function")
                    .filter(|f| truthy(f))
                    .cloned()
                    .unwrap_or(json!({}));
                let name = get_str(&fun, "name");
                if name.is_empty() {
                    continue;
                }
                let id = {
                    let i = get_str(tc, "id");
                    if i.is_empty() {
                        new_call_id()
                    } else {
                        i
                    }
                };
                calls.push(ToolCall {
                    id,
                    name,
                    args: parse_args(fun.get("arguments").unwrap_or(&Value::Null)),
                });
            }
        }
        if calls.is_empty() && has_tools && content.contains("<tool_call>") {
            for c in TOOLCALL_TAG.captures_iter(&content) {
                let j = parse_args(&Value::String(c[1].to_string()));
                let name = get_str(&j, "name");
                if !name.is_empty() {
                    let raw = j
                        .get("arguments")
                        .filter(|a| truthy(a))
                        .or_else(|| j.get("parameters").filter(|a| truthy(a)))
                        .cloned()
                        .unwrap_or(json!({}));
                    calls.push(ToolCall {
                        id: new_call_id(),
                        name,
                        args: parse_args(&raw),
                    });
                }
            }
            content = TOOLCALL_TAG.replace_all(&content, "").trim().to_string();
        }
        if calls.is_empty() && has_tools && content.contains("<function=") {
            for (name, body) in toolcall_xml(&content) {
                let mut args = Map::new();
                for (k, v) in xml_params(&body) {
                    args.insert(k, xml_value(&v));
                }
                calls.push(ToolCall {
                    id: new_call_id(),
                    name,
                    args: Value::Object(args),
                });
            }
        }
        content = strip_tool_markup(&content); // also when no tools were offered (final answer): never show raw markup
        let usage = data
            .get("usage")
            .filter(|u| truthy(u))
            .cloned()
            .unwrap_or(json!({}));
        let model_used = body.get("model").cloned().unwrap_or(Value::Null);
        Self::call_hook(
            &self.on_call,
            json!({"purpose": o.purpose, "task_id": o.task_id, "model": model_used,
                   "latency_s": round2(latency), "wait_s": wait_s, "prompt_tokens": usage.get("prompt_tokens").cloned().unwrap_or(Value::Null),
                   "completion_tokens": usage.get("completion_tokens").cloned().unwrap_or(Value::Null),
                   "tool_calls": calls.iter().map(|c| c.name.clone()).collect::<Vec<_>>()}),
        )
        .await;
        let finish_reason = data["choices"][0]
            .get("finish_reason")
            .cloned()
            .unwrap_or(Value::Null);
        Ok(ChatReply {
            content,
            reasoning,
            tool_calls: calls,
            usage,
            finish_reason,
        })
    }
}

fn new_call_id() -> String {
    format!("call_{}", &omuse_common::util::uuid_hex()[..10])
}

fn round2(x: f64) -> f64 {
    (x * 100.0).round() / 100.0
}

#[cfg(test)]
mod tests {
    use super::*;
    use parking_lot::Mutex;

    #[test]
    fn tool_markup_never_reaches_the_answer() {
        let raw = "<tool_call>\n<function=update_plan>\n<parameter=steps>\n[{\"id\": \"s1\", \"status\": \"done\"}]\n</parameter>\n</function>\n</tool_call>";
        assert_eq!(strip_tool_markup(raw), "");
        assert_eq!(
            strip_tool_markup(&format!("Answer here.\n{}", raw)),
            "Answer here."
        );
        assert_eq!(
            strip_tool_markup("Cut off <tool_call>\n<function=x>"),
            "Cut off"
        );
        let found = toolcall_xml(raw);
        assert_eq!(found.len(), 1);
        let (name, body) = &found[0];
        let args: Map<String, Value> = xml_params(body)
            .into_iter()
            .map(|(k, v)| (k, xml_value(&v)))
            .collect();
        assert_eq!(name, "update_plan");
        assert_eq!(
            Value::Object(args),
            json!({"steps": [{"id": "s1", "status": "done"}]})
        );
        let found = toolcall_xml("<tool_call><function=web_search><parameter=query>\nHSBC price\n</parameter></function></tool_call>");
        assert_eq!(found[0].0, "web_search");
        assert_eq!(
            xml_params(&found[0].1),
            vec![("query".to_string(), "HSBC price".to_string())]
        );
    }

    #[test]
    fn args_and_json_extraction() {
        assert_eq!(parse_args(&json!({"a": 1})), json!({"a": 1}));
        assert_eq!(parse_args(&json!("")), json!({}));
        assert_eq!(parse_args(&json!("{\"a\": 1,}")), json!({"a": 1}));
        assert_eq!(parse_args(&json!("[1]")), json!({"value": [1]}));
        assert_eq!(
            parse_args(&json!("call with {\"q\": \"x\"} please")),
            json!({"q": "x"})
        );
        assert_eq!(
            parse_args(&json!("garbage")),
            json!({"_unparsed": "garbage"})
        );
        assert_eq!(
            extract_json("<think>no</think>```json\n{\"a\": [1,2,]}\n```"),
            Some(json!({"a": [1, 2]}))
        );
        assert_eq!(extract_json("list: [1, 2]"), Some(json!([1, 2])));
        assert_eq!(extract_json("nothing"), None);
        assert_eq!(extract_json(""), None);
    }

    #[test]
    fn retry_after_header_parsing() {
        assert_eq!(retry_after(Some("12")), Some(12.0));
        assert_eq!(retry_after(None), None);
        assert_eq!(retry_after(Some("junk")), None);
        assert_eq!(retry_after(Some("99999")), Some(RETRY_AFTER_MAX)); // capped
        let future = chrono::Utc::now() + chrono::Duration::seconds(30);
        let v = retry_after(Some(&future.to_rfc2822())).unwrap();
        assert!((25.0..=31.0).contains(&v)); // HTTP date
        let past = chrono::Utc::now() - chrono::Duration::seconds(60);
        assert_eq!(retry_after(Some(&past.to_rfc2822())), Some(0.0)); // in the past: now
    }

    #[test]
    fn model_fallback_never_picks_tts() {
        let models = vec![
            json!({"id": "speaches/speaches-ai/Kokoro-82M-v1.0-ONNX", "mode": "tts", "readiness": "ready"}),
            json!({"id": "speaches/Systran/faster-whisper-small", "mode": "audio", "readiness": "ready"}),
            json!({"id": "Olares/Qwen", "mode": "chat", "readiness": "ready"}),
        ];
        assert_eq!(pick_chat_model(&models, ""), "Olares/Qwen");
        assert_eq!(pick_chat_model(&models, "Olares/Qwen"), "");
        assert_eq!(
            pick_chat_model(
                &[
                    json!({"id": "speaches/speaches-ai/Kokoro-82M-v1.0-ONNX"}),
                    json!({"id": "gpt-x"})
                ],
                ""
            ),
            "gpt-x"
        );
        assert_eq!(
            pick_chat_model(&[json!({"id": "x", "readiness": "loading"})], ""),
            ""
        );
    }

    #[test]
    fn auth_header_rules() {
        let cfg = Config::parse("[model]\napi_key=' sk-test '\n").unwrap();
        assert_eq!(
            auth_headers(None, &cfg),
            vec![("Authorization".to_string(), "Bearer sk-test".to_string())]
        );
        assert!(auth_headers(None, &Config::parse("").unwrap()).is_empty()); // Olares router: no key, no header
        assert_eq!(
            auth_headers(Some(&json!({"model_api_key": "my-key"})), &cfg),
            vec![("Authorization".to_string(), "Bearer my-key".to_string())]
        );
        assert!(auth_headers(Some(&json!({"model_api_key": ""})), &cfg).is_empty());
    }

    /// Serves scripted responses (or transport errors) in order and records the request bodies.
    struct Scripted {
        responses: Mutex<Vec<Result<HttpResponse, String>>>,
        requests: Mutex<Vec<HttpRequest>>,
    }

    impl LlmTransport for Scripted {
        fn send(&self, req: HttpRequest) -> BoxFuture<'_, Result<HttpResponse, String>> {
            self.requests.lock().push(req);
            let r = self.responses.lock().remove(0);
            Box::pin(async move { r })
        }
    }

    fn resp(status: u16, body: Value, retry_after: Option<&str>) -> Result<HttpResponse, String> {
        Ok(HttpResponse {
            status,
            retry_after: retry_after.map(str::to_string),
            text: pyjson::dumps(&body),
        })
    }

    fn ok_body() -> Value {
        json!({"choices": [{"message": {"role": "assistant", "content": "fine"}}], "usage": {}})
    }

    #[allow(clippy::type_complexity)]
    fn scripted_llm(
        responses: Vec<Result<HttpResponse, String>>,
    ) -> (
        Llm,
        Arc<Scripted>,
        Arc<Mutex<Vec<f64>>>,
        Arc<Mutex<Vec<Value>>>,
    ) {
        let tr = Arc::new(Scripted {
            responses: Mutex::new(responses),
            requests: Mutex::new(vec![]),
        });
        let sleeps = Arc::new(Mutex::new(vec![]));
        let log = Arc::new(Mutex::new(vec![]));
        let log2 = log.clone();
        let on_retry: Hook = Arc::new(move |info| {
            let l = log2.clone();
            Box::pin(async move {
                l.lock().push(info);
            })
        });
        let settings: SettingsFn = Arc::new(
            || json!({"model_base_url": "http://x/v1", "model_name": "m", "temperature": 0, "max_tokens": 10}),
        );
        let mut llm = Llm::with_transport(settings, None, Some(on_retry), tr.clone());
        let s2 = sleeps.clone();
        llm.sleep_hook = Some(Arc::new(move |d| s2.lock().push(d)));
        (llm, tr, sleeps, log)
    }

    #[tokio::test]
    async fn retries_obey_retry_after() {
        // 429 with Retry-After: wait exactly that long; a 403 with Retry-After is obeyed the same way
        let (llm, tr, sleeps, log) = scripted_llm(vec![
            resp(429, json!({"error": "slow down"}), Some("7")),
            resp(403, json!({"error": "blocked"}), Some("2")),
            resp(200, ok_body(), None),
        ]);
        let opts = ChatOpts {
            task_id: "t1".into(),
            ..Default::default()
        };
        let r = llm
            .chat(&[json!({"role": "user", "content": "hi"})], None, &opts)
            .await
            .unwrap();
        assert_eq!(r.content, "fine");
        assert_eq!(*sleeps.lock(), vec![7.0, 2.0]);
        let l = log.lock();
        let seen: Vec<(i64, i64, String, f64)> = l
            .iter()
            .map(|x| {
                (
                    x["attempt"].as_i64().unwrap(),
                    x["of"].as_i64().unwrap(),
                    get_str(x, "source"),
                    x["wait_s"].as_f64().unwrap(),
                )
            })
            .collect();
        assert_eq!(
            seen,
            vec![
                (1, 10, "retry-after".to_string(), 7.0),
                (2, 10, "retry-after".to_string(), 2.0)
            ]
        );
        assert_eq!(l[0]["task_id"], "t1");
        assert!(get_str(&l[0], "reason").contains("HTTP 429"));
        // the request body is the OpenAI shape
        let body = tr.requests.lock()[0].json.clone().unwrap();
        assert_eq!(
            body,
            json!({"model": "m", "messages": [{"role": "user", "content": "hi"}], "temperature": 0.0, "max_tokens": 10, "stream": false})
        );
        assert!(tr.requests.lock()[0].url.ends_with("/v1/chat/completions"));
    }

    #[tokio::test]
    async fn retries_general_errors_5_10_20() {
        // a 503, a connection error and a 403 without Retry-After: three retries after 5, 10 and 20 s, then success
        let (llm, _tr, sleeps, log) = scripted_llm(vec![
            resp(503, json!({"error": "busy"}), None),
            Err("ConnectError: down".into()),
            resp(403, json!({"error": "nope"}), None),
            resp(200, ok_body(), None),
            resp(500, json!({"error": "a"}), None),
            resp(500, json!({"error": "b"}), None),
            resp(500, json!({"error": "c"}), None),
            resp(500, json!({"error": "d"}), None),
        ]);
        let r = llm
            .chat(
                &[json!({"role": "user", "content": "hi"})],
                None,
                &ChatOpts::default(),
            )
            .await
            .unwrap();
        assert_eq!(r.content, "fine");
        assert_eq!(*sleeps.lock(), vec![5.0, 10.0, 20.0]);
        let seen: Vec<(i64, i64, String)> = log
            .lock()
            .iter()
            .map(|x| {
                (
                    x["attempt"].as_i64().unwrap(),
                    x["of"].as_i64().unwrap(),
                    get_str(x, "source"),
                )
            })
            .collect();
        assert_eq!(
            seen,
            vec![
                (1, 3, "backoff".into()),
                (2, 3, "backoff".into()),
                (3, 3, "backoff".into())
            ]
        );
        // a fourth failure in a row is the end
        sleeps.lock().clear();
        let e = llm
            .chat(
                &[json!({"role": "user", "content": "hi"})],
                None,
                &ChatOpts::default(),
            )
            .await
            .unwrap_err();
        assert!(e.to_string().contains("after retries") && e.name() == "LLMError");
        assert_eq!(*sleeps.lock(), vec![5.0, 10.0, 20.0]);
    }

    #[tokio::test]
    async fn context_overflow_is_not_retried() {
        let (llm, _tr, sleeps, log) = scripted_llm(vec![resp(
            400,
            json!({"error": {"message": "the prompt exceeds the maximum context length"}}),
            None,
        )]);
        let e = llm
            .chat(
                &[json!({"role": "user", "content": "hi"})],
                None,
                &ChatOpts::default(),
            )
            .await
            .unwrap_err();
        assert!(e.is_context() && e.name() == "LLMContextError");
        assert!(matches!(e, LlmError::Context { chars: 2, .. }));
        assert!(sleeps.lock().is_empty() && log.lock().is_empty());
    }

    #[tokio::test]
    async fn missing_model_falls_back_and_tool_calls_are_parsed() {
        let (llm, tr, _sleeps, _log) = scripted_llm(vec![
            resp(404, json!({"error": "model m not found"}), None),
            resp(
                200,
                json!({"data": [{"id": "Kokoro-tts"}, {"id": "Olares/Qwen"}]}),
                None,
            ),
            resp(
                200,
                json!({"choices": [{"message": {"role": "assistant", "content": "<think>hm</think> ok",
                        "tool_calls": [{"id": "c1", "function": {"name": "browser_search", "arguments": "{\"query\": \"x\"}"}}]},
                        "finish_reason": "tool_calls"}], "usage": {"prompt_tokens": 5, "completion_tokens": 2}}),
                None,
            ),
        ]);
        let tools = vec![json!({"type": "function", "function": {"name": "browser_search"}})];
        let r = llm
            .chat(
                &[json!({"role": "user", "content": "hi"})],
                Some(&tools),
                &ChatOpts::default(),
            )
            .await
            .unwrap();
        assert_eq!(r.content, "ok");
        assert_eq!(r.reasoning, "hm");
        assert_eq!(
            r.tool_calls,
            vec![ToolCall {
                id: "c1".into(),
                name: "browser_search".into(),
                args: json!({"query": "x"})
            }]
        );
        assert_eq!(r.finish_reason, "tool_calls");
        assert_eq!(r.usage["prompt_tokens"], 5);
        let reqs = tr.requests.lock();
        assert!(reqs[1].url.ends_with("/v1/models"));
        assert_eq!(reqs[2].json.as_ref().unwrap()["model"], "Olares/Qwen");
        assert_eq!(reqs[2].json.as_ref().unwrap()["tool_choice"], "auto");
        assert!(llm.fallbacks().contains_key("m"));
    }

    #[tokio::test]
    async fn text_tool_calls_and_kwargs_strip() {
        let settings: SettingsFn = Arc::new(
            || json!({"model_base_url": "http://x/v1", "model_name": "m", "temperature": 0.3, "max_tokens": 10, "disable_thinking": true}),
        );
        let tr = Arc::new(Scripted {
            responses: Mutex::new(vec![
                resp(
                    422,
                    json!({"error": "unknown field chat_template_kwargs"}),
                    None,
                ),
                resp(
                    200,
                    json!({"choices": [{"message": {"role": "assistant",
                        "content": "<tool_call>{\"name\": \"update_plan\", \"arguments\": {\"steps\": []}}</tool_call>"}}]}),
                    None,
                ),
            ]),
            requests: Mutex::new(vec![]),
        });
        let llm = Llm::with_transport(settings, None, None, tr.clone());
        let tools = vec![json!({"type": "function", "function": {"name": "update_plan"}})];
        let r = llm
            .chat(
                &[json!({"role": "user", "content": "hi"})],
                Some(&tools),
                &ChatOpts::default(),
            )
            .await
            .unwrap();
        assert_eq!(r.content, "");
        assert_eq!(r.tool_calls.len(), 1);
        assert_eq!(r.tool_calls[0].name, "update_plan");
        assert_eq!(r.tool_calls[0].args, json!({"steps": []}));
        assert!(r.tool_calls[0].id.starts_with("call_"));
        let reqs = tr.requests.lock();
        assert_eq!(
            reqs[0].json.as_ref().unwrap()["chat_template_kwargs"],
            json!({"enable_thinking": false})
        );
        assert!(reqs[1]
            .json
            .as_ref()
            .unwrap()
            .get("chat_template_kwargs")
            .is_none());
    }

    #[tokio::test]
    async fn real_transport_never_reuses_a_connection() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = l.local_addr().unwrap();
        let conns = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let n = conns.clone();
        tokio::spawn(async move {
            loop {
                let (mut sock, _) = l.accept().await.unwrap();
                n.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                tokio::spawn(async move {
                    let mut buf = vec![0u8; 8192];
                    // answer every request on this connection and keep it open (keep-alive)
                    while let Ok(k) = sock.read(&mut buf).await {
                        if k == 0 {
                            break;
                        }
                        let body = r#"{"data": []}"#;
                        let resp = format!(
                            "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\n\r\n{}",
                            body.len(),
                            body
                        );
                        if sock.write_all(resp.as_bytes()).await.is_err() {
                            break;
                        }
                    }
                });
            }
        });
        let t = ReqwestTransport::new();
        for _ in 0..2 {
            let r = t
                .send(HttpRequest {
                    method: "GET",
                    url: format!("http://{addr}/v1/models"),
                    headers: vec![],
                    json: None,
                    audio: None,
                    timeout: 5.0,
                    connect_timeout: 5.0,
                })
                .await
                .unwrap();
            assert_eq!(r.status, 200);
        }
        assert_eq!(conns.load(std::sync::atomic::Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn transcribe_needs_a_speech_model() {
        let settings: SettingsFn =
            Arc::new(|| json!({"model_base_url": "http://x/v1", "model_name": "m"}));
        let tr = Arc::new(Scripted {
            responses: Mutex::new(vec![
                resp(200, json!({"data": [{"id": "gpt"}]}), None),
                resp(
                    200,
                    json!({"data": [{"id": "gpt"}, {"id": "faster-whisper-small"}]}),
                    None,
                ),
                resp(200, json!({"text": " hello "}), None),
            ]),
            requests: Mutex::new(vec![]),
        });
        let llm = Llm::with_transport(settings, None, None, tr.clone());
        let e = llm.transcribe(vec![1, 2], "a.wav", "").await.unwrap_err();
        assert!(e.to_string().contains("speech-to-text"));
        assert_eq!(
            llm.transcribe(vec![1, 2], "a.wav", "t").await.unwrap(),
            "hello"
        );
        let reqs = tr.requests.lock();
        assert!(reqs[2].url.ends_with("/audio/transcriptions"));
        assert_eq!(reqs[2].audio.as_ref().unwrap().0, "faster-whisper-small");
    }
}
