//! One Chromium page target driven through CDP: navigation with Playwright-like waiting, per-frame JavaScript
//! evaluation, input events, screenshots and the egress filter. Replaces the Playwright `Page` the Python broker used.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Weak};
use std::time::{Duration, Instant};

use chromiumoxide::cdp::browser_protocol::dom::{
    DescribeNodeParams, GetContentQuadsParams, ScrollIntoViewIfNeededParams,
};
use chromiumoxide::cdp::browser_protocol::emulation::{
    SetFocusEmulationEnabledParams, SetLocaleOverrideParams,
};
use chromiumoxide::cdp::browser_protocol::fetch::{
    ContinueRequestParams, EnableParams as FetchEnableParams, EventRequestPaused,
    FailRequestParams, RequestPattern,
};
use chromiumoxide::cdp::browser_protocol::input::{
    DispatchMouseEventParams, DispatchMouseEventType, MouseButton,
};
use chromiumoxide::cdp::browser_protocol::network::{
    EnableParams as NetworkEnableParams, ErrorReason, EventLoadingFailed, EventLoadingFinished,
    EventRequestWillBeSent, EventResponseReceived, ResourceType, SetUserAgentOverrideParams,
};
use chromiumoxide::cdp::browser_protocol::page::{
    CaptureScreenshotFormat, CaptureScreenshotParams, CreateIsolatedWorldParams,
    EventFrameNavigated, EventJavascriptDialogOpening, EventNavigatedWithinDocument, FrameId,
    FrameTree, GetFrameTreeParams, GetLayoutMetricsParams, GetNavigationHistoryParams,
    HandleJavaScriptDialogParams, NavigateToHistoryEntryParams, ReloadParams, Viewport,
};
use chromiumoxide::cdp::js_protocol::runtime::{
    CallArgument, CallFunctionOnParams, ExecutionContextId, RemoteObjectId,
};
use chromiumoxide::error::CdpError;
use chromiumoxide::Page;
use futures::StreamExt;
use omuse_common::guard::host_is_private;
use parking_lot::Mutex;
use serde_json::Value;
use tokio::task::JoinHandle;

pub type CdpResult<T> = Result<T, CdpError>;

/// A frame of the page (`page.frames`): id, URL, parent.
#[derive(Debug, Clone)]
pub struct FrameInfo {
    pub id: FrameId,
    pub url: String,
    pub parent: Option<FrameId>,
}

/// Navigation bookkeeping fed by the Network / Page event listeners.
#[derive(Debug, Default)]
pub struct NavState {
    /// Main-frame commits so far (`Page.frameNavigated` / `navigatedWithinDocument`).
    pub commits: u64,
    /// HTTP status of the last main-document response.
    pub status: Option<i64>,
    /// MIME type of the last main-document response (`application/pdf` when a link opened a PDF).
    pub mime: Option<String>,
    /// `net::ERR_*` of the main document, when it failed.
    pub error: Option<String>,
    /// Request id of the main document in flight.
    doc_req: Option<String>,
    /// Requests in flight (for "network idle").
    inflight: std::collections::HashSet<String>,
    last_net: Option<Instant>,
}

/// A page target with the state the broker keeps per Playwright page.
pub struct TaskPage {
    pub page: Page,
    target: String,
    closed: AtomicBool,
    pub nav: Mutex<NavState>,
    mouse: Mutex<(f64, f64)>,
    url_cache: Mutex<String>,
    tasks: Mutex<Vec<JoinHandle<()>>>,
    prepared: AtomicBool,
}

/// A screenshot clip in viewport CSS pixels.
#[derive(Debug, Clone, Copy)]
pub struct Clip {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
    pub scale: f64,
}

/// Why a navigation did not reach a loaded page (messages shaped like Playwright's so the broker's
/// `"ERR_BLOCKED_BY_CLIENT" in str(e)` / `"imeout" in str(e)` checks keep working).
#[derive(Debug, thiserror::Error)]
pub enum NavError {
    #[error("Page.goto: {0} at {1}")]
    Net(String, String),
    #[error("Page.goto: Timeout {0}ms exceeded.")]
    Timeout(u64),
    #[error("Page.goto: {0}")]
    Cdp(String),
}

