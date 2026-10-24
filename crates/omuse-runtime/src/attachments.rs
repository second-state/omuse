//! Files the user attaches in the chat (and files the agent reads): store, classify, extract text, pick images.
//!
//! Uploads live in the workspace under uploads/<YYYY-MM>/. Nothing here executes a file: documents are parsed as
//! data, videos are only decoded by ffmpeg into still frames / audio, and every extracted text is handed to the model
//! as untrusted content.
//!
//! External programs (both in the Docker image): `ffmpeg` (video frames, audio track, duration) and `heif-convert`
//! (iPhone `.heic` photos → JPEG). All functions are blocking: call them from `tokio::task::spawn_blocking`, like
//! the Python called them through `asyncio.to_thread`.

use std::io::{Cursor, Read};
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::{LazyLock, OnceLock};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use base64::Engine;
use image::{DynamicImage, GenericImageView, ImageDecoder, Rgb, RgbImage};
use omuse_common::util::take_chars;
use regex::Regex;
use serde_json::{json, Value};

pub const UPLOAD_MAX: usize = 50 * 1024 * 1024;
pub const UPLOAD_MAX_FILES: usize = 10;
pub const IMAGE_EXT: &[&str] = &[
    ".png", ".jpg", ".jpeg", ".gif", ".webp", ".bmp", ".heic", ".heif",
];
pub const VIDEO_EXT: &[&str] = &[".mp4", ".mov", ".m4v", ".webm", ".mkv", ".avi"];
pub const AUDIO_EXT: &[&str] = &[".mp3", ".m4a", ".wav", ".ogg", ".aac", ".flac", ".opus"];
pub const TEXT_EXT: &[&str] = &[
    ".txt",
    ".md",
    ".markdown",
    ".csv",
    ".tsv",
    ".json",
    ".log",
    ".yaml",
    ".yml",
    ".xml",
    ".ini",
    ".py",
    ".js",
    ".ts",
    ".html",
    ".htm",
    ".sql",
    ".sh",
];
pub const DOC_EXT: &[&str] = &[".pdf", ".docx", ".xlsx", ".xlsm", ".pptx"];
const OTHER_ALLOWED: &[&str] = &[".doc", ".xls", ".ppt", ".zip", ".rtf"];

/// `ALLOWED_EXT`: every extension an upload may have.
pub fn allowed_ext(ext: &str) -> bool {
    [
        IMAGE_EXT,
        VIDEO_EXT,
        AUDIO_EXT,
        TEXT_EXT,
        DOC_EXT,
        OTHER_ALLOWED,
    ]
    .iter()
    .any(|set| set.contains(&ext))
}

/// `AttachmentError` (a `ValueError`): the message is shown to the user / the model. Wrapped in `anyhow::Error`.
#[derive(Debug, Clone, thiserror::Error)]
#[error("{0}")]
pub struct AttachmentError(pub String);

fn aerr<T>(msg: impl Into<String>) -> Result<T> {
    Err(AttachmentError(msg.into()).into())
}

/// Python `os.path.splitext`: `(root, ext)`, leading dots of the base name are not an extension.
pub fn splitext(path: &str) -> (&str, &str) {
    let base_start = path.rfind('/').map(|i| i + 1).unwrap_or(0);
    let base = &path[base_start..];
    let lead = base.len() - base.trim_start_matches('.').len();
    match base[lead..].rfind('.') {
        Some(i) => {
            let at = base_start + lead + i;
            (&path[..at], &path[at..])
        }
        None => (path, ""),
    }
}

fn ext_lower(path: &str) -> String {
    splitext(path).1.to_lowercase()
}

fn basename(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

/// image / video / audio / pdf / docx / xlsx / pptx / text / other.
pub fn kind_of(path: &str) -> &'static str {
    let ext = ext_lower(path);
    let e = ext.as_str();
    if IMAGE_EXT.contains(&e) {
        "image"
    } else if VIDEO_EXT.contains(&e) {
        "video"
    } else if AUDIO_EXT.contains(&e) {
        "audio"
    } else if e == ".pdf" {
        "pdf"
    } else if e == ".docx" {
        "docx"
    } else if e == ".xlsx" || e == ".xlsm" {
        "xlsx"
    } else if e == ".pptx" {
        "pptx"
    } else if TEXT_EXT.contains(&e) {
        "text"
    } else {
        "other"
    }
}

