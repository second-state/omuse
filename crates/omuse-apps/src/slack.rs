//! Slack connector (Slack Web API with a bot token xoxb-… or a user token xoxp-…).
//!
//! Bot token: the Slack app must be invited to the channels it should read (/invite @app).
//! User token: acts as the user (can also search); only the scopes the user granted work.

use std::collections::HashMap;
use std::sync::LazyLock;
use std::time::Duration;

use chrono::TimeZone;
use indexmap::IndexMap;
use omuse_common::util::{get_str, get_truthy, py_str, take_chars, truthy};
use parking_lot::Mutex;
use regex::Regex;
use serde_json::{json, Value};

use crate::py::httpx_name;

/// Python `SlackError`.
#[derive(Debug, Clone, thiserror::Error)]
#[error("{0}")]
pub struct SlackError(pub String);

fn hint(err: &str) -> Option<&'static str> {
    Some(match err {
        "not_in_channel" => "机器人还不在这个频道里：请在频道里输入 /invite @你的应用名 (invite the app to the channel)",
        "channel_not_found" => "找不到这个频道，或者没有权限 (channel not found)",
        "missing_scope" => "Slack 应用缺少权限范围 (missing OAuth scope)",
        "invalid_auth" => "Slack 令牌无效 (invalid token)",
        "token_revoked" => "Slack 令牌已被撤销 (token revoked)",
        "not_allowed_token_type" => "这个操作需要用户令牌 xoxp- (needs a user token)",
        "ratelimited" => "Slack 请求过于频繁，请稍后再试 (rate limited)",
        _ => return None,
    })
}

fn arr(v: Option<&Value>) -> &[Value] {
    v.and_then(Value::as_array)
        .map(|a| a.as_slice())
        .unwrap_or(&[])
}

fn clamp(limit: i64, default: i64, max: i64) -> i64 {
    (if limit == 0 { default } else { limit }).clamp(1, max)
}

/// The Slack Web API client for one token.
pub struct Slack {
    client: reqwest::Client,
    api: String,
    token: String,
    timeout: Duration,
    users: Mutex<HashMap<String, String>>,
    chans: Mutex<Option<IndexMap<String, Value>>>,
}

impl Slack {
    /// `api` is `cfg.endpoints.slack` (`https://slack.com/api`).
    pub fn new(client: reqwest::Client, api: &str, token: &str) -> Self {
        Self {
            client,
            api: api.trim_end_matches('/').to_string(),
            token: token.to_string(),
            timeout: Duration::from_secs(30),
            users: Mutex::new(HashMap::new()),
            chans: Mutex::new(None),
        }
    }

    pub fn with_timeout(mut self, secs: f64) -> Self {
        self.timeout = Duration::from_secs_f64(secs);
        self
    }

