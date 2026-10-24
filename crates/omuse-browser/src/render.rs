//! Local rendering with a separate small headless Chromium (not the user's browser profile), JavaScript off and
//! every network request blocked: `/pdf`, `/png` and `/compose` of `app/browser/main.py`.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, LazyLock};
use std::time::{Duration, Instant};

use base64::Engine;
use chromiumoxide::cdp::browser_protocol::emulation::{
    SetDeviceMetricsOverrideParams, SetLocaleOverrideParams, SetScriptExecutionDisabledParams,
};
use chromiumoxide::cdp::browser_protocol::fetch::{
    ContinueRequestParams, EnableParams as FetchEnableParams, EventRequestPaused,
    FailRequestParams, RequestPattern,
};
use chromiumoxide::cdp::browser_protocol::network::ErrorReason;
use chromiumoxide::cdp::browser_protocol::page::{PrintToPdfParams, SetDocumentContentParams};
use chromiumoxide::cdp::browser_protocol::target::CreateTargetParams;
use chromiumoxide::Browser;
use futures::StreamExt;
use omuse_common::api::ApiError;
use omuse_common::util::{as_i64, get_str, take_chars, truthy};
use omuse_common::Config;
use regex::Regex;
use serde_json::{json, Value};
use tokio::task::JoinHandle;

use crate::broker::{browser_error, cdp_err};
use crate::downloads::rel_ws;
use crate::launch;
use crate::pages::{err_text, Clip, TaskPage};
use crate::textfmt::cut;
use omuse_docs::mdhtml;

const PDF_MAX_SRC: u64 = 5_000_000;

struct RenderChrome {
    browser: Browser,
    handler: JoinHandle<()>,
    dead: Arc<AtomicBool>,
}

/// `_pdf = {"browser": None, "lock": asyncio.Lock()}`.
pub struct Renderer {
    cfg: Arc<Config>,
    workspace: PathBuf,
    inner: tokio::sync::Mutex<Option<RenderChrome>>,
}

/// `_ws(rel)`: a path inside the workspace (never the quarantine).
pub fn ws_path(workspace: &Path, rel: &str) -> Result<PathBuf, ApiError> {
    let rel = rel.trim().trim_start_matches('/');
    let joined = workspace.join(rel);
    let p = normalize(&joined);
    let ws = format!("{}/", workspace.display());
    if !p.to_string_lossy().starts_with(&ws) || p.to_string_lossy().contains("/.quarantine") {
        return Err(ApiError::new(
            400,
            "路径必须在工作区内 (path must be inside the workspace)",
        ));
    }
    Ok(p)
}

/// `os.path.realpath` without requiring the file to exist: resolve what exists, normalise the rest.
fn normalize(p: &Path) -> PathBuf {
    if let Ok(c) = std::fs::canonicalize(p) {
        return c;
    }
    let mut out = PathBuf::new();
    let mut comps: Vec<std::path::Component> = vec![];
    for c in p.components() {
        match c {
            std::path::Component::ParentDir => {
                comps.pop();
            }
            std::path::Component::CurDir => {}
            other => comps.push(other),
        }
    }
    for c in comps {
        out.push(c.as_os_str());
    }
    // resolve the longest existing prefix (symlinked /tmp etc.)
    let mut existing = out.clone();
    let mut rest = vec![];
    while !existing.exists() {
        match existing.file_name() {
            Some(f) => {
                rest.push(f.to_os_string());
                existing.pop();
            }
            None => return out,
        }
    }
    let mut res = std::fs::canonicalize(&existing).unwrap_or(existing);
    for r in rest.iter().rev() {
        res.push(r);
    }
    res
}

/// `_css_color(s, default)`.
pub fn css_color(s: &str, default: &str) -> String {
    static RE: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"^(#[0-9a-fA-F]{3,8}|[a-zA-Z]{3,20}|rgba?\([0-9., %]+\))$").unwrap()
    });
    let s = s.trim();
    if RE.is_match(s) {
        s.to_string()
    } else {
        default.to_string()
    }
}

