//! The image tools: `make_image`, `edit_image`, `vary_image` (AI image model through `imagegen`), `caption_image`
//! (exact text laid out by the browser service via Sentinel) and `upscale_image` (local Lanczos + unsharp mask).

use std::sync::atomic::Ordering;
use std::sync::LazyLock;

use omuse_common::util::{get_str, py_str, take_chars, truthy};
use regex::Regex;
use serde_json::{json, Value};

use super::ext::ig;
use super::local::wants_send;
use super::runtime::Runtime;
use super::util::{basename, sget, splitext, LocalErr};

static SLUG: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[^\w\-一-鿿]+").unwrap());

fn img_n(a: &Value, default: i64) -> i64 {
    match a.get("n").filter(|v| truthy(v)) {
        None => default,
        Some(v) => omuse_common::util::as_i64(v)
            .map(|n| n.clamp(1, 4))
            .unwrap_or(default),
    }
}

fn opt_str(a: &Value, k: &str) -> Option<String> {
    a.get(k).filter(|v| truthy(v)).map(py_str)
}

/// `_img_error`: what the model is told about an `ImageError`.
pub fn img_error(msg: &str) -> String {
    let setup = [
        "没有图像生成模型",
        "no image-generation model",
        "image_base_url",
        "没有配置模型地址",
        "no model endpoint",
        "无法读取模型列表",
        "could not list models",
    ];
    if setup.iter().any(|k| msg.contains(k)) {
        return format!("ERROR: 图像生成没有配置 (image generation is not set up): {} 告诉用户：需要在 Settings 里配置图像模型后才能生成图片；不要重试，不要改用其他工具画。 Tell the user an image model must be configured in Settings first; do not retry or draw another way.", msg);
    }
    if msg.contains("不支持图片编辑") || msg.contains("does not support image edits") {
        return format!("ERROR: {} 告诉用户这个图像模型不支持编辑/变体，可以用 make_image 按新描述重新生成一张。 Tell the user this model can't edit; offer to regenerate with make_image instead.", msg);
    }
    format!(
        "ERROR: {}。不要用同样的参数重试超过一次 (don't retry the same request more than once).",
        msg
    )
}

fn budget_msg(made: i64, budget: i64) -> String {
    format!("ERROR: 本任务已生成 {} 张图片，达到上限 {} 张（每张都要付费）。不要再生成：把已有的图交付给用户，或说明情况并询问是否继续 (image budget for this task reached; deliver what you have or ask the user).", made, budget)
}

impl Runtime {
    /// A workspace image as (filename, bytes); `Err` with a message for the model.
    pub(crate) fn img_read(&self, rel: &str) -> Result<(String, Vec<u8>), String> {
        let rel = rel.trim();
        if rel.is_empty() {
            return Err("需要图片路径 (image path required)".into());
        }
        let full = self.path(rel).map_err(|e| e.msg)?;
        if !full.is_file() {
            return Err(format!("找不到图片 image not found: {}", rel));
        }
        let ext = splitext(&full.to_string_lossy()).1.to_lowercase();
        if ![".png", ".jpg", ".jpeg", ".webp"].contains(&ext.as_str()) {
            return Err(format!(
                "不是 PNG/JPEG/WebP 图片 not a PNG/JPEG/WebP image: {}",
                rel
            ));
        }
        if std::fs::metadata(&full).map(|m| m.len()).unwrap_or(0) > 50 * 1024 * 1024 {
            return Err(format!("图片太大 image over 50 MB: {}", rel));
        }
        let data = std::fs::read(&full).map_err(|e| e.to_string())?;
        Ok((basename(&full.to_string_lossy()), data))
    }

    fn take_budget(&self, tid: &str, n: i64) -> Result<(), String> {
        let budget = self.image_budget.load(Ordering::Relaxed);
        let mut inner = self.inner.lock();
        let made = inner.img_count.get(tid).copied().unwrap_or(0);
        if made + n > budget {
            return Err(budget_msg(made, budget));
        }
        inner.img_count.insert(tid.to_string(), made + n);
        Ok(())
    }

