//! Image generation client: the OpenAI-compatible Images API (POST {base}/images/generations) — the Olares Router,
//! OpenAI (gpt-image-1 / DALL·E 3), or any local server that speaks the same schema. Replaceable by design, like llm.py.
//!
//! Settings (runtime Settings): image_base_url ("" = the model endpoint), image_model ("" = auto-pick an image model
//! the endpoint serves). The key comes from the configuration only (`[model] image_api_key`, the Python
//! `OMUSE_IMAGE_API_KEY`, else the model key), never from Settings.

use std::sync::{Arc, LazyLock};
use std::time::Duration;

use base64::Engine;
use futures::future::BoxFuture;
use omuse_common::util::{get_str, get_truthy, now_ts, take_chars};
use omuse_common::Config;
use regex::Regex;
use serde_json::{json, Map, Value};

use crate::llm::auth_headers;

/// Model ids that generate images (vision-LANGUAGE models like Qwen-VL understand images, they don't make them).
pub const IMG_KEYS: &[&str] = &[
    "gpt-image",
    "dall",
    "flux",
    "diffus",
    "sdxl",
    "sd3",
    "sd-",
    "stable",
    "imagen",
    "kolors",
    "janus",
    "qwen-image",
    "hidream",
    "playground",
    "image",
];
pub const NOT_IMG: &[&str] = &[
    "embed", "rerank", "whisper", "-vl", "vision", "tts", "kokoro", "speech", "ocr", "caption",
];
pub const IMG_MODES: &[&str] = &[
    "image",
    "image_generation",
    "images",
    "text-to-image",
    "t2i",
];

/// Aspect words / ratios -> candidate sizes, best first. gpt-image-1 wants 1024x1024 / 1536x1024 / 1024x1536,
/// DALL·E 3 wants 1024x1024 / 1792x1024 / 1024x1792, local SD servers take most multiples of 64 — so each shape lists
/// fallbacks and generate() walks them when the server rejects a size.
pub fn sizes(shape: &str) -> &'static [&'static str] {
    match shape {
        "landscape" => &["1536x1024", "1792x1024", "1344x768", "1024x1024"],
        "portrait" => &["1024x1536", "1024x1792", "768x1344", "1024x1024"],
        _ => &["1024x1024", "1024x1024"],
    }
}

/// `ImageError`: the message is relayed to the agent.
#[derive(Debug, Clone, thiserror::Error)]
#[error("{0}")]
pub struct ImageError(pub String);

fn ierr<T>(msg: impl Into<String>) -> Result<T, ImageError> {
    Err(ImageError(msg.into()))
}

/// Generated pictures allowed per task (`IMAGE_BUDGET_PER_TASK`, `[model] image_budget`).
pub fn image_budget(cfg: &Config) -> u32 {
    cfg.model.image_budget
}

// Round 3 — style control. A short style word (English or Chinese) becomes a concrete prompt suffix the image models
// respond to, like Midjourney's style keywords / DALL·E's style presets. Unknown words are passed through as
// "<word> style".
pub const STYLE_PRESETS: &[(&str, &str)] = &[
    ("photorealistic", "photorealistic, ultra-detailed, natural lighting, shot on a DSLR, 85mm lens, shallow depth of field"),
    ("cinematic", "cinematic still, dramatic lighting, anamorphic lens, film grain, moody colour grading"),
    ("anime", "anime style, clean line art, cel shading, vibrant colours, Studio-quality key visual"),
    ("watercolor", "watercolor painting, soft washes, visible paper texture, loose brushwork, pastel palette"),
    ("oil painting", "oil painting, visible impasto brushstrokes, rich warm palette, classical composition"),
    ("3d render", "3D render, octane render, soft studio lighting, subsurface scattering, high detail"),
    ("flat vector", "flat vector illustration, minimal shapes, bold solid colours, clean geometry, no gradients"),
    ("pixel art", "pixel art, 16-bit, limited palette, crisp pixels"),
    ("sketch", "pencil sketch, hand-drawn lines, cross-hatching, white paper"),
    ("ink wash", "traditional Chinese ink wash painting (shuimo), expressive brush strokes, rice paper, negative space"),
    ("line art", "black-and-white line art, single-weight outlines, no shading"),
    ("isometric", "isometric illustration, clean 30° geometry, soft shadows"),
    ("low poly", "low-poly 3D style, faceted surfaces, pastel lighting"),
    ("pop art", "pop art, bold outlines, halftone dots, saturated primary colours"),
    ("minimalist", "minimalist composition, lots of negative space, one focal subject, muted palette"),
    ("logo", "clean logo design, flat vector, centred mark, simple shapes, solid background, no text unless asked"),
    ("product", "product photography, white seamless background, soft box lighting, sharp focus, commercial quality"),
];
const STYLE_ZH: &[(&str, &str)] = &[
    ("写实", "photorealistic"),
    ("真实", "photorealistic"),
    ("照片", "photorealistic"),
    ("摄影", "photorealistic"),
    ("电影", "cinematic"),
    ("电影感", "cinematic"),
    ("动漫", "anime"),
    ("二次元", "anime"),
    ("日漫", "anime"),
    ("水彩", "watercolor"),
    ("油画", "oil painting"),
    ("3d", "3d render"),
    ("三维", "3d render"),
    ("渲染", "3d render"),
    ("扁平", "flat vector"),
    ("矢量", "flat vector"),
    ("插画", "flat vector"),
    ("像素", "pixel art"),
    ("素描", "sketch"),
    ("铅笔", "sketch"),
    ("水墨", "ink wash"),
    ("国画", "ink wash"),
    ("线稿", "line art"),
    ("等距", "isometric"),
    ("低多边形", "low poly"),
    ("波普", "pop art"),
    ("极简", "minimalist"),
    ("简约", "minimalist"),
    ("logo", "logo"),
    ("标志", "logo"),
    ("产品图", "product"),
    ("商品图", "product"),
    ("电商", "product"),
];

fn preset(key: &str) -> &'static str {
    STYLE_PRESETS
        .iter()
        .find(|(k, _)| *k == key)
        .map(|(_, v)| *v)
        .unwrap_or("")
}

/// The prompt suffix for a style word ('水彩', 'anime', 'oil painting' …); '' when no style was given.
pub fn style_suffix(style: Option<&str>) -> String {
    let s = style.unwrap_or("").trim().to_lowercase();
    if s.is_empty() {
        return String::new();
    }
    if let Some((_, v)) = STYLE_PRESETS.iter().find(|(k, _)| *k == s) {
        return (*v).to_string();
    }
    if let Some((_, en)) = STYLE_ZH.iter().find(|(zh, _)| s.contains(zh)) {
        return preset(en).to_string();
    }
    let s_ns = s.replace(' ', "");
    for (key, suffix) in STYLE_PRESETS {
        // 'photo' / 'realistic' / 'water color' …
        let k = key.replace(' ', "");
        if s_ns.contains(&k) || k.contains(&s_ns) {
            return (*suffix).to_string();
        }
    }
    format!("{} style", s)
}

