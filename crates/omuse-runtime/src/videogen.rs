//! Video generation client: the OpenAI-compatible Videos API. `POST {base}/videos` starts a job,
//! `GET {base}/videos/{id}` polls it and `GET {base}/videos/{id}/content` downloads the clip — OpenAI's Sora models,
//! xAI's grok-imagine-video behind an OpenAI-style gateway, and any local server with the same schema. A server that
//! answers the POST with the clip itself (`data[0].b64_json` / `url`, the Images API shape) is taken as it is.
//!
//! Same pattern as `imagegen`: the optional request fields are dropped one by one when a server rejects them by name,
//! the sizes walk a fallback list (with the shape as `aspect_ratio` for ratio-based servers), and the key rules are
//! `media_auth_headers("video", …)`. Settings in force (`store::effective_model`): video_base_url ("" = the model
//! endpoint), video_model ("" = auto-pick a video model the endpoint serves), video_api_key ("" = the model endpoint's
//! key) — the config's `[model] video_url` / `video` / `video_api_key` for the system's models, the user's own optional
//! values with "use my own model". Chat options (`service_tier`, `reasoning_effort`) never reach these requests.

use std::sync::Arc;
use std::time::Duration;

use base64::Engine;
use omuse_common::util::{get_str, now_ts, take_chars};
use omuse_common::Config;
use serde_json::{json, Map, Value};

use crate::imagegen::{
    aspect_ratio, media_auth_headers, shape_of, ImageBody, ImageRequest, ImageResponse,
    ImageTransport, ReqwestImageTransport,
};

/// Model ids that generate video. Editing / extension variants are skipped (`NOT_VID`).
pub const VID_KEYS: &[&str] = &[
    "video", "sora", "veo", "kling", "wan2", "wan-", "seedance", "hailuo", "runway", "luma",
    "pika", "cogvideo", "hunyuan", "mochi", "ltx",
];
pub const NOT_VID: &[&str] = &[
    "videoedit",
    "video-edit",
    "video-extend",
    "video-remix",
    "embed",
    "-vl",
    "vision",
    "caption",
    "whisper",
    "tts",
];
pub const VID_MODES: &[&str] = &["video", "video_generation", "text-to-video", "t2v"];

/// Candidate sizes per shape, best first: OpenAI's Sora sizes and xAI's 720p / 480p frames. Walked like the image
/// sizes when a server rejects one; the shape also goes as `aspect_ratio` for servers that take ratios.
pub fn video_sizes(shape: &str) -> &'static [&'static str] {
    match shape {
        "landscape" => &["1280x720", "1792x1024", "854x480"],
        "portrait" => &["720x1280", "1024x1792", "480x854"],
        _ => &["720x720", "1024x1024", "1280x720"],
    }
}

/// Clip length bounds (seconds) the tool accepts; servers snap or reject beyond their own set.
pub const SECONDS_MIN: i64 = 1;
pub const SECONDS_MAX: i64 = 15;
pub const SECONDS_DEFAULT: i64 = 5;
/// OpenAI's Sora lengths: tried when a server rejects the requested length as unsupported.
const OPENAI_SECONDS: [i64; 3] = [4, 8, 12];

/// Optional request fields, dropped one by one when a server says it doesn't know them.
pub const OPTIONAL_FIELDS: &[&str] = &["seconds", "size", "aspect_ratio"];

/// `VideoError`: the message is relayed to the agent.
#[derive(Debug, Clone, thiserror::Error)]
#[error("{0}")]
pub struct VideoError(pub String);

fn verr<T>(msg: impl Into<String>) -> Result<T, VideoError> {
    Err(VideoError(msg.into()))
}

/// Generated clips allowed per task (`[model] video_budget`).
pub fn video_budget(cfg: &Config) -> u32 {
    cfg.model.video_budget
}

/// The first model that generates video: an explicit video mode wins, else judge by the name. Loading models are
/// skipped; chat / vision-language / embedding / speech models and video editors are never picked.
pub fn pick_video_model(models: &[Value]) -> String {
    let mut by_name: Vec<String> = Vec::new();
    for m in models {
        let mid = get_str(m, "id");
        if mid.is_empty() {
            continue;
        }
        let low = mid.to_lowercase();
        let mode = {
            let a = get_str(m, "mode");
            if a.is_empty() {
                get_str(m, "type")
            } else {
                a
            }
        }
        .to_lowercase();
        let ready = {
            let r = get_str(m, "readiness");
            if r.is_empty() {
                "ready".to_string()
            } else {
                r.to_lowercase()
            }
        };
        if !matches!(ready.as_str(), "ready" | "running" | "loaded") {
            continue;
        }
        if VID_MODES.contains(&mode.as_str()) {
            return mid;
        }
        if !mode.is_empty() {
            continue;
        }
        if NOT_VID.iter().any(|k| low.contains(k)) {
            continue;
        }
        if VID_KEYS.iter().any(|k| low.contains(k)) {
            by_name.push(mid);
        }
    }
    by_name.into_iter().next().unwrap_or_default()
}

