//! Minimal MCP (Model Context Protocol) client used by Sentinel (`app/sentinel/mcp_client.py`).
//!
//! Supports the two HTTP transports:
//! * Streamable HTTP (spec 2025-03-26+): JSON-RPC via POST, reply as JSON or as an SSE stream, optional Mcp-Session-Id.
//! * Legacy HTTP+SSE (spec 2024-11-05): GET an SSE stream, the first `endpoint` event says where to POST messages;
//!   replies arrive on the stream.
//!
//! Only the client features OMuse needs are implemented: initialize, tools/list, tools/call, ping.
//! Server->client requests (sampling, elicitation, roots) are answered with "method not found" — a third-party
//! server can never make OMuse' LLM do anything through this channel.
//! stdio servers are deliberately not supported: Sentinel holds the vault and must not spawn arbitrary commands.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use futures_util::StreamExt;
use omuse_common::guard::{host_is_private, urlparse};
use omuse_common::util::{py_str, take_chars, truthy, VERSION};
use parking_lot::Mutex;
use serde_json::{json, Map, Value};
use tokio::sync::{oneshot, watch};
use tokio::task::JoinHandle;

/// MCP protocol version this client speaks.
pub const PROTOCOL_VERSION: &str = "2025-06-18";
/// Largest JSON reply accepted from a server.
pub const MAX_BYTES: usize = 4 * 1024 * 1024;

/// HTTP headers as the Python code carried them (`dict[str, str]`), e.g. `[("Authorization", "Bearer …")]`.
pub type Headers = Vec<(String, String)>;

/// `CLIENT_INFO`: `{"name": "OMuse", "version": VERSION}`.
pub fn client_info() -> Value {
    json!({"name": "OMuse", "version": *VERSION})
}

/// The `User-Agent` every MCP request carries.
pub fn user_agent() -> String {
    format!("OMuse/{} (MCP client)", *VERSION)
}

/// `MCPError(msg, auth=..., code=...)`: `auth` marks 401/403 answers (callers refresh tokens / re-login),
/// `code` is the HTTP status or the JSON-RPC error code when there was one.
#[derive(Debug, Clone, thiserror::Error)]
#[error("{msg}")]
pub struct MCPError {
    pub msg: String,
    pub auth: bool,
    pub code: Option<i64>,
}

impl MCPError {
    pub fn new(msg: impl Into<String>) -> Self {
        Self {
            msg: msg.into(),
            auth: false,
            code: None,
        }
    }
    pub fn with_code(msg: impl Into<String>, code: Option<i64>) -> Self {
        Self {
            msg: msg.into(),
            auth: false,
            code,
        }
    }
    pub fn auth(msg: impl Into<String>, code: Option<i64>) -> Self {
        Self {
            msg: msg.into(),
            auth: true,
            code,
        }
    }
}

/// `_private_host`: LAN / loopback / link-local / cluster-internal names where plain http is acceptable.
pub fn private_host(host: &str) -> bool {
    host_is_private(host)
}

/// `(username, password)` of a netloc, each `None` when there is no userinfo (Python `urlparse(..).username/.password`).
fn userinfo(netloc: &str) -> (Option<&str>, Option<&str>) {
    match netloc.rsplit_once('@') {
        Some((ui, _)) => match ui.split_once(':') {
            Some((u, p)) => (Some(u), Some(p)),
            None => (Some(ui), None),
        },
        None => (None, None),
    }
}

/// Return a normalised URL or an `MCPError`. Plain http is only allowed for LAN / in-cluster servers
/// so tokens never travel unencrypted over the internet.
pub fn check_server_url(url: &str) -> Result<String, MCPError> {
    let url = url.trim().to_string();
    let p = urlparse(&url).ok();
    let hostname = p.as_ref().and_then(|p| p.hostname());
    let (Some(p), Some(host)) = (p, hostname) else {
        return Err(MCPError::new(
            "MCP 服务器地址必须是 http(s):// 开头的网址 (URL must start with http:// or https://)",
        ));
    };
    if p.scheme != "http" && p.scheme != "https" {
        return Err(MCPError::new(
            "MCP 服务器地址必须是 http(s):// 开头的网址 (URL must start with http:// or https://)",
        ));
    }
    let (user, pass) = userinfo(&p.netloc);
    if user.is_some_and(|u| !u.is_empty()) || pass.is_some_and(|p| !p.is_empty()) {
        return Err(MCPError::new(
            "请不要把账号密码写在网址里，改用下面的「认证 Auth」字段 (no credentials in the URL)",
        ));
    }
    if p.scheme == "http" && !private_host(&host) {
        return Err(MCPError::new(
            "公网 MCP 服务器必须使用 https:// (plain http only for LAN / local servers)",
        ));
    }
    Ok(url)
}