/// The prompt with the style suffix appended (once).
pub fn with_style(prompt: &str, style: Option<&str>) -> String {
    let suf = style_suffix(style);
    if suf.is_empty() {
        return prompt.to_string();
    }
    let head = suf.split(',').next().unwrap_or("").to_lowercase();
    if prompt.to_lowercase().contains(&head) {
        return prompt.to_string();
    }
    format!("{}, {}", prompt.trim_end_matches(['.', ' ']), suf)
}

/// The key for the image endpoint: with the user's own model settings in force, their key (the system's image key
/// must never go to a user's endpoint); otherwise `[model] image_api_key`, else the model endpoint's key.
pub fn image_auth_headers(settings: Option<&Value>, cfg: &Config) -> Vec<(String, String)> {
    if settings.is_some_and(|s| get_truthy(s, "model_override")) {
        return auth_headers(settings, cfg);
    }
    let key = cfg.model.image_api_key.trim();
    if !key.is_empty() {
        return vec![("Authorization".to_string(), format!("Bearer {}", key))];
    }
    auth_headers(settings, cfg)
}

/// The first model that generates images: an explicit image mode wins, else judge by the name. Loading models are
/// skipped; vision-language / embedding / speech models are never picked.
pub fn pick_image_model(models: &[Value]) -> String {
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
        if IMG_MODES.contains(&mode.as_str()) {
            return mid;
        }
        if !mode.is_empty() {
            continue; // the server says it is something else (chat / embedding / tts …)
        }
        if NOT_IMG.iter().any(|k| low.contains(k)) {
            continue;
        }
        if IMG_KEYS.iter().any(|k| low.contains(k)) {
            by_name.push(mid);
        }
    }
    by_name.into_iter().next().unwrap_or_default()
}

fn dedup(v: &[&str]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for x in v {
        if !out.iter().any(|o| o == x) {
            out.push((*x).to_string());
        }
    }
    out
}

/// Normalise what the model asked for into a shape name and the sizes to try, best first. Accepts '1024x1024',
/// ratios like '16:9' / '9:16' / '1:1' / '4:3', or words (square / landscape / portrait / wide / tall).
pub fn shape_of(size: Option<&str>, aspect: Option<&str>) -> (String, Vec<String>) {
    static SIZE_RE: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"^(\d{3,4})\s*[x×*]\s*(\d{3,4})$").unwrap());
    static RATIO_RE: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"^(\d{1,2})\s*[:/]\s*(\d{1,2})$").unwrap());
    let raw = size.filter(|s| !s.is_empty()).or(aspect).unwrap_or("");
    let s = raw.trim().to_lowercase();
    let shape_for = |a: u32, b: u32| {
        if a == b {
            "square"
        } else if a > b {
            "landscape"
        } else {
            "portrait"
        }
    };
    if let Some(m) = SIZE_RE.captures(&s) {
        // an explicit size: try it first, then the matching shape's fallbacks
        let (w, h): (u32, u32) = (m[1].parse().unwrap_or(0), m[2].parse().unwrap_or(0));
        let shape = shape_for(w, h);
        let first = format!("{}x{}", w, h);
        let mut out = vec![first.clone()];
        out.extend(
            sizes(shape)
                .iter()
                .filter(|x| **x != first)
                .map(|x| x.to_string()),
        );
        return (shape.to_string(), out);
    }
    if let Some(m) = RATIO_RE.captures(&s) {
        let (a, b): (u32, u32) = (m[1].parse().unwrap_or(0), m[2].parse().unwrap_or(0));
        let shape = shape_for(a, b);
        return (shape.to_string(), dedup(sizes(shape)));
    }
    if matches!(s.as_str(), "" | "square" | "1" | "方" | "方形" | "正方形") {
        return ("square".into(), vec!["1024x1024".into()]);
    }
    if [
        "land",
        "wide",
        "horizontal",
        "横",
        "宽",
        "banner",
        "16:9",
        "cover",
        "desktop",
        "桌面",
        "电脑",
    ]
    .iter()
    .any(|k| s.contains(k))
    {
        return ("landscape".into(), dedup(sizes("landscape")));
    }
    if [
        "port", "tall", "vertical", "竖", "phone", "手机", "story", "poster", "海报",
    ]
    .iter()
    .any(|k| s.contains(k))
    {
        return ("portrait".into(), dedup(sizes("portrait")));
    }
    ("square".into(), vec!["1024x1024".into()])
}

fn bad_size(text: &str) -> bool {
    let t = text.to_lowercase();
    ["size", "dimension", "resolution", "width", "height"]
        .iter()
        .any(|k| t.contains(k))
}

/// Optional request fields: dropped one by one when a server says it doesn't know them (OpenAI rejects
/// negative_prompt, local SD servers reject background / output_format / quality …), so one code path serves every
/// vendor.
pub const OPTIONAL_FIELDS: &[&str] = &[
    "response_format",
    "quality",
    "negative_prompt",
    "background",
    "output_format",
    "style",
];

fn moderated(status: u16, text: &str) -> bool {
    let t = text.to_lowercase();
    matches!(status, 400 | 403)
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

// ------------------------------------------------------------------ transport

/// A request body.
#[derive(Debug, Clone)]
pub enum ImageBody {
    Json(Value),
    /// Text fields and file parts `(field, filename, bytes, mime)`.
    Multipart {
        fields: Vec<(String, String)>,
        files: Vec<(String, String, Vec<u8>, String)>,
    },
}

/// One HTTP exchange with the images server.
#[derive(Debug, Clone)]
pub struct ImageRequest {
    pub method: &'static str,
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub body: Option<ImageBody>,
    pub timeout: f64,
    pub connect_timeout: f64,
}

/// Status and raw body.
#[derive(Debug, Clone, Default)]
pub struct ImageResponse {
    pub status: u16,
    pub body: Vec<u8>,
}

impl ImageResponse {
    /// `r.text`.
    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }
    /// `r.json()` (`null` when the body is not JSON).
    pub fn json(&self) -> Value {
        serde_json::from_slice(&self.body).unwrap_or(Value::Null)
    }
}

/// A transport failure: `timeout` = httpx.TimeoutException; `message` = `"<ExceptionName>: <detail>"`.
#[derive(Debug, Clone)]
pub struct TransportError {
    pub timeout: bool,
    pub message: String,
}

