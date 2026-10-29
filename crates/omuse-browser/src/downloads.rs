//! Downloads (quarantine, size / extension checks) and `save_media` (pictures and videos from a page into
//! `workspace/media/<YYYY-MM>/`).

use std::path::{Path, PathBuf};
use std::sync::LazyLock;
use std::time::Duration;

use base64::Engine;
use chromiumoxide::cdp::browser_protocol::browser::{DownloadProgressState, EventDownloadProgress};
use chromiumoxide::cdp::browser_protocol::network::GetCookiesParams;
use omuse_common::api::ApiError;
use omuse_common::guard::check_url;
use omuse_common::util::{get_str, now_ts, take_chars};
use regex::Regex;
use serde_json::{json, Value};

use crate::actions::ElementRef;
use crate::broker::Broker;
use crate::pages::{err_text, Clip, TaskPage};
use crate::textfmt::{cut, BLOCKED_EXT, MAX_DOWNLOAD};

const MEDIA_MAX: usize = 50 * 1024 * 1024;
/// Characters not kept in the stem of a saved file's name.
static STEM_BAD: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[^\w\-() 一-鿿]+").unwrap());
const MEDIA_TYPES: &[(&str, &str)] = &[
    ("image/jpeg", ".jpg"),
    ("image/png", ".png"),
    ("image/gif", ".gif"),
    ("image/webp", ".webp"),
    ("image/avif", ".avif"),
    ("image/bmp", ".bmp"),
    ("video/mp4", ".mp4"),
    ("video/webm", ".webm"),
    ("video/quicktime", ".mov"),
    ("audio/mpeg", ".mp3"),
    ("audio/mp4", ".m4a"),
    ("audio/ogg", ".ogg"),
    ("audio/wav", ".wav"),
    ("audio/x-wav", ".wav"),
];

fn media_ext(mime: &str) -> Option<&'static str> {
    MEDIA_TYPES
        .iter()
        .find(|(m, _)| *m == mime)
        .map(|(_, e)| *e)
}

/// `os.path.relpath(p, WORKSPACE)`.
pub fn rel_ws(p: &Path, ws: &Path) -> String {
    p.strip_prefix(ws)
        .map(|r| r.to_string_lossy().to_string())
        .unwrap_or_else(|_| p.to_string_lossy().to_string())
}

fn split_ext(name: &str) -> (String, String) {
    match name.rfind('.') {
        Some(i) if i > 0 => (name[..i].to_string(), name[i..].to_string()),
        _ => (name.to_string(), String::new()),
    }
}

impl Broker {
    /// `Browser.downloadWillBegin`: a record with status "downloading".
    pub fn on_download_begin(&self, guid: &str, suggested: &str, url: &str) {
        static BAD: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[^\w.\-() 一-鿿]").unwrap());
        let base = if suggested.is_empty() {
            "download"
        } else {
            suggested
        };
        let name = take_chars(&BAD.replace_all(base, "_"), 120).to_string();
        let rec = json!({"name": name, "url": url, "ts": now_ts(), "status": "downloading"});
        let mut st = self.st.lock();
        st.downloads.push(rec);
        let idx = st.downloads.len() - 1;
        st.download_idx.insert(guid.to_string(), (idx, name));
    }