/// Text of a CDP / JavaScript error the way `str(e)` read in Python.
pub fn err_text(e: &CdpError) -> String {
    match e {
        CdpError::JavascriptException(ex) => ex
            .exception
            .as_ref()
            .and_then(|o| o.description.clone())
            .unwrap_or_else(|| ex.text.clone()),
        CdpError::Chrome(c) => c.message.clone(),
        CdpError::Timeout => "Timeout exceeded".to_string(),
        other => other.to_string(),
    }
}

fn deep_find_js() -> &'static str {
    // querySelector that pierces open shadow roots (Playwright locators do), used to resolve refs
    "const deepFind = (root, sel) => { let e = root.querySelector(sel); if (e) return e; \
     for (const h of root.querySelectorAll('*')) { if (h.shadowRoot) { e = deepFind(h.shadowRoot, sel); if (e) return e; } } return null; };"
}

/// JS that resolves a ref to its element (or null).
pub fn ref_finder_js() -> String {
    format!(
        "(ref) => {{ {} return deepFind(document, '[data-persona-ref=\"' + ref + '\"]'); }}",
        deep_find_js()
    )
}

impl TaskPage {
    pub fn new(page: Page, url: String) -> Arc<Self> {
        let target = page.target_id().inner().clone();
        Arc::new(Self {
            page,
            target,
            closed: AtomicBool::new(false),
            nav: Mutex::new(NavState::default()),
            mouse: Mutex::new((0.0, 0.0)),
            url_cache: Mutex::new(url),
            tasks: Mutex::new(vec![]),
            prepared: AtomicBool::new(false),
        })
    }

    /// The target id (also the main frame id).
    pub fn id(&self) -> &str {
        &self.target
    }

    pub fn main_frame(&self) -> FrameId {
        FrameId::new(self.target.clone())
    }

    pub fn is_closed(&self) -> bool {
        self.closed.load(Ordering::SeqCst)
    }

    /// The target is gone (closed by us, the user, or a script).
    pub fn mark_closed(&self) {
        self.closed.store(true, Ordering::SeqCst);
        for t in self.tasks.lock().drain(..) {
            t.abort();
        }
    }

    /// `page.close()`.
    pub async fn close(&self) {
        if self.is_closed() {
            return;
        }
        self.mark_closed();
        let _ = tokio::time::timeout(Duration::from_secs(5), self.page.clone().close()).await;
    }

    /// `page.url` (the last committed main-frame URL).
    pub fn url(&self) -> String {
        self.url_cache.lock().clone()
    }

    fn set_url(&self, u: &str) {
        if !u.is_empty() {
            *self.url_cache.lock() = u.to_string();
        }
    }

    /// Ask Chromium for the current URL (after actions that may have navigated).
    pub async fn refresh_url(&self) -> String {
        if let Ok(Ok(Some(u))) = tokio::time::timeout(Duration::from_secs(3), self.page.url()).await
        {
            self.set_url(&u);
        }
        self.url()
    }

    /// `page.title()` ("" on any error).
    pub async fn title(&self) -> String {
        match tokio::time::timeout(
            Duration::from_secs(5),
            self.eval(None, "() => document.title", vec![]),
        )
        .await
        {
            Ok(Ok(Value::String(s))) => s,
            _ => String::new(),
        }
    }

    /// Per-page setup Playwright did for every page of the context: focus emulation, user agent + Accept-Language,
    /// locale, the egress filter (requests to private hosts fail with `ERR_BLOCKED_BY_CLIENT`), dialogs auto-dismissed,
    /// and the listeners that track navigation status and network activity.
    pub async fn prepare(self: &Arc<Self>, user_agent: &str, egress_filter: bool) {
        if self.prepared.swap(true, Ordering::SeqCst) {
            return;
        }
        let _ = self.page.execute(NetworkEnableParams::default()).await;
        // Playwright enables this on every page. Besides `document.hasFocus()`, it keeps a background tab of the
        // headed (Xvfb) browser rendering: without it, the first mouse event sent to a tab that is not in front
        // waits ~5 s for a frame (a takeover click on another task's page).
        let _ = self
            .page
            .execute(SetFocusEmulationEnabledParams::new(true))
            .await;
        if !user_agent.is_empty() {
            let mut ua = SetUserAgentOverrideParams::new(user_agent.to_string());
            ua.accept_language = Some("en-US".to_string());
            let _ = self.page.execute(ua).await;
        }
        let _ = self
            .page
            .execute(SetLocaleOverrideParams {
                locale: Some("en-US".to_string()),
            })
            .await;
        self.spawn_listeners(egress_filter).await;
    }