fn moderated(status: u16, text: &str) -> bool {
    let t = text.to_lowercase();
    matches!(status, 400 | 403 | 422)
        && [
            "content_policy",
            "safety system",
            "moderation",
            "not allowed by our",
            "violat",
            "rejected as a result of",
            "blocked",
        ]
        .iter()
        .any(|k| t.contains(k))
}

fn bad_size(text: &str) -> bool {
    let t = text.to_lowercase();
    ["size", "dimension", "resolution", "width", "height"]
        .iter()
        .any(|k| t.contains(k))
}

fn bad_seconds(text: &str) -> bool {
    let t = text.to_lowercase();
    ["seconds", "duration", "length"]
        .iter()
        .any(|k| t.contains(k))
}

/// What a job's `status` means.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobState {
    Pending,
    Done,
    Failed,
}

pub fn job_state(status: &str) -> JobState {
    match status.trim().to_lowercase().as_str() {
        "completed" | "done" | "succeeded" | "success" | "finished" | "ready" => JobState::Done,
        "failed" | "error" | "errored" | "expired" | "cancelled" | "canceled" | "rejected" => {
            JobState::Failed
        }
        _ => JobState::Pending,
    }
}

/// Where a response keeps the clip: `("b64", data)` or `("url", url)`. Covers the OpenAI job object, xAI's
/// `video.url`, and Images-API-style `data[0]`.
pub fn clip_ref(v: &Value) -> Option<(&'static str, String)> {
    let first = |k: &str| {
        v.get(k)
            .and_then(|d| d.as_array())
            .and_then(|a| a.first())
            .cloned()
            .unwrap_or(Value::Null)
    };
    let data0 = first("data");
    let out0 = first("outputs");
    let spots: [&Value; 6] = [v, &v["video"], &data0, &v["output"], &v["result"], &out0];
    for s in spots {
        if !s.is_object() {
            continue;
        }
        let b64 = get_str(s, "b64_json");
        if !b64.is_empty() {
            return Some(("b64", b64));
        }
    }
    for s in spots {
        if !s.is_object() {
            continue;
        }
        for k in ["url", "video_url", "output_url", "download_url"] {
            let u = get_str(s, k);
            if u.starts_with("http://") || u.starts_with("https://") {
                return Some(("url", u));
            }
        }
    }
    None
}

fn job_error(v: &Value) -> String {
    let e = &v["error"];
    let msg = if e.is_string() {
        get_str(v, "error")
    } else {
        let m = get_str(e, "message");
        if m.is_empty() {
            get_str(v, "failure_reason")
        } else {
            m
        }
    };
    if msg.is_empty() {
        get_str(v, "status")
    } else {
        msg
    }
}

fn round1(x: f64) -> f64 {
    (x * 10.0).round() / 10.0
}

/// `{"video": bytes, "model", "size", "seconds", "latency_s", "job_id"}`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct VideoResult {
    pub video: Vec<u8>,
    pub model: String,
    /// The size or aspect ratio the server took.
    pub size: String,
    pub seconds: i64,
    pub latency_s: f64,
    /// The server's job id ("" for a synchronous server).
    pub job_id: String,
}

/// Keyword arguments of [`VideoClient::generate`].
#[derive(Debug, Clone, Default)]
pub struct GenerateVideoOpts {
    /// 0 = [`SECONDS_DEFAULT`].
    pub seconds: i64,
    pub size: Option<String>,
    pub aspect: Option<String>,
    pub model: Option<String>,
    /// Overall time allowed, including the job's queue (0 / None = `llm_timeout` from the settings, at least 900 s).
    pub timeout: Option<f64>,
}

fn clamp_seconds(n: i64) -> i64 {
    (if n == 0 { SECONDS_DEFAULT } else { n }).clamp(SECONDS_MIN, SECONDS_MAX)
}

fn timeout_of(settings: &Value, timeout: Option<f64>) -> f64 {
    timeout.filter(|t| *t > 0.0).unwrap_or_else(|| {
        omuse_common::util::as_f64(&settings["llm_timeout"])
            .filter(|t| *t > 0.0)
            .unwrap_or(600.0)
            .max(900.0)
    })
}