/// The HTTP layer (replaceable in tests).
pub trait ImageTransport: Send + Sync {
    fn send(&self, req: ImageRequest) -> BoxFuture<'_, Result<ImageResponse, TransportError>>;
}

/// The real transport (reqwest, rustls, no proxy).
pub struct ReqwestImageTransport {
    client: reqwest::Client,
}

impl Default for ReqwestImageTransport {
    fn default() -> Self {
        Self {
            client: reqwest::Client::builder()
                .connect_timeout(Duration::from_secs(15))
                .no_proxy()
                .build()
                .expect("reqwest client"),
        }
    }
}

impl ImageTransport for ReqwestImageTransport {
    fn send(&self, req: ImageRequest) -> BoxFuture<'_, Result<ImageResponse, TransportError>> {
        Box::pin(async move {
            let terr = |e: reqwest::Error| TransportError {
                timeout: e.is_timeout(),
                message: crate::rt_api::describe_reqwest_error(&e),
            };
            let mut b = match req.method {
                "GET" => self.client.get(&req.url),
                _ => self.client.post(&req.url),
            };
            b = b.timeout(Duration::from_secs_f64(req.timeout.max(0.001)));
            for (k, v) in &req.headers {
                b = b.header(k.as_str(), v.as_str());
            }
            match req.body {
                Some(ImageBody::Json(j)) => b = b.json(&j),
                Some(ImageBody::Multipart { fields, files }) => {
                    let mut form = reqwest::multipart::Form::new();
                    for (k, v) in fields {
                        form = form.text(k, v);
                    }
                    for (field, name, bytes, mime) in files {
                        let part = reqwest::multipart::Part::bytes(bytes)
                            .file_name(name)
                            .mime_str(&mime)
                            .map_err(|e| TransportError {
                                timeout: false,
                                message: format!("ValueError: {}", e),
                            })?;
                        form = form.part(field, part);
                    }
                    b = b.multipart(form);
                }
                None => {}
            }
            let r = b.send().await.map_err(terr)?;
            let status = r.status().as_u16();
            let body = r.bytes().await.map_err(terr)?.to_vec();
            Ok(ImageResponse { status, body })
        })
    }
}

// ------------------------------------------------------------------ results / options

/// `{"images": [bytes …], "model", "size", "shape", "latency_s", "revised_prompt", "via"?}`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ImageResult {
    pub images: Vec<Vec<u8>>,
    pub model: String,
    pub size: String,
    pub shape: String,
    pub latency_s: f64,
    pub revised_prompt: String,
    /// `variation()` only: "variations" or "edit".
    pub via: Option<String>,
}

/// Keyword arguments of [`ImageClient::generate`].
#[derive(Debug, Clone, Default)]
pub struct GenerateOpts {
    pub size: Option<String>,
    pub aspect: Option<String>,
    pub n: i64,
    pub quality: Option<String>,
    pub model: Option<String>,
    pub timeout: Option<f64>,
    pub negative_prompt: Option<String>,
    pub background: Option<String>,
    pub output_format: Option<String>,
    /// DALL·E 3's vivid/natural.
    pub vendor_style: Option<String>,
}

/// Keyword arguments of [`ImageClient::edit`] / [`ImageClient::variation`].
#[derive(Debug, Clone, Default)]
pub struct EditOpts {
    pub mask: Option<Vec<u8>>,
    pub size: Option<String>,
    pub aspect: Option<String>,
    pub n: i64,
    pub model: Option<String>,
    pub timeout: Option<f64>,
    /// `variation()` only: extra instruction.
    pub prompt: String,
}

pub const VARIATION_PROMPT: &str = "Create a close variation of this image: keep the same subject, composition, style and palette, with small natural differences in details and pose";

fn guess_mime(name: &str) -> &'static str {
    let n = name.to_lowercase();
    if n.ends_with(".jpg") || n.ends_with(".jpeg") {
        "image/jpeg"
    } else if n.ends_with(".webp") {
        "image/webp"
    } else {
        "image/png"
    }
}

fn round1(x: f64) -> f64 {
    (x * 10.0).round() / 10.0
}

fn clamp_n(n: i64) -> i64 {
    (if n == 0 { 1 } else { n }).clamp(1, 4)
}

fn timeout_of(settings: &Value, timeout: Option<f64>) -> f64 {
    timeout
        .filter(|t| *t != 0.0)
        .or_else(|| omuse_common::util::as_f64(&settings["llm_timeout"]).filter(|t| *t != 0.0))
        .unwrap_or(600.0)
}

// ------------------------------------------------------------------ client

/// The image client (`imagegen` module functions bound to a transport and the configuration).
pub struct ImageClient {
    cfg: Arc<Config>,
    transport: Arc<dyn ImageTransport>,
}

impl ImageClient {
    pub fn new(cfg: Arc<Config>) -> Self {
        Self::with_transport(cfg, Arc::new(ReqwestImageTransport::default()))
    }

    pub fn with_transport(cfg: Arc<Config>, transport: Arc<dyn ImageTransport>) -> Self {
        Self { cfg, transport }
    }

    fn req(
        &self,
        method: &'static str,
        url: String,
        headers: Vec<(String, String)>,
        body: Option<ImageBody>,
        timeout: f64,
    ) -> ImageRequest {
        ImageRequest {
            method,
            url,
            headers,
            body,
            timeout,
            connect_timeout: 15.0,
        }
    }