/// `_parse_sse_block(lines) -> (event, data)`: the `event:` name (default `message`) and the joined `data:` lines.
pub fn parse_sse_block(lines: &[String]) -> (String, String) {
    let mut event = "message".to_string();
    let mut data: Vec<&str> = Vec::new();
    for ln in lines {
        if ln.starts_with(':') {
            continue;
        }
        let (k, v) = ln.split_once(':').unwrap_or((ln.as_str(), ""));
        let v = v.strip_prefix(' ').unwrap_or(v);
        if k == "event" {
            event = v.to_string();
        } else if k == "data" {
            data.push(v);
        }
    }
    (event, data.join("\n"))
}

/// `urllib.parse.urljoin(base, rel)` for the cases an SSE `endpoint` event uses: absolute URLs, `//host/…`,
/// absolute paths and relative paths (with `.`/`..` resolution). Nothing is normalised beyond that.
pub fn urljoin(base: &str, rel: &str) -> String {
    if rel.is_empty() {
        return base.to_string();
    }
    let Ok(b) = urlparse(base) else {
        return rel.to_string();
    };
    let Ok(r) = urlparse(rel) else {
        return rel.to_string();
    };
    if !r.scheme.is_empty() {
        return rel.to_string();
    }
    if rel.starts_with("//") {
        return format!("{}:{}", b.scheme, rel);
    }
    let origin = format!("{}://{}", b.scheme, b.netloc);
    let (path_part, tail) = match rel.find(['?', '#']) {
        Some(i) => (&rel[..i], &rel[i..]),
        None => (rel, ""),
    };
    if path_part.is_empty() {
        // "?x=1" / "#frag" keep the base path
        return format!("{}{}{}", origin, b.path, tail);
    }
    let joined = if path_part.starts_with('/') {
        path_part.to_string()
    } else {
        let dir = match b.path.rfind('/') {
            Some(i) => &b.path[..=i],
            None => "/",
        };
        format!("{}{}", dir, path_part)
    };
    // remove dot segments (RFC 3986 §5.2.4)
    let mut out: Vec<&str> = Vec::new();
    let segs: Vec<&str> = joined.split('/').collect();
    let n = segs.len();
    for (i, seg) in segs.iter().enumerate() {
        let last = i + 1 == n;
        match *seg {
            "." => {
                if last {
                    out.push("");
                }
            }
            ".." => {
                if out.len() > 1 {
                    out.pop();
                }
                if last {
                    out.push("");
                }
            }
            s => out.push(s),
        }
    }
    let mut path = out.join("/");
    if !path.starts_with('/') {
        path.insert(0, '/');
    }
    format!("{}{}{}", origin, path, tail)
}

/// `httpx` exception class name for a reqwest error (used in the `connection failed: <Name>: <msg>` texts).
fn err_name(e: &reqwest::Error) -> &'static str {
    if e.is_timeout() {
        if e.is_connect() {
            "ConnectTimeout"
        } else {
            "ReadTimeout"
        }
    } else if e.is_connect() {
        "ConnectError"
    } else if e.is_body() || e.is_decode() {
        "ReadError"
    } else if e.is_redirect() {
        "TooManyRedirects"
    } else {
        "HTTPError"
    }
}

fn connection_failed(e: &reqwest::Error) -> MCPError {
    MCPError::new(format!(
        "无法连接 MCP 服务器 connection failed: {}: {}",
        err_name(e),
        take_chars(&e.to_string(), 200)
    ))
}

fn dur(secs: f64) -> Duration {
    Duration::from_secs_f64(secs.max(0.0))
}

/// What can go wrong inside a transport before it is turned into the user-facing `MCPError` of `request()`.
enum Fail {
    Mcp(MCPError),
    Http(reqwest::Error),
    Timeout,
}

impl From<MCPError> for Fail {
    fn from(e: MCPError) -> Self {
        Fail::Mcp(e)
    }
}
impl From<reqwest::Error> for Fail {
    fn from(e: reqwest::Error) -> Self {
        Fail::Http(e)
    }
}

impl Fail {
    fn into_error(self, method: &str) -> MCPError {
        match self {
            Fail::Mcp(e) => e,
            Fail::Http(e) => connection_failed(&e),
            Fail::Timeout => MCPError::new(format!("MCP 服务器响应超时 timeout ({method})")),
        }
    }
}

/// Lines of a streaming HTTP body (`Response.aiter_lines()`): split on `\n`, a trailing `\r` removed.
struct LineReader<S> {
    stream: S,
    buf: Vec<u8>,
    done: bool,
}

impl<S> LineReader<S>
where
    S: futures_util::Stream<Item = reqwest::Result<bytes::Bytes>> + Unpin,
{
    fn new(stream: S) -> Self {
        Self {
            stream,
            buf: Vec::new(),
            done: false,
        }
    }

    async fn next_line(&mut self) -> Result<Option<String>, reqwest::Error> {
        loop {
            if let Some(i) = self.buf.iter().position(|b| *b == b'\n') {
                let mut line: Vec<u8> = self.buf.drain(..=i).collect();
                line.pop();
                if line.last() == Some(&b'\r') {
                    line.pop();
                }
                return Ok(Some(String::from_utf8_lossy(&line).into_owned()));
            }
            if self.done {
                if self.buf.is_empty() {
                    return Ok(None);
                }
                let rest = std::mem::take(&mut self.buf);
                return Ok(Some(String::from_utf8_lossy(&rest).into_owned()));
            }
            match self.stream.next().await {
                Some(chunk) => self.buf.extend_from_slice(&chunk?),
                None => self.done = true,
            }
        }
    }
}

