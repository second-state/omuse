//! The `Broker` of `app/browser/main.py`: lazy persistent Chromium, pages per task, the live view and user
//! takeover state, pacing, popups, downloads, page recycling and the idle shutdown.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::process::Child;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Weak};
use std::time::Duration;

use chromiumoxide::cdp::browser_protocol::browser::{
    EventDownloadProgress, EventDownloadWillBegin, SetDownloadBehaviorBehavior,
    SetDownloadBehaviorParams,
};
use chromiumoxide::cdp::browser_protocol::page::FrameId;
use chromiumoxide::cdp::browser_protocol::target::{
    CreateTargetParams, EventTargetCreated, EventTargetDestroyed, TargetId,
};
use chromiumoxide::Browser;
use futures::StreamExt;
use indexmap::IndexMap;
use omuse_common::api::ApiError;
use omuse_common::util::now_ts;
use omuse_common::Config;
use parking_lot::Mutex;
use serde_json::{json, Value};
use tokio::task::JoinHandle;

use crate::launch;
use crate::pages::{err_text, CdpResult, TaskPage};
use crate::render::Renderer;
use crate::textfmt::{reg_domain, PACE_BLOCK_HOLD, PACE_JITTER, PACE_MAX_WAIT, UA_HEADLESS};

/// The running persistent Chromium (`self.ctx`).
pub struct Chrome {
    browser: tokio::sync::Mutex<Browser>,
    handler: JoinHandle<()>,
    listeners: Mutex<Vec<JoinHandle<()>>>,
    /// Every page target of the context, in creation order (`ctx.pages`).
    all_pages: Mutex<IndexMap<String, Arc<TaskPage>>>,
    reg_lock: tokio::sync::Mutex<()>,
    user_agent: String,
    dead: Arc<AtomicBool>,
}

impl Chrome {
    /// Pages of the context that are still open.
    pub fn open_pages(&self) -> Vec<Arc<TaskPage>> {
        self.all_pages
            .lock()
            .values()
            .filter(|p| !p.is_closed())
            .cloned()
            .collect()
    }

    pub fn page_by_target(&self, target: &str) -> Option<Arc<TaskPage>> {
        self.all_pages.lock().get(target).cloned()
    }

    pub fn is_dead(&self) -> bool {
        self.dead.load(Ordering::SeqCst)
    }
}

/// Mutable broker state (the attributes of the Python `Broker`).
#[derive(Default)]
pub struct State {
    chrome: Option<Arc<Chrome>>,
    /// task_id -> page
    pub pages: IndexMap<String, Arc<TaskPage>>,
    /// task_id -> [(prefix, frame)]
    pub frame_maps: HashMap<String, Vec<(String, FrameId)>>,
    /// task_id -> HTTP status of the last navigation
    pub nav_status: HashMap<String, i64>,
    pub view_task: String,
    /// agent | user
    pub mode: String,
    pub takeover_reason: String,
    pub takeover_task: String,
    /// task_id -> {"task_id","reason","ts"}: tasks that asked the user for help
    pub requests: IndexMap<String, Value>,
    pub downloads: Vec<Value>,
    /// the user picked a task to watch: agents don't move the view meanwhile
    pub view_pinned_until: f64,
    /// task_id -> when the task ended (its page may be recycled)
    pub finished: HashMap<String, f64>,
    /// short-lived pages of web_search / browser_read (target ids)
    pub temp_pages: HashSet<String>,
    pub last_used: HashMap<String, f64>,
    /// registered domain -> when it was last requested (pacing)
    pub domain_at: HashMap<String, f64>,
    /// registered domain -> don't hit before this time (block / 429 cool-off)
    pub domain_hold: HashMap<String, f64>,
    /// popup page (target id) -> opener page
    pub parent: HashMap<String, Arc<TaskPage>>,
    xvfb: Option<Child>,
    own_display: String,
    display: String,
    pub headless: bool,
    /// is the persistent Chromium currently up?
    pub started: bool,
    /// last real web action, for the idle-shutdown reaper
    pub last_activity: f64,
    /// the "driver" is up: headless mode decided, Xvfb started when needed (`ensure_pw`)
    pub pw: bool,
    /// download guid -> index into `downloads`
    pub download_idx: HashMap<String, (usize, String)>,
}

