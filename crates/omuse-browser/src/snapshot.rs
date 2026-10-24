//! The page as text and as pictures: `snapshot`, `find`, `look` (set-of-marks), `grid` (browser_locate) and
//! `at` (what is under a point), with the JavaScript helpers of `crates/omuse-browser/js/*.js` injected unchanged.

use std::time::Duration;

use base64::Engine;
use omuse_common::api::ApiError;
use omuse_common::util::{as_f64, char_len, get_str, take_chars, truthy};
use serde_json::{json, Value};

use crate::broker::{cdp_err, Broker, CdpExt};
use crate::pages::{err_text, Clip, FrameInfo, TaskPage};
use crate::textfmt::{cut, format_feed, looks_loading, VIEWPORT_H, VIEWPORT_W};

pub const SNAPSHOT_JS: &str = include_str!("../js/snapshot.js");
pub const FEED_JS: &str = include_str!("../js/feed.js");
pub const FIND_JS: &str = include_str!("../js/find.js");
pub const MARKS_JS: &str = include_str!("../js/marks.js");
pub const GRID_JS: &str = include_str!("../js/grid.js");
pub const AT_JS: &str = include_str!("../js/at.js");

fn frame_arg<'a>(
    page: &TaskPage,
    f: &'a FrameInfo,
) -> Option<&'a chromiumoxide::cdp::browser_protocol::page::FrameId> {
    if f.id.inner() == page.id() {
        None
    } else {
        Some(&f.id)
    }
}

impl Broker {
    /// The page as text. A list still showing "Loading..." gets up to ~8 s more (`snapshot`).
    pub async fn snapshot(
        &self,
        task_id: &str,
        max_chars: i64,
        near: bool,
    ) -> Result<Value, ApiError> {
        let mut snap = self.snapshot_once(task_id, max_chars, near).await?;
        for _ in 0..2 {
            if truthy(snap.get("feed").unwrap_or(&Value::Null))
                || !looks_loading(&get_str(&snap, "snapshot"))
            {
                break;
            }
            let page = self.page(task_id).await?;
            page.wait_network_idle(Duration::from_secs(3)).await;
            tokio::time::sleep(Duration::from_secs(1)).await;
            snap = self.snapshot_once(task_id, max_chars, near).await?;
        }
        Ok(snap)
    }

    async fn snapshot_once(
        &self,
        task_id: &str,
        max_chars: i64,
        near: bool,
    ) -> Result<Value, ApiError> {
        let page = self.page(task_id).await?;
        self.settle(&page, 200).await;
        page.refresh_url().await;
        let feed = tokio::time::timeout(Duration::from_secs(10), page.eval(None, FEED_JS, vec![]))
            .await
            .ok()
            .and_then(|r| r.ok());
        if let Some(f) = &feed {
            if f.get("items")
                .and_then(|i| i.as_array())
                .map(|a| !a.is_empty())
                .unwrap_or(false)
            {
                let title = get_str(f, "feed");
                let url = page.url();
                return Ok(
                    json!({"url": url, "title": title, "snapshot": format_feed(f, &url), "truncated": false,
                    "tabs": self.tabs(), "feed": true}),
                );
            }
        }
        // Payment forms put every card field in its own iframe, after many tracker / captcha frames (2026-10-05 decathlon.sg:
        // Adyen's expiry and CVC boxes were frames 8 and 9, so only the number box was listed and all three card values
        // were typed into it). Read up to 24 frames; empty ones add nothing to the snapshot.
        let frames = page.frames().await.api()?;
        let mut fmap = vec![];
        let mut parts: Vec<String> = vec![];
        let mut iframe_parts: Vec<String> = vec![];
        for (i, fr) in frames.iter().take(24).enumerate() {
            let prefix = if i == 0 {
                String::new()
            } else {
                format!("f{}", i)
            };
            fmap.push((prefix.clone(), fr.id.clone()));
            let opts = json!({"prefix": prefix, "near": near && i == 0});
            let txt = match tokio::time::timeout(
                Duration::from_secs(20),
                page.eval(frame_arg(&page, fr), SNAPSHOT_JS, vec![opts]),
            )
            .await
            {
                Ok(Ok(Value::String(s))) => s,
                Ok(Ok(_)) => String::new(),
                Ok(Err(e)) => {
                    if i == 0 {
                        format!("(无法读取 frame: {})", cut(&err_text(&e), 80))
                    } else {
                        String::new()
                    }
                }
                Err(_) => {
                    if i == 0 {
                        "(无法读取 frame: timeout)".to_string()
                    } else {
                        String::new()
                    }
                }
            };
            if i == 0 {
                parts.push(txt);
            } else if !txt.trim().is_empty() {
                iframe_parts.push(format!(
                    "--- iframe {}: {} ---\n{}",
                    prefix,
                    take_chars(&fr.url, 100),
                    take_chars(&txt, 1500)
                ));
            }
        }
        let key = if task_id.is_empty() {
            "default"
        } else {
            task_id
        };
        self.st.lock().frame_maps.insert(key.to_string(), fmap);
        let title = page.title().await;
        let max_chars =
            (if max_chars == 0 { 12000 } else { max_chars }).clamp(2000, 40000) as usize;
        // iframes (card fields, chat widgets) come after the page and must not be cut off by a long page
        let frames_txt =
            take_chars(&iframe_parts.join("\n"), std::cmp::max(1500, max_chars / 3)).to_string();
        let mut main = parts.join("\n");
        let room = std::cmp::max(1000, max_chars.saturating_sub(char_len(&frames_txt)));
        let truncated = char_len(&main) > room;
        if truncated {
            main = format!(
                "{}\n…[快照已截断 snapshot truncated — browser_find(\"text\") locates anything on the page; browser_scroll shows the part around the new position; browser_look lets you see the page]",
                take_chars(&main, room)
            );
        }
        let mut body = main;
        if !frames_txt.is_empty() {
            body.push('\n');
            body.push_str(&frames_txt);
        }
        if near {
            body = format!(
                "(showing the part of the page around the current scroll position)\n{}",
                body
            );
        }
        Ok(
            json!({"url": page.url(), "title": title, "snapshot": body, "truncated": truncated, "tabs": self.tabs()}),
        )
    }

