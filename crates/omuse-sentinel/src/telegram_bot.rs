//! Two-way Telegram control, run inside Sentinel.
//!
//! * Long polling (getUpdates) — works behind NAT, no public URL / webhook needed.
//! * Only the owner's chat (the configured chat_id) is served; everything else is ignored and audited.
//! * Text messages become tasks in a "Telegram" conversation; progress is edited in place, the result is sent
//!   when the task finishes. Commands: /new /tasks /stop /status /help.
//! * Approvals are pushed with inline ✅ / ❌ buttons and resolved through the same code path as the web UI
//!   (every decision is audited with via=telegram).
//! * The bot token never leaves Sentinel (vault); the agent/LLM never sees it.

use std::sync::{Arc, LazyLock};
use std::time::Duration;

use futures::future::BoxFuture;
use regex::Regex;
use serde_json::{json, Map, Value};

use omuse_common::api::ApiError;
use omuse_common::http::H_UI;
use omuse_common::util::{get_str, now_ts, py_str, take_chars, truthy};
use omuse_common::Config;
use omuse_sentinel_core::{Audit, Store};
use omuse_web::templates::py_html_escape as esc;

/// `resolve_core(approval_id, body, via)` of the routes layer.
pub type Resolver =
    Arc<dyn Fn(String, Value, String) -> BoxFuture<'static, Result<Value, ApiError>> + Send + Sync>;

pub const TERMINAL: [&str; 3] = ["COMPLETED", "FAILED", "CANCELLED"];

/// `STATUS_ZH.get(status, status)`.
pub fn status_zh(s: &str) -> &str {
    match s {
        "CREATED" => "已创建",
        "PLANNING" => "规划中",
        "RUNNING" => "执行中",
        "WAITING_APPROVAL" => "等待审批",
        "WAITING_EXTERNAL" => "等待你接管浏览器",
        "PAUSED" => "已暂停",
        "COMPLETED" => "已完成",
        "FAILED" => "失败",
        "CANCELLED" => "已取消",
        other => other,
    }
}

fn mark(s: &str) -> &'static str {
    match s {
        "pending" => "○",
        "running" => "▸",
        "done" => "✓",
        "failed" => "✕",
        "skipped" => "–",
        _ => "○",
    }
}

pub const HELP: &str = "🤖 <b>OMuse on Telegram</b>\n\
直接发消息 = 给 OMuse 布置任务（和网页里的对话一样）。\n\n\
/new — 开始新对话 new conversation\n\
/tasks — 最近的任务 recent tasks\n\
/stop — 停止当前任务 stop the running task\n\
/status — 待审批 / 浏览器状态 pending approvals & browser\n\
/help — 帮助\n\n\
需要审批的操作会带 ✅ 批准 / ❌ 拒绝 按钮；需要你登录或输入验证码时，我会发网页链接给你接管浏览器。";

static TABLE_SEP: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^(?:\|?[\s:\-|]+\|?)$").unwrap());
static HEADING: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^\s{0,3}#{1,6}\s+(.*)$").unwrap());
static BOLD: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\*\*(.+?)\*\*").unwrap());
static ITALIC: LazyLock<fancy_regex::Regex> = LazyLock::new(|| {
    fancy_regex::Regex::new(r"(?<![\w*])\*(?!\s)(.+?)(?<!\s)\*(?![\w*])").unwrap()
});
static CODE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"`([^`]+)`").unwrap());
static LINK: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\[([^\]]+)\]\((https?://[^)\s]+)\)").unwrap());
static BULLET: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^(\s*)[-*]\s+").unwrap());
static TAGS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"<[^>]+>").unwrap());
static TAGS0: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"<[^>]*>").unwrap());
static CALLBACK: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^ap:([\w\-]+):([yn])$").unwrap());

/// Python `str.splitlines()`.
pub(crate) fn splitlines(s: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut start = 0;
    let mut it = s.char_indices().peekable();
    while let Some((i, c)) = it.next() {
        let brk = matches!(
            c,
            '\n' | '\r'
                | '\x0b'
                | '\x0c'
                | '\x1c'
                | '\x1d'
                | '\x1e'
                | '\u{85}'
                | '\u{2028}'
                | '\u{2029}'
        );
        if brk {
            out.push(&s[start..i]);
            let mut end = i + c.len_utf8();
            if c == '\r' {
                if let Some((j, '\n')) = it.peek().copied() {
                    it.next();
                    end = j + 1;
                }
            }
            start = end;
        }
    }
    if start < s.len() {
        out.push(&s[start..]);
    }
    out
}