/// The broker.
pub struct Broker {
    pub cfg: Arc<Config>,
    /// `WORKSPACE` (canonical).
    pub workspace: PathBuf,
    pub st: Mutex<State>,
    /// only for things that touch every task (start-up / teardown)
    start_lock: tokio::sync::Mutex<()>,
    /// user events are applied strictly in arrival order
    pub input_lock: tokio::sync::Mutex<()>,
    task_locks: Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
    pub render: Renderer,
    /// the private-host egress filter (off only in local tests against 127.0.0.1)
    pub egress_filter: AtomicBool,
    pub http: reqwest::Client,
    me: Weak<Broker>,
}

/// `HTTPException(500, "浏览器错误 browser error: ...")` for an unexpected CDP error.
pub fn browser_error(msg: &str) -> ApiError {
    ApiError::new(
        500,
        format!(
            "浏览器错误 browser error: {}",
            crate::textfmt::cut(msg, 300)
        ),
    )
}

pub fn cdp_err(e: chromiumoxide::error::CdpError) -> ApiError {
    browser_error(&err_text(&e))
}

impl Broker {
    pub fn new(cfg: Arc<Config>) -> Arc<Self> {
        let ws = cfg.workspace();
        let _ = std::fs::create_dir_all(&ws);
        let workspace = std::fs::canonicalize(&ws).unwrap_or(ws);
        Arc::new_cyclic(|me| Self {
            render: Renderer::new(cfg.clone(), workspace.clone()),
            cfg,
            workspace,
            st: Mutex::new(State {
                mode: "agent".into(),
                headless: true,
                ..Default::default()
            }),
            start_lock: tokio::sync::Mutex::new(()),
            input_lock: tokio::sync::Mutex::new(()),
            task_locks: Mutex::new(HashMap::new()),
            egress_filter: AtomicBool::new(true),
            http: omuse_common::http::client(60.0),
            me: me.clone(),
        })
    }

    fn arc(&self) -> Arc<Broker> {
        self.me.upgrade().expect("broker alive")
    }

    pub fn mode(&self) -> String {
        self.st.lock().mode.clone()
    }

    pub fn started(&self) -> bool {
        self.st.lock().started
    }

    pub fn headless(&self) -> bool {
        self.st.lock().headless
    }

    /// The persistent browser, when up.
    pub fn chrome(&self) -> Option<Arc<Chrome>> {
        self.st.lock().chrome.clone()
    }

    /// Decide headed/headless and bring up the virtual display (`ensure_pw`). Cheap; needed by the renderer too.
    pub async fn ensure_pw(&self) {
        if self.st.lock().pw {
            return;
        }
        let _g = self.start_lock.lock().await;
        if self.st.lock().pw {
            return;
        }
        let mode = self.cfg.browser.headless.trim().to_string();
        let env_display = std::env::var("DISPLAY").unwrap_or_default();
        let (headless, display, xvfb, own) = if mode == "1" {
            (true, String::new(), None, String::new())
        } else if mode == "0"
            || (!env_display.is_empty() && crate::textfmt::display_alive(&env_display))
        {
            (false, env_display, None, String::new())
        } else {
            let disp = if self.cfg.browser.display.trim().is_empty() {
                ":99".to_string()
            } else {
                self.cfg.browser.display.trim().to_string()
            };
            match tokio::task::spawn_blocking({
                let d = disp.clone();
                move || launch::start_xvfb(&d)
            })
            .await
            .ok()
            .flatten()
            {
                Some(child) => (false, disp.clone(), Some(child), disp),
                None => (true, String::new(), None, String::new()),
            }
        };
        let mut st = self.st.lock();
        st.headless = headless;
        st.display = display;
        st.xvfb = xvfb;
        st.own_display = own;
        st.pw = true;
    }

