//! Health watch: notice when OMuse cannot work and tell the user, instead of failing every task silently.
//!
//! 2026-10-05: the Olares router lost the chat model for a while; OMuse fell back to a text-to-speech model and every
//! task failed with HTTP 422 for a day before anyone noticed. Now:
//!
//! * every CHECK_EVERY seconds the model router, the browser and Sentinel are checked;
//! * a component that fails two checks in a row is "down": one alert (in-app + Telegram), then at most one reminder
//!   every REMIND_AFTER seconds, and one "back to normal" message when it recovers;
//! * FAILED tasks are also grouped by cause (model / browser / mail / other): 3 with the same cause within an hour
//!   raise an alert even when the periodic checks look fine (a mailbox whose sign-in expired, a model that answers
//!   /models but not chats).

use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use indexmap::IndexMap;
use omuse_common::util::{as_f64, get_str, get_truthy, now_ts, py_str, take_chars};
use parking_lot::Mutex;
use serde_json::{json, Map, Value};

use crate::llm::auth_headers;
use crate::rt_api::{describe_reqwest_error, exc_name, RuntimeApi};

pub const CHECK_EVERY: u64 = 120;
/// Consecutive failed checks.
pub const DOWN_AFTER: i64 = 2;
/// Merchants' emails are matched to ledger orders every 6 hours.
pub const LEDGER_EVERY: f64 = 6.0 * 3600.0;
/// Mailboxes are checked every other round (≤ 8 min to an alert).
pub const MAIL_EVERY: u64 = 2;
pub const REMIND_AFTER: f64 = 6.0 * 3600.0;
/// Failed tasks with the same cause …
pub const STREAK: usize = 3;
/// … within this many seconds.
pub const STREAK_WINDOW: f64 = 3600.0;

/// Component → (Chinese label, English label).
pub const LABELS: &[(&str, (&str, &str))] = &[
    ("mail", ("邮箱", "mailbox")),
    ("model", ("模型服务", "model service")),
    ("browser", ("浏览器", "browser")),
    ("sentinel", ("安全网关 Sentinel", "Sentinel")),
    ("tasks", ("任务执行", "task runs")),
];
/// Failure cause → label.
pub const CAUSES: &[(&str, &str)] = &[
    ("model", "模型"),
    ("browser", "浏览器"),
    ("mail", "邮箱"),
    ("sentinel", "Sentinel"),
    ("timeout", "超时"),
    ("step_limit", "步数用完"),
    ("gave_up", "来源反复失败"),
    ("other", "其他"),
];

/// `CAUSES.get(cause, cause)`.
pub fn cause_label(cause: &str) -> String {
    CAUSES
        .iter()
        .find(|(k, _)| *k == cause)
        .map(|(_, v)| (*v).to_string())
        .unwrap_or_else(|| cause.to_string())
}

/// Which part of OMuse a task failure points at.
pub fn failure_cause(error: &str) -> &'static str {
    let e = error;
    let low = e.to_lowercase();
    if low.contains("llmerror")
        || e.contains("模型")
        || low.contains("model api")
        || low.contains("model unavailable")
        || low.contains("chat/completions")
    {
        return "model";
    }
    if e.contains("浏览器")
        || low.contains("browser")
        || low.contains("playwright")
        || low.contains("chromium")
    {
        return "browser";
    }
    if ["gmail", "imap", "invalid_grant", "oauth", "mailbox"]
        .iter()
        .any(|k| low.contains(k))
        || e.contains("邮箱")
    {
        return "mail";
    }
    if low.contains("sentinel") {
        return "sentinel";
    }
    if e.contains("用时上限")
        || low.contains("time limit")
        || e.contains("超时")
        || low.contains("timeout")
    {
        return "timeout";
    }
    if e.contains("步数上限") || low.contains("step limit") {
        return "step_limit";
    }
    if e.contains("反复失败") || low.contains("kept failing") || low.contains("stopped retrying")
    {
        return "gave_up";
    }
    "other"
}

