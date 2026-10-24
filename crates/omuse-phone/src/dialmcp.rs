//! DialMCP as a phone line: calls to US / Canadian numbers from the user's own verified number.
//!
//! DialMCP (https://dialmcp.com) is a hosted MCP server: `place_call` starts a call handled by its own voice agent
//! (which says it is an AI calling for the user and that the call is recorded), `get_call` long-polls the status and,
//! at the end, returns a structured resolution + transcript + recording, `end_call` hangs up. OMuse signs in once with
//! OAuth 2.1 (mcp_oauth); the tokens stay in the vault as `cred_phone_dialmcp`.
//!
//! The agent never sees these MCP tools directly: it keeps using phone_call / phone_call_status / phone_hangup, so the
//! same policy applies to every line (approval for every call, number rules, daily limit). phone.py picks the line.
//!
//! Port of `app/sentinel/dialmcp.py`. The module globals `_session` / `_lock` live in [`Dial`] (one per [`crate::phone::Phone`]).

use std::sync::{Arc, LazyLock};

use omuse_common::pyjson::dumps;
use omuse_common::util::{as_f64, get_str, py_str, take_chars, truncate, truthy};
use omuse_mcp::mcp_client::{self, Client, MCPError};
use omuse_mcp::mcp_oauth;
use omuse_sentinel_core::store::Store;
use regex::Regex;
use serde_json::{json, Map, Value};
use tokio::sync::Mutex;

/// Vault handle of the DialMCP OAuth record (`{"oauth": {...}, "account": {...}}`).
pub const HANDLE: &str = "cred_phone_dialmcp";
/// The connector the handle belongs to.
pub const CONNECTOR: &str = "phone";
/// DialMCP call states that mean the call is over.
pub const TERMINAL: &[&str] = &[
    "completed",
    "no_answer",
    "busy",
    "voicemail",
    "failed",
    "canceled",
    "cancelled",
    "declined_recording",
    "opted_out",
];
/// DialMCP's own cap on a call.
pub const MAX_MINUTES: i64 = 10;

static NANP: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^\+1[2-9]\d{2}[2-9]\d{6}$").unwrap());
static LINE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\s*(?:\[[^\]]*\]\s*)?([^:]{1,30}):\s*(.+)").unwrap());
static WHO_TEXT: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)agent|assistant|\bai\b|caller|dialmcp|omuse").unwrap());
static WHO_ROLE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)agent|assistant|\bai\b|bot|caller").unwrap());
static LIVE_STATE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"progress|connect|answer|hold|talk|live|speaking").unwrap());

/// The `DialError` exception: `code` is DialMCP's error code (`"auth"` / `"network"` for connection problems).
#[derive(Debug, Clone, thiserror::Error)]
#[error("{msg}")]
pub struct DialError {
    pub msg: String,
    pub code: String,
}

impl DialError {
    /// `DialError(msg, code)`.
    pub fn new(msg: impl Into<String>, code: impl Into<String>) -> Self {
        Self {
            msg: msg.into(),
            code: code.into(),
        }
    }
}

enum ClientErr {
    Dial(DialError),
    Mcp(MCPError),
}

/// The DialMCP line: the store, the default server URL (`[voice] dialmcp_url`) and the cached MCP session.
pub struct Dial {
    store: Arc<Store>,
    default_url: String,
    session: Mutex<Option<Arc<Client>>>,
}

impl Dial {
    /// Create the line for `store`; `default_url` is used when the phone connection has no `dialmcp_url`.
    pub fn new(store: Arc<Store>, default_url: String) -> Dial {
        Dial {
            store,
            default_url,
            session: Mutex::new(None),
        }
    }

    /// `url(store)`: the DialMCP server URL in use.
    pub fn url(&self) -> String {
        let c = self.store.connection("phone").unwrap_or_else(|_| json!({}));
        match c.get("config").and_then(|c| c.get("dialmcp_url")) {
            Some(v) if truthy(v) => py_str(v),
            _ => self.default_url.clone(),
        }
    }

    /// `ready(store)`: signed in to DialMCP.
    pub fn ready(&self) -> bool {
        mcp_oauth::connected(&self.store, HANDLE)
    }