// ---------------------------------------------------------------- shared base

#[derive(Default)]
struct BaseState {
    server_info: Value,
    capabilities: Value,
    protocol_version: String,
    instructions: String,
}

struct Base {
    url: String,
    headers: Headers,
    timeout: f64,
    http: reqwest::Client,
    id: AtomicI64,
    st: Mutex<BaseState>,
}

impl Base {
    fn new(url: &str, headers: &[(String, String)], timeout: f64) -> Base {
        let http = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(10))
            .redirect(reqwest::redirect::Policy::none())
            .user_agent(user_agent())
            .no_proxy()
            .build()
            .expect("reqwest client");
        Base {
            url: url.to_string(),
            headers: headers
                .iter()
                .filter(|(k, v)| !k.is_empty() && !v.is_empty())
                .cloned()
                .collect(),
            timeout,
            http,
            id: AtomicI64::new(0),
            st: Mutex::new(BaseState {
                server_info: json!({}),
                capabilities: json!({}),
                protocol_version: PROTOCOL_VERSION.to_string(),
                instructions: String::new(),
            }),
        }
    }

    fn next_id(&self) -> i64 {
        self.id.fetch_add(1, Ordering::SeqCst) + 1
    }

    /// `_check_status`: 401/403 → auth error, 3xx → "redirect not followed", other 4xx/5xx → `HTTP n`.
    fn check_status(resp: &reqwest::Response) -> Result<(), MCPError> {
        let code = resp.status().as_u16();
        if code == 401 || code == 403 {
            return Err(MCPError::auth(
                format!(
                    "认证失败 authentication failed (HTTP {code})：请检查令牌/请求头 (check token)"
                ),
                Some(code as i64),
            ));
        }
        if (300..400).contains(&code) {
            let loc = resp
                .headers()
                .get("location")
                .and_then(|v| v.to_str().ok())
                .unwrap_or("?");
            return Err(MCPError::with_code(
                format!("服务器要求跳转到 {loc}，请直接填写最终地址 (redirect not followed)"),
                Some(code as i64),
            ));
        }
        if code >= 400 {
            return Err(MCPError::with_code(
                format!("HTTP {code}"),
                Some(code as i64),
            ));
        }
        Ok(())
    }

    /// `_result(msg)`: the `result` of a JSON-RPC reply, or an `MCPError` for its `error`.
    fn result(msg: &Value) -> Result<Value, MCPError> {
        if let Some(e) = msg.get("error").filter(|e| truthy(e)) {
            let code = e.get("code").map(py_str).unwrap_or_default();
            let text = e.get("message").map(py_str).unwrap_or_default();
            let num = e.get("code").and_then(omuse_common::util::as_i64);
            return Err(MCPError::with_code(
                format!("MCP 错误 {}: {}", code, take_chars(&text, 300)),
                num,
            ));
        }
        Ok(match msg.get("result") {
            Some(r) if truthy(r) => r.clone(),
            _ => json!({}),
        })
    }

    /// `_answer_server_request`: `ping` is answered, everything else gets "method not found".
    fn answer_server_request(msg: &Value) -> Value {
        let id = msg.get("id").cloned().unwrap_or(Value::Null);
        if msg.get("method").and_then(Value::as_str) == Some("ping") {
            return json!({"jsonrpc": "2.0", "id": id, "result": {}});
        }
        json!({"jsonrpc": "2.0", "id": id, "error": {"code": -32601, "message": "not supported by OMuse"}})
    }

    fn apply_headers(&self, mut rb: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        for (k, v) in &self.headers {
            rb = rb.header(k.as_str(), v.as_str());
        }
        rb
    }
}

fn notification(method: &str, params: Option<Value>) -> Value {
    let mut m = Map::new();
    m.insert("jsonrpc".into(), json!("2.0"));
    m.insert("method".into(), json!(method));
    if let Some(p) = params.filter(truthy) {
        m.insert("params".into(), p);
    }
    Value::Object(m)
}

fn params_or_empty(params: Option<Value>) -> Value {
    match params {
        Some(p) if truthy(&p) => p,
        _ => json!({}),
    }
}

// ---------------------------------------------------------------- Streamable HTTP

struct StreamableInner {
    base: Base,
    session_id: Mutex<String>,
}

/// Streamable HTTP transport (`StreamableHTTP`).
#[derive(Clone)]
pub struct StreamableHttp(Arc<StreamableInner>);

impl StreamableHttp {
    pub fn new(url: &str, headers: &[(String, String)], timeout: f64) -> Self {
        Self(Arc::new(StreamableInner {
            base: Base::new(url, headers, timeout),
            session_id: Mutex::new(String::new()),
        }))
    }

    /// The `Mcp-Session-Id` the server issued (empty before `initialize`).
    pub fn session_id(&self) -> String {
        self.0.session_id.lock().clone()
    }

