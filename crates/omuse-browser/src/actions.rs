//! Element refs → DOM nodes, the Playwright-style actions on them (click, fill, type, select, upload) and the
//! `/agent/{action}` dispatcher plus `/user/input` of `app/browser/main.py`.

use std::sync::Arc;
use std::sync::LazyLock;
use std::time::{Duration, Instant};

use chromiumoxide::cdp::browser_protocol::dom::SetFileInputFilesParams;
use chromiumoxide::cdp::browser_protocol::page::FrameId;
use chromiumoxide::cdp::js_protocol::runtime::RemoteObjectId;
use omuse_common::api::ApiError;
use omuse_common::guard::check_url;
use omuse_common::util::{as_f64, as_i64, get_str, get_truthy, now_ts, truthy};
use regex::Regex;
use serde_json::{json, Value};

use crate::broker::{browser_error, cdp_err, Broker, CdpExt};
use crate::keys;
use crate::pages::{err_text, ref_finder_js, CdpResult, TaskPage};
use crate::textfmt::{cut, expiry_for_field, normalize_date_input, VIEWPORT_H, VIEWPORT_W};

pub const BLOCK_JS: &str = include_str!("../js/block.js");

/// A resolved ref: the element's remote object in its frame.
pub struct ElementRef {
    pub page: Arc<TaskPage>,
    pub frame: Option<FrameId>,
    pub obj: RemoteObjectId,
}

/// Why a ref could not be resolved.
pub enum LocateError {
    /// 400 / 409 straight from the Python `locate`
    Api(ApiError),
    /// the frame is fine but no element carries the ref any more (Playwright would time out waiting)
    NotFound(String),
}

impl LocateError {
    /// `409 找不到元素 …` (describe / save_media).
    pub fn not_found(self, r: &str) -> ApiError {
        match self {
            LocateError::Api(e) => e,
            LocateError::NotFound(msg) => ApiError::new(
                409,
                format!(
                    "找不到元素 {}，请重新获取快照 (element not found): {}",
                    r,
                    cut(&msg, 100)
                ),
            ),
        }
    }
    fn text(&self) -> String {
        match self {
            LocateError::Api(e) => e.detail_text(),
            LocateError::NotFound(m) => m.clone(),
        }
    }
}

fn not_found_msg(r: &str) -> String {
    format!(
        "Timeout 8000ms exceeded. waiting for locator('[data-persona-ref=\"{}\"]').first",
        r
    )
}

const VISIBLE_HIT_JS: &str = r#"(el) => {
  const r = el.getBoundingClientRect();
  if (r.width < 1 || r.height < 1) return {visible: false};
  const cx = r.left + r.width / 2, cy = r.top + r.height / 2;
  let t = document.elementFromPoint(cx, cy);
  while (t && t.shadowRoot) { const i = t.shadowRoot.elementFromPoint(cx, cy); if (!i || i === t) break; t = i; }
  const within = (a, b) => { for (let n = b; n; n = n.parentNode || n.host) if (n === a) return true; return false; };
  const ok = !t || t === el || within(el, t) || within(t, el);
  const desc = t ? ('<' + t.tagName.toLowerCase() + (t.id ? '#' + t.id : '') + (t.className && typeof t.className === 'string' ? '.' + t.className.trim().split(/\s+/).slice(0, 2).join('.') : '') + '>') : '';
  return {visible: true, ok, desc};
}"#;

impl ElementRef {
    /// `loc.evaluate(func)`.
    pub async fn eval(&self, func: &str, args: Vec<Value>) -> CdpResult<Value> {
        self.page.call_on(&self.obj, func, args).await
    }