    /// `account(store)`: `{"name", "phone"}` of the signed-in user (`{}` when unknown).
    pub fn account(&self) -> Value {
        match self.store.get_secret(HANDLE) {
            Ok(Some(s)) => match s.get("account") {
                Some(a) if a.is_object() && truthy(a) => a.clone(),
                _ => json!({}),
            },
            _ => json!({}),
        }
    }

    /// `check_number(n, store)`: `""` if DialMCP can call normalized number `n`, else the reason.
    pub fn check_number(&self, n: &str) -> String {
        if !NANP.is_match(n) {
            return "DialMCP 只能拨打美国和加拿大的号码（+1 开头的 10 位号码）(DialMCP calls US / Canadian numbers only)".into();
        }
        if self.account().get("phone").map(py_str).as_deref() == Some(n) {
            return "不能拨打你自己的外呼号码 (that is your own caller-ID number)".into();
        }
        String::new()
    }

    /// `_drop()`: forget (and close) the cached MCP session — after a sign-in, sign-out or a failed call.
    pub async fn drop_session(&self) {
        let c = self.session.lock().await.take();
        if let Some(c) = c {
            c.close().await;
        }
    }

    async fn client(&self, force_refresh: bool) -> Result<Arc<Client>, ClientErr> {
        let mut guard = self.session.lock().await;
        if let (Some(s), false) = (guard.as_ref(), force_refresh) {
            return Ok(s.clone());
        }
        if let Some(c) = guard.take() {
            c.close().await;
        }
        let headers = mcp_oauth::auth_headers(&self.store, HANDLE, force_refresh).await.map_err(|e| {
            ClientErr::Dial(DialError::new(
                format!("DialMCP 授权失效，请在「连接 → 电话」重新登录 (sign in to DialMCP again): {e}"),
                "auth",
            ))
        })?;
        let session = match mcp_client::connect(&self.url(), &headers, "auto", 60.0).await {
            Ok(s) => Arc::new(s),
            Err(e) => {
                if e.auth && !force_refresh {
                    return Err(ClientErr::Mcp(e));
                }
                let code = if e.auth { "auth" } else { "network" };
                return Err(ClientErr::Dial(DialError::new(
                    format!("连不上 DialMCP (cannot reach DialMCP): {e}"),
                    code,
                )));
            }
        };
        *guard = Some(session.clone());
        Ok(session)
    }

    /// `call_tool(store, name, args, timeout)`: call a DialMCP tool, reconnecting / refreshing the token once.
    pub async fn call_tool(
        &self,
        name: &str,
        args: Value,
        timeout: f64,
    ) -> Result<Value, DialError> {
        let mut res = Value::Null;
        for attempt in 0..2 {
            let r: Result<Value, MCPError> = match self.client(attempt > 0).await {
                Ok(c) => c.call_tool(name, args.clone(), Some(timeout)).await,
                Err(ClientErr::Dial(e)) => return Err(e),
                Err(ClientErr::Mcp(e)) => Err(e),
            };
            match r {
                Ok(v) => {
                    res = v;
                    break;
                }
                Err(e) => {
                    self.drop_session().await;
                    if attempt == 0 && (e.auth || matches!(e.code, None | Some(404))) {
                        continue; // expired token or a stale session: refresh / reconnect once
                    }
                    let code = if e.auth { "auth" } else { "network" };
                    return Err(DialError::new(
                        format!("DialMCP {name} 调用失败 (failed): {e}"),
                        code,
                    ));
                }
            }
        }
        let data = parse(&res);
        if truthy(res.get("isError").unwrap_or(&Value::Null)) {
            let msg = ["message", "error_description", "text"]
                .iter()
                .find_map(|k| data.get(*k).filter(|v| truthy(v)).map(py_str))
                .unwrap_or_else(|| dumps(&data));
            let code = match data.get("error") {
                Some(v) if truthy(v) => py_str(v),
                _ => String::new(),
            };
            return Err(DialError::new(
                format!("DialMCP: {}", truncate(&msg, 600)),
                code,
            ));
        }
        Ok(data)
    }