    /// `Browser.downloadProgress`: on completion the file is checked and moved out of quarantine (`_on_download`).
    pub async fn on_download_progress(&self, ev: &EventDownloadProgress) {
        let (idx, name) = match ev.state {
            DownloadProgressState::InProgress => return,
            _ => match self.st.lock().download_idx.remove(&ev.guid) {
                Some(x) => x,
                None => return,
            },
        };
        let set = |st: &mut crate::broker::State, k: &str, v: Value| {
            if let Some(o) = st.downloads.get_mut(idx).and_then(|r| r.as_object_mut()) {
                o.insert(k.to_string(), v);
            }
        };
        if ev.state == DownloadProgressState::Canceled {
            let mut st = self.st.lock();
            set(&mut st, "status", json!("failed"));
            set(&mut st, "reason", json!("canceled"));
            return;
        }
        let base = self.workspace.join("downloads");
        let quarantine = base.join(".quarantine");
        let _ = std::fs::create_dir_all(&quarantine);
        let src = ev
            .file_path
            .as_ref()
            .map(PathBuf::from)
            .unwrap_or_else(|| quarantine.join(&ev.guid));
        let tmp = quarantine.join(format!("{}_{}", now_ts() as i64, name));
        let ws = self.workspace.clone();
        let res: Result<(bool, PathBuf, u64), String> = tokio::task::spawn_blocking(move || {
            if src != tmp {
                std::fs::rename(&src, &tmp)
                    .or_else(|_| std::fs::copy(&src, &tmp).map(|_| ()))
                    .map_err(|e| e.to_string())?;
            }
            let size = std::fs::metadata(&tmp).map_err(|e| e.to_string())?.len();
            let (_, ext) = split_ext(&name);
            let ext = ext.to_lowercase();
            if BLOCKED_EXT.contains(&ext.as_str()) || size > MAX_DOWNLOAD {
                return Ok((true, tmp, size));
            }
            let mut dest = base.join(&name);
            if dest.exists() {
                let (stem, e) = split_ext(&name);
                dest = base.join(format!("{}_{}{}", stem, now_ts() as i64, e));
            }
            std::fs::rename(&tmp, &dest)
                .or_else(|_| std::fs::copy(&tmp, &dest).and_then(|_| std::fs::remove_file(&tmp)))
                .map_err(|e| e.to_string())?;
            let _ = &ws;
            Ok((false, dest, size))
        })
        .await
        .unwrap_or_else(|e| Err(e.to_string()));
        let mut st = self.st.lock();
        match res {
            Ok((blocked, path, size)) => {
                set(&mut st, "size", json!(size));
                if blocked {
                    set(&mut st, "status", json!("blocked"));
                    set(&mut st, "reason", json!("可执行文件或文件过大，已隔离 (executable or too large; kept in quarantine)"));
                } else {
                    set(&mut st, "status", json!("ok"));
                }
                set(&mut st, "path", json!(rel_ws(&path, &self.workspace)));
            }
            Err(e) => {
                set(&mut st, "status", json!("failed"));
                set(&mut st, "reason", json!(cut(&e, 200)));
            }
        }
    }

    /// `GET /agent/downloads`.
    pub fn downloads(&self) -> Value {
        let st = self.st.lock();
        let n = st.downloads.len();
        json!({"downloads": st.downloads[n.saturating_sub(30)..].to_vec()})
    }