/// The video client (bound to a transport and the configuration).
pub struct VideoClient {
    cfg: Arc<Config>,
    transport: Arc<dyn ImageTransport>,
    /// Seconds between two polls of a job (tests shorten it).
    poll_s: f64,
}

impl VideoClient {
    pub fn new(cfg: Arc<Config>) -> Self {
        Self::with_transport(cfg, Arc::new(ReqwestImageTransport::default()), 3.0)
    }

    pub fn with_transport(
        cfg: Arc<Config>,
        transport: Arc<dyn ImageTransport>,
        poll_s: f64,
    ) -> Self {
        Self {
            cfg,
            transport,
            poll_s,
        }
    }

    fn req(
        &self,
        method: &'static str,
        url: String,
        headers: Vec<(String, String)>,
        body: Option<Value>,
        timeout: f64,
    ) -> ImageRequest {
        ImageRequest {
            method,
            url,
            headers,
            body: body.map(ImageBody::Json),
            timeout,
            connect_timeout: 15.0,
        }
    }

    fn headers(&self, settings: &Value) -> Vec<(String, String)> {
        media_auth_headers("video", Some(settings), &self.cfg)
    }

    /// `GET {base}/models` → the model dicts.
    pub async fn list_models(
        &self,
        base: &str,
        timeout: f64,
        settings: &Value,
    ) -> Result<Vec<Value>, VideoError> {
        let r = self
            .transport
            .send(ImageRequest {
                connect_timeout: timeout,
                ..self.req(
                    "GET",
                    format!("{}/models", base),
                    self.headers(settings),
                    None,
                    timeout,
                )
            })
            .await
            .map_err(|e| VideoError(e.message))?;
        if r.status >= 400 {
            return verr(format!("HTTP {} from {}/models", r.status, base));
        }
        let data = r.json();
        let list = data
            .get("data")
            .filter(|v| omuse_common::util::truthy(v))
            .or_else(|| data.get("models"))
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();
        Ok(list.into_iter().filter(|m| m.is_object()).collect())
    }

    /// (base_url, model) for video generation, or a [`VideoError`] with a message the agent can relay.
    pub async fn resolve(&self, settings: &Value) -> Result<(String, String), VideoError> {
        let b = {
            let v = get_str(settings, "video_base_url");
            if v.trim().is_empty() {
                get_str(settings, "model_base_url")
            } else {
                v
            }
        };
        let base = b.trim().trim_end_matches('/').to_string();
        if base.is_empty() {
            return verr(
                "没有配置模型地址 (no model endpoint configured: Settings → Model endpoint)",
            );
        }
        let model = get_str(settings, "video_model").trim().to_string();
        if !model.is_empty() {
            return Ok((base, model));
        }
        let model = match self.list_models(&base, 15.0, settings).await {
            Ok(ms) => pick_video_model(&ms),
            Err(e) => {
                return verr(format!(
                    "无法读取模型列表 (could not list models at the endpoint): {}",
                    take_chars(&e.0, 120)
                ))
            }
        };
        if model.is_empty() {
            return verr("模型端点上没有视频生成模型 (the endpoint serves no video-generation model). 在 Settings → Video model 填一个视频模型（如 sora-2 / grok-imagine-video / veo），或设置 video_base_url 指向一个支持 OpenAI Videos API 的服务 (set Settings → Video model, or point video_base_url at an OpenAI-Videos-compatible server).");
        }
        Ok((base, model))
    }

    /// Bytes from a clip reference: decode `b64`, or download the URL (no auth: signed links).
    async fn fetch_clip(&self, r: (&str, String), tmo: f64) -> Result<Vec<u8>, VideoError> {
        match r {
            ("b64", b64) => base64::engine::general_purpose::STANDARD
                .decode(b64.trim())
                .map_err(|e| VideoError(format!("Error: {}", e))),
            (_, url) => {
                let rr = self
                    .transport
                    .send(self.req("GET", url, vec![], None, tmo))
                    .await
                    .map_err(|e| VideoError(e.message))?;
                if rr.status >= 400 {
                    return verr(format!(
                        "视频下载失败 (video download failed) HTTP {}",
                        rr.status
                    ));
                }
                Ok(rr.body)
            }
        }
    }