/// Very small Markdown -> Telegram-HTML converter (bold, code, links, headings, tables as `<pre>`).
pub fn md_to_tg(text: &str, limit: usize) -> Vec<String> {
    let t = text.trim();
    let text = if t.is_empty() { "（空）" } else { t };
    let mut out: Vec<String> = Vec::new();
    let mut in_code = false;
    let mut table: Vec<String> = Vec::new();

    fn flush_table(table: &mut Vec<String>, out: &mut Vec<String>) {
        if !table.is_empty() {
            let rows: Vec<String> = table
                .iter()
                .filter(|r| !TABLE_SEP.is_match(r))
                .map(|r| {
                    r.trim()
                        .trim_matches('|')
                        .split('|')
                        .map(str::trim)
                        .collect::<Vec<_>>()
                        .join(" | ")
                })
                .collect();
            out.push(format!("<pre>{}</pre>", esc(&rows.join("\n"))));
            table.clear();
        }
    }

    for line in splitlines(text) {
        if line.trim().starts_with("```") {
            flush_table(&mut table, &mut out);
            out.push(if in_code { "</pre>" } else { "<pre>" }.to_string());
            in_code = !in_code;
            continue;
        }
        if in_code {
            out.push(esc(line));
            continue;
        }
        if line.trim().starts_with('|') && line.matches('|').count() >= 2 {
            table.push(line.to_string());
            continue;
        }
        flush_table(&mut table, &mut out);
        let mut e = esc(line);
        if let Some(m) = HEADING.captures(&e) {
            e = format!("<b>{}</b>", &m[1]);
        }
        e = BOLD.replace_all(&e, "<b>${1}</b>").into_owned();
        e = ITALIC.replace_all(&e, "<i>${1}</i>").into_owned();
        e = CODE.replace_all(&e, "<code>${1}</code>").into_owned();
        e = LINK
            .replace_all(&e, "<a href=\"${2}\">${1}</a>")
            .into_owned();
        e = BULLET.replace(&e, "${1}• ").into_owned();
        out.push(e);
    }
    flush_table(&mut table, &mut out);
    if in_code {
        out.push("</pre>".into());
    }
    let body = out.join("\n");
    // split on line boundaries, keeping <pre> blocks balanced
    let mut chunks: Vec<String> = Vec::new();
    let mut cur = String::new();
    let mut cur_len = 0usize;
    for ln in body.split('\n') {
        let mut ln = ln.to_string();
        let n = ln.chars().count();
        if cur_len + n + 1 > limit && !cur.is_empty() {
            if cur.matches("<pre>").count() > cur.matches("</pre>").count() {
                cur.push_str("</pre>");
                ln = format!("<pre>{ln}");
            }
            chunks.push(std::mem::take(&mut cur));
            cur_len = 0;
        }
        if !cur.is_empty() {
            cur.push('\n');
            cur_len += 1;
        }
        cur_len += ln.chars().count();
        cur.push_str(&ln);
    }
    if !cur.is_empty() {
        chunks.push(cur);
    }
    chunks
}

/// The bot's state shown on the Connections page (`bot.status`).
#[derive(Debug, Clone, Default)]
pub struct BotStatus {
    pub running: bool,
    pub last_error: String,
    pub last_ok: f64,
    pub username: String,
}

impl BotStatus {
    pub fn to_value(&self) -> Value {
        json!({"running": self.running, "last_error": self.last_error, "last_ok": self.last_ok, "username": self.username})
    }
}

/// A Telegram API answer with `ok: false` (Python `RuntimeError`).
#[derive(Debug, Clone, thiserror::Error)]
#[error("{0}")]
pub struct TgApiError(pub String);

/// The two-way Telegram bot. Share it as `Arc<TelegramBot>`.
pub struct TelegramBot {
    pub cfg: Arc<Config>,
    pub store: Arc<Store>,
    pub runtime_url: String,
    pub actor: String,
    resolver: parking_lot::RwLock<Option<Resolver>>,
    /// task_id -> {"msg_id": int, "last": str}
    pub tracked: parking_lot::Mutex<Map<String, Value>>,
    pub status: parking_lot::Mutex<BotStatus>,
    client: reqwest::Client,
    tasks: parking_lot::Mutex<Vec<tokio::task::JoinHandle<()>>>,
    rejected_log: parking_lot::Mutex<f64>,
}