/// The configured chat model is listed, can chat and is loaded.
pub fn model_check(models: &[Value], want: &str) -> (bool, String) {
    let ids: Vec<String> = models.iter().map(|m| get_str(m, "id")).collect();
    let Some(m) = models
        .iter()
        .find(|x| x.get("id").and_then(|i| i.as_str()) == Some(want))
    else {
        let shown = ids.iter().take(5).cloned().collect::<Vec<_>>().join(", ");
        return (
            false,
            format!(
                "模型 {} 不在模型服务的列表里（可能已卸载或正在加载）；当前可用：{}",
                want,
                if shown.is_empty() {
                    "无"
                } else {
                    shown.as_str()
                }
            ),
        );
    };
    let mode = {
        let s = get_str(m, "mode");
        if s.is_empty() {
            "chat".to_string()
        } else {
            s.to_lowercase()
        }
    };
    if mode != "chat" {
        return (
            false,
            format!("模型 {} 不是聊天模型（mode={}）", want, mode),
        );
    }
    let ready = {
        let s = get_str(m, "readiness");
        if s.is_empty() {
            "ready".to_string()
        } else {
            s.to_lowercase()
        }
    };
    if !matches!(ready.as_str(), "ready" | "running" | "loaded") {
        return (false, format!("模型 {} 还没就绪（{}）", want, ready));
    }
    (true, format!("{} 正常", want))
}

/// The periodic health watch. Shared behind `Arc`; `start` spawns the loop.
pub struct Health {
    rt: Arc<dyn RuntimeApi>,
    /// component -> {ok, detail, fails, since, alerted_at, down, checked_at}
    pub state: Mutex<IndexMap<String, Value>>,
    /// Alerts sent (newest last, max 50).
    pub recent: Mutex<Vec<Value>>,
    round: Mutex<u64>,
    ledger_at: Mutex<f64>,
    task: Mutex<Option<tokio::task::JoinHandle<()>>>,
    http: reqwest::Client,
}

impl Health {
    pub fn new(rt: Arc<dyn RuntimeApi>) -> Self {
        Self {
            rt,
            state: Mutex::new(IndexMap::new()),
            recent: Mutex::new(Vec::new()),
            round: Mutex::new(0),
            ledger_at: Mutex::new(0.0),
            task: Mutex::new(None),
            http: reqwest::Client::builder()
                .timeout(Duration::from_secs(20))
                .connect_timeout(Duration::from_secs(20))
                .no_proxy()
                .build()
                .expect("reqwest client"),
        }
    }

    fn settings(&self) -> Value {
        self.rt.store().settings().unwrap_or_else(|_| json!({}))
    }

    // ------------------------------------------------------------ checks
    /// The model endpoint lists the configured chat model (or at least answers).
    pub async fn check_model(&self) -> (bool, String) {
        let s = self.settings();
        let base = get_str(&s, "model_base_url")
            .trim_end_matches('/')
            .to_string();
        if base.is_empty() {
            return (false, "没有配置模型服务地址".into());
        }
        let mut req = self.http.get(format!("{}/models", base));
        for (k, v) in auth_headers(Some(&s), self.rt.config()) {
            req = req.header(k, v);
        }
        let models: Vec<Value> = match req.send().await {
            Err(e) => {
                return (
                    false,
                    format!(
                        "连不上模型服务：{}",
                        exc_name(&anyhow::anyhow!(describe_reqwest_error(&e)))
                    ),
                )
            }
            Ok(r) => {
                let status = r.status().as_u16();
                if status >= 400 {
                    return (false, format!("模型服务返回 HTTP {}", status));
                }
                match r.json::<Value>().await {
                    Ok(j) => j
                        .get("data")
                        .and_then(|d| d.as_array())
                        .map(|a| a.iter().filter(|m| m.is_object()).cloned().collect())
                        .unwrap_or_default(),
                    Err(_) => return (false, "连不上模型服务：JSONDecodeError".into()),
                }
            }
        };
        if models.is_empty() {
            // a plain OpenAI-compatible server may not list models: don't call that an outage
            return (true, "模型服务在线（未列出模型）".into());
        }
        model_check(&models, &get_str(&s, "model_name"))
    }

