//! The slice of the `Runtime` object (`app/runtime/agent.py`) that the scheduler, goals, golden runs and the health
//! watch call. `agent.rs` implements it for `Runtime`; the modules here hold an `Arc<dyn RuntimeApi>` so they can be
//! built and unit-tested with a fake.
//!
//! Conventions:
//! * dict-shaped arguments and results are `serde_json::Value` (same keys as the Python dicts);
//! * async methods return `BoxFuture`;
//! * [`RuntimeApi::sentinel`] errors format (`Display`) as `"<httpx exception name>: <detail>"` — `ConnectError: …`,
//!   `ConnectTimeout: …`, `ReadTimeout: …`, `HTTPStatusError: …` — because the Python code showed `type(e).__name__`
//!   and tested it (`"Connect" in type(e).__name__`); see [`exc_name`].

use std::sync::Arc;

use futures::future::BoxFuture;
use omuse_common::Config;
use serde_json::Value;

use crate::llm::Llm;
use crate::store::Store;

/// What `scheduler.py`, `goals.py`, `golden.py` and `health.py` use of `Runtime`.
pub trait RuntimeApi: Send + Sync + 'static {
    /// `rt.store`.
    fn store(&self) -> &Arc<Store>;
    /// `rt.llm`.
    fn llm(&self) -> &Arc<Llm>;
    /// The configuration (`[model]` key for the health check's `auth_headers`, `OMUSE_*` values).
    fn config(&self) -> &Arc<Config>;

    /// `await rt.publish(event)` — fan an event out to the UI websocket clients. Never fails.
    fn publish(&self, event: Value) -> BoxFuture<'_, ()>;

    /// `await rt.sentinel(method, path, payload, timeout)` — one JSON call to Sentinel with the runtime token
    /// (`X-Persona-Runtime`). `payload` is the JSON body (`None` = no body); `timeout` in seconds. An HTTP error
    /// status is an error too (`raise_for_status`). Errors display as `"<ExceptionName>: <detail>"`.
    fn sentinel(
        &self,
        method: &str,
        path: &str,
        payload: Option<Value>,
        timeout: f64,
    ) -> BoxFuture<'_, anyhow::Result<Value>>;

    /// `await rt.audit(actor, action, task_id, **kw)` — posts `/internal/audit`, swallowing failures. `kw` is an
    /// object with any of `resource`, `result`, `detail`, …
    fn audit(&self, actor: &str, action: &str, task_id: &str, kw: Value) -> BoxFuture<'_, ()>;

    /// `await rt.submit(conv_id, goal, source, schedule_id)` — create a task, publish, audit, start it; returns the
    /// task record.
    fn submit(
        &self,
        conv_id: &str,
        goal: &str,
        source: &str,
        schedule_id: &str,
    ) -> BoxFuture<'_, anyhow::Result<Value>>;

    /// `rt.start(task_id)` — run an already created task in the background (no-op when it is already running).
    fn start_task(&self, task_id: &str);

    /// `await rt.cancel(task_id, reason)`.
    fn cancel(&self, task_id: &str, reason: &str) -> BoxFuture<'_, ()>;

    /// `rt.running.get(task_id)` is a live (not done) worker.
    fn is_running(&self, task_id: &str) -> bool;

    /// `rt.ctx_mode[task_id] = mode` (`Some("legacy")`) / `rt.ctx_mode.pop(task_id, None)` (`None`).
    fn set_ctx_mode(&self, task_id: &str, mode: Option<&str>);

    /// `await rt.catalog()` — Sentinel's tool catalog (`{"tools": [...], "connections": {...}}`), cached 20 s;
    /// never fails (empty catalog when Sentinel is down).
    fn catalog(&self) -> BoxFuture<'_, Value>;

    /// `rt._facts_legacy(goal)` — the pre-batch-2 fact selection (golden A/B).
    fn facts_legacy(&self, goal: &str) -> Vec<Value>;

    /// `context.select(goal, rt.store.facts(500), rt.store.entities(), limit=12)` — personal context v1.
    fn facts_select(&self, goal: &str) -> Vec<Value>;

    /// `await rt._plan(task, facts, history_txt)` — the planner; `task` is `{"id", "goal", "conv_id", "attachments"}`.
    fn plan(
        &self,
        task: Value,
        facts: Vec<Value>,
        history_txt: &str,
    ) -> BoxFuture<'_, anyhow::Result<Value>>;

    /// `rt.skills()` — the enabled skills.
    fn skills(&self) -> Vec<Value>;

    /// `memory_tidy.due(rt.store, now)` — the daily consolidation is due.
    fn memory_tidy_due(&self, now: f64) -> bool;

    /// `memory_tidy.start(rt, dry_run=False, kind="daily")` — start it in the background (no-op while one runs).
    fn memory_tidy_start(&self);
}

/// Python's `type(e).__name__` for an error that follows the `"<ExceptionName>: <detail>"` convention
/// (the text before the first `": "`, or the whole text when there is none).
pub fn exc_name(e: &anyhow::Error) -> String {
    let s = e.to_string();
    match s.find(": ") {
        Some(i) => s[..i].to_string(),
        None => s,
    }
}