    /// GET `url` with the browser's cookies for it (Playwright's `context.request` shared them) and the page as
    /// referer: `(mime, body)` of a successful response, `None` when the request failed or was not 2xx.
    async fn fetch_with_cookies(
        &self,
        page: &TaskPage,
        url: &str,
        accept: &str,
        max: usize,
    ) -> Result<Option<(String, Vec<u8>)>, ApiError> {
        let cookie = match page
            .page
            .execute(GetCookiesParams {
                urls: Some(vec![url.to_string()]),
            })
            .await
        {
            Ok(r) => r
                .result
                .cookies
                .iter()
                .map(|c| format!("{}={}", c.name, c.value))
                .collect::<Vec<_>>()
                .join("; "),
            Err(_) => String::new(),
        };
        let mut req = self
            .http
            .get(url)
            .timeout(Duration::from_secs(45))
            .header("Referer", page.url())
            .header("Accept", accept);
        if !cookie.is_empty() {
            req = req.header("Cookie", cookie);
        }
        let Ok(resp) = req.send().await else {
            return Ok(None);
        };
        let mime = resp
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .split(';')
            .next()
            .unwrap_or("")
            .trim()
            .to_lowercase();
        let clen: usize = resp
            .headers()
            .get("content-length")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.parse().ok())
            .unwrap_or(0);
        if clen > max {
            return Err(ApiError::new(
                413,
                format!("文件超过 {} MB (file over {} MB)", max >> 20, max >> 20),
            ));
        }
        if !resp.status().is_success() {
            return Ok(None);
        }
        Ok(resp.bytes().await.ok().map(|b| (mime, b.to_vec())))
    }

    /// A PDF the page navigated to (Chromium shows it in its viewer and saves nothing): fetched again with the
    /// page's cookies and written to `downloads/<name>.pdf`, listed with the browser's downloads.
    pub async fn save_pdf(&self, page: &TaskPage, url: &str) -> Result<Value, ApiError> {
        let (ok, why) = check_url(url);
        if !ok {
            return Err(ApiError::new(403, why));
        }
        let max = MAX_DOWNLOAD as usize;
        let data = match self
            .fetch_with_cookies(page, url, "application/pdf,*/*;q=0.5", max)
            .await?
        {
            Some((_, d)) if d.starts_with(b"%PDF-") && d.len() <= max => d,
            _ => {
                return Err(ApiError::new(
                    422,
                    "无法保存这个 PDF (could not save this PDF)",
                ))
            }
        };
        let path = url::Url::parse(url)
            .map(|u| u.path().to_string())
            .unwrap_or_default();
        let fname = path.rsplit('/').next().unwrap_or("").to_string();
        let fname = percent_encoding::percent_decode_str(&fname)
            .decode_utf8_lossy()
            .to_string();
        let (stem, _) = split_ext(&fname);
        let stem = STEM_BAD.replace_all(&stem, "_").to_string();
        let stem = take_chars(stem.trim_matches(|c| c == '_' || c == ' '), 80).to_string();
        let stem = if stem.is_empty() {
            "document".to_string()
        } else {
            stem
        };
        let folder = self.workspace.join("downloads");
        std::fs::create_dir_all(&folder)
            .map_err(|e| crate::broker::browser_error(&e.to_string()))?;
        let mut dest = folder.join(format!("{}.pdf", stem));
        let mut n = 1;
        while dest.exists() {
            n += 1;
            dest = folder.join(format!("{}_{}.pdf", stem, n));
        }
        std::fs::write(&dest, &data).map_err(|e| crate::broker::browser_error(&e.to_string()))?;
        let rel = rel_ws(&dest, &self.workspace);
        let name = dest
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
        self.st.lock().downloads.push(
            json!({"name": name, "url": take_chars(url, 300), "ts": now_ts(),
            "status": "ok", "size": data.len(), "path": rel}),
        );
        Ok(json!({"path": rel, "size": data.len(),
            "note": "PDF 已保存 saved: read it with files_read, or pdf_form_fields / pdf_form_fill when it is a form"}))
    }

    /// Save an image / video / audio from the page into workspace/media/<YYYY-MM>/ (`save_media`).
    pub async fn save_media(
        &self,
        task_id: &str,
        page: &TaskPage,
        r: &str,
        url: &str,
        name: &str,
    ) -> Result<Value, ApiError> {
        static DATA_RE: LazyLock<Regex> =
            LazyLock::new(|| Regex::new(r"(?s)^data:([\w/+.-]+)(;base64)?,(.*)").unwrap());
        let mut url = url.to_string();
        let mut name = name.to_string();
        let mut loc: Option<ElementRef> = None;
        if !r.is_empty() {
            let el = self.locate(task_id, r).await.map_err(|e| e.not_found(r))?;
            let found = el.eval(PICK_MEDIA_JS, vec![]).await.map_err(|e| {
                ApiError::new(
                    409,
                    format!(
                        "找不到元素 {}，请重新获取快照 (element not found): {}",
                        r,
                        cut(&err_text(&e), 100)
                    ),
                )
            })?;
            if url.is_empty() {
                url = get_str(&found, "url");
            }
            if name.is_empty() {
                name = get_str(&found, "alt");
            }
            loc = Some(el);
        }
        let url = url.trim().to_string();
        let (mut data, mut mime, mut how) = (Vec::<u8>::new(), String::new(), "download");
        if url.starts_with("data:") {
            if let Some(c) = DATA_RE.captures(&url) {
                if c.get(2).is_some() {
                    let payload = take_chars(&c[3], MEDIA_MAX * 2);
                    data = base64::engine::general_purpose::STANDARD
                        .decode(payload.trim())
                        .unwrap_or_default();
                    mime = c[1].to_lowercase();
                }
            }
        } else if url.starts_with("http://") || url.starts_with("https://") {
            let (ok, why) = check_url(&url);
            if !ok {
                return Err(ApiError::new(403, why));
            }
            if let Some((m, d)) = self
                .fetch_with_cookies(page, &url, "image/*,video/*,audio/*,*/*;q=0.5", MEDIA_MAX)
                .await?
            {
                (mime, data) = (m, d);
            }
        }
        if !data.is_empty() && media_ext(&mime).is_none() {
            data.clear(); // html, svg, scripts… are never saved as media
        }
        if data.is_empty() {
            if let Some(el) = &loc {
                // last resort: what the user sees
                if let Ok(png) = element_screenshot(el).await {
                    data = png;
                    mime = "image/png".to_string();
                    how = "screenshot";
                }
            }
        }
        if data.is_empty() {
            return Err(ApiError::new(
                422,
                "无法保存这个图片/视频（可能是流媒体或受保护内容）(could not save this media — it may be streamed or protected; try another element or a screenshot)",
            ));
        }
        if data.len() > MEDIA_MAX {
            return Err(ApiError::new(413, "文件超过 50 MB (file over 50 MB)"));
        }
        let base_stem = if !name.is_empty() {
            name.clone()
        } else {
            let path = url::Url::parse(&url)
                .map(|u| u.path().to_string())
                .unwrap_or_default();
            let fname = path.rsplit('/').next().unwrap_or("").to_string();
            let (stem, _) = split_ext(&fname);
            if stem.is_empty() {
                "media".to_string()
            } else {
                stem
            }
        };
        let stem = STEM_BAD.replace_all(&base_stem, "_").to_string();
        let stem = take_chars(stem.trim_matches(|c| c == '_' || c == ' '), 60).to_string();
        let stem = if stem.is_empty() {
            "media".to_string()
        } else {
            stem
        };
        let folder = self
            .workspace
            .join("media")
            .join(chrono::Local::now().format("%Y-%m").to_string());
        std::fs::create_dir_all(&folder)
            .map_err(|e| crate::broker::browser_error(&e.to_string()))?;
        let ext = media_ext(&mime).unwrap_or(".png");
        let mut dest = folder.join(format!("{}{}", stem, ext));
        let mut n = 1;
        while dest.exists() {
            n += 1;
            dest = folder.join(format!("{}_{}{}", stem, n, ext));
        }
        std::fs::write(&dest, &data).map_err(|e| crate::broker::browser_error(&e.to_string()))?;
        Ok(
            json!({"path": rel_ws(&dest, &self.workspace), "mime": mime, "size": data.len(), "method": how,
            "source": if url.starts_with("data:") { "data: URL".to_string() } else { take_chars(&url, 300).to_string() }}),
        )
    }
}