    /// `whoami(store)`: the signed-in user's name and number (from `list_calls`).
    pub async fn whoami(&self) -> Result<Value, DialError> {
        let d = self
            .call_tool("list_calls", json!({"limit": 1}), 30.0)
            .await?;
        let u = d
            .get("user")
            .filter(|u| u.is_object())
            .cloned()
            .unwrap_or(json!({}));
        let s = |k: &str| match u.get(k) {
            Some(v) if truthy(v) => py_str(v),
            _ => String::new(),
        };
        Ok(json!({"name": take_chars(&s("name"), 80), "phone": take_chars(&s("phone"), 20)}))
    }

    /// `place(store, call, owner)`: `place_call` for the OMuse call record.
    pub async fn place(&self, call: &Value, owner: &str) -> Result<Value, DialError> {
        let mut objective = get_str(call, "purpose");
        let language = get_str(call, "language");
        if !language.is_empty() {
            objective.push_str(&format!("\nSpeak {language}."));
        }
        let mut ctx = Vec::new();
        if !owner.is_empty() {
            ctx.push(format!("You are calling on behalf of {owner}."));
        }
        let may_share = get_str(call, "may_share");
        if !may_share.is_empty() {
            ctx.push(format!("Facts you may share if asked: {may_share}"));
        }
        ctx.push(
            "Never pay, never read out card numbers, passwords, one-time codes or ID numbers, and never agree to \
             anything beyond the objective; say the account holder will follow up instead."
                .to_string(),
        );
        let minutes = call
            .get("max_minutes")
            .and_then(as_f64)
            .map(|m| m as i64)
            .unwrap_or(MAX_MINUTES);
        let mut args = json!({
            "to": get_str(call, "to"), "objective": truncate(&objective, 2000), "context": truncate(&ctx.join(" "), 2000),
            "max_duration_minutes": minutes.clamp(1, MAX_MINUTES),
        });
        let callee = get_str(call, "callee_name");
        if !callee.is_empty() {
            args["callee_name"] = json!(truncate(&callee, 200));
        }
        self.call_tool("place_call", args, 60.0).await
    }

    /// `get(store, remote_id, wait)`: long-poll the call (`wait` seconds, 0–50).
    pub async fn get(&self, remote_id: &str, wait: i64) -> Result<Value, DialError> {
        let w = wait.clamp(0, 50);
        self.call_tool(
            "get_call",
            json!({"call_id": remote_id, "wait_seconds": w}),
            (wait + 30) as f64,
        )
        .await
    }

    /// `end(store, remote_id)`: hang up.
    pub async fn end(&self, remote_id: &str) -> Result<Value, DialError> {
        self.call_tool("end_call", json!({"call_id": remote_id}), 40.0)
            .await
    }
}

/// `_parse(res)`: the tool result as an object — `structuredContent`, or the JSON in its text content, or `{"text": ...}`.
pub fn parse(res: &Value) -> Value {
    if let Some(sc) = res.get("structuredContent").filter(|v| v.is_object()) {
        return sc.clone();
    }
    let text = res
        .get("content")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter(|c| c.is_object() && c.get("type").and_then(Value::as_str) == Some("text"))
                .map(|c| get_str(c, "text"))
                .collect::<Vec<_>>()
                .join("\n")
        })
        .unwrap_or_default();
    match serde_json::from_str::<Value>(&text) {
        Ok(d) if d.is_object() => d,
        Ok(d) => json!({"result": d}),
        Err(_) => json!({"text": text}),
    }
}

// ------------------------------------------------------------------ DialMCP state -> OMuse call record

fn outcome_of(s: &str) -> String {
    match s {
        "achieved" => "done".into(),
        "partially_achieved" => "partly_done".into(),
        "not_achieved" | "no_conversation" => "not_done".into(),
        other => take_chars(other, 30).to_string(),
    }
}

/// Python `round(float(t), 1)` for a JSON value; `0` when it is not a number.
fn round1(v: &Value) -> Value {
    match as_f64(v) {
        Some(f) if v.is_number() || v.is_string() => {
            let r = (f * 10.0).round() / 10.0;
            json!(r)
        }
        _ => json!(0),
    }
}