    /// `GET {base}/models` → the model dicts.
    pub async fn list_models(
        &self,
        base: &str,
        timeout: f64,
        settings: Option<&Value>,
    ) -> Result<Vec<Value>, ImageError> {
        let r = self
            .transport
            .send(ImageRequest {
                connect_timeout: timeout,
                ..self.req(
                    "GET",
                    format!("{}/models", base),
                    image_auth_headers(settings, &self.cfg),
                    None,
                    timeout,
                )
            })
            .await
            .map_err(|e| ImageError(e.message))?;
        if r.status >= 400 {
            return ierr(format!("HTTP {} from {}/models", r.status, base));
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

    /// (base_url, model) for image generation, or an [`ImageError`] with a message the agent can relay.
    pub async fn resolve(&self, settings: &Value) -> Result<(String, String), ImageError> {
        let b = {
            let i = get_str(settings, "image_base_url");
            if i.is_empty() {
                get_str(settings, "model_base_url")
            } else {
                i
            }
        };
        let base = b.trim_end_matches('/').to_string();
        if base.is_empty() {
            return ierr(
                "没有配置模型地址 (no model endpoint configured: Settings → Model endpoint)",
            );
        }
        let model = get_str(settings, "image_model").trim().to_string();
        if !model.is_empty() {
            return Ok((base, model));
        }
        let model = match self.list_models(&base, 15.0, Some(settings)).await {
            Ok(ms) => pick_image_model(&ms),
            Err(e) => {
                return ierr(format!(
                    "无法读取模型列表 (could not list models at the endpoint): {}",
                    take_chars(&e.0, 120)
                ))
            }
        };
        if model.is_empty() {
            return ierr("模型端点上没有图像生成模型 (the endpoint serves no image-generation model). 在 Settings → Image model 填一个图像模型（如 gpt-image-1 / dall-e-3 / FLUX），或设置 image_base_url 指向一个支持 OpenAI Images API 的服务 (set Settings → Image model, or point image_base_url at an OpenAI-Images-compatible server).");
        }
        Ok((base, model))
    }

    async fn collect(&self, items: &[Value], timeout: f64) -> Result<Vec<Vec<u8>>, ImageError> {
        let mut out = Vec::new();
        for it in items {
            let b64 = get_str(it, "b64_json");
            if !b64.is_empty() {
                let bytes = base64::engine::general_purpose::STANDARD
                    .decode(b64.trim())
                    .map_err(|e| ImageError(format!("Error: {}", e)))?;
                out.push(bytes);
            } else {
                let url = get_str(it, "url");
                if url.is_empty() {
                    continue;
                }
                let rr = self
                    .transport
                    .send(self.req("GET", url, vec![], None, timeout))
                    .await;
                match rr {
                    Ok(rr) if rr.status < 400 => out.push(rr.body),
                    Ok(_) => {}
                    Err(e) => return ierr(e.message),
                }
            }
        }
        Ok(out)
    }

    /// Make `n` images. Walks the size fallbacks when the server rejects one, and drops optional fields the server
    /// doesn't know — so gpt-image-1, DALL·E 3 and local SD servers all work without per-vendor code.
    pub async fn generate(
        &self,
        settings: &Value,
        prompt: &str,
        o: &GenerateOpts,
    ) -> Result<ImageResult, ImageError> {
        let prompt = prompt.trim();
        if prompt.is_empty() {
            return ierr("需要图片描述 (prompt required)");
        }
        let (base, mut mdl) = self.resolve(settings).await?;
        if let Some(m) = o.model.as_ref().filter(|m| !m.is_empty()) {
            mdl = m.clone();
        }
        let n = clamp_n(o.n);
        let (shape, sizes) = shape_of(o.size.as_deref(), o.aspect.as_deref());
        let t0 = now_ts();
        let tmo = timeout_of(settings, o.timeout);
        let mut last = String::new();
        let mut body = Map::new();
        body.insert("model".into(), json!(mdl));
        body.insert("prompt".into(), json!(prompt));
        body.insert("n".into(), json!(n));
        body.insert("response_format".into(), json!("b64_json"));
        for (k, v) in [
            ("quality", &o.quality),
            ("negative_prompt", &o.negative_prompt),
            ("background", &o.background),
            ("output_format", &o.output_format),
            ("style", &o.vendor_style),
        ] {
            if let Some(v) = v.as_ref().filter(|v| !v.is_empty()) {
                body.insert(k.into(), json!(v));
            }
        }
        let headers = image_auth_headers(Some(settings), &self.cfg);
        for sz in &sizes {
            body.insert("size".into(), json!(sz));
            let mut resp: Option<ImageResponse> = None;
            let mut next_size = false;
            for _ in 0..OPTIONAL_FIELDS.len() + 1 {
                // strip fields the server rejects, then retry the same size
                let r = self
                    .transport
                    .send(self.req(
                        "POST",
                        format!("{}/images/generations", base),
                        headers.clone(),
                        Some(ImageBody::Json(Value::Object(body.clone()))),
                        tmo,
                    ))
                    .await;
                let r = match r {
                    Ok(r) => r,
                    Err(e) if e.timeout => {
                        return ierr("图像生成超时 (image generation timed out) — 稍后再试一次 try once more later")
                    }
                    Err(e) => return ierr(e.message),
                };
                if r.status < 400 {
                    resp = Some(r);
                    break;
                }
                let txt = take_chars(&r.text(), 300).to_string();
                let low = txt.to_lowercase();
                if moderated(r.status, &txt) {
                    return ierr("内容被模型的安全策略拒绝 (the image model's safety policy declined this prompt) — 换一个描述；不要重试同样的内容 rephrase; do not retry the same prompt");
                }
                let mut hit: Vec<&str> = OPTIONAL_FIELDS
                    .iter()
                    .copied()
                    .filter(|f| body.contains_key(*f) && low.contains(f))
                    .collect();
                if hit.is_empty()
                    && [
                        "unknown parameter",
                        "unrecognized",
                        "unexpected",
                        "extra_forbidden",
                    ]
                    .iter()
                    .any(|k| low.contains(k))
                {
                    hit = OPTIONAL_FIELDS
                        .iter()
                        .copied()
                        .filter(|f| body.contains_key(*f))
                        .take(1)
                        .collect();
                }
                if r.status == 400 && !hit.is_empty() {
                    body.shift_remove(hit[0]);
                    resp = Some(r);
                    continue;
                }
                if r.status == 400 && bad_size(&low) {
                    last = txt;
                    next_size = true;
                    break; // try the next size
                }
                return ierr(format!(
                    "图像生成失败 image generation failed HTTP {}: {}",
                    r.status, txt
                ));
            }
            if next_size {
                continue;
            }
            let data = resp.map(|r| r.json()).unwrap_or(Value::Null);
            let items: Vec<Value> = data
                .get("data")
                .and_then(|d| d.as_array())
                .cloned()
                .unwrap_or_default();
            let out = self.collect(&items, tmo).await?;
            if out.is_empty() {
                return ierr("图像生成没有返回图片 (the server returned no image data)");
            }
            return Ok(ImageResult {
                images: out,
                model: mdl,
                size: sz.clone(),
                shape,
                latency_s: round1(now_ts() - t0),
                revised_prompt: items
                    .first()
                    .map(|i| get_str(i, "revised_prompt"))
                    .unwrap_or_default(),
                via: None,
            });
        }
        ierr(format!(
            "服务不接受任何尺寸 (the server rejected every size tried: {}): {}",
            sizes.join(", "),
            last
        ))
    }

    /// POST {base}/images/edits (multipart). `images` = [(filename, bytes)…]: the first is the picture being edited;
    /// more are reference images (gpt-image-1 takes several). `mask`: PNG whose transparent pixels mark the area to
    /// repaint (inpainting). Size fallbacks and unknown-field dropping as in generate().
    pub async fn edit(
        &self,
        settings: &Value,
        images: &[(String, Vec<u8>)],
        prompt: &str,
        o: &EditOpts,
    ) -> Result<ImageResult, ImageError> {
        let prompt = prompt.trim();
        if images.is_empty() {
            return ierr("需要一张图片 (an image is required)");
        }
        if prompt.is_empty() {
            return ierr("需要说明怎么改 (prompt required: what to change)");
        }
        let (base, mut mdl) = self.resolve(settings).await?;
        if let Some(m) = o.model.as_ref().filter(|m| !m.is_empty()) {
            mdl = m.clone();
        }
        let n = clamp_n(o.n);
        let (shape, sizes) = shape_of(o.size.as_deref(), o.aspect.as_deref());
        let t0 = now_ts();
        let tmo = timeout_of(settings, o.timeout);
        let mut data: Vec<(String, String)> = vec![
            ("model".into(), mdl.clone()),
            ("prompt".into(), prompt.to_string()),
            ("n".into(), n.to_string()),
            ("response_format".into(), "b64_json".into()),
        ];
        let mut images: Vec<(String, Vec<u8>)> = images.to_vec();
        let mask = o.mask.as_ref().filter(|m| !m.is_empty());
        let headers = image_auth_headers(Some(settings), &self.cfg);
        let mut last = String::new();
        for sz in &sizes {
            match data.iter_mut().find(|(k, _)| k == "size") {
                Some(e) => e.1 = sz.clone(),
                None => data.push(("size".into(), sz.clone())),
            }
            let mut multi = images.len() > 1;
            let mut resp: Option<ImageResponse> = None;
            let mut next_size = false;
            for _attempt in 0..6 {
                let mut files = Vec::new();
                for (i, (name, blob)) in images.iter().enumerate() {
                    let fname = if name.is_empty() {
                        format!("image{}.png", i)
                    } else {
                        name.clone()
                    };
                    files.push((
                        if multi { "image[]" } else { "image" }.to_string(),
                        fname,
                        blob.clone(),
                        guess_mime(name).to_string(),
                    ));
                }
                if let Some(m) = mask {
                    files.push((
                        "mask".into(),
                        "mask.png".into(),
                        m.clone(),
                        "image/png".into(),
                    ));
                }
                let r = self
                    .transport
                    .send(self.req(
                        "POST",
                        format!("{}/images/edits", base),
                        headers.clone(),
                        Some(ImageBody::Multipart {
                            fields: data.clone(),
                            files,
                        }),
                        tmo,
                    ))
                    .await;
                let r = match r {
                    Ok(r) => r,
                    Err(e) if e.timeout => return ierr(
                        "图像编辑超时 (image edit timed out) — 稍后再试一次 try once more later",
                    ),
                    Err(e) => return ierr(e.message),
                };
                if r.status < 400 {
                    resp = Some(r);
                    break;
                }
                let txt = take_chars(&r.text(), 300).to_string();
                let low = txt.to_lowercase();
                if moderated(r.status, &txt) {
                    return ierr("内容被模型的安全策略拒绝 (the image model's safety policy declined this edit) — 换一个描述；不要重试同样的内容 rephrase; do not retry the same prompt");
                }
                if r.status == 400
                    && low.contains("response_format")
                    && data.iter().any(|(k, _)| k == "response_format")
                {
                    data.retain(|(k, _)| k != "response_format");
                    resp = Some(r);
                    continue;
                }
                if r.status == 400
                    && multi
                    && (low.contains("image[]") || (low.contains("image") && low.contains("array")))
                {
                    images.truncate(1); // server takes one image: keep the main one
                    multi = false;
                    resp = Some(r);
                    continue;
                }
                if r.status == 400 && bad_size(&low) {
                    last = txt;
                    next_size = true;
                    break;
                }
                if r.status == 404 || (r.status == 400 && low.contains("mask") && mask.is_some()) {
                    return ierr(format!(
                        "这个模型/服务不支持图片编辑 (this model or server does not support image edits): {}",
                        txt
                    ));
                }
                return ierr(format!(
                    "图像编辑失败 image edit failed HTTP {}: {}",
                    r.status, txt
                ));
            }
            if next_size {
                continue;
            }
            let body = resp.map(|r| r.json()).unwrap_or(Value::Null);
            let items: Vec<Value> = body
                .get("data")
                .and_then(|d| d.as_array())
                .cloned()
                .unwrap_or_default();
            let out = self.collect(&items, tmo).await?;
            if out.is_empty() {
                return ierr("图像编辑没有返回图片 (the server returned no image data)");
            }
            return Ok(ImageResult {
                images: out,
                model: mdl,
                size: sz.clone(),
                shape,
                latency_s: round1(now_ts() - t0),
                revised_prompt: items
                    .first()
                    .map(|i| get_str(i, "revised_prompt"))
                    .unwrap_or_default(),
                via: None,
            });
        }
        ierr(format!(
            "服务不接受任何尺寸 (the server rejected every size tried: {}): {}",
            sizes.join(", "),
            last
        ))
    }

    /// Variations of one image: {base}/images/variations when the server has it (DALL·E 2 style), otherwise an edit
    /// with a 'close variation' prompt (gpt-image-1 and most local servers).
    pub async fn variation(
        &self,
        settings: &Value,
        image: &(String, Vec<u8>),
        o: &EditOpts,
    ) -> Result<ImageResult, ImageError> {
        let (base, mut mdl) = self.resolve(settings).await?;
        if let Some(m) = o.model.as_ref().filter(|m| !m.is_empty()) {
            mdl = m.clone();
        }
        let n = clamp_n(o.n);
        let (shape, sizes) = shape_of(o.size.as_deref(), o.aspect.as_deref());
        let tmo = timeout_of(settings, o.timeout);
        let t0 = now_ts();
        let fname = if image.0.is_empty() {
            "image.png".to_string()
        } else {
            image.0.clone()
        };
        let r = self
            .transport
            .send(self.req(
                "POST",
                format!("{}/images/variations", base),
                image_auth_headers(Some(settings), &self.cfg),
                Some(ImageBody::Multipart {
                    fields: vec![
                        ("model".into(), mdl.clone()),
                        ("n".into(), n.to_string()),
                        ("size".into(), sizes[0].clone()),
                        ("response_format".into(), "b64_json".into()),
                    ],
                    files: vec![(
                        "image".into(),
                        fname,
                        image.1.clone(),
                        guess_mime(&image.0).to_string(),
                    )],
                }),
                tmo,
            ))
            .await;
        let r = match r {
            Ok(r) => Some(r),
            Err(e) if e.timeout => None,
            Err(e) => return ierr(e.message),
        };
        if let Some(r) = r.filter(|r| r.status < 400) {
            let items: Vec<Value> = r
                .json()
                .get("data")
                .and_then(|d| d.as_array())
                .cloned()
                .unwrap_or_default();
            let mut out = Vec::new();
            for it in &items {
                let b = get_str(it, "b64_json");
                if !b.is_empty() {
                    out.push(
                        base64::engine::general_purpose::STANDARD
                            .decode(b.trim())
                            .map_err(|e| ImageError(format!("Error: {}", e)))?,
                    );
                }
            }
            if !out.is_empty() {
                return Ok(ImageResult {
                    images: out,
                    model: mdl,
                    size: sizes[0].clone(),
                    shape,
                    latency_s: round1(now_ts() - t0),
                    revised_prompt: String::new(),
                    via: Some("variations".into()),
                });
            }
        }
        let extra = o.prompt.trim();
        let p = if extra.is_empty() {
            VARIATION_PROMPT.to_string()
        } else {
            format!("{}. Also: {}", VARIATION_PROMPT, extra)
        };
        let eo = EditOpts {
            mask: None,
            prompt: String::new(),
            ..o.clone()
        };
        let mut res = self
            .edit(settings, std::slice::from_ref(image), &p, &eo)
            .await?;
        res.via = Some("edit".into());
        Ok(res)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use parking_lot::Mutex;

    #[test]
    fn image_model_picking() {
        // an explicit image mode wins over names; vision-LANGUAGE / embedding / speech models are never picked
        let ms = vec![
            json!({"id": "Qwen3-VL-8B", "mode": "chat"}),
            json!({"id": "bge-embed"}),
            json!({"id": "whisper-large"}),
            json!({"id": "FLUX.1-schnell"}),
            json!({"id": "my-painter", "mode": "image"}),
        ];
        assert_eq!(pick_image_model(&ms), "my-painter");
        assert_eq!(
            pick_image_model(&[
                json!({"id": "Qwen2-VL"}),
                json!({"id": "stable-diffusion-xl"})
            ]),
            "stable-diffusion-xl"
        );
        assert_eq!(
            pick_image_model(&[
                json!({"id": "gpt-image-1", "readiness": "loading"}),
                json!({"id": "dall-e-3"})
            ]),
            "dall-e-3"
        );
        assert_eq!(
            pick_image_model(&[
                json!({"id": "Qwen3-27B", "mode": "chat"}),
                json!({"id": "bge-m3", "mode": "embedding"})
            ]),
            ""
        );
    }

    #[test]
    fn shape_normalisation() {
        assert_eq!(shape_of(None, None).0, "square");
        assert_eq!(shape_of(Some("square"), None).1, vec!["1024x1024"]);
        assert_eq!(shape_of(Some("16:9"), None).0, "landscape");
        assert_eq!(shape_of(Some("16:9"), None).1[0], "1536x1024");
        assert_eq!(shape_of(Some("9:16"), None).0, "portrait");
        assert!(shape_of(Some("9:16"), None)
            .1
            .contains(&"1024x1792".to_string()));
        assert_eq!(shape_of(None, Some("poster")).0, "portrait");
        assert_eq!(shape_of(None, Some("桌面壁纸")).0, "landscape");
        let (sh, sz) = shape_of(Some("1792x1024"), None);
        assert_eq!(sh, "landscape");
        assert_eq!(sz[0], "1792x1024");
        assert!(sz.contains(&"1536x1024".to_string()));
        assert_eq!(shape_of(Some("4:3"), None).0, "landscape");
        assert_eq!(shape_of(Some("3:4"), None).0, "portrait");
        assert_eq!(shape_of(Some("21:9"), None).0, "landscape");
        assert_eq!(shape_of(Some("1:1"), None).1, vec!["1024x1024"]);
        assert_eq!(shape_of(None, Some("手机壁纸")).0, "portrait");
    }

    #[test]
    fn style_presets_zh_and_en() {
        assert!(style_suffix(Some("水彩")).contains("watercolor"));
        assert!(style_suffix(Some("动漫")).contains("anime"));
        assert!(style_suffix(Some("水墨")).contains("ink wash"));
        assert!(style_suffix(Some("Photorealistic")).starts_with("photorealistic"));
        assert!(style_suffix(Some("扁平矢量")).contains("flat vector"));
        assert_eq!(style_suffix(Some("steampunk")), "steampunk style");
        let p = with_style("a red fox", Some("油画"));
        assert!(p.starts_with("a red fox, oil painting"));
        assert_eq!(with_style(&p, Some("油画")), p);
        assert_eq!(with_style("a cat", Some("")), "a cat");
    }

    #[test]
    fn auth_header_selection() {
        let cfg =
            Config::parse("[model]\napi_key='sys-key'\nimage_api_key='sys-image-key'").unwrap();
        let s = json!({"model_override": false, "model_api_key": "sys-key"});
        assert_eq!(
            image_auth_headers(Some(&s), &cfg),
            vec![("Authorization".into(), "Bearer sys-image-key".into())]
        );
        let own = json!({"model_override": true, "model_api_key": "my-key"});
        assert_eq!(
            image_auth_headers(Some(&own), &cfg),
            vec![("Authorization".into(), "Bearer my-key".into())]
        );
    }

    // ---------------------------------------------------------------- fake Images API
    #[derive(Default)]
    struct Fake {
        reject_sizes: Vec<&'static str>,
        reject_fields: Vec<&'static str>,
        use_url: bool,
        models: Option<Value>,
        moderate: bool,
        slow: bool,
        single_image_only: bool,
        has_variations: bool,
        no_edits: bool,
        /// generations: the JSON body; edits/variations: {"url", "data", "parts"}.
        calls: Mutex<Vec<Value>>,
    }

    const PNG: &[u8] = b"\x89PNG\r\n\x1a\nfakepng";

    fn r(status: u16, v: Value) -> Result<ImageResponse, TransportError> {
        Ok(ImageResponse {
            status,
            body: serde_json::to_vec(&v).unwrap(),
        })
    }

    impl ImageTransport for Fake {
        fn send(&self, req: ImageRequest) -> BoxFuture<'_, Result<ImageResponse, TransportError>> {
            let b64 = |b: &[u8]| base64::engine::general_purpose::STANDARD.encode(b);
            let out = (|| {
                if self.slow {
                    return Err(TransportError {
                        timeout: true,
                        message: "ReadTimeout: slow".into(),
                    });
                }
                if req.method == "GET" {
                    if req.url.ends_with("/models") {
                        return r(
                            200,
                            json!({"data": self.models.clone().unwrap_or(json!([{"id": "FLUX.1-dev"}]))}),
                        );
                    }
                    return Ok(ImageResponse {
                        status: 200,
                        body: PNG.to_vec(),
                    });
                }
                match req.body.clone() {
                    Some(ImageBody::Json(j)) => {
                        self.calls.lock().push(j.clone());
                        if self.moderate {
                            return r(
                                400,
                                json!({"error": {"code": "content_policy_violation",
                                "message": "Your request was rejected as a result of our safety system."}}),
                            );
                        }
                        for f in &self.reject_fields {
                            if j.get(*f).is_some() {
                                return r(
                                    400,
                                    json!({"error": {"message": format!("Unknown parameter: '{}'", f)}}),
                                );
                            }
                        }
                        let size = get_str(&j, "size");
                        if self.reject_sizes.contains(&size.as_str()) {
                            return r(
                                400,
                                json!({"error": {"message": format!("Invalid value: '{}'. Supported size values are 1024x1024", size)}}),
                            );
                        }
                        let mut item = if self.use_url {
                            json!({"url": "http://img/x.png"})
                        } else {
                            json!({"b64_json": b64(PNG)})
                        };
                        item["revised_prompt"] =
                            json!(format!("a detailed {}", get_str(&j, "prompt")));
                        let n = j["n"].as_i64().unwrap_or(1) as usize;
                        r(200, json!({"data": vec![item; n]}))
                    }
                    Some(ImageBody::Multipart { fields, files }) => {
                        let data: Map<String, Value> =
                            fields.iter().map(|(k, v)| (k.clone(), json!(v))).collect();
                        let parts: Vec<String> = files.iter().map(|f| f.0.clone()).collect();
                        let url = req.url.rsplit("/images/").next().unwrap().to_string();
                        self.calls
                            .lock()
                            .push(json!({"url": url, "data": data, "parts": parts}));
                        let n: usize = get_str(&Value::Object(data.clone()), "n")
                            .parse()
                            .unwrap_or(1);
                        if url == "variations" {
                            if !self.has_variations {
                                return r(404, json!({"error": {"message": "Not found"}}));
                            }
                            let mut v = PNG.to_vec();
                            v.push(b'v');
                            return r(200, json!({"data": vec![json!({"b64_json": b64(&v)}); n]}));
                        }
                        if self.no_edits {
                            return r(404, json!({"error": {"message": "Not found"}}));
                        }
                        if self.single_image_only && parts.iter().any(|p| p == "image[]") {
                            return r(
                                400,
                                json!({"error": {"message": "Invalid value for 'image': expected a file, got an array"}}),
                            );
                        }
                        if data.contains_key("response_format") {
                            return r(
                                400,
                                json!({"error": {"message": "Unknown parameter: 'response_format'"}}),
                            );
                        }
                        let prompt = get_str(&Value::Object(data), "prompt");
                        r(
                            200,
                            json!({"data": vec![json!({"b64_json": b64(b"\x89PNG\r\n\x1a\nedited"),
                                                          "revised_prompt": format!("edited: {}", prompt)}); n]}),
                        )
                    }
                    None => r(500, json!({})),
                }
            })();
            Box::pin(async move { out })
        }
    }

    fn client(f: Fake) -> (ImageClient, Arc<Fake>) {
        let f = Arc::new(f);
        (
            ImageClient::with_transport(Arc::new(Config::parse("").unwrap()), f.clone()),
            f,
        )
    }

    #[tokio::test]
    async fn generates_png_with_auto_model() {
        let (c, f) = client(Fake::default());
        let s =
            json!({"model_base_url": "http://router/v1", "image_base_url": "", "image_model": ""});
        let res = c
            .generate(
                &s,
                "a red fox in snow",
                &GenerateOpts {
                    aspect: Some("landscape".into()),
                    n: 2,
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        assert_eq!(res.model, "FLUX.1-dev");
        assert_eq!(res.size, "1536x1024");
        assert_eq!(res.images.len(), 2);
        assert_eq!(res.images[0], PNG);
        assert!(res.revised_prompt.starts_with("a detailed"));
        let calls = f.calls.lock();
        assert_eq!(calls[0]["model"], "FLUX.1-dev");
        assert_eq!(calls[0]["prompt"], "a red fox in snow");
        assert_eq!(calls[0]["n"], 2);
    }

    #[tokio::test]
    async fn chat_options_never_reach_image_requests() {
        // fast mode and reasoning effort belong to chat requests only: not on generations, edits or variations
        let (c, f) = client(Fake {
            has_variations: true,
            ..Default::default()
        });
        let s = json!({"model_base_url": "http://x/v1", "image_model": "gpt-image-1",
                       "fast_mode": true, "reasoning_effort": "high"});
        c.generate(&s, "a fox", &GenerateOpts::default())
            .await
            .unwrap();
        c.edit(
            &s,
            &[("a.png".into(), b"A".to_vec())],
            "add a hat",
            &EditOpts::default(),
        )
        .await
        .unwrap();
        c.variation(&s, &("a.png".into(), b"A".to_vec()), &EditOpts::default())
            .await
            .unwrap();
        let calls = f.calls.lock();
        assert!(calls.len() >= 3, "{:?}", calls);
        for call in calls.iter() {
            let txt = call.to_string();
            assert!(
                !txt.contains("service_tier") && !txt.contains("reasoning_effort"),
                "{}",
                txt
            );
        }
    }

    #[tokio::test]
    async fn walks_size_fallbacks_and_drops_unknown_fields() {
        let (c, f) = client(Fake {
            reject_sizes: vec!["1536x1024"],
            reject_fields: vec!["quality"],
            ..Default::default()
        });
        let s = json!({"model_base_url": "http://router/v1", "image_model": "dall-e-3"});
        let res = c
            .generate(
                &s,
                "a lighthouse",
                &GenerateOpts {
                    aspect: Some("landscape".into()),
                    quality: Some("high".into()),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        assert_eq!(res.size, "1792x1024");
        let calls = f.calls.lock();
        assert!(calls.last().unwrap().get("quality").is_none());
        assert_eq!(calls.last().unwrap()["model"], "dall-e-3");
    }

    #[tokio::test]
    async fn n_cap_url_response_and_errors() {
        let (c, f) = client(Fake::default());
        let s = json!({"model_base_url": "http://router/v1", "image_model": "FLUX.1-dev"});
        let res = c
            .generate(
                &s,
                "cat",
                &GenerateOpts {
                    n: 9,
                    quality: Some("high".into()),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        assert_eq!(res.images.len(), 4);
        assert_eq!(f.calls.lock().last().unwrap()["n"], 4);
        assert_eq!(f.calls.lock().last().unwrap()["quality"], "high");

        let (c, _) = client(Fake {
            use_url: true,
            ..Default::default()
        });
        let s = json!({"model_base_url": "http://router/v1", "image_base_url": "https://api.example/v1", "image_model": "gpt-image-1"});
        let res = c
            .generate(&s, "studio portrait", &GenerateOpts::default())
            .await
            .unwrap();
        assert_eq!(res.images, vec![PNG.to_vec()]);
        assert_eq!(res.size, "1024x1024");

        let (c, f) = client(Fake {
            models: Some(json!([{"id": "Qwen3-27B", "mode": "chat"}])),
            ..Default::default()
        });
        let e = c
            .generate(
                &json!({"model_base_url": "http://router/v1", "image_model": ""}),
                "anything",
                &GenerateOpts::default(),
            )
            .await
            .unwrap_err();
        assert!(e.0.contains("Image model") || e.0.contains("图像生成模型"));
        assert!(f.calls.lock().is_empty());

        let (c, _) = client(Fake {
            moderate: true,
            ..Default::default()
        });
        let e = c
            .generate(
                &json!({"model_base_url": "http://x/v1", "image_model": "dall-e-3"}),
                "something",
                &GenerateOpts::default(),
            )
            .await
            .unwrap_err();
        assert!(e.0.contains("safety") && e.0.contains("do not retry"));

        let (c, _) = client(Fake {
            slow: true,
            ..Default::default()
        });
        let e = c
            .generate(
                &json!({"model_base_url": "http://x/v1", "image_model": "dall-e-3"}),
                "something",
                &GenerateOpts::default(),
            )
            .await
            .unwrap_err();
        assert!(e.0.contains("timed out"));
        let e = c
            .generate(
                &json!({"model_base_url": "", "image_model": ""}),
                "x",
                &GenerateOpts::default(),
            )
            .await
            .unwrap_err();
        assert_eq!(
            e.0,
            "没有配置模型地址 (no model endpoint configured: Settings → Model endpoint)"
        );
    }

    #[tokio::test]
    async fn negative_prompt_background_output_format() {
        let (c, f) = client(Fake {
            reject_fields: vec!["negative_prompt"],
            ..Default::default()
        });
        let s = json!({"model_base_url": "http://x/v1", "image_model": "gpt-image-1"});
        c.generate(
            &s,
            "a dog",
            &GenerateOpts {
                negative_prompt: Some("text, watermark".into()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
        let calls = f.calls.lock().clone();
        assert!(calls.last().unwrap().get("negative_prompt").is_none());
        assert_eq!(calls[0]["negative_prompt"], "text, watermark");
        let (c, f) = client(Fake {
            reject_fields: vec!["background", "output_format"],
            ..Default::default()
        });
        let res = c
            .generate(
                &json!({"model_base_url": "http://x/v1", "image_model": "sdxl"}),
                "fox",
                &GenerateOpts {
                    background: Some("transparent".into()),
                    output_format: Some("webp".into()),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        assert!(!res.images.is_empty());
        let last = f.calls.lock().last().unwrap().clone();
        assert!(last.get("background").is_none() && last.get("output_format").is_none());
    }

    #[tokio::test]
    async fn edit_inpainting_references_and_variations() {
        let (c, f) = client(Fake {
            has_variations: true,
            ..Default::default()
        });
        let s = json!({"model_base_url": "http://x/v1", "image_model": "gpt-image-1"});
        let imgs = vec![
            ("photo.png".to_string(), b"MAIN".to_vec()),
            ("style.jpg".to_string(), b"REF".to_vec()),
        ];
        let res = c
            .edit(
                &s,
                &imgs,
                "make the sky purple",
                &EditOpts {
                    mask: Some(b"MASK".to_vec()),
                    aspect: Some("landscape".into()),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        assert_eq!(res.images, vec![b"\x89PNG\r\n\x1a\nedited".to_vec()]);
        assert!(res.revised_prompt.starts_with("edited:"));
        let last = f.calls.lock().last().unwrap().clone();
        assert_eq!(last["url"], "edits");
        assert_eq!(last["parts"], json!(["image[]", "image[]", "mask"]));
        assert_eq!(last["data"]["prompt"], "make the sky purple");
        assert!(last["data"].get("response_format").is_none());
        assert_eq!(last["data"]["size"], "1536x1024");

        let (c, f) = client(Fake {
            single_image_only: true,
            ..Default::default()
        });
        let res = c
            .edit(
                &json!({"model_base_url": "http://x/v1", "image_model": "sd-inpaint"}),
                &[
                    ("a.png".into(), b"A".to_vec()),
                    ("b.png".into(), b"B".to_vec()),
                ],
                "add a hat",
                &EditOpts::default(),
            )
            .await
            .unwrap();
        assert_eq!(res.images.len(), 1);
        assert_eq!(f.calls.lock().last().unwrap()["parts"], json!(["image"]));

        let (c, f) = client(Fake {
            has_variations: true,
            ..Default::default()
        });
        let res = c
            .variation(
                &json!({"model_base_url": "http://x/v1", "image_model": "dall-e-2"}),
                &("a.png".into(), b"A".to_vec()),
                &EditOpts {
                    n: 3,
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        assert_eq!(res.via.as_deref(), Some("variations"));
        assert_eq!(res.images.len(), 3);
        assert_eq!(f.calls.lock().last().unwrap()["url"], "variations");

        let (c, f) = client(Fake {
            has_variations: false,
            ..Default::default()
        });
        let res = c
            .variation(
                &json!({"model_base_url": "http://x/v1", "image_model": "gpt-image-1"}),
                &("a.png".into(), b"A".to_vec()),
                &EditOpts {
                    n: 2,
                    prompt: "warmer colours".into(),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        assert_eq!(res.via.as_deref(), Some("edit"));
        let last = f.calls.lock().last().unwrap().clone();
        assert_eq!(last["url"], "edits");
        let p = last["data"]["prompt"].as_str().unwrap();
        assert!(p.starts_with("Create a close variation") && p.contains("warmer colours"));
        assert_eq!(res.images.len(), 2);

        let (c, _) = client(Fake {
            no_edits: true,
            ..Default::default()
        });
        let e = c
            .edit(
                &json!({"model_base_url": "http://x/v1", "image_model": "flux-txt2img-only"}),
                &[("a.png".into(), b"A".to_vec())],
                "add a hat",
                &EditOpts::default(),
            )
            .await
            .unwrap_err();
        assert!(e.0.contains("does not support image edits"));
    }
}