    /// `"user"` for xoxp- tokens, else `"bot"`.
    pub fn token_type(&self) -> &'static str {
        if self.token.starts_with("xoxp-") {
            "user"
        } else {
            "bot"
        }
    }

    /// One Web API call; `None`/"" parameters are dropped. `chat.postMessage` goes as JSON, the rest as a form.
    pub async fn call(&self, method: &str, params: &[(&str, Value)]) -> Result<Value, SlackError> {
        let params: Vec<(&str, &Value)> = params
            .iter()
            .filter(|(_, v)| !v.is_null() && v.as_str() != Some(""))
            .map(|(k, v)| (*k, v))
            .collect();
        for attempt in 0..3 {
            let rb = self
                .client
                .post(format!("{}/{}", self.api, method))
                .timeout(self.timeout)
                .header("Authorization", format!("Bearer {}", self.token));
            let rb = if method == "chat.postMessage" {
                let body: serde_json::Map<String, Value> = params
                    .iter()
                    .map(|(k, v)| (k.to_string(), (*v).clone()))
                    .collect();
                rb.header("Content-Type", "application/json; charset=utf-8")
                    .json(&Value::Object(body))
            } else {
                let form: Vec<(&str, String)> =
                    params.iter().map(|(k, v)| (*k, py_str(v))).collect();
                rb.form(&form)
            };
            let r = rb
                .send()
                .await
                .map_err(|e| SlackError(format!("无法连接 Slack (network): {}", httpx_name(&e))))?;
            let status = r.status().as_u16();
            if status == 429 && attempt < 2 {
                let wait = r
                    .headers()
                    .get("retry-after")
                    .and_then(|v| v.to_str().ok())
                    .filter(|s| !s.is_empty())
                    .and_then(|s| s.trim().parse::<f64>().ok())
                    .unwrap_or(1.0)
                    .min(5.0);
                tokio::time::sleep(Duration::from_secs_f64(wait.max(0.0))).await;
                continue;
            }
            let data: Value = r
                .json()
                .await
                .map_err(|_| SlackError(format!("Slack HTTP {}", status)))?;
            if !get_truthy(&data, "ok") {
                let err = data
                    .get("error")
                    .map(py_str)
                    .unwrap_or_else(|| "unknown_error".into());
                let extra = match data.get("needed").filter(|n| truthy(n)) {
                    Some(n) => format!(" (needed: {})", py_str(n)),
                    None => String::new(),
                };
                return Err(SlackError(format!(
                    "Slack: {}{}",
                    hint(&err).unwrap_or(&err),
                    extra
                )));
            }
            return Ok(data);
        }
        Err(SlackError(
            hint("ratelimited").unwrap_or_default().to_string(),
        ))
    }

    // ---------------------------------------------------------------- identity / lookup

    /// `auth.test`: team, ids and the token type.
    pub async fn auth_test(&self) -> Result<Value, SlackError> {
        let d = self.call("auth.test", &[]).await?;
        let g = |k: &str| d.get(k).cloned().unwrap_or(json!(""));
        Ok(
            json!({"team": g("team"), "team_id": g("team_id"), "user": g("user"), "user_id": g("user_id"),
                  "bot_id": g("bot_id"), "url": g("url"), "token_type": self.token_type()}),
        )
    }

    /// Display name of a user id (cached; the id itself when lookup fails).
    pub async fn user_name(&self, uid: &str) -> String {
        if uid.is_empty() {
            return String::new();
        }
        if let Some(n) = self.users.lock().get(uid) {
            return n.clone();
        }
        let name = match self.call("users.info", &[("user", json!(uid))]).await {
            Ok(d) => {
                let u = d
                    .get("user")
                    .filter(|u| truthy(u))
                    .cloned()
                    .unwrap_or(json!({}));
                let p = u
                    .get("profile")
                    .filter(|p| truthy(p))
                    .cloned()
                    .unwrap_or(json!({}));
                [
                    get_str(&p, "display_name"),
                    get_str(&p, "real_name"),
                    get_str(&u, "name"),
                ]
                .into_iter()
                .find(|s| !s.is_empty())
                .unwrap_or_else(|| uid.to_string())
            }
            Err(_) => uid.to_string(),
        };
        self.users.lock().insert(uid.to_string(), name.clone());
        name
    }

    /// Channels, groups, DMs the token can see (cached for the client's lifetime).
    pub async fn channels(&self, limit: usize) -> Result<Vec<Value>, SlackError> {
        if self.chans.lock().is_none() {
            let mut out: IndexMap<String, Value> = IndexMap::new();
            let mut cursor: Option<String> = None;
            for _ in 0..10 {
                let d = self
                    .call(
                        "conversations.list",
                        &[
                            ("types", json!("public_channel,private_channel,im,mpim")),
                            ("exclude_archived", json!("true")),
                            ("limit", json!(200)),
                            (
                                "cursor",
                                cursor.clone().map(Value::String).unwrap_or(Value::Null),
                            ),
                        ],
                    )
                    .await?;
                for ch in arr(d.get("channels")) {
                    let id = get_str(ch, "id");
                    let is_im = get_truthy(ch, "is_im");
                    let mut name = get_str(ch, "name");
                    if name.is_empty() {
                        name = if is_im {
                            format!("DM:{}", self.user_name(&get_str(ch, "user")).await)
                        } else {
                            id.clone()
                        };
                    }
                    let is_member = match ch.get("is_member") {
                        Some(m) => truthy(m),
                        None => is_im,
                    };
                    let topic = ch
                        .get("topic")
                        .filter(|t| truthy(t))
                        .map(|t| get_str(t, "value"))
                        .unwrap_or_default();
                    out.insert(
                        id.clone(),
                        json!({"id": id, "name": name, "is_private": get_truthy(ch, "is_private"), "is_member": is_member,
                               "is_im": is_im, "topic": take_chars(&topic, 120)}),
                    );
                }
                cursor = d
                    .get("response_metadata")
                    .and_then(|m| m.get("next_cursor"))
                    .filter(|c| truthy(c))
                    .map(py_str);
                if cursor.is_none() || out.len() >= limit {
                    break;
                }
            }
            *self.chans.lock() = Some(out);
        }
        let g = self.chans.lock();
        Ok(g.as_ref()
            .map(|m| m.values().take(limit).cloned().collect())
            .unwrap_or_default())
    }

    /// A channel by id or `#name`.
    pub async fn resolve_channel(&self, reference: &str) -> Result<Value, SlackError> {
        static ID: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[CGD][A-Z0-9]{6,}$").unwrap());
        let r = reference.trim();
        if ID.is_match(r) {
            let known = self
                .channels(200)
                .await?
                .into_iter()
                .find(|c| get_str(c, "id") == r);
            return Ok(known.unwrap_or_else(|| json!({"id": r, "name": r})));
        }
        let name = r.trim_start_matches('#').to_lowercase();
        for c in self.channels(200).await? {
            if get_str(&c, "name").to_lowercase() == name {
                return Ok(c);
            }
        }
        Err(SlackError(format!(
            "找不到频道「{}」(channel not found) — 用 slack_list_channels 查看可用频道",
            r
        )))
    }

    // ---------------------------------------------------------------- messages

    /// A raw message → the agent-facing record (mentions resolved, Slack links as markdown).
    pub async fn msg(&self, m: &Value, channel: &Value) -> Value {
        static USER: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"<@([UW][A-Z0-9]+)>").unwrap());
        static CHAN: LazyLock<Regex> =
            LazyLock::new(|| Regex::new(r"<#([CG][A-Z0-9]+)\|?([^>]*)>").unwrap());
        static LINK: LazyLock<Regex> =
            LazyLock::new(|| Regex::new(r"<(https?://[^|>]+)\|([^>]+)>").unwrap());
        let raw = get_str(m, "text");
        let mut text = String::new();
        let mut last = 0;
        for c in USER.captures_iter(&raw) {
            let whole = c.get(0).unwrap();
            text.push_str(&raw[last..whole.start()]);
            text.push('@');
            text.push_str(&self.user_name(&c[1]).await);
            last = whole.end();
        }
        text.push_str(&raw[last..]);
        let text = CHAN
            .replace_all(&text, |c: &regex::Captures| {
                format!("#{}", if c[2].is_empty() { &c[1] } else { &c[2] })
            })
            .to_string();
        let text = LINK.replace_all(&text, "[$2]($1)").to_string();
        let user_id = get_str(m, "user");
        let user = if !user_id.is_empty() {
            Value::String(self.user_name(&user_id).await)
        } else {
            m.get("username")
                .filter(|u| truthy(u))
                .cloned()
                .unwrap_or_else(|| m.get("bot_id").cloned().unwrap_or(json!("")))
        };
        let ts = get_str(m, "ts");
        let secs = if ts.is_empty() {
            0.0
        } else {
            ts.parse::<f64>().unwrap_or(0.0)
        };
        let time = chrono::Local
            .timestamp_opt(secs.floor() as i64, 0)
            .single()
            .map(|t| t.format("%Y-%m-%d %H:%M").to_string())
            .unwrap_or_default();
        json!({
            "channel": channel.get("name").cloned().unwrap_or(json!("")), "channel_id": channel.get("id").cloned().unwrap_or(json!("")),
            "ts": m.get("ts").cloned().unwrap_or(json!("")), "thread_ts": m.get("thread_ts").cloned().unwrap_or(json!("")),
            "reply_count": m.get("reply_count").cloned().unwrap_or(json!(0)),
            "user": user, "user_id": m.get("user").cloned().unwrap_or(json!("")), "bot": get_truthy(m, "bot_id"),
            "time": time, "text": take_chars(&text, 4000),
            "files": arr(m.get("files")).iter().map(|f| f.get("name").cloned().unwrap_or(json!(""))).collect::<Vec<_>>(),
        })
    }

    async fn messages(&self, ch: &Value, d: &Value) -> Vec<Value> {
        let mut out = Vec::new();
        for m in arr(d.get("messages")) {
            out.push(self.msg(m, ch).await);
        }
        out
    }

    /// Recent messages of a channel (`oldest` is a Slack ts to start after).
    pub async fn history(
        &self,
        reference: &str,
        limit: i64,
        oldest: &str,
    ) -> Result<Value, SlackError> {
        let ch = self.resolve_channel(reference).await?;
        let d = self
            .call(
                "conversations.history",
                &[
                    ("channel", ch["id"].clone()),
                    ("limit", json!(clamp(limit, 20, 100))),
                    (
                        "oldest",
                        if oldest.is_empty() {
                            Value::Null
                        } else {
                            json!(oldest)
                        },
                    ),
                ],
            )
            .await?;
        Ok(
            json!({"channel": ch.get("name").cloned().unwrap_or(Value::Null), "channel_id": ch["id"].clone(), "messages": self.messages(&ch, &d).await}),
        )
    }

    /// Replies of a thread.
    pub async fn thread(&self, reference: &str, ts: &str, limit: i64) -> Result<Value, SlackError> {
        let ch = self.resolve_channel(reference).await?;
        let d = self
            .call(
                "conversations.replies",
                &[
                    ("channel", ch["id"].clone()),
                    ("ts", json!(ts)),
                    ("limit", json!(clamp(limit, 50, 200))),
                ],
            )
            .await?;
        Ok(
            json!({"channel": ch.get("name").cloned().unwrap_or(Value::Null), "channel_id": ch["id"].clone(), "messages": self.messages(&ch, &d).await}),
        )
    }

    /// `search.messages` (user tokens only).
    pub async fn search(&self, query: &str, limit: i64) -> Result<Value, SlackError> {
        if !self.token.starts_with("xoxp-") {
            return Err(SlackError("搜索需要 Slack 用户令牌 (xoxp-, scope search:read)；机器人令牌不能搜索。可以改用 slack_read_channel".into()));
        }
        let d = self
            .call(
                "search.messages",
                &[
                    ("query", json!(query)),
                    ("count", json!(clamp(limit, 20, 50))),
                    ("sort", json!("timestamp")),
                ],
            )
            .await?;
        let msgs = d
            .get("messages")
            .filter(|m| truthy(m))
            .cloned()
            .unwrap_or(json!({}));
        let mut out = Vec::new();
        for m in arr(msgs.get("matches")) {
            let ch = m
                .get("channel")
                .filter(|c| truthy(c))
                .cloned()
                .unwrap_or(json!({}));
            let info = json!({"id": ch.get("id").cloned().unwrap_or(json!("")), "name": ch.get("name").cloned().unwrap_or(json!(""))});
            let mut rec = self.msg(m, &info).await;
            rec["permalink"] = m.get("permalink").cloned().unwrap_or(json!(""));
            out.push(rec);
        }
        Ok(json!({"total": msgs.get("total").cloned().unwrap_or(json!(0)), "messages": out}))
    }

    /// Post a message (optionally in a thread).
    pub async fn post(
        &self,
        reference: &str,
        text: &str,
        thread_ts: &str,
    ) -> Result<Value, SlackError> {
        let ch = self.resolve_channel(reference).await?;
        let d = self
            .call(
                "chat.postMessage",
                &[
                    ("channel", ch["id"].clone()),
                    ("text", json!(take_chars(text, 39000))),
                    (
                        "thread_ts",
                        if thread_ts.is_empty() {
                            Value::Null
                        } else {
                            json!(thread_ts)
                        },
                    ),
                    ("unfurl_links", json!(false)),
                ],
            )
            .await?;
        Ok(
            json!({"sent": true, "channel": ch.get("name").cloned().unwrap_or(Value::Null), "channel_id": ch["id"].clone(),
                  "ts": d.get("ts").cloned().unwrap_or(json!(""))}),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hints_and_token_type() {
        assert!(hint("not_in_channel").unwrap().contains("/invite"));
        let s = Slack::new(reqwest::Client::new(), "http://127.0.0.1:1/api", "xoxp-abc");
        assert_eq!(s.token_type(), "user");
        let s = Slack::new(reqwest::Client::new(), "http://127.0.0.1:1/api", "xoxb-abc");
        assert_eq!(s.token_type(), "bot");
    }

    #[tokio::test]
    async fn message_formatting_without_lookups() {
        let s = Slack::new(reqwest::Client::new(), "http://127.0.0.1:1/api", "xoxb-abc");
        s.users.lock().insert("U2".into(), "John".into());
        let m = json!({"ts": "1790000000.000100", "user": "U2", "text": "hi <@U2> in <#C1|general> see <https://x.test/a|docs>",
                      "files": [{"name": "a.pdf"}]});
        let r = s.msg(&m, &json!({"id": "C1", "name": "general"})).await;
        assert_eq!(
            r["text"],
            json!("hi @John in #general see [docs](https://x.test/a)")
        );
        assert_eq!(r["user"], json!("John"));
        assert_eq!(r["files"], json!(["a.pdf"]));
        assert_eq!(r["bot"], json!(false));
        assert_eq!(r["reply_count"], json!(0));
        let b = s
            .msg(
                &json!({"ts": "1.0", "bot_id": "BBOT", "text": "x"}),
                &json!({}),
            )
            .await;
        assert_eq!(b["user"], json!("BBOT"));
        assert_eq!(b["bot"], json!(true));
    }
}