    /// Start the persistent Chromium on demand — the heavy part (`ensure_ctx`). Idempotent.
    pub async fn ensure_ctx(&self) -> Result<Arc<Chrome>, ApiError> {
        if let Some(c) = self.live_chrome() {
            return Ok(c);
        }
        self.ensure_pw().await;
        let _g = self.start_lock.lock().await;
        if let Some(c) = self.live_chrome() {
            return Ok(c);
        }
        let profile = self.cfg.browser_profile();
        std::fs::create_dir_all(&profile).map_err(|e| browser_error(&e.to_string()))?;
        for lock in ["SingletonLock", "SingletonCookie", "SingletonSocket"] {
            let _ = std::fs::remove_file(profile.join(lock));
        }
        let (headless, display) = {
            let st = self.st.lock();
            (st.headless, st.display.clone())
        };
        let exe = launch::find_executable(&self.cfg)
            .ok_or_else(|| browser_error("chromium executable not found"))?;
        let config = launch::persistent_config(&exe, &profile, headless, &display)
            .map_err(|e| browser_error(&e))?;
        let (browser, mut handler) = Browser::launch(config).await.map_err(cdp_err)?;
        let dead = Arc::new(AtomicBool::new(false));
        let dead2 = dead.clone();
        let handler_task = tokio::spawn(async move {
            while let Some(h) = handler.next().await {
                if h.is_err() {
                    break;
                }
            }
            dead2.store(true, Ordering::SeqCst);
        });
        let ua_cfg = self.cfg.browser.user_agent.trim().to_string();
        let ua = if !ua_cfg.is_empty() {
            ua_cfg
        } else if headless {
            UA_HEADLESS.to_string()
        } else {
            String::new()
        };
        let chrome = Arc::new(Chrome {
            browser: tokio::sync::Mutex::new(browser),
            handler: handler_task,
            listeners: Mutex::new(vec![]),
            all_pages: Mutex::new(IndexMap::new()),
            reg_lock: tokio::sync::Mutex::new(()),
            user_agent: ua,
            dead,
        });
        println!(
            "[browser] chromium started headless={} display={}",
            if headless { "True" } else { "False" },
            display
        );
        // downloads land in the quarantine folder under a GUID name; Browser.downloadProgress tells us when
        let quarantine = self.workspace.join("downloads").join(".quarantine");
        let _ = std::fs::create_dir_all(&quarantine);
        {
            let b = chrome.browser.lock().await;
            let mut p = SetDownloadBehaviorParams::new(SetDownloadBehaviorBehavior::AllowAndName);
            p.download_path = Some(quarantine.to_string_lossy().to_string());
            p.events_enabled = Some(true);
            let _ = b.execute(p).await;
        }
        self.spawn_browser_listeners(&chrome).await;
        {
            let b = chrome.browser.lock().await;
            if let Ok(pages) = b.pages().await {
                drop(b);
                for pg in pages {
                    let url = pg.url().await.ok().flatten().unwrap_or_default();
                    self.register_page(&chrome, pg, url).await;
                }
            }
        }
        let mut st = self.st.lock();
        st.chrome = Some(chrome.clone());
        st.started = true;
        st.last_activity = now_ts();
        Ok(chrome)
    }

    fn live_chrome(&self) -> Option<Arc<Chrome>> {
        let st = self.st.lock();
        match &st.chrome {
            Some(c) if st.started && !c.is_dead() => Some(c.clone()),
            _ => None,
        }
    }

