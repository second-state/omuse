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
use super::lang::{agent_lang, with_langs};
use super::tools::{concurrent_runs, tool_name, FINISH_TOOLS, LOCAL_TOOLS};
use super::util::{arr, content_of, sget, tool_calls};
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
    /// a second travel search in the turn: what the first one searched (one travel search per turn)
    one_search: Option<(&'static str, &'static str)>,
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
    /// task_id -> sites whose remembered habits were shown
    pub site_noted: HashMap<String, HashSet<String>>,
    /// task_id -> seconds spent queued for a model slot
    pub llm_wait: HashMap<String, f64>,
    /// task_id -> sub-agent lanes handed out so far (`sub1`, `sub2`, …); seeded from the task's events, reserved
    /// atomically so parallel `delegate` calls never share a lane or exceed `SUBAGENT_MAX`
    pub sub_lanes: HashMap<String, usize>,
    /// task_id -> the loaded skill that turned the browser off (front matter `browser: off`, e.g. the city guides):
    /// browser tools are hidden from that task and its sub-agents, and refused if called anyway, until a skill
    /// without that policy is loaded
    pub no_browser: HashMap<String, String>,
    /// task_id -> a loaded skill that needs the browser (no `browser: off`): while one is loaded, a city guide loaded
    /// after it does not turn the browser off (2026-10-30, travel_info_real: getting-around-asia then tokyo)
    pub browser_skill: HashMap<String, String>,
    /// task_id -> the step budget a loaded skill asks for (`steps: N` in its front matter), when above the setting's
    /// (arrival_cards2_real 2026-10-30: Visit Japan Web — a login hand-over and a 20-field form — ran out at 40)
    pub skill_steps: HashMap<String, i64>,
    /// sites whose bot wall was learned as a habit
    pub block_noted: HashSet<String>,
    /// tasks already sent back once for a listings comparison without `ranking` criteria
    pub ranking_asked: HashSet<String>,
    /// tasks whose known page already got its fixed ticks (`guards::apdt_fixed_ticks`)
    pub fixed_ticks: HashSet<String>,
    /// `task|kind|key` of the pages a judgement already looked at (`judgements::PageCheck`): once each
    pub page_checked: HashSet<String>,
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

/// The hosts that failed or were refused at least twice in this task, most failures first (at most 8).
pub(crate) fn dead_ends(fails: &indexmap::IndexMap<String, i64>) -> Vec<(String, i64)> {
    let mut bad: Vec<(String, i64)> = fails
        .iter()
        .filter(|(_, n)| **n >= 2)
        .map(|(h, n)| (h.clone(), *n))
        .collect();
    bad.sort_by_key(|(_, n)| -*n); // stable: ties keep the order they first failed in
    bad.truncate(8);
    bad
}