    async fn spawn_listeners(self: &Arc<Self>, egress_filter: bool) {
        let mut handles = vec![];
        let weak = Arc::downgrade(self);

        if let Ok(mut st) = self.page.event_listener::<EventRequestWillBeSent>().await {
            let w = weak.clone();
            handles.push(tokio::spawn(async move {
                while let Some(ev) = st.next().await {
                    let Some(p) = w.upgrade() else { break };
                    let mut nav = p.nav.lock();
                    nav.inflight.insert(ev.request_id.inner().clone());
                    nav.last_net = Some(Instant::now());
                    if ev.r#type == Some(ResourceType::Document)
                        && ev
                            .frame_id
                            .as_ref()
                            .map(|f| f.inner() == &p.target)
                            .unwrap_or(false)
                    {
                        nav.doc_req = Some(ev.request_id.inner().clone());
                    }
                }
            }));
        }
        if let Ok(mut st) = self.page.event_listener::<EventResponseReceived>().await {
            let w = weak.clone();
            handles.push(tokio::spawn(async move {
                while let Some(ev) = st.next().await {
                    let Some(p) = w.upgrade() else { break };
                    if ev.r#type == ResourceType::Document
                        && ev
                            .frame_id
                            .as_ref()
                            .map(|f| f.inner() == &p.target)
                            .unwrap_or(false)
                    {
                        let mut nav = p.nav.lock();
                        nav.status = Some(ev.response.status);
                        nav.mime = Some(ev.response.mime_type.to_lowercase());
                    }
                }
            }));
        }
        if let Ok(mut st) = self.page.event_listener::<EventLoadingFinished>().await {
            let w = weak.clone();
            handles.push(tokio::spawn(async move {
                while let Some(ev) = st.next().await {
                    let Some(p) = w.upgrade() else { break };
                    let mut nav = p.nav.lock();
                    nav.inflight.remove(ev.request_id.inner());
                    nav.last_net = Some(Instant::now());
                }
            }));
        }
        if let Ok(mut st) = self.page.event_listener::<EventLoadingFailed>().await {
            let w = weak.clone();
            handles.push(tokio::spawn(async move {
                while let Some(ev) = st.next().await {
                    let Some(p) = w.upgrade() else { break };
                    let mut nav = p.nav.lock();
                    nav.inflight.remove(ev.request_id.inner());
                    nav.last_net = Some(Instant::now());
                    if nav.doc_req.as_deref() == Some(ev.request_id.inner().as_str())
                        && ev.canceled != Some(true)
                    {
                        nav.error = Some(ev.error_text.clone());
                    }
                }
            }));
        }
        if let Ok(mut st) = self.page.event_listener::<EventFrameNavigated>().await {
            let w = weak.clone();
            handles.push(tokio::spawn(async move {
                while let Some(ev) = st.next().await {
                    let Some(p) = w.upgrade() else { break };
                    if ev.frame.parent_id.is_none() {
                        p.set_url(&ev.frame.url);
                        let mut nav = p.nav.lock();
                        nav.commits += 1;
                        nav.inflight.clear();
                        if ev.frame.unreachable_url.is_some() && nav.error.is_none() {
                            nav.error = Some("net::ERR_FAILED".to_string());
                        }
                    }
                }
            }));
        }
        if let Ok(mut st) = self
            .page
            .event_listener::<EventNavigatedWithinDocument>()
            .await
        {
            let w = weak.clone();
            handles.push(tokio::spawn(async move {
                while let Some(ev) = st.next().await {
                    let Some(p) = w.upgrade() else { break };
                    if ev.frame_id.inner() == &p.target {
                        p.set_url(&ev.url);
                        p.nav.lock().commits += 1;
                    }
                }
            }));
        }
        if let Ok(mut st) = self
            .page
            .event_listener::<EventJavascriptDialogOpening>()
            .await
        {
            let w = weak.clone();
            handles.push(tokio::spawn(async move {
                while let Some(_ev) = st.next().await {
                    let Some(p) = w.upgrade() else { break };
                    // Playwright dismisses dialogs nobody listens for
                    let _ = p
                        .page
                        .execute(HandleJavaScriptDialogParams {
                            accept: false,
                            prompt_text: None,
                        })
                        .await;
                }
            }));
        }
        if egress_filter {
            if let Ok(mut st) = self.page.event_listener::<EventRequestPaused>().await {
                let w = weak.clone();
                handles.push(tokio::spawn(async move {
                    while let Some(ev) = st.next().await {
                        let Some(p) = w.upgrade() else { break };
                        let page = p.page.clone();
                        let id = ev.request_id.clone();
                        let blocked = egress_blocked(&ev.request.url);
                        tokio::spawn(async move {
                            if blocked {
                                let _ = page
                                    .execute(FailRequestParams::new(
                                        id,
                                        ErrorReason::BlockedByClient,
                                    ))
                                    .await;
                            } else {
                                let _ = page.execute(ContinueRequestParams::new(id)).await;
                            }
                        });
                    }
                }));
                let _ = self
                    .page
                    .execute(
                        FetchEnableParams::builder()
                            .pattern(RequestPattern::builder().url_pattern("*").build())
                            .build(),
                    )
                    .await;
            }
        }
        self.tasks.lock().extend(handles);
    }