    /// A finished job's clip: a reference in the job object, else `GET {base}/videos/{id}/content`.
    async fn job_clip(
        &self,
        base: &str,
        id: &str,
        job: &Value,
        headers: &[(String, String)],
        tmo: f64,
    ) -> Result<Vec<u8>, VideoError> {
        if let Some(r) = clip_ref(job) {
            return self.fetch_clip(r, tmo).await;
        }
        let rr = self
            .transport
            .send(self.req(
                "GET",
                format!("{}/videos/{}/content", base, id),
                headers.to_vec(),
                None,
                tmo,
            ))
            .await
            .map_err(|e| VideoError(e.message))?;
        if rr.status >= 400 {
            return verr(format!(
                "视频下载失败 (video download failed) HTTP {}: {}",
                rr.status,
                take_chars(&rr.text(), 200)
            ));
        }
        // a JSON answer at /content points at the file instead of being it
        if rr.body.first() == Some(&b'{') {
            if let Some(r) = clip_ref(&rr.json()) {
                return self.fetch_clip(r, tmo).await;
            }
            return verr("视频生成没有返回视频 (the server returned no video data)");
        }
        Ok(rr.body)
    }

    /// Poll `GET {base}/videos/{id}` until the job is done or failed, within `deadline` (epoch seconds).
    async fn wait_job(
        &self,
        base: &str,
        id: &str,
        headers: &[(String, String)],
        deadline: f64,
        tmo: f64,
    ) -> Result<Value, VideoError> {
        let mut wait = self.poll_s;
        loop {
            if now_ts() >= deadline {
                return verr(format!(
                    "视频生成超时 (video generation timed out; job {}) — 稍后再试一次 try once more later",
                    id
                ));
            }
            tokio::time::sleep(Duration::from_secs_f64(wait.max(0.001))).await;
            wait = (wait * 1.5).min(10.0_f64.max(self.poll_s));
            let r = self
                .transport
                .send(self.req(
                    "GET",
                    format!("{}/videos/{}", base, id),
                    headers.to_vec(),
                    None,
                    tmo.min(60.0),
                ))
                .await;
            let r = match r {
                Ok(r) => r,
                Err(e) if e.timeout => continue,
                Err(e) => return verr(e.message),
            };
            if r.status >= 500 || r.status == 429 {
                continue; // a transient server error: keep polling
            }
            if r.status >= 400 {
                return verr(format!(
                    "视频任务查询失败 (polling the video job failed) HTTP {}: {}",
                    r.status,
                    take_chars(&r.text(), 200)
                ));
            }
            let j = r.json();
            match job_state(&get_str(&j, "status")) {
                JobState::Done => return Ok(j),
                JobState::Failed => {
                    let msg = job_error(&j);
                    if moderated(400, &msg) {
                        return verr("内容被模型的安全策略拒绝 (the video model's safety policy declined this prompt) — 换一个描述；不要重试同样的内容 rephrase; do not retry the same prompt");
                    }
                    return verr(format!(
                        "视频生成失败 (the video job failed): {}",
                        take_chars(&msg, 300)
                    ));
                }
                JobState::Pending => {}
            }
        }
    }