    async fn spawn_browser_listeners(&self, chrome: &Arc<Chrome>) {
        let mut handles = vec![];
        let b = chrome.browser.lock().await;
        if let Ok(mut st) = b.event_listener::<EventTargetCreated>().await {
            let me = self.me.clone();
            let ch = Arc::downgrade(chrome);
            handles.push(tokio::spawn(async move {
                while let Some(ev) = st.next().await {
                    let (Some(bk), Some(ch)) = (me.upgrade(), ch.upgrade()) else {
                        break;
                    };
                    if ev.target_info.r#type != "page" {
                        continue;
                    }
                    let info = ev.target_info.clone();
                    tokio::spawn(async move {
                        bk.on_target_created(ch, info).await;
                    });
                }
            }));
        }
        if let Ok(mut st) = b.event_listener::<EventTargetDestroyed>().await {
            let me = self.me.clone();
            let ch = Arc::downgrade(chrome);
            handles.push(tokio::spawn(async move {
                while let Some(ev) = st.next().await {
                    let (Some(bk), Some(ch)) = (me.upgrade(), ch.upgrade()) else {
                        break;
                    };
                    bk.on_target_destroyed(&ch, ev.target_id.inner());
                }
            }));
        }
        if let Ok(mut st) = b.event_listener::<EventDownloadWillBegin>().await {
            let me = self.me.clone();
            handles.push(tokio::spawn(async move {
                while let Some(ev) = st.next().await {
                    let Some(bk) = me.upgrade() else { break };
                    bk.on_download_begin(&ev.guid, &ev.suggested_filename, &ev.url);
                }
            }));
        }
        if let Ok(mut st) = b.event_listener::<EventDownloadProgress>().await {
            let me = self.me.clone();
            handles.push(tokio::spawn(async move {
                while let Some(ev) = st.next().await {
                    let Some(bk) = me.upgrade() else { break };
                    let ev = ev.clone();
                    tokio::spawn(async move { bk.on_download_progress(&ev).await });
                }
            }));
        }
        drop(b);
        chrome.listeners.lock().extend(handles);
    }

    /// Wrap a page target once (`ctx.pages` registry) and give it the per-page setup.
    async fn register_page(
        &self,
        chrome: &Arc<Chrome>,
        page: chromiumoxide::Page,
        url: String,
    ) -> Arc<TaskPage> {
        let _g = chrome.reg_lock.lock().await;
        let id = page.target_id().inner().clone();
        if let Some(p) = chrome.all_pages.lock().get(&id).cloned() {
            return p;
        }
        let tp = TaskPage::new(page, url);
        chrome.all_pages.lock().insert(id, tp.clone());
        tp.prepare(
            &chrome.user_agent,
            self.egress_filter.load(Ordering::SeqCst),
        )
        .await;
        tp
    }

    async fn on_target_created(
        self: Arc<Self>,
        chrome: Arc<Chrome>,
        info: chromiumoxide::cdp::browser_protocol::target::TargetInfo,
    ) {
        let id = info.target_id.inner().clone();
        let mut page = None;
        for _ in 0..60 {
            let b = chrome.browser.lock().await;
            if let Ok(p) = b.get_page(TargetId::new(id.clone())).await {
                page = Some(p);
                break;
            }
            drop(b);
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        let Some(page) = page else { return };
        let tp = self.register_page(&chrome, page, info.url.clone()).await;
        // the first navigation of a popup may commit before our listeners are attached
        tp.refresh_url().await;
        if let Some(opener) = info.opener_id.as_ref() {
            self.on_popup(opener.inner(), tp);
        }
    }

    /// A window opened by a task's page (e.g. "Sign in with Google") becomes that task's current page, so the
    /// agent and the user's live view follow it; when it closes, the task goes back to the page that opened it.
    fn on_popup(&self, opener: &str, pop: Arc<TaskPage>) {
        let mut st = self.st.lock();
        let hit = st
            .pages
            .iter()
            .find(|(_, p)| p.id() == opener)
            .map(|(k, p)| (k.clone(), p.clone()));
        if let Some((tid, parent)) = hit {
            st.parent.insert(pop.id().to_string(), parent);
            st.pages.insert(tid, pop);
        }
    }

    fn on_target_destroyed(&self, chrome: &Arc<Chrome>, target: &str) {
        let page = chrome.all_pages.lock().shift_remove(target);
        if let Some(p) = &page {
            p.mark_closed();
        }
        let mut st = self.st.lock();
        st.temp_pages.remove(target);
        // _on_popup_closed
        let mut parent = st.parent.remove(target);
        while let Some(p) = &parent {
            if !p.is_closed() {
                break;
            }
            let pid = p.id().to_string();
            parent = st.parent.remove(&pid);
        }
        if let Some(parent) = parent {
            let tids: Vec<String> = st
                .pages
                .iter()
                .filter(|(_, p)| p.id() == target)
                .map(|(k, _)| k.clone())
                .collect();
            for tid in tids {
                st.pages.insert(tid, parent.clone());
            }
        }
    }

    /// `ctx.new_page()`.
    pub async fn new_page(&self, chrome: &Arc<Chrome>) -> Result<Arc<TaskPage>, ApiError> {
        let page = {
            let b = chrome.browser.lock().await;
            tokio::time::timeout(
                Duration::from_secs(30),
                b.new_page(CreateTargetParams::new("about:blank")),
            )
            .await
            .map_err(|_| browser_error("new page timed out"))?
            .map_err(cdp_err)?
        };
        Ok(self.register_page(chrome, page, "about:blank".into()).await)
    }

    /// backwards-compatible alias: start == bring the persistent browser up
    pub async fn start(&self) -> Result<(), ApiError> {
        self.ensure_ctx().await.map(|_| ())
    }

    /// Mark a real web action, so the idle-shutdown reaper doesn't close the browser while it's in use.
    pub fn touch(&self) {
        self.st.lock().last_activity = now_ts();
    }

    /// True when it is safe to shut the persistent browser down (`_idle_ok`).
    pub fn idle_ok(&self) -> bool {
        let st = self.st.lock();
        let idle = self.cfg.browser.idle_shutdown;
        if !st.started || idle <= 0.0 {
            return false;
        }
        if st.mode == "user" || !st.takeover_task.is_empty() || !st.requests.is_empty() {
            return false;
        }
        let now = now_ts();
        if now < st.view_pinned_until {
            return false;
        }
        for (k, p) in &st.pages {
            if !st.finished.contains_key(k) && !p.is_closed() {
                return false;
            }
        }
        (now - st.last_activity) > idle
    }

    /// Release the persistent Chromium (and Xvfb) so an idle install drops back to near-zero memory (`teardown`).
    pub async fn teardown(&self) {
        let _g = self.start_lock.lock().await;
        if !self.idle_ok() {
            return;
        }
        let (chrome, xvfb) = {
            let mut st = self.st.lock();
            let chrome = st.chrome.take();
            st.started = false;
            st.pages.clear();
            st.frame_maps.clear();
            st.last_used.clear();
            st.finished.clear();
            st.temp_pages.clear();
            st.parent.clear();
            st.nav_status.clear();
            st.view_task.clear();
            let xvfb = st.xvfb.take();
            if xvfb.is_some() {
                st.own_display.clear();
                st.display.clear();
                st.pw = false;
            }
            (chrome, xvfb)
        };
        if let Some(c) = chrome {
            close_chrome(&c).await;
        }
        self.render.close().await;
        if let Some(mut x) = xvfb {
            let _ = x.kill();
            let _ = x.wait();
        }
        println!("[browser] chromium shut down (idle)");
    }

    /// Process exit (`stop`).
    pub async fn stop(&self) {
        let (chrome, xvfb) = {
            let mut st = self.st.lock();
            st.started = false;
            (st.chrome.take(), st.xvfb.take())
        };
        if let Some(c) = chrome {
            close_chrome(&c).await;
        }
        self.render.close().await;
        if let Some(mut x) = xvfb {
            let _ = x.kill();
        }
    }

    // ------------------------------------------------------------ locks & guards

    pub fn task_lock(&self, task_id: &str) -> Arc<tokio::sync::Mutex<()>> {
        let key = if task_id.is_empty() {
            "default"
        } else {
            task_id
        };
        self.task_locks
            .lock()
            .entry(key.to_string())
            .or_default()
            .clone()
    }

    /// A takeover pauses only the task whose page the user is driving.
    pub fn check_agent(&self, task_id: &str) -> Result<(), ApiError> {
        let st = self.st.lock();
        let t = if task_id.is_empty() {
            "default"
        } else {
            task_id
        };
        let tt = if st.takeover_task.is_empty() {
            "default"
        } else {
            st.takeover_task.as_str()
        };
        if st.mode == "user" && t == tt {
            return Err(ApiError::new(
                423,
                "用户正在接管浏览器，Agent 已暂停 (user takeover in progress)",
            ));
        }
        Ok(())
    }

    /// Move the live view to this task unless the user is watching (or driving) another task, or the task being
    /// shown is still busy (`_follow`). Call with the state lock held.
    fn follow(st: &mut State, task_id: &str) {
        if st.view_task == task_id {
            return;
        }
        if st.mode == "user" || now_ts() < st.view_pinned_until {
            return;
        }
        let cur = st.view_task.clone();
        let busy = match st.pages.get(&cur) {
            Some(p) => {
                !p.is_closed()
                    && !st.finished.contains_key(&cur)
                    && now_ts() - st.last_used.get(&cur).copied().unwrap_or(0.0) < 20.0
            }
            None => false,
        };
        if !busy {
            st.view_task = task_id.to_string();
        }
    }

    // ------------------------------------------------------------ pages

    /// `page_for(task_id, create)`.
    pub async fn page_for(
        &self,
        task_id: &str,
        create: bool,
    ) -> Result<Option<Arc<TaskPage>>, ApiError> {
        let task_id = if task_id.is_empty() {
            "default".to_string()
        } else {
            task_id.to_string()
        };
        let chrome = if create {
            self.ensure_ctx().await?
        } else {
            match self.live_chrome() {
                Some(c) => c,
                None => return Ok(None),
            }
        };
        self.touch();
        {
            let mut st = self.st.lock();
            if let Some(p) = st.pages.get(&task_id).cloned() {
                if !p.is_closed() {
                    st.last_used.insert(task_id.clone(), now_ts());
                    st.finished.remove(&task_id);
                    Self::follow(&mut st, &task_id);
                    return Ok(Some(p));
                }
            }
        }
        if !create {
            return Ok(None);
        }
        self.recycle(8).await;
        let blank = {
            let st = self.st.lock();
            let used: HashSet<String> = st.pages.values().map(|p| p.id().to_string()).collect();
            chrome
                .all_pages
                .lock()
                .values()
                .find(|pg| {
                    let u = pg.url();
                    (u == "about:blank" || u.is_empty())
                        && !pg.is_closed()
                        && !used.contains(pg.id())
                        && !st.temp_pages.contains(pg.id())
                })
                .cloned()
        };
        let p = match blank {
            Some(p) => p,
            None => self.new_page(&chrome).await?,
        };
        let mut st = self.st.lock();
        st.pages.insert(task_id.clone(), p.clone());
        st.last_used.insert(task_id.clone(), now_ts());
        st.finished.remove(&task_id);
        Self::follow(&mut st, &task_id);
        Ok(Some(p))
    }

    /// `page_for(task_id)` (create=True) — always a page.
    pub async fn page(&self, task_id: &str) -> Result<Arc<TaskPage>, ApiError> {
        Ok(self.page_for(task_id, true).await?.expect("created"))
    }

    /// Close pages of finished tasks after 10 minutes, and when there are too many pages close the least recently
    /// used finished ones first (`_recycle`).
    pub async fn recycle(&self, keep: usize) {
        let victims: Vec<(String, Arc<TaskPage>)> = {
            let mut st = self.st.lock();
            let now = now_ts();
            let mut protected: HashSet<String> = st.requests.keys().cloned().collect();
            protected.insert(st.takeover_task.clone());
            protected.insert(st.view_task.clone());
            let closable = |k: &str| !protected.contains(k);
            let mut old: Vec<String> = st
                .finished
                .iter()
                .filter(|(k, t)| now - **t > 600.0 && closable(k))
                .map(|(k, _)| k.clone())
                .collect();
            if st.pages.len() >= keep {
                let lu = |k: &String| st.last_used.get(k).copied().unwrap_or(0.0);
                let mut done: Vec<String> = st
                    .pages
                    .keys()
                    .filter(|k| st.finished.contains_key(*k) && closable(k))
                    .cloned()
                    .collect();
                done.sort_by(|a, b| {
                    lu(a)
                        .partial_cmp(&lu(b))
                        .unwrap_or(std::cmp::Ordering::Equal)
                });
                let mut idle: Vec<String> = st
                    .pages
                    .keys()
                    .filter(|k| {
                        !st.finished.contains_key(*k) && closable(k) && now - lu(k) > 1800.0
                    })
                    .cloned()
                    .collect();
                idle.sort_by(|a, b| {
                    lu(a)
                        .partial_cmp(&lu(b))
                        .unwrap_or(std::cmp::Ordering::Equal)
                });
                let n = st.pages.len() - keep + 1;
                old.extend(done.into_iter().chain(idle).take(n));
            }
            let mut seen = HashSet::new();
            let mut out = vec![];
            for k in old {
                if !seen.insert(k.clone()) {
                    continue;
                }
                let pg = st.pages.shift_remove(&k);
                st.finished.remove(&k);
                st.frame_maps.remove(&k);
                self.task_locks.lock().remove(&k);
                if let Some(pg) = pg {
                    out.push((k, pg));
                }
            }
            out
        };
        for (_, pg) in victims {
            pg.close().await;
        }
    }

    /// Takeover requests still waiting, newest first (requests older than 6 h are dropped).
    pub fn open_requests(&self) -> Vec<Value> {
        let mut st = self.st.lock();
        let now = now_ts();
        st.requests.retain(|_, v| {
            now - v.get("ts").and_then(|t| t.as_f64()).unwrap_or(0.0) <= 6.0 * 3600.0
        });
        let mut v: Vec<Value> = st.requests.values().cloned().collect();
        v.sort_by(|a, b| {
            let ta = a.get("ts").and_then(|t| t.as_f64()).unwrap_or(0.0);
            let tb = b.get("ts").and_then(|t| t.as_f64()).unwrap_or(0.0);
            tb.partial_cmp(&ta).unwrap_or(std::cmp::Ordering::Equal)
        });
        v
    }

    pub fn requested(&self) -> Option<Value> {
        self.open_requests().into_iter().next()
    }

    /// The page the live view shows (`view_page`).
    pub fn view_page(&self) -> Option<Arc<TaskPage>> {
        let mut st = self.st.lock();
        if let Some(p) = st.pages.get(&st.view_task) {
            if !p.is_closed() {
                return Some(p.clone());
            }
        }
        let hit = st
            .pages
            .iter()
            .find(|(_, p)| !p.is_closed())
            .map(|(k, p)| (k.clone(), p.clone()));
        if let Some((tid, p)) = hit {
            st.view_task = tid;
            return Some(p);
        }
        st.chrome
            .as_ref()
            .and_then(|c| c.open_pages().into_iter().next())
    }

    /// Open tabs of the context (`len([p for p in ctx.pages if not p.is_closed()])`).
    pub fn tabs(&self) -> usize {
        self.chrome().map(|c| c.open_pages().len()).unwrap_or(0)
    }

    /// `settle(page, ms)`.
    pub async fn settle(&self, page: &TaskPage, ms: u64) {
        page.wait_dom_ready(Duration::from_secs(8)).await;
        tokio::time::sleep(Duration::from_millis(ms)).await;
    }

    // ------------------------------------------------------------ pacing

    /// How long to wait before hitting this site, so we don't hammer it (pure function, for testing).
    pub fn pace_wait(&self, url: &str, now: Option<f64>) -> f64 {
        let dom = reg_domain(url);
        if dom.is_empty() {
            return 0.0;
        }
        let now = now.unwrap_or_else(now_ts);
        let st = self.st.lock();
        let hold = st.domain_hold.get(&dom).copied().unwrap_or(0.0);
        if hold > now {
            return (hold - now).min(PACE_MAX_WAIT);
        }
        let last = st.domain_at.get(&dom).copied().unwrap_or(0.0);
        let gap = self.cfg.browser.pace_gap + rand::random::<f64>() * PACE_JITTER;
        (last + gap - now).clamp(0.0, PACE_MAX_WAIT)
    }

    pub async fn pace(&self, url: &str) {
        let w = self.pace_wait(url, None);
        if w > 0.0 {
            tokio::time::sleep(Duration::from_secs_f64(w)).await;
        }
        let dom = reg_domain(url);
        if !dom.is_empty() {
            self.st.lock().domain_at.insert(dom, now_ts());
        }
    }

    /// A site returned a bot wall / 429: cool off before hitting it again this session.
    pub fn note_block(&self, url: &str) {
        let dom = reg_domain(url);
        if !dom.is_empty() {
            self.st
                .lock()
                .domain_hold
                .insert(dom, now_ts() + PACE_BLOCK_HOLD);
        }
    }

    // ------------------------------------------------------------ temp pages (search / read)

    pub async fn temp_page(&self) -> Result<Arc<TaskPage>, ApiError> {
        let chrome = self.ensure_ctx().await?;
        self.touch();
        let p = self.new_page(&chrome).await?;
        self.st.lock().temp_pages.insert(p.id().to_string());
        Ok(p)
    }

    pub async fn close_temp(&self, p: &TaskPage) {
        self.st.lock().temp_pages.remove(p.id());
        p.close().await;
    }

    // ------------------------------------------------------------ misc JS helpers

    /// `describe`: what an element is, for Sentinel's approval checks.
    pub async fn describe(&self, task_id: &str, r: &str) -> Result<Value, ApiError> {
        let el = self.locate(task_id, r).await.map_err(|e| e.not_found(r))?;
        let mut info = el
            .page
            .call_on(&el.obj, DESCRIBE_JS, vec![])
            .await
            .map_err(|e| {
                ApiError::new(
                    409,
                    format!(
                        "找不到元素 {}，请重新获取快照 (element not found): {}",
                        r,
                        crate::textfmt::cut(&err_text(&e), 100)
                    ),
                )
            })?;
        let page = self.page(task_id).await?;
        if let Some(o) = info.as_object_mut() {
            o.insert("page_url".into(), json!(page.url()));
            o.insert("page_title".into(), json!(page.title().await));
        }
        Ok(info)
    }

    /// `focused`: the active element.
    pub async fn focused(&self, task_id: &str) -> Result<Value, ApiError> {
        let page = self.page(task_id).await?;
        Ok(page
            .eval(None, FOCUSED_JS, vec![])
            .await
            .unwrap_or_else(|_| json!({})))
    }

    /// Record a page-level JSON snapshot of tasks for `/state`.
    pub async fn task_list(&self) -> Vec<Value> {
        let pages: Vec<(String, Arc<TaskPage>)> = self
            .st
            .lock()
            .pages
            .iter()
            .map(|(k, p)| (k.clone(), p.clone()))
            .collect();
        pages
            .into_iter()
            .filter(|(_, p)| !p.is_closed())
            .map(|(k, p)| json!({"task_id": k, "url": p.url()}))
            .collect()
    }

    /// For tests: the Arc handle.
    pub fn handle(&self) -> Arc<Broker> {
        self.arc()
    }
}