    /// Click like Playwright's `locator.click(timeout=…)`: scroll into view, wait until the element's centre is
    /// not covered, then real mouse events at that point.
    pub async fn click(&self, timeout: Duration) -> Result<(), String> {
        let _ = self.page.scroll_into_view(&self.obj).await;
        let deadline = Instant::now() + timeout;
        loop {
            let quads = self.page.quads(&self.obj).await.map_err(|e| err_text(&e))?;
            let Some(q) = quads.first().filter(|q| q.len() >= 8) else {
                return Err("element is not visible".to_string());
            };
            let (cx, cy) = (
                (q[0] + q[2] + q[4] + q[6]) / 4.0,
                (q[1] + q[3] + q[5] + q[7]) / 4.0,
            );
            let hit = self
                .eval(VISIBLE_HIT_JS, vec![])
                .await
                .map_err(|e| err_text(&e))?;
            if !get_truthy(&hit, "visible") {
                return Err("element is not visible".to_string());
            }
            if get_truthy(&hit, "ok") {
                self.page
                    .click_at(cx, cy, false)
                    .await
                    .map_err(|e| err_text(&e))?;
                return Ok(());
            }
            let last = format!("{} intercepts pointer events", get_str(&hit, "desc"));
            if Instant::now() >= deadline {
                return Err(format!(
                    "Timeout {}ms exceeded. {}",
                    timeout.as_millis(),
                    last
                ));
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }

    /// `el.focus()`.
    pub async fn focus(&self) -> CdpResult<()> {
        self.eval("el => { el.focus(); }", vec![]).await.map(|_| ())
    }
}

static REF_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^(f\d+)?e\d+$").unwrap());

impl Broker {
    /// `locate(task_id, ref)`: the element behind a snapshot ref (in the right frame).
    pub async fn locate(&self, task_id: &str, r: &str) -> Result<ElementRef, LocateError> {
        let r = r.trim().trim_matches(|c| c == '[' || c == ']').to_string();
        let caps = REF_RE.captures(&r).ok_or_else(|| {
            LocateError::Api(ApiError::new(
                400,
                format!("无效的 ref: {}（请使用快照中的 [e12] 之类的 ref）", r),
            ))
        })?;
        let prefix = caps.get(1).map(|m| m.as_str()).unwrap_or("").to_string();
        let key = if task_id.is_empty() {
            "default"
        } else {
            task_id
        };
        let stale = || {
            LocateError::Api(ApiError::new(409, "页面已变化，请先调用 browser_snapshot 获取新的 ref (stale ref, take a new snapshot)"))
        };
        let frame = {
            let st = self.st.lock();
            st.frame_maps
                .get(key)
                .and_then(|m| m.iter().find(|(p, _)| *p == prefix).map(|(_, f)| f.clone()))
        }
        .ok_or_else(stale)?;
        let page = self.page(task_id).await.map_err(LocateError::Api)?;
        let frames = page
            .frames()
            .await
            .map_err(|e| LocateError::Api(cdp_err(e)))?;
        if !frames.iter().any(|f| f.id == frame) {
            return Err(stale());
        }
        let fid = if frame.inner() == page.id() {
            None
        } else {
            Some(frame)
        };
        let finder = ref_finder_js();
        for attempt in 0..3 {
            match page
                .eval_obj(fid.as_ref(), &finder, vec![Value::String(r.clone())])
                .await
            {
                Ok(Some(obj)) => {
                    return Ok(ElementRef {
                        page,
                        frame: fid,
                        obj,
                    })
                }
                Ok(None) => {}
                Err(e) => return Err(LocateError::NotFound(err_text(&e))),
            }
            if attempt < 2 {
                tokio::time::sleep(Duration::from_millis(250)).await;
            }
        }
        Err(LocateError::NotFound(not_found_msg(&r)))
    }
}

const EDITABLE_IN_JS: &str = r#"(el) => {
  const ok = el.tagName === 'INPUT' || el.tagName === 'TEXTAREA' || el.isContentEditable || el.getAttribute('role') === 'textbox' || el.getAttribute('role') === 'searchbox';
  if (ok) return el;
  const vis = e => { const r = e.getBoundingClientRect(); const s = getComputedStyle(e); return r.width > 0 && r.height > 0 && s.visibility !== 'hidden' && s.display !== 'none'; };
  const inner = Array.from(el.querySelectorAll('input:not([type=hidden]), textarea, [contenteditable=true]')).find(vis);
  return inner || el;
}"#;

const FILL_JS: &str = r#"(el, value) => {
  const kinds = ['color', 'date', 'time', 'datetime-local', 'month', 'range', 'week'];
  const tag = el.tagName;
  const fire = () => { el.dispatchEvent(new Event('input', {bubbles: true})); el.dispatchEvent(new Event('change', {bubbles: true})); };
  if (tag === 'INPUT' || tag === 'TEXTAREA') {
    if (el.disabled) throw new Error('Element is not enabled');
    if (el.readOnly) throw new Error('Element is readonly');
    const t = (el.type || '').toLowerCase();
    if (tag === 'INPUT' && kinds.includes(t)) { el.focus(); el.value = value; if (el.value !== value) throw new Error('Malformed value'); fire(); return 'done'; }
    el.focus(); el.select();
    if (!value) { el.value = ''; fire(); return 'done'; }
    return 'insert';
  }
  if (el.isContentEditable) {
    el.focus();
    const r = document.createRange(); r.selectNodeContents(el); const s = getSelection(); s.removeAllRanges(); s.addRange(r);
    if (!value) { document.execCommand('delete', false); return 'done'; }
    return 'insert';
  }
  throw new Error('Element is not an <input>, <textarea> or [contenteditable] element');
}"#;

