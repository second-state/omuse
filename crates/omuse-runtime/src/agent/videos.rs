//! The video tool: `make_video` (a short clip from an AI video model through `videogen`). Same shape as
//! `make_image`: a per-task budget (every call is billed), the file lands in `videos/<title>.mp4` and is shown in the
//! chat with a player (`send_file`), and setup errors tell the model to stop and point the user at Settings.

use std::sync::atomic::Ordering;
use std::sync::LazyLock;

use omuse_common::util::{get_str, take_chars, truthy};
use regex::Regex;
use serde_json::{json, Value};

use super::local::wants_send;
use super::runtime::Runtime;
use super::util::{sget, splitext};
use crate::videogen::{GenerateVideoOpts, SECONDS_DEFAULT, SECONDS_MAX, SECONDS_MIN};

static SLUG: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[^\w\-一-鿿]+").unwrap());
static CTRL: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"[\x00-\x08\x0b\x0c\x0e-\x1f\x7f]").unwrap());

/// What the model is told about a `VideoError`.
pub fn vid_error(msg: &str) -> String {
    let setup = [
        "没有视频生成模型",
        "no video-generation model",
        "video_base_url",
        "没有配置模型地址",
        "no model endpoint",
        "无法读取模型列表",
        "could not list models",
    ];
    if setup.iter().any(|k| msg.contains(k)) {
        return format!("ERROR: 视频生成没有配置 (video generation is not set up): {} 告诉用户：需要在 Settings 里配置视频模型后才能生成视频；不要重试，不要用其他方式做视频。 Tell the user a video model must be configured in Settings first; do not retry or make a video another way.", msg);
    }
    format!(
        "ERROR: {}。不要用同样的参数重试超过一次 (don't retry the same request more than once).",
        msg
    )
}

fn budget_msg(made: i64, budget: i64) -> String {
    format!("ERROR: 本任务已生成 {} 段视频，达到上限 {} 段（每段都要付费）。不要再生成：把已有的视频交付给用户，或说明情况并询问是否继续 (video budget for this task reached; deliver what you have or ask the user).", made, budget)
}

fn opt_str(a: &Value, k: &str) -> Option<String> {
    a.get(k)
        .filter(|v| truthy(v))
        .map(omuse_common::util::py_str)
}

impl Runtime {
    fn take_video_budget(&self, tid: &str) -> Result<(), String> {
        let budget = self.video_budget.load(Ordering::Relaxed);
        let mut inner = self.inner.lock();
        let made = inner.vid_count.get(tid).copied().unwrap_or(0);
        if made + 1 > budget {
            return Err(budget_msg(made, budget));
        }
        inner.vid_count.insert(tid.to_string(), made + 1);
        Ok(())
    }

    /// AI video generation (make_video); the clip lands in videos/<title>.mp4 and is shown in the chat.
    pub async fn make_video(&self, t: &Value, a: &Value) -> String {
        let tid = get_str(t, "id");
        let prompt = take_chars(CTRL.replace_all(&sget(a, "prompt"), " ").trim(), 4000).to_string();
        if prompt.is_empty() {
            return "ERROR: 需要 prompt（画面和动作描述）prompt required.".into();
        }
        let seconds = a
            .get("seconds")
            .and_then(omuse_common::util::as_i64)
            .filter(|n| *n > 0)
            .map(|n| n.clamp(SECONDS_MIN, SECONDS_MAX))
            .unwrap_or(SECONDS_DEFAULT);
        // cost guard: a task gets a small budget of clips (each call bills the user and takes minutes)
        if let Err(m) = self.take_video_budget(&tid) {
            return m;
        }
        let s = self.settings();
        let o = GenerateVideoOpts {
            seconds,
            size: opt_str(a, "size"),
            aspect: opt_str(a, "aspect"),
            ..Default::default()
        };
        let res = match self.videos.generate(&s, &prompt, &o).await {
            Ok(r) => r,
            Err(e) => return vid_error(&e.0),
        };
        let title = if truthy(a.get("title").unwrap_or(&Value::Null)) {
            sget(a, "title")
        } else {
            take_chars(&prompt, 40).to_string()
        };
        let mut slug = take_chars(SLUG.replace_all(&title, "_").trim_matches('_'), 50).to_string();
        if slug.is_empty() {
            slug = "video".into();
        }
        let explicit = truthy(a.get("output").unwrap_or(&Value::Null));
        let out = if explicit {
            sget(a, "output")
        } else {
            format!("videos/{}.mp4", slug)
        };
        let (mut stem, mut ext) = splitext(&out);
        if ![".mp4", ".webm", ".mov"].contains(&ext.to_lowercase().as_str()) {
            stem = out.clone();
            ext = ".mp4".into();
        }
        let mut p = format!("{}{}", stem, ext);
        let mut k = 2;
        while self.path(&p).map(|x| x.exists()).unwrap_or(false) && !explicit {
            p = format!("{}-{}{}", stem, k, ext);
            k += 1;
        }
        let full = match self.path(&p) {
            Ok(f) => f,
            Err(e) => return format!("ERROR: {}", e.msg),
        };
        if let Some(d) = full.parent() {
            let _ = std::fs::create_dir_all(d);
        }
        if let Err(e) = std::fs::write(&full, &res.video) {
            return format!("ERROR: OSError: {}", e);
        }
        self.inner.lock().last_video.insert(tid.clone(), p.clone());
        self.event(&tid, "video", json!({"paths": [p.clone()], "model": res.model, "size": res.size, "seconds": res.seconds,
                                         "latency_s": res.latency_s, "prompt": take_chars(&prompt, 300)}))
            .await;
        let info = format!(
            "{} {} {}s clip, {}s",
            res.model,
            res.size,
            res.seconds,
            super::util::fmt_float(res.latency_s)
        );
        if wants_send(a) {
            let sent = self
                .send_file(t, &json!({"path": p, "note": sget(a, "title")}))
                .await
                .unwrap_or_else(|e| format!("ERROR: {}", e));
            if sent.starts_with("ERROR") {
                return format!(
                    "视频已生成: {}（{}），但发送失败 but sending failed: {}",
                    p, info, sent
                );
            }
            return format!("视频已生成并显示在对话里 shown in the chat: {}（{}）。不要再用 send_file 重复发送；在回答里用一两句话描述内容并问用户是否要调整（要改就换 prompt 再调一次 make_video）。 Don't send it again; describe it briefly and offer changes (a new make_video with a new prompt).",
                p, info);
        }
        format!(
            "视频已生成: {}（{}，未发送 not sent）。要给用户看就用 send_file。",
            p, info
        )
    }
}