impl TelegramBot {
    /// `TelegramBot(store, runtime_url, resolver)`; the Telegram API base is `cfg.telegram.api`.
    pub fn new(
        cfg: Arc<Config>,
        store: Arc<Store>,
        runtime_url: String,
        resolver: Option<Resolver>,
    ) -> Self {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(40))
            .connect_timeout(Duration::from_secs(10))
            .no_proxy()
            .build()
            .expect("reqwest client");
        TelegramBot {
            cfg,
            store,
            runtime_url,
            actor: "telegram".into(),
            resolver: parking_lot::RwLock::new(resolver),
            tracked: parking_lot::Mutex::new(Map::new()),
            status: parking_lot::Mutex::new(BotStatus::default()),
            client,
            tasks: parking_lot::Mutex::new(Vec::new()),
            rejected_log: parking_lot::Mutex::new(0.0),
        }
    }

    /// Install the approval resolver (`resolve_core` of the routes layer).
    pub fn set_resolver(&self, r: Resolver) {
        *self.resolver.write() = Some(r);
    }

    /// How many tasks are being tracked.
    pub fn tracked_count(&self) -> usize {
        self.tracked.lock().len()
    }

    // ------------------------------------------------------------ config
    /// `(bot_token, chat_id)` when the bot is connected, enabled and allowed to control.
    pub fn config(&self) -> Option<(String, String)> {
        let conn = self.store.connection("telegram").ok()?;
        let sec = self
            .store
            .get_secret("cred_telegram_1")
            .ok()
            .flatten()
            .filter(truthy)?;
        let chat = conn["config"]
            .get("chat_id")
            .map(py_str)
            .unwrap_or_default()
            .trim()
            .to_string();
        let control = conn["permissions"]
            .get("control")
            .map(truthy)
            .unwrap_or(true);
        if !truthy(&conn["enabled"]) || chat.is_empty() || !control {
            return None;
        }
        Some((get_str(&sec, "bot_token"), chat))
    }

    /// `{public_base}/#{frag}`, or "" when the public URL is unknown.
    pub fn public_url(&self, frag: &str) -> String {
        let base = self
            .store
            .kv_get("public_base")
            .ok()
            .flatten()
            .map(|v| py_str(&v))
            .unwrap_or_default();
        let base = if base == "None" { String::new() } else { base };
        if base.is_empty() {
            String::new()
        } else {
            format!("{base}/#{frag}")
        }
    }

    fn kv(&self, k: &str, default: Value) -> Value {
        self.store.kv_get(k).ok().flatten().unwrap_or(default)
    }

    fn audit(&self, actor: &str, action: &str, a: Audit) {
        if let Err(e) = self.store.audit(actor, action, a) {
            println!("[telegram] audit failed: {e:#}");
        }
    }

    // ------------------------------------------------------------ lifecycle
    /// Start the poll and progress loops (no-op while they run).
    pub fn start(self: &Arc<Self>) {
        let mut tasks = self.tasks.lock();
        if tasks.is_empty() || tasks.iter().all(|t| t.is_finished()) {
            tasks.clear();
            let a = self.clone();
            tasks.push(tokio::spawn(async move { a.poll_loop().await }));
            let b = self.clone();
            tasks.push(tokio::spawn(async move { b.track_loop().await }));
        }
    }

    /// Stop both loops.
    pub async fn stop(&self) {
        for t in self.tasks.lock().drain(..) {
            t.abort();
        }
    }

    // ------------------------------------------------------------ Telegram API
    /// One Bot API call (JSON); `ok: false` → [`TgApiError`].
    pub async fn api(&self, method: &str, token: &str, params: Value) -> anyhow::Result<Value> {
        let r = self
            .client
            .post(format!("{}/bot{}/{}", self.cfg.telegram.api, token, method))
            .json(&params)
            .send()
            .await?;
        let status = r.status().as_u16();
        let body = r.bytes().await?;
        let data: Value = if body.is_empty() {
            json!({})
        } else {
            serde_json::from_slice(&body)?
        };
        if !data.get("ok").is_some_and(truthy) {
            let d = data
                .get("description")
                .filter(|d| truthy(d))
                .map(py_str)
                .unwrap_or_else(|| status.to_string());
            return Err(TgApiError(format!("telegram {method}: {d}")).into());
        }
        Ok(data.get("result").cloned().unwrap_or(Value::Null))
    }

    /// Send a message to the owner (HTML; falls back to plain text when Telegram rejects the markup).
    pub async fn send(
        &self,
        text: &str,
        markup: Option<Value>,
        html_mode: bool,
    ) -> anyhow::Result<Option<i64>> {
        let Some((token, chat)) = self.config() else {
            return Ok(None);
        };
        let mut msg_id = None;
        let parts = if !html_mode {
            md_to_tg(text, 3900)
        } else {
            vec![take_chars(text, 4000).to_string()]
        };
        let last = parts.len().saturating_sub(1);
        for (i, part) in parts.iter().enumerate() {
            let mut params = json!({"chat_id": chat, "text": part, "parse_mode": "HTML", "disable_web_page_preview": true});
            if let Some(m) = markup.as_ref().filter(|_| i == last) {
                params["reply_markup"] = m.clone();
            }
            let res = match self.api("sendMessage", &token, params.clone()).await {
                Ok(r) => r,
                Err(e) if e.downcast_ref::<TgApiError>().is_some() => {
                    if let Some(o) = params.as_object_mut() {
                        o.remove("parse_mode");
                    }
                    params["text"] = json!(TAGS.replace_all(part, "").into_owned());
                    self.api("sendMessage", &token, params).await?
                }
                Err(e) => return Err(e),
            };
            msg_id = res.get("message_id").and_then(Value::as_i64);
        }
        Ok(msg_id)
    }

    /// [`send`](Self::send) of an HTML text without buttons.
    pub async fn say(&self, text: &str) -> anyhow::Result<Option<i64>> {
        self.send(text, None, true).await
    }

    fn file_form(
        chat: &str,
        caption: &str,
        field: &'static str,
        name: &str,
        data: &[u8],
    ) -> reqwest::multipart::Form {
        let mut f = reqwest::multipart::Form::new().text("chat_id", chat.to_string());
        if !caption.is_empty() {
            f = f.text("caption", take_chars(caption, 1000).to_string());
        }
        f.part(
            field,
            reqwest::multipart::Part::bytes(data.to_vec()).file_name(name.to_string()),
        )
    }

    async fn post_file(
        &self,
        token: &str,
        method: &str,
        form: reqwest::multipart::Form,
    ) -> anyhow::Result<(u16, Value)> {
        let r = self
            .client
            .post(format!("{}/bot{}/{}", self.cfg.telegram.api, token, method))
            .multipart(form)
            .timeout(Duration::from_secs(120))
            .send()
            .await?;
        let status = r.status().as_u16();
        let body = r.bytes().await?;
        let res: Value = if body.is_empty() {
            json!({})
        } else {
            serde_json::from_slice(&body)?
        };
        Ok((status, res))
    }

    /// Photos go as photos (shown inline, ≤10 MB), MP4 videos as videos, everything else as a document.
    pub async fn send_document(
        &self,
        name: &str,
        data: &[u8],
        caption: &str,
        mime: &str,
    ) -> anyhow::Result<Option<i64>> {
        let Some((token, chat)) = self.config() else {
            anyhow::bail!("Telegram 未连接 (not connected)");
        };
        let (mut method, mut field) = ("sendDocument", "document");
        if matches!(mime, "image/jpeg" | "image/png" | "image/webp")
            && data.len() <= 10 * 1024 * 1024
        {
            (method, field) = ("sendPhoto", "photo");
        } else if mime == "video/mp4" {
            (method, field) = ("sendVideo", "video");
        }
        let (mut status, mut res) = self
            .post_file(
                &token,
                method,
                Self::file_form(&chat, caption, field, name, data),
            )
            .await?;
        if !res.get("ok").is_some_and(truthy) && method != "sendDocument" {
            // e.g. a photo Telegram can't process: send it as a file
            (status, res) = self
                .post_file(
                    &token,
                    "sendDocument",
                    Self::file_form(&chat, caption, "document", name, data),
                )
                .await?;
        }
        if !res.get("ok").is_some_and(truthy) {
            let d = res
                .get("description")
                .filter(|d| truthy(d))
                .map(py_str)
                .unwrap_or_else(|| status.to_string());
            return Err(TgApiError(format!("telegram {method}: {d}")).into());
        }
        Ok(res
            .get("result")
            .and_then(|r| r.get("message_id"))
            .and_then(Value::as_i64))
    }

    /// Edit a sent message ("not modified" is fine).
    pub async fn edit(
        &self,
        msg_id: Option<i64>,
        text: &str,
        markup: Option<Value>,
    ) -> anyhow::Result<()> {
        let (Some((token, chat)), Some(mid)) = (self.config(), msg_id.filter(|m| *m != 0)) else {
            return Ok(());
        };
        let mut params = json!({"chat_id": chat, "message_id": mid, "text": take_chars(text, 4000), "parse_mode": "HTML",
            "disable_web_page_preview": true});
        if let Some(m) = markup {
            params["reply_markup"] = m;
        }
        match self.api("editMessageText", &token, params).await {
            Ok(_) => Ok(()),
            Err(e)
                if e.downcast_ref::<TgApiError>().is_some()
                    && e.to_string().contains("not modified") =>
            {
                Ok(())
            }
            Err(e) => Err(e),
        }
    }

    // ------------------------------------------------------------ runtime API
    /// A call to the runtime's user API (`X-Persona-UI: 1`), 30 s.
    pub async fn rt(&self, method: &str, path: &str, body: Option<Value>) -> anyhow::Result<Value> {
        let m = reqwest::Method::from_bytes(method.as_bytes())?;
        let mut rb = self
            .client
            .request(m, format!("{}{}", self.runtime_url, path))
            .header(H_UI, "1")
            .timeout(Duration::from_secs(30));
        if let Some(b) = body {
            rb = rb.json(&b);
        }
        let r = rb.send().await?.error_for_status()?;
        Ok(r.json().await?)
    }

    // ------------------------------------------------------------ polling
    async fn poll_loop(self: Arc<Self>) {
        let mut backoff = 2u64;
        loop {
            let Some((token, chat)) = self.config() else {
                self.status.lock().running = false;
                tokio::time::sleep(Duration::from_secs(5)).await;
                continue;
            };
            let r: anyhow::Result<()> = async {
                if self.status.lock().username.is_empty() {
                    let me = self.api("getMe", &token, json!({})).await?;
                    self.status.lock().username = get_str(&me, "username");
                    self.api("deleteWebhook", &token, json!({})).await?;
                }
                let offset = self.kv("tg_offset", json!(0));
                let ups = self
                    .api(
                        "getUpdates",
                        &token,
                        json!({"offset": offset, "timeout": 25, "allowed_updates": ["message", "callback_query"]}),
                    )
                    .await?;
                {
                    let mut s = self.status.lock();
                    s.running = true;
                    s.last_error.clear();
                    s.last_ok = now_ts();
                }
                backoff = 2;
                for up in ups.as_array().cloned().unwrap_or_default() {
                    let uid = up.get("update_id").and_then(Value::as_i64).unwrap_or(0);
                    self.store.kv_set("tg_offset", &json!(uid + 1))?;
                    // never let one bad update kill the loop
                    if let Err(e) = self.handle(&up, &chat).await {
                        self.audit(
                            &self.actor,
                            "telegram.error",
                            Audit {
                                result: "error".into(),
                                detail: Some(json!({"error": take_chars(&format!("{e:#}"), 300)})),
                                ..Default::default()
                            },
                        );
                    }
                }
                Ok(())
            }
            .await;
            if let Err(e) = r {
                {
                    let mut s = self.status.lock();
                    s.running = false;
                    s.last_error = take_chars(&format!("{e:#}"), 300).to_string();
                }
                tokio::time::sleep(Duration::from_secs(backoff)).await;
                backoff = (backoff * 2).min(60);
            }
        }
    }

    /// Handle one update from the owner's chat (others are rejected and audited).
    pub async fn handle(&self, up: &Value, chat: &str) -> anyhow::Result<()> {
        if let Some(cq) = up.get("callback_query") {
            let frm = cq
                .get("message")
                .and_then(|m| m.get("chat"))
                .and_then(|c| c.get("id"))
                .map(py_str)
                .unwrap_or_default();
            if frm != chat {
                self.reject(&frm, None)?;
                return Ok(());
            }
            return self.on_callback(cq).await;
        }
        let msg = up.get("message").cloned().unwrap_or(json!({}));
        let ch = msg.get("chat").cloned().unwrap_or(json!({}));
        let frm = ch.get("id").map(py_str).unwrap_or_default();
        if frm != chat {
            if ch.get("type").and_then(Value::as_str) != Some("private") {
                self.reject(&frm, None)?;
                return Ok(());
            }
            let name = ["first_name", "last_name"]
                .iter()
                .filter_map(|k| ch.get(*k).filter(|v| truthy(v)).map(py_str))
                .collect::<Vec<_>>()
                .join(" ");
            let name = if name.is_empty() {
                get_str(&ch, "username")
            } else {
                name
            };
            self.reject(&frm, Some(&name))?;
            return Ok(());
        }
        let text = msg
            .get("text")
            .filter(|t| truthy(t))
            .map(py_str)
            .unwrap_or_default()
            .trim()
            .to_string();
        if text.is_empty() {
            self.say("暂时只支持文字消息 (text only for now) 🙏")
                .await?;
            return Ok(());
        }
        let cmd = if text.starts_with('/') {
            text.split_whitespace()
                .next()
                .unwrap_or("")
                .split('@')
                .next()
                .unwrap_or("")
                .to_lowercase()
        } else {
            String::new()
        };
        match cmd.as_str() {
            "/start" | "/help" => {
                self.say(HELP).await?;
            }
            "/new" => {
                self.store.kv_set("tg_conv", &json!(""))?;
                self.say("🆕 已开始新对话 New conversation. 直接发消息布置任务吧。")
                    .await?;
            }
            "/tasks" => self.cmd_tasks().await?,
            "/stop" => self.cmd_stop().await?,
            "/status" => self.cmd_status().await?,
            "" => self.new_task(&text).await?,
            _ => {
                self.say("未知命令 unknown command。发送 /help 查看用法。")
                    .await?;
            }
        }
        Ok(())
    }

    fn reject(&self, frm: &str, name: Option<&str>) -> anyhow::Result<()> {
        if let Some(name) = name {
            // a private chat (latest 5): while this loop is polling, getUpdates-based "detect chat id" sees nothing,
            // so the Connections page reads these instead
            let mut seen: Vec<Value> = self
                .kv("tg_seen_chats", json!([]))
                .as_array()
                .cloned()
                .unwrap_or_default()
                .into_iter()
                .filter(|x| x.get("chat_id").and_then(Value::as_str) != Some(frm))
                .collect();
            seen.push(json!({"chat_id": frm, "name": take_chars(name, 60), "ts": now_ts()}));
            let skip = seen.len().saturating_sub(5);
            self.store
                .kv_set("tg_seen_chats", &Value::Array(seen.split_off(skip)))?;
        }
        let mut last = self.rejected_log.lock();
        if now_ts() - *last > 60.0 {
            // rate-limited audit of strangers poking the bot
            *last = now_ts();
            drop(last);
            self.audit(
                &self.actor,
                "telegram.rejected",
                Audit {
                    result: "denied".into(),
                    detail: Some(json!({"from_chat": frm})),
                    ..Default::default()
                },
            );
        }
        Ok(())
    }

    // ------------------------------------------------------------ commands
    /// A text message: a new task in the Telegram conversation.
    pub async fn new_task(&self, text: &str) -> anyhow::Result<()> {
        let conv = self.kv("tg_conv", json!(""));
        let conv = if truthy(&conv) { conv } else { Value::Null };
        let r = self
            .rt(
                "POST",
                "/api/chat",
                Some(json!({"message": text, "conversation_id": conv})),
            )
            .await?;
        self.store
            .kv_set("tg_conv", r.get("conversation_id").unwrap_or(&Value::Null))?;
        let tid = get_str(&r, "task_id");
        self.audit(
            "user",
            "telegram.task",
            Audit {
                task_id: tid.clone(),
                result: "success".into(),
                detail: Some(json!({"chars": text.chars().count()})),
                ..Default::default()
            },
        );
        let mid = self.say("🤖 收到，开始处理… <i>Working on it</i>").await?;
        self.tracked
            .lock()
            .insert(tid, json!({"msg_id": mid, "last": ""}));
        Ok(())
    }

    async fn cmd_tasks(&self) -> anyhow::Result<()> {
        let ts = self.rt("GET", "/api/tasks?limit=8", None).await?["tasks"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        if ts.is_empty() {
            self.say("还没有任务。").await?;
            return Ok(());
        }
        let lines: Vec<String> = ts
            .iter()
            .map(|t| {
                let s = get_str(t, "status");
                let icon = if !TERMINAL.contains(&s.as_str()) {
                    "⏳"
                } else if s == "COMPLETED" {
                    "✅"
                } else {
                    "⚠️"
                };
                format!(
                    "{icon} <b>{}</b> · {}",
                    status_zh(&s),
                    esc(take_chars(&get_str(t, "goal"), 60))
                )
            })
            .collect();
        let link = self.public_url("tasks");
        let tail = if link.is_empty() {
            String::new()
        } else {
            format!("\n\n<a href=\"{link}\">在网页中查看</a>")
        };
        self.say(&format!(
            "📋 <b>最近任务 Recent tasks</b>\n{}{tail}",
            lines.join("\n")
        ))
        .await?;
        Ok(())
    }

    async fn cmd_stop(&self) -> anyhow::Result<()> {
        let conv = py_str(&self.kv("tg_conv", json!("")));
        let conv = if conv == "None" { String::new() } else { conv };
        let ts = self.rt("GET", "/api/tasks?limit=30", None).await?["tasks"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        let active: Vec<&Value> = ts
            .iter()
            .filter(|t| {
                !TERMINAL.contains(&get_str(t, "status").as_str())
                    && (conv.is_empty()
                        || t.get("conv_id").and_then(Value::as_str) == Some(conv.as_str()))
            })
            .collect();
        if active.is_empty() {
            self.say("当前没有正在执行的任务。").await?;
            return Ok(());
        }
        for t in &active {
            self.rt(
                "POST",
                &format!("/api/tasks/{}/cancel", get_str(t, "id")),
                Some(json!({})),
            )
            .await?;
        }
        self.say(&format!("⏹ 已停止 {} 个任务 stopped.", active.len()))
            .await?;
        Ok(())
    }

    async fn cmd_status(&self) -> anyhow::Result<()> {
        let pend = self.store.approvals(Some("pending"), 20)?;
        let mut lines = vec![format!("🛡 待审批 Pending approvals: <b>{}</b>", pend.len())];
        for a in pend.iter().take(5) {
            lines.push(format!("  • {}", esc(&summary_title(a))));
        }
        self.say(&lines.join("\n")).await?;
        for a in pend.iter().take(5) {
            self.send_approval(a).await?;
        }
        Ok(())
    }

    // ------------------------------------------------------------ progress tracking
    /// The progress message of a task.
    pub fn progress_text(t: &Value) -> String {
        let status = get_str(t, "status");
        let st = status_zh(&status);
        let plan = t
            .get("plan")
            .filter(|p| truthy(p))
            .cloned()
            .unwrap_or(json!({}));
        let obj = plan
            .get("objective")
            .filter(|o| truthy(o))
            .map(py_str)
            .unwrap_or_else(|| get_str(t, "goal"));
        let mut lines = vec![format!("🤖 <b>{st}</b> · {}", esc(take_chars(&obj, 120)))];
        for s in plan
            .get("steps")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
            .iter()
            .take(8)
        {
            let ss = s
                .get("status")
                .map(py_str)
                .unwrap_or_else(|| "pending".into());
            let desc = s.get("description").map(py_str).unwrap_or_default();
            lines.push(format!("{} {}", mark(&ss), esc(take_chars(&desc, 90))));
        }
        if status == "WAITING_APPROVAL" {
            lines.push("\n🛡 需要你批准，见下一条消息 ⬇️".into());
        }
        lines.join("\n")
    }

    async fn track_loop(self: Arc<Self>) {
        loop {
            tokio::time::sleep(Duration::from_secs(3)).await;
            let items: Vec<(String, Value)> = self
                .tracked
                .lock()
                .iter()
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect();
            for (tid, tr) in items {
                let Ok(t) = self.rt("GET", &format!("/api/tasks/{tid}"), None).await else {
                    continue;
                };
                let txt = Self::progress_text(&t);
                let msg_id = tr.get("msg_id").and_then(Value::as_i64).filter(|m| *m != 0);
                if txt != get_str(&tr, "last")
                    && msg_id.is_some()
                    && self.edit(msg_id, &txt, None).await.is_ok()
                {
                    if let Some(rec) = self.tracked.lock().get_mut(&tid) {
                        rec["last"] = json!(txt);
                    }
                }
                if TERMINAL.contains(&get_str(&t, "status").as_str()) {
                    self.tracked.lock().remove(&tid);
                    let _ = self.send_result(&t).await;
                }
            }
        }
    }

    /// The final message of a task (head line, then the result as Telegram HTML).
    pub async fn send_result(&self, t: &Value) -> anyhow::Result<()> {
        let head = match get_str(t, "status").as_str() {
            "COMPLETED" => "✅ 完成 Done",
            "FAILED" => "⚠️ 失败 Failed",
            "CANCELLED" => "⏹ 已取消 Cancelled",
            _ => "",
        };
        let body = t
            .get("result")
            .filter(|r| truthy(r))
            .or_else(|| t.get("error").filter(|e| truthy(e)))
            .map(py_str)
            .unwrap_or_default();
        if self.config().is_none() {
            return Ok(());
        }
        self.say(&format!("<b>{head}</b>")).await?;
        for part in md_to_tg(&body, 3900) {
            self.say(&part).await?;
        }
        Ok(())
    }

    // ------------------------------------------------------------ approvals & takeover
    /// Push an approval card with ✅ / ❌ buttons.
    pub async fn send_approval(&self, ap: &Value) -> anyhow::Result<()> {
        let s = ap
            .get("summary")
            .filter(|s| truthy(s))
            .cloned()
            .unwrap_or(json!({}));
        let mut lines = vec![
            "🛡 <b>需要你批准 Approval needed</b>".to_string(),
            format!(
                "<b>{}</b> · 风险 {}",
                esc(&summary_title(ap)),
                esc(&get_str(ap, "risk"))
            ),
        ];
        if let Some(r) = ap.get("reason").filter(|r| truthy(r)) {
            lines.push(format!("<i>{}</i>", esc(take_chars(&py_str(r), 300))));
        }
        for f in s
            .get("fields")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
            .iter()
            .take(6)
        {
            let (Some(k), Some(v)) = (f.get(0), f.get(1)) else {
                continue;
            };
            if truthy(v) {
                lines.push(format!(
                    "{}: <code>{}</code>",
                    esc(&py_str(k)),
                    esc(take_chars(&py_str(v), 200))
                ));
            }
        }
        if let Some(items) = s
            .get("items")
            .and_then(Value::as_array)
            .filter(|a| !a.is_empty())
        {
            for it in items.iter().take(15) {
                let frm = it
                    .get("from")
                    .filter(|f| truthy(f))
                    .map(py_str)
                    .unwrap_or_default();
                let frm = TAGS0.replace_all(&frm, "").replace('"', "");
                let subj = it
                    .get("subject")
                    .filter(|f| truthy(f))
                    .map(py_str)
                    .unwrap_or_default();
                lines.push(format!(
                    "• {} — {}",
                    esc(take_chars(frm.trim(), 40)),
                    esc(take_chars(&subj, 50))
                ));
            }
            if items.len() > 15 {
                lines.push(format!("… 共 {} 封", items.len()));
            }
        }
        if let Some(b) = s.get("body").filter(|b| truthy(b)) {
            lines.push(format!("<pre>{}</pre>", esc(take_chars(&py_str(b), 1500))));
        }
        let link = self.public_url("chat");
        let aid = get_str(ap, "id");
        let mut rows = vec![json!([
            {"text": "✅ 批准 Approve", "callback_data": format!("ap:{aid}:y")},
            {"text": "❌ 拒绝 Deny", "callback_data": format!("ap:{aid}:n")},
        ])];
        if !link.is_empty() {
            rows.push(json!([{"text": "🌐 在网页中查看/修改", "url": link}]));
        }
        self.send(
            &lines.join("\n"),
            Some(json!({"inline_keyboard": rows})),
            true,
        )
        .await?;
        Ok(())
    }

    /// The button opens that task's browser tab (not just the Browser page): with several tasks the right one shows.
    pub async fn send_takeover(&self, reason: &str, task_id: &str) -> anyhow::Result<()> {
        let link = self.public_url(&if task_id.is_empty() {
            "browser".to_string()
        } else {
            format!("browser/{task_id}")
        });
        let markup = if link.is_empty() {
            None
        } else {
            Some(
                json!({"inline_keyboard": [[{"text": "🖐 打开浏览器接管 Take over", "url": link}]]}),
            )
        };
        let mid = if link.is_empty() {
            String::new()
        } else {
            format!("{}\n\n", esc(&link))
        };
        self.send(
            &format!(
                "🙋 <b>需要你接管浏览器</b>\n{}\n\n{mid}完成后在网页里点「交还给 Agent」。",
                esc(take_chars(reason, 300))
            ),
            markup,
            true,
        )
        .await?;
        Ok(())
    }

    /// An inline button press: resolve the approval through the routes' resolver and update the card.
    pub async fn on_callback(&self, cq: &Value) -> anyhow::Result<()> {
        let Some((token, _)) = self.config() else {
            return Ok(());
        };
        let data = cq
            .get("data")
            .filter(|d| truthy(d))
            .map(py_str)
            .unwrap_or_default();
        let cq_id = cq.get("id").cloned().unwrap_or(Value::Null);
        let Some(m) = CALLBACK.captures(&data) else {
            self.api(
                "answerCallbackQuery",
                &token,
                json!({"callback_query_id": cq_id}),
            )
            .await?;
            return Ok(());
        };
        let (aid, yes) = (m[1].to_string(), &m[2] == "y");
        let msg = cq.get("message").cloned().unwrap_or(json!({}));
        let resolver = self.resolver.read().clone();
        let res: Result<Value, String> = match resolver {
            Some(r) => r(
                aid,
                json!({"decision": if yes { "approve" } else { "deny" }, "scope": "ONCE"}),
                "telegram".into(),
            )
            .await
            .map_err(|e| e.detail_text()),
            None => Err("'NoneType' object is not callable".into()),
        };
        let outcome = match res {
            Ok(_) if !yes => "❌ 已拒绝 Denied".to_string(),
            Ok(r) => {
                let status = get_str(&r, "status");
                let result = r
                    .get("result")
                    .filter(|x| truthy(x))
                    .cloned()
                    .unwrap_or(json!({}));
                if status == "approved" && get_str(&result, "status") == "ok" {
                    "✅ 已批准并执行 Approved & executed".to_string()
                } else if status == "approved" {
                    let what = result
                        .get("error")
                        .filter(|e| truthy(e))
                        .or_else(|| result.get("status"))
                        .map(py_str)
                        .unwrap_or_else(|| "None".into());
                    format!("⚠️ 已批准，但执行结果：{}", esc(take_chars(&what, 200)))
                } else {
                    let what = r
                        .get("reason")
                        .filter(|e| truthy(e))
                        .or_else(|| r.get("status"))
                        .map(py_str)
                        .unwrap_or_else(|| "None".into());
                    format!("⚠️ {}", esc(&what))
                }
            }
            // already resolved elsewhere, etc.
            Err(e) => format!("ℹ️ {}", esc(take_chars(&e, 200))),
        };
        let plain = TAGS.replace_all(&outcome, "").into_owned();
        self.api(
            "answerCallbackQuery",
            &token,
            json!({"callback_query_id": cq_id, "text": take_chars(&plain, 190)}),
        )
        .await?;
        let old = msg
            .get("text")
            .filter(|t| truthy(t))
            .map(py_str)
            .unwrap_or_default();
        let _ = self
            .edit(
                msg.get("message_id").and_then(Value::as_i64),
                &format!("{}\n\n<b>{outcome}</b>", esc(take_chars(&old, 3500))),
                Some(json!({"inline_keyboard": []})),
            )
            .await;
        Ok(())
    }
}

/// `(a['summary'] or {}).get('title') or a['tool']`.
fn summary_title(a: &Value) -> String {
    a.get("summary")
        .and_then(|s| s.get("title"))
        .filter(|t| truthy(t))
        .map(py_str)
        .unwrap_or_else(|| get_str(a, "tool"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn telegram_markdown() {
        let out = md_to_tg(
            "## Title\n**bold** and `code` <x>\n| a | b |\n|---|---|\n| 1 | 2 |\n- item",
            3900,
        )
        .join("\n");
        assert!(
            out.contains("<b>Title</b>")
                && out.contains("<b>bold</b>")
                && out.contains("<code>code</code>")
        );
        assert!(out.contains("&lt;x&gt;"));
        assert!(
            out.contains("<pre>a | b\n1 | 2</pre>") && out.contains("• item"),
            "{out}"
        );
        let long = md_to_tg(&"line\n".repeat(3000), 3900);
        assert!(long.len() > 1 && long.iter().all(|p| p.chars().count() <= 3900));
        assert_eq!(md_to_tg("", 3900), vec!["（空）"]);
        assert_eq!(
            md_to_tg("*it* [l](https://a.b)", 3900),
            vec!["<i>it</i> <a href=\"https://a.b\">l</a>"]
        );
        let code = md_to_tg("```\n<a>\n```", 3900);
        assert_eq!(code, vec!["<pre>\n&lt;a&gt;\n</pre>"]);
    }

    #[test]
    fn splitlines_like_python() {
        assert_eq!(splitlines("a\r\nb\rc\n"), vec!["a", "b", "c"]);
        assert_eq!(splitlines("a\n\nb"), vec!["a", "", "b"]);
    }

    #[tokio::test]
    async fn remembers_rejected_private_chats() {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(Store::new(dir.path()).unwrap());
        let bot = TelegramBot::new(
            Arc::new(Config::default()),
            store.clone(),
            "http://x".into(),
            None,
        );
        let up = json!({"message": {"chat": {"id": 415690541, "type": "private", "first_name": "Lucas", "last_name": "Lu"}, "text": "/start"}});
        bot.handle(&up, "999").await.unwrap();
        bot.handle(&json!({"message": {"chat": {"id": -100, "type": "group", "title": "g"}, "text": "hi"}}), "999")
            .await
            .unwrap();
        let seen = store.kv_get("tg_seen_chats").unwrap().unwrap();
        let got: Vec<(String, String)> = seen
            .as_array()
            .unwrap()
            .iter()
            .map(|x| (get_str(x, "chat_id"), get_str(x, "name")))
            .collect();
        assert_eq!(got, vec![("415690541".to_string(), "Lucas Lu".to_string())]);
    }

    #[test]
    fn progress_text_marks_steps() {
        let t = json!({"status": "WAITING_APPROVAL", "goal": "g <x>", "plan": {"steps": [{"status": "done", "description": "a"}, {"description": "b"}]}});
        assert_eq!(
            TelegramBot::progress_text(&t),
            "🤖 <b>等待审批</b> · g &lt;x&gt;\n✓ a\n○ b\n\n🛡 需要你批准，见下一条消息 ⬇️"
        );
    }
}