/// `mimetypes.guess_type(path)[0] or "application/octet-stream"`, with the module's extra registrations and
/// Python's built-in table for the extensions OMuse accepts.
pub fn mime_of(path: &str) -> String {
    let ext = ext_lower(path);
    let known = match ext.as_str() {
        ".md" => "text/markdown",
        ".heic" => "image/heic",
        ".webp" => "image/webp",
        ".m4a" => "audio/mp4",
        ".opus" => "audio/ogg",
        ".mkv" => "video/x-matroska",
        ".docx" => "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
        ".xlsx" => "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
        ".pptx" => "application/vnd.openxmlformats-officedocument.presentationml.presentation",
        ".png" => "image/png",
        ".jpg" | ".jpeg" => "image/jpeg",
        ".gif" => "image/gif",
        ".bmp" => "image/bmp",
        ".heif" => "image/heif",
        ".mp4" => "video/mp4",
        ".mov" => "video/quicktime",
        ".m4v" => "video/mp4",
        ".webm" => "video/webm",
        ".avi" => "video/x-msvideo",
        ".mp3" => "audio/mpeg",
        ".wav" => "audio/x-wav",
        ".ogg" => "audio/ogg",
        ".aac" => "audio/aac",
        ".flac" => "audio/flac",
        ".txt" | ".log" | ".ini" => "text/plain",
        ".csv" => "text/csv",
        ".tsv" => "text/tab-separated-values",
        ".json" => "application/json",
        ".yaml" | ".yml" => "application/yaml",
        ".xml" => "text/xml",
        ".py" => "text/x-python",
        ".js" => "text/javascript",
        ".html" | ".htm" => "text/html",
        ".sql" => "application/sql",
        ".sh" => "application/x-sh",
        ".pdf" => "application/pdf",
        ".doc" => "application/msword",
        ".xls" => "application/vnd.ms-excel",
        ".ppt" => "application/vnd.ms-powerpoint",
        ".zip" => "application/zip",
        ".rtf" => "application/rtf",
        _ => "",
    };
    if !known.is_empty() {
        return known.to_string();
    }
    mime_guess::from_path(path)
        .first_raw()
        .unwrap_or("application/octet-stream")
        .to_string()
}

/// A file name that is safe to store: no directories, no control or shell-special characters, ≤ 80 + 10 chars.
pub fn safe_name(name: &str) -> String {
    static BAD: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"[\x00-\x1f/<>:"|?*]+"#).unwrap());
    static WS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\s+").unwrap());
    let n = name.replace('\\', "/");
    let n = basename(&n).trim().to_string();
    let n = BAD.replace_all(&n, "_").to_string();
    let n = WS.replace_all(&n, " ").to_string();
    let mut n = n.trim_matches([' ', '.']).to_string();
    if n.is_empty() {
        n = "file".into();
    }
    let (base, ext) = splitext(&n);
    format!(
        "{}{}",
        take_chars(base, 80),
        take_chars(ext, 10).to_lowercase()
    )
}

/// Write an uploaded file under uploads/<YYYY-MM>/ with a unique name. Returns its info.
pub fn save_upload(workspace: &Path, name: &str, data: &[u8]) -> Result<Value> {
    if data.is_empty() {
        return aerr("文件是空的 (empty file)");
    }
    if data.len() > UPLOAD_MAX {
        return aerr(format!(
            "文件超过 {} MB (file too large)",
            UPLOAD_MAX / 1024 / 1024
        ));
    }
    let name = safe_name(name);
    let ext = ext_lower(&name);
    if !allowed_ext(&ext) {
        return aerr(format!(
            "不支持这种文件类型 {} (unsupported file type)",
            if ext.is_empty() {
                "(none)"
            } else {
                ext.as_str()
            }
        ));
    }
    let folder = workspace
        .join("uploads")
        .join(chrono::Local::now().format("%Y-%m").to_string());
    std::fs::create_dir_all(&folder)?;
    let (base, ext) = splitext(&name);
    let mut path = folder.join(&name);
    let mut n = 1;
    while path.exists() {
        n += 1;
        path = folder.join(format!("{} ({}){}", base, n, ext));
    }
    std::fs::write(&path, data)?;
    info(workspace, &path)
}

/// `{"path": relative to the workspace, "name", "size", "mime", "kind"}`.
pub fn info(workspace: &Path, path: &Path) -> Result<Value> {
    let rel = path
        .strip_prefix(workspace)
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|_| path.to_string_lossy().into_owned());
    let p = path.to_string_lossy();
    Ok(
        json!({"path": rel, "name": basename(&p), "size": std::fs::metadata(path)?.len(),
              "mime": mime_of(&p), "kind": kind_of(&p)}),
    )
}