    /// Locate elements by text anywhere on the page (all frames); refs stay valid for click/type (`find`).
    pub async fn find(&self, task_id: &str, query: &str) -> Result<Value, ApiError> {
        let snap = self.snapshot(task_id, 2000, false).await?; // assigns fresh refs in every frame
        let page = self.page(task_id).await?;
        let key = if task_id.is_empty() {
            "default"
        } else {
            task_id
        };
        let fmap = self
            .st
            .lock()
            .frame_maps
            .get(key)
            .cloned()
            .unwrap_or_default();
        let mut matches: Vec<Value> = vec![];
        for (_, fid) in &fmap {
            let f = if fid.inner() == page.id() {
                None
            } else {
                Some(fid)
            };
            if let Ok(Value::Array(found)) = page.eval(f, FIND_JS, vec![json!(query)]).await {
                matches.extend(found);
            }
        }
        matches.sort_by(|a, b| {
            let sa = a.get("score").and_then(as_f64).unwrap_or(0.0);
            let sb = b.get("score").and_then(as_f64).unwrap_or(0.0);
            sb.partial_cmp(&sa).unwrap_or(std::cmp::Ordering::Equal)
        });
        matches.truncate(25);
        let url = get_str(&snap, "url");
        let mut lines = vec![format!(
            "browser_find \"{}\": {} match(es) on {}",
            query,
            matches.len(),
            url
        )];
        for m in &matches {
            let ctx = get_str(m, "context");
            let name = get_str(m, "name");
            let mut line = format!(
                "[{}] {} \"{}\" ({})",
                get_str(m, "ref"),
                get_str(m, "role"),
                name,
                get_str(m, "where")
            );
            if !ctx.is_empty() && ctx != name {
                line.push_str(&format!("\n    context: {}", ctx));
            }
            lines.push(line);
        }
        if matches.is_empty() {
            lines.push("(nothing found — try other words, browser_scroll to load more, or browser_look to see the page)".to_string());
        }
        Ok(
            json!({"url": url, "title": snap.get("title").cloned().unwrap_or(Value::Null), "snapshot": lines.join("\n"),
            "tabs": snap.get("tabs").cloned().unwrap_or(Value::Null), "matches": matches.len()}),
        )
    }