    /// The browser broker (through Sentinel) is usable.
    pub async fn check_browser(&self) -> (bool, String) {
        let st = match self
            .rt
            .sentinel("GET", "/internal/browser_state", None, 15.0)
            .await
        {
            Ok(v) => v,
            Err(e) => return (false, format!("连不上安全网关 Sentinel：{}", exc_name(&e))),
        };
        let mode = get_str(&st, "mode");
        if mode == "unknown" || mode == "offline" || get_truthy(&st, "error") {
            return (
                false,
                format!(
                    "浏览器服务不可用：{}",
                    take_chars(&get_str(&st, "error"), 120)
                ),
            );
        }
        (true, "浏览器正常".into())
    }

    /// `None` = no mailbox connected (nothing to watch).
    pub async fn check_mail(&self) -> Option<(bool, String)> {
        let r = match self
            .rt
            .sentinel("GET", "/internal/mail_check", None, 90.0)
            .await
        {
            Ok(v) => v,
            Err(e) => {
                if exc_name(&e).contains("Connect") {
                    return None; // Sentinel itself is down: the sentinel check reports that
                }
                return Some((false, format!("邮箱检查没有完成：{}", exc_name(&e))));
            }
        };
        let accs: Vec<Value> = r
            .get("accounts")
            .and_then(|a| a.as_array())
            .cloned()
            .unwrap_or_default();
        if accs.is_empty() {
            return None;
        }
        let bad: Vec<&Value> = accs.iter().filter(|a| !get_truthy(a, "ok")).collect();
        if !bad.is_empty() {
            let msg = bad
                .iter()
                .take(3)
                .map(|a| {
                    format!(
                        "{} 连不上（{}）",
                        a.get("email").map(py_str).unwrap_or_else(|| "None".into()),
                        take_chars(&get_str(a, "error"), 100)
                    )
                })
                .collect::<Vec<_>>()
                .join("；");
            return Some((false, msg));
        }
        Some((true, format!("{} 个邮箱正常", accs.len())))
    }

    /// One round of checks: component → (ok, detail), in the Python order.
    pub async fn run_checks(&self) -> IndexMap<String, (bool, String)> {
        let mut out = IndexMap::new();
        out.insert("model".to_string(), self.check_model().await);
        let round = {
            let mut r = self.round.lock();
            *r += 1;
            *r
        };
        if MAIL_EVERY <= 1 || round % MAIL_EVERY == 1 {
            // IMAP sign-ins are slower: every other round
            if let Some(m) = self.check_mail().await {
                out.insert("mail".to_string(), m);
            }
        }
        let (b_ok, b_detail) = self.check_browser().await;
        if !b_ok && b_detail.contains("Sentinel") {
            out.insert("sentinel".to_string(), (false, b_detail));
        } else {
            out.insert("sentinel".to_string(), (true, "Sentinel 正常".to_string()));
            out.insert("browser".to_string(), (b_ok, b_detail));
        }
        out
    }