/// `f"{type(e).__name__}: {e}"` for a [`reqwest::Error`], with the httpx class names the Python code printed.
pub fn describe_reqwest_error(e: &reqwest::Error) -> String {
    let kind = if e.is_timeout() {
        if e.is_connect() {
            "ConnectTimeout"
        } else {
            "ReadTimeout"
        }
    } else if e.is_connect() {
        "ConnectError"
    } else if e.is_status() {
        "HTTPStatusError"
    } else if e.is_body() || e.is_decode() {
        "ReadError"
    } else {
        "TransportError"
    };
    format!("{}: {}", kind, e)
}

#[cfg(test)]
pub(crate) mod testing {
    //! A fake runtime over a real (temporary) [`Store`]: records Sentinel calls, cancellations and submissions.

    use std::sync::Arc;

    use futures::future::BoxFuture;
    use omuse_common::Config;
    use parking_lot::Mutex;
    use serde_json::{json, Value};

    use super::RuntimeApi;
    use crate::llm::Llm;
    use crate::store::Store;

    pub struct FakeRt {
        pub _dir: tempfile::TempDir,
        pub store: Arc<Store>,
        pub llm: Arc<Llm>,
        pub cfg: Arc<Config>,
        /// `(path, payload)` of every `sentinel` call.
        pub calls: Mutex<Vec<(String, Value)>>,
        pub cancelled: Mutex<Vec<String>>,
        pub submitted: Mutex<Vec<String>>,
        pub published: Mutex<Vec<Value>>,
        /// What `sentinel` answers for a path (default `{}`).
        pub answers: Mutex<Vec<(String, Value)>>,
    }

    impl FakeRt {
        pub fn new() -> Arc<Self> {
            let dir = tempfile::tempdir().unwrap();
            let cfg = Arc::new(Config::parse("").unwrap());
            let store = Arc::new(Store::new(dir.path(), cfg.clone()).unwrap());
            let st = store.clone();
            let llm = Arc::new(Llm::new(
                Arc::new(move || st.settings().unwrap_or_else(|_| json!({}))),
                None,
                None,
            ));
            Arc::new(Self {
                _dir: dir,
                store,
                llm,
                cfg,
                calls: Mutex::new(vec![]),
                cancelled: Mutex::new(vec![]),
                submitted: Mutex::new(vec![]),
                published: Mutex::new(vec![]),
                answers: Mutex::new(vec![]),
            })
        }

        pub fn notify_count(&self) -> usize {
            self.calls
                .lock()
                .iter()
                .filter(|(p, _)| p == "/internal/notify")
                .count()
        }
    }

    impl RuntimeApi for FakeRt {
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
            self.published.lock().push(event);
            Box::pin(async {})
        }
        fn sentinel(
            &self,
            _method: &str,
            path: &str,
            payload: Option<Value>,
            _timeout: f64,
        ) -> BoxFuture<'_, anyhow::Result<Value>> {
            self.calls
                .lock()
                .push((path.to_string(), payload.unwrap_or(Value::Null)));
            let ans = self
                .answers
                .lock()
                .iter()
                .find(|(p, _)| p == path)
                .map(|(_, v)| v.clone())
                .unwrap_or_else(|| json!({}));
            Box::pin(async move { Ok(ans) })
        }
        fn audit(
            &self,
            _actor: &str,
            _action: &str,
            _task_id: &str,
            _kw: Value,
        ) -> BoxFuture<'_, ()> {
            Box::pin(async {})
        }
        fn submit(
            &self,
            conv_id: &str,
            goal: &str,
            source: &str,
            schedule_id: &str,
        ) -> BoxFuture<'_, anyhow::Result<Value>> {
            let t = self
                .store
                .create_task(goal, conv_id, source, schedule_id, "", None);
            Box::pin(async move {
                let t = t?;
                self.submitted
                    .lock()
                    .push(t["id"].as_str().unwrap_or("").to_string());
                Ok(t)
            })
        }
        fn start_task(&self, task_id: &str) {
            self.submitted.lock().push(task_id.to_string());
        }
        fn cancel(&self, task_id: &str, _reason: &str) -> BoxFuture<'_, ()> {
            self.cancelled.lock().push(task_id.to_string());
            let _ = self
                .store
                .update_task(task_id, &json!({"status": "CANCELLED"}));
            Box::pin(async {})
        }
        fn is_running(&self, _task_id: &str) -> bool {
            false
        }
        fn set_ctx_mode(&self, _task_id: &str, _mode: Option<&str>) {}
        fn catalog(&self) -> BoxFuture<'_, Value> {
            Box::pin(async { json!({"tools": [], "connections": {}}) })
        }
        fn facts_legacy(&self, _goal: &str) -> Vec<Value> {
            vec![]
        }
        fn facts_select(&self, _goal: &str) -> Vec<Value> {
            vec![]
        }
        fn plan(
            &self,
            _task: Value,
            _facts: Vec<Value>,
            _history_txt: &str,
        ) -> BoxFuture<'_, anyhow::Result<Value>> {
            Box::pin(async { Ok(json!({"objective": "", "steps": []})) })
        }
        fn skills(&self) -> Vec<Value> {
            vec![]
        }
        fn memory_tidy_due(&self, _now: f64) -> bool {
            false
        }
        fn memory_tidy_start(&self) {}
    }
}