    /// Screenshot of the viewport with every visible element labelled by its ref (set-of-marks) for a vision model.
    pub async fn look(&self, task_id: &str) -> Result<Value, ApiError> {
        let page = self.page(task_id).await?;
        self.snapshot(task_id, 2000, false).await?; // fresh refs
        let key = if task_id.is_empty() {
            "default"
        } else {
            task_id
        };
        let fmap = self
            .st
            .lock()
            .frame_maps
            .get(key)
            .cloned()
            .unwrap_or_default();
        let live = page.frames().await.api()?;
        // label the page and every iframe (chat widgets like Zendesk/Intercom live in iframes; their refs look like f1e3)
        let mut frames: Vec<Option<chromiumoxide::cdp::browser_protocol::page::FrameId>> = fmap
            .iter()
            .filter(|(_, f)| live.iter().any(|l| l.id == *f))
            .map(|(_, f)| {
                if f.inner() == page.id() {
                    None
                } else {
                    Some(f.clone())
                }
            })
            .collect();
        if frames.is_empty() {
            frames.push(None);
        }
        let mut marks: Vec<Value> = vec![];
        let mut text = String::new();
        let shot = async {
            for fr in &frames {
                let Ok(got) = page.eval(fr.as_ref(), MARKS_JS, vec![json!("draw")]).await else {
                    continue;
                };
                if let Some(Value::Array(m)) = got.get("marks") {
                    marks.extend(m.clone());
                }
                let t = get_str(&got, "text");
                if !t.is_empty() {
                    if !text.is_empty() {
                        text.push('\n');
                    }
                    text.push_str(&t);
                }
            }
            page.screenshot(true, Some(70), None, Duration::from_secs(10))
                .await
        }
        .await;
        for fr in &frames {
            let _ = page.eval(fr.as_ref(), MARKS_JS, vec![json!("clear")]).await;
        }
        let img = shot.map_err(cdp_err)?;
        let text = take_chars(&text, 3000).to_string();
        let title = page.title().await;
        let lines: Vec<String> = marks
            .iter()
            .map(|m| {
                format!(
                    "[{}] {} \"{}\"",
                    get_str(m, "ref"),
                    get_str(m, "role"),
                    get_str(m, "name")
                )
            })
            .collect();
        let mut snapshot = format!("Labelled elements visible on screen:\n{}", lines.join("\n"));
        if !text.is_empty() {
            snapshot.push_str(&format!("\n\nText visible on screen:\n{}", text));
        }
        Ok(
            json!({"url": page.url(), "title": title, "image_b64": base64::engine::general_purpose::STANDARD.encode(&img),
            "image_type": "image/jpeg", "marks": marks, "snapshot": snapshot,
            "viewport": {"width": VIEWPORT_W, "height": VIEWPORT_H}, "tabs": self.tabs()}),
        )
    }

    /// Screenshot for visual targeting: a labelled grid over the viewport (or a zoomed region), and/or a red marker.
    pub async fn grid(
        &self,
        task_id: &str,
        region: Option<Value>,
        cols: i64,
        rows: i64,
        labels: &str,
        mark: Option<Value>,
    ) -> Result<Value, ApiError> {
        let page = self.page(task_id).await?;
        let (mut vw, mut vh) = (VIEWPORT_W as f64, VIEWPORT_H as f64);
        if let Ok(Value::Array(a)) = page
            .eval(
                None,
                "() => [window.innerWidth, window.innerHeight]",
                vec![],
            )
            .await
        {
            if a.len() == 2 {
                vw = as_f64(&a[0]).unwrap_or(vw);
                vh = as_f64(&a[1]).unwrap_or(vh);
            }
        }
        let region: Option<Vec<f64>> = match region {
            Some(Value::Array(a)) if a.len() >= 4 => {
                let v: Vec<f64> = a.iter().take(4).map(|x| as_f64(x).unwrap_or(0.0)).collect();
                let (x, y) = (v[0].clamp(0.0, vw - 20.0), v[1].clamp(0.0, vh - 20.0));
                Some(vec![
                    x,
                    y,
                    v[2].min(vw - x).max(20.0),
                    v[3].min(vh - y).max(20.0),
                ])
            }
            _ => None,
        };
        let mark_xy: Option<(f64, f64)> = match &mark {
            Some(Value::Array(a)) if a.len() >= 2 => {
                Some((as_f64(&a[0]).unwrap_or(0.0), as_f64(&a[1]).unwrap_or(0.0)))
            }
            _ => None,
        };
        let opts = json!({"mode": "draw", "region": region, "cols": cols, "rows": rows, "labels": labels,
            "mark": mark_xy.map(|(x, y)| json!([x, y]))});
        let shot = async {
            page.eval(None, GRID_JS, vec![opts.clone()]).await?;
            let clip = region.as_ref().map(|r| Clip {
                x: r[0],
                y: r[1],
                width: r[2],
                height: r[3],
                scale: 1.0,
            });
            page.screenshot(true, Some(75), clip, Duration::from_secs(10))
                .await
        }
        .await;
        let _ = page
            .eval(None, GRID_JS, vec![json!({"mode": "clear"})])
            .await;
        let img = shot.map_err(cdp_err)?;
        let mut out = json!({"url": page.url(), "title": page.title().await,
            "image_b64": base64::engine::general_purpose::STANDARD.encode(&img), "image_type": "image/jpeg",
            "region": region.clone().unwrap_or_else(|| vec![0.0, 0.0, vw, vh]), "cols": cols, "rows": rows, "labels": labels,
            "viewport": {"width": vw, "height": vh}, "tabs": self.tabs(), "snapshot": ""});
        if let Some((x, y)) = mark_xy {
            let at = self.at(task_id, x, y).await?;
            out.as_object_mut().unwrap().insert("at".into(), at);
        }
        Ok(out)
    }

