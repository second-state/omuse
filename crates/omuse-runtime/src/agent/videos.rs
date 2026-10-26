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

// ------------------------------------------------------------------ edit_video (ffmpeg, local)

/// Candidate font files for captions (Debian `fonts-noto-cjk`, then common fallbacks); none → fontconfig's default.
pub const CAPTION_FONTS: &[&str] = &[
    "/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc",
    "/usr/share/fonts/opentype/noto/NotoSansCJK-Bold.ttc",
    "/usr/share/fonts/truetype/noto/NotoSansCJK-Regular.ttc",
    "/usr/share/fonts/truetype/liberation/LiberationSans-Regular.ttf",
    "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
];
/// Clips one `concat` may join.
pub const CONCAT_MAX: usize = 20;
/// Seconds one ffmpeg run may take.
pub const FFMPEG_TIMEOUT_S: u64 = 900;

/// `(width, height)` of a resize preset.
pub fn preset_size(preset: &str) -> Option<(u32, u32)> {
    match preset.trim().to_lowercase().as_str() {
        "720p" | "hd" => Some((1280, 720)),
        "1080p" | "fhd" => Some((1920, 1080)),
        "square" | "1:1" => Some((1080, 1080)),
        "vertical" | "portrait" | "9:16" | "story" | "reel" | "shorts" => Some((1080, 1920)),
        _ => None,
    }
}

/// ffmpeg's `atempo` takes 0.5–2.0 per stage: chain stages for larger factors.
pub fn atempo_chain(factor: f64) -> String {
    let mut f = factor.clamp(0.25, 4.0);
    let mut parts = Vec::new();
    while f > 2.0 {
        parts.push("atempo=2.0".to_string());
        f /= 2.0;
    }
    while f < 0.5 {
        parts.push("atempo=0.5".to_string());
        f /= 0.5;
    }
    parts.push(format!("atempo={:.4}", f));
    parts.join(",")
}

/// The `scale,pad` filter that fits a frame into `w`×`h` with black bars, even dimensions, square pixels.
pub fn fit_filter(w: u32, h: u32) -> String {
    let (w, h) = (w & !1, h & !1);
    format!(
        "scale={w}:{h}:force_original_aspect_ratio=decrease,pad={w}:{h}:(ow-iw)/2:(oh-ih)/2,setsar=1"
    )
}

/// What `ffprobe` says about a media file.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Probe {
    pub duration_s: f64,
    pub width: u32,
    pub height: u32,
    pub has_audio: bool,
    pub has_video: bool,
    pub size: u64,
}

/// Parse `ffprobe -print_format json -show_format -show_streams` output.
pub fn parse_probe(v: &Value, size: u64) -> Probe {
    let mut p = Probe {
        size,
        ..Default::default()
    };
    p.duration_s = omuse_common::util::as_f64(&v["format"]["duration"])
        .or_else(|| get_str(&v["format"], "duration").parse().ok())
        .unwrap_or(0.0);
    for s in v["streams"].as_array().cloned().unwrap_or_default() {
        match get_str(&s, "codec_type").as_str() {
            "video" if !p.has_video => {
                p.has_video = true;
                p.width = omuse_common::util::as_i64(&s["width"]).unwrap_or(0) as u32;
                p.height = omuse_common::util::as_i64(&s["height"]).unwrap_or(0) as u32;
            }
            "audio" => p.has_audio = true,
            _ => {}
        }
    }
    p
}

fn f_arg(a: &Value, k: &str) -> Option<f64> {
    a.get(k)
        .filter(|v| truthy(v))
        .and_then(omuse_common::util::as_f64)
}

fn ffmpeg_err(stderr: &str) -> String {
    let tail: Vec<&str> = stderr
        .lines()
        .filter(|l| !l.trim().is_empty())
        .rev()
        .take(4)
        .collect();
    let mut t: Vec<&str> = tail.into_iter().rev().collect();
    t.retain(|l| !l.starts_with("  "));
    take_chars(&t.join(" | "), 400).to_string()
}