    // ------------------------------------------------------------------ frames & evaluation

    /// All frames of the page, main frame first (`page.frames`).
    pub async fn frames(&self) -> CdpResult<Vec<FrameInfo>> {
        let tree = self
            .page
            .execute(GetFrameTreeParams::default())
            .await?
            .result
            .frame_tree;
        let mut out = vec![];
        fn walk(t: &FrameTree, out: &mut Vec<FrameInfo>) {
            out.push(FrameInfo {
                id: t.frame.id.clone(),
                url: t.frame.url.clone(),
                parent: t.frame.parent_id.clone(),
            });
            if let Some(ch) = &t.child_frames {
                for c in ch {
                    walk(c, out);
                }
            }
        }
        walk(&tree, &mut out);
        Ok(out)
    }

    /// The main-world execution context of a frame (`None` = main frame); an isolated world when the main world
    /// is not known (a frame that never ran a script).
    pub async fn context(&self, frame: Option<&FrameId>) -> CdpResult<ExecutionContextId> {
        for attempt in 0..6 {
            let ctx = match frame {
                None => self.page.execution_context().await?,
                Some(f) => self.page.frame_execution_context(f.clone()).await?,
            };
            if let Some(c) = ctx {
                return Ok(c);
            }
            if attempt >= 2 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        let fid = frame.cloned().unwrap_or_else(|| self.main_frame());
        let mut p = CreateIsolatedWorldParams::new(fid);
        p.world_name = Some("__persona".to_string());
        p.grant_univeral_access = Some(true);
        Ok(self.page.execute(p).await?.result.execution_context_id)
    }

    fn args(args: Vec<Value>) -> Vec<CallArgument> {
        args.into_iter()
            .map(|v| CallArgument {
                value: Some(v),
                unserializable_value: None,
                object_id: None,
            })
            .collect()
    }

    async fn call(
        &self,
        mut params: CallFunctionOnParams,
        by_value: bool,
    ) -> CdpResult<chromiumoxide::cdp::js_protocol::runtime::RemoteObject> {
        params.return_by_value = Some(by_value);
        params.await_promise = Some(true);
        let r = self.page.execute(params).await?.result;
        if let Some(ex) = r.exception_details {
            return Err(CdpError::JavascriptException(Box::new(ex)));
        }
        Ok(r.result)
    }

    /// Call a JS function (source text) in a frame with JSON arguments; the result by value.
    pub async fn eval(
        &self,
        frame: Option<&FrameId>,
        func: &str,
        args: Vec<Value>,
    ) -> CdpResult<Value> {
        let mut last = None;
        for attempt in 0..2 {
            let ctx = self.context(frame).await?;
            let mut p = CallFunctionOnParams::new(func.to_string());
            p.execution_context_id = Some(ctx);
            p.arguments = Some(Self::args(args.clone()));
            match self.call(p, true).await {
                Ok(r) => return Ok(r.value.unwrap_or(Value::Null)),
                Err(e) => {
                    let t = err_text(&e);
                    if attempt == 0
                        && (t.contains("Cannot find context")
                            || t.contains("Execution context was destroyed"))
                    {
                        last = Some(e);
                        tokio::time::sleep(Duration::from_millis(150)).await;
                        continue;
                    }
                    return Err(e);
                }
            }
        }
        Err(last.unwrap_or(CdpError::NotFound))
    }

    /// Like [`eval`](Self::eval) but the function returns a DOM node: its remote object id (`None` for null).
    pub async fn eval_obj(
        &self,
        frame: Option<&FrameId>,
        func: &str,
        args: Vec<Value>,
    ) -> CdpResult<Option<RemoteObjectId>> {
        let ctx = self.context(frame).await?;
        let mut p = CallFunctionOnParams::new(func.to_string());
        p.execution_context_id = Some(ctx);
        p.arguments = Some(Self::args(args));
        let r = self.call(p, false).await?;
        Ok(r.object_id)
    }

    fn on_wrapper(func: &str) -> String {
        format!(
            "function() {{ return ({}).call(null, this, ...arguments); }}",
            func
        )
    }

    /// `loc.evaluate("el => ...")`: call `func(el, ...args)` on an element; the result by value.
    pub async fn call_on(
        &self,
        obj: &RemoteObjectId,
        func: &str,
        args: Vec<Value>,
    ) -> CdpResult<Value> {
        let mut p = CallFunctionOnParams::new(Self::on_wrapper(func));
        p.object_id = Some(obj.clone());
        p.arguments = Some(Self::args(args));
        Ok(self.call(p, true).await?.value.unwrap_or(Value::Null))
    }

    /// `func(el, ...args)` returning another node (or null).
    pub async fn call_on_obj(
        &self,
        obj: &RemoteObjectId,
        func: &str,
        args: Vec<Value>,
    ) -> CdpResult<Option<RemoteObjectId>> {
        let mut p = CallFunctionOnParams::new(Self::on_wrapper(func));
        p.object_id = Some(obj.clone());
        p.arguments = Some(Self::args(args));
        Ok(self.call(p, false).await?.object_id)
    }

    /// Content quads of an element in main-frame viewport pixels (`DOM.getContentQuads`).
    pub async fn quads(&self, obj: &RemoteObjectId) -> CdpResult<Vec<Vec<f64>>> {
        let r = self
            .page
            .execute(
                GetContentQuadsParams::builder()
                    .object_id(obj.clone())
                    .build(),
            )
            .await?;
        Ok(r.result.quads.iter().map(|q| q.inner().clone()).collect())
    }

    /// `DOM.scrollIntoViewIfNeeded`.
    pub async fn scroll_into_view(&self, obj: &RemoteObjectId) -> CdpResult<()> {
        self.page
            .execute(
                ScrollIntoViewIfNeededParams::builder()
                    .object_id(obj.clone())
                    .build(),
            )
            .await?;
        Ok(())
    }

    /// The frame an `<iframe>` element hosts.
    pub async fn frame_of_element(&self, obj: &RemoteObjectId) -> CdpResult<Option<FrameId>> {
        let r = self
            .page
            .execute(DescribeNodeParams::builder().object_id(obj.clone()).build())
            .await?;
        Ok(r.result.node.frame_id)
    }

    // ------------------------------------------------------------------ navigation & waiting

    fn nav_marks(&self) -> (u64, Option<String>) {
        let n = self.nav.lock();
        (n.commits, n.error.clone())
    }

    /// `page.goto(url, wait_until="domcontentloaded", timeout=…)`: the HTTP status of the document (0 when none).
    pub async fn navigate(&self, url: &str, timeout: Duration) -> Result<i64, NavError> {
        let deadline = Instant::now() + timeout;
        let start = {
            let mut n = self.nav.lock();
            n.status = None;
            n.mime = None;
            n.error = None;
            n.doc_req = None;
            n.commits
        };
        let mut kicked = false;
        for _ in 0..3 {
            match self
                .eval(
                    None,
                    "(u) => { location.href = u; return true; }",
                    vec![Value::String(url.to_string())],
                )
                .await
            {
                Ok(_) => {
                    kicked = true;
                    break;
                }
                Err(e) => {
                    let t = err_text(&e);
                    if t.contains("context") {
                        tokio::time::sleep(Duration::from_millis(200)).await;
                        continue;
                    }
                    return Err(NavError::Cdp(t));
                }
            }
        }
        if !kicked {
            return Err(NavError::Cdp("no execution context".to_string()));
        }
        loop {
            let (commits, error) = self.nav_marks();
            if let Some(e) = error {
                return Err(NavError::Net(e, url.to_string()));
            }
            if commits > start {
                break;
            }
            if Instant::now() >= deadline {
                return Err(NavError::Timeout(timeout.as_millis() as u64));
            }
            tokio::time::sleep(Duration::from_millis(40)).await;
        }
        let left = deadline.saturating_duration_since(Instant::now());
        self.wait_dom_ready(left).await;
        let (_, error) = self.nav_marks();
        if let Some(e) = error {
            return Err(NavError::Net(e, url.to_string()));
        }
        let _ = self.refresh_url().await;
        Ok(self.nav.lock().status.unwrap_or(0))
    }

    /// `page.go_back(timeout=…)`: nothing happens when there is no previous entry (Playwright returns null).
    pub async fn go_back(&self, timeout: Duration) -> CdpResult<()> {
        let hist = self
            .page
            .execute(GetNavigationHistoryParams::default())
            .await?
            .result;
        let idx = hist.current_index;
        if idx <= 0 {
            return Ok(());
        }
        let Some(entry) = hist.entries.get((idx - 1) as usize) else {
            return Ok(());
        };
        let start = self.nav.lock().commits;
        self.page
            .execute(NavigateToHistoryEntryParams::new(entry.id))
            .await?;
        self.wait_commit(start, timeout).await;
        let _ = self.refresh_url().await;
        Ok(())
    }

    /// `page.reload()`.
    pub async fn reload(&self) -> CdpResult<()> {
        let start = self.nav.lock().commits;
        self.page.execute(ReloadParams::default()).await?;
        self.wait_commit(start, Duration::from_secs(30)).await;
        Ok(())
    }

    async fn wait_commit(&self, start: u64, timeout: Duration) {
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            if self.nav.lock().commits > start {
                break;
            }
            tokio::time::sleep(Duration::from_millis(40)).await;
        }
        self.wait_dom_ready(deadline.saturating_duration_since(Instant::now()))
            .await;
    }