// ------------------------------------------------------------------ text extraction
const W: &str = "http://schemas.openxmlformats.org/wordprocessingml/2006/main";
const A: &str = "http://schemas.openxmlformats.org/drawingml/2006/main";

fn is(n: &roxmltree::Node, ns: &str, name: &str) -> bool {
    n.is_element() && n.tag_name().name() == name && n.tag_name().namespace() == Some(ns)
}

/// `"".join(t.text or "" for t in el.iter(text_tag))`.
fn joined_text(el: roxmltree::Node, ns: &str, text_tag: &str) -> String {
    el.descendants()
        .filter(|t| is(t, ns, text_tag))
        .map(|t| t.text().unwrap_or(""))
        .collect()
}

fn xml_text(data: &str, ns: &str, para_tag: &str, text_tag: &str) -> Vec<String> {
    let Ok(doc) = roxmltree::Document::parse(data) else {
        return vec![];
    };
    doc.root()
        .descendants()
        .filter(|p| is(p, ns, para_tag))
        .map(|p| joined_text(p, ns, text_tag))
        .filter(|s| !s.trim().is_empty())
        .collect()
}

fn zip_read(z: &mut zip::ZipArchive<std::fs::File>, name: &str) -> Result<String> {
    let mut f = z.by_name(name)?;
    let mut s = String::new();
    f.read_to_string(&mut s)?;
    Ok(s)
}

/// The text of a .docx: paragraphs (headings as `#`), tables as `| a | b |` rows, then foot- and endnotes.
pub fn docx_text(path: &Path) -> Result<String> {
    static HEAD: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"^(?i:heading\s*(\d)|标题\s*(\d))").unwrap());
    let mut z = zip::ZipArchive::new(std::fs::File::open(path)?)
        .context("BadZipFile: File is not a zip file")?;
    let names: Vec<String> = z.file_names().map(str::to_string).collect();
    if !names.iter().any(|n| n == "word/document.xml") {
        return aerr("不是有效的 Word 文件 (not a .docx)");
    }
    let xml = zip_read(&mut z, "word/document.xml")?;
    let doc = roxmltree::Document::parse(&xml).map_err(|e| anyhow::anyhow!("ParseError: {}", e))?;
    let mut lines: Vec<String> = Vec::new();
    let body = doc.root_element().children().find(|c| is(c, W, "body"));
    for el in body
        .into_iter()
        .flat_map(|b| b.children())
        .filter(|c| c.is_element())
    {
        if is(&el, W, "p") {
            let s = joined_text(el, W, "t");
            let lvl = el
                .children()
                .find(|c| is(c, W, "pPr"))
                .and_then(|ppr| ppr.children().find(|c| is(c, W, "pStyle")))
                .and_then(|st| st.attribute((W, "val")))
                .unwrap_or("");
            if !s.trim().is_empty() {
                let prefix = HEAD
                    .captures(lvl)
                    .and_then(|m| m.get(1).or(m.get(2)))
                    .and_then(|d| d.as_str().parse::<usize>().ok())
                    .map(|n| format!("{} ", "#".repeat(n)))
                    .unwrap_or_default();
                lines.push(prefix + &s);
            }
        } else if is(&el, W, "tbl") {
            for tr in el.descendants().filter(|n| is(n, W, "tr")) {
                let cells: Vec<String> = tr
                    .descendants()
                    .filter(|n| is(n, W, "tc"))
                    .map(|tc| joined_text(tc, W, "t").trim().to_string())
                    .collect();
                lines.push(format!("| {} |", cells.join(" | ")));
            }
        }
    }
    static NOTES: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"^word/(footnotes|endnotes)\.xml$").unwrap());
    let mut extras: Vec<&String> = names.iter().filter(|n| NOTES.is_match(n)).collect();
    extras.sort();
    for extra in extras {
        let x = zip_read(&mut z, extra)?;
        lines.extend(xml_text(&x, W, "p", "t"));
    }
    Ok(lines.join("\n"))
}

/// The text of a .pptx, one `## Slide n` section per slide.
pub fn pptx_text(path: &Path) -> Result<String> {
    static SLIDE: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"^ppt/slides/slide\d+\.xml$").unwrap());
    static NUM: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\d+").unwrap());
    let mut z = zip::ZipArchive::new(std::fs::File::open(path)?)
        .context("BadZipFile: File is not a zip file")?;
    let mut slides: Vec<String> = z
        .file_names()
        .filter(|n| SLIDE.is_match(n))
        .map(str::to_string)
        .collect();
    slides.sort_by_key(|n| {
        NUM.find_iter(n)
            .last()
            .and_then(|m| m.as_str().parse::<u64>().ok())
            .unwrap_or(0)
    });
    let mut out = Vec::new();
    for (i, n) in slides.iter().enumerate() {
        out.push(format!("## Slide {}", i + 1));
        let x = zip_read(&mut z, n)?;
        out.extend(xml_text(&x, A, "p", "t"));
    }
    Ok(out.join("\n"))
}