/// `(width, height)` from the file header of a PNG / JPEG / GIF / WebP (`_image_size`).
pub fn image_size(path: &Path) -> Result<(u32, u32), String> {
    let data = std::fs::read(path).map_err(|e| e.to_string())?;
    let head = &data[..data.len().min(32)];
    let be32 = |b: &[u8]| u32::from_be_bytes([b[0], b[1], b[2], b[3]]);
    let le16 = |b: &[u8]| u16::from_le_bytes([b[0], b[1]]) as u32;
    let be16 = |b: &[u8]| u16::from_be_bytes([b[0], b[1]]) as u32;
    if head.len() >= 24 && head.starts_with(b"\x89PNG\r\n\x1a\n") {
        return Ok((be32(&head[16..20]), be32(&head[20..24])));
    }
    if head.len() >= 10 && (head.starts_with(b"GIF87a") || head.starts_with(b"GIF89a")) {
        return Ok((le16(&head[6..8]), le16(&head[8..10])));
    }
    if head.len() >= 16 && head.starts_with(b"RIFF") && &head[8..12] == b"WEBP" {
        let tag = &head[12..16];
        let chunk = &data[12..data.len().min(42)];
        if tag == b"VP8X" && chunk.len() >= 18 {
            let w = 1 + u32::from_le_bytes([chunk[12], chunk[13], chunk[14], 0]);
            let h = 1 + u32::from_le_bytes([chunk[15], chunk[16], chunk[17], 0]);
            return Ok((w, h));
        }
        if tag == b"VP8L" && chunk.len() >= 13 {
            let (b0, b1, b2, b3) = (
                chunk[9] as u32,
                chunk[10] as u32,
                chunk[11] as u32,
                chunk[12] as u32,
            );
            return Ok((
                1 + (((b1 & 0x3F) << 8) | b0),
                1 + (((b3 & 0xF) << 10) | (b2 << 2) | ((b1 & 0xC0) >> 6)),
            ));
        }
        if tag == b"VP8 " && chunk.len() >= 18 {
            return Ok((le16(&chunk[14..16]) & 0x3FFF, le16(&chunk[16..18]) & 0x3FFF));
        }
    }
    if head.len() >= 2 && head[0] == 0xFF && head[1] == 0xD8 {
        let mut i = 2;
        while i + 4 <= data.len() {
            if data[i] != 0xFF {
                break;
            }
            let m = data[i + 1];
            if m == 0xD8 || m == 0x01 || (0xD0..=0xD7).contains(&m) {
                i += 2;
                continue;
            }
            let ln = be16(&data[i + 2..i + 4]) as usize;
            if matches!(
                m,
                0xC0 | 0xC1
                    | 0xC2
                    | 0xC3
                    | 0xC5
                    | 0xC6
                    | 0xC7
                    | 0xC9
                    | 0xCA
                    | 0xCB
                    | 0xCD
                    | 0xCE
                    | 0xCF
            ) {
                if i + 9 <= data.len() {
                    return Ok((be16(&data[i + 7..i + 9]), be16(&data[i + 5..i + 7])));
                }
                break;
            }
            i += 2 + ln;
        }
    }
    Err("unsupported image format (png/jpeg/gif/webp only)".to_string())
}

/// `_img_data(base_dir)`: local workspace images become data URLs; remote ones are not fetched.
pub fn img_data<'a>(
    workspace: &'a Path,
    base_dir: &'a Path,
) -> impl Fn(&str) -> Option<String> + 'a {
    static SCHEME: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)^[a-z]+:").unwrap());
    move |src: &str| {
        if SCHEME.is_match(src) {
            return None;
        }
        let p = normalize(&base_dir.join(src));
        let ws = format!("{}/", workspace.display());
        if !p.to_string_lossy().starts_with(&ws) || !p.is_file() {
            return None;
        }
        let meta = std::fs::metadata(&p).ok()?;
        if meta.len() > 10_000_000 {
            return None;
        }
        let mt = mime_guess::from_path(&p).first_raw().unwrap_or("");
        if !["image/png", "image/jpeg", "image/gif", "image/webp"].contains(&mt) {
            return None;
        }
        let bytes = std::fs::read(&p).ok()?;
        Some(format!(
            "data:{};base64,{}",
            mt,
            base64::engine::general_purpose::STANDARD.encode(bytes)
        ))
    }
}