    fn hdrs(&self, rb: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        let mut rb = rb
            .header("Accept", "application/json, text/event-stream")
            .header("Content-Type", "application/json");
        rb = self.0.base.apply_headers(rb);
        let sid = self.0.session_id.lock().clone();
        if !sid.is_empty() {
            rb = rb.header("Mcp-Session-Id", sid);
        }
        let st = self.0.base.st.lock();
        if truthy(&st.server_info) {
            rb = rb.header("MCP-Protocol-Version", st.protocol_version.clone());
        }
        rb
    }

    async fn post_quiet(&self, msg: &Value) {
        let rb = self
            .hdrs(self.0.base.http.post(&self.0.base.url))
            .json(msg)
            .timeout(dur(self.0.base.timeout));
        let _ = rb.send().await;
    }

    /// `notify(method, params)`: a JSON-RPC notification; transport errors are ignored.
    pub async fn notify(&self, method: &str, params: Option<Value>) {
        self.post_quiet(&notification(method, params)).await;
    }

    /// `request(method, params, timeout)`.
    pub async fn request(
        &self,
        method: &str,
        params: Option<Value>,
        timeout: Option<f64>,
    ) -> Result<Value, MCPError> {
        self.request_with(method, params_or_empty(params), timeout, true)
            .await
    }

    async fn request_with(
        &self,
        method: &str,
        params: Value,
        timeout: Option<f64>,
        retry: bool,
    ) -> Result<Value, MCPError> {
        let rid = self.0.base.next_id();
        let msg = json!({"jsonrpc": "2.0", "id": rid, "method": method, "params": params});
        let t = timeout.unwrap_or(self.0.base.timeout);
        match tokio::time::timeout(dur(t), self.do_request(&msg, rid, retry)).await {
            Err(_) => Err(Fail::Timeout.into_error(method)),
            Ok(Err(f)) => Err(f.into_error(method)),
            Ok(Ok(v)) => Ok(v),
        }
    }

    async fn do_request(&self, msg: &Value, rid: i64, retry: bool) -> Result<Value, Fail> {
        let method = msg
            .get("method")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let resp = self
            .hdrs(self.0.base.http.post(&self.0.base.url))
            .json(msg)
            .send()
            .await?;
        let has_session = !self.0.session_id.lock().is_empty();
        if resp.status().as_u16() == 404 && has_session && retry && method != "initialize" {
            // session expired on the server: start a new one and retry once
            self.0.session_id.lock().clear();
            self.0.base.st.lock().server_info = json!({});
            Box::pin(Client::StreamableHttp(self.clone()).initialize()).await?;
            let params = msg.get("params").cloned().unwrap_or_else(|| json!({}));
            return Ok(Box::pin(self.request_with(&method, params, None, false)).await?);
        }
        Base::check_status(&resp)?;
        if let Some(sid) = resp
            .headers()
            .get("mcp-session-id")
            .and_then(|v| v.to_str().ok())
        {
            if !sid.is_empty() {
                *self.0.session_id.lock() = sid.to_string();
            }
        }
        let ctype = resp
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .split(';')
            .next()
            .unwrap_or("")
            .trim()
            .to_lowercase();
        if ctype == "text/event-stream" {
            let mut lines = LineReader::new(Box::pin(resp.bytes_stream()));
            let mut buf: Vec<String> = Vec::new();
            while let Some(line) = lines.next_line().await? {
                if !line.trim().is_empty() {
                    buf.push(line.trim_end_matches('\r').to_string());
                    continue;
                }
                if buf.is_empty() {
                    continue;
                }
                let (ev, data) = parse_sse_block(&buf);
                buf.clear();
                if ev != "message" || data.is_empty() {
                    continue;
                }
                if let Some(found) = self.handle(&data, rid)? {
                    return Ok(found);
                }
            }
            if !buf.is_empty() {
                let (_, data) = parse_sse_block(&buf);
                if !data.is_empty() {
                    if let Some(found) = self.handle(&data, rid)? {
                        return Ok(found);
                    }
                }
            }
            return Err(MCPError::new(
                "MCP 服务器提前结束了响应流 (stream ended without a result)",
            )
            .into());
        }
        let mut body: Vec<u8> = Vec::new();
        let mut stream = Box::pin(resp.bytes_stream());
        while let Some(chunk) = stream.next().await {
            body.extend_from_slice(&chunk?);
            if body.len() > MAX_BYTES {
                return Err(MCPError::new("MCP 响应过大 (response too large)").into());
            }
        }
        if body.iter().all(|b| b.is_ascii_whitespace()) {
            return Err(MCPError::new("MCP 服务器返回了空响应 (empty response)").into());
        }
        match self.handle(&String::from_utf8_lossy(&body), rid)? {
            Some(v) => Ok(v),
            None => {
                Err(MCPError::new("MCP 响应中没有找到对应结果 (no matching response id)").into())
            }
        }
    }