const SELECT_JS: &str = r#"(el, v, byLabel) => {
  if (el.tagName !== 'SELECT') throw new Error('Element is not a <select> element');
  const o = Array.from(el.options).find(o => byLabel ? (o.label === v || o.text === v || o.text.trim() === v) : o.value === v);
  if (!o) throw new Error('did not find some options');
  el.value = o.value;
  el.dispatchEvent(new Event('input', {bubbles: true})); el.dispatchEvent(new Event('change', {bubbles: true}));
  return o.value;
}"#;

/// The `fill` path of `type`: Playwright's `locator.fill(text)`.
async fn fill(el: &ElementRef, text: &str) -> Result<(), String> {
    let r = el
        .eval(FILL_JS, vec![json!(text)])
        .await
        .map_err(|e| err_text(&e))?;
    if r == json!("insert") {
        keys::insert_text(&el.page, text)
            .await
            .map_err(|e| err_text(&e))?;
    }
    Ok(())
}

/// Click, select all, delete, then type key by key (`keys=True` and the fill fallback).
async fn clear_and_type(el: &ElementRef, text: &str, delay: u64) -> Result<(), String> {
    el.click(Duration::from_secs(5)).await?;
    keys::press(&el.page, "Control+a")
        .await
        .map_err(|e| err_text(&e))?;
    keys::press(&el.page, "Backspace")
        .await
        .map_err(|e| err_text(&e))?;
    keys::type_text(&el.page, text, delay)
        .await
        .map_err(|e| err_text(&e))?;
    Ok(())
}

async fn field_hint(el: &ElementRef) -> String {
    match el
        .eval("e => [e.placeholder, e.getAttribute('aria-label'), e.maxLength > 0 ? 'maxlength' + e.maxLength : ''].filter(Boolean).join(' ')", vec![])
        .await
    {
        Ok(Value::String(s)) => s,
        _ => String::new(),
    }
}

fn body_task(body: &Value) -> String {
    let t = get_str(body, "task_id");
    if t.is_empty() {
        "default".to_string()
    } else {
        t
    }
}