    /// The element under a viewport point, looking into iframes (also cross-origin) and open shadow roots (`at`).
    pub async fn at(&self, task_id: &str, x: f64, y: f64) -> Result<Value, ApiError> {
        let page = self.page(task_id).await?;
        let mut frame: Option<chromiumoxide::cdp::browser_protocol::page::FrameId> = None;
        let (mut fx, mut fy) = (x, y);
        let mut frames: Vec<String> = vec![];
        for _ in 0..4 {
            let info = match page
                .eval(frame.as_ref(), AT_JS, vec![json!([fx, fy])])
                .await
            {
                Ok(v) => v,
                Err(e) => {
                    return Ok(
                        json!({"tag": "", "name": "", "error": cut(&err_text(&e), 120), "frames": frames}),
                    )
                }
            };
            if !truthy(&info) {
                return Ok(json!({"tag": "", "name": "", "frames": frames}));
            }
            if !truthy(info.get("iframe").unwrap_or(&Value::Null)) {
                let mut info = info;
                let o = info.as_object_mut().unwrap();
                o.insert("frames".into(), json!(frames));
                o.insert("page_url".into(), json!(page.url()));
                o.insert("page_title".into(), json!(page.title().await));
                return Ok(info);
            }
            let bx = info
                .get("box")
                .and_then(|b| b.get(0))
                .and_then(as_f64)
                .unwrap_or(0.0);
            let by = info
                .get("box")
                .and_then(|b| b.get(1))
                .and_then(as_f64)
                .unwrap_or(0.0);
            // the <iframe> element at that box (or containing the point) -> its frame
            let pick = page
                .eval_obj(
                    frame.as_ref(),
                    r#"(bx, by, fx, fy) => { for (const f of document.querySelectorAll('iframe,frame')) { const r = f.getBoundingClientRect();
                        if ((Math.abs(r.x - bx) < 3 && Math.abs(r.y - by) < 3) || (r.x <= fx && fx <= r.x + r.width && r.y <= fy && fy <= r.y + r.height)) return f; } return null; }"#,
                    vec![json!(bx), json!(by), json!(fx), json!(fy)],
                )
                .await
                .ok()
                .flatten();
            let child = match pick {
                Some(obj) => page.frame_of_element(&obj).await.ok().flatten(),
                None => None,
            };
            // a frame rendered out of process (error pages, isolated sites) is not reachable through this session
            let known = match &child {
                Some(c) => page
                    .frames()
                    .await
                    .ok()
                    .and_then(|fs| fs.into_iter().find(|f| f.id == *c).map(|f| f.url)),
                None => None,
            };
            let (Some(child), Some(url)) = (child, known) else {
                return Ok(
                    json!({"tag": "iframe", "name": get_str(&info, "src"), "frames": frames}),
                );
            };
            frames.push(take_chars(url.split('?').next().unwrap_or(""), 160).to_string());
            frame = Some(child);
            fx -= bx;
            fy -= by;
        }
        Ok(json!({"tag": "", "name": "", "frames": frames}))
    }
}