async fn close_chrome(c: &Arc<Chrome>) {
    for t in c.listeners.lock().drain(..) {
        t.abort();
    }
    for p in c.all_pages.lock().values() {
        p.mark_closed();
    }
    let mut b = c.browser.lock().await;
    let _ = tokio::time::timeout(Duration::from_secs(5), b.close()).await;
    let _ = tokio::time::timeout(Duration::from_secs(5), b.wait()).await;
    let _ = b.kill().await;
    c.handler.abort();
}

const DESCRIBE_JS: &str = r#"el => ({
                tag: el.tagName.toLowerCase(),
                role: el.getAttribute('role') || '',
                name: (el.getAttribute('aria-label') || (el.labels && el.labels[0] && el.labels[0].innerText) || el.innerText || (['submit','button','reset'].includes((el.type||'').toLowerCase()) ? el.value : '') || el.getAttribute('placeholder') || el.getAttribute('name') || el.getAttribute('title') || '').trim().slice(0,200),
                editable: !!el.isContentEditable,
                autocomplete: (el.getAttribute('autocomplete') || '').toLowerCase().slice(0,40),
                input_type: (el.type || '').toLowerCase(),
                is_password: (el.type || '').toLowerCase() === 'password' || (el.getAttribute('autocomplete')||'').includes('password'),
                in_form: !!el.closest('form'),
                // a site search box whose label is a rotating promo text (2026-10-04 FairPrice: "1 for $21.30 - ...")
                searchy: (el.type || '').toLowerCase() === 'search' || /search|query|keyword|^q$/i.test((el.getAttribute('name') || '') + ' ' + (el.id || '')) ||
                         (el.getAttribute('enterkeyhint') || '') === 'search' || !!(el.closest('form') && (el.closest('form').getAttribute('role') === 'search' ||
                         /search|query/i.test(el.closest('form').getAttribute('action') || ''))) || !!el.closest('[role=search]'),
                href: el.getAttribute('href') || ''
            })"#;