    /// `_handle(data, rid)`: the matching reply, if `data` holds it; server→client requests are answered in the background.
    fn handle(&self, data: &str, rid: i64) -> Result<Option<Value>, MCPError> {
        let Ok(obj) = serde_json::from_str::<Value>(data) else {
            return Ok(None);
        };
        let items: Vec<Value> = match obj {
            Value::Array(a) => a,
            other => vec![other],
        };
        for m in items {
            if !m.is_object() {
                continue;
            }
            let id = m.get("id").and_then(omuse_common::util::as_i64);
            if id == Some(rid) && (m.get("result").is_some() || m.get("error").is_some()) {
                return Base::result(&m).map(Some);
            }
            if m.get("method").is_some() && m.get("id").is_some() {
                let me = self.clone();
                let reply = Base::answer_server_request(&m);
                tokio::spawn(async move { me.post_quiet(&reply).await });
            }
        }
        Ok(None)
    }

    /// `close()`: end the session (`DELETE`) when the server issued one.
    pub async fn close(&self) {
        if !self.0.session_id.lock().is_empty() {
            let rb = self
                .hdrs(self.0.base.http.delete(&self.0.base.url))
                .timeout(Duration::from_secs(5));
            let _ = rb.send().await;
        }
    }
}

// ---------------------------------------------------------------- legacy HTTP+SSE

type Pending = HashMap<i64, oneshot::Sender<Result<Value, MCPError>>>;

struct SseInner {
    base: Base,
    endpoint: Mutex<String>,
    pending: Mutex<Pending>,
    reader: Mutex<Option<JoinHandle<()>>>,
    ready: Mutex<watch::Receiver<bool>>,
    closed: AtomicBool,
    error: Mutex<Option<MCPError>>,
}

/// Legacy HTTP+SSE transport (`LegacySSE`).
#[derive(Clone)]
pub struct LegacySse(Arc<SseInner>);

impl LegacySse {
    pub fn new(url: &str, headers: &[(String, String)], timeout: f64) -> Self {
        let (tx, rx) = watch::channel(false);
        drop(tx);
        Self(Arc::new(SseInner {
            base: Base::new(url, headers, timeout),
            endpoint: Mutex::new(String::new()),
            pending: Mutex::new(HashMap::new()),
            reader: Mutex::new(None),
            ready: Mutex::new(rx),
            closed: AtomicBool::new(false),
            error: Mutex::new(None),
        }))
    }

    /// The message endpoint announced by the server's `endpoint` event.
    pub fn endpoint(&self) -> String {
        self.0.endpoint.lock().clone()
    }

    async fn wait_ready(&self) -> Result<(), ()> {
        let mut rx = self.0.ready.lock().clone();
        let ready = tokio::time::timeout(Duration::from_secs(20), rx.wait_for(|v| *v))
            .await
            .is_ok();
        if ready {
            Ok(())
        } else {
            Err(())
        }
    }

    /// `_connect()`: make sure the SSE stream is open and the endpoint is known.
    async fn connect(&self) -> Result<(), Fail> {
        let running = self
            .0
            .reader
            .lock()
            .as_ref()
            .is_some_and(|h| !h.is_finished());
        if running {
            return self.wait_ready().await.map_err(|_| Fail::Timeout);
        }
        let (tx, rx) = watch::channel(false);
        *self.0.ready.lock() = rx;
        *self.0.error.lock() = None;
        let me = self.clone();
        let handle = tokio::spawn(async move { me.read_loop(tx).await });
        *self.0.reader.lock() = Some(handle);
        if self.wait_ready().await.is_err() {
            return Err(
                MCPError::new("SSE 连接超时：没有收到 endpoint 事件 (no endpoint event)").into(),
            );
        }
        if let Some(e) = self.0.error.lock().clone() {
            return Err(e.into());
        }
        Ok(())
    }

    async fn read_loop(&self, tx: watch::Sender<bool>) {
        let r = self.read_stream(&tx).await;
        if let Err(f) = r {
            let e = match f {
                Fail::Mcp(e) => e,
                Fail::Http(e) => MCPError::new(format!(
                    "SSE 连接断开 stream closed: {}: {}",
                    err_name(&e),
                    take_chars(&e.to_string(), 200)
                )),
                Fail::Timeout => MCPError::new("SSE 连接断开 stream closed: TimeoutException: "),
            };
            *self.0.error.lock() = Some(e);
        }
        tx.send_replace(true);
        let err = self
            .0
            .error
            .lock()
            .clone()
            .unwrap_or_else(|| MCPError::new("SSE 连接已关闭 (stream closed)"));
        self.fail_pending(err);
        if self.0.closed.load(Ordering::SeqCst) {
            *self.0.reader.lock() = None;
        }
    }

    fn fail_pending(&self, err: MCPError) {
        let pending: Vec<_> = self.0.pending.lock().drain().collect();
        for (_, tx) in pending {
            let _ = tx.send(Err(err.clone()));
        }
    }