impl Runtime {
    /// A workspace video (or audio for add_audio) as an absolute path; `Err` with a message for the model.
    fn media_path(&self, rel: &str, what: &str) -> Result<std::path::PathBuf, String> {
        let rel = rel.trim();
        if rel.is_empty() {
            return Err(format!("需要{what}路径 ({what} path required)"));
        }
        let full = self.path(rel).map_err(|e| e.msg)?;
        if !full.is_file() {
            return Err(format!("找不到{what} {what} not found: {rel}"));
        }
        Ok(full)
    }

    async fn ffprobe(&self, path: &std::path::Path) -> Result<Probe, String> {
        let out = tokio::time::timeout(
            std::time::Duration::from_secs(60),
            tokio::process::Command::new("ffprobe")
                .args([
                    "-v",
                    "error",
                    "-print_format",
                    "json",
                    "-show_format",
                    "-show_streams",
                ])
                .arg(path)
                .output(),
        )
        .await
        .map_err(|_| "ffprobe timed out".to_string())?
        .map_err(|e| format!("ffprobe: {e}"))?;
        if !out.status.success() {
            return Err(format!(
                "ffprobe: {}",
                ffmpeg_err(&String::from_utf8_lossy(&out.stderr))
            ));
        }
        let v: Value = serde_json::from_slice(&out.stdout).unwrap_or(Value::Null);
        let size = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
        Ok(parse_probe(&v, size))
    }

    async fn ffmpeg(&self, args: &[String]) -> Result<(), String> {
        let out = tokio::time::timeout(
            std::time::Duration::from_secs(FFMPEG_TIMEOUT_S),
            tokio::process::Command::new("ffmpeg")
                .args(["-hide_banner", "-nostdin", "-y", "-loglevel", "error"])
                .args(args)
                .output(),
        )
        .await
        .map_err(|_| "ffmpeg timed out (over 15 minutes)".to_string())?
        .map_err(|e| format!("ffmpeg: {e}"))?;
        if !out.status.success() {
            return Err(format!(
                "ffmpeg: {}",
                ffmpeg_err(&String::from_utf8_lossy(&out.stderr))
            ));
        }
        Ok(())
    }