    /// AI image generation (make_image); files land in images/<title>.png and are shown in the chat like a chart.
    pub async fn make_image(&self, t: &Value, a: &Value) -> String {
        static CTRL: LazyLock<Regex> =
            LazyLock::new(|| Regex::new(r"[\x00-\x08\x0b\x0c\x0e-\x1f\x7f]").unwrap());
        let tid = get_str(t, "id");
        let prompt = take_chars(CTRL.replace_all(&sget(a, "prompt"), " ").trim(), 4000).to_string();
        if prompt.is_empty() {
            return "ERROR: 需要 prompt（画面描述）prompt required.".into();
        }
        let mut prompt = ig::with_style(&prompt, a.get("style").unwrap_or(&Value::Null));
        if sget(a, "background") == "transparent" && !prompt.to_lowercase().contains("transparent")
        {
            prompt += ", isolated on a transparent background"; // models that ignore the parameter still get the hint
        }
        let n = img_n(a, 1);
        // cost guard: a task gets a small budget of generated pictures (each call bills the user)
        if let Err(m) = self.take_budget(&tid, n) {
            return m;
        }
        let s = self.settings();
        let mut fmt = sget(a, "output_format").to_lowercase();
        if !["png", "jpeg", "webp"].contains(&fmt.as_str()) {
            fmt = "png".into();
        }
        let o = ig::GenOpts {
            size: opt_str(a, "size"),
            aspect: opt_str(a, "aspect"),
            n,
            quality: opt_str(a, "quality"),
            negative_prompt: opt_str(a, "negative_prompt"),
            background: opt_str(a, "background"),
            output_format: if fmt != "png" {
                Some(fmt.clone())
            } else {
                None
            },
            ..Default::default()
        };
        let res = match ig::generate(&self.images, &s, &prompt, &o).await {
            Ok(r) => r,
            Err(ig::IgError::Image(m)) => return img_error(&m),
            Err(ig::IgError::Other(m)) => {
                return format!(
                    "ERROR: 图像生成失败 image generation failed: {}",
                    take_chars(&m, 200)
                )
            }
        };
        self.save_images(t, a, &res, &prompt, &fmt, "image", "生成 generated")
            .await
    }

    #[allow(clippy::too_many_arguments)] // mirrors `_save_images(t, a, res, prompt, fmt, *, kind, verb)`
    async fn save_images(
        &self,
        t: &Value,
        a: &Value,
        res: &ig::ImageResult,
        prompt: &str,
        fmt: &str,
        kind: &str,
        verb: &str,
    ) -> String {
        let tid = get_str(t, "id");
        let ext = match fmt {
            "jpeg" => "jpg",
            "webp" => "webp",
            _ => "png",
        };
        let title = if truthy(a.get("title").unwrap_or(&Value::Null)) {
            sget(a, "title")
        } else {
            take_chars(prompt, 40).to_string()
        };
        let mut slug = take_chars(SLUG.replace_all(&title, "_").trim_matches('_'), 50).to_string();
        if slug.is_empty() {
            slug = "image".into();
        }
        let explicit = truthy(a.get("output").unwrap_or(&Value::Null));
        let out = if explicit {
            sget(a, "output")
        } else {
            format!("images/{}.{}", slug, ext)
        };
        let (mut stem, mut oext) = splitext(&out);
        if ![".png", ".jpg", ".jpeg", ".webp"].contains(&oext.to_lowercase().as_str()) {
            stem = out.clone();
            oext = format!(".{}", ext);
        }
        let out = format!("{}{}", stem, oext);
        let mut paths = Vec::new();
        for (i, png) in res.images.iter().enumerate() {
            let mut p = if i == 0 {
                out.clone()
            } else {
                format!("{}-{}{}", stem, i + 1, oext)
            };
            let base = splitext(&p).0;
            let mut k = 2;
            while self.path(&p).map(|x| x.exists()).unwrap_or(false) && !explicit {
                p = format!("{}-{}{}", base, k, oext);
                k += 1;
            }
            let full = match self.path(&p) {
                Ok(f) => f,
                Err(e) => return format!("ERROR: {}", e.msg),
            };
            if let Some(d) = full.parent() {
                let _ = std::fs::create_dir_all(d);
            }
            if let Err(e) = std::fs::write(&full, png) {
                return format!("ERROR: OSError: {}", e);
            }
            paths.push(p);
        }
        if paths.is_empty() {
            return "ERROR: 图像生成没有返回图片 (the server returned no image data)".into();
        }
        self.inner
            .lock()
            .last_image
            .insert(tid.clone(), paths[0].clone()); // "change the colour" / "more like this" without a path
        self.event(&tid, "image", json!({"paths": paths, "model": res.model, "size": res.size, "kind": kind,
                                         "latency_s": res.latency_s, "prompt": take_chars(prompt, 300)})).await;
        let via = res.via.clone().unwrap_or_default();
        let info = format!(
            "{} {} {}s{}",
            res.model,
            res.size,
            super::util::fmt_float(res.latency_s),
            if via.is_empty() {
                String::new()
            } else {
                format!(" via {}", via)
            }
        );
        if wants_send(a) {
            let arg = if paths.len() > 1 {
                json!({"paths": paths, "note": sget(a, "title")})
            } else {
                json!({"path": paths[0], "note": sget(a, "title")})
            };
            let sent = self
                .send_file(t, &arg)
                .await
                .unwrap_or_else(|e| format!("ERROR: {}", e));
            if sent.starts_with("ERROR") {
                return format!(
                    "图片已{}: {}（{}），但发送失败 but sending failed: {}",
                    verb,
                    paths.join(", "),
                    info,
                    sent
                );
            }
            return format!("图片已{}并显示在对话里 shown in the chat: {}（{}）。不要再用 send_file 重复发送；在回答里用一两句话描述画面并问用户是否要调整。要改这张就用 edit_image，要更多类似的用 vary_image（都可以不传 path）。{} Don't send it again; describe it briefly and offer adjustments (edit_image / vary_image).",
                verb, paths.join(", "), info,
                if res.revised_prompt.is_empty() { String::new() } else { format!(" 模型改写后的描述 revised prompt: {}", take_chars(&res.revised_prompt, 200)) });
        }
        format!(
            "图片已{}: {}（{}，未发送 not sent）。要给用户看就用 send_file。",
            verb,
            paths.join(", "),
            info
        )
    }

