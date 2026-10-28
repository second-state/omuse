//! The [`Runtime`]: Sentinel client, events, task lifecycle, planner and the executor loop (`class Runtime` of
//! `app/runtime/agent.py`, minus the tool execution in `exec.rs` / `local.rs` / `images.rs`).

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicI64;
use std::sync::{Arc, OnceLock, Weak};
use std::time::Duration;

use anyhow::{anyhow, Result};
use futures::future::BoxFuture;
use omuse_common::http::H_RUNTIME;
use omuse_common::util::{get_str, get_truthy, now_ts, py_str, take_chars, truncate, truthy};
use omuse_common::{pyjson, Config};
use parking_lot::Mutex;
use serde_json::{json, Map, Value};
use tokio::task::JoinHandle;

use super::exec::CallMode;
use super::guards::*;
use super::lang::{agent_lang, request_lang, with_langs};
use super::tools::{concurrent_runs, tool_name, FINISH_TOOLS, LOCAL_TOOLS};
use super::util::{arr, content_of, fmt_float, sget, stored_call, tool_calls};
use super::{is_terminal, TERMINAL};
use crate::health::Health;
use crate::llm::{
    extract_json, strip_tool_markup, ChatOpts, ChatReply, Hook, Llm, LlmError, LlmTransport,
    ToolCall,
};
use crate::prompts;
use crate::rt_api::{describe_reqwest_error, RuntimeApi};
use crate::skills::Skills;
use crate::store::{FactOpts, Store};

/// `async fn(event: dict)`: fan an event out to the UI stream.
pub type Publish = Arc<dyn Fn(Value) -> BoxFuture<'static, ()> + Send + Sync>;

/// Replaces the HTTP call to Sentinel (tests): `(method, path, payload) -> result`.
pub type SentinelHook =
    Arc<dyn Fn(String, String, Option<Value>) -> BoxFuture<'static, Result<Value>> + Send + Sync>;

/// A call of the current turn that is not run because it repeats an earlier one (`first` = that call's id); see
/// [`Runtime::skip_duplicate`].
struct Skip {
    first: String,
    search: bool,
    typed: bool,
}

/// A tool call that must wait for the user (approval / takeover): the loop stops and the task is parked.
#[derive(Debug, Clone)]
pub struct Suspend {
    pub status: String,
    /// The `waiting` record; `_pending` (the calls to resume with) is popped off by [`Runtime::suspend`].
    pub waiting: Value,
}

/// Per-task bookkeeping that Python kept as attributes of the runtime object.
#[derive(Default)]
pub(crate) struct Inner {
    /// task_id -> latest generated/edited image (edit_image / vary_image default)
    pub last_image: HashMap<String, String>,
    /// task_id -> pictures generated so far (cost guard, image budget)
    pub img_count: HashMap<String, i64>,
    /// task_id -> latest generated clip
    pub last_video: HashMap<String, String>,
    /// task_id -> clips generated so far (cost guard, video budget)
    pub vid_count: HashMap<String, i64>,
    /// (task, tool, output) -> files made, see REMAKE_MAX
    pub remakes: HashMap<(String, String, String), i64>,
    /// task -> id-like tokens seen in its tool results (survives compression)
    pub seen_ids: HashMap<String, HashSet<String>>,
    /// learned from a "context exceeded" error: keep requests below this many chars
    pub ctx_chars: usize,
    /// tasks that read the user's files (Sentinel was told)
    pub confidential: HashSet<String>,
    /// (task, file, size, mtime) already posted to the chat
    pub sent_files: HashSet<(String, String, u64, i64)>,
    /// task_id -> pages/emails actually read (for present_choices)
    pub evidence: HashMap<String, Vec<Value>>,
    /// task_id -> "legacy" (golden A/B of personal context)
    pub ctx_mode: HashMap<String, String>,
    /// task_id -> sites whose remembered habits were shown
    pub site_noted: HashMap<String, HashSet<String>>,
    /// task_id -> full tool results, newest last (outcome evidence)
    pub tool_texts: HashMap<String, Vec<String>>,
    /// task_id -> seconds spent queued for a model slot
    pub llm_wait: HashMap<String, f64>,
    /// task_id -> sub-agent lanes handed out so far (`sub1`, `sub2`, …); seeded from the task's events, reserved
    /// atomically so parallel `delegate` calls never share a lane or exceed `SUBAGENT_MAX`
    pub sub_lanes: HashMap<String, usize>,
    /// task_id -> the loaded skill that turned the browser off (front matter `browser: off`, e.g. the city guides):
    /// browser tools are hidden from that task and its sub-agents, and refused if called anyway, until a skill
    /// without that policy is loaded
    pub no_browser: HashMap<String, String>,
    /// sites whose bot wall was learned as a habit
    pub block_noted: HashSet<String>,
    /// task_id -> product pages that already got the cart hint
    pub product_hinted: HashMap<String, HashSet<String>>,
}

/// The agent runtime. Always used behind `Arc` ([`Runtime::new`] returns one).
pub struct Runtime {
    me: Weak<Runtime>,
    pub cfg: Arc<Config>,
    pub store: Arc<Store>,
    pub llm: Arc<Llm>,
    publish_fn: Publish,
    /// `WORKSPACE` (realpath).
    pub workspace: PathBuf,
    pub skills_dirs: Skills,
    /// The image-generation client (`app/runtime/imagegen.py`).
    pub images: crate::imagegen::ImageClient,
    /// `IMAGE_BUDGET_PER_TASK` (`[model] image_budget`; tests lower it).
    pub image_budget: AtomicI64,
    /// The video-generation client (`videogen`).
    pub videos: crate::videogen::VideoClient,
    /// `[model] video_budget`: clips per task (tests lower it).
    pub video_budget: AtomicI64,
    http: reqwest::Client,
    sentinel_hook: Mutex<Option<SentinelHook>>,
    health: OnceLock<Arc<Health>>,
    running: Mutex<HashMap<String, JoinHandle<()>>>,
    pub cancel_flags: Mutex<HashSet<String>>,
    pub pause_flags: Mutex<HashSet<String>>,
    catalog_cache: Mutex<(f64, Option<Value>)>,
    /// Woken by [`Runtime::cancel`]: a model call that is waiting (queued for a slot, or sleeping on a Retry-After)
    /// is dropped at once instead of finishing its retries first.
    cancel_notify: tokio::sync::Notify,
    pub(crate) inner: Mutex<Inner>,
    /// Serialises read-modify-write of a task's plan: parallel sub-agents mark their steps at the same time.
    pub(crate) plan_mu: Mutex<()>,
}

pub(crate) fn opts(purpose: &str, task_id: &str) -> ChatOpts {
    ChatOpts {
        purpose: purpose.into(),
        task_id: task_id.into(),
        ..Default::default()
    }
}

/// `{"type": "function", ...}` tool call as stored in the transcript.
pub(crate) fn assistant_with_calls(content: &str, calls: &[ToolCall]) -> Value {
    json!({"role": "assistant", "content": content,
           "tool_calls": calls.iter().map(|c| json!({"id": c.id, "type": "function",
               "function": {"name": c.name, "arguments": pyjson::dumps(&c.args)}})).collect::<Vec<_>>()})
}

fn content_chars(msgs: &[Value]) -> usize {
    msgs.iter().map(|m| content_of(m).chars().count()).sum()
}

fn user_said(transcript: &[Value]) -> String {
    transcript
        .iter()
        .filter(|m| get_str(m, "role") == "user")
        .map(content_of)
        .collect::<Vec<_>>()
        .join("\n")
}

fn user_msg(text: impl Into<String>) -> Value {
    json!({"role": "user", "content": text.into()})
}

impl Runtime {
    /// `Runtime(data_dir, publish)`: open the runtime store under `data_dir`, make the workspace, wire the model
    /// client's hooks and the health watch.
    pub fn new(data_dir: &Path, cfg: Arc<Config>, publish: Publish) -> Result<Arc<Runtime>> {
        Self::with_llm_transport(data_dir, cfg, publish, None)
    }