    async fn read_stream(&self, tx: &watch::Sender<bool>) -> Result<(), Fail> {
        let base = &self.0.base;
        let rb = base.apply_headers(
            base.http
                .get(&base.url)
                .header("Accept", "text/event-stream"),
        );
        let resp = rb.send().await?;
        Base::check_status(&resp)?;
        let ctype = resp
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        if !ctype.contains("text/event-stream") {
            return Err(MCPError::new("这个地址不是 MCP SSE 端点 (not an SSE endpoint)").into());
        }
        let mut lines = LineReader::new(Box::pin(resp.bytes_stream()));
        let mut buf: Vec<String> = Vec::new();
        while let Some(line) = lines.next_line().await? {
            if !line.trim().is_empty() {
                buf.push(line.trim_end_matches('\r').to_string());
                continue;
            }
            if buf.is_empty() {
                continue;
            }
            let (ev, data) = parse_sse_block(&buf);
            buf.clear();
            if ev == "endpoint" {
                let ep = urljoin(&base.url, data.trim());
                let same_host = match (urlparse(&ep), urlparse(&base.url)) {
                    (Ok(a), Ok(b)) => a.netloc == b.netloc,
                    _ => false,
                };
                if !same_host {
                    return Err(MCPError::new(
                        "SSE endpoint 指向了其他主机，已拒绝 (endpoint on a different host)",
                    )
                    .into());
                }
                *self.0.endpoint.lock() = ep;
                tx.send_replace(true);
            } else if ev == "message" && !data.is_empty() {
                let Ok(obj) = serde_json::from_str::<Value>(&data) else {
                    continue;
                };
                let items: Vec<Value> = match obj {
                    Value::Array(a) => a,
                    other => vec![other],
                };
                for m in items {
                    if !m.is_object() {
                        continue;
                    }
                    if m.get("method").is_some() && m.get("id").is_some() {
                        let me = self.clone();
                        let reply = Base::answer_server_request(&m);
                        tokio::spawn(async move {
                            let _ = me.post(&reply).await;
                        });
                    } else if let Some(id) = m.get("id").and_then(omuse_common::util::as_i64) {
                        if let Some(waiter) = self.0.pending.lock().remove(&id) {
                            let _ = waiter.send(Ok(m));
                        }
                    }
                }
            }
        }
        Ok(())
    }

    async fn post(&self, msg: &Value) -> Result<(), Fail> {
        let base = &self.0.base;
        let endpoint = self.0.endpoint.lock().clone();
        let rb = base
            .apply_headers(
                base.http
                    .post(&endpoint)
                    .header("Content-Type", "application/json"),
            )
            .json(msg)
            .timeout(dur(base.timeout));
        let resp = rb.send().await?;
        Base::check_status(&resp)?;
        Ok(())
    }

    /// `notify(method, params)`: best effort, errors are ignored.
    pub async fn notify(&self, method: &str, params: Option<Value>) {
        if self.connect().await.is_err() {
            return;
        }
        let _ = self.post(&notification(method, params)).await;
    }

    /// `request(method, params, timeout)`.
    pub async fn request(
        &self,
        method: &str,
        params: Option<Value>,
        timeout: Option<f64>,
    ) -> Result<Value, MCPError> {
        match self
            .request_inner(method, params_or_empty(params), timeout)
            .await
        {
            Ok(v) => Ok(v),
            Err(f) => Err(f.into_error(method)),
        }
    }

    async fn request_inner(
        &self,
        method: &str,
        params: Value,
        timeout: Option<f64>,
    ) -> Result<Value, Fail> {
        self.connect().await?;
        let rid = self.0.base.next_id();
        let (tx, rx) = oneshot::channel();
        self.0.pending.lock().insert(rid, tx);
        let msg = json!({"jsonrpc": "2.0", "id": rid, "method": method, "params": params});
        let t = timeout.unwrap_or(self.0.base.timeout);
        let outcome: Result<Value, Fail> = async {
            self.post(&msg).await?;
            match tokio::time::timeout(dur(t), rx).await {
                Err(_) => Err(Fail::Timeout),
                Ok(Err(_)) => Err(MCPError::new("SSE 连接已关闭 (stream closed)").into()),
                Ok(Ok(Err(e))) => Err(e.into()),
                Ok(Ok(Ok(m))) => Ok(m),
            }
        }
        .await;
        self.0.pending.lock().remove(&rid);
        let m = outcome?;
        Ok(Base::result(&m)?)
    }

    /// `close()`: stop the reader and fail every waiting request.
    pub async fn close(&self) {
        self.0.closed.store(true, Ordering::SeqCst);
        if let Some(h) = self.0.reader.lock().take() {
            h.abort();
        }
        self.fail_pending(MCPError::new("SSE 连接已关闭 (stream closed)"));
    }
}

// ---------------------------------------------------------------- the session handle

/// An initialised MCP session over one of the two transports (the Python `_Base` object). Cheap to clone
/// (shared state behind an `Arc`), so the hub can cache it and hand copies to callers.
#[derive(Clone)]
pub enum Client {
    StreamableHttp(StreamableHttp),
    LegacySse(LegacySse),
}

impl std::fmt::Debug for Client {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Client")
            .field("transport", &self.transport())
            .field("url", &self.url())
            .finish()
    }
}

impl Client {
    fn base(&self) -> &Base {
        match self {
            Client::StreamableHttp(c) => &c.0.base,
            Client::LegacySse(c) => &c.0.base,
        }
    }