    // ------------------------------------------------------------ state + alerts
    /// Record a check result; alert when a component goes down (after `DOWN_AFTER` failures, or at once with
    /// `immediate`), remind at most every `REMIND_AFTER`, and say when it is back.
    pub async fn observe(&self, comp: &str, ok: bool, detail: &str, immediate: bool) {
        let now = now_ts();
        let alert: Option<(String, bool)> = {
            let mut state = self.state.lock();
            let st = state.entry(comp.to_string()).or_insert_with(|| {
                json!({"ok": true, "detail": "", "fails": 0, "since": now, "alerted_at": 0.0, "down": false})
            });
            let o = st.as_object_mut().unwrap();
            o.insert("detail".into(), json!(detail));
            o.insert("checked_at".into(), json!(now));
            if ok {
                o.insert("fails".into(), json!(0));
                let mut out = None;
                if get_truthy(st, "down") {
                    let o = st.as_object_mut().unwrap();
                    o.insert("down".into(), json!(false));
                    o.insert("since".into(), json!(now));
                    if as_f64(&st["alerted_at"]).unwrap_or(0.0) != 0.0 {
                        out = Some((format!("已恢复正常：{}", detail), true));
                    }
                    st.as_object_mut()
                        .unwrap()
                        .insert("alerted_at".into(), json!(0.0));
                }
                st.as_object_mut().unwrap().insert("ok".into(), json!(true));
                out
            } else {
                let fails = st["fails"].as_i64().unwrap_or(0) + 1;
                let o = st.as_object_mut().unwrap();
                o.insert("fails".into(), json!(fails));
                o.insert("ok".into(), json!(false));
                if !get_truthy(st, "down") && (immediate || fails >= DOWN_AFTER) {
                    let o = st.as_object_mut().unwrap();
                    o.insert("down".into(), json!(true));
                    o.insert("since".into(), json!(now));
                }
                if get_truthy(st, "down")
                    && now - as_f64(&st["alerted_at"]).unwrap_or(0.0) >= REMIND_AFTER
                {
                    st.as_object_mut()
                        .unwrap()
                        .insert("alerted_at".into(), json!(now));
                    Some((detail.to_string(), false))
                } else {
                    None
                }
            }
        };
        if let Some((text, recovered)) = alert {
            self.alert(comp, &text, recovered).await;
        }
    }

    /// Notify in-app and through Sentinel (Telegram).
    pub async fn alert(&self, comp: &str, detail: &str, recovered: bool) {
        let en = get_str(&self.settings(), "language") == "en";
        let (zh_name, en_name) = LABELS
            .iter()
            .find(|(k, _)| *k == comp)
            .map(|(_, v)| *v)
            .unwrap_or((comp, comp));
        let title = if recovered {
            if en {
                format!("✅ {} is back", en_name)
            } else {
                format!("✅ {}已恢复", zh_name)
            }
        } else if en {
            format!("🚨 OMuse: {} problem", en_name)
        } else {
            format!("🚨 OMuse：{}出问题了", zh_name)
        };
        let body = detail.to_string()
            + if recovered {
                ""
            } else if en {
                "\nNew tasks may fail until this is fixed."
            } else {
                "\n修好之前，新任务可能会失败。"
            };
        {
            let mut recent = self.recent.lock();
            recent.push(
                json!({"ts": now_ts(), "component": comp, "title": title, "body": body, "recovered": recovered}),
            );
            let extra = recent.len().saturating_sub(50);
            recent.drain(..extra);
        }
        if let Ok(n) = self.rt.store().notify(
            &title,
            &body,
            "",
            if recovered { "info" } else { "warning" },
        ) {
            self.rt
                .publish(json!({"kind": "notification", "notification": n}))
                .await;
        }
        let _ = self
            .rt
            .sentinel(
                "POST",
                "/internal/notify",
                Some(json!({"task_id": "", "text": format!("{}\n{}", title, body)})),
                20.0,
            )
            .await;
    }

    /// FAILED tasks grouped by cause: STREAK of the same cause within STREAK_WINDOW = an alert on "tasks"; the next
    /// task that completes clears it (the periodic checks can look fine while every chat fails).
    pub async fn on_task_finished(&self, t: &Value) -> Result<()> {
        if !t.is_object() || get_str(t, "source") == "golden" {
            return Ok(());
        }
        let status = get_str(t, "status");
        if status == "COMPLETED" {
            let down = self
                .state
                .lock()
                .get("tasks")
                .is_some_and(|s| get_truthy(s, "down"));
            if down {
                self.observe("tasks", true, "任务又能正常完成了", false)
                    .await;
            }
            return Ok(());
        }
        if status != "FAILED" {
            return Ok(());
        }
        let cause = failure_cause(&get_str(t, "error"));
        let since = now_ts() - STREAK_WINDOW;
        let rows: Vec<Value> = self
            .rt
            .store()
            .tasks(Some("FAILED"), 30, None)?
            .into_iter()
            .filter(|r| {
                as_f64(&r["finished_at"])
                    .filter(|v| *v != 0.0)
                    .or_else(|| as_f64(&r["updated_at"]).filter(|v| *v != 0.0))
                    .unwrap_or(0.0)
                    >= since
                    && get_str(r, "source") != "golden"
            })
            .collect();
        let same = rows
            .iter()
            .filter(|r| failure_cause(&get_str(r, "error")) == cause)
            .count();
        if same >= STREAK {
            let err = take_chars(&get_str(t, "error"), 200).to_string();
            self.observe(
                "tasks",
                false,
                &format!(
                    "最近 1 小时有 {} 个任务因{}问题失败：{}",
                    same,
                    cause_label(cause),
                    err
                ),
                true,
            )
            .await;
        }
        Ok(())
    }

