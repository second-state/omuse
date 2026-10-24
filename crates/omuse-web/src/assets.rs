//! The web UI's static files (web/), served from memory the way a CDN would — but from our own server only.
//!
//! Why: the page shell must depend on nothing but this server (no CDN `<link>`/`<script>`: a slow or blocked third
//! party leaves the page blank, and a cloud VM may sit behind a filtered network). And it must be cheap on both ends:
//! - every asset URL in index.html is stamped `?v=<version>`, a digest of all files, so a stamped URL changes exactly
//!   when the content does and may be cached for a year (`public, max-age=31536000, immutable`) — no revalidation
//!   round trips on a repeat visit; an unstamped or stale URL stays `no-cache` + ETag → 304, so a shipped UI change
//!   always reaches existing users (browsers otherwise cache JS/CSS heuristically for days);
//! - text assets are gzip-compressed once (level 9: ~510 KB of JS/CSS → ~160 KB) and served compressed when the
//!   client accepts it, with `Vary: Accept-Encoding` and their own ETag — never compressed per request on a one-vCPU
//!   host, never a middleware on every API request.
//!
//! The files come either from the binary ([`Assets::embedded`], `web/` compiled in with `include_dir!`) or from
//! a directory ([`Assets::from_dir`], the `WEB_DIR` hot patch): the directory is re-read when any file's size or
//! mtime changes, so a running instance never serves stale bytes. `index.html` is the page itself (rendered from the
//! Tera template in `templates.rs`), not an asset.

use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock};

use axum::body::Body;
use axum::http::header::{
    HeaderMap, HeaderValue, ACCEPT_ENCODING, CACHE_CONTROL, CONTENT_ENCODING, CONTENT_LENGTH,
    CONTENT_TYPE, ETAG, IF_NONE_MATCH, VARY,
};
use axum::http::{Method, StatusCode};
use axum::response::Response;
use flate2::{Compression, GzBuilder};
use include_dir::{include_dir, Dir};
use parking_lot::RwLock;
use regex::Regex;
use sha2::{Digest, Sha256};

/// The UI files compiled into the binary (`web/`).
pub static WEB: Dir<'static> = include_dir!("$CARGO_MANIFEST_DIR/../../web");

/// Extensions that get a pre-compressed gzip variant.
pub const COMPRESSIBLE: &[&str] = &[
    ".css", ".js", ".mjs", ".svg", ".json", ".html", ".txt", ".md", ".map",
];
/// Media types we name ourselves (the platform's mime table disagrees on `.js`).
pub const TEXT_TYPES: &[(&str, &str)] = &[
    (".js", "text/javascript"),
    (".mjs", "text/javascript"),
    (".css", "text/css"),
    (".html", "text/html"),
    (".json", "application/json"),
    (".svg", "image/svg+xml"),
    (".txt", "text/plain"),
    (".md", "text/markdown"),
];
/// `Cache-Control` for a URL stamped with the current version.
pub const IMMUTABLE: &str = "public, max-age=31536000, immutable";
/// `Cache-Control` for an unstamped or stale URL.
pub const REVALIDATE: &str = "no-cache";
/// The page shell's name: served as the rendered template, never as an asset.
pub const INDEX: &str = "index.html";

/// Python `(?P<q>["'])static/(?P<name>[A-Za-z0-9_.\-/]+)(?P=q)`, without the back-reference.
static STAMP_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#""static/(?P<dq>[A-Za-z0-9_.\-/]+)"|'static/(?P<sq>[A-Za-z0-9_.\-/]+)'"#)
        .expect("STAMP_RE")
});

/// One static file, loaded into memory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Asset {
    pub name: String,
    pub body: Vec<u8>,
    /// gzip variant for compressible types, else `None`.
    pub gz: Option<Vec<u8>>,
    pub media_type: String,
    /// `"<version>-<name>"`; the gzip variant is etag + `-gz"`.
    pub etag: String,
}

impl Asset {
    /// The ETag of the gzip variant: `"<version>-<name>-gz"`.
    fn etag_gz(&self) -> String {
        format!("{}-gz\"", &self.etag[..self.etag.len() - 1])
    }
}