/// The text of a PDF's first `max_pages` pages (`--- page n ---` sections) and its page count.
pub fn pdf_text(path: &Path, max_pages: usize) -> Result<(String, usize)> {
    let total = lopdf::Document::load(path)
        .map(|d| d.get_pages().len())
        .map_err(|e| anyhow::anyhow!("PdfReadError: {}", e))?;
    let p = path.to_path_buf();
    // pdf-extract can panic on unusual fonts: a page without text is better than a crashed worker
    let pages = std::panic::catch_unwind(move || pdf_extract::extract_text_by_pages(&p))
        .ok()
        .and_then(|r| r.ok())
        .unwrap_or_default();
    let parts: Vec<String> = pages
        .iter()
        .take(max_pages)
        .enumerate()
        .filter_map(|(i, t)| {
            let t = t.trim();
            (!t.is_empty()).then(|| format!("--- page {} ---\n{}", i + 1, t))
        })
        .collect();
    Ok((parts.join("\n"), total))
}

/// Readable text of a document, or '' when it has none (images, scanned PDFs, media).
pub fn text_of(path: &Path, limit: usize) -> Result<String> {
    let p = path.to_string_lossy();
    match kind_of(&p) {
        "pdf" => Ok(take_chars(&pdf_text(path, 80)?.0, limit).to_string()),
        "docx" => Ok(take_chars(&docx_text(path)?, limit).to_string()),
        "pptx" => Ok(take_chars(&pptx_text(path)?, limit).to_string()),
        "xlsx" => omuse_docs::xlsx::read_text(&p, limit).map_err(|e| anyhow::anyhow!("{}", e.0)),
        "text" => {
            let mut raw = Vec::new();
            std::fs::File::open(path)?
                .take((limit * 4) as u64)
                .read_to_end(&mut raw)?;
            if raw[..raw.len().min(2000)].contains(&0) {
                return Ok(String::new());
            }
            Ok(take_chars(&String::from_utf8_lossy(&raw), limit).to_string())
        }
        _ => Ok(String::new()),
    }
}

// ------------------------------------------------------------------ external programs

/// Run a program, capturing stdout/stderr, killing it after `timeout` (`subprocess.run(..., timeout=)`).
fn run_capture(cmd: &mut Command, timeout: Duration) -> std::io::Result<Output> {
    let mut child = cmd
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let mut so = child.stdout.take().unwrap();
    let mut se = child.stderr.take().unwrap();
    let t_out = std::thread::spawn(move || {
        let mut b = Vec::new();
        let _ = so.read_to_end(&mut b);
        b
    });
    let t_err = std::thread::spawn(move || {
        let mut b = Vec::new();
        let _ = se.read_to_end(&mut b);
        b
    });
    let start = Instant::now();
    let status = loop {
        if let Some(st) = child.try_wait()? {
            break st;
        }
        if start.elapsed() > timeout {
            let _ = child.kill();
            let _ = child.wait();
            return Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                format!(
                    "TimeoutExpired: timed out after {} seconds",
                    timeout.as_secs()
                ),
            ));
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    Ok(Output {
        status,
        stdout: t_out.join().unwrap_or_default(),
        stderr: t_err.join().unwrap_or_default(),
    })
}

/// The ffmpeg program when one is installed (`imageio_ffmpeg` / `shutil.which("ffmpeg")`).
pub fn ffmpeg_exe() -> Option<PathBuf> {
    static EXE: OnceLock<Option<PathBuf>> = OnceLock::new();
    EXE.get_or_init(|| {
        Command::new("ffmpeg")
            .arg("-version")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .ok()
            .filter(|s| s.success())
            .map(|_| PathBuf::from("ffmpeg"))
    })
    .clone()
}

/// The duration of a media file in seconds (0 when unknown).
pub fn media_duration(path: &Path) -> f64 {
    static DUR: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"Duration:\s*(\d+):(\d+):(\d+(?:\.\d+)?)").unwrap());
    let Some(exe) = ffmpeg_exe() else {
        return 0.0;
    };
    let Ok(p) = run_capture(
        Command::new(exe).arg("-hide_banner").arg("-i").arg(path),
        Duration::from_secs(60),
    ) else {
        return 0.0;
    };
    let err = String::from_utf8_lossy(&p.stderr);
    DUR.captures(&err)
        .map(|m| {
            m[1].parse::<f64>().unwrap_or(0.0) * 3600.0
                + m[2].parse::<f64>().unwrap_or(0.0) * 60.0
                + m[3].parse::<f64>().unwrap_or(0.0)
        })
        .unwrap_or(0.0)
}