    /// edit_image (inpainting by mask / prompt edit / reference-guided) and vary_image (close variations).
    pub async fn edit_image(&self, t: &Value, a: &Value, vary: bool) -> String {
        let tid = get_str(t, "id");
        let mut rel = sget(a, "path").trim().to_string();
        if rel.is_empty() {
            rel = self
                .inner
                .lock()
                .last_image
                .get(&tid)
                .cloned()
                .unwrap_or_default();
        }
        if rel.is_empty() {
            return "ERROR: 没有可以修改的图片 (no image to edit): 先用 make_image 生成一张，或给 path（工作区里的图片）。 Generate one with make_image first, or pass the workspace path of an image.".into();
        }
        let main = match self.img_read(&rel) {
            Ok(m) => m,
            Err(e) => return format!("ERROR: {}", e),
        };
        let mut refs = Vec::new();
        for r in super::util::arr(a, "references").iter().take(4) {
            match self.img_read(&py_str(r)) {
                Ok(x) => refs.push(x),
                Err(e) => return format!("ERROR: {}", e),
            }
        }
        let mask = if truthy(a.get("mask").unwrap_or(&Value::Null)) {
            match self.img_read(&sget(a, "mask")) {
                Ok(x) => Some(x.1),
                Err(e) => return format!("ERROR: {}", e),
            }
        } else {
            None
        };
        let mut prompt = sget(a, "prompt").trim().to_string();
        let n = img_n(a, if vary { 2 } else { 1 });
        if let Err(m) = self.take_budget(&tid, n) {
            return m;
        }
        let s = self.settings();
        let (res, label) = if vary {
            let o = ig::EditOpts {
                size: opt_str(a, "size"),
                aspect: opt_str(a, "aspect"),
                n,
                prompt: prompt.clone(),
                ..Default::default()
            };
            let r = ig::variation(&self.images, &s, &main, &o).await;
            let label = if prompt.is_empty() {
                format!("variation of {}", basename(&rel))
            } else {
                prompt.clone()
            };
            (r, label)
        } else {
            if prompt.is_empty() {
                return "ERROR: 需要 prompt（说明改什么）prompt required: what to change.".into();
            }
            if !refs.is_empty() {
                prompt += " Use the additional reference images for style, character and object consistency.";
            }
            let mut imgs = vec![main];
            imgs.extend(refs);
            let o = ig::EditOpts {
                mask,
                size: opt_str(a, "size"),
                aspect: opt_str(a, "aspect"),
                n,
                ..Default::default()
            };
            (
                ig::edit(&self.images, &s, &imgs, &prompt, &o).await,
                prompt.clone(),
            )
        };
        let res = match res {
            Ok(r) => r,
            Err(ig::IgError::Image(m)) => return img_error(&m),
            Err(ig::IgError::Other(m)) => {
                return format!(
                    "ERROR: 图像编辑失败 image edit failed: {}",
                    take_chars(&m, 200)
                )
            }
        };
        let mut a = a.clone();
        if a.get("title").is_none() {
            let stem = splitext(&basename(&rel)).0;
            a["title"] = json!(take_chars(
                &format!("{}{}", if vary { "variation_" } else { "edited_" }, stem),
                50
            ));
        }
        self.save_images(
            t,
            &a,
            &res,
            &label,
            "png",
            if vary { "variation" } else { "edit" },
            if vary {
                "生成变体 varied"
            } else {
                "修改 edited"
            },
        )
        .await
    }