    /// Make one clip. Starts the job, polls it and downloads the result; a synchronous server's answer is used
    /// directly. Walks the size fallbacks and drops optional fields the server doesn't know.
    pub async fn generate(
        &self,
        settings: &Value,
        prompt: &str,
        o: &GenerateVideoOpts,
    ) -> Result<VideoResult, VideoError> {
        let prompt = prompt.trim();
        if prompt.is_empty() {
            return verr("需要视频描述 (prompt required)");
        }
        let (base, mut mdl) = self.resolve(settings).await?;
        if let Some(m) = o.model.as_ref().filter(|m| !m.is_empty()) {
            mdl = m.clone();
        }
        let mut secs = clamp_seconds(o.seconds);
        let (shape, img_sizes) = shape_of(o.size.as_deref(), o.aspect.as_deref());
        // an explicit WxH first, then the shape's video sizes
        let mut sizes: Vec<String> = Vec::new();
        if let Some(s) = o.size.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
            if img_sizes.first().map(String::as_str) == Some(s) {
                sizes.push(s.to_string());
            }
        }
        for s in video_sizes(&shape) {
            if !sizes.iter().any(|x| x == s) {
                sizes.push((*s).to_string());
            }
        }
        let ratio = aspect_ratio(&shape);
        let mut attempts: Vec<(Option<String>, Option<String>)> =
            sizes.iter().map(|s| (Some(s.clone()), None)).collect();
        if ratio != "1:1" {
            attempts.insert(1.min(attempts.len()), (None, Some(ratio.to_string())));
        }
        let t0 = now_ts();
        let tmo = timeout_of(settings, o.timeout);
        let deadline = t0 + tmo;
        let headers = self.headers(settings);
        let mut body = Map::new();
        body.insert("model".into(), json!(mdl));
        body.insert("prompt".into(), json!(prompt));
        body.insert("seconds".into(), json!(secs.to_string()));
        let mut last = String::new();
        let mut snapped = false;
        for (sz, ar) in &attempts {
            body.shift_remove("size");
            body.shift_remove("aspect_ratio");
            if let Some(sz) = sz {
                body.insert("size".into(), json!(sz));
            }
            if let Some(ar) = ar {
                body.insert("aspect_ratio".into(), json!(ar));
            }
            let label = sz.clone().or_else(|| ar.clone()).unwrap_or_default();
            let mut resp: Option<ImageResponse> = None;
            let mut next_size = false;
            for _ in 0..OPTIONAL_FIELDS.len() + 3 {
                let r = self
                    .transport
                    .send(self.req(
                        "POST",
                        format!("{}/videos", base),
                        headers.clone(),
                        Some(Value::Object(body.clone())),
                        tmo.min(120.0),
                    ))
                    .await;
                let r = match r {
                    Ok(r) => r,
                    Err(e) if e.timeout => {
                        return verr("视频生成超时 (video generation timed out) — 稍后再试一次 try once more later")
                    }
                    Err(e) => return verr(e.message),
                };
                if r.status < 400 {
                    resp = Some(r);
                    break;
                }
                let txt = take_chars(&r.text(), 300).to_string();
                let low = txt.to_lowercase();
                if moderated(r.status, &txt) {
                    return verr("内容被模型的安全策略拒绝 (the video model's safety policy declined this prompt) — 换一个描述；不要重试同样的内容 rephrase; do not retry the same prompt");
                }
                if ar.is_some() && r.status == 400 && low.contains("aspect") {
                    last = txt;
                    next_size = true;
                    break; // the server doesn't take aspect_ratio: on to the next size
                }
                let unknown = [
                    "unknown parameter",
                    "unrecognized",
                    "unexpected",
                    "extra_forbidden",
                    "unsupported parameter",
                ]
                .iter()
                .any(|k| low.contains(k));
                let mut hit: Vec<&str> = OPTIONAL_FIELDS
                    .iter()
                    .copied()
                    .filter(|f| body.contains_key(*f) && low.contains(f) && unknown)
                    .collect();
                if hit.is_empty() && unknown {
                    hit = OPTIONAL_FIELDS
                        .iter()
                        .copied()
                        .filter(|f| body.contains_key(*f))
                        .take(1)
                        .collect();
                }
                if r.status == 400 && !hit.is_empty() {
                    body.shift_remove(hit[0]);
                    continue;
                }
                if matches!(r.status, 400 | 422)
                    && body.contains_key("seconds")
                    && bad_seconds(&low)
                {
                    // the length is known but not offered: snap to OpenAI's set once, then leave it to the server
                    if !snapped {
                        snapped = true;
                        let near = OPENAI_SECONDS
                            .iter()
                            .copied()
                            .min_by_key(|s| (s - secs).abs())
                            .unwrap_or(secs);
                        if near != secs {
                            secs = near;
                            body.insert("seconds".into(), json!(secs.to_string()));
                            continue;
                        }
                    }
                    body.shift_remove("seconds");
                    continue;
                }
                if matches!(r.status, 400 | 422) && sz.is_some() && bad_size(&low) {
                    last = txt;
                    next_size = true;
                    break; // try the next size
                }
                return verr(format!(
                    "视频生成失败 video generation failed HTTP {}: {}",
                    r.status, txt
                ));
            }
            if next_size {
                continue;
            }
            let Some(resp) = resp else {
                return verr(format!(
                    "视频生成失败 video generation failed: {}",
                    take_chars(&last, 200)
                ));
            };
            let data = resp.json();
            // a synchronous server: the clip is in the answer
            let (video, job_id, size) = if let Some(r) = clip_ref(&data)
                .filter(|_| data.get("data").is_some() || get_str(&data, "status").is_empty())
            {
                (self.fetch_clip(r, tmo).await?, String::new(), label.clone())
            } else {
                let id = {
                    let i = get_str(&data, "id");
                    if i.is_empty() {
                        get_str(&data, "request_id")
                    } else {
                        i
                    }
                };
                if id.is_empty() {
                    return verr(
                        "视频生成没有返回任务号 (the server returned neither a video nor a job id)",
                    );
                }
                let job = match job_state(&get_str(&data, "status")) {
                    JobState::Done => data.clone(),
                    JobState::Failed => {
                        return verr(format!(
                            "视频生成失败 (the video job failed): {}",
                            take_chars(&job_error(&data), 300)
                        ))
                    }
                    JobState::Pending => self.wait_job(&base, &id, &headers, deadline, tmo).await?,
                };
                let taken = {
                    let s = get_str(&job, "size");
                    if s.is_empty() {
                        label.clone()
                    } else {
                        s
                    }
                };
                (
                    self.job_clip(&base, &id, &job, &headers, tmo).await?,
                    id,
                    taken,
                )
            };
            if video.is_empty() {
                return verr("视频生成没有返回视频 (the server returned no video data)");
            }
            return Ok(VideoResult {
                video,
                model: mdl,
                size,
                seconds: secs,
                latency_s: round1(now_ts() - t0),
                job_id,
            });
        }
        verr(format!(
            "服务不接受任何尺寸 (the server rejected every size tried: {}): {}",
            sizes.join(", "),
            last
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::imagegen::TransportError;
    use futures::future::BoxFuture;
    use parking_lot::Mutex;

    #[test]
    fn video_model_picking() {
        let ms = vec![
            json!({"id": "Qwen3-VL-8B", "mode": "chat"}),
            json!({"id": "gpt-image-1"}),
            json!({"id": "wan2.7-videoedit"}),
            json!({"id": "sora-2", "readiness": "loading"}),
            json!({"id": "xai/grok-imagine-video"}),
            json!({"id": "my-clipper", "mode": "video"}),
        ];
        assert_eq!(pick_video_model(&ms), "my-clipper");
        assert_eq!(pick_video_model(&ms[..5]), "xai/grok-imagine-video");
        assert_eq!(pick_video_model(&ms[..3]), "");
        assert_eq!(job_state("in_progress"), JobState::Pending);
        assert_eq!(job_state("Completed"), JobState::Done);
        assert_eq!(job_state("expired"), JobState::Failed);
        assert_eq!(
            clip_ref(&json!({"status": "done", "video": {"url": "https://cdn/x.mp4"}})),
            Some(("url", "https://cdn/x.mp4".to_string()))
        );
        assert_eq!(
            clip_ref(&json!({"data": [{"b64_json": "AAAA"}]})),
            Some(("b64", "AAAA".to_string()))
        );
        assert_eq!(clip_ref(&json!({"id": "v1", "status": "completed"})), None);
    }

    // ---------------------------------------------------------------- fake Videos API
    #[derive(Default)]
    struct Fake {
        /// answer the POST with the clip (Images-API shape) instead of a job
        sync: bool,
        /// polls before the job is done
        polls_needed: usize,
        fail_job: Option<&'static str>,
        /// the job object carries `video.url` (xAI) instead of needing /content
        url_in_job: bool,
        reject_sizes: Vec<&'static str>,
        reject_fields: Vec<&'static str>,
        aspect_ok: bool,
        /// reject `seconds` values outside OpenAI's set
        openai_seconds: bool,
        models: Option<Value>,
        never_done: bool,
        /// POST bodies; polls and downloads as {"get": url}
        calls: Mutex<Vec<Value>>,
        headers: Mutex<Vec<Vec<(String, String)>>>,
        polls: Mutex<usize>,
    }

    const MP4: &[u8] = b"\x00\x00\x00\x18ftypmp42fakemp4";

    fn r(status: u16, v: Value) -> Result<ImageResponse, TransportError> {
        Ok(ImageResponse {
            status,
            body: serde_json::to_vec(&v).unwrap(),
        })
    }

    impl ImageTransport for Fake {
        fn send(&self, req: ImageRequest) -> BoxFuture<'_, Result<ImageResponse, TransportError>> {
            let out = (|| {
                self.headers.lock().push(req.headers.clone());
                if req.method == "GET" {
                    self.calls.lock().push(json!({"get": req.url}));
                    if req.url.ends_with("/models") {
                        return r(
                            200,
                            json!({"data": self.models.clone().unwrap_or(json!([{"id": "sora-2"}]))}),
                        );
                    }
                    if req.url.ends_with("/content") || req.url.ends_with(".mp4") {
                        return Ok(ImageResponse {
                            status: 200,
                            body: MP4.to_vec(),
                        });
                    }
                    // a poll
                    let mut p = self.polls.lock();
                    *p += 1;
                    if self.never_done {
                        return r(
                            200,
                            json!({"id": "vid_1", "status": "in_progress", "progress": 10}),
                        );
                    }
                    if let Some(msg) = self.fail_job {
                        return r(
                            200,
                            json!({"id": "vid_1", "status": "failed", "error": {"message": msg}}),
                        );
                    }
                    if *p < self.polls_needed {
                        return r(
                            200,
                            json!({"id": "vid_1", "status": "in_progress", "progress": 50}),
                        );
                    }
                    let mut j = json!({"id": "vid_1", "status": "completed", "size": "1280x720", "seconds": "5"});
                    if self.url_in_job {
                        j["video"] = json!({"url": "https://cdn.example/vid_1.mp4"});
                    }
                    return r(200, j);
                }
                let Some(ImageBody::Json(j)) = req.body.clone() else {
                    return r(400, json!({"error": {"message": "json body expected"}}));
                };
                self.calls.lock().push(j.clone());
                for f in &self.reject_fields {
                    if j.get(*f).is_some() {
                        return r(
                            400,
                            json!({"error": {"message": format!("Unknown parameter: '{}'", f)}}),
                        );
                    }
                }
                if j.get("aspect_ratio").is_some() && !self.aspect_ok {
                    return r(
                        400,
                        json!({"error": {"message": "Unknown parameter: 'aspect_ratio'."}}),
                    );
                }
                let size = get_str(&j, "size");
                if self.reject_sizes.contains(&size.as_str()) {
                    return r(
                        400,
                        json!({"error": {"message": format!("size must be one of 1280x720, 720x1280, got {}", size)}}),
                    );
                }
                if self.openai_seconds {
                    let s = get_str(&j, "seconds");
                    if !["4", "8", "12"].contains(&s.as_str()) {
                        return r(
                            400,
                            json!({"error": {"message": "Invalid value: seconds must be one of '4', '8', '12'"}}),
                        );
                    }
                }
                if self.sync {
                    return r(
                        200,
                        json!({"data": [{"b64_json": base64::engine::general_purpose::STANDARD.encode(MP4)}]}),
                    );
                }
                r(
                    200,
                    json!({"id": "vid_1", "object": "video", "status": "queued", "progress": 0}),
                )
            })();
            Box::pin(async move { out })
        }
    }

    fn client(f: Fake) -> (VideoClient, Arc<Fake>) {
        let f = Arc::new(f);
        let cfg = Arc::new(
            Config::parse("[model]\napi_key='sys-key'\nvideo_api_key='sys-vid-key'").unwrap(),
        );
        (VideoClient::with_transport(cfg, f.clone(), 0.001), f)
    }

    #[tokio::test]
    async fn job_is_polled_and_downloaded_with_the_video_key() {
        let (c, f) = client(Fake {
            polls_needed: 2,
            ..Default::default()
        });
        // the user's own set with their optional video endpoint and key; chat options present but never sent
        let s = json!({"model_override": true, "model_api_key": "my-key", "model_base_url": "http://llm/v1",
                       "video_base_url": "http://vid/v1/", "video_api_key": "vid-key", "video_model": "sora-2",
                       "fast_mode": true, "reasoning_effort": "high", "llm_timeout": 5});
        let res = c
            .generate(
                &s,
                "a paper boat drifting on a pond",
                &GenerateVideoOpts {
                    aspect: Some("landscape".into()),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        assert_eq!(res.video, MP4);
        assert_eq!(
            (
                res.model.as_str(),
                res.size.as_str(),
                res.seconds,
                res.job_id.as_str()
            ),
            ("sora-2", "1280x720", 5, "vid_1")
        );
        let calls = f.calls.lock();
        assert_eq!(calls[0]["model"], "sora-2");
        assert_eq!(calls[0]["seconds"], "5");
        assert_eq!(calls[0]["size"], "1280x720");
        assert!(
            calls[0].get("service_tier").is_none() && calls[0].get("reasoning_effort").is_none()
        );
        assert_eq!(calls[1]["get"], "http://vid/v1/videos/vid_1");
        assert_eq!(calls[2]["get"], "http://vid/v1/videos/vid_1");
        assert_eq!(calls[3]["get"], "http://vid/v1/videos/vid_1/content");
        for h in f.headers.lock().iter() {
            assert_eq!(
                h,
                &vec![("Authorization".to_string(), "Bearer vid-key".to_string())]
            );
        }
    }

    #[tokio::test]
    async fn system_models_use_the_config_video_key_and_auto_pick() {
        let (c, f) = client(Fake {
            url_in_job: true,
            ..Default::default()
        });
        let s = json!({"model_override": false, "model_api_key": "sys-key", "model_base_url": "http://sys/v1",
                       "video_base_url": "", "video_model": ""});
        let res = c
            .generate(&s, "waves", &GenerateVideoOpts::default())
            .await
            .unwrap();
        assert_eq!(res.model, "sora-2"); // from /models
        assert_eq!(res.video, MP4);
        let calls = f.calls.lock();
        assert_eq!(calls[0]["get"], "http://sys/v1/models");
        assert_eq!(
            calls.last().unwrap()["get"],
            "https://cdn.example/vid_1.mp4"
        );
        let hs = f.headers.lock();
        assert_eq!(
            hs[0],
            vec![(
                "Authorization".to_string(),
                "Bearer sys-vid-key".to_string()
            )]
        );
        assert!(hs.last().unwrap().is_empty()); // the signed download link: no key
    }

    #[tokio::test]
    async fn synchronous_servers_fields_and_seconds_fallbacks() {
        let (c, f) = client(Fake {
            sync: true,
            reject_fields: vec!["size"],
            openai_seconds: true,
            ..Default::default()
        });
        let s = json!({"model_base_url": "http://x/v1", "video_model": "local-t2v"});
        let res = c
            .generate(
                &s,
                "rain",
                &GenerateVideoOpts {
                    seconds: 6,
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        assert_eq!(res.video, MP4);
        assert_eq!(res.job_id, "");
        assert_eq!(res.seconds, 4); // 6 is not offered: snapped to OpenAI's nearest
        let calls = f.calls.lock();
        assert!(calls.last().unwrap().get("size").is_none());
        assert_eq!(calls.last().unwrap()["seconds"], "4");
    }

    #[tokio::test]
    async fn size_walk_and_aspect_ratio() {
        let (c, f) = client(Fake {
            sync: true,
            reject_sizes: vec!["720x1280"],
            ..Default::default()
        });
        let s = json!({"model_base_url": "http://x/v1", "video_model": "m"});
        let res = c
            .generate(
                &s,
                "a tower",
                &GenerateVideoOpts {
                    aspect: Some("portrait".into()),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        assert_eq!(res.size, "1024x1792");
        {
            let calls = f.calls.lock();
            assert_eq!(calls[0]["size"], "720x1280");
            assert_eq!(calls[1]["aspect_ratio"], "9:16"); // tried once, unknown here
            assert_eq!(calls[2]["size"], "1024x1792");
        }
        let (c, f) = client(Fake {
            sync: true,
            reject_sizes: vec!["720x1280", "1024x1792", "480x854"],
            aspect_ok: true,
            ..Default::default()
        });
        let res = c
            .generate(
                &s,
                "a tower",
                &GenerateVideoOpts {
                    aspect: Some("portrait".into()),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        assert_eq!(res.size, "9:16");
        assert!(f.calls.lock()[1].get("size").is_none());
    }

    #[tokio::test]
    async fn failures_are_explained() {
        let (c, _) = client(Fake {
            fail_job: Some("Your request was rejected as a result of our safety system."),
            ..Default::default()
        });
        let s = json!({"model_base_url": "http://x/v1", "video_model": "m"});
        let e = c
            .generate(&s, "x", &GenerateVideoOpts::default())
            .await
            .unwrap_err();
        assert!(e.0.contains("safety policy"), "{}", e.0);
        let (c, _) = client(Fake {
            fail_job: Some("GPU exploded"),
            ..Default::default()
        });
        let e = c
            .generate(&s, "x", &GenerateVideoOpts::default())
            .await
            .unwrap_err();
        assert!(e.0.contains("GPU exploded"), "{}", e.0);
        let (c, _) = client(Fake {
            never_done: true,
            ..Default::default()
        });
        let e = c
            .generate(
                &s,
                "x",
                &GenerateVideoOpts {
                    timeout: Some(0.05),
                    ..Default::default()
                },
            )
            .await
            .unwrap_err();
        assert!(
            e.0.contains("timed out") && e.0.contains("vid_1"),
            "{}",
            e.0
        );
        // no video model anywhere
        let (c, _) = client(Fake {
            models: Some(json!([{"id": "gpt-4.1"}, {"id": "gpt-image-1"}])),
            ..Default::default()
        });
        let e = c
            .generate(
                &json!({"model_base_url": "http://x/v1"}),
                "x",
                &GenerateVideoOpts::default(),
            )
            .await
            .unwrap_err();
        assert!(e.0.contains("no video-generation model"), "{}", e.0);
    }
}