    /// `wait_for_load_state("domcontentloaded")`: the current document is past "loading" (or the timeout passed).
    pub async fn wait_dom_ready(&self, timeout: Duration) {
        let deadline = Instant::now() + timeout;
        loop {
            match tokio::time::timeout(
                Duration::from_secs(3),
                self.eval(None, "() => document.readyState", vec![]),
            )
            .await
            {
                Ok(Ok(Value::String(s))) if s != "loading" => return,
                _ => {}
            }
            if Instant::now() >= deadline {
                return;
            }
            tokio::time::sleep(Duration::from_millis(60)).await;
        }
    }

    /// `wait_for_load_state("networkidle")`: no request in flight for 500 ms (or the timeout passed).
    pub async fn wait_network_idle(&self, timeout: Duration) {
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            {
                let n = self.nav.lock();
                let quiet = n
                    .last_net
                    .map(|t| t.elapsed() >= Duration::from_millis(500))
                    .unwrap_or(true);
                if n.inflight.is_empty() && quiet {
                    return;
                }
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }

    // ------------------------------------------------------------------ screenshots

    /// Scroll position of the main frame (document pixels), for clips.
    pub async fn scroll_offset(&self) -> (f64, f64) {
        match self.page.execute(GetLayoutMetricsParams::default()).await {
            Ok(r) => (
                r.result.css_visual_viewport.page_x,
                r.result.css_visual_viewport.page_y,
            ),
            Err(_) => (0.0, 0.0),
        }
    }

    /// `page.screenshot(type=…, quality=…, clip=…)` (the clip is given in viewport coordinates).
    pub async fn screenshot(
        &self,
        jpeg: bool,
        quality: Option<i64>,
        clip: Option<Clip>,
        timeout: Duration,
    ) -> CdpResult<Vec<u8>> {
        let mut p = CaptureScreenshotParams::builder()
            .format(if jpeg {
                CaptureScreenshotFormat::Jpeg
            } else {
                CaptureScreenshotFormat::Png
            })
            .from_surface(true);
        if let (true, Some(q)) = (jpeg, quality) {
            p = p.quality(q);
        }
        if let Some(c) = clip {
            let (sx, sy) = self.scroll_offset().await;
            p = p.clip(Viewport {
                x: c.x + sx,
                y: c.y + sy,
                width: c.width,
                height: c.height,
                scale: c.scale,
            });
        }
        let fut = self.page.execute(p.build());
        let r = tokio::time::timeout(timeout, fut)
            .await
            .map_err(|_| CdpError::Timeout)??;
        use base64::Engine;
        Ok(base64::engine::general_purpose::STANDARD
            .decode(AsRef::<[u8]>::as_ref(&r.result.data))
            .unwrap_or_default())
    }

    // ------------------------------------------------------------------ mouse

    async fn mouse_event(
        &self,
        t: DispatchMouseEventType,
        x: f64,
        y: f64,
        button: Option<MouseButton>,
        clicks: Option<i64>,
        delta: Option<(f64, f64)>,
    ) -> CdpResult<()> {
        let mut p = DispatchMouseEventParams::new(t, x, y);
        p.button = button;
        p.click_count = clicks;
        if let Some((dx, dy)) = delta {
            p.delta_x = Some(dx);
            p.delta_y = Some(dy);
        }
        self.page.execute(p).await?;
        Ok(())
    }

    /// `page.mouse.move(x, y)`.
    pub async fn mouse_move(&self, x: f64, y: f64) -> CdpResult<()> {
        *self.mouse.lock() = (x, y);
        self.mouse_event(
            DispatchMouseEventType::MouseMoved,
            x,
            y,
            Some(MouseButton::None),
            None,
            None,
        )
        .await
    }

    /// `page.mouse.click(x, y)` / `dblclick`.
    pub async fn click_at(&self, x: f64, y: f64, double: bool) -> CdpResult<()> {
        self.mouse_move(x, y).await?;
        let n = if double { 2 } else { 1 };
        for c in 1..=n {
            self.mouse_event(
                DispatchMouseEventType::MousePressed,
                x,
                y,
                Some(MouseButton::Left),
                Some(c),
                None,
            )
            .await?;
            self.mouse_event(
                DispatchMouseEventType::MouseReleased,
                x,
                y,
                Some(MouseButton::Left),
                Some(c),
                None,
            )
            .await?;
        }
        Ok(())
    }

    /// `page.mouse.wheel(dx, dy)` at the last mouse position.
    pub async fn wheel(&self, dx: f64, dy: f64) -> CdpResult<()> {
        let (x, y) = *self.mouse.lock();
        self.mouse_event(
            DispatchMouseEventType::MouseWheel,
            x,
            y,
            Some(MouseButton::None),
            None,
            Some((dx, dy)),
        )
        .await
    }
}

/// The egress rule of the Python `_egress_filter`: http(s)/ws(s) requests to private hosts are aborted.
pub fn egress_blocked(url: &str) -> bool {
    match url::Url::parse(url) {
        Ok(u) => {
            let scheme = u.scheme().to_lowercase();
            if ["http", "https", "ws", "wss"].contains(&scheme.as_str()) {
                host_is_private(u.host_str().unwrap_or(""))
            } else {
                false
            }
        }
        Err(_) => true,
    }
}

/// Keep a `Weak` to a page in listener tasks.
pub type WeakPage = Weak<TaskPage>;