    /// caption_image: exact text on an image, laid out by the browser container (CJK fonts), offline.
    pub async fn caption_image(&self, t: &Value, a: &Value) -> String {
        let tid = get_str(t, "id");
        let mut rel = sget(a, "path").trim().to_string();
        if rel.is_empty() {
            rel = self
                .inner
                .lock()
                .last_image
                .get(&tid)
                .cloned()
                .unwrap_or_default();
        }
        if rel.is_empty() {
            return "ERROR: 没有图片可以加字 (no image): 先 make_image 生成底图，或给 path。Generate one first or pass a path.".into();
        }
        if let Err(e) = self.img_read(&rel) {
            return format!("ERROR: {}", e);
        }
        let text = sget(a, "text").trim().to_string();
        if text.is_empty() {
            return "ERROR: 需要 text（要写的文字）text required.".into();
        }
        let stem = splitext(&rel).0;
        let mut out = format!("{}_text.png", stem);
        let mut k = 2;
        while self.path(&out).map(|p| p.exists()).unwrap_or(false) {
            out = format!("{}_text-{}.png", stem, k);
            k += 1;
        }
        let body = json!({"task_id": tid, "image": rel, "text": take_chars(&text, 400),
            "position": a.get("position").filter(|v| truthy(v)).cloned().unwrap_or(json!("bottom")),
            "size": a.get("size").filter(|v| truthy(v)).cloned().unwrap_or(json!(0)),
            "color": a.get("color").filter(|v| truthy(v)).cloned().unwrap_or(json!("")),
            "band": truthy(a.get("band").unwrap_or(&Value::Null)),
            "weight": if truthy(a.get("weight").unwrap_or(&Value::Null)) { sget(a, "weight") } else { "700".into() },
            "output": out});
        let r = match self
            .sentinel("POST", "/internal/compose_image", Some(body), 120.0)
            .await
        {
            Ok(r) => r,
            Err(e) => return format!("ERROR: {}", e),
        };
        if truthy(r.get("error").unwrap_or(&Value::Null))
            || !truthy(r.get("path").unwrap_or(&Value::Null))
        {
            let why = if truthy(r.get("error").unwrap_or(&Value::Null)) {
                sget(&r, "error")
            } else {
                omuse_common::pyjson::py_repr(&r)
            };
            return format!(
                "ERROR: 加字失败 (caption failed): {}。不要重试超过一次。",
                why
            );
        }
        let size = format!(
            "{}x{}",
            super::util::pystr_opt(r.get("width")),
            super::util::pystr_opt(r.get("height"))
        );
        let path = get_str(&r, "path");
        self.inner
            .lock()
            .last_image
            .insert(tid.clone(), path.clone());
        self.event(
            &tid,
            "image",
            json!({"paths": [path], "model": "local-compose", "size": size, "kind": "caption",
                                         "latency_s": 0, "prompt": take_chars(&text, 300)}),
        )
        .await;
        if wants_send(a) {
            let sent = self
                .send_file(t, &json!({"path": path, "note": sget(a, "title")}))
                .await
                .unwrap_or_else(|e| format!("ERROR: {}", e));
            if sent.starts_with("ERROR") {
                return format!(
                    "已加字 text added: {}，但发送失败 but sending failed: {}",
                    path, sent
                );
            }
            return format!("已把文字加到图上并显示在对话里 text added and shown: {}（字号 {}px）。不要再 send_file；文字是本机排版的，字对字准确。Don't send again; the text is letter-perfect.",
                           path, super::util::pystr_opt(r.get("font_px")));
        }
        format!("已加字 text added: {}（未发送）。", path)
    }