/// `(relative name, mtime in ns, size)` of every file: the directory's identity for change detection.
type Snapshot = Vec<(String, u128, u64)>;

#[derive(Default)]
struct Loaded {
    key: Option<Snapshot>,
    version: String,
    files: HashMap<String, Arc<Asset>>,
}

enum Source {
    Embedded,
    Dir(PathBuf),
}

/// One set of files, loaded into memory; a directory source is re-loaded when a file's size or mtime changes.
pub struct Assets {
    source: Source,
    state: RwLock<Loaded>,
}

impl std::fmt::Debug for Assets {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let src = match &self.source {
            Source::Embedded => "embedded".to_string(),
            Source::Dir(p) => p.display().to_string(),
        };
        f.debug_struct("Assets")
            .field("source", &src)
            .field("version", &self.state.read().version)
            .finish()
    }
}

/// Python `os.path.splitext(name)[1].lower()`.
fn ext_of(name: &str) -> String {
    let base = name.rsplit('/').next().unwrap_or(name);
    match base.rfind('.') {
        Some(i) if i > 0 => base[i..].to_ascii_lowercase(),
        _ => String::new(),
    }
}

/// Media type for a file name, with `; charset=utf-8` on text-like types.
pub fn media_type(name: &str) -> String {
    let ext = ext_of(name);
    let mut mt = TEXT_TYPES
        .iter()
        .find(|(e, _)| *e == ext)
        .map(|(_, t)| t.to_string())
        .or_else(|| mime_guess::from_path(name).first_raw().map(str::to_string))
        .unwrap_or_else(|| "application/octet-stream".to_string());
    if mt.starts_with("text/") || mt == "application/json" || mt == "image/svg+xml" {
        mt.push_str("; charset=utf-8");
    }
    mt
}

/// `gzip.compress(body, compresslevel=9, mtime=0)`.
pub fn gzip9(body: &[u8]) -> Vec<u8> {
    let mut enc = GzBuilder::new()
        .mtime(0)
        .write(Vec::new(), Compression::new(9));
    enc.write_all(body).expect("gzip into Vec");
    enc.finish().expect("gzip finish")
}

/// Build the asset table from raw files: the version digest, media types, gzip variants and ETags.
fn build(raw: Vec<(String, Vec<u8>)>) -> (String, HashMap<String, Arc<Asset>>) {
    let mut sorted = raw;
    sorted.sort_by(|a, b| a.0.cmp(&b.0));
    let mut h = Sha256::new();
    for (name, body) in &sorted {
        h.update(name.as_bytes());
        h.update(b"\0");
        h.update(body);
    }
    let version = hex::encode(h.finalize())[..12].to_string();
    let files = sorted
        .into_iter()
        .map(|(name, body)| {
            let ext = ext_of(&name);
            let gz = COMPRESSIBLE.contains(&ext.as_str()).then(|| gzip9(&body));
            let asset = Asset {
                etag: format!("\"{version}-{name}\""),
                media_type: media_type(&name),
                name,
                body,
                gz,
            };
            (asset.name.clone(), Arc::new(asset))
        })
        .collect();
    (version, files)
}

fn embedded_files(dir: &Dir<'static>, out: &mut Vec<(String, Vec<u8>)>) {
    for f in dir.files() {
        let name = f.path().to_string_lossy().replace('\\', "/");
        if name != INDEX {
            out.push((name, f.contents().to_vec()));
        }
    }
    for d in dir.dirs() {
        embedded_files(d, out);
    }
}

impl Assets {
    /// The files compiled into the binary (`web/`, everything but `index.html`).
    pub fn embedded() -> Self {
        let mut raw = Vec::new();
        embedded_files(&WEB, &mut raw);
        let (version, files) = build(raw);
        Self {
            source: Source::Embedded,
            state: RwLock::new(Loaded {
                key: Some(Vec::new()),
                version,
                files,
            }),
        }
    }