    /// `"streamable_http"` or `"sse"`.
    pub fn transport(&self) -> &'static str {
        match self {
            Client::StreamableHttp(_) => "streamable_http",
            Client::LegacySse(_) => "sse",
        }
    }

    /// The server URL.
    pub fn url(&self) -> &str {
        &self.base().url
    }

    /// `serverInfo` from `initialize` (`{}` before).
    pub fn server_info(&self) -> Value {
        self.base().st.lock().server_info.clone()
    }

    /// `capabilities` from `initialize` (`{}` before).
    pub fn capabilities(&self) -> Value {
        self.base().st.lock().capabilities.clone()
    }

    /// The negotiated protocol version.
    pub fn protocol_version(&self) -> String {
        self.base().st.lock().protocol_version.clone()
    }

    /// The server's `instructions` (first 2000 characters).
    pub fn instructions(&self) -> String {
        self.base().st.lock().instructions.clone()
    }

    /// `request(method, params, timeout)`: one JSON-RPC call; `timeout` in seconds (default: the client's).
    pub async fn request(
        &self,
        method: &str,
        params: Option<Value>,
        timeout: Option<f64>,
    ) -> Result<Value, MCPError> {
        match self {
            Client::StreamableHttp(c) => c.request(method, params, timeout).await,
            Client::LegacySse(c) => c.request(method, params, timeout).await,
        }
    }

    /// `notify(method, params)`: a JSON-RPC notification (errors ignored).
    pub async fn notify(&self, method: &str, params: Option<Value>) {
        match self {
            Client::StreamableHttp(c) => c.notify(method, params).await,
            Client::LegacySse(c) => c.notify(method, params).await,
        }
    }

    /// `initialize()`: the handshake; records server info / capabilities / instructions and sends `notifications/initialized`.
    pub async fn initialize(&self) -> Result<Value, MCPError> {
        let res = self
            .request(
                "initialize",
                Some(json!({"protocolVersion": PROTOCOL_VERSION, "capabilities": {}, "clientInfo": client_info()})),
                None,
            )
            .await?;
        {
            let mut st = self.base().st.lock();
            st.server_info = match res.get("serverInfo") {
                Some(v) if truthy(v) => v.clone(),
                _ => json!({}),
            };
            st.capabilities = match res.get("capabilities") {
                Some(v) if truthy(v) => v.clone(),
                _ => json!({}),
            };
            st.protocol_version = match res.get("protocolVersion") {
                Some(v) if truthy(v) => py_str(v),
                _ => PROTOCOL_VERSION.to_string(),
            };
            st.instructions = match res.get("instructions") {
                Some(v) if truthy(v) => take_chars(&py_str(v), 2000).to_string(),
                _ => String::new(),
            };
        }
        self.notify("notifications/initialized", None).await;
        Ok(res)
    }

    /// `list_tools(limit)`: every tool definition, following `nextCursor` (at most 20 pages).
    pub async fn list_tools(&self, limit: usize) -> Result<Vec<Value>, MCPError> {
        let mut tools: Vec<Value> = Vec::new();
        let mut cursor = String::new();
        for _ in 0..20 {
            let params = if cursor.is_empty() {
                json!({})
            } else {
                json!({"cursor": cursor})
            };
            let res = self.request("tools/list", Some(params), None).await?;
            if let Some(list) = res.get("tools").and_then(Value::as_array) {
                tools.extend(
                    list.iter()
                        .filter(|t| t.is_object() && truthy(&t["name"]))
                        .cloned(),
                );
            }
            cursor = match res.get("nextCursor") {
                Some(c) if truthy(c) => py_str(c),
                _ => String::new(),
            };
            if cursor.is_empty() || tools.len() >= limit {
                break;
            }
        }
        tools.truncate(limit);
        Ok(tools)
    }

    /// `call_tool(name, arguments, timeout)`: the raw `tools/call` result.
    pub async fn call_tool(
        &self,
        name: &str,
        arguments: Value,
        timeout: Option<f64>,
    ) -> Result<Value, MCPError> {
        let args = if truthy(&arguments) {
            arguments
        } else {
            json!({})
        };
        self.request(
            "tools/call",
            Some(json!({"name": name, "arguments": args})),
            timeout,
        )
        .await
    }

    /// `close()`.
    pub async fn close(&self) {
        match self {
            Client::StreamableHttp(c) => c.close().await,
            Client::LegacySse(c) => c.close().await,
        }
    }
}

/// Open and initialise a session. `transport`: `auto` | `streamable_http` | `sse`; `timeout` in seconds (Python default 60).
pub async fn connect(
    url: &str,
    headers: &[(String, String)],
    transport: &str,
    timeout: f64,
) -> Result<Client, MCPError> {
    let url = check_server_url(url)?;
    let mut transport = transport;
    if transport == "auto"
        && urlparse(&url).is_ok_and(|p| p.path.trim_end_matches('/').ends_with("/sse"))
    {
        transport = "sse";
    }
    if transport == "auto" || transport == "streamable_http" {
        let c = Client::StreamableHttp(StreamableHttp::new(&url, headers, timeout));
        match c.initialize().await {
            Ok(_) => return Ok(c),
            Err(e) => {
                c.close().await;
                if transport == "streamable_http"
                    || e.auth
                    || !matches!(e.code, Some(400 | 404 | 405 | 406 | 415))
                {
                    return Err(e);
                }
            }
        }
    }
    let c = Client::LegacySse(LegacySse::new(&url, headers, timeout));
    match c.initialize().await {
        Ok(_) => Ok(c),
        Err(e) => {
            c.close().await;
            Err(e)
        }
    }
}