const FOCUSED_JS: &str = r#"() => { const el = document.activeElement; if (!el || el === document.body) return {};
                return {tag: el.tagName.toLowerCase(), role: el.getAttribute('role')||'', name: (el.getAttribute('aria-label')||el.getAttribute('placeholder')||el.name||'').slice(0,200),
                        input_type: (el.type||'').toLowerCase(), in_form: !!el.closest('form')}; }"#;

/// Convenience for `Result<_, CdpError>` → 500 browser error.
pub trait CdpExt<T> {
    fn api(self) -> Result<T, ApiError>;
}

impl<T> CdpExt<T> for CdpResult<T> {
    fn api(self) -> Result<T, ApiError> {
        self.map_err(cdp_err)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn broker() -> Arc<Broker> {
        let cfg =
            Config::parse("[browser]\nheadless = \"1\"\n[data]\ndir = \"/tmp/omuse-browser-unit\"")
                .unwrap();
        Broker::new(Arc::new(cfg))
    }

    #[test]
    fn reg_domain_and_pacing() {
        let b = broker();
        let gap = b.cfg.browser.pace_gap;
        assert_eq!(b.pace_wait("https://decathlon.sg/a", Some(1000.0)), 0.0);
        b.st.lock().domain_at.insert("decathlon.sg".into(), 1000.0);
        let w = b.pace_wait("https://decathlon.sg/b", Some(1000.2));
        assert!(gap - 0.2 <= w && w <= gap + 0.6 + 0.01, "{w}");
        assert_eq!(b.pace_wait("https://other.test/x", Some(1000.2)), 0.0);
        b.note_block("https://decathlon.sg/c");
        let w = b.pace_wait("https://decathlon.sg/d", None);
        assert!(0.0 < w && w <= PACE_MAX_WAIT);
    }

    #[tokio::test]
    async fn browser_is_not_launched_until_used() {
        let b = broker();
        assert!(!b.started());
        assert!(b.view_page().is_none());
        assert!(b.page_for("t", false).await.unwrap().is_none());
        assert!(!b.started() && b.chrome().is_none());
        // idle shutdown needs a started browser; takeover state blocks it
        assert!(!b.idle_ok());
        assert!(b.check_agent("t").is_ok());
        {
            let mut st = b.st.lock();
            st.mode = "user".into();
            st.takeover_task = "t".into();
        }
        assert_eq!(b.check_agent("t").unwrap_err().status, 423);
        assert!(b.check_agent("other").is_ok());
    }

    #[test]
    fn open_requests_newest_first_and_expire() {
        let b = broker();
        {
            let mut st = b.st.lock();
            st.requests.insert(
                "a".into(),
                json!({"task_id": "a", "reason": "x", "ts": now_ts() - 10.0}),
            );
            st.requests.insert(
                "b".into(),
                json!({"task_id": "b", "reason": "y", "ts": now_ts()}),
            );
            st.requests.insert(
                "old".into(),
                json!({"task_id": "old", "reason": "z", "ts": now_ts() - 7.0 * 3600.0}),
            );
        }
        let r = b.open_requests();
        assert_eq!(r.len(), 2);
        assert_eq!(r[0]["task_id"], "b");
        assert_eq!(b.requested().unwrap()["task_id"], "b");
    }
}
