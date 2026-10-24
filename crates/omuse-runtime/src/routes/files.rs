//! Workspace files (read-only for the UI): a directory listing and the raw file.

use std::path::{Component, Path, PathBuf};
use std::time::UNIX_EPOCH;

use axum::body::Body;
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use percent_encoding::{utf8_percent_encode, AsciiSet, NON_ALPHANUMERIC};
use serde_json::{json, Value};
use tokio::io::{AsyncReadExt, AsyncSeekExt};

use super::{http, int_param, not_found, Fail, St, Q, R};
use crate::agent::util::realpath;

/// Python `os.path.relpath(path, start)` for absolute paths.
pub(crate) fn relpath(path: &Path, start: &Path) -> String {
    let comps = |p: &Path| -> Vec<String> {
        p.components()
            .filter_map(|c| match c {
                Component::Normal(s) => Some(s.to_string_lossy().into_owned()),
                Component::ParentDir => Some("..".into()),
                _ => None,
            })
            .collect()
    };
    let (a, b) = (comps(path), comps(start));
    let common = a.iter().zip(b.iter()).take_while(|(x, y)| x == y).count();
    let mut parts: Vec<String> = vec!["..".into(); b.len() - common];
    parts.extend(a[common..].iter().cloned());
    if parts.is_empty() {
        ".".into()
    } else {
        parts.join("/")
    }
}

/// `realpath(join(WORKSPACE, path.lstrip("/")))`.
pub(crate) fn resolve(ws: &Path, path: &str) -> PathBuf {
    realpath(&ws.join(path.trim_start_matches('/')))
}