    /// `{"components": {name: {ok, down, detail, since, checked_at}}, "alerts": [last 10]}`.
    pub fn status(&self) -> Value {
        let comps: Map<String, Value> = self
            .state
            .lock()
            .iter()
            .map(|(k, v)| {
                let mut m = Map::new();
                for kk in ["ok", "down", "detail", "since", "checked_at"] {
                    m.insert(kk.into(), v.get(kk).cloned().unwrap_or(Value::Null));
                }
                (k.clone(), Value::Object(m))
            })
            .collect();
        let recent = self.recent.lock();
        let start = recent.len().saturating_sub(10);
        json!({"components": comps, "alerts": recent[start..].to_vec()})
    }

    // ------------------------------------------------------------ loop
    /// Start the periodic checks (once).
    pub fn start(self: &Arc<Self>) {
        let mut task = self.task.lock();
        if task.is_none() {
            let me = self.clone();
            *task = Some(tokio::spawn(async move { me.run_loop().await }));
        }
    }

    async fn run_loop(&self) {
        tokio::time::sleep(Duration::from_secs(20)).await;
        loop {
            let res = self.run_checks().await;
            for (comp, (ok, detail)) in res {
                self.observe(&comp, ok, &detail, false).await;
            }
            let due = now_ts() - *self.ledger_at.lock() >= LEDGER_EVERY;
            if due {
                *self.ledger_at.lock() = now_ts();
                if let Err(e) = self.reconcile_ledger().await {
                    println!("[health] ledger reconcile failed: {}", e);
                }
            }
            tokio::time::sleep(Duration::from_secs(CHECK_EVERY)).await;
        }
    }