/// `_transcript(raw)`: DialMCP's transcript (text or a list of turns) as OMuse transcript lines (last 400).
pub fn transcript(raw: &Value) -> Vec<Value> {
    let mut out = Vec::new();
    match raw {
        Value::String(s) => {
            for ln in s.lines() {
                if let Some(m) = LINE.captures(ln) {
                    let who = if WHO_TEXT.is_match(&m[1]) {
                        "omuse"
                    } else {
                        "them"
                    };
                    out.push(json!({"who": who, "text": truncate(m[2].trim(), 2000), "t": 0}));
                } else if !ln.trim().is_empty() {
                    out.push(json!({"who": "them", "text": truncate(ln.trim(), 2000), "t": 0}));
                }
            }
        }
        Value::Array(a) => {
            for x in a {
                if !x.is_object() {
                    continue;
                }
                let first = |keys: &[&str]| {
                    keys.iter()
                        .find_map(|k| x.get(*k).filter(|v| truthy(v)).map(py_str))
                };
                let role = first(&["role", "speaker", "who"]).unwrap_or_default();
                let text = first(&["text", "content", "message"])
                    .unwrap_or_default()
                    .trim()
                    .to_string();
                if text.is_empty() {
                    continue;
                }
                let t = ["t", "offset_seconds", "time"]
                    .iter()
                    .find_map(|k| x.get(*k).filter(|v| truthy(v)))
                    .map(round1)
                    .unwrap_or(json!(0));
                let who = if WHO_ROLE.is_match(&role) {
                    "omuse"
                } else {
                    "them"
                };
                out.push(json!({"who": who, "text": truncate(&text, 2000), "t": t}));
            }
        }
        _ => {}
    }
    let skip = out.len().saturating_sub(400);
    out.into_iter().skip(skip).collect()
}

fn set(call: &mut Value, k: &str, v: Value) {
    if let Some(o) = call.as_object_mut() {
        o.insert(k.to_string(), v);
    }
}

fn set_if_empty(call: &mut Value, k: &str, v: Value) {
    if !truthy(call.get(k).unwrap_or(&Value::Null)) {
        set(call, k, v);
    }
}

