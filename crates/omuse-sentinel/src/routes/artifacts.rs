//! `/artifacts/…`: the files OMuse publishes for the user (the news research reports: `artifacts/news/<subject>/…`),
//! read from `<workspace>/artifacts/`. Behind the login gate like every route (`routes::app`). The HTML is OMuse's
//! own, written from escaped text, and is served under a policy that runs nothing and loads nothing from elsewhere.

use std::path::{Component, Path as FsPath, PathBuf};

use axum::body::Body;
use axum::extract::{Path, State};
use axum::http::{header, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};

use super::App;

/// The page policy of a published HTML file: its own inline styles and pictures only, no scripts, sandboxed.
pub const HTML_CSP: &str =
    "default-src 'none'; style-src 'unsafe-inline'; img-src data: https:; font-src data:; \
sandbox allow-popups allow-popups-to-escape-sandbox";

/// `rel` inside `root`, or `None` when it would leave it (`..`, an absolute path, a link out).
pub fn resolve(root: &FsPath, rel: &str) -> Option<PathBuf> {
    let rel = FsPath::new(rel.trim_start_matches('/'));
    if rel
        .components()
        .any(|c| !matches!(c, Component::Normal(_) | Component::CurDir))
    {
        return None;
    }
    let full = root.join(rel).canonicalize().ok()?;
    let root = root.canonicalize().ok()?;
    full.starts_with(&root).then_some(full)
}

fn mime(p: &FsPath) -> &'static str {
    match p
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_lowercase)
        .as_deref()
    {
        Some("html" | "htm") => "text/html; charset=utf-8",
        Some("pdf") => "application/pdf",
        Some("png") => "image/png",
        Some("jpg" | "jpeg") => "image/jpeg",
        Some("txt" | "md") => "text/plain; charset=utf-8",
        _ => "application/octet-stream",
    }
}

fn esc(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// Every file under `root`, newest first: (path relative to root, modified time).
fn listing(root: &FsPath) -> Vec<(String, std::time::SystemTime)> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&d) else {
            continue;
        };
        for e in rd.flatten() {
            let p = e.path();
            match e.file_type() {
                Ok(t) if t.is_dir() => stack.push(p),
                Ok(t) if t.is_file() => {
                    if let (Ok(rel), Ok(m)) = (p.strip_prefix(root), e.metadata()) {
                        out.push((
                            rel.to_string_lossy().replace('\\', "/"),
                            m.modified().unwrap_or(std::time::UNIX_EPOCH),
                        ));
                    }
                }
                _ => {}
            }
        }
    }
    out.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    out
}

fn index(root: &FsPath) -> Response {
    let rows: String = listing(root)
        .iter()
        .map(|(rel, _)| {
            format!(
                "<li><a href=\"/artifacts/{}\">{}</a></li>",
                esc(rel),
                esc(rel)
            )
        })
        .collect();
    let body = format!(
        "<!doctype html><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\
<title>Artifacts</title><style>body{{font:15px/1.5 system-ui,sans-serif;max-width:760px;margin:24px auto;padding:0 16px}}\
li{{margin:4px 0}}</style><h1>Artifacts</h1>{}",
        if rows.is_empty() {
            "<p>Nothing published yet.</p>".to_string()
        } else {
            format!("<ul>{rows}</ul>")
        }
    );
    file_response(body.into_bytes(), "text/html; charset=utf-8")
}

fn file_response(bytes: Vec<u8>, ct: &'static str) -> Response {
    let mut r = Response::new(Body::from(bytes));
    let h = r.headers_mut();
    h.insert(header::CONTENT_TYPE, HeaderValue::from_static(ct));
    h.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    h.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-cache"));
    if ct.starts_with("text/html") {
        h.insert(
            header::CONTENT_SECURITY_POLICY,
            HeaderValue::from_static(HTML_CSP),
        );
    }
    r
}

/// `GET /artifacts/`: the list, newest first.
pub async fn artifacts_index(State(ctx): State<App>) -> Response {
    index(&ctx.cfg.workspace().join("artifacts"))
}

/// `GET /artifacts/{path}`: one file, shown in the browser.
pub async fn artifact(State(ctx): State<App>, Path(rel): Path<String>) -> Response {
    let root = ctx.cfg.workspace().join("artifacts");
    if rel.trim_matches('/').is_empty() {
        return index(&root);
    }
    let Some(full) = resolve(&root, &rel).filter(|p| p.is_file()) else {
        return (StatusCode::NOT_FOUND, "Not Found").into_response();
    };
    match std::fs::read(&full) {
        Ok(b) => file_response(b, mime(&full)),
        Err(_) => (StatusCode::NOT_FOUND, "Not Found").into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_stay_inside_the_artifacts_folder() {
        let d = tempfile::tempdir().unwrap();
        let root = d.path().join("artifacts");
        std::fs::create_dir_all(root.join("news/x")).unwrap();
        std::fs::write(root.join("news/x/a.html"), "<p>hi</p>").unwrap();
        std::fs::write(d.path().join("secret.txt"), "no").unwrap();
        assert!(resolve(&root, "news/x/a.html").is_some());
        assert!(resolve(&root, "/news/x/a.html").is_some());
        assert!(resolve(&root, "../secret.txt").is_none());
        assert!(resolve(&root, "news/../../secret.txt").is_none());
        assert!(resolve(&root, "news/x/missing.html").is_none());
        assert_eq!(listing(&root)[0].0, "news/x/a.html");
        assert_eq!(mime(FsPath::new("a.PDF")), "application/pdf");
    }
}