impl Renderer {
    pub fn new(cfg: Arc<Config>, workspace: PathBuf) -> Self {
        Self {
            cfg,
            workspace,
            inner: tokio::sync::Mutex::new(None),
        }
    }

    /// Shut the render browser (teardown / exit).
    pub async fn close(&self) {
        let mut g = self.inner.lock().await;
        if let Some(mut rc) = g.take() {
            let _ = tokio::time::timeout(Duration::from_secs(5), rc.browser.close()).await;
            let _ = rc.browser.kill().await;
            rc.handler.abort();
        }
    }

    async fn ensure<'a>(&self, g: &'a mut Option<RenderChrome>) -> Result<&'a Browser, ApiError> {
        if g.as_ref()
            .map(|rc| rc.dead.load(Ordering::SeqCst))
            .unwrap_or(true)
        {
            if let Some(mut old) = g.take() {
                let _ = old.browser.kill().await;
                old.handler.abort();
            }
            let exe = launch::find_executable(&self.cfg)
                .ok_or_else(|| browser_error("chromium executable not found"))?;
            let scratch = std::env::temp_dir().join(format!("omuse-render-{}", std::process::id()));
            let _ = std::fs::create_dir_all(&scratch);
            let config = launch::render_config(&exe, &scratch).map_err(|e| browser_error(&e))?;
            let (browser, mut handler) = Browser::launch(config).await.map_err(cdp_err)?;
            let dead = Arc::new(AtomicBool::new(false));
            let d2 = dead.clone();
            let task = tokio::spawn(async move {
                while let Some(h) = handler.next().await {
                    if h.is_err() {
                        break;
                    }
                }
                d2.store(true, Ordering::SeqCst);
            });
            *g = Some(RenderChrome {
                browser,
                handler: task,
                dead,
            });
        }
        Ok(&g.as_ref().unwrap().browser)
    }

    /// A fresh page with scripts off, requests blocked (except `data:` and, when asked, `file:`), the given
    /// locale and device scale.
    async fn render_page(
        browser: &Browser,
        allow_file: bool,
        locale: &str,
        scale: f64,
        size: Option<(u32, u32)>,
    ) -> Result<Arc<TaskPage>, ApiError> {
        let page = tokio::time::timeout(
            Duration::from_secs(30),
            browser.new_page(CreateTargetParams::new("about:blank")),
        )
        .await
        .map_err(|_| browser_error("new page timed out"))?
        .map_err(cdp_err)?;
        let tp = TaskPage::new(page, "about:blank".into());
        let p = &tp.page;
        p.execute(SetScriptExecutionDisabledParams::new(true))
            .await
            .map_err(cdp_err)?;
        if !locale.is_empty() {
            let _ = p
                .execute(SetLocaleOverrideParams {
                    locale: Some(locale.to_string()),
                })
                .await;
        }
        let (w, h) = size.unwrap_or((crate::textfmt::VIEWPORT_W, crate::textfmt::VIEWPORT_H));
        let _ = p
            .execute(SetDeviceMetricsOverrideParams::new(
                w as i64, h as i64, scale, false,
            ))
            .await;
        if let Ok(mut st) = p.event_listener::<EventRequestPaused>().await {
            let pg = p.clone();
            tokio::spawn(async move {
                while let Some(ev) = st.next().await {
                    let u = ev.request.url.to_lowercase();
                    let ok = u.starts_with("data:") || (allow_file && u.starts_with("file:"));
                    let pg = pg.clone();
                    let id = ev.request_id.clone();
                    tokio::spawn(async move {
                        if ok {
                            let _ = pg.execute(ContinueRequestParams::new(id)).await;
                        } else {
                            let _ = pg
                                .execute(FailRequestParams::new(id, ErrorReason::Aborted))
                                .await;
                        }
                    });
                }
            });
            let _ = p
                .execute(
                    FetchEnableParams::builder()
                        .pattern(RequestPattern::builder().url_pattern("*").build())
                        .build(),
                )
                .await;
        }
        Ok(tp)
    }

    /// `page.set_content(doc, wait_until="load")`.
    async fn set_content(tp: &TaskPage, html: &str) -> Result<(), ApiError> {
        tp.page
            .execute(SetDocumentContentParams::new(
                tp.main_frame(),
                html.to_string(),
            ))
            .await
            .map_err(cdp_err)?;
        Self::wait_loaded(tp).await;
        Ok(())
    }

    async fn wait_loaded(tp: &TaskPage) {
        let deadline = Instant::now() + Duration::from_secs(30);
        while Instant::now() < deadline {
            match tp
                .eval(None, "() => document.readyState === 'complete' && Array.from(document.images).every(i => i.complete)", vec![])
                .await
            {
                Ok(Value::Bool(true)) => return,
                Ok(_) => {}
                Err(_) => {
                    tokio::time::sleep(Duration::from_millis(300)).await;
                    return;
                }
            }
            tokio::time::sleep(Duration::from_millis(80)).await;
        }
    }

    async fn element_png(tp: &TaskPage, selector: &str, scale: f64) -> Result<Vec<u8>, ApiError> {
        let obj = tp
            .eval_obj(
                None,
                "(sel) => document.querySelector(sel)",
                vec![json!(selector)],
            )
            .await
            .map_err(cdp_err)?
            .ok_or_else(|| browser_error(&format!("element {} not found", selector)))?;
        let quads = tp.quads(&obj).await.map_err(cdp_err)?;
        let q = quads
            .first()
            .filter(|q| q.len() >= 8)
            .ok_or_else(|| browser_error("element is not visible"))?;
        let xs = [q[0], q[2], q[4], q[6]];
        let ys = [q[1], q[3], q[5], q[7]];
        let (x0, x1) = (
            xs.iter().cloned().fold(f64::MAX, f64::min),
            xs.iter().cloned().fold(f64::MIN, f64::max),
        );
        let (y0, y1) = (
            ys.iter().cloned().fold(f64::MAX, f64::min),
            ys.iter().cloned().fold(f64::MIN, f64::max),
        );
        tp.screenshot(
            false,
            None,
            Some(Clip {
                x: x0,
                y: y0,
                width: (x1 - x0).max(1.0),
                height: (y1 - y0).max(1.0),
                scale,
            }),
            Duration::from_secs(30),
        )
        .await
        .map_err(cdp_err)
    }

    /// `_print_pdf(doc)`.
    pub async fn print_pdf(&self, doc: &str) -> Result<Vec<u8>, ApiError> {
        let mut g = self.inner.lock().await;
        let browser = self.ensure(&mut g).await?;
        let tp = Self::render_page(browser, false, "zh-CN", 1.0, None).await?;
        let res: Result<Vec<u8>, ApiError> = async {
            Self::set_content(&tp, doc).await?;
            let footer = "<div style=\"width:100%;font-size:8px;color:#9ca3af;text-align:center;font-family:sans-serif\">\
                          <span class=\"pageNumber\"></span> / <span class=\"totalPages\"></span></div>";
            let p = PrintToPdfParams {
                paper_width: Some(8.27),
                paper_height: Some(11.69),
                print_background: Some(true),
                prefer_css_page_size: Some(true),
                display_header_footer: Some(true),
                header_template: Some("<div></div>".to_string()),
                footer_template: Some(footer.to_string()),
                margin_top: Some(18.0 / 25.4),
                margin_bottom: Some(20.0 / 25.4),
                margin_left: Some(16.0 / 25.4),
                margin_right: Some(16.0 / 25.4),
                ..Default::default()
            };
            let r = tokio::time::timeout(Duration::from_secs(60), tp.page.execute(p)).await.map_err(|_| browser_error("pdf timed out"))?.map_err(cdp_err)?;
            base64::engine::general_purpose::STANDARD
                .decode(AsRef::<[u8]>::as_ref(&r.result.data))
                .map_err(|e| browser_error(&e.to_string()))
        }
        .await;
        tp.close().await;
        res
    }

    /// `POST /pdf`.
    pub async fn make_pdf(&self, b: &Value) -> Result<Value, ApiError> {
        let src_rel = get_str(b, "source");
        let mut title = get_str(b, "title");
        let ws = &self.workspace;
        let mut base = ws.clone();
        let (text, ext) = if !src_rel.is_empty() {
            let sp = ws_path(ws, &src_rel)?;
            if !sp.is_file() {
                return Err(ApiError::new(
                    404,
                    format!("文件不存在 file not found: {}", src_rel),
                ));
            }
            if std::fs::metadata(&sp).map(|m| m.len()).unwrap_or(0) > PDF_MAX_SRC {
                return Err(ApiError::new(400, "源文件太大 (source over 5 MB)"));
            }
            let bytes = std::fs::read(&sp).map_err(|e| browser_error(&e.to_string()))?;
            let text = String::from_utf8_lossy(&bytes).to_string();
            base = sp.parent().map(|p| p.to_path_buf()).unwrap_or(base);
            let ext = sp
                .extension()
                .map(|e| format!(".{}", e.to_string_lossy().to_lowercase()))
                .unwrap_or_default();
            (text, ext)
        } else {
            let text = get_str(b, "markdown");
            if text.trim().is_empty() {
                return Err(ApiError::new(400, "没有内容 (nothing to print)"));
            }
            (text, ".md".to_string())
        };
        if title.is_empty() {
            static H1: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?m)^#\s+(.+)$").unwrap());
            title = match H1.captures(&text) {
                Some(c) => c[1].trim().to_string(),
                None => {
                    if src_rel.is_empty() {
                        "Document".to_string()
                    } else {
                        Path::new(&src_rel)
                            .file_stem()
                            .map(|s| s.to_string_lossy().to_string())
                            .unwrap_or_default()
                    }
                }
            };
        }
        let doc = match ext.as_str() {
            ".html" | ".htm" => text.clone(), // printed with JavaScript off and no network, so scripts / trackers in it do nothing
            ".md" | ".markdown" | ".txt" | "" => {
                let body = if ext == ".txt" {
                    format!("<pre>{}</pre>", html_escape::encode_safe(&text))
                } else {
                    let resolver = img_data(ws, &base);
                    mdhtml::md_to_html(&text, Some(&resolver))
                };
                mdhtml::page(&title, &body)
            }
            _ => {
                return Err(ApiError::new(
                    400,
                    format!(
                        "不支持的格式 unsupported source type: {}（支持 .md .txt .html）",
                        ext
                    ),
                ))
            }
        };
        let mut out_rel = get_str(b, "output");
        if out_rel.is_empty() {
            static BAD: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[^\w\-一-鿿]+").unwrap());
            out_rel = if !src_rel.is_empty() {
                match src_rel.rfind('.') {
                    Some(i) if i > src_rel.rfind('/').map(|j| j + 1).unwrap_or(0) => {
                        src_rel[..i].to_string()
                    }
                    _ => src_rel.clone(),
                }
            } else {
                format!("reports/{}", take_chars(&BAD.replace_all(&title, "_"), 60))
            } + ".pdf";
        }
        if !out_rel.to_lowercase().ends_with(".pdf") {
            out_rel.push_str(".pdf");
        }
        let op = ws_path(ws, &out_rel)?;
        let data = self.print_pdf(&doc).await?;
        if let Some(d) = op.parent() {
            std::fs::create_dir_all(d).map_err(|e| browser_error(&e.to_string()))?;
        }
        std::fs::write(&op, &data).map_err(|e| browser_error(&e.to_string()))?;
        Ok(json!({"path": rel_ws(&op, ws), "size": data.len(), "title": title}))
    }

    /// `POST /png`: an SVG (a chart made by the runtime) to a PNG in the workspace.
    pub async fn make_png(&self, b: &Value) -> Result<Value, ApiError> {
        let svg = get_str(b, "svg");
        if !svg.trim_start().starts_with("<svg") || svg.len() > 3_000_000 {
            return Err(ApiError::new(400, "需要 SVG (svg required, max 3 MB)"));
        }
        let mut out_rel = get_str(b, "output");
        if out_rel.is_empty() {
            out_rel = "charts/chart.png".to_string();
        }
        if !out_rel.to_lowercase().ends_with(".png") {
            out_rel.push_str(".png");
        }
        let op = ws_path(&self.workspace, &out_rel)?;
        let data = {
            let mut g = self.inner.lock().await;
            let browser = self.ensure(&mut g).await?;
            let tp = Self::render_page(browser, false, "zh-CN", 2.0, None).await?;
            let res: Result<Vec<u8>, ApiError> = async {
                Self::set_content(&tp, &format!("<!doctype html><html><body style='margin:0;background:#fff'>{}</body></html>", svg)).await?;
                Self::element_png(&tp, "svg", 1.0).await
            }
            .await;
            tp.close().await;
            res?
        };
        if let Some(d) = op.parent() {
            std::fs::create_dir_all(d).map_err(|e| browser_error(&e.to_string()))?;
        }
        std::fs::write(&op, &data).map_err(|e| browser_error(&e.to_string()))?;
        Ok(json!({"path": rel_ws(&op, &self.workspace), "size": data.len()}))
    }

    /// `POST /compose`: overlay exact text on a workspace image and save a PNG.
    pub async fn compose(&self, b: &Value) -> Result<Value, ApiError> {
        let ws = &self.workspace;
        let src = ws_path(ws, &get_str(b, "image"))?;
        if !src.is_file() {
            return Err(ApiError::new(404, "图片不存在 image not found"));
        }
        let text = get_str(b, "text").trim().to_string();
        let tlen = text.chars().count();
        if text.is_empty() || tlen > 400 {
            return Err(ApiError::new(
                400,
                "需要 1-400 字的文字 text required (1-400 chars)",
            ));
        }
        let mut pos = get_str(b, "position").to_lowercase();
        if pos.is_empty() {
            pos = "bottom".into();
        }
        if ![
            "top",
            "bottom",
            "center",
            "top-left",
            "top-right",
            "bottom-left",
            "bottom-right",
        ]
        .contains(&pos.as_str())
        {
            pos = "bottom".into();
        }
        let mut size = b.get("size").and_then(as_i64).unwrap_or(0);
        if size != 0 {
            size = size.clamp(12, 400); // a given size is clamped; 0 = auto-size below
        }
        let color = css_color(&get_str(b, "color"), "#ffffff");
        let band_v = b.get("band").cloned().unwrap_or(Value::Null);
        let mut band = if truthy(&band_v) {
            css_color(&omuse_common::util::py_str(&band_v), "")
        } else {
            String::new()
        };
        if band_v == Value::Bool(true) {
            band = "rgba(0,0,0,0.45)".into();
        }
        let mut weight = get_str(b, "weight");
        if weight.is_empty() {
            weight = "700".into();
        }
        if !["300", "400", "500", "600", "700", "800", "900"].contains(&weight.as_str()) {
            weight = "700".into();
        }
        let mut align = get_str(b, "align").to_lowercase();
        if align.is_empty() {
            align = if ["top", "bottom", "center"].contains(&pos.as_str()) {
                "center".into()
            } else if pos.contains("left") {
                "left".into()
            } else {
                "right".into()
            };
        }
        if !["left", "center", "right"].contains(&align.as_str()) {
            align = "center".into();
        }
        let font = take_chars(get_str(b, "font").trim(), 80).to_string();
        let pad = b
            .get("padding")
            .filter(|v| truthy(v))
            .and_then(as_i64)
            .unwrap_or(24)
            .clamp(0, 200);
        let mut out_rel = get_str(b, "output");
        if out_rel.is_empty() {
            let rel = rel_ws(&src, ws);
            let stem = match rel.rfind('.') {
                Some(i) if i > rel.rfind('/').map(|j| j + 1).unwrap_or(0) => rel[..i].to_string(),
                _ => rel,
            };
            out_rel = format!("{}_text.png", stem);
        }
        if !out_rel.to_lowercase().ends_with(".png") {
            out_rel.push_str(".png");
        }
        let op = ws_path(ws, &out_rel)?;
        // size the text to the picture when not given: ~1/12 of the shorter side, smaller for long text
        let (w, h) = image_size(&src).map_err(|e| {
            ApiError::new(
                400,
                format!("无法读取图片 cannot read image: {}", cut(&e, 100)),
            )
        })?;
        if size == 0 {
            let f = if tlen <= 16 {
                1.0
            } else if tlen <= 40 {
                0.75
            } else {
                0.55
            };
            size = std::cmp::max(14, (w.min(h) as f64 / 12.0 * f) as i64);
        }
        let v = if pos.starts_with("top") {
            "top"
        } else if pos.starts_with("bottom") {
            "bottom"
        } else {
            "center"
        };
        let just = match v {
            "top" => "flex-start",
            "bottom" => "flex-end",
            _ => "center",
        };
        let fam = if font.is_empty() {
            String::new()
        } else {
            format!("{}, ", html_escape::encode_safe(&font))
        };
        let cap_bg = if band.is_empty() {
            String::new()
        } else {
            format!("background:{};", band)
        };
        let shadow = if band.is_empty() {
            "0 2px 6px rgba(0,0,0,.6), 0 0 2px rgba(0,0,0,.8)"
        } else {
            "none"
        };
        let page_html = format!(
            "<!doctype html><html><head><meta charset=\"utf-8\"><style>\n\
html,body{{margin:0;background:#fff}}\n\
#w{{position:relative;width:{w}px;height:{h}px;overflow:hidden}}\n\
#w img{{display:block;width:{w}px;height:{h}px}}\n\
#c{{position:absolute;left:0;top:0;width:{w}px;height:{h}px;display:flex;flex-direction:column;justify-content:{just};align-items:stretch;box-sizing:border-box;padding:{pad}px}}\n\
#t{{{cap_bg}color:{color};font:{weight} {size}px/1.25 {fam}\"Noto Sans CJK SC\",\"Noto Sans SC\",\"PingFang SC\",\"Microsoft YaHei\",\"Helvetica Neue\",Arial,sans-serif;text-align:{align};padding:{py}px {px}px;white-space:pre-wrap;word-break:break-word;text-shadow:{shadow};border-radius:{radius}px}}\n\
</style></head><body><div id=\"w\"><img src=\"file://{src}\"><div id=\"c\"><div id=\"t\">{text}</div></div></div></body></html>",
            w = w,
            h = h,
            just = just,
            pad = pad,
            cap_bg = cap_bg,
            color = color,
            weight = weight,
            size = size,
            fam = fam,
            align = align,
            py = (size as f64 * 0.35) as i64,
            px = (size as f64 * 0.6) as i64,
            shadow = shadow,
            radius = (size as f64 * 0.25) as i64,
            src = src.display(),
            text = html_escape::encode_safe(&text),
        );
        let tmp = std::env::temp_dir().join(format!(
            "compose_{}_{}.html",
            (omuse_common::util::now_ts() * 1000.0) as i64,
            std::process::id()
        ));
        std::fs::write(&tmp, &page_html).map_err(|e| browser_error(&e.to_string()))?;
        let data = {
            let mut g = self.inner.lock().await;
            let res: Result<Vec<u8>, ApiError> = async {
                let browser = self.ensure(&mut g).await?;
                let tp = Self::render_page(browser, true, "", 1.0, Some((w, h))).await?;
                let r: Result<Vec<u8>, ApiError> = async {
                    // Page.navigate (a renderer may not move from about:blank to file:// by itself)
                    tokio::time::timeout(
                        Duration::from_secs(30),
                        tp.page.goto(format!("file://{}", tmp.display())),
                    )
                    .await
                    .map_err(|_| browser_error("Page.goto: Timeout 30000ms exceeded."))?
                    .map_err(cdp_err)?;
                    Self::wait_loaded(&tp).await;
                    Self::element_png(&tp, "#w", 1.0).await
                }
                .await;
                tp.close().await;
                r
            }
            .await;
            let _ = std::fs::remove_file(&tmp);
            res?
        };
        if let Some(d) = op.parent() {
            std::fs::create_dir_all(d).map_err(|e| browser_error(&e.to_string()))?;
        }
        std::fs::write(&op, &data).map_err(|e| browser_error(&e.to_string()))?;
        Ok(
            json!({"path": rel_ws(&op, ws), "size": data.len(), "width": w, "height": h, "font_px": size}),
        )
    }
}