/// Convenience: `connect(url, headers, "auto", 60.0)` (the Python defaults).
pub async fn connect_default(url: &str, headers: &[(String, String)]) -> Result<Client, MCPError> {
    connect(url, headers, "auto", 60.0).await
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn server_url_checks() {
        assert!(check_server_url("https://mcp.notion.com/mcp").is_ok());
        assert!(check_server_url("http://192.168.1.5:3000/mcp").is_ok());
        assert!(check_server_url("http://notion-mcp.svc:80/mcp").is_ok());
        assert!(check_server_url(" http://127.0.0.1:8093/mcp ").is_ok());
        for bad in [
            "http://example.com/mcp",
            "ftp://x/mcp",
            "https://u:p@x.com/mcp",
            "",
            "mcp.example.com",
        ] {
            assert!(check_server_url(bad).is_err(), "{bad}");
        }
        let e = check_server_url("http://example.com/mcp").unwrap_err();
        assert!(e.msg.contains("https"), "{}", e.msg);
        assert_eq!(
            check_server_url("https://u:p@x.com/mcp").unwrap_err().msg,
            "请不要把账号密码写在网址里，改用下面的「认证 Auth」字段 (no credentials in the URL)"
        );
    }

    #[test]
    fn sse_block_parsing() {
        assert_eq!(
            parse_sse_block(&lines(&["event: endpoint", "data: /m?x=1"])),
            ("endpoint".into(), "/m?x=1".into())
        );
        assert_eq!(
            parse_sse_block(&lines(&[": keepalive"])),
            ("message".into(), String::new())
        );
        assert_eq!(
            parse_sse_block(&lines(&["data: a", "data:b", "id: 3"])),
            ("message".into(), "a\nb".into())
        );
    }

    #[test]
    fn url_join() {
        assert_eq!(
            urljoin("http://127.0.0.1:8093/sse", "/messages?sid=1"),
            "http://127.0.0.1:8093/messages?sid=1"
        );
        assert_eq!(
            urljoin("http://h:80/a/sse", "messages"),
            "http://h:80/a/messages"
        );
        assert_eq!(urljoin("http://h/a/b/sse", "../x"), "http://h/a/x");
        assert_eq!(urljoin("https://h/sse", "https://evil/m"), "https://evil/m");
        assert_eq!(urljoin("https://h/sse", "//evil/m"), "https://evil/m");
        assert_eq!(urljoin("https://h/sse", "?x=1"), "https://h/sse?x=1");
    }

    #[test]
    fn jsonrpc_result_and_errors() {
        assert_eq!(
            Base::result(&json!({"id": 1, "result": {"a": 1}})).unwrap(),
            json!({"a": 1})
        );
        assert_eq!(
            Base::result(&json!({"id": 1, "result": null})).unwrap(),
            json!({})
        );
        let e = Base::result(
            &json!({"id": 1, "error": {"code": -32601, "message": "method not found"}}),
        )
        .unwrap_err();
        assert_eq!(e.msg, "MCP 错误 -32601: method not found");
        assert_eq!(e.code, Some(-32601));
        let e = Base::result(&json!({"id": 1, "error": {"message": "x"}})).unwrap_err();
        assert_eq!(e.msg, "MCP 错误 : x");
        assert_eq!(
            Base::answer_server_request(&json!({"id": 7, "method": "sampling/createMessage"})),
            json!({"jsonrpc": "2.0", "id": 7, "error": {"code": -32601, "message": "not supported by OMuse"}})
        );
        assert_eq!(
            Base::answer_server_request(&json!({"id": 8, "method": "ping"})),
            json!({"jsonrpc": "2.0", "id": 8, "result": {}})
        );
    }

    #[test]
    fn user_agent_and_client_info() {
        assert!(user_agent().starts_with("OMuse/"));
        assert!(user_agent().ends_with(" (MCP client)"));
        assert_eq!(client_info()["name"], "OMuse");
    }

    #[tokio::test]
    async fn line_reader_splits_chunks() {
        let chunks: Vec<reqwest::Result<bytes::Bytes>> = vec![
            Ok(bytes::Bytes::from_static(b"ab\r\nc")),
            Ok(bytes::Bytes::from_static(b"d\n\ntail")),
        ];
        let mut r = LineReader::new(futures_util::stream::iter(chunks));
        let mut got = Vec::new();
        while let Some(l) = r.next_line().await.unwrap() {
            got.push(l);
        }
        assert_eq!(got, vec!["ab", "cd", "", "tail"]);
    }
}