    /// Same, with a custom HTTP layer for the model client (tests, scripted models).
    pub fn with_llm_transport(
        data_dir: &Path,
        cfg: Arc<Config>,
        publish: Publish,
        transport: Option<Arc<dyn LlmTransport>>,
    ) -> Result<Arc<Runtime>> {
        prompts::set_app_id(&cfg.server.app_id);
        let store = Arc::new(Store::new(data_dir, cfg.clone())?);
        let ws = cfg.workspace();
        std::fs::create_dir_all(&ws)?;
        let workspace = super::util::realpath(&ws);
        let skills_dirs = Skills::new(&cfg);
        let http = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(10))
            .no_proxy()
            .build()
            .map_err(|e| anyhow!("{}", e))?;
        let rt = Arc::new_cyclic(|weak: &Weak<Runtime>| {
            let st = store.clone();
            let w1 = weak.clone();
            let on_call: Hook = Arc::new(move |info: Value| {
                let w = w1.clone();
                Box::pin(async move {
                    if let Some(rt) = w.upgrade() {
                        rt.on_llm_call(info).await;
                    }
                })
            });
            let w2 = weak.clone();
            let on_retry: Hook = Arc::new(move |info: Value| {
                let w = w2.clone();
                Box::pin(async move {
                    if let Some(rt) = w.upgrade() {
                        rt.on_llm_retry(info).await;
                    }
                })
            });
            let get_settings: crate::llm::SettingsFn =
                Arc::new(move || st.settings().unwrap_or_else(|_| json!({})));
            let llm = Arc::new(match transport {
                Some(tr) => Llm::with_transport(get_settings, Some(on_call), Some(on_retry), tr),
                None => Llm::new(get_settings, Some(on_call), Some(on_retry)),
            });
            Runtime {
                me: weak.clone(),
                image_budget: AtomicI64::new(crate::imagegen::image_budget(&cfg) as i64),
                video_budget: AtomicI64::new(crate::videogen::video_budget(&cfg) as i64),
                videos: crate::videogen::VideoClient::new(cfg.clone()),
                cfg: cfg.clone(),
                store: store.clone(),
                llm,
                publish_fn: publish,
                workspace,
                skills_dirs,
                images: crate::imagegen::ImageClient::new(cfg.clone()),
                http,
                sentinel_hook: Mutex::new(None),
                health: OnceLock::new(),
                running: Mutex::new(HashMap::new()),
                cancel_flags: Mutex::new(HashSet::new()),
                pause_flags: Mutex::new(HashSet::new()),
                catalog_cache: Mutex::new((0.0, None)),
                cancel_notify: tokio::sync::Notify::new(),
                inner: Mutex::new(Inner::default()),
                plan_mu: Mutex::new(()),
            }
        });
        let api: Arc<dyn RuntimeApi> = rt.clone();
        let _ = rt.health.set(Arc::new(Health::new(api)));
        Ok(rt)
    }

    /// `self` as an `Arc` (for spawning background work).
    pub fn arc(&self) -> Arc<Runtime> {
        self.me.upgrade().expect("Runtime is alive")
    }

    /// The health watch (`rt.health`).
    pub fn health(&self) -> &Arc<Health> {
        self.health.get().expect("health is set in Runtime::new")
    }

    /// Tests: answer Sentinel calls in-process.
    pub fn set_sentinel_hook(&self, hook: Option<SentinelHook>) {
        *self.sentinel_hook.lock() = hook;
    }

    /// The settings in force (defaults when the store cannot be read).
    pub fn settings(&self) -> Value {
        self.store
            .settings()
            .unwrap_or_else(|_| self.store.default_settings())
    }

    /// `llm.chat(...)` for a task, abandoned as soon as the task is cancelled (`None`). Python let a cancelled task
    /// finish its model call (and every Retry-After sleep in it) before noticing the flag; here the call future is
    /// dropped, which also hands its concurrency slot back.
    pub async fn chat_or_cancel(
        &self,
        task_id: &str,
        msgs: &[Value],
        tools: Option<&[Value]>,
        o: &ChatOpts,
    ) -> Option<Result<ChatReply, LlmError>> {
        let fut = self.llm.chat(msgs, tools, o);
        tokio::pin!(fut);
        loop {
            let notified = self.cancel_notify.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if self.cancel_flags.lock().contains(task_id) {
                return None;
            }
            tokio::select! {
                r = &mut fut => return Some(r),
                _ = &mut notified => {}
            }
        }
    }

    // ================================================================ sentinel client
    /// One JSON call to Sentinel with the runtime token; HTTP errors are errors (`raise_for_status`).
    /// Errors display as `"<httpx exception name>: <detail>"`.
    pub async fn sentinel(
        &self,
        method: &str,
        path: &str,
        payload: Option<Value>,
        timeout: f64,
    ) -> Result<Value> {
        let hook = self.sentinel_hook.lock().clone();
        if let Some(h) = hook {
            return h(method.to_string(), path.to_string(), payload).await;
        }
        let url = format!("{}{}", self.cfg.sentinel_url(), path);
        let m = reqwest::Method::from_bytes(method.as_bytes()).unwrap_or(reqwest::Method::GET);
        let mut req = self
            .http
            .request(m, &url)
            .timeout(Duration::from_secs_f64(timeout.max(0.1)))
            .header(H_RUNTIME, self.cfg.runtime_token());
        if let Some(p) = payload {
            req = req.json(&p);
        }
        let r = req
            .send()
            .await
            .map_err(|e| anyhow!("{}", describe_reqwest_error(&e)))?;
        let status = r.status();
        if status.is_client_error() || status.is_server_error() {
            return Err(anyhow!(
                "HTTPStatusError: {} error '{}' for url '{}'",
                if status.is_client_error() {
                    "Client"
                } else {
                    "Server"
                },
                status,
                url
            ));
        }
        let text = r
            .text()
            .await
            .map_err(|e| anyhow!("{}", describe_reqwest_error(&e)))?;
        serde_json::from_str(&text).map_err(|e| anyhow!("JSONDecodeError: {}", e))
    }

    /// Absolute link to a task's browser tab when the public address is known (Sentinel learns it from the first
    /// page load), else the in-app #browser/<task> link.
    pub async fn browser_link(&self, task_id: &str) -> String {
        let base = match self
            .sentinel("GET", "/internal/public_base", None, 5.0)
            .await
        {
            Ok(v) => sget(&v, "base"),
            Err(_) => String::new(),
        };
        if base.is_empty() {
            format!("#browser/{}", task_id)
        } else {
            format!("{}/#browser/{}", base, task_id)
        }
    }

    /// Post an audit record to Sentinel (`kw` = resource / risk / decision / result / detail …); never fails.
    pub async fn audit(&self, actor: &str, action: &str, task_id: &str, kw: Value) {
        let mut body = json!({"actor": actor, "action": action, "task_id": task_id});
        if let (Some(b), Some(k)) = (body.as_object_mut(), kw.as_object()) {
            for (key, v) in k {
                b.insert(key.clone(), v.clone());
            }
        }
        let _ = self
            .sentinel("POST", "/internal/audit", Some(body), 10.0)
            .await;
    }

    /// Fan an event out to the UI stream.
    pub async fn publish(&self, ev: Value) {
        (self.publish_fn)(ev).await;
    }

    /// A library report was written: rebuild that subject's search index (cheap: one file).
    pub(crate) fn reindex_report(&self, rel_path: &str) {
        static RE: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
            regex::Regex::new(r"^/?library/([^/]+)/report\.md$").unwrap()
        });
        let p = rel_path.replace('\\', "/");
        let Some(c) = RE.captures(&p) else {
            return;
        };
        let slug = c[1].to_string();
        let sub = self
            .store
            .subjects()
            .unwrap_or_default()
            .into_iter()
            .find(|x| get_str(x, "slug") == slug);
        if let Some(sub) = sub {
            if let Err(e) = crate::library::index_subject(&self.store, &self.workspace, &sub) {
                let msg = e.to_string();
                println!(
                    "[library] reindex {}: Exception: {}",
                    get_str(&sub, "slug"),
                    take_chars(&msg, 120)
                );
            }
        }
    }

    /// Give the chat a title that summarises it (the list showed the truncated first question). Re-done after
    /// every finished task, so the title follows the conversation; cheap: a tiny no-think call.
    pub async fn retitle(&self, cid: &str, task_id: &str) {
        if let Err(e) = self.retitle_inner(cid, task_id).await {
            // a title is a nicety: never let it touch the task
            let msg = e.to_string();
            println!("[title] {}: {}", cid, take_chars(&msg, 140));
        }
    }

    async fn retitle_inner(&self, cid: &str, task_id: &str) -> Result<()> {
        let Some(conv) = self.store.conv(cid)? else {
            return Ok(());
        };
        let kind = conv.get("kind").and_then(Value::as_str).unwrap_or("chat");
        if kind != "chat" {
            return Ok(());
        }
        let all: Vec<Value> = self
            .store
            .msgs(cid, 40)?
            .into_iter()
            .filter(|m| {
                matches!(get_str(m, "role").as_str(), "user" | "assistant")
                    && !content_of(m).trim().is_empty()
            })
            .collect();
        let msgs = &all[all.len().saturating_sub(8)..];
        if msgs.is_empty() {
            return Ok(());
        }
        let s = self.settings();
        let mut language = sget(&s, "language");
        if language.is_empty() {
            language = "zh".into();
        }
        if get_str(&s, "reply_language") == "match" {
            let last_user = msgs
                .iter()
                .rev()
                .find(|m| get_str(m, "role") == "user")
                .map(content_of)
                .unwrap_or_default();
            language = if prompts::request_language(&last_user, &language) == "English" {
                "en".into()
            } else {
                "zh".into()
            };
        }
        let text = msgs
            .iter()
            .map(|m| {
                format!(
                    "{}: {}",
                    if get_str(m, "role") == "user" {
                        "User"
                    } else {
                        "Assistant"
                    },
                    truncate(&py_str(m.get("content").unwrap_or(&Value::Null)), 400)
                )
            })
            .collect::<Vec<_>>()
            .join("\n\n");
        let o = ChatOpts {
            max_tokens: Some(60),
            temperature: Some(0.0),
            no_think: true,
            ..opts("title", task_id)
        };
        let r = self
            .llm
            .chat(
                &[
                    json!({"role": "system", "content": prompts::title_prompt(&language)}),
                    user_msg(text),
                ],
                None,
                &o,
            )
            .await
            .map_err(|e| anyhow!("{}: {}", e.name(), e))?;
        let title = prompts::clean_title(&r.content, &language);
        if !title.is_empty() && title != get_str(&conv, "title") {
            self.store.set_conv_title(cid, &title)?;
            self.publish(json!({"kind": "conv_update", "conv_id": cid}))
                .await;
        }
        Ok(())
    }

    /// A model call failed and will be retried: tell the task's live view ("Retrying 1 of 3").
    async fn on_llm_retry(&self, info: Value) {
        let tid = sget(&info, "task_id");
        if !tid.is_empty() {
            let mut d = Map::new();
            for k in ["attempt", "of", "wait_s", "reason", "source", "purpose"] {
                d.insert(k.into(), info.get(k).cloned().unwrap_or(Value::Null));
            }
            self.event(&tid, "llm_retry", Value::Object(d)).await;
        }
    }

    async fn on_llm_call(&self, info: Value) {
        let tid = sget(&info, "task_id");
        let wait = info
            .get("wait_s")
            .and_then(omuse_common::util::as_f64)
            .unwrap_or(0.0);
        if !tid.is_empty() && wait != 0.0 {
            // time spent queued for a model slot (other tasks were using it) does not count against this task's budget
            *self.inner.lock().llm_wait.entry(tid.clone()).or_insert(0.0) += wait;
        }
        self.audit(
            "llm",
            &format!("model.{}", sget(&info, "purpose")),
            &tid,
            json!({"resource": sget(&info, "model"), "result": "success", "detail": info}),
        )
        .await;
    }

    /// Sentinel's tool catalog (`{"tools": [...], "connections": {...}}`), cached 20 s; never fails.
    pub async fn catalog(&self, force: bool) -> Value {
        {
            let c = self.catalog_cache.lock();
            if let Some(cat) = &c.1 {
                if !force && now_ts() - c.0 < 20.0 {
                    return cat.clone();
                }
            }
        }
        let cat = match self.sentinel("GET", "/internal/catalog", None, 15.0).await {
            Ok(v) => v,
            Err(_) => json!({"tools": [], "connections": {}}),
        };
        *self.catalog_cache.lock() = (now_ts(), Some(cat.clone()));
        cat
    }

    /// Golden (test) runs stop where a real run would change something or need the user; the stop is the result.
    pub(crate) async fn dry_run_stop(
        &self,
        task_id: &str,
        name: &str,
        args: &Value,
        why: &str,
    ) -> String {
        self.event(
            task_id,
            "dry_run_stop",
            json!({"tool": name, "args": preview_args(args), "why": truncate(why, 200)}),
        )
        .await;
        format!(
            "（演练模式）到这一步会真正执行操作或需要用户批准，演练到此为止，没有执行：{}。不要重试这一步，也不要换别的方法绕过；直接写最终回答，说明准备怎么做、做到了哪一步。\
 (Dry run: this step would act for real or need the user's approval, so it was not run. Do not retry it or \
work around it; write the final answer saying what you would do and how far you got.)",
            if why.is_empty() { name } else { why }
        )
    }

    pub(crate) fn note_tool_text(&self, task_id: &str, content: &str) {
        let mut inner = self.inner.lock();
        let log = inner.tool_texts.entry(task_id.to_string()).or_default();
        log.push(take_chars(content, 20000).to_string());
        let n = log.len();
        if n > 40 {
            log.drain(..n - 40);
        }
    }

    // ================================================================ events
    /// Store a task event and publish it (`{"kind": "task_event", **event}`).
    pub async fn event(&self, task_id: &str, type_: &str, data: Value) {
        match self.store.add_event(task_id, type_, &data) {
            Ok(ev) => {
                let mut m = Map::new();
                m.insert("kind".into(), json!("task_event"));
                if let Some(o) = ev.as_object() {
                    for (k, v) in o {
                        m.insert(k.clone(), v.clone());
                    }
                }
                self.publish(Value::Object(m)).await;
            }
            Err(e) => println!("[event] {}: {}", task_id, e),
        }
    }

    /// `set_status(task_id, status, **kw)`: update, publish the brief, audit; terminal states notify the health
    /// watch and let the browser recycle the task's page.
    pub async fn set_status(&self, task_id: &str, status: &str, kw: Value) {
        let mut upd = Map::new();
        upd.insert("status".into(), json!(status));
        if let Some(o) = kw.as_object() {
            for (k, v) in o {
                upd.insert(k.clone(), v.clone());
            }
        }
        if let Err(e) = self.store.update_task(task_id, &Value::Object(upd)) {
            println!("[status] {}: {}", task_id, e);
        }
        let t = self
            .store
            .task(task_id)
            .ok()
            .flatten()
            .unwrap_or_else(|| json!({}));
        self.publish(json!({"kind": "task_update", "task": Self::task_brief(&t)}))
            .await;
        let mut detail = Map::new();
        detail.insert("status".into(), json!(status));
        if let Some(e) = kw.get("error") {
            detail.insert("error".into(), e.clone());
        }
        self.audit(
            "runtime",
            "task.status",
            task_id,
            json!({"result": status, "detail": detail}),
        )
        .await;
        if is_terminal(status) {
            if let Err(e) = self.health().on_task_finished(&t).await {
                println!("[health] {:?}", e);
            }
            // let the browser recycle this task's page later (best effort, never blocks)
            let me = self.arc();
            let tid = task_id.to_string();
            tokio::spawn(async move {
                let _ = me
                    .sentinel(
                        "POST",
                        "/internal/browser_release",
                        Some(json!({"task_id": tid})),
                        10.0,
                    )
                    .await;
            });
        }
    }

    /// Mark one plan step `running` / `done` / `failed` (a sub-agent working on it), publish the plan, and say
    /// whether the step exists. Runs under [`Runtime::plan_mu`], so parallel sub-agents never lose each other's update.
    pub async fn set_step_status(&self, task_id: &str, step_id: &str, status: &str) -> bool {
        if step_id.trim().is_empty() {
            return false;
        }
        let plan = {
            let _g = self.plan_mu.lock();
            let Some(task) = self.store.task(task_id).ok().flatten() else {
                return false;
            };
            let mut plan = task.get("plan").cloned().unwrap_or_else(|| json!({}));
            let Some(st) = plan
                .get_mut("steps")
                .and_then(Value::as_array_mut)
                .and_then(|sts| sts.iter_mut().find(|s| sget(s, "id") == step_id.trim()))
            else {
                return false;
            };
            if status != "running" || sget(st, "status") != "done" {
                st["status"] = json!(status);
            }
            if self
                .store
                .update_task(task_id, &json!({"plan": plan}))
                .is_err()
            {
                return false;
            }
            plan
        };
        self.event(task_id, "plan", plan).await;
        true
    }

    /// The task fields the UI shows.
    pub fn task_brief(t: &Value) -> Value {
        let mut m = Map::new();
        for k in [
            "id",
            "conv_id",
            "goal",
            "status",
            "plan",
            "result",
            "error",
            "source",
            "schedule_id",
            "parent_id",
            "steps",
            "waiting",
            "created_at",
            "updated_at",
            "finished_at",
            "outcome",
        ] {
            m.insert(k.into(), t.get(k).cloned().unwrap_or(Value::Null));
        }
        Value::Object(m)
    }

    // ================================================================ public entry points
    /// Create a task, publish it, audit and start it; returns the task record.
    pub async fn submit(
        &self,
        conv_id: &str,
        goal: &str,
        source: &str,
        schedule_id: &str,
        attachments: Option<&Value>,
    ) -> Result<Value> {
        let t = self
            .store
            .create_task(goal, conv_id, source, schedule_id, "", attachments)?;
        self.publish(json!({"kind": "task_update", "task": Self::task_brief(&t)}))
            .await;
        let tid = get_str(&t, "id");
        self.audit(
            "runtime",
            "task.create",
            &tid,
            json!({"detail": {"goal": truncate(goal, 500), "source": source}}),
        )
        .await;
        self.start(&tid);
        Ok(t)
    }

    /// Run an already created task in the background (no-op while it is running).
    pub fn start(&self, task_id: &str) {
        let mut running = self.running.lock();
        if let Some(h) = running.get(task_id) {
            if !h.is_finished() {
                return;
            }
        }
        let me = self.arc();
        let tid = task_id.to_string();
        running.insert(
            task_id.to_string(),
            tokio::spawn(async move { me.run_guarded(tid).await }),
        );
    }

    /// A live (not finished) worker for this task.
    pub fn is_running(&self, task_id: &str) -> bool {
        self.running
            .lock()
            .get(task_id)
            .is_some_and(|h| !h.is_finished())
    }

    async fn run_guarded(self: Arc<Self>, task_id: String) {
        let t0 = self
            .store
            .task(&task_id)
            .ok()
            .flatten()
            .unwrap_or_else(|| json!({}));
        let s0 = self.settings();
        let goal = sget(&t0, "goal");
        let (mut tl, mut fl) = (String::new(), String::new());
        if get_str(&s0, "reply_language") == "match" {
            tl = request_lang(&goal).to_string(); // this task only (and what it spawns)
        } else if get_str(&s0, "language") == "en"
            && request_lang(&goal) == "zh"
            && prompts::wants_cjk_output(&goal)
        {
            // 2026-10-02 R8/R10: a note in the English prompt wasn't enough — "写一篇 5000 字的科幻小说" still came out in English
            fl = "zh".into();
        }
        let res = with_langs(tl, fl, self.run(&task_id)).await;
        if let Err(e) = res {
            let name = e
                .downcast_ref::<LlmError>()
                .map(|l| l.name())
                .unwrap_or("RuntimeError");
            let msg = e.to_string();
            println!("[runtime] task {} failed: {}: {:#}", task_id, name, e);
            self.set_status(
                &task_id,
                "FAILED",
                json!({"error": format!("{}: {}", name, msg), "finished_at": now_ts()}),
            )
            .await;
            self.event(&task_id, "error", json!({"message": take_chars(&msg, 500)}))
                .await;
            if let Some(t) = self.store.task(&task_id).ok().flatten() {
                let en = get_str(&self.settings(), "language") == "en";
                let txt = if en {
                    format!("⚠️ Task failed: {}", msg)
                } else {
                    format!("⚠️ 任务失败：{}", msg)
                };
                let cid = get_str(&t, "conv_id");
                let _ = self.store.add_msg(&cid, "assistant", &txt, &task_id, None);
                self.publish(json!({"kind": "conv_update", "conv_id": cid}))
                    .await;
                self.warn_unattended(
                    &t,
                    ("运行出错", "failed with an error"),
                    &format!("{}: {}", name, msg),
                )
                .await;
            }
        }
        let t = self.store.task(&task_id).ok().flatten();
        if t.as_ref()
            .is_none_or(|t| TERMINAL.contains(&get_str(t, "status").as_str()))
        {
            let mut inner = self.inner.lock();
            inner.evidence.remove(&task_id);
            inner.tool_texts.remove(&task_id);
            inner.no_browser.remove(&task_id);
        }
    }

    /// A scheduled / triggered run went wrong while nobody was watching: tell the user (app + Telegram).
    /// `what` = (Chinese, English) — the notice follows the app language.
    pub(crate) async fn warn_unattended(&self, t: &Value, what: (&str, &str), detail: &str) {
        if get_str(t, "source") != "schedule" {
            return;
        }
        let sid = get_str(t, "schedule_id");
        let sch = if sid.is_empty() {
            None
        } else {
            self.store.schedule(&sid).ok().flatten()
        };
        let mut name = sch.as_ref().map(|s| sget(s, "name")).unwrap_or_default();
        if name.is_empty() {
            name = truncate(&get_str(t, "goal"), 40);
        }
        let (title, body) = if get_str(&self.settings(), "language") == "en" {
            (
                format!("⚠️ \"{}\" {}", name, what.1),
                format!(
                    "{}\nOpen the task to see what happened, or run it again with ▶ Run now in Automations.",
                    truncate(detail, 600)
                ),
            )
        } else {
            (
                format!("⚠️ 「{}」这次{}", name, what.0),
                format!(
                    "{}\n可以在「自动化」里点 ▶ 立即运行 重试，或在任务详情里查看过程。",
                    truncate(detail, 600)
                ),
            )
        };
        let tid = get_str(t, "id");
        if let Ok(n) = self.store.notify(&title, &body, &tid, "warning") {
            self.publish(json!({"kind": "notification", "notification": n}))
                .await;
            let _ = self
                .sentinel(
                    "POST",
                    "/internal/notify",
                    Some(json!({"task_id": tid, "text": format!("{}\n{}", title, body)})),
                    20.0,
                )
                .await;
        }
    }

    /// Cancel a task (and expire its pending approval / takeover).
    pub async fn cancel(&self, task_id: &str, reason: &str) {
        self.cancel_flags.lock().insert(task_id.to_string());
        self.cancel_notify.notify_waiters();
        let Some(t) = self.store.task(task_id).ok().flatten() else {
            return;
        };
        let status = get_str(&t, "status");
        if is_terminal(&status) {
            return;
        }
        let kw = if reason.is_empty() {
            json!({"finished_at": now_ts()})
        } else {
            json!({"finished_at": now_ts(), "error": reason})
        };
        self.set_status(task_id, "CANCELLED", kw).await;
        self.event(
            task_id,
            "cancelled",
            if reason.is_empty() {
                json!({})
            } else {
                json!({"reason": reason})
            },
        )
        .await;
        if super::WAITING.contains(&status.as_str()) {
            // don't leave an orphan approval behind: approving it later would run an action for a dead task
            // (Sentinel also ends a takeover of this task's page)
            let why = if reason.is_empty() {
                "任务已取消 (task cancelled)"
            } else {
                reason
            };
            let _ = self
                .sentinel(
                    "POST",
                    "/internal/expire_approvals",
                    Some(json!({"task_id": task_id, "reason": why})),
                    15.0,
                )
                .await;
        }
    }

    /// Ask a running task to pause at its next step.
    pub async fn pause(&self, task_id: &str) {
        self.pause_flags.lock().insert(task_id.to_string());
        if let Some(t) = self.store.task(task_id).ok().flatten() {
            if matches!(
                get_str(&t, "status").as_str(),
                "RUNNING" | "PLANNING" | "CREATED"
            ) {
                self.event(task_id, "pause_requested", json!({})).await;
            }
        }
    }

    /// Resume a paused task.
    pub async fn resume(&self, task_id: &str) {
        self.pause_flags.lock().remove(task_id);
        if let Some(t) = self.store.task(task_id).ok().flatten() {
            if get_str(&t, "status") == "PAUSED" {
                self.set_status(task_id, "RUNNING", json!({"waiting": null}))
                    .await;
                self.start(task_id);
            }
        }
    }

    /// `/internal/approval_resolved` from Sentinel.
    pub async fn on_approval_resolved(&self, payload: &Value) {
        let task_id = sget(payload, "task_id");
        let Some(t) = self.store.task(&task_id).ok().flatten() else {
            return;
        };
        if get_str(&t, "status") != "WAITING_APPROVAL" {
            return;
        }
        let mut pend = t
            .get("pending")
            .filter(|p| truthy(p))
            .cloned()
            .unwrap_or_else(|| json!({}));
        let aid = sget(&pend, "approval_id");
        if !aid.is_empty()
            && Some(aid.as_str()) != payload.get("approval_id").and_then(Value::as_str)
        {
            return;
        }
        pend["resolved"] = json!({"call_id": payload.get("call_id"), "decision": payload.get("decision"),
                                  "result": payload.get("result")});
        let _ = self.store.update_task(&task_id, &json!({"pending": pend}));
        self.event(
            &task_id,
            "approval_resolved",
            json!({"approval_id": payload.get("approval_id"), "decision": payload.get("decision")}),
        )
        .await;
        self.set_status(&task_id, "RUNNING", json!({"waiting": null}))
            .await;
        self.start(&task_id);
    }

    /// The user handed the browser back. Resume (a) tasks that were only paused because the user had the browser and
    /// (b) the task(s) the takeover was actually for. Other tasks that asked for their own takeover keep waiting:
    /// resuming them too used to make every one of them ask again at once — a burst of "take over" pop-ups.
    pub async fn on_takeover_ended(&self, payload: &Value) {
        let for_tasks: HashSet<String> =
            [sget(payload, "task_id"), sget(payload, "requested_task")]
                .into_iter()
                .filter(|x| !x.is_empty() && x != "default")
                .collect();
        for t in self
            .store
            .tasks(Some("WAITING_EXTERNAL"), 50, None)
            .unwrap_or_default()
        {
            let w = t.get("waiting").cloned().unwrap_or(Value::Null);
            let wt = sget(&w, "type");
            let tid = get_str(&t, "id");
            if wt == "takeover" || (wt == "takeover_requested" && for_tasks.contains(&tid)) {
                let Some(full) = self.store.task(&tid).ok().flatten() else {
                    continue;
                };
                let mut pend = full
                    .get("pending")
                    .filter(|p| truthy(p))
                    .cloned()
                    .unwrap_or_else(|| json!({}));
                let lg = agent_lang(&self.settings());
                let text = prompts::l(
                    lg,
                    "用户已完成接管并交还浏览器控制权。请先调用 browser_snapshot 查看当前页面状态再继续。",
                    "",
                );
                let text = if lg == "en" {
                    format!(
                        "{}: the user handed browser control back. Take a browser_snapshot first to see the current page, then continue.",
                        TAKEOVER_DONE_EN
                    )
                } else {
                    text.to_string()
                };
                pend["resolved"] = json!({"call_id": pend.get("call_id"), "decision": "takeover_ended",
                                          "result": {"status": "ok", "result": text}});
                let _ = self.store.update_task(&tid, &json!({"pending": pend}));
                self.event(&tid, "takeover_ended", json!({})).await;
                self.set_status(&tid, "RUNNING", json!({"waiting": null}))
                    .await;
                self.start(&tid);
            }
        }
    }

    /// On startup: tasks that were mid-flight are resumed; waiting ones stay waiting.
    pub async fn recover(&self) {
        for t in self
            .store
            .tasks(Some("RUNNING,PLANNING,CREATED"), 50, None)
            .unwrap_or_default()
        {
            let tid = get_str(&t, "id");
            self.event(&tid, "recovered", json!({"note": "runtime restarted"}))
                .await;
            self.start(&tid);
        }
    }

    // ================================================================ the loop
    /// What an earlier task in this conversation actually did, for a follow-up message (2026-10-05: after a cancelled
    /// decathlon.sg order test, "刚才这个操作…能否把这个订单取消" was taken to mean an Amazon order, because the cancelled
    /// task had left no reply in the chat).
    pub(crate) fn task_digest(&self, task: &Value) -> String {
        let evs = self
            .store
            .events(&get_str(task, "id"), 0)
            .unwrap_or_default();
        let (mut sites, mut approvals): (Vec<String>, Vec<String>) = (vec![], vec![]);
        let mut fills = 0;
        let mut last_wait = String::new();
        let mut pending: HashMap<String, String> = HashMap::new();
        for e in &evs {
            let d = e.get("data").cloned().unwrap_or_else(|| json!({}));
            match get_str(e, "type").as_str() {
                "tool_call" => {
                    let a = d.get("args").cloned().unwrap_or_else(|| json!({}));
                    if sget(&d, "name") == "browser_navigate" && a.get("url").is_some_and(truthy) {
                        let netloc = omuse_common::guard::urlparse(&sget(&a, "url"))
                            .map(|p| p.netloc)
                            .unwrap_or_default();
                        let dom = netloc.strip_prefix("www.").unwrap_or(&netloc).to_string();
                        if !dom.is_empty() && !sites.contains(&dom) {
                            sites.push(dom);
                        }
                    }
                    if sget(&d, "name") == "browser_fill_secret" {
                        fills += 1;
                    }
                }
                "waiting" => {
                    let w = if d.is_object() { d.clone() } else { json!({}) };
                    let summ = w
                        .get("summary")
                        .filter(|s| truthy(s))
                        .cloned()
                        .unwrap_or_else(|| json!({}));
                    let el = arr(&summ, "fields")
                        .iter()
                        .filter_map(|f| f.as_array())
                        .find(|f| {
                            let k = f.first().map(py_str).unwrap_or_default();
                            k.contains("Element") || k.contains("元素")
                        })
                        .and_then(|f| f.get(1).map(py_str))
                        .unwrap_or_default();
                    let aid = sget(&w, "approval_id");
                    if !aid.is_empty() {
                        let lab = if !el.is_empty() {
                            el.clone()
                        } else if !sget(&summ, "title").is_empty() {
                            sget(&summ, "title")
                        } else {
                            get_str(&w, "tool")
                        };
                        pending.insert(aid, lab);
                    }
                    let r = sget(&w, "reason");
                    last_wait = take_chars(if r.is_empty() { &el } else { &r }, 200).to_string();
                }
                "approval_resolved" => {
                    let lab = pending
                        .get(&sget(&d, "approval_id"))
                        .cloned()
                        .unwrap_or_default();
                    if !lab.is_empty() {
                        approvals.push(format!(
                            "{} → {}",
                            lab,
                            super::util::pystr_opt(d.get("decision"))
                        ));
                    }
                }
                _ => {}
            }
        }
        let steps: Vec<String> = task
            .get("plan")
            .map(|p| arr(p, "steps"))
            .unwrap_or(&[])
            .iter()
            .take(8)
            .map(|st| {
                format!(
                    "{}: {}",
                    super::util::pystr_opt(st.get("status")),
                    super::util::pystr_opt(st.get("description"))
                )
            })
            .collect();
        let status = get_str(task, "status");
        let mut parts = vec![format!(
            "[Earlier task in this chat — status {}] {}",
            status,
            truncate(&get_str(task, "goal"), 300)
        )];
        if !sites.is_empty() {
            parts.push(format!(
                "Sites used: {}",
                sites.iter().take(6).cloned().collect::<Vec<_>>().join(", ")
            ));
        }
        if !steps.is_empty() {
            parts.push(format!(
                "Plan: {}",
                steps
                    .iter()
                    .map(|x| truncate(x, 120))
                    .collect::<Vec<_>>()
                    .join(" | ")
            ));
        }
        if !approvals.is_empty() {
            parts.push(format!(
                "Approvals: {}",
                approvals[approvals.len().saturating_sub(8)..].join("; ")
            ));
        }
        parts.push(format!(
            "Card details entered from the vault: {}",
            if fills > 0 { "yes" } else { "no" }
        ));
        if status != "COMPLETED" && !last_wait.is_empty() {
            parts.push(format!("Stopped at: {}", last_wait));
        }
        let result = sget(task, "result");
        if !result.is_empty() {
            parts.push(format!("Result: {}", truncate(&result, 600)));
        }
        parts.join("\n")
    }

    pub(crate) fn history(&self, conv_id: &str, exclude_task: &str) -> (Vec<Value>, String) {
        let mut msgs: Vec<Value> = self
            .store
            .msgs(conv_id, 30)
            .unwrap_or_default()
            .into_iter()
            .filter(|m| get_str(m, "task_id") != exclude_task || get_str(m, "role") != "user")
            .collect();
        // tasks that ended without a reply (cancelled, failed, still waiting) get a digest of what they did
        if let Ok(tasks) = self.store.tasks(None, 10, Some(conv_id)) {
            let answered: HashSet<String> = msgs
                .iter()
                .filter(|m| get_str(m, "role") == "assistant" && !get_str(m, "task_id").is_empty())
                .map(|m| get_str(m, "task_id"))
                .collect();
            let tasks: HashMap<String, Value> = tasks
                .into_iter()
                .filter(|t| get_str(t, "id") != exclude_task)
                .map(|t| (get_str(&t, "id"), t))
                .collect();
            let mut out = Vec::new();
            for m in msgs {
                let tid = get_str(&m, "task_id");
                let role = get_str(&m, "role");
                out.push(m);
                if role == "user" && tasks.contains_key(&tid) && !answered.contains(&tid) {
                    out.push(json!({"role": "assistant", "content": self.task_digest(&tasks[&tid]), "task_id": tid, "meta": {}}));
                }
            }
            msgs = out;
        }
        let (mut hist, mut lines): (Vec<Value>, Vec<String>) = (vec![], vec![]);
        for m in &msgs[msgs.len().saturating_sub(12)..] {
            let role = get_str(m, "role");
            if role != "user" && role != "assistant" {
                continue;
            }
            let raw = py_str(m.get("content").unwrap_or(&Value::Null));
            let mut c = truncate(&raw, 2500);
            let atts = m
                .get("meta")
                .filter(|x| x.is_object())
                .map(|x| arr(x, "attachments"))
                .unwrap_or(&[]);
            if !atts.is_empty() {
                c += &format!(
                    "\n[attached: {}]",
                    atts.iter()
                        .map(|x| format!(
                            "{} ({})",
                            super::util::pystr_opt(x.get("path")),
                            super::util::pystr_opt(x.get("kind"))
                        ))
                        .collect::<Vec<_>>()
                        .join(", ")
                );
            }
            hist.push(json!({"role": role, "content": c}));
            lines.push(format!("{}: {}", role, truncate(&raw, 400)));
        }
        // drop the current user message (it's appended as the goal)
        if hist.last().is_some_and(|h| get_str(h, "role") == "user") {
            hist.pop();
            lines.pop();
        }
        while hist.first().is_some_and(|h| get_str(h, "role") != "user") {
            hist.remove(0);
        }
        let txt = lines[lines.len().saturating_sub(8)..].join("\n");
        (hist, txt)
    }

    /// The skills the agent may use: built-in + imported, minus the ones turned off in Settings → Skills.
    pub fn skills(&self) -> Vec<Value> {
        self.skills_dirs.enabled(&self.settings())
    }

    /// Long-term facts for this task (personal context v1): the ones that matter for what is asked, grouped by
    /// domain. A golden A/B run can ask for the old selection with `ctx_mode[task_id] = "legacy"`.
    /// The profile's standing preferences for the executor prompt, as (label, value): the fixed
    /// `preferred_airline` / `preferred_hotel_brand` fields, then any custom field whose label carries one of
    /// `store::PREFERENCE_WORDS` (a second airline, a loyalty programme, a seat wish…). Empty values are skipped;
    /// nothing else of the profile is ever put in the prompt.
    pub fn standing_prefs(&self) -> Vec<(String, String)> {
        let prof = self.store.profile().unwrap_or_else(|_| json!({}));
        let Some(map) = prof.as_object() else {
            return Vec::new();
        };
        let mut out = Vec::new();
        for key in crate::store::PREFERENCE_KEYS {
            let v = map.get(key).and_then(Value::as_str).unwrap_or("");
            if !v.trim().is_empty() {
                let label = crate::store::PROFILE_FIELDS
                    .iter()
                    .find(|(k, _, _)| *k == key)
                    .map(|(_, _, en)| en.to_string())
                    .unwrap_or_else(|| key.to_string());
                out.push((label, v.trim().to_string()));
            }
        }
        for (k, v) in map {
            let Some(label) = k.strip_prefix("custom:") else {
                continue;
            };
            let v = v.as_str().unwrap_or("");
            let low = label.to_lowercase();
            if !v.trim().is_empty()
                && crate::store::PREFERENCE_WORDS
                    .iter()
                    .any(|w| low.contains(w))
            {
                out.push((label.to_string(), v.trim().to_string()));
            }
        }
        out
    }

    pub fn facts_for(&self, goal: &str, history_txt: &str, task_id: &str) -> Vec<Value> {
        if self.inner.lock().ctx_mode.get(task_id).map(String::as_str) == Some("legacy") {
            return self.facts_legacy(goal);
        }
        let users: Vec<&str> = history_txt
            .lines()
            .filter_map(|ln| ln.strip_prefix("user: "))
            .collect();
        let users = users[users.len().saturating_sub(3)..].join("\n");
        let facts = self.store.facts(500, Some("long")).unwrap_or_default();
        let ents = self.store.entities(None).unwrap_or_default();
        let out = crate::context::select(goal, &facts, Some(&ents), 12, &users);
        let golden = self
            .store
            .task(task_id)
            .ok()
            .flatten()
            .map(|t| get_str(&t, "source") == "golden")
            .unwrap_or(false);
        if !golden {
            let ids: Vec<String> = out.iter().take(12).map(|f| get_str(f, "id")).collect();
            let _ = self.store.mark_used(&ids);
        }
        out
    }

    /// The selection before batch 2 (kept for the A/B comparison).
    pub fn facts_legacy(&self, goal: &str) -> Vec<Value> {
        let found = self
            .store
            .search_facts(goal, 10, Some("long"), false)
            .unwrap_or_default();
        let mut prefs: Vec<Value> = self
            .store
            .facts(300, Some("long"))
            .unwrap_or_default()
            .into_iter()
            .filter(|f| {
                matches!(
                    get_str(f, "category").as_str(),
                    "preference" | "person" | "habit"
                )
            })
            .collect();
        let num = |f: &Value, k: &str| f.get(k).and_then(omuse_common::util::as_f64).unwrap_or(0.0);
        prefs.sort_by(|a, b| {
            (num(b, "uses"), num(b, "last_verified"))
                .partial_cmp(&(num(a, "uses"), num(a, "last_verified")))
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        prefs.truncate(10);
        let mut seen = HashSet::new();
        let mut out = Vec::new();
        for f in found.into_iter().chain(prefs) {
            if seen.insert(get_str(&f, "id")) {
                out.push(f);
            }
        }
        out.truncate(15);
        out
    }

    /// Tell the ledger how a purchase ended: the order number from the confirmation page, or none.
    pub async fn ledger_confirm(&self, task_id: &str, oc: &Value, final_: &str) {
        let num = arr(oc, "evidence")
            .iter()
            .find(|r| r.is_object() && get_str(r, "kind") == "purchase")
            .map(|r| sget(r, "order_number"))
            .unwrap_or_default();
        let evidence = if num.is_empty() {
            Value::Null
        } else {
            json!({"kind": "confirmation", "text": truncate(final_, 400)})
        };
        match self
            .sentinel(
                "POST",
                "/internal/ledger/confirm",
                Some(json!({"task_id": task_id, "order_number": num, "evidence": evidence})),
                15.0,
            )
            .await
        {
            Ok(res) => {
                let orders = arr(&res, "orders");
                if !orders.is_empty() {
                    let list: Vec<Value> = orders
                        .iter()
                        .map(|o| json!({"merchant": o.get("merchant"), "order_number": o.get("order_number"),
                                        "status": o.get("status_label")}))
                        .collect();
                    self.event(task_id, "ledger", json!({"orders": list})).await;
                }
            }
            Err(e) => println!("[ledger] {}", e),
        }
    }

    /// Remember that a site blocks the automated browser, so next time OMuse goes another way from the start
    /// (a site habit, domain=site, kept as 'to confirm'). Recorded once per site.
    pub async fn learn_site_block(&self, task_id: &str, site: &str, detail: &str) {
        if site.chars().count() < 3 {
            return;
        }
        if !self.inner.lock().block_noted.insert(site.to_string()) {
            return;
        }
        let existing = self
            .store
            .search_facts(&format!("{} 反机器人 bot", site), 5, Some("long"), true)
            .unwrap_or_default();
        if existing.iter().any(|f| {
            let fact = get_str(f, "fact");
            fact.contains(site)
                && (fact.contains("反机器人") || fact.to_lowercase().contains("bot"))
        }) {
            return;
        }
        let fact = format!(
            "{site} 会拦截自动浏览器（{}）：直接浏览通常进不去，优先用别的来源，必须用时让用户接管浏览器通过验证 ({site} blocks the automated browser — prefer another source, or a user takeover when it must be used)",
            if detail.is_empty() { "反机器人检查" } else { detail }
        );
        let o = FactOpts {
            source: format!("learned:block:{}", task_id),
            confidence: 0.8,
            domain: "site".into(),
            status: "pending".into(),
            ..Default::default()
        };
        match self.store.add_fact(&fact, "habit", site, &o) {
            Ok(_) => self.publish(json!({"kind": "memory_update"})).await,
            Err(e) => println!("[block-learn] {}", e),
        }
    }

    /// The first time a task lands on a site, what OMuse already learned about it (guest checkout, phone format …).
    pub async fn site_note(&self, task_id: &str, url: &str) -> String {
        let name = crate::context::site_name(url);
        if name.is_empty() {
            return String::new();
        }
        if !self
            .inner
            .lock()
            .site_noted
            .entry(task_id.to_string())
            .or_default()
            .insert(name.clone())
        {
            return String::new();
        }
        let found = crate::context::site_facts(
            url,
            &self.store.facts(500, Some("long")).unwrap_or_default(),
        );
        if found.is_empty() {
            return String::new();
        }
        let _ = self
            .store
            .mark_used(&found.iter().map(|f| get_str(f, "id")).collect::<Vec<_>>());
        let lines: Vec<String> = found
            .iter()
            .map(|f| {
                format!(
                    "- {}{}",
                    get_str(f, "fact"),
                    if get_str(f, "status") == "pending" {
                        " (unconfirmed)"
                    } else {
                        ""
                    }
                )
            })
            .collect();
        self.event(task_id, "context_site",
                   json!({"site": name, "facts": found.iter().map(|f| truncate(&get_str(f, "fact"), 160)).collect::<Vec<_>>()}))
            .await;
        format!(
            "[记忆：你以前在 {} 学到的 — what you learned about this site before; follow it]\n{}",
            name,
            lines.join("\n")
        )
    }

    /// One setting decides the agent's language (reasoning, plans, notes, answers) — see Settings → Language.
    pub fn reply_lang(settings: &Value) -> &'static str {
        prompts::lang_name(agent_lang(settings))
    }

    /// The planner: `{"objective", "steps": [...], "version": 1}` (an empty plan when the model fails).
    pub async fn plan(
        &self,
        task: &Value,
        facts: &[Value],
        history_txt: &str,
        state: &str,
    ) -> Value {
        let s = self.settings();
        let cat = self.catalog(false).await;
        let conns = cat
            .get("connections")
            .filter(|c| truthy(c))
            .cloned()
            .unwrap_or_else(|| json!({}));
        let mcp = conns
            .get("mcp")
            .filter(|c| truthy(c))
            .cloned()
            .unwrap_or_else(|| json!({}));
        let live: Vec<String> = arr(&mcp, "servers")
            .iter()
            .filter(|x| get_truthy(x, "enabled") && get_truthy(x, "tools"))
            .map(|x| format!("{} (mcp:{})", sget(x, "name"), sget(x, "id")))
            .collect();
        // 2026-10-02 R7-13: a "plan my Saturday" request got a "check Google Calendar" step although the calendar isn't
        // connected; the executor then web-searched "Google Calendar API status" until it gave up
        let off: Vec<&str> = ["gmail", "calendar", "notion", "slack"]
            .into_iter()
            .filter(|k| !conns.get(*k).is_some_and(|c| get_truthy(c, "ready")))
            .collect();
        let lg = agent_lang(&s);
        let mut sys_prompt = format!(
            "{}\n\n{}",
            prompts::language_rule(lg),
            prompts::PLANNER_SYSTEM
        );
        if !live.is_empty() {
            sys_prompt += &format!("\nConnected MCP servers: {}.", live.join(", "));
        }
        if !off.is_empty() {
            sys_prompt += &format!(
                "\nNOT connected (do not plan steps that use them; if the request needs one, plan to do the rest and tell the user it can be connected in Connections): {}.",
                off.join(", ")
            );
        }
        let goal = get_str(task, "goal");
        let msgs = [
            json!({"role": "system", "content": sys_prompt}),
            user_msg(prompts::planner_user(
                &goal,
                history_txt,
                facts,
                state,
                Self::reply_lang(&s),
            )),
        ];
        let tid = get_str(task, "id");
        let pm = sget(&s, "planner_model");
        let o = ChatOpts {
            max_tokens: Some(1500),
            temperature: Some(0.2),
            model: if pm.is_empty() { None } else { Some(pm) },
            no_think: true,
            ..opts("planner", &tid)
        };
        let plan = match self.chat_or_cancel(&tid, &msgs, None, &o).await {
            None => json!({}),
            Some(Ok(r)) => extract_json(&r.content).unwrap_or_else(|| json!({})),
            Some(Err(e)) => {
                self.event(
                    &tid,
                    "planner_error",
                    json!({"message": take_chars(&e.to_string(), 300)}),
                )
                .await;
                json!({})
            }
        };
        let plan = if plan.is_object() { plan } else { json!({}) };
        let mut steps = Vec::new();
        for (i, st) in arr(&plan, "steps").iter().enumerate() {
            if st.is_object() && st.get("description").is_some_and(truthy) {
                let id = sget(st, "id");
                let mut step = json!({
                    "id": if id.is_empty() { format!("s{}", i + 1) } else { id },
                    "description": take_chars(&py_str(&st["description"]), 300),
                    "tool_hint": take_chars(&st.get("tool_hint").map(py_str).unwrap_or_default(), 40),
                    "risk": st.get("risk").map(py_str).unwrap_or_else(|| "read".into()),
                    "status": "pending",
                });
                // steps that share a label are independent: the executor runs them at the same time
                let group = prompts::parallel_group(st);
                if !group.is_empty() {
                    step["parallel"] = json!(take_chars(&group, 20));
                }
                steps.push(step);
            }
        }
        steps.truncate(10);
        let obj = sget(&plan, "objective");
        json!({"objective": take_chars(if obj.is_empty() { &goal } else { &obj }, 300), "steps": steps, "version": 1})
    }

    /// The executor's call options. With Settings `stream_answers` on (the default) the answer streams to the page
    /// while the model writes it: `answer_delta` events (`task_id`, `conv_id`, `text` = the visible text so far,
    /// `calls` = the turn carries tool calls, so the text is commentary), published and not stored, at most ten a
    /// second with the newest text. Reasoning is never streamed (`llm::SseAssembler::visible`), so the text starts
    /// when the answer proper does. The returned task ends once the call is over and the last text is out: the loop
    /// awaits it before acting on the reply, so no delta trails the stored message.
    fn executor_opts(
        &self,
        task_id: &str,
        conv_id: &str,
        s: &Value,
    ) -> (ChatOpts, Option<JoinHandle<()>>) {
        let mut o = opts("executor", task_id);
        if !s.get("stream_answers").map(truthy).unwrap_or(true) {
            return (o, None);
        }
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<crate::llm::StreamDelta>();
        o.stream_to = Some(tx);
        let rt = self.arc();
        let (tid, cid) = (task_id.to_string(), conv_id.to_string());
        let fwd = tokio::spawn(async move {
            let mut last: Option<crate::llm::StreamDelta> = None;
            while let Some(mut d) = rx.recv().await {
                while let Ok(newer) = rx.try_recv() {
                    d = newer; // only the newest text matters
                }
                if last.as_ref() != Some(&d) {
                    rt.publish(
                        json!({"kind": "answer_delta", "task_id": tid, "conv_id": cid,
                                      "text": d.text, "calls": d.tool_calls}),
                    )
                    .await;
                    last = Some(d);
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        });
        (o, Some(fwd))
    }

    /// The plan when the executor asks for one (`plan_task`) or loads a skill, instead of before every turn: the task
    /// shows PLANNING while the planner model works, the plan is stored and announced, and comes back — `None` when the
    /// task vanished or was cancelled meanwhile (the status is then left alone). `state` is the executor's context
    /// (its notes, or the skill text) for the planner.
    pub async fn plan_on_request(&self, task_id: &str, state: &str) -> Option<Value> {
        let t = self.store.task(task_id).ok().flatten()?;
        let goal_text = get_str(&t, "goal");
        let (_, history_txt) = self.history(&get_str(&t, "conv_id"), task_id);
        let facts = self.facts_for(&goal_text, &history_txt, task_id);
        let mut pt = t.clone();
        let atts = arr(&t, "attachments");
        if !atts.is_empty() {
            pt["goal"] = json!(format!(
                "{}\n(Attached files: {})",
                goal_text,
                atts.iter()
                    .map(|x| format!(
                        "{} [{}]",
                        super::util::pystr_opt(x.get("name")),
                        super::util::pystr_opt(x.get("kind"))
                    ))
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        self.set_status(task_id, "PLANNING", json!({})).await;
        let plan = self.plan(&pt, &facts, &history_txt, state).await;
        if self.cancel_flags.lock().contains(task_id) {
            return None;
        }
        self.store
            .update_task(task_id, &json!({"plan": plan}))
            .ok()?;
        self.event(task_id, "plan", plan.clone()).await;
        self.set_status(task_id, "RUNNING", json!({})).await;
        Some(plan)
    }

    /// Drop the browser tools when a loaded skill turned the browser off for this task (see `Inner::no_browser`); and
    /// `delegate`, since a sub-agent without the browser only wanders through the workspace.
    pub fn without_browser_if_off(&self, task_id: &str, tools: Vec<Value>) -> Vec<Value> {
        if !self.inner.lock().no_browser.contains_key(task_id) {
            return tools;
        }
        tools
            .into_iter()
            .filter(|t| browser_policy_allows(tool_name(t)))
            .collect()
    }

    /// The tool list for this run: Sentinel's catalog + the local tools, minus the schedule/goal-only ones.
    pub fn tools_for(
        catalog: &Value,
        allow: Option<&[&str]>,
        schedule: bool,
        goal: bool,
    ) -> Vec<Value> {
        let mut tools: Vec<Value> = arr(catalog, "tools").to_vec();
        tools.extend(LOCAL_TOOLS.iter().cloned());
        tools
            .into_iter()
            .filter(|t| {
                let n = tool_name(t);
                (schedule || (n != "schedule_state_get" && n != "schedule_state_set"))
                    && (goal || n != "goal_update")
                    && allow.is_none_or(|a| a.contains(&n))
            })
            .collect()
    }

    /// Keep the last steps for delivering what the user asked for.
    ///
    /// A research-heavy task used to spend every step reading pages and hit the limit before writing the PDF / sending
    /// the email. Now: (1) after many page visits, one nudge to start writing; (2) with BUDGET_RESERVE steps left, a note
    /// to stop gathering and deliver; (3) in the last 2 steps only delivery tools remain.
    pub fn budget(
        transcript: &mut Vec<Value>,
        tools: Vec<Value>,
        remaining: i64,
        lang: &str,
    ) -> Vec<Value> {
        let said = user_said(transcript);
        let pages = transcript
            .iter()
            .filter(|m| get_str(m, "role") == "assistant")
            .flat_map(tool_calls)
            .filter(|c| {
                c.get("function")
                    .and_then(|f| f.get("name"))
                    .and_then(Value::as_str)
                    == Some("browser_navigate")
            })
            .count() as i64;
        if pages >= RESEARCH_NUDGE_PAGES
            && !said.contains(BUDGET_MARK_PAGES)
            && remaining > BUDGET_RESERVE
        {
            let txt = if lang == "en" {
                format!(
                    "(System) [{BUDGET_MARK_PAGES}] You have opened {pages} pages. If you have enough, start writing the result now; otherwise read at most 2–3 more pages or delegate the rest to a sub-agent."
                )
            } else {
                format!(
                    "（系统）[{BUDGET_MARK_PAGES}] 你已经打开了 {pages} 个网页。如果信息已经够用，现在就开始写结果/交付物；还缺的话最多再看 2–3 个页面，或者用 delegate 交给子 Agent 去查。"
                )
            };
            transcript.push(user_msg(txt));
        }
        if remaining <= BUDGET_RESERVE && !said.contains(BUDGET_MARK) {
            let txt = if lang == "en" {
                format!(
                    "(System) [{BUDGET_MARK}] Only {remaining} steps left: stop gathering yourself and produce the deliverable the \
user asked for with what you have (write the file, make_pdf/make_xlsx, send_file, send the email…), then give \
the final answer and note anything left unverified. If a fixed number of items the user asked for is still \
short, use one delegate call for the rest (it doesn't use these steps). Internal control — do not mention a \
'budget' or remaining steps to the user."
                )
            } else {
                format!(
                    "（系统）[{BUDGET_MARK}] 只剩 {remaining} 步了。停止自己继续搜集资料，用已有的信息马上完成\
用户要的交付物（写文件、生成 PDF/Excel、send_file、发邮件等），然后给出最终回答，并说明哪些没来得及核实。\
如果用户要的固定数量条目还没凑齐，用一次 delegate 把剩下的交给子 Agent（不占用这里的步数）。\
这是内部控制，不要向用户提「预算」或剩余步数。"
                )
            };
            transcript.push(user_msg(txt));
        }
        if remaining <= 2 {
            let fin: Vec<Value> = tools
                .iter()
                .filter(|x| FINISH_TOOLS.contains(&tool_name(x)))
                .cloned()
                .collect();
            if !fin.is_empty() {
                return fin;
            }
        }
        tools
    }

    /// Keep the context small: shrink old tool results, keep the last few intact (see [`compress_with`]).
    pub fn compress(&self, transcript: Vec<Value>, high: Option<usize>) -> Vec<Value> {
        let ctx = self.inner.lock().ctx_chars;
        compress_with(transcript, high, ctx)
    }

    /// Run (or resume) a task until it finishes, waits for the user or is cancelled / paused.
    pub async fn run(&self, task_id: &str) -> Result<()> {
        let Some(mut t) = self.store.task(task_id)? else {
            return Ok(());
        };
        if is_terminal(&get_str(&t, "status")) {
            return Ok(());
        }
        let s = self.settings();
        let catalog = self.catalog(true).await;
        let conv_id = get_str(&t, "conv_id");
        let (history, history_txt) = self.history(&conv_id, task_id);
        let goal_text = get_str(&t, "goal");
        let facts = self.facts_for(&goal_text, &history_txt, task_id);
        let t_transcript_empty = arr(&t, "transcript").is_empty();
        if facts
            .first()
            .is_some_and(|f| f.get("score").is_some_and(|x| !x.is_null()))
            && t_transcript_empty
        {
            let list: Vec<Value> = facts
                .iter()
                .map(|f| json!({"id": f.get("id"), "fact": truncate(&get_str(f, "fact"), 160), "domain": f.get("domain"),
                                "why": f.get("why"), "status": f.get("status")}))
                .collect();
            self.event(task_id, "context_used", json!({"facts": list}))
                .await;
        }

        // No planner call before the first turn: the executor sees every request first and answers the simple ones
        // itself; it asks for a plan (plan_task) or gets one with the skill it loads — see `plan_on_request`.
        if self.cancel_flags.lock().contains(task_id) {
            return Ok(()); // cancelled before the first turn: don't flip CANCELLED back to RUNNING
        }
        if get_str(&t, "status") != "RUNNING" {
            self.set_status(task_id, "RUNNING", json!({})).await;
        }

        let mut transcript: Vec<Value> = arr(&t, "transcript").to_vec();
        let mut extra = String::new();
        let goal = crate::goals::goal_for_task(&self.store, &t)?;
        let schedule_id = get_str(&t, "schedule_id");
        let lg0 = agent_lang(&s);
        if get_str(&t, "source") == "schedule" && !schedule_id.is_empty() {
            let sch = self.store.schedule(&schedule_id)?;
            if goal.is_some() {
                extra = "\n## Goal check\nThis task is an automatic check of a long-running goal the user set (the goal, its success \
criteria and the progress history are in the user message). The user is not watching. Take the next useful \
step toward the goal (actions that send/submit still need approval — just call the tool). Do not repeat work \
already recorded in the progress history. You MUST call goal_update exactly once at the end."
                    .to_string();
            } else if let Some(sch) = sch {
                let mut st = Map::new();
                if let Some(o) = sch.get("state").and_then(Value::as_object) {
                    for (k, v) in o {
                        if !k.starts_with('_') {
                            st.insert(k.clone(), v.clone());
                        }
                    }
                }
                let kind = if get_str(&sch, "kind") == "event" {
                    "event trigger"
                } else {
                    "schedule"
                };
                extra = format!(
                    "\n## Scheduled run\nThis task is an automatic run of {} 「{}」. The user is not watching. \
State saved by previous runs: {}. Use schedule_state_set to save what you observed; \
call notify_user only if something the user cares about happened.",
                    kind,
                    get_str(&sch, "name"),
                    pyjson::dumps(&Value::Object(st))
                );
            }
        }
        if let Some(subject) = self.store.subject_by_conv(&conv_id)? {
            extra += &crate::library::chat_context(
                &subject,
                &crate::library::read_report(&self.workspace, &get_str(&subject, "slug")),
                lg0,
            );
        } else if !self.store.subjects()?.is_empty() {
            let hits = self.store.library_search(&goal_text, 3)?;
            if !hits.is_empty() {
                extra += &crate::library::hits_context(&hits, lg0);
                let list: Vec<Value> = hits
                    .iter()
                    .map(|h| json!({"title": h.get("title"), "heading": h.get("heading")}))
                    .collect();
                self.event(task_id, "context_library", json!({"hits": list}))
                    .await;
            }
        }
        let conv_files = self.store.conv_attachments(&conv_id)?;
        if !conv_files.is_empty() {
            let lines: Vec<String> = conv_files[conv_files.len().saturating_sub(30)..]
                .iter()
                .map(|x| {
                    let k = sget(x, "kind");
                    let k = if k.is_empty() {
                        super::ext::at::kind_of(&sget(x, "path")).to_string()
                    } else {
                        k
                    };
                    format!(
                        "- {} ({}, {} bytes)",
                        super::util::pystr_opt(x.get("path")),
                        k,
                        super::util::pystr_opt(x.get("size"))
                    )
                })
                .collect();
            extra += &format!(
                "\n## Files the user attached in this conversation\n{}\nWhen the user refers to them, open them with files_read (documents) or file_look (images, videos, audio, scanned PDFs).",
                lines.join("\n")
            );
        }
        if lg0 == "en" && prompts::wants_cjk_output(&goal_text) {
            // 2026-10-02 R8: the app is in English but this request asks for text to use in Chinese (a post, a story …)
            extra += "\n## Output language for this request\nThe user asked, in Chinese, for text they will use as written: write \
that text (the post, story, poem, message, translation …) in Chinese — or in the language they name. \
Your own notes and the short explanation around it stay in English.";
        }
        if transcript.is_empty() {
            let mut first = goal_text.clone();
            if !arr(&t, "attachments").is_empty() {
                first += &self.attachment_context(&t).await;
                self.mark_confidential(task_id, "attachment").await;
            }
            transcript = vec![json!({"role": "system", "content": ""})];
            transcript.extend(history);
            transcript.push(user_msg(first));
        }

        // ---------------------------------------------------------- resume after approval / takeover
        let pend = t.get("pending").cloned().unwrap_or(Value::Null);
        if pend.get("calls").is_some_and(truthy) {
            let Some(resolved) = pend.get("resolved").filter(|r| truthy(r)).cloned() else {
                return Ok(()); // still waiting
            };
            let calls: Vec<ToolCall> = arr(&pend, "calls")
                .iter()
                .filter_map(|c| serde_json::from_value(strip_private(c)).ok())
                .collect();
            if let Some(first) = calls.first() {
                let result = resolved
                    .get("result")
                    .filter(|r| truthy(r))
                    .cloned()
                    .unwrap_or_else(|| json!({}));
                let content = self.format_external(&first.name, &result);
                transcript
                    .push(json!({"role": "tool", "tool_call_id": first.id, "content": content}));
                self.note_tool_text(task_id, &content);
                let st = result
                    .get("status")
                    .cloned()
                    .unwrap_or_else(|| resolved.get("decision").cloned().unwrap_or(Value::Null));
                self.event(task_id, "tool_result",
                           json!({"call_id": first.id, "name": first.name, "status": st, "preview": truncate(&content, 800)}))
                    .await;
                self.store
                    .update_task(task_id, &json!({"pending": null, "transcript": transcript}))?;
                let rest = &calls[1..];
                for (i, call) in rest.iter().enumerate() {
                    if let Err(sp) = self
                        .exec_call(
                            &t,
                            call,
                            &mut transcript,
                            &catalog,
                            &rest[i + 1..],
                            None,
                            CallMode::MAIN,
                        )
                        .await
                    {
                        return self.suspend(task_id, sp, transcript).await;
                    }
                }
            }
        }

        let mut steps = t
            .get("steps")
            .and_then(omuse_common::util::as_i64)
            .unwrap_or(0);
        let max_steps = s
            .get("max_steps")
            .and_then(omuse_common::util::as_i64)
            .filter(|n| *n != 0)
            .unwrap_or(30);
        let mut consecutive_errors = 0;
        let run_started = now_ts();
        self.inner.lock().llm_wait.insert(task_id.to_string(), 0.0);
        let tz = get_str(&s, "timezone");
        let started_txt = prompts::now_str(&tz, agent_lang(&s));
        let max_minutes = s
            .get("max_minutes")
            .filter(|v| truthy(v))
            .map(|v| {
                omuse_common::util::as_f64(v)
                    .map(|m| m.max(0.5))
                    .unwrap_or(20.0)
            })
            .unwrap_or(20.0);
        let mut stuck = 0; // turns in a row where every call was refused by a loop guard
        let mut gave_up = false; // set when the agent keeps going round in circles: it must answer now, without tools
        let mut searched: HashMap<String, String> = HashMap::new(); // normalised web_search query -> first call id
        let (mut nudged, mut cut_nudged, mut numbers_checked, mut outcome_checked) =
            (false, false, false, false);
        let mut ctx_retries = 0;
        loop {
            if self.cancel_flags.lock().contains(task_id) {
                return Ok(());
            }
            if self.pause_flags.lock().remove(task_id) {
                self.store
                    .update_task(task_id, &json!({"transcript": transcript}))?;
                self.set_status(task_id, "PAUSED", json!({"waiting": {"type": "paused"}}))
                    .await;
                return Ok(());
            }
            t = self
                .store
                .task(task_id)?
                .ok_or_else(|| anyhow!("task vanished"))?;
            let lg = agent_lang(&s);
            // the system prompt stays identical for the whole run (live state goes in the status note at the end)
            transcript[0] = json!({"role": "system", "content": prompts::executor_system(&prompts::ExecutorPrompt {
                user_name: get_str(&s, "user_name"),
                tz: tz.clone(),
                connections: catalog.get("connections").cloned().unwrap_or_else(|| json!({})),
                plan: None,
                facts: facts.clone(),
                prefs: self.standing_prefs(),
                skills: self.skills(),
                extra: extra.clone(),
                language: lg.to_string(),
                reply_lang: Self::reply_lang(&s).to_string(),
                now_txt: started_txt.clone(),
            })});
            transcript = self.compress(transcript, None);
            let waited = self
                .inner
                .lock()
                .llm_wait
                .get(task_id)
                .copied()
                .unwrap_or(0.0);
            let minutes = (now_ts() - run_started - waited).max(0.0) / 60.0;
            if minutes >= max_minutes * TIME_NUDGE && !user_said(&transcript).contains(TIME_MARK) {
                let txt = if lg == "en" {
                    format!("(System) [{TIME_MARK}] This task has run for {:.0} minutes (limit {}). Stop gathering \
and deliver what the user asked for with what you have (chart, file, answer); note what is unverified.", minutes, fmt_float(max_minutes))
                } else {
                    format!(
                        "（系统）[{TIME_MARK}] 这个任务已经用了 {:.0} 分钟（上限 {} 分钟）。\
停止继续搜集，用已有的信息尽快完成用户要的结果（图表、文件、回答），并说明哪些还没核实。",
                        minutes,
                        fmt_float(max_minutes)
                    )
                };
                transcript.push(user_msg(txt));
            }
            let web_calls = transcript
                .iter()
                .filter(|m| get_str(m, "role") == "assistant")
                .flat_map(tool_calls)
                .filter(|c| {
                    let n = stored_call(c).0;
                    n.starts_with("browser_") || n == "web_search" || n == "news_search"
                    // the searches of the web count
                })
                .count();
            if web_calls as i64 >= WEB_NUDGE && !user_said(&transcript).contains(WEB_MARK) {
                // 2026-10-02 R4-04b: a "top 3 products" lookup made 47 web calls in 12 minutes chasing cleaner results
                let txt = if lg == "en" {
                    format!("(System) [{WEB_MARK}] This task has made {web_calls} web calls. Stop opening pages one by one yourself. \
If the user asked for a fixed number of items (e.g. 3 jobs, 5 companies) and you don't have them all yet, \
use delegate to fetch the rest (a sub-agent per remaining item — its calls don't count here) and compile \
them yourself; if you have enough or truly cannot find more, answer with what you have and note what is \
unverified. This is an internal efficiency control — never mention a 'budget' or a call count to the user.")
                } else {
                    format!("（系统）[{WEB_MARK}] 这个任务已经调用了 {web_calls} 次网页工具。不要自己再一个个翻网页了。\
如果用户要的是固定数量的条目（例如 3 个岗位、5 家公司）而你还没凑齐，就用 delegate 把还差的交给子 Agent 去查\
（子 Agent 的调用不算在这里），再由你汇总；如果已经够了或确实查不到更多，就用已有信息完成回答，说明哪些没核实。\
这是内部效率控制，不要向用户提起「预算」或调用次数。")
                };
                transcript.push(user_msg(txt));
            }
            let timed_out = minutes >= max_minutes;
            let force_final = steps >= max_steps || gave_up || timed_out;
            let mut tools: Option<Vec<Value>> = None;
            if !force_final {
                let tl = Self::tools_for(
                    &catalog,
                    None,
                    !schedule_id.is_empty() && goal.is_none(),
                    goal.is_some(),
                );
                let tl = self.without_browser_if_off(task_id, tl);
                let tl = Self::budget(&mut transcript, tl, max_steps - steps, lg);
                tools = Some(if lg == "en" {
                    prompts::strip_tools_en(&tl)
                } else {
                    tl
                });
            }
            if gave_up {
                transcript.push(user_msg(prompts::l(lg,
                    "（系统）同样的方法和来源反复失败，已经换过计划也没有进展，所以停止重试。请不要再调用工具，\
用已经拿到的信息给出尽可能有用的回答；清楚说明哪些数据没拿到、为什么（例如哪个网站打不开或被拦截），\
并建议用户下一步可以怎么做（例如换哪个来源、或者由用户提供数据）。",
                    "(System) The same methods and sources keep failing even after re-planning, so retrying has stopped. \
Do not call tools. Give the most useful answer you can from what you already have, say clearly what could \
not be fetched and why (e.g. which site would not open or was blocked), and suggest what the user can do \
next (another source, or providing the data).")));
            } else if timed_out {
                let txt = if lg == "en" {
                    format!("(System) The {:.0}-minute time limit is reached: stop calling tools and give the most complete \
answer you can from what you have — what is done, the findings and what is missing.", max_minutes)
                } else {
                    format!("（系统）已达到 {:.0} 分钟的用时上限。请停止调用工具，用已有的信息给出尽量完整的回答，\
说明完成了什么、结论是什么、还缺什么。", max_minutes)
                };
                transcript.push(user_msg(txt));
            } else if force_final {
                transcript.push(user_msg(prompts::l(lg,
                    "（系统）已达到步数上限。请停止调用工具，总结目前完成的内容、结果和未完成的部分。",
                    "(System) Step limit reached: stop calling tools and summarize what is done, the results and what is left.")));
            }
            self.event(task_id, "thinking", json!({"step": steps + 1}))
                .await;
            let mut msgs = transcript.clone();
            if !force_final {
                // live state at the end (not stored), see prompts::status_note
                let dead = dead_ends_text(&source_failures(&self.store.events(task_id, 0)?), lg);
                msgs.push(user_msg(prompts::status_note(
                    t.get("plan"),
                    &tz,
                    lg,
                    minutes,
                    &dead,
                )));
            }
            let (exec_opts, live) = self.executor_opts(task_id, &conv_id, &s);
            let called = self
                .chat_or_cancel(task_id, &msgs, tools.as_deref(), &exec_opts)
                .await;
            drop(exec_opts); // closes the live-text channel …
            if let Some(live) = live {
                let _ = live.await; // … and the last text is out before the reply is acted on
            }
            let resp: ChatReply = match called {
                None => return Ok(()), // cancelled while waiting for the model (e.g. sleeping on a Retry-After)
                Some(Ok(r)) => r,
                Some(Err(LlmError::Context { msg, chars })) => {
                    // too big for the model server (several large results in one turn): learn the limit, shrink, try again
                    ctx_retries += 1;
                    let sent = if chars != 0 {
                        chars
                    } else {
                        content_chars(&msgs)
                    };
                    let limit = {
                        let mut inner = self.inner.lock();
                        let cur = if inner.ctx_chars != 0 {
                            inner.ctx_chars
                        } else {
                            sent
                        };
                        inner.ctx_chars = 8000usize.max((sent.min(cur) as f64 * 0.7) as usize);
                        inner.ctx_chars
                    };
                    self.event(
                        task_id,
                        "context_shrunk",
                        json!({"chars": sent, "limit": limit, "try": ctx_retries}),
                    )
                    .await;
                    if ctx_retries > 3 {
                        self.store
                            .update_task(task_id, &json!({"transcript": transcript}))?;
                        return Err(anyhow::Error::new(LlmError::Context { msg, chars }));
                    }
                    transcript = self.compress(transcript, None);
                    self.store
                        .update_task(task_id, &json!({"transcript": transcript}))?;
                    continue;
                }
                Some(Err(e)) => {
                    self.store
                        .update_task(task_id, &json!({"transcript": transcript}))?;
                    return Err(anyhow::Error::new(e));
                }
            };
            steps += 1;
            self.store.update_task(task_id, &json!({"steps": steps}))?;
            if !resp.reasoning.is_empty() {
                self.event(
                    task_id,
                    "reasoning",
                    json!({"text": truncate(&resp.reasoning, 1500)}),
                )
                .await;
            }
            let calls: Vec<ToolCall> = if force_final {
                vec![]
            } else {
                resp.tool_calls.clone()
            };
            if !calls.is_empty() {
                transcript.push(assistant_with_calls(&resp.content, &calls));
                if !resp.content.is_empty() {
                    self.event(
                        task_id,
                        "message",
                        json!({"text": truncate(&resp.content, 2000)}),
                    )
                    .await;
                }
                self.store
                    .update_task(task_id, &json!({"transcript": transcript}))?;
                // 1. which calls run and which are skipped as copies (decided up front, from the calls alone, so the
                //    concurrent runs below are known before anything executes)
                let mut seen_sigs: HashMap<String, String> = HashMap::new();
                let mut progress = false;
                let mut typed_this_turn = String::new();
                let mut skips: Vec<Option<Skip>> = Vec::with_capacity(calls.len());
                for call in &calls {
                    let sig = call_sig(&call.name, &call.args);
                    if let Some(first) = seen_sigs
                        .get(&sig)
                        .filter(|_| !REPEAT_STREAK_OK.contains(&call.name.as_str()))
                    {
                        // the model sometimes emits the same call 3-4 times in one turn: run it once, skip the copies
                        // (they are not failures, so they must not trigger a re-plan on their own)
                        skips.push(Some(Skip {
                            first: first.clone(),
                            search: false,
                            typed: false,
                        }));
                        continue;
                    }
                    seen_sigs.insert(sig, call.id.clone());
                    if call.name == "browser_type" && get_truthy(&call.args, "submit") {
                        if !typed_this_turn.is_empty() {
                            // 2026-10-04 V3-03: 12 searches typed into one shop's search box in a single turn — only the
                            // last results page existed, the rest were never read, and the list was searched 3x over
                            skips.push(Some(Skip {
                                first: typed_this_turn.clone(),
                                search: false,
                                typed: true,
                            }));
                            continue;
                        }
                        typed_this_turn = call.id.clone();
                    }
                    if call.name == "web_search" || call.name == "news_search" {
                        // per tool: the same words may be a news search and a web search
                        let raw = search_key(call.args.get("query").unwrap_or(&Value::Null));
                        let qk = if raw.is_empty() {
                            raw
                        } else {
                            format!("{}:{raw}", call.name)
                        };
                        if let Some(first) = searched.get(&qk).filter(|_| !qk.is_empty()) {
                            // 2026-10-04 V1-07: the same weather / hotel searches ran 3 times in one task
                            skips.push(Some(Skip {
                                first: first.clone(),
                                search: true,
                                typed: false,
                            }));
                            continue;
                        }
                        if !qk.is_empty() {
                            searched.insert(qk, call.id.clone());
                        }
                    }
                    skips.push(None);
                }
                // 2. run them: stretches of independent calls (sub-agents, reads) at the same time, the rest in order
                let shape: Vec<(&str, bool)> = calls
                    .iter()
                    .zip(&skips)
                    .map(|(c, s)| (c.name.as_str(), s.is_none()))
                    .collect();
                let runs = concurrent_runs(&shape);
                let mut i = 0;
                while i < calls.len() {
                    if let Some(run) = runs.iter().find(|r| r.start == i) {
                        let batch = &calls[run.clone()];
                        self.event(
                            task_id,
                            "parallel_calls",
                            json!({"calls": batch.iter().map(|c| json!({"call_id": c.id, "name": c.name})).collect::<Vec<_>>()}),
                        )
                        .await;
                        let base = transcript.clone();
                        let futs = batch.iter().map(|call| {
                            let mut tr = base.clone();
                            let (t, catalog) = (&t, &catalog);
                            async move {
                                let r = self
                                    .exec_call(
                                        t,
                                        call,
                                        &mut tr,
                                        catalog,
                                        &[],
                                        None,
                                        CallMode {
                                            batch: true,
                                            ..CallMode::MAIN
                                        },
                                    )
                                    .await;
                                (r, tr)
                            }
                        });
                        let results = futures::future::join_all(futs).await;
                        // the results go into the transcript in call order, whatever order they finished in
                        let mut suspend = None;
                        for (r, tr) in results {
                            transcript.extend(tr.into_iter().skip(base.len()));
                            match r {
                                Ok(out) => {
                                    consecutive_errors =
                                        if out.ok { 0 } else { consecutive_errors + 1 };
                                    progress = progress || !out.refused;
                                }
                                Err(sp) => suspend = suspend.or(Some(sp)),
                            }
                        }
                        if let Some(sp) = suspend {
                            return self.suspend(task_id, sp, transcript).await;
                        }
                        i = run.end;
                        continue;
                    }
                    let call = &calls[i];
                    if let Some(sk) = &skips[i] {
                        self.skip_duplicate(
                            task_id,
                            call,
                            &sk.first,
                            &mut transcript,
                            sk.search,
                            sk.typed,
                        )
                        .await;
                        i += 1;
                        continue;
                    }
                    match self
                        .exec_call(
                            &t,
                            call,
                            &mut transcript,
                            &catalog,
                            &calls[i + 1..],
                            None,
                            CallMode::MAIN,
                        )
                        .await
                    {
                        Ok(out) => {
                            consecutive_errors = if out.ok { 0 } else { consecutive_errors + 1 };
                            progress = progress || !out.refused;
                        }
                        Err(sp) => return self.suspend(task_id, sp, transcript).await,
                    }
                    i += 1;
                }
                self.store
                    .update_task(task_id, &json!({"transcript": transcript}))?;
                stuck = if progress { 0 } else { stuck + 1 };
                let evs = self.store.events(task_id, 0)?;
                let replans = evs
                    .iter()
                    .filter(|e| get_str(e, "type") == "replanning")
                    .count() as i64;
                let dead = dead_ends_text(&source_failures(&evs), lg);
                if stuck >= 3 || ((stuck != 0 || consecutive_errors >= 3) && replans >= MAX_REPLANS)
                {
                    // still going round in circles after a nudge and a re-plan: stop and answer with what we have
                    gave_up = true;
                    self.event(task_id, "gave_up",
                               json!({"reason": "stuck", "replans": replans, "dead_ends": take_chars(&dead, 500)}))
                        .await;
                } else if stuck == 2 || consecutive_errors >= 3 {
                    self.replan(task_id, &mut transcript, &facts, &history_txt, &dead)
                        .await?;
                    consecutive_errors = 0;
                } else if stuck == 1 {
                    let tail = if dead.is_empty() {
                        String::new()
                    } else {
                        format!("\n{}", dead)
                    };
                    let txt = if lg == "en" {
                        format!("(System) Every call you just made was refused (a repeat, or a site that already failed several times). \
Do not repeat them. Switch to a completely different method or source, or finish the task if you \
already have enough.{tail}")
                    } else {
                        format!("（系统）你刚才的调用全部被拦截了（重复调用，或者这个网站已经多次失败）。不要再重复同样的调用。\
换一个完全不同的方法或来源继续；如果前面已经拿到了足够的信息，就直接完成任务。{tail}")
                    };
                    transcript.push(user_msg(txt));
                }
                continue;
            }
            let mut final_ = resp.content.clone();
            let plan_steps = t
                .get("plan")
                .map(|p| arr(p, "steps").to_vec())
                .unwrap_or_default();
            let open_steps = plan_steps
                .iter()
                .filter(|st| matches!(sget(st, "status").as_str(), "pending" | "running"))
                .count();
            if !final_.is_empty()
                && !cut_nudged
                && !force_final
                && steps < max_steps - 1
                && cut_off_final(&final_, resp.finish_reason.as_str(), open_steps)
            {
                cut_nudged = true;
                self.event(
                    task_id,
                    "cut_off_answer",
                    json!({"text": truncate(&final_, 200)}),
                )
                .await;
                transcript.push(json!({"role": "assistant", "content": final_}));
                transcript.push(user_msg(prompts::l(lg,
                    "（系统）你刚才的回复被截断了，只有半句话。任务还没完成的话就继续用工具做完；已经做完了就把完整的最终回答重新写一遍（包括前面查到的所有结果）。",
                    "(System) Your last reply was cut off mid-sentence. If the task is not finished, keep going with tools; \
if it is, write the complete final answer again (including everything you found).")));
                continue;
            }
            if final_.is_empty() && !nudged {
                nudged = true;
                transcript.push(user_msg(prompts::l(lg,
                    "（系统）请直接用文字写出最终回答（不要调用工具，也不要输出 <tool_call> 之类的标记）。",
                    "(System) Write the final answer now as plain text (no tool calls, no <tool_call> markup).")));
                continue;
            }
            if final_.is_empty() {
                final_ = self
                    .rescue_final(task_id, &goal_text, &transcript, lg)
                    .await;
                if !final_.is_empty() {
                    self.event(
                        task_id,
                        "rescued_final",
                        json!({"chars": final_.chars().count()}),
                    )
                    .await;
                }
            }
            if final_.is_empty() {
                // the model still gave no text: say what was done instead of an empty bubble
                let done: Vec<String> = plan_steps
                    .iter()
                    .filter(|st| sget(st, "status") == "done")
                    .map(|st| {
                        let d = sget(st, "description");
                        if d.is_empty() {
                            super::util::pystr_opt(st.get("id"))
                        } else {
                            d
                        }
                    })
                    .collect();
                final_ = if lg == "en" {
                    format!(
                        "(The task ended but the model wrote no answer.){}",
                        if done.is_empty() {
                            String::new()
                        } else {
                            format!(" Done: {}", done.join("; "))
                        }
                    )
                } else {
                    format!(
                        "（任务已结束，但模型没有返回文字说明。）{}",
                        if done.is_empty() {
                            String::new()
                        } else {
                            format!("已完成：{}", done.join("；"))
                        }
                    )
                };
            }
            if !final_.is_empty()
                && !numbers_checked
                && !force_final
                && !timed_out
                && steps < max_steps - 1
            {
                numbers_checked = true;
                let bad = ungrounded_numbers(&transcript, &final_, &goal_text);
                let codes = ungrounded_codes(&transcript, &final_, &goal_text);
                if self_corrects(&final_) && bad.is_empty() {
                    self.event(task_id, "number_check", json!({"self_correction": true}))
                        .await;
                    transcript.push(json!({"role": "assistant", "content": final_}));
                    transcript.push(user_msg(prompts::l(lg,
                        "（系统）你的回答中途改口了（例如「等一下，这里需要修正」）。用 calculate 把所有数字重新算一遍，然后只写最终正确的版本：不要保留错误的版本，也不要提到这次更正。",
                        "(System) Your answer changes its mind half-way (\"wait, this needs correcting\"). Recompute every figure \
with calculate, then write only the final correct version: drop the wrong one and don't mention the fix.")));
                    continue;
                }
                if !codes.is_empty() && bad.is_empty() {
                    // 2026-10-04 V8-05: a return flight "SQ 637 11:30" that no email mentioned
                    let c10: Vec<String> = codes.iter().take(10).cloned().collect();
                    self.event(task_id, "number_check", json!({"unsupported_codes": c10}))
                        .await;
                    transcript.push(json!({"role": "assistant", "content": final_}));
                    let txt = if lg == "en" {
                        format!("(System) These flight numbers / codes in your answer appear in no tool result: {}. Never invent flights, booking codes or times: remove them or say \"not found\", then write the complete final answer again.", c10.join(", "))
                    } else {
                        format!("（系统）你回答里的这些航班号/编号没有出现在任何工具结果里：{}。不要编造航班、预订号或时间：删掉它们，或者写明「未找到」，然后重新写完整的最终回答。", c10.join("、"))
                    };
                    transcript.push(user_msg(txt));
                    continue;
                }
                if !bad.is_empty() {
                    // 2026-10-03 M1-06: calculate returned the right building blocks, the answer then carried a
                    // hand-made table whose balances came from nowhere (17,016.64 instead of 16,962.71)
                    let b10: Vec<String> = bad.iter().take(10).cloned().collect();
                    self.event(task_id, "number_check", json!({"unsupported": b10}))
                        .await;
                    transcript.push(json!({"role": "assistant", "content": final_}));
                    let txt = if lg == "en" {
                        format!("(System) These figures in your answer do not appear in any tool result: {}. \
Recompute them with calculate (data_query for table data) — if an earlier result looked wrong, fix the \
inputs — then write the complete final answer again using only figures the tools produced. Start \
straight with the content: don't mention this check or say \"here is the complete answer\".", b10.join(", "))
                    } else {
                        format!("（系统）你回答里的这些数字没有出现在任何工具结果里：{}。\
请用 calculate（表格数据用 data_query）把它们重新算一遍——如果之前某个工具结果看起来不对，就改正输入再算——\
然后重新写完整的最终回答，只使用工具算出的数字。回答直接从内容开始，不要提到这次核对或“以下是完整回答”之类的话。", b10.join("、"))
                    };
                    transcript.push(user_msg(txt));
                    continue;
                }
            }
            if !final_.is_empty()
                && !outcome_checked
                && !force_final
                && !timed_out
                && steps < max_steps - 1
            {
                outcome_checked = true;
                let texts = self.inner.lock().tool_texts.get(task_id).cloned();
                let oc = crate::outcome::assess(
                    &self.store.events(task_id, 0)?,
                    &final_,
                    texts.as_deref(),
                    Some(&goal_text),
                );
                let hint = crate::outcome::nudge(&oc, lg != "en");
                if !hint.is_empty() {
                    // 2026-10-06 (roadmap batch 1): "done" needs proof — an order number the confirmation page shows,
                    // or no claim at all
                    self.event(
                        task_id,
                        "outcome_check",
                        json!({"status": oc.get("status"), "missing": oc.get("missing")}),
                    )
                    .await;
                    transcript.push(json!({"role": "assistant", "content": final_}));
                    transcript.push(user_msg(hint));
                    continue;
                }
            }
            final_ = merge_stranded_answer(&transcript, &final_);
            if lg == "en"
                && prompts::cjk_share(&final_) > 0.5
                && !prompts::wants_cjk_output(&goal_text)
            {
                final_ = self.rewrite_in_english(task_id, &transcript, &final_).await;
            }
            let texts = self.inner.lock().tool_texts.get(task_id).cloned();
            let oc = crate::outcome::assess(
                &self.store.events(task_id, 0)?,
                &final_,
                texts.as_deref(),
                Some(&goal_text),
            );
            final_ += &crate::outcome::notice(&oc, lg != "en");
            self.store.update_task(task_id, &json!({"outcome": oc}))?;
            self.event(task_id, "outcome", oc.clone()).await;
            let acts = arr(&oc, "actions");
            if acts.iter().any(|a| {
                a.as_str() == Some("purchase")
                    || (a.is_object() && get_str(a, "kind") == "purchase")
            }) {
                self.ledger_confirm(task_id, &oc, &final_).await;
            }
            transcript.push(json!({"role": "assistant", "content": final_}));
            let mut plan = t.get("plan").cloned().unwrap_or_else(|| json!({}));
            if let Some(sts) = plan.get_mut("steps").and_then(Value::as_array_mut) {
                for st in sts {
                    if matches!(sget(st, "status").as_str(), "pending" | "running") && !force_final
                    {
                        st["status"] = json!("done");
                    }
                }
            }
            self.store.update_task(
                task_id,
                &json!({"transcript": transcript, "result": final_, "plan": plan, "finished_at": now_ts()}),
            )?;
            let en_ui = get_str(&s, "language") == "en";
            let needs_user = (gave_up || force_final) && !timed_out && asks_user(&final_);
            let (mut gave_up_f, mut force_final_f) = (gave_up, force_final);
            if needs_user {
                // stopped because something only the user knows is missing, and the answer asks for it: not a failure
                self.event(
                    task_id,
                    "needs_user",
                    json!({"reason": if gave_up { "gave_up" } else { "step_limit" }}),
                )
                .await;
                self.set_status(task_id, "COMPLETED", json!({})).await;
                gave_up_f = false;
                force_final_f = false;
            } else if timed_out && !gave_up && steps < max_steps {
                let why = if en_ui {
                    format!(
                        "Stopped at the {:.0}-minute time limit; answered with what was found",
                        max_minutes
                    )
                } else {
                    format!("达到 {:.0} 分钟用时上限，按已有信息作答", max_minutes)
                };
                self.set_status(task_id, "FAILED", json!({"error": why}))
                    .await;
            } else if gave_up {
                // stopped retrying on purpose: the answer explains what is missing, but the task is not a full success
                let why = if en_ui {
                    "Stopped retrying: the same sources kept failing, answered with what was available"
                } else {
                    "同样的来源反复失败，已停止重试，按已有信息作答"
                };
                self.set_status(task_id, "FAILED", json!({"error": why}))
                    .await;
            } else if force_final {
                // ran out of steps: this is not a success — say so instead of quietly marking it completed
                let why = if en_ui {
                    format!(
                        "Stopped at the step limit ({} steps) before finishing",
                        max_steps
                    )
                } else {
                    format!("达到步数上限（{} 步），任务没有做完", max_steps)
                };
                self.set_status(task_id, "FAILED", json!({"error": why}))
                    .await;
            } else {
                self.set_status(task_id, "COMPLETED", json!({})).await;
            }
            self.event(task_id, "final", json!({"text": truncate(&final_, 4000)}))
                .await;
            self.store
                .add_msg(&conv_id, "assistant", &final_, task_id, None)?;
            self.publish(json!({"kind": "conv_update", "conv_id": conv_id}))
                .await;
            if let Some(subject) = self.store.subject_by_conv(&conv_id)? {
                let n = crate::library::index_subject(&self.store, &self.workspace, &subject)?;
                let runs = subject
                    .get("runs")
                    .and_then(omuse_common::util::as_i64)
                    .unwrap_or(0)
                    + 1;
                self.store.update_subject(
                    &get_str(&subject, "id"),
                    &json!({"last_task": task_id, "runs": runs,
                            "error": if n > 0 { "" } else { "the task finished without writing the report" }}),
                )?;
                self.publish(json!({"kind": "library_update", "subject_id": subject.get("id")}))
                    .await;
            } else {
                let me = self.arc();
                let (cid, tid) = (conv_id.clone(), task_id.to_string());
                tokio::spawn(async move { me.retitle(&cid, &tid).await });
            }
            if timed_out && !gave_up_f && steps < max_steps {
                self.warn_unattended(
                    &t,
                    (
                        &format!("超过 {:.0} 分钟上限，按已有信息作答", max_minutes),
                        &format!("hit the {:.0}-minute limit", max_minutes),
                    ),
                    &final_,
                )
                .await;
                return Ok(());
            }
            if gave_up_f {
                self.warn_unattended(
                    &t,
                    (
                        "同样的来源反复失败，已停止重试",
                        "kept failing on the same sources and stopped retrying",
                    ),
                    &final_,
                )
                .await;
                return Ok(());
            }
            if force_final_f {
                self.warn_unattended(
                    &t,
                    (
                        &format!("达到 {} 步上限，没有做完", max_steps),
                        &format!("hit the {}-step limit and didn't finish", max_steps),
                    ),
                    &final_,
                )
                .await;
                return Ok(());
            }
            self.store.add_episode(
                task_id,
                &format!(
                    "{} → {}",
                    take_chars(&goal_text, 200),
                    take_chars(&final_, 600)
                ),
            )?;
            if get_str(&t, "source") == "schedule" {
                self.store.db.execute(
                    "UPDATE schedules SET last_task=? WHERE id=?",
                    omuse_common::params![task_id, schedule_id.as_str()],
                )?;
                if let Some(g) = crate::goals::goal_for_task(&self.store, &t)? {
                    let recorded = arr(&g, "progress")
                        .iter()
                        .any(|p| sget(p, "task_id") == task_id);
                    if get_str(&g, "status") == "active" && !recorded {
                        let auto = if en_ui {
                            "(auto-recorded) "
                        } else {
                            "（自动记录 auto）"
                        };
                        let api: &dyn RuntimeApi = self;
                        crate::goals::apply_update(
                            api,
                            &g,
                            "active",
                            &format!("{}{}", auto, truncate(&final_, 500)),
                            task_id,
                        )
                        .await?;
                    }
                }
            }
            if get_truthy(&s, "memory_extraction") && get_str(&t, "source") == "chat" {
                let me = self.arc();
                let tt = t.clone();
                tokio::spawn(async move { me.extract_memory(&tt).await });
            }
            return Ok(());
        }
    }

    /// English mode but the model answered in Chinese (e.g. memory says the user likes Chinese): ask once for English.
    async fn rewrite_in_english(
        &self,
        task_id: &str,
        transcript: &[Value],
        final_: &str,
    ) -> String {
        let mut msgs = transcript.to_vec();
        msgs.push(json!({"role": "assistant", "content": final_}));
        msgs.push(user_msg(
            "(System) Settings → Language is English. Rewrite your complete final answer \
in English now — same content, same structure, no tool calls. Keep \
names, email subjects and quotes in their original language only where needed.",
        ));
        let Ok(r) = self.llm.chat(&msgs, None, &opts("executor", task_id)).await else {
            return final_.to_string();
        };
        let text = r.content.trim().to_string();
        if !text.is_empty() && prompts::cjk_share(&text) < prompts::cjk_share(final_) {
            self.event(task_id, "language_fixed", json!({"from": "zh", "to": "en"}))
                .await;
            return text;
        }
        final_.to_string()
    }

    /// The model returned no text even when asked for the final answer (2026-10-05 S1-04: after 27 steps of price
    /// lookups the task ended with an empty bubble). Ask once more from a short, tool-free context: the goal, what the
    /// agent noted while working and the last results — a long tool-call history primes it to call tools again.
    async fn rescue_final(
        &self,
        task_id: &str,
        goal: &str,
        transcript: &[Value],
        lg: &str,
    ) -> String {
        let notes: Vec<String> = transcript
            .iter()
            .filter(|m| get_str(m, "role") == "assistant")
            .map(|m| content_of(m).trim().to_string())
            .filter(|c| !c.is_empty())
            .collect();
        let results: Vec<String> = transcript
            .iter()
            .filter(|m| get_str(m, "role") == "tool")
            .map(|m| take_chars(&content_of(m), 500).to_string())
            .collect();
        let results = &results[results.len().saturating_sub(10)..];
        if notes.is_empty() && results.is_empty() {
            return String::new();
        }
        let body = format!(
            "Task / 任务:\n{}\n\nNotes written while working / 工作中记下的内容:\n{}\n\nLast tool results / 最近的工具结果:\n{}",
            goal,
            notes[notes.len().saturating_sub(30)..].iter().map(|n| format!("- {}", take_chars(n, 600))).collect::<Vec<_>>().join("\n"),
            results.join("\n---\n")
        );
        let sysmsg = prompts::l(lg,
            "你根据一个任务的工作记录写最终回答。不能调用工具。只用记录里的事实和数字（原样照抄），说清楚找到了什么、哪些没核实以及原因、用户下一步可以怎么做。用用户的语言。",
            "You write the final answer for a task from its working notes. No tools. Use only facts and numbers \
from the notes (copied exactly); say what was found, what is unverified and why, and what the \
user can do next. Answer in the user's language.");
        match self
            .llm
            .chat(
                &[json!({"role": "system", "content": sysmsg}), user_msg(body)],
                None,
                &opts("rescue_final", task_id),
            )
            .await
        {
            Ok(r) => strip_tool_markup(&r.content).trim().to_string(),
            Err(_) => String::new(),
        }
    }

    async fn replan(
        &self,
        task_id: &str,
        transcript: &mut Vec<Value>,
        facts: &[Value],
        history_txt: &str,
        dead_ends: &str,
    ) -> Result<()> {
        let t = self
            .store
            .task(task_id)?
            .ok_or_else(|| anyhow!("task vanished"))?;
        let recent: Vec<&Value> = transcript
            .iter()
            .filter(|m| get_str(m, "role") == "tool")
            .collect();
        let state = recent[recent.len().saturating_sub(4)..]
            .iter()
            .map(|m| truncate(&py_str(m.get("content").unwrap_or(&Value::Null)), 400))
            .collect::<Vec<_>>()
            .join("\n");
        self.event(task_id, "replanning", json!({"reason": "连续失败 consecutive failures", "dead_ends": take_chars(dead_ends, 500)}))
            .await;
        let avoid = if dead_ends.is_empty() {
            "\nThe new plan must take a genuinely different approach from the steps that failed, not retry them.".to_string()
        } else {
            format!("\n{}\nThe new plan must NOT use those sources again; plan a genuinely different approach \
(other websites, other tools, or finishing with the information already gathered).", dead_ends)
        };
        let old_plan = t.get("plan").cloned().unwrap_or_else(|| json!({}));
        let mut plan = self
            .plan(
                &t,
                facts,
                history_txt,
                &format!(
                    "Previous plan: {}\nRecent failures:\n{}{}",
                    pyjson::dumps(&old_plan),
                    state,
                    avoid
                ),
            )
            .await;
        plan["version"] = json!(
            old_plan
                .get("version")
                .and_then(omuse_common::util::as_i64)
                .unwrap_or(1)
                + 1
        );
        self.store.update_task(task_id, &json!({"plan": plan}))?;
        self.event(task_id, "plan", plan).await;
        let base = prompts::l(agent_lang(&self.settings()),
            "（系统）多次失败后已重新规划，请按新计划换一种方法继续，不要重复失败过的调用。",
            "(System) Several steps failed; a new plan was made — try a different approach and do not repeat the calls that failed.");
        transcript.push(user_msg(format!(
            "{}{}",
            base,
            if dead_ends.is_empty() {
                String::new()
            } else {
                format!("\n{}", dead_ends)
            }
        )));
        Ok(())
    }

    /// A call identical to one earlier in the same turn (or, for searches, earlier in the task): answer its tool_call
    /// id without running it again.
    async fn skip_duplicate(
        &self,
        task_id: &str,
        call: &ToolCall,
        first_id: &str,
        transcript: &mut Vec<Value>,
        whole_task: bool,
        one_page: bool,
    ) {
        let msg = if one_page {
            format!("已跳过 — 这一轮已经在网页搜索框里搜过一次（调用 {first_id}），同一个浏览器页面一次只能显示一个结果页，\
后面的搜索会把前面的结果冲掉。先读完这次的结果并记下（update_plan 的 note），下一轮再搜下一个；\
要一次查多个，用 browser_read 读多个搜索网址（每次最多 4 个）。\
 Skipped: this turn already searched the site (call {first_id}); one page shows one result page, so later \
searches would replace it unread. Read and note these results, then search the next item in the next turn — \
or browser_read several search URLs (up to 4) at once.")
        } else if whole_task {
            format!("已跳过 — 这个任务里已经搜索过几乎相同的内容（调用 {first_id}）。请使用那次的结果：读其中的网址，\
或换一个明显不同的关键词 / 来源；如果确实找不到，就在回答里说明“未找到”。\
 Skipped: you already ran this search in this task (call {first_id}). Use those results (read their URLs), \
try a clearly different query or source, or say in the answer that it wasn't found.")
        } else {
            format!(
                "已跳过 — 与本轮前面的调用 {first_id} 完全相同，请直接使用那个结果。\
 Skipped: identical to call {first_id} earlier in this same turn; use that result."
            )
        };
        transcript.push(json!({"role": "tool", "tool_call_id": call.id, "content": msg}));
        self.event(task_id, "tool_call", json!({"call_id": call.id, "name": call.name, "args": preview_args(&call.args), "sub": false})).await;
        self.event(task_id, "tool_result",
                   json!({"call_id": call.id, "name": call.name, "ok": false, "skipped": true, "sub": false, "preview": msg}))
            .await;
    }

    async fn suspend(&self, task_id: &str, sp: Suspend, transcript: Vec<Value>) -> Result<()> {
        let mut waiting = sp.waiting;
        let pending = waiting
            .as_object_mut()
            .and_then(|o| o.remove("_pending"))
            .unwrap_or(Value::Null);
        self.store.update_task(
            task_id,
            &json!({"transcript": transcript, "pending": pending, "waiting": waiting}),
        )?;
        let t = self
            .store
            .task(task_id)?
            .ok_or_else(|| anyhow!("task vanished"))?;
        let cid = get_str(&t, "conv_id");
        // published after the status flips (the messages are stored first, so a poll on the status never misses them)
        let mut notify = Vec::new();
        if sp.status == "WAITING_APPROVAL" {
            let title = waiting
                .get("summary")
                .map(|s| sget(s, "title"))
                .unwrap_or_default();
            self.store.add_msg(
                &cid,
                "system",
                &pyjson::dumps(&json!({"type": "approval", "approval_id": waiting.get("approval_id"), "title": title, "task_id": task_id})),
                task_id,
                None,
            )?;
            notify.push(json!({"kind": "approval_requested", "task_id": task_id, "approval_id": waiting.get("approval_id")}));
        } else if sget(&waiting, "type") == "takeover_requested" {
            let reason = sget(&waiting, "reason");
            self.store.add_msg(
                &cid,
                "system",
                &pyjson::dumps(&json!({"type": "takeover", "reason": reason, "task_id": task_id})),
                task_id,
                None,
            )?;
            // said in the agent's own words too, with a direct link to this task's browser tab: it travels with the
            // message (Telegram, notifications, copied text), unlike the card's button
            let link = self.browser_link(task_id).await;
            let en = agent_lang(&self.settings()) == "en";
            let txt = if en {
                format!("I need you to take over the browser{}.\n\nOpen this task's browser tab: {}\nWhen you are done, click \"Hand back\" there and I will continue.",
                        if reason.is_empty() { String::new() } else { format!(": {}", reason) }, link)
            } else {
                format!("需要你接管浏览器{}。\n\n打开这个任务的浏览器标签页：{}\n完成后在那里点「交还给 Agent」，我会继续。",
                        if reason.is_empty() { String::new() } else { format!("：{}", reason) }, link)
            };
            self.store.add_msg(&cid, "assistant", &txt, task_id, None)?;
            notify
                .push(json!({"kind": "takeover_requested", "task_id": task_id, "reason": reason}));
        }
        self.set_status(task_id, &sp.status, json!({"waiting": waiting}))
            .await;
        self.event(task_id, "waiting", waiting).await;
        for ev in notify {
            self.publish(ev).await;
        }
        self.publish(json!({"kind": "conv_update", "conv_id": cid}))
            .await;
        Ok(())
    }

    // ---------------------------------------------------------------- memory extraction
    async fn extract_memory(&self, t: &Value) {
        let users: Vec<String> = self
            .store
            .msgs(&get_str(t, "conv_id"), 20)
            .unwrap_or_default()
            .iter()
            .filter(|m| get_str(m, "role") == "user")
            .map(content_of)
            .collect();
        let text = users[users.len().saturating_sub(4)..]
            .iter()
            .map(|x| format!("- {}", truncate(x, 600)))
            .collect::<Vec<_>>()
            .join("\n");
        if text.chars().count() < 15 {
            return;
        }
        let o = ChatOpts {
            max_tokens: Some(900),
            temperature: Some(0.1),
            no_think: true,
            ..opts("memory", &get_str(t, "id"))
        };
        let Ok(r) = self
            .llm
            .chat(
                &[user_msg(format!("{}{}", prompts::MEMORY_EXTRACT, text))],
                None,
                &o,
            )
            .await
        else {
            return;
        };
        let data = extract_json(&r.content).unwrap_or_else(|| json!({}));
        self.file_memory_items(&get_str(t, "id"), &data).await;
    }

    /// Write-time sorting: profile details become suggestions the user confirms, one-off details go to recent memory
    /// (30 days), the rest to long-term memory. ID / card numbers and passwords are dropped (they belong in the vault).
    pub async fn file_memory_items(&self, task_id: &str, data: &Value) -> Value {
        let items: Vec<Value> = if data.is_object() {
            let i = arr(data, "items");
            if i.is_empty() {
                arr(data, "facts").to_vec()
            } else {
                i.to_vec()
            }
        } else {
            vec![]
        };
        let (mut added, mut recent, mut suggested): (Vec<String>, Vec<String>, Vec<String>) =
            (vec![], vec![], vec![]);
        for f in items.iter().take(8) {
            if !f.is_object() {
                continue;
            }
            let mut kind = sget(f, "kind");
            if kind.is_empty() {
                kind = sget(f, "category");
            }
            if kind.is_empty() {
                kind = "other".into();
            }
            let kind = kind.to_lowercase();
            let fact = sget(f, "fact").trim().to_string();
            if kind == "profile" {
                let val = sget(f, "value").trim().to_string();
                if let Ok(Some(r)) = self.store.suggest_profile(
                    &sget(f, "field"),
                    &val,
                    take_chars(&fact, 200),
                    &format!("chat:{}", task_id),
                ) {
                    suggested.push(format!("{} → {}", get_str(&r, "key"), get_str(&r, "value")));
                }
                continue;
            }
            if fact.is_empty() || fact.chars().count() > 300 || crate::store::looks_sensitive(&fact)
            {
                continue;
            }
            let tier = if kind == "ephemeral" {
                "recent"
            } else {
                "long"
            };
            let cat = if ["preference", "person", "company", "project", "habit"]
                .contains(&kind.as_str())
            {
                kind.clone()
            } else {
                "other".into()
            };
            let dom = if crate::store::DOMAIN_KEYS.contains(&kind.as_str()) {
                kind.clone()
            } else if kind == "company" || kind == "project" {
                "work".into()
            } else {
                String::new()
            };
            // the user's own words are kept as they are; a habit OMuse inferred waits for the user's OK
            let o = FactOpts {
                source: format!("extracted:{}", task_id),
                confidence: 0.7,
                tier: tier.into(),
                domain: dom,
                status: if kind == "habit" {
                    "pending".into()
                } else {
                    "active".into()
                },
                ..Default::default()
            };
            if let Ok(Some(res)) = self.store.add_fact(&fact, &cat, &sget(f, "entity"), &o) {
                if !get_truthy(&res, "duplicate") {
                    if tier == "recent" {
                        &mut recent
                    } else {
                        &mut added
                    }
                    .push(get_str(&res, "fact"));
                }
            }
        }
        let out = json!({"facts": added, "recent": recent, "profile_suggestions": suggested});
        if !added.is_empty() || !recent.is_empty() || !suggested.is_empty() {
            self.event(task_id, "memory_saved", out.clone()).await;
            self.audit(
                "memory",
                "memory.extract",
                task_id,
                json!({"result": "success", "detail": out}),
            )
            .await;
            self.publish(json!({"kind": "memory_update"})).await;
        }
        out
    }

    /// `rt.ctx_mode[task_id] = mode` / `pop`.
    pub fn set_ctx_mode(&self, task_id: &str, mode: Option<&str>) {
        let mut inner = self.inner.lock();
        match mode {
            Some(m) => {
                inner.ctx_mode.insert(task_id.to_string(), m.to_string());
            }
            None => {
                inner.ctx_mode.remove(task_id);
            }
        }
    }
}

/// Drop the private `_refused` marker of a stored call before parsing it back.
fn strip_private(c: &Value) -> Value {
    let mut c = c.clone();
    if let Some(o) = c.as_object_mut() {
        o.retain(|k, _| !k.starts_with('_'));
        o.entry("args").or_insert_with(|| json!({}));
    }
    c
}

/// Keep the context small: shrink old tool results, keep the last few intact.
///
/// Compresses in batches (above `high` chars, oldest first, down to ~57% of it) rather than a little every step:
/// between batches the conversation only grows at the end, so the model server reuses its cache. When one turn
/// brings back several big results (three 30k-char mail searches), the recent ones are shrunk too — all but the
/// newest first, then the newest — so the request always fits the model's context window.
pub fn compress_with(
    mut transcript: Vec<Value>,
    high: Option<usize>,
    ctx_chars: usize,
) -> Vec<Value> {
    let h = high.filter(|h| *h != 0).unwrap_or(COMPRESS_HIGH);
    let high = h.min(if ctx_chars != 0 {
        ctx_chars
    } else {
        COMPRESS_HIGH
    });
    let low = high * COMPRESS_LOW / COMPRESS_HIGH;
    let mut total = content_chars(&transcript);
    if total < high {
        return transcript;
    }
    let tool_idx: Vec<usize> = transcript
        .iter()
        .enumerate()
        .filter(|(_, m)| get_str(m, "role") == "tool")
        .map(|(i, _)| i)
        .collect();
    let shrink = |transcript: &mut Vec<Value>,
                  idxs: &[usize],
                  keep: usize,
                  goal: usize,
                  total: &mut usize| {
        for &i in idxs {
            if *total <= goal {
                break;
            }
            let c = content_of(&transcript[i]);
            let n = c.chars().count();
            if n > keep + 100 {
                let new = format!(
                    "{}\n…[较早的工具结果已压缩 older result compressed]",
                    take_chars(&c, keep)
                );
                *total -= n - new.chars().count();
                transcript[i]["content"] = json!(new);
            }
        }
    };
    let k = tool_idx.len();
    shrink(
        &mut transcript,
        &tool_idx[..k.saturating_sub(3)],
        500,
        low,
        &mut total,
    );
    if total > high {
        let a = k.saturating_sub(3);
        let b = k.saturating_sub(1);
        shrink(
            &mut transcript,
            &tool_idx[a.min(b)..b],
            2000usize.max(high / 12),
            high,
            &mut total,
        );
    }
    if total > high && k > 0 {
        shrink(
            &mut transcript,
            &tool_idx[k - 1..],
            4000usize.max(high / 3),
            high,
            &mut total,
        );
    }
    transcript
}

// ================================================================ trait impls for the sibling modules
impl RuntimeApi for Runtime {
    fn store(&self) -> &Arc<Store> {
        &self.store
    }
    fn llm(&self) -> &Arc<Llm> {
        &self.llm
    }
    fn config(&self) -> &Arc<Config> {
        &self.cfg
    }
    fn publish(&self, event: Value) -> BoxFuture<'_, ()> {
        Box::pin(Runtime::publish(self, event))
    }
    fn sentinel(
        &self,
        method: &str,
        path: &str,
        payload: Option<Value>,
        timeout: f64,
    ) -> BoxFuture<'_, Result<Value>> {
        let (m, p) = (method.to_string(), path.to_string());
        Box::pin(async move { Runtime::sentinel(self, &m, &p, payload, timeout).await })
    }
    fn audit(&self, actor: &str, action: &str, task_id: &str, kw: Value) -> BoxFuture<'_, ()> {
        let (a, b, c) = (actor.to_string(), action.to_string(), task_id.to_string());
        Box::pin(async move { Runtime::audit(self, &a, &b, &c, kw).await })
    }
    fn submit(
        &self,
        conv_id: &str,
        goal: &str,
        source: &str,
        schedule_id: &str,
    ) -> BoxFuture<'_, Result<Value>> {
        let (a, b, c, d) = (
            conv_id.to_string(),
            goal.to_string(),
            source.to_string(),
            schedule_id.to_string(),
        );
        Box::pin(async move { Runtime::submit(self, &a, &b, &c, &d, None).await })
    }
    fn start_task(&self, task_id: &str) {
        self.start(task_id)
    }
    fn cancel(&self, task_id: &str, reason: &str) -> BoxFuture<'_, ()> {
        let (a, b) = (task_id.to_string(), reason.to_string());
        Box::pin(async move { Runtime::cancel(self, &a, &b).await })
    }
    fn is_running(&self, task_id: &str) -> bool {
        Runtime::is_running(self, task_id)
    }
    fn set_ctx_mode(&self, task_id: &str, mode: Option<&str>) {
        Runtime::set_ctx_mode(self, task_id, mode)
    }
    fn catalog(&self) -> BoxFuture<'_, Value> {
        Box::pin(Runtime::catalog(self, false))
    }
    fn facts_legacy(&self, goal: &str) -> Vec<Value> {
        Runtime::facts_legacy(self, goal)
    }
    fn facts_select(&self, goal: &str) -> Vec<Value> {
        let facts = self.store.facts(500, Some("long")).unwrap_or_default();
        let ents = self.store.entities(None).unwrap_or_default();
        crate::context::select(goal, &facts, Some(&ents), 12, "")
    }
    fn plan(
        &self,
        task: Value,
        facts: Vec<Value>,
        history_txt: &str,
    ) -> BoxFuture<'_, Result<Value>> {
        let h = history_txt.to_string();
        Box::pin(async move { Ok(Runtime::plan(self, &task, &facts, &h, "").await) })
    }
    fn skills(&self) -> Vec<Value> {
        Runtime::skills(self)
    }
    fn memory_tidy_due(&self, now: f64) -> bool {
        crate::memory_tidy::due(&self.store, Some(now)).unwrap_or(false)
    }
    fn memory_tidy_start(&self) {
        let rt: Arc<dyn crate::memory_tidy::TidyRuntime> = self.arc();
        crate::memory_tidy::start(rt, false, "daily", None);
    }
}

impl crate::memory_tidy::TidyRuntime for Runtime {
    fn store(&self) -> &Store {
        &self.store
    }
    fn llm(&self) -> &Llm {
        &self.llm
    }
    fn audit(&self, actor: &str, action: &str, result: &str, detail: Value) -> BoxFuture<'_, ()> {
        let (a, b) = (actor.to_string(), action.to_string());
        let kw = json!({"result": result, "detail": detail});
        Box::pin(async move { Runtime::audit(self, &a, &b, "", kw).await })
    }
    fn publish(&self, event: Value) -> BoxFuture<'_, ()> {
        Box::pin(Runtime::publish(self, event))
    }
    fn sentinel(
        &self,
        method: &str,
        path: &str,
        body: Option<Value>,
        timeout: f64,
    ) -> BoxFuture<'_, Result<Value>> {
        let (m, p) = (method.to_string(), path.to_string());
        Box::pin(async move { Runtime::sentinel(self, &m, &p, body, timeout).await })
    }
}

/// Whether a tool stays available while a skill has turned the browser off: no `browser_*`, no `delegate`.
pub fn browser_policy_allows(name: &str) -> bool {
    !name.starts_with("browser_") && name != "delegate"
}