impl Broker {
    /// `POST /agent/{action}` — the whole dispatcher.
    pub async fn agent_action(&self, action: &str, body: &Value) -> Result<Value, ApiError> {
        let task_id = body_task(body);
        self.ensure_ctx().await?; // every agent web action needs the persistent browser (lazy start)
        self.touch();
        match action {
            "describe" => return self.describe(&task_id, &get_str(body, "ref")).await,
            "page_text" => {
                let page = self.page(&task_id).await?;
                let txt = page
                    .eval(
                        None,
                        "() => (document.body && document.body.innerText || '').slice(0, 60000)",
                        vec![],
                    )
                    .await
                    .ok()
                    .and_then(|v| v.as_str().map(|s| s.to_string()))
                    .unwrap_or_default();
                return Ok(json!({"url": page.url(), "text": txt}));
            }
            "focused" => return self.focused(&task_id).await,
            "describe_at" => {
                let x = body.get("x").and_then(as_f64).unwrap_or(0.0);
                let y = body.get("y").and_then(as_f64).unwrap_or(0.0);
                return self.at(&task_id, x, y).await;
            }
            "request_takeover" => {
                let reason = cut(&get_str(body, "reason"), 300);
                self.st.lock().requests.insert(
                    task_id.clone(),
                    json!({"task_id": task_id, "reason": reason, "ts": now_ts()}),
                );
                self.page(&task_id).await?;
                return Ok(json!({"ok": true, "status": "takeover_requested"}));
            }
            "release" => {
                let mut st = self.st.lock();
                if st.pages.contains_key(&task_id) {
                    st.finished.insert(task_id.clone(), now_ts());
                }
                st.requests.shift_remove(&task_id);
                return Ok(json!({"ok": true}));
            }
            "search" => {
                let q = cut(&get_str(body, "query"), 300);
                let n = body
                    .get("max_results")
                    .filter(|v| truthy(v))
                    .and_then(as_i64)
                    .unwrap_or(8);
                return self.web_search(&q, n).await;
            }
            "read" => {
                let mut urls: Vec<String> = match body.get("urls") {
                    Some(Value::Array(a)) if !a.is_empty() => {
                        a.iter().map(omuse_common::util::py_str).collect()
                    }
                    Some(Value::String(s)) if !s.is_empty() => vec![s.clone()],
                    _ => match body.get("url") {
                        Some(u) if truthy(u) => vec![omuse_common::util::py_str(u)],
                        _ => vec![],
                    },
                };
                urls.retain(|u| !u.trim().is_empty());
                urls.truncate(4);
                return self.web_read(&urls, 6000).await;
            }
            _ => {}
        }
        self.check_agent(&task_id)?;
        self.st.lock().requests.shift_remove(&task_id); // the task is working again, so it is no longer waiting for the user
        if matches!(action, "find" | "look" | "locate") {
            let lock = self.task_lock(&task_id);
            let _g = lock.lock().await;
            return match action {
                "find" => {
                    self.find(&task_id, &cut(&get_str(body, "query"), 200))
                        .await
                }
                "locate" => {
                    let region = body.get("region").filter(|v| truthy(v)).cloned();
                    let mark = body.get("mark").filter(|v| truthy(v)).cloned();
                    if let Some(m) = mark {
                        self.grid(&task_id, None, 0, 0, "letters", Some(m)).await
                    } else if let Some(r) = region {
                        self.grid(&task_id, Some(r), 8, 6, "numbers", None).await
                    } else {
                        self.grid(&task_id, None, 8, 6, "letters", None).await
                    }
                }
                _ => self.look(&task_id).await,
            };
        }
        let lock = self.task_lock(&task_id);
        let _g = lock.lock().await;
        let mut page = self.page(&task_id).await?;
        let mut saved = Value::Null;
        match action {
            "navigate" => {
                let mut url = get_str(body, "url").trim().to_string();
                if !url.is_empty() && !url.contains("://") {
                    url = format!("https://{}", url);
                }
                let (ok, why) = check_url(&url);
                if !ok {
                    return Err(ApiError::new(403, why));
                }
                self.pace(&url).await;
                match page.navigate(&url, Duration::from_secs(45)).await {
                    Ok(status) => {
                        self.st.lock().nav_status.insert(task_id.clone(), status);
                    }
                    Err(e) => {
                        let msg = e.to_string();
                        if msg.contains("ERR_BLOCKED_BY_CLIENT") {
                            return Err(ApiError::new(
                                403,
                                "该地址被安全策略拦截 (blocked by egress policy)",
                            ));
                        }
                        // a plain load timeout is often transient: wait a moment and try once more before giving up
                        if msg.contains("imeout") {
                            tokio::time::sleep(Duration::from_secs(2)).await;
                            match page.navigate(&url, Duration::from_secs(45)).await {
                                Ok(status) => {
                                    self.st.lock().nav_status.insert(task_id.clone(), status);
                                }
                                Err(e2) => {
                                    return Err(ApiError::new(
                                        502,
                                        format!(
                                            "打开页面失败 navigation failed (retried once): {}",
                                            cut(&e2.to_string(), 180)
                                        ),
                                    ));
                                }
                            }
                        } else {
                            return Err(ApiError::new(
                                502,
                                format!("打开页面失败 navigation failed: {}", cut(&msg, 200)),
                            ));
                        }
                    }
                }
                self.settle(&page, 1200).await;
                // a PDF opens in Chromium's viewer, which neither the snapshot nor the downloads list can use:
                // keep a copy in downloads/ so files_read and pdf_form_fields can work with it
                if page.nav.lock().mime.as_deref() == Some("application/pdf") {
                    saved = match self.save_pdf(&page, &page.url()).await {
                        Ok(v) => v,
                        Err(e) => json!({"error": e.to_string()}),
                    };
                }
            }
            "snapshot" => {}
            "click" => {
                let r = get_str(body, "ref");
                let el = match self.locate(&task_id, &r).await {
                    Ok(el) => el,
                    Err(LocateError::Api(e)) => return Err(e),
                    Err(e) => {
                        return Err(ApiError::new(
                            409,
                            format!("点击失败 click failed: {} / {}", cut(&e.text(), 160), ""),
                        ))
                    }
                };
                if let Err(first) = el.click(Duration::from_millis(1500)).await {
                    // 2026-10-04 chope.co "Book Now": the button was covered by another layer and every click timed out.
                    // Scroll it into view and try again; if something still covers it, click the element itself via script
                    // (it is the same element the agent chose, so the approval check already covered it).
                    let _ = el.page.scroll_into_view(&el.obj).await;
                    if el.click(Duration::from_millis(1000)).await.is_err() {
                        if let Err(e2) = el.eval("e => e.click()", vec![]).await {
                            let cover = if first.contains("intercepts pointer events") {
                                " (另一个元素挡住了它 another element covers it)"
                            } else {
                                ""
                            };
                            return Err(ApiError::new(
                                409,
                                format!(
                                    "点击失败 click failed{}: {} / {}",
                                    cover,
                                    cut(&first, 160),
                                    cut(&err_text(&e2), 80)
                                ),
                            ));
                        }
                    }
                }
                self.settle(&page, 1200).await;
                page = self.page(&task_id).await?;
            }
            "click_at" => {
                // visual click (browser_locate found the point): works for iframes, shadow DOM, canvas, unlabeled icons
                let x = body.get("x").and_then(as_f64).unwrap_or(-1.0);
                let y = body.get("y").and_then(as_f64).unwrap_or(-1.0);
                if !((0.0..=(VIEWPORT_W * 2) as f64).contains(&x)
                    && (0.0..=(VIEWPORT_H * 2) as f64).contains(&y))
                {
                    return Err(ApiError::new(
                        400,
                        "x/y must be viewport coordinates from browser_locate",
                    ));
                }
                page.click_at(x, y, false).await.api()?;
                self.settle(&page, 900).await;
                let text = get_str(body, "text");
                if !text.is_empty() {
                    keys::type_text(&page, &text, 15).await.api()?;
                    if get_truthy(body, "submit") {
                        keys::press(&page, "Enter").await.api()?;
                        self.settle(&page, 1500).await;
                    }
                }
                page = self.page(&task_id).await?;
            }
            "type" => {
                let r = get_str(body, "ref");
                let el = match self.locate(&task_id, &r).await {
                    Ok(el) => el,
                    Err(LocateError::Api(e)) => return Err(e),
                    Err(e) => {
                        return Err(ApiError::new(
                            409,
                            format!("输入失败 type failed: {}", cut(&e.text(), 200)),
                        ))
                    }
                };
                let mut text = get_str(body, "text");
                let res: Result<(), String> = async {
                    let inner = el
                        .page
                        .call_on_obj(&el.obj, EDITABLE_IN_JS, vec![])
                        .await
                        .ok()
                        .flatten();
                    let el = match inner {
                        Some(obj) => ElementRef {
                            page: el.page.clone(),
                            frame: el.frame.clone(),
                            obj,
                        },
                        None => ElementRef {
                            page: el.page.clone(),
                            frame: el.frame.clone(),
                            obj: el.obj.clone(),
                        },
                    };
                    // <input type=time/date/…> only accepts its ISO format: "12:00 PM" or "2026-10-03 12:00" fail
                    let kind = match el
                        .eval("e => (e.tagName === 'INPUT' ? e.type : '')", vec![])
                        .await
                    {
                        Ok(Value::String(s)) => s,
                        _ => String::new(),
                    };
                    text = normalize_date_input(&kind, &text);
                    if get_truthy(body, "expiry") {
                        text = expiry_for_field(&text, &field_hint(&el).await);
                    }
                    if get_truthy(body, "keys") {
                        // card fields (Adyen, Stripe…) format and validate on key events; fill() leaves them "incomplete"
                        clear_and_type(&el, &text, 60).await?;
                    } else if fill(&el, &text).await.is_err() {
                        // clear first: typing into a field that still holds "Hartford" gives "HartfordTokyo"
                        clear_and_type(&el, &text, 15).await?;
                    }
                    if get_truthy(body, "submit") {
                        el.focus().await.map_err(|e| err_text(&e))?;
                        keys::press(&el.page, "Enter")
                            .await
                            .map_err(|e| err_text(&e))?;
                        self.settle(&el.page, 1500).await;
                    }
                    Ok(())
                }
                .await;
                if let Err(e) = res {
                    return Err(ApiError::new(
                        409,
                        format!("输入失败 type failed: {}", cut(&e, 200)),
                    ));
                }
            }
            "select" => {
                let r = get_str(body, "ref");
                let v = get_str(body, "value");
                let el = match self.locate(&task_id, &r).await {
                    Ok(el) => el,
                    Err(LocateError::Api(e)) => return Err(e),
                    Err(e) => {
                        return Err(ApiError::new(
                            409,
                            format!("选择失败 select failed: {}", cut(&e.text(), 200)),
                        ))
                    }
                };
                if el
                    .eval(SELECT_JS, vec![json!(v), json!(true)])
                    .await
                    .is_err()
                {
                    if let Err(e) = el.eval(SELECT_JS, vec![json!(v), json!(false)]).await {
                        return Err(ApiError::new(
                            409,
                            format!("选择失败 select failed: {}", cut(&err_text(&e), 200)),
                        ));
                    }
                }
                self.settle(&page, 500).await;
            }
            "press" => {
                static KEY_RE: LazyLock<Regex> =
                    LazyLock::new(|| Regex::new(r"^[A-Za-z0-9+]{1,24}$").unwrap());
                let key = match body.get("key") {
                    Some(k) if !k.is_null() => omuse_common::util::py_str(k),
                    _ => "Enter".to_string(),
                };
                if !KEY_RE.is_match(&key) {
                    return Err(ApiError::new(400, "invalid key"));
                }
                keys::press(&page, &key).await.api()?;
                self.settle(&page, 1000).await;
            }
            "scroll" => {
                let d = match body.get("direction") {
                    Some(v) if !v.is_null() => omuse_common::util::py_str(v),
                    _ => "down".to_string(),
                };
                let (js, dy) = match d.as_str() {
                    "down" => ("window.scrollBy(0, window.innerHeight*0.85)", 700.0),
                    "up" => ("window.scrollBy(0, -window.innerHeight*0.85)", -700.0),
                    "top" => ("window.scrollTo(0,0)", -100000.0),
                    "bottom" => ("window.scrollTo(0, document.body.scrollHeight)", 100000.0),
                    _ => return Err(ApiError::new(400, "direction must be up/down/top/bottom")),
                };
                let y0 = page
                    .eval(None, "() => window.scrollY", vec![])
                    .await
                    .api()?;
                page.wheel(0.0, dy).await.api()?;
                tokio::time::sleep(Duration::from_millis(500)).await;
                let y1 = page
                    .eval(None, "() => window.scrollY", vec![])
                    .await
                    .api()?;
                if y1 == y0 {
                    // wheel went to an inner element (or nowhere)
                    page.eval(None, &format!("() => {{ {}; }}", js), vec![])
                        .await
                        .api()?;
                    tokio::time::sleep(Duration::from_millis(300)).await;
                }
            }
            "back" => {
                page.go_back(Duration::from_secs(20)).await.api()?;
                self.settle(&page, 800).await;
            }
            "wait" => {
                let secs = body
                    .get("seconds")
                    .and_then(as_i64)
                    .unwrap_or(5)
                    .clamp(1, 120);
                tokio::time::sleep(Duration::from_secs(secs as u64)).await;
            }
            "save_media" => {
                saved = self
                    .save_media(
                        &task_id,
                        &page,
                        &get_str(body, "ref"),
                        &get_str(body, "url"),
                        &get_str(body, "name"),
                    )
                    .await?;
            }
            "upload" => {
                let rel = get_str(body, "path");
                let full = self.workspace.join(rel.trim_start_matches('/'));
                let full = std::fs::canonicalize(&full).unwrap_or(full);
                let ws = format!("{}/", self.workspace.display());
                if !full.to_string_lossy().starts_with(&ws) || !full.is_file() {
                    return Err(ApiError::new(
                        400,
                        "文件必须位于工作区内 (file must exist inside the workspace)",
                    ));
                }
                if full.to_string_lossy().contains("/.quarantine/") {
                    return Err(ApiError::new(403, "隔离区文件不能上传 (quarantined file)"));
                }
                let r = get_str(body, "ref");
                let el = match self.locate(&task_id, &r).await {
                    Ok(el) => el,
                    Err(LocateError::Api(e)) => return Err(e),
                    Err(e) => return Err(browser_error(&e.text())),
                };
                let input = el
                    .page
                    .call_on_obj(
                        &el.obj,
                        "el => (el.tagName === 'INPUT' && (el.type || '').toLowerCase() === 'file') ? el : (el.querySelector ? el.querySelector('input[type=file]') : null)",
                        vec![],
                    )
                    .await
                    .api()?
                    .ok_or_else(|| browser_error("Node is not an HTMLInputElement"))?;
                let mut p = SetFileInputFilesParams::new(vec![full.to_string_lossy().to_string()]);
                p.object_id = Some(input);
                page.page.execute(p).await.api()?;
                self.settle(&page, 500).await;
            }
            _ => return Err(ApiError::new(404, format!("unknown action {}", action))),
        }
        if action == "save_media" {
            return Ok(json!({"saved": saved, "url": page.url(), "title": page.title().await}));
        }
        let near = action == "scroll" || get_truthy(body, "near");
        let max_chars = body.get("max_chars").and_then(as_i64).unwrap_or(12000);
        let mut snap = self.snapshot(&task_id, max_chars, near).await?;
        if !saved.is_null() {
            if let Some(o) = snap.as_object_mut() {
                o.insert("pdf".into(), saved);
            }
        }
        if matches!(
            action,
            "navigate" | "click" | "snapshot" | "back" | "wait" | "press"
        ) {
            let status = if action == "navigate" {
                self.st
                    .lock()
                    .nav_status
                    .get(&task_id)
                    .copied()
                    .unwrap_or(0)
            } else {
                0
            };
            let blk = page
                .eval(None, BLOCK_JS, vec![json!(status)])
                .await
                .ok()
                .filter(truthy);
            if let Some(b) = blk {
                let url = get_str(&snap, "url");
                let url = if url.is_empty() { page.url() } else { url };
                self.note_block(&url);
                if let Some(o) = snap.as_object_mut() {
                    o.insert("blocked".into(), b);
                }
            }
        }
        Ok(snap)
    }