    /// edit_video: one ffmpeg operation on workspace videos; the result lands in videos/<title>.mp4 and is shown.
    pub async fn edit_video(&self, t: &Value, a: &Value) -> String {
        let tid = get_str(t, "id");
        let op = sget(a, "op").trim().to_lowercase();
        if op.is_empty() {
            return "ERROR: 需要 op（info / concat / trim / resize / speed / mute / add_audio / caption / gif）op required.".into();
        }
        let t0 = omuse_common::util::now_ts();
        // inputs
        let inputs: Vec<std::path::PathBuf> = if op == "concat" {
            let list: Vec<String> = a["paths"]
                .as_array()
                .map(|v| v.iter().map(omuse_common::util::py_str).collect())
                .unwrap_or_default();
            if list.len() < 2 {
                return "ERROR: concat 需要 paths 里至少两段视频 (concat needs at least two paths).".into();
            }
            if list.len() > CONCAT_MAX {
                return format!(
                    "ERROR: 一次最多拼接 {CONCAT_MAX} 段 (at most {CONCAT_MAX} clips per concat)."
                );
            }
            let mut v = Vec::new();
            for p in &list {
                match self.media_path(p, "视频 video") {
                    Ok(f) => v.push(f),
                    Err(e) => return format!("ERROR: {e}"),
                }
            }
            v
        } else {
            match self.media_path(&sget(a, "path"), "视频 video") {
                Ok(f) => vec![f],
                Err(e) => return format!("ERROR: {e}"),
            }
        };
        let mut probes = Vec::new();
        for p in &inputs {
            match self.ffprobe(p).await {
                Ok(pr) => probes.push(pr),
                Err(e) => return format!("ERROR: {} — {}", self.rel(p), e),
            }
        }
        let first = probes[0].clone();
        if op == "info" {
            let lines: Vec<String> = inputs
                .iter()
                .zip(&probes)
                .map(|(p, pr)| {
                    format!(
                        "{}: {:.1}s, {}x{}, {} MB, audio {}",
                        self.rel(p),
                        pr.duration_s,
                        pr.width,
                        pr.height,
                        pr.size / 1_048_576,
                        if pr.has_audio { "yes" } else { "no" }
                    )
                })
                .collect();
            return format!("视频信息 video info: {}", lines.join("; "));
        }
        if !first.has_video && op != "add_audio" {
            return format!("ERROR: 不是视频文件 not a video: {}", self.rel(&inputs[0]));
        }
        // output path
        let title = if truthy(a.get("title").unwrap_or(&Value::Null)) {
            sget(a, "title")
        } else {
            let stem = splitext(&super::util::basename(&inputs[0].to_string_lossy())).0;
            format!("{}-{}", stem, op)
        };
        let mut slug = take_chars(SLUG.replace_all(&title, "_").trim_matches('_'), 50).to_string();
        if slug.is_empty() {
            slug = "video".into();
        }
        let want_ext = if op == "gif" { ".gif" } else { ".mp4" };
        let explicit = truthy(a.get("output").unwrap_or(&Value::Null));
        let out = if explicit {
            sget(a, "output")
        } else {
            format!("videos/{}{}", slug, want_ext)
        };
        let (mut stem, mut ext) = splitext(&out);
        if op == "gif" {
            if ext.to_lowercase() != ".gif" {
                stem = out.clone();
            }
            ext = ".gif".into();
        } else if ![".mp4", ".mov", ".webm"].contains(&ext.to_lowercase().as_str()) {
            stem = out.clone();
            ext = ".mp4".into();
        }
        let mut rel_out = format!("{}{}", stem, ext);
        let mut k = 2;
        while self.path(&rel_out).map(|x| x.exists()).unwrap_or(false) && !explicit {
            rel_out = format!("{}-{}{}", stem, k, ext);
            k += 1;
        }
        let full_out = match self.path(&rel_out) {
            Ok(f) => f,
            Err(e) => return format!("ERROR: {}", e.msg),
        };
        if let Some(d) = full_out.parent() {
            let _ = std::fs::create_dir_all(d);
        }
        let venc = [
            "-c:v",
            "libx264",
            "-preset",
            "veryfast",
            "-crf",
            "23",
            "-pix_fmt",
            "yuv420p",
            "-movflags",
            "+faststart",
        ];
        let aenc = ["-c:a", "aac", "-b:a", "128k"];
        let s = |x: &str| x.to_string();
        let mut args: Vec<String> = Vec::new();
        let mut tmp_text: Option<std::path::PathBuf> = None;
        match op.as_str() {
            "concat" => {
                let (w, h) = (first.width.max(2), first.height.max(2));
                let all_audio = probes.iter().all(|p| p.has_audio);
                for p in &inputs {
                    args.push(s("-i"));
                    args.push(p.to_string_lossy().into_owned());
                }
                let mut fc = String::new();
                let mut maps = String::new();
                for i in 0..inputs.len() {
                    fc.push_str(&format!(
                        "[{i}:v]{},fps=30,format=yuv420p[v{i}];",
                        fit_filter(w, h)
                    ));
                    maps.push_str(&format!("[v{i}]"));
                    if all_audio {
                        maps.push_str(&format!("[{i}:a]"));
                    }
                }
                fc.push_str(&format!(
                    "{maps}concat=n={}:v=1:a={}[v]{}",
                    inputs.len(),
                    if all_audio { 1 } else { 0 },
                    if all_audio { "[a]" } else { "" }
                ));
                args.extend(["-filter_complex", &fc, "-map", "[v]"].map(s));
                if all_audio {
                    args.extend(["-map", "[a]"].map(s));
                    args.extend(aenc.map(s));
                } else {
                    args.push(s("-an"));
                }
                args.extend(venc.map(s));
            }
            "trim" => {
                let start = f_arg(a, "start").unwrap_or(0.0).max(0.0);
                let end = f_arg(a, "end").unwrap_or(first.duration_s);
                if end <= start {
                    return format!("ERROR: end 必须大于 start (end must be after start; the clip is {:.1}s long)", first.duration_s);
                }
                args.extend(
                    [
                        "-ss",
                        &format!("{start:.3}"),
                        "-t",
                        &format!("{:.3}", end - start),
                        "-i",
                    ]
                    .map(s),
                );
                args.push(inputs[0].to_string_lossy().into_owned());
                args.extend(venc.map(s));
                if first.has_audio {
                    args.extend(aenc.map(s));
                }
            }
            "resize" => {
                let preset = sget(a, "preset");
                let (w, h) = match preset_size(&preset) {
                    Some(x) => x,
                    None => {
                        let w = f_arg(a, "width").unwrap_or(0.0) as u32;
                        let h = f_arg(a, "height").unwrap_or(0.0) as u32;
                        if w == 0 && h == 0 {
                            return "ERROR: resize 需要 preset 或 width/height (resize needs a preset or width / height).".into();
                        }
                        if w == 0 || h == 0 {
                            // keep the aspect: scale by the given side only
                            args.extend(["-i", &inputs[0].to_string_lossy(), "-vf"].map(s));
                            args.push(if w > 0 {
                                format!("scale={}:-2", w & !1)
                            } else {
                                format!("scale=-2:{}", h & !1)
                            });
                            args.extend(venc.map(s));
                            if first.has_audio {
                                args.extend(["-c:a", "copy"].map(s));
                            }
                            (0, 0)
                        } else {
                            (w, h)
                        }
                    }
                };
                if w > 0 {
                    args.extend(
                        ["-i", &inputs[0].to_string_lossy(), "-vf", &fit_filter(w, h)].map(s),
                    );
                    args.extend(venc.map(s));
                    if first.has_audio {
                        args.extend(["-c:a", "copy"].map(s));
                    }
                }
            }
            "speed" => {
                let f = f_arg(a, "factor").unwrap_or(0.0);
                if !(0.25..=4.0).contains(&f) {
                    return "ERROR: factor 需在 0.25–4 之间 (speed factor must be between 0.25 and 4).".into();
                }
                args.extend(["-i", &inputs[0].to_string_lossy(), "-filter_complex"].map(s));
                if first.has_audio {
                    args.push(format!(
                        "[0:v]setpts=PTS/{f:.4}[v];[0:a]{}[a]",
                        atempo_chain(f)
                    ));
                    args.extend(["-map", "[v]", "-map", "[a]"].map(s));
                    args.extend(aenc.map(s));
                } else {
                    args.push(format!("[0:v]setpts=PTS/{f:.4}[v]"));
                    args.extend(["-map", "[v]", "-an"].map(s));
                }
                args.extend(venc.map(s));
            }
            "mute" => {
                args.extend(["-i", &inputs[0].to_string_lossy(), "-c:v", "copy", "-an"].map(s));
            }
            "add_audio" => {
                let audio = match self.media_path(&sget(a, "audio_path"), "音频 audio") {
                    Ok(f) => f,
                    Err(e) => return format!("ERROR: {e}"),
                };
                args.extend(
                    [
                        "-i",
                        &inputs[0].to_string_lossy(),
                        "-stream_loop",
                        "-1",
                        "-i",
                        &audio.to_string_lossy(),
                        "-map",
                        "0:v",
                        "-map",
                        "1:a",
                        "-c:v",
                        "copy",
                        "-shortest",
                    ]
                    .map(s),
                );
                args.extend(aenc.map(s));
            }
            "caption" => {
                let text = sget(a, "text").trim().to_string();
                if text.is_empty() {
                    return "ERROR: caption 需要 text (caption text required).".into();
                }
                let dir = self.workspace.join(".tmp");
                let _ = std::fs::create_dir_all(&dir);
                let tf = dir.join(format!("caption-{}.txt", (t0 * 1000.0) as u64));
                if let Err(e) = std::fs::write(&tf, take_chars(&text, 300)) {
                    return format!("ERROR: OSError: {e}");
                }
                tmp_text = Some(tf.clone());
                let font = CAPTION_FONTS
                    .iter()
                    .find(|f| std::path::Path::new(f).exists())
                    .map(|f| format!("fontfile={f}:"))
                    .unwrap_or_default();
                let y = match sget(a, "position").trim().to_lowercase().as_str() {
                    "top" => "h*0.08",
                    "center" | "middle" => "(h-text_h)/2",
                    _ => "h*0.86-text_h",
                };
                let mut enable = String::new();
                if let Some(st) = f_arg(a, "start") {
                    let en = f_arg(a, "end").unwrap_or(first.duration_s.max(st + 1.0));
                    enable = format!(":enable='between(t,{st:.2},{en:.2})'");
                }
                let vf = format!(
                    "drawtext={font}textfile='{}':fontsize=h/16:fontcolor=white:borderw=3:bordercolor=black@0.8:x=(w-text_w)/2:y={y}{enable}",
                    tf.to_string_lossy().replace('\\', "\\\\").replace('\'', "\\'").replace(':', "\\:")
                );
                args.extend(["-i", &inputs[0].to_string_lossy(), "-vf", &vf].map(s));
                args.extend(venc.map(s));
                if first.has_audio {
                    args.extend(["-c:a", "copy"].map(s));
                }
            }
            "gif" => {
                args.extend(["-i", &inputs[0].to_string_lossy(), "-vf",
                             "fps=12,scale=480:-1:flags=lanczos,split[s0][s1];[s0]palettegen[p];[s1][p]paletteuse"].map(s));
            }
            other => {
                return format!("ERROR: 不认识的 op unknown op: {other}. Use info / concat / trim / resize / speed / mute / add_audio / caption / gif.");
            }
        }
        args.push(full_out.to_string_lossy().into_owned());
        let res = self.ffmpeg(&args).await;
        if let Some(tf) = tmp_text {
            let _ = std::fs::remove_file(tf);
        }
        if let Err(e) = res {
            return format!("ERROR: {op} 失败 failed: {e}");
        }
        let after = self.ffprobe(&full_out).await.unwrap_or_default();
        let latency = ((omuse_common::util::now_ts() - t0) * 10.0).round() / 10.0;
        self.inner
            .lock()
            .last_video
            .insert(tid.clone(), rel_out.clone());
        self.event(&tid, "video", json!({"paths": [rel_out.clone()], "op": op, "model": "ffmpeg", "size": format!("{}x{}", after.width, after.height),
                                         "seconds": after.duration_s.round() as i64, "latency_s": latency, "inputs": inputs.iter().map(|p| self.rel(p)).collect::<Vec<_>>()}))
            .await;
        let info = format!(
            "{} {:.1}s {}x{} {} MB, {}s",
            op,
            after.duration_s,
            after.width,
            after.height,
            after.size / 1_048_576,
            latency
        );
        if wants_send(a) {
            let sent = self
                .send_file(t, &json!({"path": rel_out, "note": sget(a, "title")}))
                .await
                .unwrap_or_else(|e| format!("ERROR: {}", e));
            if sent.starts_with("ERROR") {
                return format!(
                    "视频已处理: {}（{}），但发送失败 but sending failed: {}",
                    rel_out, info, sent
                );
            }
            return format!("视频已处理并显示在对话里 shown in the chat: {}（{}）。不要再用 send_file 重复发送；要继续改就把这个路径作为下一步的 path。 Don't send it again; use this path for the next op.",
                rel_out, info);
        }
        format!("视频已处理: {}（{}，未发送 not sent）。要给用户看就用 send_file；要继续改就把它作为下一步的 path。", rel_out, info)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ffmpeg_helpers() {
        assert_eq!(preset_size("720p"), Some((1280, 720)));
        assert_eq!(preset_size("vertical"), Some((1080, 1920)));
        assert_eq!(preset_size("weird"), None);
        assert_eq!(atempo_chain(2.0), "atempo=2.0000");
        assert_eq!(atempo_chain(4.0), "atempo=2.0,atempo=2.0000");
        assert_eq!(atempo_chain(0.25), "atempo=0.5,atempo=0.5000");
        assert!(fit_filter(1281, 721).starts_with("scale=1280:720:"));
        let p = parse_probe(
            &json!({"format": {"duration": "4.5"}, "streams": [{"codec_type": "video", "width": 320, "height": 240}, {"codec_type": "audio"}]}),
            1000,
        );
        assert_eq!(
            (p.duration_s, p.width, p.height, p.has_audio, p.has_video),
            (4.5, 320, 240, true, true)
        );
        assert_eq!(
            ffmpeg_err("a\n  detail\nError: no such file\n"),
            "a | Error: no such file"
        );
    }
}