/// `apply(call, d, now)`: update the call record from a `get_call` / `place_call` result. `true` when the call is over.
pub fn apply(call: &mut Value, d: &Value, now: f64) -> bool {
    let st = get_str(d, "status").to_lowercase();
    for k in ["listen_url", "recording_url"] {
        if let Some(v) = d.get(k).filter(|v| truthy(v)) {
            set(call, k, json!(take_chars(&py_str(v), 500)));
        }
    }
    if let Some(v) = d.get("recording_status").filter(|v| truthy(v)) {
        set(call, "recording_status", json!(take_chars(&py_str(v), 30)));
    }
    let tr = transcript(d.get("transcript").unwrap_or(&Value::Null));
    if !tr.is_empty() {
        set(call, "transcript", Value::Array(tr));
    }
    if let Some(res) = d.get("resolution").filter(|r| r.is_object() && truthy(r)) {
        let mut keep = Map::new();
        for k in [
            "outcome",
            "summary",
            "commitments",
            "follow_ups",
            "key_facts",
        ] {
            if let Some(v) = res.get(k) {
                keep.insert(k.to_string(), v.clone());
            }
        }
        set(call, "resolution", Value::Object(keep));
        let mut parts = vec![res
            .get("summary")
            .filter(|v| truthy(v))
            .map(py_str)
            .unwrap_or_default()
            .trim()
            .to_string()];
        for (k, label) in [
            ("commitments", "Commitments"),
            ("follow_ups", "Follow-ups"),
            ("key_facts", "Key facts"),
        ] {
            if let Some(items) = res
                .get(k)
                .and_then(Value::as_array)
                .filter(|a| !a.is_empty())
            {
                let joined = items
                    .iter()
                    .take(12)
                    .map(|x| if x.is_object() { dumps(x) } else { py_str(x) })
                    .collect::<Vec<_>>()
                    .join("; ");
                parts.push(format!("{label}: {joined}"));
            }
        }
        let summary = parts
            .into_iter()
            .filter(|p| !p.is_empty())
            .collect::<Vec<_>>()
            .join("\n");
        set(call, "summary", json!(truncate(&summary, 3000)));
        let outcome = res
            .get("outcome")
            .filter(|v| truthy(v))
            .map(py_str)
            .unwrap_or_default();
        set(call, "outcome", json!(outcome_of(&outcome)));
    }
    let status = get_str(call, "status");
    if !TERMINAL.contains(&st.as_str()) {
        if LIVE_STATE.is_match(&st) && status == "dialing" {
            set(call, "status", json!("connected"));
            if !truthy(call.get("answered_at").unwrap_or(&Value::Null)) {
                set(call, "answered_at", json!(now));
            }
        }
        return false;
    }
    if call.get("answered_at").is_none_or(Value::is_null)
        && matches!(
            st.as_str(),
            "completed"
                | "voicemail"
                | "declined_recording"
                | "opted_out"
                | "canceled"
                | "cancelled"
        )
    {
        let created = call.get("created_at").cloned().unwrap_or(Value::Null);
        set_if_empty(call, "answered_at", created);
    }
    match st.as_str() {
        "no_answer" | "busy" => {
            set(call, "status", json!("no_answer"));
            set_if_empty(call, "hangup_cause", json!(st.replace('_', " ")));
        }
        "failed" => {
            set(call, "status", json!("failed"));
            let why = ["error", "failure_reason"]
                .iter()
                .find_map(|k| d.get(*k).filter(|v| truthy(v)).map(py_str))
                .unwrap_or_else(|| "call failed".into());
            set_if_empty(call, "error", json!(truncate(&why, 300)));
        }
        _ => {
            set(call, "status", json!("ended"));
            if st == "voicemail" {
                set_if_empty(call, "outcome", json!("voicemail"));
            } else if st == "declined_recording" || st == "opted_out" {
                set(call, "outcome", json!("refused"));
                set(call, "hangup_cause", json!(st.replace('_', " ")));
            } else if st == "canceled" || st == "cancelled" {
                let by_user = d.get("ended_by").and_then(Value::as_str) == Some("user");
                set(
                    call,
                    "hangup_cause",
                    json!(if by_user {
                        "canceled by the user"
                    } else {
                        "canceled"
                    }),
                );
            }
        }
    }
    set(call, "remote_status", json!(st));
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nanp_rule() {
        assert!(NANP.is_match("+14155550123"));
        assert!(!NANP.is_match("+6561234567"));
        assert!(!NANP.is_match("+11155550123"));
        assert!(!NANP.is_match("+141555501234"));
    }

    #[test]
    fn parse_results() {
        assert_eq!(
            parse(&json!({"structuredContent": {"a": 1}, "content": []})),
            json!({"a": 1})
        );
        assert_eq!(
            parse(&json!({"content": [{"type": "text", "text": "{\"error\": \"not_found\"}"}]})),
            json!({"error": "not_found"})
        );
        assert_eq!(
            parse(&json!({"content": [{"type": "text", "text": "[1, 2]"}]})),
            json!({"result": [1, 2]})
        );
        assert_eq!(
            parse(&json!({"content": [{"type": "text", "text": "unknown tool"}]})),
            json!({"text": "unknown tool"})
        );
    }

    #[test]
    fn transcript_shapes() {
        let t = transcript(&json!(
            "[00:01] Agent: Hello there\nCallee: Hi, Zuni Cafe\nnoise\n\n"
        ));
        assert_eq!(t.len(), 3);
        assert_eq!(t[0], json!({"who": "omuse", "text": "Hello there", "t": 0}));
        assert_eq!(
            t[1],
            json!({"who": "them", "text": "Hi, Zuni Cafe", "t": 0})
        );
        assert_eq!(t[2], json!({"who": "them", "text": "noise", "t": 0}));
        let t = transcript(&json!([
            {"role": "agent", "text": "Hi"}, {"role": "callee", "content": "Yes", "offset_seconds": "2.26"},
            {"role": "callee", "text": "  "}, "junk", {"speaker": "AI", "message": "Bye", "t": 7}
        ]));
        assert_eq!(t.len(), 3);
        assert_eq!(t[0]["who"], "omuse");
        assert_eq!(t[1], json!({"who": "them", "text": "Yes", "t": 2.3}));
        assert_eq!(t[2], json!({"who": "omuse", "text": "Bye", "t": 7.0}));
        assert!(transcript(&json!(null)).is_empty());
    }

    fn fresh() -> Value {
        json!({"id": "c", "status": "dialing", "created_at": 100.0, "answered_at": null, "transcript": [],
               "outcome": "", "summary": "", "error": "", "hangup_cause": ""})
    }

    #[test]
    fn apply_progression() {
        // the fake DialMCP's script for +14155550123: ringing -> in_progress -> completed
        let mut c = fresh();
        assert!(!apply(
            &mut c,
            &json!({"status": "ringing", "listen_url": "http://d/listen/1"}),
            101.0
        ));
        assert_eq!(c["status"], "dialing");
        assert_eq!(c["listen_url"], "http://d/listen/1");
        assert!(!apply(&mut c, &json!({"status": "in_progress"}), 102.0));
        assert_eq!(c["status"], "connected");
        assert_eq!(c["answered_at"], 102.0);
        let done = json!({"status": "completed", "recording_url": "http://d/rec/1.mp3", "transcript": [
            {"role": "agent", "text": "Hi"}, {"role": "callee", "text": "Reference Z-77."}],
            "resolution": {"outcome": "achieved", "summary": "Table for 2 booked.", "commitments": ["Hold 15 minutes"],
                           "follow_ups": [], "key_facts": ["Reference Z-77", {"k": "v"}], "ignored": 1}});
        assert!(apply(&mut c, &done, 103.0));
        assert_eq!(c["status"], "ended");
        assert_eq!(c["outcome"], "done");
        assert_eq!(c["summary"], "Table for 2 booked.\nCommitments: Hold 15 minutes\nKey facts: Reference Z-77; {\"k\": \"v\"}");
        assert_eq!(c["transcript"].as_array().unwrap().len(), 2);
        assert_eq!(c["remote_status"], "completed");
        assert!(c["resolution"].get("ignored").is_none());
        assert!(dumps(&c["resolution"]).contains("\"outcome\": \"achieved\""));
    }

    #[test]
    fn apply_terminal_states() {
        let mut c = fresh();
        assert!(apply(
            &mut c,
            &json!({"status": "no_answer", "resolution": {"outcome": "no_conversation", "summary": "Nobody answered."}}),
            1.0
        ));
        assert_eq!(
            (
                c["status"].as_str(),
                c["hangup_cause"].as_str(),
                c["outcome"].as_str()
            ),
            (Some("no_answer"), Some("no answer"), Some("not_done"))
        );
        assert!(c["answered_at"].is_null());
        let mut c = fresh();
        assert!(apply(
            &mut c,
            &json!({"status": "FAILED", "failure_reason": "carrier"}),
            1.0
        ));
        assert_eq!(
            (c["status"].as_str(), c["error"].as_str()),
            (Some("failed"), Some("carrier"))
        );
        let mut c = fresh();
        assert!(apply(&mut c, &json!({"status": "voicemail"}), 1.0));
        assert_eq!(
            (
                c["status"].as_str(),
                c["outcome"].as_str(),
                c["answered_at"].as_f64()
            ),
            (Some("ended"), Some("voicemail"), Some(100.0))
        );
        let mut c = fresh();
        assert!(apply(
            &mut c,
            &json!({"status": "canceled", "ended_by": "agent", "resolution": {"outcome": "partially_achieved", "summary": "Call ended early."}}),
            1.0
        ));
        assert_eq!(
            (
                c["status"].as_str(),
                c["hangup_cause"].as_str(),
                c["outcome"].as_str()
            ),
            (Some("ended"), Some("canceled"), Some("partly_done"))
        );
        let mut c = fresh();
        assert!(apply(&mut c, &json!({"status": "opted_out"}), 1.0));
        assert_eq!(
            (c["outcome"].as_str(), c["hangup_cause"].as_str()),
            (Some("refused"), Some("opted out"))
        );
        let mut c = fresh();
        c["hangup_cause"] = json!("hung up by the agent");
        assert!(apply(
            &mut c,
            &json!({"status": "cancelled", "ended_by": "user"}),
            1.0
        ));
        assert_eq!(c["hangup_cause"], "canceled by the user");
    }
}