    /// Files from a directory on disk, re-read whenever a file's size or mtime changes (`WEB_DIR`).
    pub fn from_dir(path: impl Into<PathBuf>) -> Self {
        let a = Self {
            source: Source::Dir(path.into()),
            state: RwLock::new(Loaded::default()),
        };
        a.sync();
        a
    }

    /// The source directory, when the files come from disk.
    pub fn dir(&self) -> Option<&Path> {
        match &self.source {
            Source::Embedded => None,
            Source::Dir(p) => Some(p),
        }
    }

    fn snapshot(dir: &Path) -> Snapshot {
        let mut out = Vec::new();
        for entry in walkdir::WalkDir::new(dir)
            .follow_links(true)
            .into_iter()
            .filter_map(Result::ok)
        {
            if !entry.file_type().is_file() {
                continue;
            }
            let Ok(st) = std::fs::metadata(entry.path()) else {
                continue;
            };
            let rel = match entry.path().strip_prefix(dir) {
                Ok(r) => r.to_string_lossy().replace(std::path::MAIN_SEPARATOR, "/"),
                Err(_) => continue,
            };
            if rel == INDEX {
                continue;
            }
            let mtime = st
                .modified()
                .ok()
                .and_then(|m| m.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_nanos())
                .unwrap_or(0);
            out.push((rel, mtime, st.len()));
        }
        out.sort();
        out
    }

    fn load(dir: &Path, key: &Snapshot) -> (String, HashMap<String, Arc<Asset>>) {
        let raw = key
            .iter()
            .filter_map(|(name, _, _)| {
                std::fs::read(dir.join(name))
                    .ok()
                    .map(|body| (name.clone(), body))
            })
            .collect();
        build(raw)
    }

    /// Re-read the directory when any file changed (no-op for the embedded files).
    fn sync(&self) {
        let Source::Dir(dir) = &self.source else {
            return;
        };
        let key = Self::snapshot(dir);
        if self.state.read().key.as_ref() == Some(&key) {
            return;
        }
        let mut st = self.state.write();
        if st.key.as_ref() != Some(&key) {
            let (version, files) = Self::load(dir, &key);
            *st = Loaded {
                key: Some(key),
                version,
                files,
            };
        }
    }

    /// The 12-hex digest of all files: the `?v=` stamp.
    pub fn version(&self) -> String {
        self.sync();
        self.state.read().version.clone()
    }

    /// One file by its name relative to the directory (`app.js`, `fonts/x.woff2`), or `None`.
    pub fn get(&self, name: &str) -> Option<Arc<Asset>> {
        self.sync();
        self.state.read().files.get(name).cloned()
    }

    /// The names of all files, sorted.
    pub fn names(&self) -> Vec<String> {
        self.sync();
        let mut v: Vec<String> = self.state.read().files.keys().cloned().collect();
        v.sort();
        v
    }

    /// The HTTP response for `GET`/`HEAD /static/<name>`: `query_v` is the request's `?v=` value.
    pub fn response(
        &self,
        name: &str,
        method: &Method,
        headers: &HeaderMap,
        query_v: Option<&str>,
    ) -> Response {
        let Some(a) = self.get(name) else {
            return Response::builder()
                .status(StatusCode::NOT_FOUND)
                .header(CONTENT_TYPE, "text/plain; charset=utf-8")
                .body(Body::from("no such static file"))
                .expect("static 404");
        };
        let stamped = query_v == Some(self.state.read().version.as_str());
        let accepts_gzip = headers
            .get(ACCEPT_ENCODING)
            .map(|v| {
                String::from_utf8_lossy(v.as_bytes())
                    .to_lowercase()
                    .contains("gzip")
            })
            .unwrap_or(false);
        let gz = if accepts_gzip { a.gz.as_deref() } else { None };
        let etag_gz = a.etag_gz();
        let etag = if gz.is_some() {
            etag_gz.as_str()
        } else {
            a.etag.as_str()
        };
        let mut b = Response::builder()
            .header(CACHE_CONTROL, if stamped { IMMUTABLE } else { REVALIDATE })
            .header(ETAG, etag);
        if a.gz.is_some() {
            b = b.header(VARY, "Accept-Encoding");
        }
        let inm = headers
            .get(IF_NONE_MATCH)
            .map(|v| String::from_utf8_lossy(v.as_bytes()).into_owned());
        // the client may hold either variant; both stand for the same bytes
        if inm
            .as_deref()
            .is_some_and(|s| s.contains(a.etag.as_str()) || s.contains(etag_gz.as_str()))
        {
            return b
                .status(StatusCode::NOT_MODIFIED)
                .body(Body::empty())
                .expect("static 304");
        }
        if gz.is_some() {
            b = b.header(CONTENT_ENCODING, "gzip");
        }
        let bytes = gz.unwrap_or(&a.body);
        let body = if method == Method::HEAD {
            Body::empty()
        } else {
            Body::from(bytes.to_vec())
        };
        b.header(CONTENT_LENGTH, HeaderValue::from(bytes.len()))
            .header(CONTENT_TYPE, a.media_type.as_str())
            .body(body)
            .expect("static response")
    }
}