fn mtime(meta: &std::fs::Metadata) -> f64 {
    meta.modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

pub async fn files(st: St, q: Q) -> R {
    let ws = st.rt.workspace.clone();
    let path = q.get("path").cloned().unwrap_or_default();
    let base = resolve(&ws, &path);
    // a string prefix check, as in Python
    if !base.to_string_lossy().starts_with(&*ws.to_string_lossy()) {
        return Err(http(400, "Bad Request"));
    }
    if !base.is_dir() {
        return Err(not_found());
    }
    let mut names: Vec<String> = std::fs::read_dir(&base)
        .map_err(|e| Fail::Internal(e.into()))?
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    let mut items = Vec::new();
    for n in names {
        if n.starts_with('.') {
            continue;
        }
        let fp = base.join(&n);
        let meta = std::fs::metadata(&fp).or_else(|_| std::fs::symlink_metadata(&fp));
        let (is_dir, size, mt) = match &meta {
            Ok(m) => (
                m.is_dir(),
                if m.is_file() {
                    json!(m.len())
                } else {
                    Value::Null
                },
                mtime(m),
            ),
            Err(_) => (false, Value::Null, 0.0),
        };
        items.push(
            json!({"name": n, "path": relpath(&fp, &ws), "dir": is_dir, "size": size, "mtime": mt}),
        );
    }
    Ok(Json(json!({"path": relpath(&base, &ws), "items": items})))
}

/// `urllib.parse.quote(s)`: everything but letters, digits, `_.-~` and `/` percent-encoded.
const QUOTE: &AsciiSet = &NON_ALPHANUMERIC
    .remove(b'_')
    .remove(b'.')
    .remove(b'-')
    .remove(b'~')
    .remove(b'/');

fn content_disposition(kind: &str, filename: &str) -> String {
    let quoted = utf8_percent_encode(filename, QUOTE).to_string();
    if quoted != filename {
        format!("{}; filename*=utf-8''{}", kind, quoted)
    } else {
        format!("{}; filename=\"{}\"", kind, filename)
    }
}

/// The byte range of a `Range: bytes=…` header (one range): `Ok(None)` = whole file, `Err(())` = unsatisfiable.
fn parse_range(h: &str, size: u64) -> Result<Option<(u64, u64)>, ()> {
    let Some(spec) = h.trim().strip_prefix("bytes=") else {
        return Ok(None);
    };
    if spec.contains(',') {
        return Ok(None); // several ranges: the whole file
    }
    let (a, b) = spec.split_once('-').ok_or(())?;
    let (a, b) = (a.trim(), b.trim());
    let (start, end) = if a.is_empty() {
        let n: u64 = b.parse().map_err(|_| ())?;
        if n == 0 {
            return Err(());
        }
        (size.saturating_sub(n), size.saturating_sub(1))
    } else {
        let s: u64 = a.parse().map_err(|_| ())?;
        let e: u64 = if b.is_empty() {
            size.saturating_sub(1)
        } else {
            b.parse::<u64>()
                .map_err(|_| ())?
                .min(size.saturating_sub(1))
        };
        (s, e)
    };
    if size == 0 || start > end || start >= size {
        return Err(());
    }
    Ok(Some((start, end)))
}

fn hv(s: &str) -> HeaderValue {
    HeaderValue::from_str(s).unwrap_or_else(|_| HeaderValue::from_static(""))
}

pub async fn file_raw(st: St, q: Q, req_headers: HeaderMap) -> Result<Response, Fail> {
    let Some(path) = q.get("path").cloned() else {
        return Err(Fail::Api(omuse_common::api::ApiError::with_json(
            422,
            json!([{"type": "missing", "loc": ["query", "path"], "msg": "Field required", "input": null}]),
        )));
    };
    let download = int_param(&q, "query", "download", Some(0))?;
    let ws = st.rt.workspace.clone();
    let fp = resolve(&ws, &path);
    let fps = fp.to_string_lossy().to_string();
    if !fps.starts_with(&format!("{}/", ws.to_string_lossy()))
        || !fp.is_file()
        || fps.contains("/.quarantine/")
    {
        return Err(not_found());
    }
    // download=1: save with its real name (the chat's download button); otherwise open in the browser (PDF, images).
    // Active content (HTML/SVG/XML/JS, e.g. a page the agent downloaded) is never rendered on the app's own origin —
    // it could call the app's APIs — so it is always a download, and sandboxed just in case.
    let mime = mime_guess::from_path(&fp)
        .first_raw()
        .unwrap_or("application/octet-stream")
        .to_string();
    let low = fps.to_lowercase();
    let risky = ["html", "svg", "xml", "javascript"]
        .iter()
        .any(|x| mime.contains(x))
        || [".htm", ".html", ".svg", ".xhtml", ".js", ".mjs"]
            .iter()
            .any(|e| low.ends_with(e));
    let inline = download == 0 && !risky;
    let meta = tokio::fs::metadata(&fp)
        .await
        .map_err(|e| Fail::Internal(e.into()))?;
    let size = meta.len();
    let mt = mtime(&meta);
    let name = fp
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();

    let mut headers = HeaderMap::new();
    let ctype = if risky {
        "application/octet-stream".to_string()
    } else {
        mime.clone()
    };
    let ctype = if ctype.starts_with("text/") {
        format!("{}; charset=utf-8", ctype)
    } else {
        ctype
    };
    headers.insert(header::CONTENT_TYPE, hv(&ctype));
    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    if risky {
        headers.insert(
            header::CONTENT_SECURITY_POLICY,
            HeaderValue::from_static("sandbox; default-src 'none'"),
        );
    }
    headers.insert(
        header::CONTENT_DISPOSITION,
        hv(&content_disposition(
            if inline { "inline" } else { "attachment" },
            &name,
        )),
    );
    headers.insert(header::ACCEPT_RANGES, HeaderValue::from_static("bytes"));
    if let Some(dt) = chrono::DateTime::from_timestamp(mt as i64, 0) {
        headers.insert(
            header::LAST_MODIFIED,
            hv(&dt.format("%a, %d %b %Y %H:%M:%S GMT").to_string()),
        );
    }
    {
        use sha1::Digest;
        let tag = hex::encode(sha1::Sha1::digest(format!("{}-{}", mt, size).as_bytes()));
        headers.insert(header::ETAG, hv(&format!("\"{}\"", &tag[..32])));
    }

    let range = match req_headers.get(header::RANGE).and_then(|v| v.to_str().ok()) {
        Some(h) => parse_range(h, size),
        None => Ok(None),
    };
    let mut file = tokio::fs::File::open(&fp)
        .await
        .map_err(|e| Fail::Internal(e.into()))?;
    match range {
        Err(()) => {
            headers.insert(header::CONTENT_RANGE, hv(&format!("bytes */{}", size)));
            Ok((StatusCode::RANGE_NOT_SATISFIABLE, headers).into_response())
        }
        Ok(None) => {
            headers.insert(header::CONTENT_LENGTH, hv(&size.to_string()));
            let body = Body::from_stream(tokio_util::io::ReaderStream::new(file));
            Ok((StatusCode::OK, headers, body).into_response())
        }
        Ok(Some((start, end))) => {
            file.seek(std::io::SeekFrom::Start(start))
                .await
                .map_err(|e| Fail::Internal(e.into()))?;
            let len = end - start + 1;
            headers.insert(header::CONTENT_LENGTH, hv(&len.to_string()));
            headers.insert(
                header::CONTENT_RANGE,
                hv(&format!("bytes {}-{}/{}", start, end, size)),
            );
            let body = Body::from_stream(tokio_util::io::ReaderStream::new(file.take(len)));
            Ok((StatusCode::PARTIAL_CONTENT, headers, body).into_response())
        }
    }
}