/// `n` still frames spread evenly over the video, as JPEG bytes with their timestamp (seconds).
pub fn video_frames(path: &Path, n: usize, max_side: u32) -> Result<Vec<(f64, Vec<u8>)>> {
    let Some(exe) = ffmpeg_exe() else {
        return aerr("没有可用的 ffmpeg，无法读取视频 (ffmpeg not available)");
    };
    let dur = media_duration(path);
    let times: Vec<f64> = if dur > 0.0 {
        (0..n).map(|i| dur * (i as f64 + 0.5) / n as f64).collect()
    } else {
        vec![0.0]
    };
    let mut out = Vec::new();
    for ts in times {
        let p = run_capture(
            Command::new(&exe)
                .args([
                    "-hide_banner",
                    "-loglevel",
                    "error",
                    "-ss",
                    &format!("{:.2}", ts),
                    "-i",
                ])
                .arg(path)
                .args([
                    "-frames:v",
                    "1",
                    "-vf",
                    &format!("scale='min({},iw)':-2", max_side),
                ])
                .args(["-f", "image2", "-c:v", "mjpeg", "pipe:1"]),
            Duration::from_secs(60),
        )?;
        if p.status.success() && !p.stdout.is_empty() {
            out.push((ts, p.stdout));
        }
    }
    if out.is_empty() {
        return aerr("无法从视频中取出画面 (could not decode the video)");
    }
    Ok(out)
}

/// The soundtrack as 16 kHz mono WAV (for speech-to-text), at most `max_seconds`.
pub fn audio_wav(path: &Path, max_seconds: u32) -> Result<Vec<u8>> {
    let Some(exe) = ffmpeg_exe() else {
        return aerr("没有可用的 ffmpeg (ffmpeg not available)");
    };
    let p = run_capture(
        Command::new(exe)
            .args(["-hide_banner", "-loglevel", "error", "-i"])
            .arg(path)
            .args([
                "-t",
                &max_seconds.to_string(),
                "-vn",
                "-ac",
                "1",
                "-ar",
                "16000",
                "-f",
                "wav",
                "pipe:1",
            ]),
        Duration::from_secs(180),
    )?;
    if !p.status.success() || p.stdout.len() < 1000 {
        return aerr("这个文件没有可用的声音 (no audio track)");
    }
    Ok(p.stdout)
}

// ------------------------------------------------------------------ images, PDF scans

/// An ISO-BMFF HEIF/HEIC/AVIF-style container (`....ftypheic` …).
fn is_heif(data: &[u8]) -> bool {
    data.len() > 12
        && &data[4..8] == b"ftyp"
        && matches!(
            &data[8..12],
            b"heic" | b"heix" | b"hevc" | b"hevx" | b"heim" | b"heis" | b"mif1" | b"msf1"
        )
}

/// iPhone photos: `heif-convert in.heic out.jpg`.
fn heif_to_jpeg(data: &[u8]) -> Result<Vec<u8>> {
    let dir = tempfile::tempdir()?;
    let src = dir.path().join("in.heic");
    let dst = dir.path().join("out.jpg");
    std::fs::write(&src, data)?;
    let p = run_capture(
        Command::new("heif-convert").arg(&src).arg(&dst),
        Duration::from_secs(60),
    )
    .map_err(|e| anyhow::anyhow!("heif-convert: {}", e))?;
    if !p.status.success() || !dst.exists() {
        anyhow::bail!(
            "heif-convert failed: {}",
            String::from_utf8_lossy(&p.stderr).trim()
        );
    }
    Ok(std::fs::read(dst)?)
}

fn decode_oriented(data: &[u8]) -> Result<DynamicImage> {
    let reader = image::ImageReader::new(Cursor::new(data)).with_guessed_format()?;
    let mut decoder = reader.into_decoder()?;
    let orientation = decoder.orientation().ok();
    let mut im = DynamicImage::from_decoder(decoder)?; // the first frame of an animation
    if let Some(o) = orientation {
        im.apply_orientation(o);
    }
    Ok(im)
}