    /// upscale_image: local 2x/4x Lanczos resampling + unsharp mask (no new detail).
    pub async fn upscale_image(&self, t: &Value, a: &Value) -> String {
        let tid = get_str(t, "id");
        let mut rel = sget(a, "path").trim().to_string();
        if rel.is_empty() {
            rel = self
                .inner
                .lock()
                .last_image
                .get(&tid)
                .cloned()
                .unwrap_or_default();
        }
        if rel.is_empty() {
            return "ERROR: 没有图片可以放大 (no image): 先 make_image 生成一张，或给 path。"
                .into();
        }
        if let Err(e) = self.img_read(&rel) {
            return format!("ERROR: {}", e);
        }
        let factor: u32 = match a.get("factor").filter(|v| truthy(v)) {
            None => 2,
            Some(v) => match omuse_common::util::as_i64(v) {
                Some(f) if f >= 4 => 4,
                _ => 2,
            },
        };
        let src = match self.path(&rel) {
            Ok(p) => p,
            Err(e) => return format!("ERROR: {}", e.msg),
        };
        let stem = splitext(&rel).0;
        let ws = self.workspace.clone();
        let me = self.arc();
        let job = tokio::task::spawn_blocking(move || -> Result<(String, u32, u32), LocalErr> {
            let im = image::open(&src).map_err(|e| LocalErr::other("Exception", e.to_string()))?;
            let (w, h) = (im.width(), im.height());
            if w * factor > 8192 || h * factor > 8192 {
                return Err(LocalErr::value(format!(
                    "放大后超过 8192px 上限 (would exceed 8192px): {}x{}",
                    w * factor,
                    h * factor
                )));
            }
            let big = im.resize_exact(
                w * factor,
                h * factor,
                image::imageops::FilterType::Lanczos3,
            );
            let big = big.unsharpen(1.2, 2);
            let mut out = format!("{}_x{}.png", stem, factor);
            let mut k = 2;
            while me.path(&out).map(|p| p.exists()).unwrap_or(false) {
                out = format!("{}_x{}-{}.png", stem, factor, k);
                k += 1;
            }
            let full = me.path(&out)?;
            if let Some(d) = full.parent() {
                std::fs::create_dir_all(d)?;
            }
            big.save_with_format(&full, image::ImageFormat::Png)
                .map_err(|e| LocalErr::other("Exception", e.to_string()))?;
            let _ = ws;
            Ok((out, w, h))
        });
        let (out, w, h) = match job.await {
            Ok(Ok(r)) => r,
            Ok(Err(e)) if e.name == "ValueError" => return format!("ERROR: {}", e.msg),
            Ok(Err(e)) => {
                return format!(
                    "ERROR: 放大失败 upscale failed: {}",
                    take_chars(&e.msg, 200)
                )
            }
            Err(e) => {
                return format!(
                    "ERROR: 放大失败 upscale failed: {}",
                    take_chars(&e.to_string(), 200)
                )
            }
        };
        let (bw, bh) = (w * factor, h * factor);
        self.inner
            .lock()
            .last_image
            .insert(tid.clone(), out.clone());
        self.event(&tid, "image", json!({"paths": [out], "model": "local-lanczos", "size": format!("{}x{}", bw, bh),
                                         "kind": "upscale", "latency_s": 0, "prompt": format!("{}x of {}", factor, rel)})).await;
        if wants_send(a) {
            let sent = self
                .send_file(t, &json!({"path": out, "note": sget(a, "title")}))
                .await
                .unwrap_or_else(|e| format!("ERROR: {}", e));
            if sent.starts_with("ERROR") {
                return format!(
                    "已放大 upscaled: {}（{}x{}），但发送失败 but sending failed: {}",
                    out, bw, bh, sent
                );
            }
            return format!("已放大 {}x 并显示在对话里 upscaled and shown: {}（{}x{} → {}x{}）。说明一下这是像素放大、不会新增细节。Mention it is pixel upscaling, no new detail.", factor, out, w, h, bw, bh);
        }
        format!("已放大 upscaled: {}（{}x{}，未发送）。", out, bw, bh)
    }
}