fn content_chars(msgs: &[Value]) -> usize {
    msgs.iter().map(|m| content_of(m).chars().count()).sum()
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
            let mut llm = match transport {
                Some(tr) => Llm::with_transport(get_settings, Some(on_call), Some(on_retry), tr),
                None => Llm::new(get_settings, Some(on_call), Some(on_retry)),
            };
            // no model call for a stopped task, whoever makes it (the loop, a sub-agent, a judgement, a review)
            let w3 = weak.clone();
            llm.cancelled = Some(Arc::new(move |tid: &str| {
                w3.upgrade().is_some_and(|rt| rt.is_cancelled(tid))
            }));
            let llm = Arc::new(llm);
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
    /// The profile a code machine reads: the stored profile with the connected mail account's address as `mailbox`
    /// (the user's own email where the profile has none).
    fn machine_profile(&self, catalog: &Value) -> Value {
        let mut profile = self.store.profile().unwrap_or_else(|_| json!({}));
        let mailbox = own_mailbox(catalog);
        if !mailbox.is_empty() && profile.is_object() {
            profile["mailbox"] = json!(mailbox);
        }
        profile
    }

    /// The user's today, in the timezone of the settings.
    fn user_today(&self) -> chrono::NaiveDate {
        let tz = get_str(&self.settings(), "timezone");
        tz.parse::<chrono_tz::Tz>()
            .map(|z| chrono::Utc::now().with_timezone(&z).date_naive())
            .unwrap_or_else(|_| chrono::Local::now().date_naive())
    }

    /// A workflow tool's arguments through the JSON checking layer (agent/json_in.rs, docs/json-tools.md): the syntax
    /// was repaired where the call was read; here the schema (types, enums, dates made ISO), then the tool's own field
    /// checks (`booking::check`). Unparseable, a required field missing or invalid, or field checks that fail: the
    /// planner model writes the JSON once more from the user's words and the same checks run on it
    /// (`json_regenerated`). Still invalid: `invalid` for the executor's one more try — and `needs_input` once the
    /// executor already had that try in this request.
    pub(crate) async fn checked_workflow_args(
        &self,
        task_id: &str,
        tool: &str,
        args: &Value,
        transcript: &[Value],
        catalog: &Value,
    ) -> super::workflow::Checked {
        use super::workflow::Checked;
        let params = super::booking::tool_schemas()
            .into_iter()
            .chain(super::news::tool_schemas())
            .chain(super::claims::tool_schemas())
            .chain(super::driver::tool_schemas())
            .chain(super::apdt::tool_schemas())
            .chain(super::trains::tool_schemas())
            .chain(super::flightstatus::tool_schemas())
            .chain(super::entry::tool_schemas())
            .find(|s| tool_name(s) == tool)
            .map(|s| s["function"]["parameters"].clone())
            .unwrap_or_else(|| json!({}));
        // the profile's place fields (codes) for addresses saved without them: the model reads them once, as JSON
        self.fill_places(task_id).await;
        let profile = self.machine_profile(catalog);
        let user_texts = super::driver::user_words(transcript);
        let today = self.user_today();
        let cx = super::booking::Ctx {
            profile: &profile,
            today,
            user_texts: &user_texts,
        };
        let check = |a: &Value| -> (Checked, Vec<String>, Vec<String>) {
            if let Some(raw) = a.get(super::json_in::UNPARSED_KEY) {
                return (
                    Checked::Invalid(vec![format!(
                        "the arguments are not valid JSON: {}",
                        take_chars(&py_str(raw), 200)
                    )]),
                    Vec::new(),
                    Vec::new(),
                );
            }
            let mut a = a.clone();
            let c = super::json_in::conform(&params, &mut a);
            let checked = if super::news::TOOLS.contains(&tool) {
                super::news::check_in(tool, &a, transcript, &profile)
            } else if super::claims::TOOLS.contains(&tool) {
                let cx = super::claims::Ctx {
                    profile: &profile,
                    today,
                    mail_ready: truthy(&catalog["connections"]["gmail"]["ready"]),
                    transcript,
                };
                super::claims::check(tool, &a, &cx)
            } else if super::driver::TOOLS.contains(&tool) {
                let cx = super::driver::Ctx {
                    profile: &profile,
                    today,
                    mail_ready: truthy(&catalog["connections"]["gmail"]["ready"]),
                    transcript,
                };
                super::driver::check(tool, &a, &cx)
            } else if super::apdt::TOOLS.contains(&tool) {
                let cx = super::apdt::Ctx {
                    profile: &profile,
                    today,
                    mail_ready: truthy(&catalog["connections"]["gmail"]["ready"]),
                    transcript,
                };
                super::apdt::check(tool, &a, &cx)
            } else if super::trains::TOOLS.contains(&tool) {
                super::trains::check(tool, &a, today)
            } else if super::flightstatus::TOOLS.contains(&tool) {
                super::flightstatus::check(tool, &a)
            } else if super::entry::TOOLS.contains(&tool) {
                super::entry::check(tool, &a, &profile, today)
            } else {
                super::booking::check(tool, &a, &cx)
            };
            let checked = match checked {
                Checked::Invalid(mut p) if c.required_bad => {
                    p.splice(0..0, c.problems.clone());
                    Checked::Invalid(p)
                }
                _ if c.required_bad => Checked::Invalid(c.problems.clone()),
                Checked::Invalid(mut p) => {
                    p.extend(c.problems.clone());
                    Checked::Invalid(p)
                }
                other => other,
            };
            (checked, c.fixed, c.problems)
        };
        let (mut checked, fixed, dropped) = check(args);
        if !fixed.is_empty() || !dropped.is_empty() {
            self.event(
                task_id,
                "json_repaired",
                json!({"name": tool, "fixed": fixed, "dropped": dropped}),
            )
            .await;
        }
        if let Checked::Invalid(problems) = &checked {
            // the planner writes the JSON once more from the user's words
            let last_user = transcript
                .iter()
                .rposition(|m| get_str(m, "role") == "user")
                .unwrap_or(0);
            let last_answer = transcript[..last_user]
                .iter()
                .rev()
                .find(|m| get_str(m, "role") == "assistant" && !content_of(m).trim().is_empty())
                .map(content_of)
                .unwrap_or_default();
            let (system, user) = super::json_in::regen_prompt(
                tool,
                &params,
                user_texts.last().map(String::as_str).unwrap_or(""),
                &last_answer,
                &pyjson::dumps(args),
                problems,
                &today.format("%Y-%m-%d").to_string(),
            );
            let pm = sget(&self.settings(), "planner_model");
            let o = ChatOpts {
                max_tokens: Some(800),
                temperature: Some(0.0),
                model: if pm.is_empty() { None } else { Some(pm) },
                no_think: true,
                ..opts("planner", task_id)
            };
            let msgs = [json!({"role": "system", "content": system}), user_msg(user)];
            let again = match self.chat_or_cancel(task_id, &msgs, None, &o).await {
                Some(Ok(r)) => super::json_in::extract(&r.content),
                _ => None,
            };
            let mut ok = false;
            if let Some((v, repairs)) = again {
                let (c2, fixed2, _) = check(&v);
                ok = !matches!(c2, Checked::Invalid(_));
                self.event(
                    task_id,
                    "json_regenerated",
                    json!({"name": tool, "problems": problems, "repairs": repairs, "fixed": fixed2,
                           "args": v, "ok": ok}),
                )
                .await;
                if ok
                    || matches!(checked, Checked::Invalid(_))
                        && matches!(c2, Checked::NeedsInput(_))
                {
                    checked = c2;
                }
            } else {
                self.event(
                    task_id,
                    "json_regenerated",
                    json!({"name": tool, "problems": problems, "ok": ok}),
                )
                .await;
            }
        }
        // the executor already had its one more try in this request: what is still wrong is asked of the user
        if let Checked::Invalid(problems) = &checked {
            let last_user = transcript
                .iter()
                .rposition(|m| get_str(m, "role") == "user")
                .unwrap_or(0);
            let tried = transcript[last_user..].iter().any(|m| {
                get_str(m, "role") == "tool"
                    && serde_json::from_str::<Value>(&content_of(m))
                        .is_ok_and(|v| get_str(&v, "status") == "invalid")
            });
            if tried {
                return Checked::NeedsInput(problems.clone());
            }
        }
        checked
    }

    /// The code machine's turn: the next step from the transcript, as a model reply with code calls (or, for the machines
    /// not yet converted, a code-written answer). `None` when a workflow machine is done: its JSON result is appended
    /// as the `T_result` code exchange and the model writes the reply on the next turn (agent/workflow.rs). When a
    /// transitional machine finds no value by its regexes, one extraction call (recorded as an `extract_facts`
    /// result), then the step again.
    async fn drive(
        &self,
        task_id: &str,
        machine: &Machine,
        transcript: &mut Vec<Value>,
        catalog: &Value,
        lg: &str,
    ) -> Option<ChatReply> {
        use super::driver::Step;
        let profile = self.machine_profile(catalog);
        let today = self.user_today();
        let mail_ready = truthy(&catalog["connections"]["gmail"]["ready"]);
        // a schedule's run makes no new schedule (the news machine)
        let scheduled = self
            .store
            .task(task_id)
            .ok()
            .flatten()
            .is_some_and(|t| get_str(&t, "source") == "schedule");
        // the catalog's tool names: the booking machine calls a travel connector's tool when the native one is missing
        let available: Vec<String> = arr(catalog, "tools")
            .iter()
            .map(|t| tool_name(t).to_string())
            .collect();
        // the machine's code calls before this run's tool call (`prefix`): the ids go on from there
        let first = |t: &[Value], from: usize, prefix: &str| {
            t[..from.min(t.len())]
                .iter()
                .flat_map(tool_calls)
                .filter(|tc| get_str(tc, "id").starts_with(prefix))
                .count()
        };
        let next = |t: &[Value]| match machine {
            Machine::Workflow { tool, args, from }
                if super::driver::TOOLS.contains(&tool.as_str()) =>
            {
                let from = (*from).min(t.len());
                let m = super::driver::Machine {
                    profile: &profile,
                    mail_ready,
                    today,
                    first: first(t, from, super::driver::PREFIX),
                };
                super::driver::next(tool, args, &t[from..], &m)
            }
            Machine::Workflow { tool, args, from }
                if super::apdt::TOOLS.contains(&tool.as_str()) =>
            {
                let from = (*from).min(t.len());
                let m = super::apdt::Machine {
                    profile: &profile,
                    mail_ready,
                    today,
                    first: first(t, from, super::apdt::PREFIX),
                };
                super::apdt::next(tool, args, &t[from..], &m)
            }
            Machine::Workflow { tool, args, from }
                if super::news::TOOLS.contains(&tool.as_str()) =>
            {
                let from = (*from).min(t.len());
                let first = t[..from]
                    .iter()
                    .flat_map(tool_calls)
                    .filter(|tc| get_str(tc, "id").starts_with(super::news::PREFIX))
                    .count();
                let m = super::news::Machine {
                    profile: &profile,
                    today,
                    scheduled,
                    zh: lg != "en",
                    first,
                };
                super::news::next(tool, args, &t[from..], &m)
            }
            Machine::Workflow { tool, args, from }
                if super::trains::TOOLS.contains(&tool.as_str()) =>
            {
                let from = (*from).min(t.len());
                let m = super::trains::Machine {
                    today,
                    first: first(t, from, super::trains::PREFIX),
                };
                super::trains::next(tool, args, &t[from..], &m)
            }
            Machine::Workflow { tool, args, from }
                if super::flightstatus::TOOLS.contains(&tool.as_str()) =>
            {
                let from = (*from).min(t.len());
                let m = super::flightstatus::Machine {
                    first: first(t, from, super::flightstatus::PREFIX),
                };
                super::flightstatus::next(tool, args, &t[from..], &m)
            }
            Machine::Workflow { tool, args, from }
                if super::entry::TOOLS.contains(&tool.as_str()) =>
            {
                let from = (*from).min(t.len());
                let m = super::entry::Machine {
                    zh: lg != "en",
                    today,
                };
                super::entry::next(tool, args, &t[from..], &m)
            }
            Machine::Workflow { tool, args, from }
                if super::claims::TOOLS.contains(&tool.as_str()) =>
            {
                let m = super::claims::Machine {
                    profile: &profile,
                    today,
                };
                super::claims::next(tool, args, t, *from, &m)
            }
            Machine::Workflow { tool, args, from } => {
                let from = (*from).min(t.len());
                let first = t[..from]
                    .iter()
                    .flat_map(tool_calls)
                    .filter(|tc| get_str(tc, "id").starts_with(super::booking::PREFIX))
                    .count();
                let m = super::booking::Machine {
                    profile: &profile,
                    mail_ready,
                    available: &available,
                    first,
                };
                super::booking::next(tool, args, &t[from..], &m)
            }
        };
        let step = next(transcript);
        let Machine::Workflow { tool, .. } = machine;
        let card = tool.as_str();
        let (content, tool_calls) = match step {
            Step::Calls(c) => (String::new(), c),
            Step::Done(result) => {
                // the machine's JSON result: the `T_result` code exchange, then the model writes the reply
                let tool = tool.clone();
                self.event(
                    task_id,
                    "workflow_result",
                    json!({"tool": tool, "status": get_str(&result, "status"), "result": result}),
                )
                .await;
                let ex = super::workflow::result_exchange(transcript, &tool, &result);
                transcript.extend(ex);
                return None;
            }
        };
        self.event(
            task_id,
            "driver",
            json!({"card": card, "calls": tool_calls.iter().map(|c| c.name.clone()).collect::<Vec<_>>(), "final": !content.is_empty()}),
        )
        .await;
        Some(ChatReply {
            content,
            reasoning: String::new(),
            tool_calls,
            usage: json!({}),
            finish_reason: json!("stop"),
        })
    }

    /// One reading of the final answer by the model, as the user would read it: `Some(review)` when it does not hold
    /// together (it contradicts itself, stops mid-way, answers another question, or claims the opposite of what it
    /// reports) or breaks the language rule; `None` when it is fine. JSON only, no judgement of style. The facts the
    /// user must read are not the review's: code appends them after it (agent/show.rs).
    async fn review_answer(
        &self,
        task_id: &str,
        request: &str,
        answer: &str,
        lg: &str,
    ) -> Option<Review> {
        if answer.chars().count() < 40 {
            return None;
        }
        let o = ChatOpts {
            max_tokens: Some(120),
            temperature: Some(0.0),
            no_think: true,
            ..opts("classify", task_id)
        };
        let body = format!(
            "The user's request:\n{}\n\nThe reply:\n{}",
            take_chars(request, 2000),
            take_chars(answer, 6000)
        );
        let msgs = [
            json!({"role": "system", "content": prompts::review_system(lg)}),
            user_msg(body),
        ];
        let r = self.llm.chat(&msgs, None, &o).await.ok()?;
        let v = crate::llm::extract_json(&r.content)?;
        let ok = v.get("ok").and_then(Value::as_bool).unwrap_or(true);
        let problem = take_chars(get_str(&v, "problem").trim(), 300).to_string();
        // the language rule (Settings → Language), judged by the model: only an explicit false counts
        let language = v.get("language_ok").and_then(Value::as_bool) == Some(false);
        if (ok || problem.is_empty()) && !language {
            return None;
        }
        Some(Review {
            problem: if ok { String::new() } else { problem },
            language,
        })
    }

    /// The reply again in the agent's language (`lg`), when the final review found it written in another one against
    /// the language rule (the model judges; code never guesses a text's language). Text the user asked for in another
    /// language stays as it is.
    async fn reply_in_language(
        &self,
        task_id: &str,
        request: &str,
        reply: &str,
        lg: &str,
    ) -> String {
        let sysmsg = format!(
            "Rewrite the reply to the user in {}. No tools. Keep its content, structure, markdown, links, numbers, names, \
email subjects and quotes exactly; text the user asked for in another language (a translation, a poem, story, post or \
message to use as written) stays in that language. Output only the rewritten reply.",
            prompts::lang_name(lg)
        );
        let body = format!(
            "The user's request:\n{}\n\nThe reply:\n{}",
            take_chars(request, 2000),
            take_chars(reply, 6000)
        );
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

    /// The reply again, with the problem the review named fixed (the model writes it; the machine's result JSON is the
    /// only source).
    async fn reply_fixed(
        &self,
        task_id: &str,
        request: &str,
        reply: &str,
        result: &Value,
        review: &Review,
        lg: &str,
    ) -> String {
        let sysmsg = prompts::l(lg,
            "你根据工具的 JSON 结果改写给用户的回复。不能调用工具。保留原回复的内容和语言，改正指出的问题（数字、编号、链接原样照抄）。只输出改写后的回复。",
            "You rewrite a reply to the user from a tool's JSON result. No tools. Keep the reply's content and language and fix the problem named (numbers, references and links copied exactly from the result). Output only the rewritten reply.");
        let body = format!(
            "The user's request:\n{}\n\nThe tool's result (JSON):\n{}\n\nThe reply:\n{}\n\nProblem: {}",
            take_chars(request, 2000),
            take_chars(&result.to_string(), 8000),
            take_chars(reply, 6000),
            review.problem
        );
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

    pub async fn chat_or_cancel(
        &self,
        task_id: &str,
        msgs: &[Value],
        tools: Option<&[Value]>,
        o: &ChatOpts,
    ) -> Option<Result<ChatReply, LlmError>> {
        let r = self.chat_cancellable(task_id, msgs, tools, o).await?;
        // an optional model (the form-filling or planner model) that stays unavailable after its retries: the call goes
        // to the LLM instead of failing the task (2026-10-30: xAI "at capacity" ended form tasks running on grok-4.7)
        if let (Err(e), Some(model)) = (&r, o.model.as_ref()) {
            if !e.is_context() && unavailable(e) {
                self.event(
                    task_id,
                    "model_fallback",
                    json!({"model": model, "error": take_chars(&e.to_string(), 200)}),
                )
                .await;
                let fallback = ChatOpts {
                    model: None,
                    ..o.clone()
                };
                return self.chat_cancellable(task_id, msgs, tools, &fallback).await;
            }
        }
        Some(r)
    }

    /// Sleep `dur`, waking at once when the task is cancelled: `true` when it was (a code step's wait — the email
    /// verification step's poll for the mail, agent/email_code.rs — ends then).
    pub(crate) async fn sleep_or_cancel(&self, task_id: &str, dur: Duration) -> bool {
        let notified = self.cancel_notify.notified();
        tokio::pin!(notified);
        notified.as_mut().enable();
        if self.cancel_flags.lock().contains(task_id) {
            return true;
        }
        tokio::select! {
            _ = tokio::time::sleep(dur) => self.cancel_flags.lock().contains(task_id),
            _ = &mut notified => self.cancel_flags.lock().contains(task_id),
        }
    }

    async fn chat_cancellable(
        &self,
        task_id: &str,
        msgs: &[Value],
        tools: Option<&[Value]>,
        o: &ChatOpts,
    ) -> Option<Result<ChatReply, LlmError>> {
        self.until_cancelled(task_id, self.llm.chat(msgs, tools, o))
            .await
    }

    /// Whether the user stopped this task (`POST /api/tasks/{id}/cancel`). Checked before every model call, tool
    /// call and code-machine step; sub-agents run under their parent's id, so they stop with it.
    pub fn is_cancelled(&self, task_id: &str) -> bool {
        !task_id.is_empty() && self.cancel_flags.lock().contains(task_id)
    }

    /// `fut`, abandoned (dropped, `None`) as soon as the task is stopped: a model call queued for a slot or sleeping on
    /// a Retry-After, a tool call waiting on Sentinel (a page load, a video job being polled, a phone call), any wait.
    pub(crate) async fn until_cancelled<F: std::future::Future>(
        &self,
        task_id: &str,
        fut: F,
    ) -> Option<F::Output> {
        tokio::pin!(fut);
        loop {
            let notified = self.cancel_notify.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if self.is_cancelled(task_id) {
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
        let language = if get_str(&s, "language") == "en" {
            "en"
        } else {
            "zh"
        };
        // Reply language = match: the model writes the title in the latest message's language and names it in
        // `lang` (JSON, checked against the list); otherwise the setting fixes it
        let fixed = (get_str(&s, "reply_language") != "match").then_some(language);
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
            max_tokens: Some(80),
            temperature: Some(0.0),
            no_think: true,
            ..opts("title", task_id)
        };
        let r = self
            .llm
            .chat(
                &[
                    json!({"role": "system", "content": prompts::title_prompt(fixed)}),
                    user_msg(text),
                ],
                None,
                &o,
            )
            .await
            .map_err(|e| anyhow!("{}: {}", e.name(), e))?;
        let (title, _lang) = prompts::parse_title(&r.content, language, fixed);
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
        // a stopped task stays stopped: a step that was already under way when the user pressed Stop (a suspend, the
        // final answer, a failure) never turns CANCELLED back into something else
        if status != "CANCELLED"
            && self
                .store
                .task(task_id)
                .ok()
                .flatten()
                .is_some_and(|t| get_str(&t, "status") == "CANCELLED")
        {
            println!("[status] {task_id}: stays CANCELLED (not {status})");
            return;
        }
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
            // a main-bot task may be waiting for this one (crate::bots)
            let me = self.arc();
            let tid = task_id.to_string();
            tokio::spawn(async move { me.handoff_done(&tid).await });
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
        let (mut tl, fl) = (String::new(), String::new());
        if get_str(&s0, "reply_language") == "match" {
            // the model reads the request's language (a JSON choice); this task only (and what it spawns)
            tl = self.request_lang(&task_id, &goal).await.to_string();
        }
        let res = with_langs(tl, fl, self.run(&task_id)).await;
        if let (Err(e), true) = (&res, self.is_cancelled(&task_id)) {
            // stopped by the user meanwhile: what broke off is not a failure (no FAILED, no error message)
            println!("[runtime] task {} stopped: {:#}", task_id, e);
        } else if let Err(e) = res {
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
            inner.no_browser.remove(&task_id);
            inner.browser_skill.remove(&task_id);
            inner.skill_steps.remove(&task_id);
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

    /// Cancel (stop) a task: the flag is seen before its next model call, tool call or code-machine step, and wakes
    /// whatever it is waiting on (`until_cancelled`); the status is CANCELLED at once and stays so. Its sub-agents run
    /// under its id and stop with it; the bot task it handed over to is cancelled too. A pending approval / takeover
    /// expires.
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
        // the sub-agents still working (started, not done): they see the parent's flag and stop at their next step
        let evs = self.store.events(task_id, 0).unwrap_or_default();
        let count = |ty: &str| evs.iter().filter(|e| get_str(e, "type") == ty).count();
        let subagents = count("subagent_start").saturating_sub(count("subagent_done"));
        let mut data = json!({});
        if !reason.is_empty() {
            data["reason"] = json!(reason);
        }
        if subagents > 0 {
            data["subagents"] = json!(subagents);
        }
        self.event(task_id, "cancelled", data).await;
        self.cancel_handoff(
            task_id,
            &t.get("waiting").cloned().unwrap_or(Value::Null),
            reason,
        )
        .await;
        {
            // don't leave an orphan approval behind: approving it later would run an action for a dead task
            // (Sentinel also ends a takeover of this task's page). Also for a running task: a call dropped mid-way may
            // have just asked for one
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
                // a fact, not an instruction: the page after the hand-back is read by code (`needs_handback_snapshot`)
                let text = if lg == "en" {
                    format!("{TAKEOVER_DONE_EN}: the user handed browser control back.")
                } else {
                    format!("{TAKEOVER_DONE_MARK}并交还浏览器控制权。")
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
            if let Some(h) = m.get("meta").and_then(|x| x.get("handoff")) {
                // a bot's answer shown in the main conversation: say where it came from, for a follow-up
                c += &format!(
                    "\n[answered by the {} bot in conversation {}]",
                    sget(h, "bot"),
                    sget(h, "conv_id")
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

    /// The profile's standing preferences for the executor prompt, as (label, value): the fixed
    /// `preferred_airline` / `preferred_hotel_brand` / `preferred_car_rental` fields, then the custom fields the user
    /// marked standing on the Memory page (`Store::profile_standing`: a second airline, a loyalty programme, a seat
    /// wish…). Empty values are skipped; nothing else of the profile is ever put in the prompt.
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
        for k in self.store.profile_standing().unwrap_or_default() {
            let (Some(label), Some(v)) = (
                k.strip_prefix("custom:"),
                map.get(&k).and_then(Value::as_str),
            ) else {
                continue;
            };
            if !v.trim().is_empty() {
                out.push((label.to_string(), v.trim().to_string()));
            }
        }
        out
    }

    /// Long-term facts for this task: the model reads the request against what memory holds (JSON choices), and
    /// code selects on the facts' structured fields and the task's data (bot, skills, tools) — `context::facts_for_task`.
    pub async fn facts_for(&self, t: &Value, history_txt: &str) -> Vec<Value> {
        let users: Vec<&str> = history_txt
            .lines()
            .filter_map(|ln| ln.strip_prefix("user: "))
            .collect();
        let users = users[users.len().saturating_sub(3)..].join("\n");
        let task = crate::context::task_data(&self.bot_of(t).id, arr(t, "transcript"));
        crate::context::facts_for_task(
            &self.store,
            &self.llm,
            &get_str(t, "id"),
            &get_str(t, "goal"),
            &users,
            &task,
        )
        .await
        .0
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
        // recorded before (in an earlier run): a fact of kind site_block with this site as its entity
        if self
            .store
            .facts(5000, None)
            .unwrap_or_default()
            .iter()
            .any(|f| get_str(f, "kind") == "site_block" && get_str(f, "entity") == site)
        {
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
            kind: "site_block".into(),
            scope: "topic".into(),
            task_id: task_id.to_string(),
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
            &self.store.entities(None).unwrap_or_default(),
        );
        if found.is_empty() {
            return String::new();
        }
        let ids: Vec<String> = found.iter().map(|f| get_str(f, "id")).collect();
        let _ = self.store.mark_used(&ids);
        let _ = self.store.note_task_memory(task_id, &[], &[], &ids);
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
        seen: Vec<String>,
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
                    // the same links as the final answer will have: repaired against the results, a half-written one
                    // held back until it is whole
                    let text =
                        crate::links::repair_answer(crate::links::trim_open_link(&d.text), &seen);
                    rt.publish(
                        json!({"kind": "answer_delta", "task_id": tid, "conv_id": cid,
                                      "text": text, "calls": d.tool_calls}),
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
        let facts = self.facts_for(&t, &history_txt).await;
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

    /// The tool list for this run: Sentinel's catalog + the local tools, minus the schedule/goal-only ones. Google
    /// first: with a SerpApi key (the catalog's `connections.travel.google`) the built-in travel searches stay. Without
    /// one they would fall back (flights to Kiwi.com, hotels to a web search), so a travel search the user's own
    /// connector also offers (`mcp_<id>__hotels_search` beside `hotels_search`) is then the connector's alone: with one
    /// travel search a turn, the model must not spend it on the weaker built-in one.
    pub fn tools_for(
        catalog: &Value,
        allow: Option<&[&str]>,
        schedule: bool,
        goal: bool,
    ) -> Vec<Value> {
        let mut tools: Vec<Value> = arr(catalog, "tools").to_vec();
        tools.extend(LOCAL_TOOLS.iter().cloned());
        let tools: Vec<Value> = tools
            .into_iter()
            .filter(|t| {
                let n = tool_name(t);
                (schedule || (n != "schedule_state_get" && n != "schedule_state_set"))
                    && (goal || n != "goal_update")
                    // run by the form functions (crate::forms) with a plan computed in code, never by the model
                    && n != "browser_fill_form"
                    // read by the code machines with their fixed selectors (a known page's elements)
                    && n != "browser_read_elements"
                    && allow.is_none_or(|a| a.contains(&n))
            })
            .collect();
        let connected: HashSet<&str> = tools
            .iter()
            .filter_map(|t| tool_name(t).strip_prefix("mcp_")?.split_once("__"))
            .map(|(_, base)| base)
            .collect();
        let google = truthy(&catalog["connections"]["travel"]["google"]);
        let shadowed = |n: &str| !google && travel_search(n).is_some() && connected.contains(n);
        tools
            .iter()
            .filter(|t| !shadowed(tool_name(t)))
            .cloned()
            .collect()
    }

    /// Keep the last steps for delivering what the user asked for.
    ///
    /// The budget is a tool-list limit, never a message to the model (the hard rule: no natural language in the
    /// decision flow): with BUDGET_RESERVE steps left, or past TIME_NUDGE of the minute budget (`late`), only the
    /// finishing tools remain (the deliverable, send_file, the answer); after RESEARCH_NUDGE_PAGES + 3 page visits the
    /// page-reading tools are withdrawn, after WEB_NUDGE web searches (`web_calls`) the search tools.
    pub fn budget(
        transcript: &[Value],
        tools: Vec<Value>,
        remaining: i64,
        web_calls: i64,
        late: bool,
    ) -> Vec<Value> {
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
        const GATHER: [&str; 6] = [
            "browser_navigate",
            "browser_read",
            "web_search",
            "news_search",
            "browser_news_search",
            "browser_click",
        ];
        if remaining <= BUDGET_RESERVE || late {
            return tools
                .into_iter()
                .filter(|t| FINISH_TOOLS.contains(&tool_name(t)))
                .collect();
        }
        if pages >= RESEARCH_NUDGE_PAGES + 3 {
            return tools
                .into_iter()
                .filter(|t| !GATHER.contains(&tool_name(t)))
                .collect();
        }
        if web_calls >= WEB_NUDGE {
            return tools
                .into_iter()
                .filter(|t| {
                    !matches!(
                        tool_name(t),
                        "web_search" | "news_search" | "browser_news_search"
                    )
                })
                .collect();
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
        let facts = self.facts_for(&t, &history_txt).await;
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
        let bot = self.bot_of(&t);
        let bot_skills = self.bot_skills(bot);
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
        extra += &super::bots::bot_prompt(bot, lg0 == "en");
        // the main bot: the planner picks the bot before the first turn (agent/bots.rs, `route`)
        let routing = bot.is_main()
            && transcript.is_empty()
            && !t.get("pending").is_some_and(truthy)
            && get_str(&t, "source") == "chat"
            && goal.is_none();
        let route_history = if routing { history.clone() } else { Vec::new() };
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

        let mut routed_main = false;
        if routing {
            match self.route(&t, &route_history).await {
                super::bots::Route::Bot(args) => {
                    let call = ToolCall {
                        id: "route_1".into(),
                        name: "ask_bot".into(),
                        args: args.clone(),
                    };
                    self.event(
                        task_id,
                        "tool_call",
                        json!({"call_id": call.id, "name": "ask_bot", "args": args, "sub": false, "routed": true}),
                    )
                    .await;
                    match self.ask_bot(&t, &args, &call, &[]).await {
                        Ok(sp) => {
                            transcript.push(assistant_with_calls("", std::slice::from_ref(&call)));
                            return self.suspend(task_id, sp, transcript).await;
                        }
                        Err(e) => {
                            self.event(task_id, "route_error", json!({"message": e}))
                                .await;
                        }
                    }
                }
                super::bots::Route::Main => {
                    routed_main = true;
                    extra += if lg0 == "en" {
                        "\n## Routing\nThe planner checked this request against the bots above: it is yours. Answer it \
yourself; ask_bot is not available for it."
                    } else {
                        "\n## 分派\n规划模型已经对照上面的专门助手看过这个请求：它归你。你自己回答；这次不能用 ask_bot。"
                    };
                }
                super::bots::Route::Unknown => {}
            }
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
            if calls.first().is_some_and(|c| c.name == "ask_bot") {
                let waiting = self
                    .store
                    .events(task_id, 0)?
                    .iter()
                    .rev()
                    .find(|e| get_str(e, "type") == "handoff")
                    .and_then(|e| e.get("data").cloned())
                    .unwrap_or(Value::Null);
                let result = resolved.get("result").cloned().unwrap_or(Value::Null);
                return self.finish_handoff(&t, &waiting, &result).await;
            }
            if let Some(first) = calls.first() {
                let result = resolved
                    .get("result")
                    .filter(|r| truthy(r))
                    .cloned()
                    .unwrap_or_else(|| json!({}));
                let content = self.format_external(&first.name, &result);
                transcript
                    .push(json!({"role": "tool", "tool_call_id": first.id, "content": content}));
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
                    if self.is_cancelled(task_id) {
                        return self.stopped(task_id, &transcript);
                    }
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
        let mut max_steps = s
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
        let mut cut_continued = false;
        let mut auto_handed = false;
        let mut answer_reviewed = false;
        let mut handback_snapped = false;
        // the compulsory workflow call (agent/required.rs): forced tries so far, and what is still owed at the end
        let mut forced_tries = 0;
        let mut unmet: Option<super::required::Need> = None;
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
            // the system prompt stays identical for the whole run (the plan and the dead ends go in the live state,
            // one JSON message at the end of the request: `prompts::live_state`)
            transcript[0] = json!({"role": "system", "content": prompts::executor_system(&prompts::ExecutorPrompt {
                user_name: get_str(&s, "user_name"),
                tz: tz.clone(),
                connections: catalog.get("connections").cloned().unwrap_or_else(|| json!({})),
                plan: None,
                facts: facts.clone(),
                prefs: self.standing_prefs(),
                skills: bot_skills.clone(),
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
            // the web searches so far: past WEB_NUDGE the search tools are withdrawn (a tool-list limit, never a
            // message to the model; `guards::web_budget_guard` refuses a search past it in the same turn)
            let web_calls = super::guards::web_searches(&transcript);
            let timed_out = minutes >= max_minutes;
            if let Some(n) = self.inner.lock().skill_steps.get(task_id) {
                max_steps = max_steps.max(*n);
            }
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
                // the page input tools and form_fill only while browser-use is loaded (forms are code's, fixed ids)
                let input_ok = super::guards::browser_use_active(&transcript);
                let tl: Vec<Value> = tl
                    .into_iter()
                    .filter(|x| {
                        let n = tool_name(x);
                        input_ok
                            || !(super::guards::MODEL_INPUT_TOOLS.contains(&n) || n == "form_fill")
                    })
                    .collect();
                // the flight and hotel searches are the booking machine's where `travel_search` runs them: not offered
                let workflow_bot = crate::bots::has_tool(bot, "travel_search");
                let tl: Vec<Value> = tl
                    .into_iter()
                    .filter(|x| !(workflow_bot && super::guards::machine_only(tool_name(x))))
                    .collect();
                // the reply to a machine's result: the workflow tools are not offered on that turn
                let reply_turn = super::workflow::reply_turn(&transcript);
                let tl: Vec<Value> = tl
                    .into_iter()
                    .filter(|x| {
                        !(reply_turn
                            && (super::booking::TOOLS.contains(&tool_name(x))
                                || super::claims::TOOLS.contains(&tool_name(x))
                                || super::driver::TOOLS.contains(&tool_name(x))
                                || super::apdt::TOOLS.contains(&tool_name(x))
                                || super::trains::TOOLS.contains(&tool_name(x))
                                || super::flightstatus::TOOLS.contains(&tool_name(x))
                                || super::entry::TOOLS.contains(&tool_name(x))))
                            && super::news::offered(&transcript, tool_name(x))
                    })
                    .collect();
                // ask_bot is the main bot's; a tool a bot file claims (the travel searches) only its bots'
                let tl: Vec<Value> = tl
                    .into_iter()
                    .filter(|x| {
                        let n = tool_name(x);
                        // the planner kept the request in Main: no hand-over from the executor either
                        (n != "ask_bot" || (bot.is_main() && !routed_main))
                            && crate::bots::has_tool(bot, n)
                    })
                    .collect();
                let tl = Self::budget(
                    &transcript,
                    tl,
                    max_steps - steps,
                    web_calls,
                    minutes >= max_minutes * TIME_NUDGE,
                );
                tools = Some(if lg == "en" {
                    prompts::strip_tools_en(&tl)
                } else {
                    tl
                });
            }
            // the budget is spent (gave_up / timed_out / force_final): the model gets no tools, nothing is said to it
            self.event(task_id, "thinking", json!({"step": steps + 1}))
                .await;
            let mut msgs = transcript.clone();
            if !force_final {
                // the live state as data at the end (not stored): the plan, the next parallel stage, the dead ends
                let dead = dead_ends(&source_failures(&self.store.events(task_id, 0)?));
                if let Some(state) = prompts::live_state(t.get("plan"), &tz, lg, minutes, &dead) {
                    msgs.push(user_msg(state.to_string()));
                }
            }
            // a known-form skill (the arrival cards with a form function): code decides the next step, no model call
            // (and the claims and the FWD purchase: `agent/claims.rs`)
            let machine = super::workflow::active(&transcript, &super::booking::TOOLS)
                .or_else(|| super::workflow::active(&transcript, &super::news::TOOLS))
                .or_else(|| super::workflow::active(&transcript, &super::claims::TOOLS))
                .or_else(|| super::workflow::active(&transcript, &super::driver::TOOLS))
                .or_else(|| super::workflow::active(&transcript, &super::apdt::TOOLS))
                .or_else(|| super::workflow::active(&transcript, &super::trains::TOOLS))
                .or_else(|| super::workflow::active(&transcript, &super::flightstatus::TOOLS))
                .or_else(|| super::workflow::active(&transcript, &super::entry::TOOLS))
                .map(|(from, tool, args)| Machine::Workflow { tool, args, from });
            let driven = match &machine {
                Some(_) if self.is_cancelled(task_id) => {
                    return self.stopped(task_id, &transcript);
                }
                Some(m) if !force_final => {
                    match self.drive(task_id, m, &mut transcript, &catalog, lg).await {
                        Some(r) => Some(r),
                        None => {
                            // the machine is done (its `T_result` is in the transcript). A page handed to browser-use:
                            // code loads the skill for the model, which fills that one page
                            if let Some(res) = super::workflow::latest_result(&transcript)
                                .filter(|r| get_str(r, "status") == "browser_use")
                            {
                                let tc = ToolCall {
                                    id: format!("{}skill_{steps}", super::booking::PREFIX),
                                    name: "load_skill".into(),
                                    args: json!({"name": super::booking::BROWSER_USE}),
                                };
                                self.event(
                                    task_id,
                                    "browser_use_handoff",
                                    json!({"url": get_str(&res, "url")}),
                                )
                                .await;
                                transcript
                                    .push(assistant_with_calls("", std::slice::from_ref(&tc)));
                                if let Err(sp) = self
                                    .exec_call(
                                        &t,
                                        &tc,
                                        &mut transcript,
                                        &catalog,
                                        &[],
                                        None,
                                        CallMode::MAIN,
                                    )
                                    .await
                                {
                                    return self.suspend(task_id, sp, transcript).await;
                                }
                            }
                            self.store
                                .update_task(task_id, &json!({"transcript": transcript}))?;
                            continue;
                        }
                    }
                }
                _ => None,
            };
            let called = if let Some(r) = driven {
                Some(Ok(r))
            } else {
                let (exec_opts, live) =
                    self.executor_opts(task_id, &conv_id, &s, crate::links::seen(&transcript));
                let called = self
                    .chat_or_cancel(task_id, &msgs, tools.as_deref(), &exec_opts)
                    .await;
                drop(exec_opts); // closes the live-text channel …
                if let Some(live) = live {
                    let _ = live.await; // … and the last text is out before the reply is acted on
                }
                called
            };
            if self.is_cancelled(task_id) {
                // stopped while the model wrote this turn: nothing of it is acted on
                return self.stopped(task_id, &transcript);
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
            let mut calls: Vec<ToolCall> = if force_final {
                vec![]
            } else {
                resp.tool_calls.clone()
            };
            let mut turn_text = resp.content.clone();
            // the compulsory workflow call (agent/required.rs): a loaded skill that asks through a workflow tool gets
            // that call before a final answer — the answer without it is not taken; the model is asked once more with
            // the provider's forced tool choice, and its call goes through the JSON layer like any workflow call
            if calls.is_empty() && !force_final && unmet.is_none() {
                if let Some(need) = super::required::pending(&transcript) {
                    let offered: Vec<Value> = tools
                        .as_deref()
                        .unwrap_or(&[])
                        .iter()
                        .filter(|x| need.tools.contains(&tool_name(x)))
                        .cloned()
                        .collect();
                    let names: Vec<&str> = offered.iter().map(tool_name).collect();
                    if offered.is_empty() || forced_tries >= super::required::MAX_FORCED {
                        self.event(
                            task_id,
                            "workflow_required_unmet",
                            json!({"skills": need.skills, "tools": need.tools,
                                   "reason": if offered.is_empty() { "not_offered" } else { "tries" }}),
                        )
                        .await;
                        unmet = Some(need);
                    } else {
                        forced_tries += 1;
                        let choice = super::required::tool_choice(&names);
                        self.event(
                            task_id,
                            "workflow_required",
                            json!({"skills": need.skills, "tools": names, "tool_choice": choice,
                                   "try": forced_tries}),
                        )
                        .await;
                        let o = ChatOpts {
                            tool_choice: Some(choice),
                            ..opts("executor", task_id)
                        };
                        let forced = self
                            .chat_or_cancel(task_id, &msgs, Some(&offered), &o)
                            .await;
                        if self.is_cancelled(task_id) {
                            return self.stopped(task_id, &transcript);
                        }
                        let got: Vec<ToolCall> = match forced {
                            None => return Ok(()),
                            Some(Ok(r)) => r
                                .tool_calls
                                .into_iter()
                                .filter(|c| need.tools.contains(&c.name.as_str()))
                                .collect(),
                            Some(Err(e)) => {
                                self.event(
                                    task_id,
                                    "workflow_required_error",
                                    json!({"message": truncate(&e.to_string(), 300)}),
                                )
                                .await;
                                Vec::new()
                            }
                        };
                        if got.is_empty() {
                            self.event(
                                task_id,
                                "workflow_required_unmet",
                                json!({"skills": need.skills, "tools": need.tools, "reason": "no_call"}),
                            )
                            .await;
                            unmet = Some(need);
                        } else {
                            // the answer without the call is dropped: the forced call is this turn
                            calls = got;
                            turn_text = String::new();
                        }
                    }
                }
            }
            let mut asked: Option<String> = None;
            if !calls.is_empty() {
                let base_len = transcript.len();
                transcript.push(assistant_with_calls(&turn_text, &calls));
                if !turn_text.is_empty() {
                    self.event(
                        task_id,
                        "message",
                        json!({"text": truncate(&turn_text, 2000)}),
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
                let mut travel_this_turn: Option<(String, (&'static str, &'static str))> = None;
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
                            one_search: None,
                        }));
                        continue;
                    }
                    seen_sigs.insert(sig, call.id.clone());
                    if let Some(kind) = super::guards::travel_search(&call.name) {
                        // ONE travel search per turn — flights, hotels, cars and places together: a range of dates,
                        // several airports, airlines or brands all go in that one call; the rest are not run
                        if let Some((first, what)) = &travel_this_turn {
                            skips.push(Some(Skip {
                                first: first.clone(),
                                search: false,
                                typed: false,
                                one_search: Some(*what),
                            }));
                            continue;
                        }
                        travel_this_turn = Some((call.id.clone(), kind));
                    }
                    if call.name == "browser_type" && get_truthy(&call.args, "submit") {
                        if !typed_this_turn.is_empty() {
                            // 2026-10-04 V3-03: 12 searches typed into one shop's search box in a single turn — only the
                            // last results page existed, the rest were never read, and the list was searched 3x over
                            skips.push(Some(Skip {
                                first: typed_this_turn.clone(),
                                search: false,
                                typed: true,
                                one_search: None,
                            }));
                            continue;
                        }
                        typed_this_turn = call.id.clone();
                    }
                    if call.name == "web_search"
                        || call.name == "news_search"
                        || call.name == "browser_news_search"
                    {
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
                                one_search: None,
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
                    // stopped between two calls of this turn (or two code calls of a machine's step): the rest never run
                    if self.is_cancelled(task_id) {
                        return self.stopped(task_id, &transcript);
                    }
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
                        self.skip_duplicate(task_id, call, sk, &mut transcript)
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
                if self.is_cancelled(task_id) {
                    return self.stopped(task_id, &transcript);
                }
                self.store
                    .update_task(task_id, &json!({"transcript": transcript}))?;
                stuck = if progress { 0 } else { stuck + 1 };
                // `ask_user`: the question is the reply, and the run waits for the user's next message
                if let Some((_, q)) = super::guards::asked_user(&transcript[base_len..]) {
                    asked = Some(q);
                } else {
                    let evs = self.store.events(task_id, 0)?;
                    let replans = evs
                        .iter()
                        .filter(|e| get_str(e, "type") == "replanning")
                        .count() as i64;
                    let dead = dead_ends_text(&source_failures(&evs), lg);
                    if stuck >= 3
                        || ((stuck != 0 || consecutive_errors >= 3) && replans >= MAX_REPLANS)
                    {
                        // still going round in circles after a nudge and a re-plan: stop and answer with what we have
                        gave_up = true;
                        self.event(task_id, "gave_up",
                               json!({"reason": "stuck", "replans": replans, "dead_ends": take_chars(&dead, 500)}))
                        .await;
                    } else if stuck == 2 || consecutive_errors >= 3 {
                        self.replan(task_id, &transcript, &facts, &history_txt, &dead)
                            .await?;
                        consecutive_errors = 0;
                    }
                    continue;
                }
            }
            // the model's markup never reaches the user: stripped in code (an empty remainder is rescued below)
            let mut final_ = asked
                .clone()
                .unwrap_or_else(|| strip_tool_markup(&resp.content).trim().to_string());
            let plan_steps = t
                .get("plan")
                .map(|p| arr(p, "steps").to_vec())
                .unwrap_or_default();
            if !final_.is_empty()
                && asked.is_none()
                && !cut_continued
                && resp.finish_reason.as_str() == Some("length")
            {
                // the reply hit the token limit: the same request once more with a higher limit, and the longer of
                // the two is the answer (no message to the model about it)
                cut_continued = true;
                self.event(
                    task_id,
                    "cut_off_answer",
                    json!({"text": truncate(&final_, 200)}),
                )
                .await;
                final_ = self.continue_cut_answer(task_id, &msgs, &final_).await;
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
            // the reply to a workflow machine's result (its `T_result` in this request): the machine did the work, so
            // no runtime step acts on the reply; code appends the result's `show` facts to it (a page handed to
            // browser-use is the model's from there on)
            let workflow_reply = super::workflow::latest_result(&transcript)
                .filter(|r| get_str(r, "status") != "browser_use");
            let driven_run = super::workflow::active(&transcript, &super::driver::TOOLS).is_some()
                || super::workflow::active(&transcript, &super::apdt::TOOLS).is_some()
                || super::workflow::active(&transcript, &super::booking::TOOLS).is_some()
                || super::workflow::active(&transcript, &super::news::TOOLS).is_some()
                || super::workflow::active(&transcript, &super::claims::TOOLS).is_some()
                || super::workflow::active(&transcript, &super::trains::TOOLS).is_some()
                || super::workflow::active(&transcript, &super::flightstatus::TOOLS).is_some()
                || super::workflow::active(&transcript, &super::entry::TOOLS).is_some()
                || workflow_reply.is_some();
            // the confirmation after a hand-back is read by code: a snapshot the model skipped is taken here, once,
            // before the answer (the model reads it and passes the reference as JSON, `confirm_reference`)
            if !final_.is_empty()
                && asked.is_none()
                && !handback_snapped
                && !driven_run
                && super::guards::needs_handback_snapshot(&transcript)
            {
                handback_snapped = true;
                let tc = ToolCall {
                    id: format!("handback_snapshot_{steps}"),
                    name: "browser_snapshot".into(),
                    args: json!({"max_chars": 6000}),
                };
                self.event(task_id, "handback_snapshot", json!({})).await;
                transcript.push(json!({"role": "assistant", "content": final_, "tool_calls": [{"id": tc.id, "type": "function",
                    "function": {"name": tc.name, "arguments": tc.args.to_string()}}]}));
                match self
                    .exec_call(
                        &t,
                        &tc,
                        &mut transcript,
                        &catalog,
                        &[],
                        None,
                        CallMode::MAIN,
                    )
                    .await
                {
                    Ok(_) => continue,
                    Err(sp) => return self.suspend(task_id, sp, transcript).await,
                }
            }
            let judged =
                !final_.is_empty() && asked.is_none() && !driven_run && !force_final && !timed_out;
            // the answer read once by the model as the user would (JSON out): an answer that does not hold together
            // is written again from the machine's result (`reply_fixed`) or the run's facts (`rescue_final`), once —
            // no message to the model. The facts the user must read are appended by code afterwards (agent/show.rs).
            if !final_.is_empty()
                && asked.is_none()
                && !answer_reviewed
                && (!driven_run || workflow_reply.is_some())
                && !force_final
                && !timed_out
            {
                answer_reviewed = true;
                if let Some(review) = self.review_answer(task_id, &goal_text, &final_, lg).await {
                    self.event(
                        task_id,
                        "answer_review",
                        json!({"problem": review.problem, "language": review.language}),
                    )
                    .await;
                    let again = match &workflow_reply {
                        _ if review.problem.is_empty() => String::new(),
                        Some(res) => {
                            self.reply_fixed(task_id, &goal_text, &final_, res, &review, lg)
                                .await
                        }
                        None => {
                            self.rescue_final(task_id, &goal_text, &transcript, lg)
                                .await
                        }
                    };
                    if !again.is_empty() {
                        final_ = again;
                    }
                    // written in another language than the rule asks for: the model writes it again in the agent's
                    if review.language {
                        let fixed = self
                            .reply_in_language(task_id, &goal_text, &final_, lg)
                            .await;
                        if !fixed.is_empty() {
                            self.event(task_id, "language_fixed", json!({"to": lg}))
                                .await;
                            final_ = fixed;
                        }
                    }
                }
            }
            // a filled form (an arrival card, a claim) or an approved purchase that ends without the hand-over: the
            // runtime hands the page over itself — a runtime step decided from the transcript, so no budget stops it
            // (cards_interruption_real 2026-10-31: the claim's last field took the last step and nothing was handed over)
            if !final_.is_empty()
                && asked.is_none()
                && !auto_handed
                && !driven_run
                && super::guards::needs_handover(&transcript)
            {
                auto_handed = true;
                self.event(task_id, "auto_handover", json!({})).await;
                let tc = crate::llm::ToolCall {
                    id: format!("{}{steps}", super::guards::AUTO_HANDOVER),
                    name: "browser_request_takeover".into(),
                    args: json!({"reason": super::guards::handover_reason(&transcript, super::exec::HANDOVER_REASON)}),
                };
                // the answer the agent was about to give rides on the hand-over call: after the hand-back it opens the
                // final answer (`with_handover_answer`), so the user still gets it
                transcript.push(json!({"role": "assistant", "content": final_, "tool_calls": [{"id": tc.id, "type": "function",
                    "function": {"name": tc.name, "arguments": tc.args.to_string()}}]}));
                match self
                    .exec_call(
                        &t,
                        &tc,
                        &mut transcript,
                        &catalog,
                        &[],
                        None,
                        CallMode::MAIN,
                    )
                    .await
                {
                    Ok(_) => continue,
                    Err(sp) => return self.suspend(task_id, sp, transcript).await,
                }
            }
            // the file formats the user asked for (the file calls' `deliver_as`): each one the run did not make is
            // converted from what it made and sent, in code
            if judged {
                let conv = self.format_calls(task_id, &transcript).await;
                for tc in &conv {
                    transcript.push(assistant_with_calls("", std::slice::from_ref(tc)));
                    if let Err(sp) = self
                        .exec_call(&t, tc, &mut transcript, &catalog, &[], None, CallMode::MAIN)
                        .await
                    {
                        return self.suspend(task_id, sp, transcript).await;
                    }
                }
            }
            // a file the user asked for (its call's `deliver_as`) that the run wrote but never sent: sent now, in code
            for path in super::guards::unsent_files(&transcript) {
                let r = self.send_file(&t, &json!({"path": path})).await;
                self.event(
                    task_id,
                    "auto_send_file",
                    json!({"path": path, "ok": r.as_ref().is_ok_and(|x| !x.starts_with("ERROR"))}),
                )
                .await;
            }
            final_ = merge_stranded_answer(&transcript, &final_);
            final_ = super::guards::with_handover_answer(&transcript, &final_);
            final_ = super::guards::with_masked_cards(&final_);
            // the answer's links, repaired against the links the run's results showed (crate::links)
            final_ = crate::links::repair_answer(&final_, &crate::links::seen(&transcript));
            // the facts the user must read (agent/facts.rs, agent/show.rs): the trusted code tools' `show` items (a
            // machine's result, emergency_contacts, visa_rules, fx_convert, loan_payment, confirm_reference …) and the
            // run's structured facts, appended by code after the reply — never read from or written into its prose
            // (a Vault card without its catalog id: the model picks the row as a JSON choice, the facts read it)
            if !final_.is_empty() && asked.is_none() {
                let mut fact_view = transcript.clone();
                fact_view.extend(self.attach_card_ids(task_id, &transcript).await);
                let items = super::facts::show_items(
                    &fact_view,
                    super::facts::Ctx {
                        zh: lg != "en",
                        scheduled: get_str(&t, "source") == "schedule",
                        workflow: workflow_reply.is_some(),
                    },
                );
                // a required workflow call that was never made (agent/required.rs): a fixed line says the check
                // could not be done
                let mut items = items;
                if unmet.is_some() || force_final {
                    if let Some(need) = super::required::pending(&transcript) {
                        items.push(super::required::unmet_item(&need, lg != "en"));
                    }
                }
                if !items.is_empty() {
                    self.event(task_id, "answer_show", json!({"items": items.len()}))
                        .await;
                }
                final_ = super::show::append(&final_, &items, lg != "en");
            }
            // the run's confirmed reference: a known site's machine result, or the model's `confirm_reference`
            let confirmed = super::confirm::confirmed(&transcript);
            let oc = crate::outcome::assess(&self.store.events(task_id, 0)?, confirmed.as_ref());
            if !arr(&oc, "missing").is_empty() {
                self.event(
                    task_id,
                    "outcome_check",
                    json!({"status": oc.get("status"), "missing": oc.get("missing")}),
                )
                .await;
            }
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
            if self.is_cancelled(task_id) {
                // stopped while the answer was being finished: no answer is posted for a stopped task
                return self.stopped(task_id, &transcript);
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
            let (mut gave_up_f, mut force_final_f) = (gave_up, force_final);
            if asked.is_some() {
                // the run asked the user (`ask_user`): the question is the reply, the user's next message answers it
                self.event(task_id, "needs_user", json!({"reason": "ask_user"}))
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

    /// A reply cut by the token limit (`finish_reason: length`): the same request once more, without tools and with
    /// a higher limit; the longer reply is the answer. No words to the model about the cut.
    async fn continue_cut_answer(&self, task_id: &str, msgs: &[Value], first: &str) -> String {
        let set = self.settings();
        let limit = omuse_common::util::as_i64(&set["max_tokens"])
            .filter(|n| *n > 0)
            .map(|n| n * 2)
            .unwrap_or(8192)
            .max(8192);
        let o = ChatOpts {
            max_tokens: Some(limit),
            ..opts("executor", task_id)
        };
        match self.llm.chat(msgs, None, &o).await {
            Ok(r) if r.content.trim().chars().count() > first.trim().chars().count() => {
                strip_tool_markup(&r.content).trim().to_string()
            }
            _ => first.to_string(),
        }
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
        transcript: &[Value],
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
        // the new plan reaches the model through the system prompt (structured data); nothing is said to it
        Ok(())
    }

    /// A call identical to one earlier in the same turn (or, for searches, earlier in the task): answer its tool_call
    /// id without running it again.
    async fn skip_duplicate(
        &self,
        task_id: &str,
        call: &ToolCall,
        sk: &Skip,
        transcript: &mut Vec<Value>,
    ) {
        let first_id = &sk.first;
        let msg = if let Some((what, zh)) = sk.one_search {
            format!("已跳过 — 这一轮已经做过唯一的一次旅行搜索（调用 {first_id}，{zh}），一轮只搜一次。一次搜索就能覆盖日期范围、\
整个城市、偏好的航空公司或酒店品牌和往返；先用那次的结果回答，其余的（酒店、租车或别的条件）在回答里问用户，下一轮再搜。\
 Skipped: this turn already ran its one travel search (call {first_id}: {what}); ONE travel search per turn. One \
search covers a date range, a whole city, the preferred airlines or brands and a round trip; answer from that result \
and offer the rest (the hotel or car, other conditions) — the next turn can search again.")
        } else if sk.typed {
            format!("已跳过 — 这一轮已经在网页搜索框里搜过一次（调用 {first_id}），同一个浏览器页面一次只能显示一个结果页，\
后面的搜索会把前面的结果冲掉。先读完这次的结果并记下（update_plan 的 note），下一轮再搜下一个；\
要一次查多个，用 browser_read 读多个搜索网址（每次最多 4 个）。\
 Skipped: this turn already searched the site (call {first_id}); one page shows one result page, so later \
searches would replace it unread. Read and note these results, then search the next item in the next turn — \
or browser_read several search URLs (up to 4) at once.")
        } else if sk.search {
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

    /// The run ends because the user stopped the task: the transcript so far is kept, nothing else happens (the status
    /// is already CANCELLED, set by [`Runtime::cancel`]).
    fn stopped(&self, task_id: &str, transcript: &[Value]) -> Result<()> {
        // the calls of the last turn that never ran are answered as stopped (the transcript stays well-formed)
        let mut transcript = transcript.to_vec();
        if let Some(i) = transcript
            .iter()
            .rposition(|m| get_str(m, "role") == "assistant" && !tool_calls(m).is_empty())
        {
            let unanswered: Vec<String> = tool_calls(&transcript[i])
                .iter()
                .map(|tc| get_str(tc, "id"))
                .filter(|id| {
                    !transcript[i + 1..]
                        .iter()
                        .any(|m| get_str(m, "role") == "tool" && get_str(m, "tool_call_id") == *id)
                })
                .collect();
            for id in unanswered {
                transcript.push(json!({"role": "tool", "tool_call_id": id,
                                       "content": format!("ERROR: {} — not run.", crate::llm::CANCELLED_MSG)}));
            }
        }
        self.store
            .update_task(task_id, &json!({"transcript": transcript}))?;
        Ok(())
    }

    async fn suspend(&self, task_id: &str, sp: Suspend, transcript: Vec<Value>) -> Result<()> {
        if self.is_cancelled(task_id) {
            // stopped while the call that asks the user ran: no card, no wait; an approval it left behind expires and a
            // bot task it created never starts
            self.stopped(task_id, &transcript)?;
            if sp.status == super::bots::WAITING_BOT {
                self.cancel_handoff(task_id, &sp.waiting, "").await;
            } else {
                let _ = self
                    .sentinel(
                        "POST",
                        "/internal/expire_approvals",
                        Some(json!({"task_id": task_id, "reason": "任务已取消 (task cancelled)"})),
                        15.0,
                    )
                    .await;
            }
            return Ok(());
        }
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
            let card = json!({"type": "approval", "approval_id": waiting.get("approval_id"), "title": title, "task_id": task_id});
            self.store
                .add_msg(&cid, "system", &pyjson::dumps(&card), task_id, None)?;
            self.mirror_to_parent(&cid, &card, task_id).await;
            notify.push(json!({"kind": "approval_requested", "task_id": task_id, "approval_id": waiting.get("approval_id")}));
        } else if sget(&waiting, "type") == "takeover_requested" {
            let reason = sget(&waiting, "reason");
            let card = json!({"type": "takeover", "reason": reason, "task_id": task_id});
            self.store
                .add_msg(&cid, "system", &pyjson::dumps(&card), task_id, None)?;
            self.mirror_to_parent(&cid, &card, task_id).await;
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
        if sp.status == super::bots::WAITING_BOT {
            // the bot's task starts only now, so it cannot end before this task waits for it
            self.start(&sget(&waiting, "task_id"));
        }
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
        let mut cards: Vec<String> = vec![];
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
            if kind == "card" {
                // a card the user holds goes into the Vault by its name (its number, expiry and CVC UNKNOWN until the
                // user adds them); Sentinel refuses a number and keeps one entry per card
                let name = sget(f, "value").trim().to_string();
                if !name.is_empty() && !crate::store::looks_sensitive(&name) {
                    if let Ok(r) = self
                        .sentinel(
                            "POST",
                            "/internal/vault/card",
                            Some(json!({"name": name, "country": sget(f, "country").trim(), "task_id": task_id})),
                            20.0,
                        )
                        .await
                    {
                        if get_truthy(&r, "added") {
                            cards.push(get_str(&r["item"], "label"));
                        }
                    }
                }
                continue;
            }
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
                task_id: task_id.to_string(),
                ..Default::default()
            }
            .with_model_fields(f); // topics, scope, trigger, id_document, relation: the model's JSON, validated
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
        let out = json!({"facts": added, "recent": recent, "profile_suggestions": suggested, "vault_cards": cards});
        if !added.is_empty() || !recent.is_empty() || !suggested.is_empty() || !cards.is_empty() {
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
    // the booking machine's own search and data results stay whole: code reads them again at every step
    // (travel_flight_real 2026-11-01: the search cut short after the card fills read as "found nothing")
    const MACHINE_DATA: &[&str] = &[
        "flights_search",
        "flight_offer",
        "hotels_search",
        "hotel_rooms",
        "places_search",
        "michelin_search",
        "restaurant_availability",
        "browser_read",
    ];
    let kept: Vec<String> = transcript
        .iter()
        .flat_map(tool_calls)
        .filter(|tc| {
            let id = get_str(tc, "id");
            let (name, _) = super::util::stored_call(tc);
            // a workflow tool's call (its validated arguments) and its result (`T_result`) are the machine's state too
            (id.starts_with("bkg_")
                && MACHINE_DATA.contains(&name.rsplit("__").next().unwrap_or(&name)))
                || super::booking::TOOLS.contains(&name.as_str())
                || super::news::TOOLS.contains(&name.as_str())
                || id.starts_with(super::news::PREFIX)
                || super::claims::TOOLS.contains(&name.as_str())
                || (id.starts_with(super::claims::PREFIX) && name == "gmail_get_message")
                || super::driver::TOOLS.contains(&name.as_str())
                || (id.starts_with(super::driver::PREFIX) && name == "gmail_get_message")
                || super::trains::TOOLS.contains(&name.as_str())
                || id.starts_with(super::trains::PREFIX)
                || super::flightstatus::TOOLS.contains(&name.as_str())
                || id.starts_with(super::flightstatus::PREFIX)
                || super::entry::TOOLS.contains(&name.as_str())
                || super::apdt::TOOLS.contains(&name.as_str())
                || (id.starts_with(super::apdt::PREFIX)
                    && matches!(name.as_str(), "gmail_get_message" | "files_read"))
                || id.starts_with(super::workflow::RESULT_PREFIX)
                // a known page's elements read by a machine's fixed selectors (code-only): code reads them again at
                // every step (shopping_real 2026-11-01: the results' cards cut short read as "0 listings")
                || name == "browser_read_elements"
        })
        .map(|tc| get_str(tc, "id"))
        .collect();
    let tool_idx: Vec<usize> = transcript
        .iter()
        .enumerate()
        .filter(|(_, m)| {
            get_str(m, "role") == "tool" && !kept.contains(&get_str(m, "tool_call_id"))
        })
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
    fn catalog(&self) -> BoxFuture<'_, Value> {
        Box::pin(Runtime::catalog(self, false))
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

/// The model service gave up after its retries (overloaded, unavailable): another model may still answer.
fn unavailable(e: &LlmError) -> bool {
    e.to_string().contains("model unavailable after retries")
}

/// The user's own address: the first connected mailbox (the one sends go from), `""` when none.
fn own_mailbox(catalog: &Value) -> String {
    let gm = &catalog["connections"]["gmail"];
    if !truthy(&gm["ready"]) {
        return String::new();
    }
    gm.get("accounts")
        .and_then(Value::as_array)
        .and_then(|a| a.first())
        .map(|a| get_str(a, "email"))
        .filter(|e| !e.is_empty())
        .or_else(|| Some(get_str(gm, "account")).filter(|e| !e.is_empty()))
        .unwrap_or_default()
}

/// What the final review found: a problem with the reply (it does not hold together) or the language rule broken.
#[derive(Debug, Clone, Default)]
struct Review {
    problem: String,
    /// the reply is not in the language the rule asks for (the model's `language_ok: false`)
    language: bool,
}

/// The code machine that decides a driven run's steps: a workflow tool's run (`arrival_card_fill`: agent/driver.rs;
/// `apdt_refund`: agent/apdt.rs; the claims, the bookings, the news).
#[derive(Clone)]
enum Machine {
    /// a workflow tool's run (`travel_search`, `travel_book`, `shop_order`: agent/booking.rs; `news_gather`,
    /// `news_publish`, `news_mail`: agent/news.rs): the tool, its validated arguments and the index of its call in the
    /// transcript (agent/workflow.rs)
    Workflow {
        tool: String,
        args: Value,
        from: usize,
    },
}