    /// Merchants' emails move ledger orders along (shipped → delivered, cancelled, refunded); tell the user.
    pub async fn reconcile_ledger(&self) -> Result<Value> {
        let res = self
            .rt
            .sentinel("POST", "/internal/ledger/reconcile", Some(json!({})), 300.0)
            .await?;
        let labels = [
            ("shipped", "已发货 shipped"),
            ("delivered", "已送达 delivered"),
            ("cancelled", "已取消 cancelled"),
            ("refunded", "已退款 refunded"),
            ("placed", "已下单 placed"),
        ];
        for c in res
            .get("changed")
            .and_then(|c| c.as_array())
            .into_iter()
            .flatten()
        {
            let to = get_str(c, "to");
            let label = labels
                .iter()
                .find(|(k, _)| *k == to)
                .map(|(_, v)| (*v).to_string())
                .unwrap_or_else(|| to.clone());
            let text = format!(
                "订单 {}：{}（来自商家邮件）",
                c.get("order_number")
                    .map(py_str)
                    .unwrap_or_else(|| "None".into()),
                label
            );
            if let Ok(n) = self.rt.store().notify(
                &format!("🧾 {}", text),
                "交易账本已更新 Ledger updated",
                "",
                "info",
            ) {
                self.rt
                    .publish(json!({"kind": "notification", "notification": n}))
                    .await;
                let _ = self
                    .rt
                    .sentinel(
                        "POST",
                        "/internal/notify",
                        Some(json!({"task_id": "", "text": format!("🧾 {}", text)})),
                        20.0,
                    )
                    .await;
            }
        }
        Ok(res)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rt_api::testing::FakeRt;

    #[test]
    fn causes_and_model_check() {
        assert_eq!(
            failure_cause("LLMError: 模型接口错误 model API error HTTP 422"),
            "model"
        );
        assert_eq!(failure_cause("浏览器服务不可用"), "browser");
        assert_eq!(failure_cause("invalid_grant for gmail"), "mail");
        assert_eq!(failure_cause("任务用时上限"), "timeout");
        assert_eq!(failure_cause("来源反复失败 kept failing"), "gave_up");
        assert_eq!(failure_cause("something else"), "other");
        let (ok, _) = model_check(
            &[json!({"id": "Q", "mode": "chat", "readiness": "ready"})],
            "Q",
        );
        assert!(ok);
        let (ok, why) = model_check(&[json!({"id": "speaches/Kokoro", "mode": "tts"})], "Q");
        assert!(!ok && why.contains("不在"));
        assert_eq!(
            why,
            "模型 Q 不在模型服务的列表里（可能已卸载或正在加载）；当前可用：speaches/Kokoro"
        );
        let (ok, why) = model_check(
            &[json!({"id": "Q", "mode": "chat", "readiness": "loading"})],
            "Q",
        );
        assert!(!ok && why.contains("loading"));
        let (ok, why) = model_check(&[json!({"id": "Q", "mode": "tts"})], "Q");
        assert!(!ok && why == "模型 Q 不是聊天模型（mode=tts）");
    }

    #[tokio::test]
    async fn alerts_once_and_recovers() {
        // roadmap batch 1: 2026-10-05 every task failed for a day (model fell back to a TTS model) and nobody was told
        let rt = FakeRt::new();
        let h = Health::new(rt.clone());
        let sent = || -> Vec<String> {
            rt.calls
                .lock()
                .iter()
                .filter(|(p, _)| p == "/internal/notify")
                .map(|(_, b)| b["text"].as_str().unwrap().to_string())
                .collect()
        };
        h.observe("model", false, "模型 Q 不在列表里", false).await; // one failed check: not yet down
        assert!(sent().is_empty());
        h.observe("model", false, "模型 Q 不在列表里", false).await; // second: alert
        h.observe("model", false, "模型 Q 不在列表里", false).await; // third: no repeat within REMIND_AFTER
        assert_eq!(sent().len(), 1);
        assert!(sent()[0].contains("模型服务出问题了"));
        assert_eq!(
            sent()[0],
            "🚨 OMuse：模型服务出问题了\n模型 Q 不在列表里\n修好之前，新任务可能会失败。"
        );
        h.observe("model", true, "Q 正常", false).await;
        assert_eq!(sent().len(), 2);
        assert!(sent()[1].contains("已恢复"));
        h.observe("model", true, "Q 正常", false).await;
        assert_eq!(sent().len(), 2);
        let st = h.status();
        assert_eq!(st["components"]["model"]["ok"], true);
        assert_eq!(st["components"]["model"]["down"], false);
        assert_eq!(st["alerts"].as_array().unwrap().len(), 2);
    }

    #[tokio::test]
    async fn task_streak_alerts() {
        let rt = FakeRt::new();
        let h = Health::new(rt.clone());
        for _ in 0..3 {
            let t = rt
                .store
                .create_task("x", "c", "chat", "", "", None)
                .unwrap();
            rt.store
                .update_task(
                    t["id"].as_str().unwrap(),
                    &json!({"status": "FAILED", "error": "LLMError: model API error", "finished_at": now_ts()}),
                )
                .unwrap();
        }
        let last = rt.store.tasks(Some("FAILED"), 1, None).unwrap().remove(0);
        h.on_task_finished(&last).await.unwrap();
        assert!(h.status()["components"]["tasks"]["down"].as_bool().unwrap());
        assert!(rt.notify_count() == 1);
        assert!(h.status()["components"]["tasks"]["detail"]
            .as_str()
            .unwrap()
            .starts_with("最近 1 小时有 3 个任务因模型问题失败："));
    }
}