/// `locator.screenshot(type="png")`: the element's box clipped out of the viewport.
pub async fn element_screenshot(el: &ElementRef) -> Result<Vec<u8>, String> {
    let _ = el.page.scroll_into_view(&el.obj).await;
    let quads = el.page.quads(&el.obj).await.map_err(|e| err_text(&e))?;
    let q = quads
        .first()
        .filter(|q| q.len() >= 8)
        .ok_or_else(|| "element is not visible".to_string())?;
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
    if x1 - x0 < 1.0 || y1 - y0 < 1.0 {
        return Err("element has an empty box".to_string());
    }
    el.page
        .screenshot(
            false,
            None,
            Some(Clip {
                x: x0,
                y: y0,
                width: x1 - x0,
                height: y1 - y0,
                scale: 1.0,
            }),
            Duration::from_secs(10),
        )
        .await
        .map_err(|e| err_text(&e))
}

const PICK_MEDIA_JS: &str = r#"el => {
                    const pick = e => {
                      if (!e) return '';
                      if (e.tagName === 'IMG') return e.currentSrc || e.src || '';
                      if (e.tagName === 'VIDEO' || e.tagName === 'AUDIO') {
                        const s = e.currentSrc || e.src || (e.querySelector('source[src]') || {}).src || '';
                        return s || (e.poster || '');
                      }
                      if (e.tagName === 'SOURCE') return e.src || '';
                      const bg = getComputedStyle(e).backgroundImage || '';
                      const m = bg.match(/url\(["']?(.*?)["']?\)/);
                      return m ? new URL(m[1], location.href).href : '';
                    };
                    let u = pick(el);
                    if (!u) { const inner = el.querySelector('img,video,audio,picture img'); u = pick(inner); }
                    return {url: u, tag: el.tagName.toLowerCase(), alt: (el.alt || el.title || el.getAttribute('aria-label') || '').slice(0, 80)};
                }"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ext_split_and_media_types() {
        assert_eq!(
            split_ext("a.b.PNG"),
            ("a.b".to_string(), ".PNG".to_string())
        );
        assert_eq!(split_ext("noext"), ("noext".to_string(), String::new()));
        assert_eq!(media_ext("image/jpeg"), Some(".jpg"));
        assert_eq!(media_ext("text/html"), None);
        assert!(BLOCKED_EXT.contains(&".exe"));
    }
}