/// `str(e)` of a CDP error, re-exported for the routes.
pub fn cdp_text(e: &chromiumoxide::error::CdpError) -> String {
    err_text(e)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn css_colors() {
        assert_eq!(css_color("#fff", "x"), "#fff");
        assert_eq!(css_color("rgba(0,0,0,0.45)", "x"), "rgba(0,0,0,0.45)");
        assert_eq!(css_color("url(evil)", "x"), "x");
    }

    #[test]
    fn image_size_header_parser() {
        let dir = std::env::temp_dir().join(format!("omuse-imgsize-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        // PNG header for 321x123
        let mut png = b"\x89PNG\r\n\x1a\n\x00\x00\x00\x0dIHDR".to_vec();
        png.extend_from_slice(&321u32.to_be_bytes());
        png.extend_from_slice(&123u32.to_be_bytes());
        png.extend_from_slice(&[8, 2, 0, 0, 0, 0, 0, 0, 0]);
        std::fs::write(dir.join("t.png"), &png).unwrap();
        assert_eq!(image_size(&dir.join("t.png")).unwrap(), (321, 123));
        let mut gif = b"GIF89a".to_vec();
        gif.extend_from_slice(&321u16.to_le_bytes());
        gif.extend_from_slice(&123u16.to_le_bytes());
        gif.extend_from_slice(&[0; 8]);
        std::fs::write(dir.join("t.gif"), &gif).unwrap();
        assert_eq!(image_size(&dir.join("t.gif")).unwrap(), (321, 123));
        // JPEG: SOI, APP0 (len 16), SOF0 with 123x321
        let mut jpg = vec![0xFF, 0xD8, 0xFF, 0xE0, 0x00, 0x10];
        jpg.extend_from_slice(&[0; 14]);
        jpg.extend_from_slice(&[0xFF, 0xC0, 0x00, 0x11, 8]);
        jpg.extend_from_slice(&123u16.to_be_bytes());
        jpg.extend_from_slice(&321u16.to_be_bytes());
        jpg.extend_from_slice(&[0; 12]);
        std::fs::write(dir.join("t.jpg"), &jpg).unwrap();
        assert_eq!(image_size(&dir.join("t.jpg")).unwrap(), (321, 123));
        // WebP VP8X
        let mut webp = b"RIFF\x00\x00\x00\x00WEBPVP8X".to_vec();
        webp.extend_from_slice(&[0; 8]);
        webp.extend_from_slice(&[64, 1, 0, 122, 0, 0]); // width-1 = 320, height-1 = 122
        std::fs::write(dir.join("t.webp"), &webp).unwrap();
        assert_eq!(image_size(&dir.join("t.webp")).unwrap(), (321, 123));
        std::fs::write(dir.join("x.bin"), b"not an image at all").unwrap();
        assert!(image_size(&dir.join("x.bin")).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn workspace_paths() {
        let ws = std::env::temp_dir().join(format!("omuse-ws-{}", std::process::id()));
        std::fs::create_dir_all(&ws).unwrap();
        let ws = std::fs::canonicalize(&ws).unwrap();
        assert!(ws_path(&ws, "reports/a.md").unwrap().starts_with(&ws));
        assert_eq!(ws_path(&ws, "../../etc/passwd").unwrap_err().status, 400);
        assert_eq!(
            ws_path(&ws, "downloads/.quarantine/x").unwrap_err().status,
            400
        );
        let _ = std::fs::remove_dir_all(&ws);
    }
}