    /// `POST /user/input` body handler (`_user_input`).
    pub async fn user_input(&self, body: &Value) -> Result<Value, ApiError> {
        // always the page being taken over — never whatever another (still running) task last touched
        let tt = {
            let st = self.st.lock();
            if st.takeover_task.is_empty() {
                "default".to_string()
            } else {
                st.takeover_task.clone()
            }
        };
        let existing = self
            .st
            .lock()
            .pages
            .get(&tt)
            .cloned()
            .filter(|p| !p.is_closed());
        let page = match existing {
            Some(p) => p,
            None => self.page(&tt).await?,
        };
        let kind = get_str(body, "type");
        let x = body.get("x").and_then(as_f64).unwrap_or(0.0);
        let y = body.get("y").and_then(as_f64).unwrap_or(0.0);
        match kind.as_str() {
            "click" | "dblclick" => {
                if let Err(e) = page.click_at(x, y, kind == "dblclick").await {
                    // the click itself closed the page (a sign-in popup's "Next" / "Allow" button): that is a successful click
                    let msg = err_text(&e);
                    if !page.is_closed() && !msg.to_lowercase().contains("closed") {
                        return Err(ApiError::new(
                            409,
                            format!("点击失败 click failed: {}", cut(&msg, 150)),
                        ));
                    }
                }
            }
            "wheel" => {
                page.mouse_move(x, y).await.api()?;
                let dx = body.get("dx").and_then(as_f64).unwrap_or(0.0);
                let dy = body.get("dy").and_then(as_f64).unwrap_or(0.0);
                page.wheel(dx, dy).await.api()?;
            }
            "text" => {
                let t = cut(&get_str(body, "text"), 2000);
                keys::type_text(&page, &t, 10).await.api()?;
            }
            "key" => {
                static KEY_RE: LazyLock<Regex> =
                    LazyLock::new(|| Regex::new(r"^[A-Za-z0-9+]{1,30}$").unwrap());
                let key = get_str(body, "key");
                if KEY_RE.is_match(&key) {
                    keys::press(&page, &key).await.api()?;
                }
            }
            "navigate" => {
                let mut url = get_str(body, "url").trim().to_string();
                if !url.is_empty() && !url.contains("://") {
                    url = format!("https://{}", url);
                }
                let (ok, why) = check_url(&url);
                if !ok {
                    return Err(ApiError::new(403, why));
                }
                if let Err(e) = page.navigate(&url, Duration::from_secs(45)).await {
                    return Err(ApiError::new(502, cut(&e.to_string(), 200)));
                }
            }
            "back" => page.go_back(Duration::from_secs(30)).await.api()?,
            "reload" => page.reload().await.api()?,
            _ => return Err(ApiError::new(400, "unknown input type")),
        }
        if kind != "wheel" {
            tokio::time::sleep(Duration::from_millis(150)).await;
        }
        Ok(json!({"ok": true}))
    }
}