/// Python `Assets.stamp`: every `static/<file>` URL in `html` gets `?v=<version>`.
pub fn stamp(html: &str, version: &str) -> String {
    STAMP_RE
        .replace_all(html, |c: &regex::Captures| {
            match (c.name("dq"), c.name("sq")) {
                (Some(n), _) => format!("\"static/{}?v={version}\"", n.as_str()),
                (None, Some(n)) => format!("'static/{}?v={version}'", n.as_str()),
                (None, None) => c[0].to_string(),
            }
        })
        .into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::to_bytes;
    use std::io::Read;

    fn req(headers: &[(&str, &str)]) -> HeaderMap {
        let mut h = HeaderMap::new();
        for (k, v) in headers {
            h.insert(
                axum::http::HeaderName::from_bytes(k.as_bytes()).unwrap(),
                HeaderValue::from_str(v).unwrap(),
            );
        }
        h
    }

    fn hdr<'a>(r: &'a Response, k: &str) -> Option<&'a str> {
        r.headers().get(k).map(|v| v.to_str().unwrap())
    }

    async fn body(r: Response) -> Vec<u8> {
        to_bytes(r.into_body(), usize::MAX).await.unwrap().to_vec()
    }

    #[tokio::test]
    async fn stamp_cache_rules_gzip_and_reload() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("app.js"), "console.log('x');".repeat(200)).unwrap();
        let mut png = b"\x89PNG".to_vec();
        png.extend(std::iter::repeat_n(0u8, 300));
        std::fs::write(tmp.path().join("pic.png"), &png).unwrap();
        std::fs::write(tmp.path().join("index.html"), "<html>").unwrap();
        let a = Assets::from_dir(tmp.path());
        let v = a.version();
        assert_eq!(v.len(), 12);
        assert_eq!(a.names(), vec!["app.js", "pic.png"]); // index.html is the page, not an asset
        let html = stamp(
            r#"<link rel="stylesheet" href="static/app.css"><script src="static/app.js"></script>"#,
            &v,
        );
        assert_eq!(
            html,
            format!(
                r#"<link rel="stylesheet" href="static/app.css?v={v}"><script src="static/app.js?v={v}"></script>"#
            )
        );
        assert_eq!(
            stamp("x='static/a/b.js' y=\"static/c.css' z=static/d.js", &v),
            format!("x='static/a/b.js?v={v}' y=\"static/c.css' z=static/d.js")
        );

        // unstamped: identity body, revalidate every time, ETag → 304
        let r = a.response("app.js", &Method::GET, &req(&[]), None);
        assert_eq!(r.status(), 200);
        assert_eq!(hdr(&r, "cache-control"), Some(REVALIDATE));
        assert_eq!(
            hdr(&r, "content-type"),
            Some("text/javascript; charset=utf-8")
        );
        assert_eq!(hdr(&r, "vary"), Some("Accept-Encoding"));
        assert!(hdr(&r, "content-encoding").is_none());
        assert_eq!(hdr(&r, "content-length"), Some("3400"));
        let etag = hdr(&r, "etag").unwrap().to_string();
        assert_eq!(etag, format!("\"{v}-app.js\""));
        assert_eq!(body(r).await.len(), 3400);
        let r = a.response(
            "app.js",
            &Method::GET,
            &req(&[("if-none-match", &etag)]),
            None,
        );
        assert_eq!(r.status(), 304);
        assert_eq!(hdr(&r, "etag"), Some(etag.as_str()));
        assert!(hdr(&r, "content-length").is_none());
        // stamped with the current version: immutable for a year; a stale stamp is not
        assert_eq!(
            hdr(
                &a.response("app.js", &Method::GET, &req(&[]), Some(&v)),
                "cache-control"
            ),
            Some(IMMUTABLE)
        );
        assert_eq!(
            hdr(
                &a.response("app.js", &Method::GET, &req(&[]), Some("deadbeef0000")),
                "cache-control"
            ),
            Some(REVALIDATE)
        );
        // gzip when accepted: smaller, its own ETag, and the plain ETag still validates
        let r = a.response(
            "app.js",
            &Method::GET,
            &req(&[("accept-encoding", "gzip, deflate, br")]),
            None,
        );
        assert_eq!(hdr(&r, "content-encoding"), Some("gzip"));
        assert!(hdr(&r, "etag").unwrap().ends_with("-gz\""));
        let len: usize = hdr(&r, "content-length").unwrap().parse().unwrap();
        let gz = body(r).await;
        assert_eq!(gz.len(), len);
        assert!(gz.len() * 3 < 200 * 17);
        // the gzip variant decodes to the plain bytes and carries mtime 0
        assert_eq!(&gz[4..8], &[0, 0, 0, 0]);
        let mut plain = Vec::new();
        flate2::read::GzDecoder::new(&gz[..])
            .read_to_end(&mut plain)
            .unwrap();
        assert_eq!(plain, "console.log('x');".repeat(200).into_bytes());
        let r = a.response(
            "app.js",
            &Method::GET,
            &req(&[("accept-encoding", "gzip"), ("if-none-match", &etag)]),
            None,
        );
        assert_eq!(r.status(), 304);
        // HEAD: the headers of the GET, no body
        let r = a.response(
            "app.js",
            &Method::HEAD,
            &req(&[("accept-encoding", "gzip")]),
            None,
        );
        assert_eq!(hdr(&r, "content-length"), Some(len.to_string().as_str()));
        assert!(body(r).await.is_empty());
        // binaries: never compressed, no Vary; unknown and traversal: 404
        let r = a.response(
            "pic.png",
            &Method::GET,
            &req(&[("accept-encoding", "gzip")]),
            None,
        );
        assert_eq!(hdr(&r, "content-type"), Some("image/png"));
        assert!(hdr(&r, "content-encoding").is_none() && hdr(&r, "vary").is_none());
        assert_eq!(hdr(&r, "content-length"), Some("304"));
        let r = a.response("nope.js", &Method::GET, &req(&[]), None);
        assert_eq!(r.status(), 404);
        assert_eq!(hdr(&r, "content-type"), Some("text/plain; charset=utf-8"));
        assert_eq!(body(r).await, b"no such static file");
        assert_eq!(
            a.response("../Cargo.toml", &Method::GET, &req(&[]), None)
                .status(),
            404
        );
        assert_eq!(
            a.response("index.html", &Method::GET, &req(&[]), None)
                .status(),
            404
        );
        // a changed file on disk (hot patch) changes the version and the served bytes
        std::fs::write(tmp.path().join("app.js"), "console.log('y');").unwrap();
        let f = std::fs::File::options()
            .write(true)
            .open(tmp.path().join("app.js"))
            .unwrap();
        f.set_modified(std::time::SystemTime::now() + std::time::Duration::from_millis(10))
            .unwrap();
        let v2 = a.version();
        assert_ne!(v2, v);
        assert_eq!(a.get("app.js").unwrap().body, b"console.log('y');");
        assert_eq!(
            hdr(
                &a.response("app.js", &Method::GET, &req(&[]), Some(&v)),
                "cache-control"
            ),
            Some(REVALIDATE)
        );
        assert_eq!(
            hdr(
                &a.response("app.js", &Method::GET, &req(&[]), Some(&v2)),
                "cache-control"
            ),
            Some(IMMUTABLE)
        );
        // a same-size edit with a newer mtime is seen too
        std::fs::write(tmp.path().join("app.js"), "console.log('z');").unwrap();
        let f = std::fs::File::options()
            .write(true)
            .open(tmp.path().join("app.js"))
            .unwrap();
        f.set_modified(std::time::SystemTime::now() + std::time::Duration::from_secs(2))
            .unwrap();
        assert_ne!(a.version(), v2);
        assert_eq!(a.get("app.js").unwrap().body, b"console.log('z');");
    }

    #[test]
    fn version_is_sha256_of_sorted_name_nul_body() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir(tmp.path().join("sub")).unwrap();
        std::fs::write(tmp.path().join("sub/b.txt"), b"bee").unwrap();
        std::fs::write(tmp.path().join("a.css"), b"a{}").unwrap();
        let a = Assets::from_dir(tmp.path());
        let mut h = Sha256::new();
        for (name, body) in [("a.css", &b"a{}"[..]), ("sub/b.txt", &b"bee"[..])] {
            h.update(name.as_bytes());
            h.update(b"\0");
            h.update(body);
        }
        assert_eq!(a.version(), hex::encode(h.finalize())[..12]);
        assert_eq!(
            a.get("sub/b.txt").unwrap().etag,
            format!("\"{}-sub/b.txt\"", a.version())
        );
        assert_eq!(
            a.get("sub/b.txt").unwrap().media_type,
            "text/plain; charset=utf-8"
        );
    }

    #[test]
    fn media_types_match_python() {
        assert_eq!(media_type("app.js"), "text/javascript; charset=utf-8");
        assert_eq!(media_type("x.mjs"), "text/javascript; charset=utf-8");
        assert_eq!(media_type("app.css"), "text/css; charset=utf-8");
        assert_eq!(media_type("i.svg"), "image/svg+xml; charset=utf-8");
        assert_eq!(media_type("d.json"), "application/json; charset=utf-8");
        assert_eq!(media_type("r.md"), "text/markdown; charset=utf-8");
        assert_eq!(media_type("p.PNG"), "image/png");
        assert_eq!(media_type("f.woff2"), "font/woff2");
        assert_eq!(media_type("noext"), "application/octet-stream");
        assert_eq!(media_type(".hidden"), "application/octet-stream");
    }

    #[test]
    fn embedded_has_the_ui_files() {
        let a = Assets::embedded();
        let names = a.names();
        for n in [
            "app.css",
            "app.js",
            "i18n.js",
            "i18n_tw.js",
            "marked.min.js",
            "purify.min.js",
        ] {
            assert!(names.contains(&n.to_string()), "{n} missing from {names:?}");
        }
        assert!(!names.contains(&INDEX.to_string()));
        assert_eq!(a.version().len(), 12);
        assert_eq!(a.version(), a.version());
        let js = a.get("app.js").unwrap();
        assert!(js.gz.as_ref().unwrap().len() * 10 < js.body.len() * 4);
        assert!(a.dir().is_none());
        // the page shell itself loads nothing off-site
        let html = WEB.get_file(INDEX).unwrap().contents_utf8().unwrap();
        let re = Regex::new(r#"<(?:link|script)\b[^>]*\b(?:href|src)="([^"]+)""#).unwrap();
        for c in re.captures_iter(html) {
            assert!(
                c[1].starts_with("static/") || c[1].starts_with("data:"),
                "{}",
                &c[1]
            );
        }
        let css = String::from_utf8_lossy(&a.get("app.css").unwrap().body).into_owned();
        assert!(!css.contains("@import") && !css.contains("url(http") && !css.contains("//fonts."));
    }
}