/// PIL `Image.thumbnail`: shrink to fit in `max`×`max`, keeping the aspect ratio; never enlarge.
fn thumbnail(im: DynamicImage, max: u32) -> DynamicImage {
    let (w, h) = im.dimensions();
    if w <= max && h <= max {
        return im;
    }
    im.resize(max, max, image::imageops::FilterType::Triangle)
}

fn encode_jpeg(im: &RgbImage, quality: u8) -> Result<Vec<u8>> {
    let mut buf = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut buf, quality).encode_image(im)?;
    Ok(buf)
}

/// Resize/convert an image for the vision model (JPEG, longest side ≤ `max_side`).
pub fn to_jpeg(data: &[u8], max_side: u32) -> Result<(Vec<u8>, String)> {
    let go = || -> Result<Vec<u8>> {
        let owned;
        let data = if is_heif(data) {
            owned = heif_to_jpeg(data)?;
            owned.as_slice()
        } else {
            data
        };
        let im = thumbnail(decode_oriented(data)?, max_side);
        encode_jpeg(&im.to_rgb8(), 85)
    };
    match go() {
        Ok(b) => Ok((b, "image/jpeg".into())),
        Err(e) => aerr(format!(
            "无法读取这张图片 (cannot read image): {}",
            take_chars(&e.to_string(), 120)
        )),
    }
}

/// An image file as base64 JPEG for the vision model: `(b64, mime)`.
pub fn image_b64(path: &Path, max_side: u32) -> Result<(String, String)> {
    let raw = std::fs::read(path)?;
    let (data, mime) = to_jpeg(&raw, max_side)?;
    Ok((base64::engine::general_purpose::STANDARD.encode(data), mime))
}

/// One embedded PDF image as an image file: JPEG streams as they are, 8-bit Flate RGB/Gray pixels as PNG.
fn pdf_image_file(img: &lopdf::xobject::PdfImage) -> Option<Vec<u8>> {
    let filters = img.filters.clone().unwrap_or_default();
    if filters.iter().any(|f| f == "DCTDecode") {
        return Some(img.content.to_vec());
    }
    if filters.iter().any(|f| f == "JPXDecode") {
        return None;
    }
    let raw = if filters.iter().any(|f| f == "FlateDecode") {
        let mut out = Vec::new();
        flate2::read::ZlibDecoder::new(img.content)
            .read_to_end(&mut out)
            .ok()?;
        out
    } else if filters.is_empty() {
        img.content.to_vec()
    } else {
        return None;
    };
    if img.bits_per_component.unwrap_or(8) != 8 {
        return None;
    }
    let (w, h) = (
        u32::try_from(img.width).ok()?,
        u32::try_from(img.height).ok()?,
    );
    let dyn_img = match img.color_space.as_deref() {
        Some("DeviceGray") => DynamicImage::ImageLuma8(image::GrayImage::from_raw(w, h, raw)?),
        _ => DynamicImage::ImageRgb8(RgbImage::from_raw(w, h, raw)?),
    };
    let mut buf = Vec::new();
    dyn_img
        .write_to(&mut Cursor::new(&mut buf), image::ImageFormat::Png)
        .ok()?;
    Some(buf)
}

/// Scanned PDFs: the biggest page image embedded in each of the first pages (no renderer needed).
pub fn pdf_page_images(path: &Path, max_pages: usize) -> Result<Vec<Vec<u8>>> {
    let doc = lopdf::Document::load(path).map_err(|e| anyhow::anyhow!("PdfReadError: {}", e))?;
    let mut out = Vec::new();
    for (_, page_id) in doc.get_pages().into_iter().take(max_pages) {
        let imgs = doc.get_page_images(page_id).unwrap_or_default();
        if let Some(biggest) = imgs.iter().max_by_key(|im| im.content.len()) {
            if let Some(file) = pdf_image_file(biggest) {
                out.push(file);
            }
        }
    }
    Ok(out)
}

// A 5×7 bitmap font for the contact-sheet labels ('#', digits, ':', ' ').
fn glyph(c: char) -> [u8; 7] {
    match c {
        '0' => [0x0E, 0x11, 0x13, 0x15, 0x19, 0x11, 0x0E],
        '1' => [0x04, 0x0C, 0x04, 0x04, 0x04, 0x04, 0x0E],
        '2' => [0x0E, 0x11, 0x01, 0x02, 0x04, 0x08, 0x1F],
        '3' => [0x1F, 0x02, 0x04, 0x02, 0x01, 0x11, 0x0E],
        '4' => [0x02, 0x06, 0x0A, 0x12, 0x1F, 0x02, 0x02],
        '5' => [0x1F, 0x10, 0x1E, 0x01, 0x01, 0x11, 0x0E],
        '6' => [0x06, 0x08, 0x10, 0x1E, 0x11, 0x11, 0x0E],
        '7' => [0x1F, 0x01, 0x02, 0x04, 0x08, 0x08, 0x08],
        '8' => [0x0E, 0x11, 0x11, 0x0E, 0x11, 0x11, 0x0E],
        '9' => [0x0E, 0x11, 0x11, 0x0F, 0x01, 0x02, 0x0C],
        ':' => [0x00, 0x0C, 0x0C, 0x00, 0x0C, 0x0C, 0x00],
        '#' => [0x0A, 0x0A, 0x1F, 0x0A, 0x1F, 0x0A, 0x0A],
        _ => [0; 7],
    }
}

fn draw_text(img: &mut RgbImage, x: u32, y: u32, text: &str) {
    for (i, c) in text.chars().enumerate() {
        let g = glyph(c);
        for (row, bits) in g.iter().enumerate() {
            for col in 0..5u32 {
                if bits & (0x10 >> col) != 0 {
                    let (px, py) = (x + i as u32 * 8 + col, y + row as u32 * 2);
                    for dy in 0..2 {
                        if px < img.width() && py + dy < img.height() {
                            img.put_pixel(px, py + dy, Rgb([255, 255, 255]));
                        }
                    }
                }
            }
        }
    }
}

/// Combine frames into one labelled grid image (one vision call instead of many).
pub fn contact_sheet(frames: &[(f64, Vec<u8>)], cols: u32, cell: u32) -> Result<(Vec<u8>, String)> {
    let mut ims = Vec::new();
    for (ts, data) in frames {
        let im = thumbnail(image::load_from_memory(data)?, cell).to_rgb8();
        ims.push((*ts, im));
    }
    if ims.is_empty() {
        return aerr("无法从视频中取出画面 (could not decode the video)");
    }
    let rows = (ims.len() as u32).div_ceil(cols);
    let h = ims.iter().map(|(_, im)| im.height()).max().unwrap_or(0);
    let mut sheet = RgbImage::from_pixel(cols * cell, rows * (h + 4), Rgb([255, 255, 255]));
    for (i, (ts, im)) in ims.iter().enumerate() {
        let i = i as u32;
        let (x, y) = ((i % cols) * cell, (i / cols) * (h + 4));
        image::imageops::replace(&mut sheet, im, i64::from(x), i64::from(y));
        let label = format!(
            "#{}  {}:{:02}",
            i + 1,
            (ts / 60.0).floor() as i64,
            (ts % 60.0).floor() as i64
        );
        let (x1, y1) = (
            (x + 8 * label.chars().count() as u32 + 8).min(sheet.width() - 1),
            (y + 18).min(sheet.height() - 1),
        );
        for py in y..=y1 {
            for px in x..=x1 {
                sheet.put_pixel(px, py, Rgb([0, 0, 0]));
            }
        }
        draw_text(&mut sheet, x + 4, y + 3, &label);
    }
    Ok((encode_jpeg(&sheet, 82)?, "image/jpeg".into()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn kinds_and_names() {
        assert_eq!(kind_of("a.HEIC"), "image");
        assert_eq!(kind_of("b.mov"), "video");
        assert_eq!(kind_of("c.pptx"), "pptx");
        assert_eq!(kind_of("d.xlsm"), "xlsx");
        assert_eq!(kind_of("e.md"), "text");
        assert_eq!(kind_of("f.zip"), "other");
        assert_eq!(kind_of(".bashrc"), "other");
        assert_eq!(mime_of("x.md"), "text/markdown");
        assert_eq!(mime_of("x.JPG"), "image/jpeg");
        assert_eq!(mime_of("x.mp4"), "video/mp4");
        assert_eq!(mime_of("x.unknownext"), "application/octet-stream");
        assert_eq!(safe_name("../../etc/passwd"), "passwd");
        assert_eq!(
            safe_name("C:\\Users\\me\\Report  Q3?.PDF"),
            "Report Q3_.pdf"
        );
        assert_eq!(safe_name("  ..  "), "file");
        assert_eq!(
            safe_name(&format!("{}.txt", "a".repeat(100))),
            format!("{}.txt", "a".repeat(80))
        );
        assert_eq!(splitext("dir/.hidden"), ("dir/.hidden", ""));
        assert_eq!(splitext("a.tar.gz"), ("a.tar", ".gz"));
    }

    #[test]
    fn save_upload_unique_and_checked() {
        let d = tempfile::tempdir().unwrap();
        let a = save_upload(d.path(), "notes.md", b"# hi").unwrap();
        assert!(a["path"].as_str().unwrap().starts_with("uploads/"));
        assert_eq!(a["kind"], "text");
        assert_eq!(a["mime"], "text/markdown");
        let b = save_upload(d.path(), "notes.md", b"# hi").unwrap();
        assert_eq!(b["name"], "notes (2).md");
        let e = save_upload(d.path(), "x.exe", b"MZ").unwrap_err();
        assert_eq!(
            e.to_string(),
            "不支持这种文件类型 .exe (unsupported file type)"
        );
        assert!(e.downcast_ref::<AttachmentError>().is_some());
        assert_eq!(
            save_upload(d.path(), "x.txt", b"").unwrap_err().to_string(),
            "文件是空的 (empty file)"
        );
        assert_eq!(
            text_of(&d.path().join(a["path"].as_str().unwrap()), 100).unwrap(),
            "# hi"
        );
    }

    fn write_zip(path: &Path, files: &[(&str, &str)]) {
        let mut z = zip::ZipWriter::new(std::fs::File::create(path).unwrap());
        for (n, c) in files {
            z.start_file(*n, zip::write::SimpleFileOptions::default())
                .unwrap();
            z.write_all(c.as_bytes()).unwrap();
        }
        z.finish().unwrap();
    }

    #[test]
    fn docx_and_pptx_text() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("a.docx");
        let w = r#"xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main""#;
        write_zip(
            &p,
            &[
                (
                    "word/document.xml",
                    &format!(
                        r#"<w:document {w}><w:body><w:p><w:pPr><w:pStyle w:val="Heading2"/></w:pPr><w:r><w:t>Title</w:t></w:r></w:p><w:p><w:r><w:t>Hello </w:t></w:r><w:r><w:t>world</w:t></w:r></w:p><w:tbl><w:tr><w:tc><w:p><w:r><w:t> a </w:t></w:r></w:p></w:tc><w:tc><w:p><w:r><w:t>b</w:t></w:r></w:p></w:tc></w:tr></w:tbl></w:body></w:document>"#
                    ),
                ),
                (
                    "word/footnotes.xml",
                    &format!(
                        r#"<w:footnotes {w}><w:footnote><w:p><w:r><w:t>note 1</w:t></w:r></w:p></w:footnote></w:footnotes>"#
                    ),
                ),
            ],
        );
        assert_eq!(
            docx_text(&p).unwrap(),
            "## Title\nHello world\n| a | b |\nnote 1"
        );
        let bad = d.path().join("b.docx");
        write_zip(&bad, &[("x.xml", "<x/>")]);
        assert_eq!(
            docx_text(&bad).unwrap_err().to_string(),
            "不是有效的 Word 文件 (not a .docx)"
        );
        let pp = d.path().join("c.pptx");
        let a = r#"xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main""#;
        write_zip(
            &pp,
            &[
                (
                    "ppt/slides/slide10.xml",
                    &format!(r#"<s {a}><a:p><a:r><a:t>ten</a:t></a:r></a:p></s>"#),
                ),
                (
                    "ppt/slides/slide2.xml",
                    &format!(r#"<s {a}><a:p><a:r><a:t>two</a:t></a:r></a:p></s>"#),
                ),
            ],
        );
        assert_eq!(pptx_text(&pp).unwrap(), "## Slide 1\ntwo\n## Slide 2\nten");
    }

    #[test]
    fn jpeg_conversion_and_contact_sheet() {
        let mut png = Vec::new();
        DynamicImage::ImageRgb8(RgbImage::from_pixel(2000, 1000, Rgb([220, 30, 30])))
            .write_to(&mut Cursor::new(&mut png), image::ImageFormat::Png)
            .unwrap();
        let (jpg, mime) = to_jpeg(&png, 1280).unwrap();
        assert_eq!(mime, "image/jpeg");
        let im = image::load_from_memory(&jpg).unwrap();
        assert_eq!(im.dimensions(), (1280, 640));
        let e = to_jpeg(b"not an image", 1280).unwrap_err();
        assert!(e
            .to_string()
            .starts_with("无法读取这张图片 (cannot read image): "));
        let frames: Vec<(f64, Vec<u8>)> = (0..4).map(|i| (i as f64 * 65.0, jpg.clone())).collect();
        let (sheet, _) = contact_sheet(&frames, 3, 420).unwrap();
        let s = image::load_from_memory(&sheet).unwrap();
        assert_eq!(s.width(), 3 * 420);
        assert_eq!(s.height(), 2 * (210 + 4));
    }
}
