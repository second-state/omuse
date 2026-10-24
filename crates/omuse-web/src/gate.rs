//! Login gate for standalone (Docker / Fly) installs.
//!
//! On Olares the entrance does the login, so Sentinel itself has none. Published straight from Docker it would be
//! open to anyone who finds the port, so this wraps Sentinel in a login. The password is `[auth] password` until the
//! user sets their own in Settings (`passwd`). It lives outside the sentinel crate on purpose: it depends on
//! nothing but two closures (`verify`, `version`).
//!
//! Two ways in:
//! 1. A signed session cookie, set by the login page (GET/POST /login). This is what browsers use. It is valid for
//!    `session_days` or until the password changes, and it survives server restarts (the signing key is kept in
//!    `<sentinel data>/session.key`) and the browser dropping the page from memory (iOS does that constantly; HTTP
//!    Basic credentials live only in that memory, which is why Basic auth kept asking on the phone).
//! 2. HTTP Basic auth, sent preemptively by scripts (curl -u, httpx auth=...). The gate issues a Basic challenge only
//!    to clients that are not browsers (no Sec-Fetch-Mode header), so a browser never sees the native password dialog.
//!
//! An unauthenticated browser navigation is redirected to `/login?next=<url>`; a browser fetch gets a 401 JSON answer
//! and the web app sends the user to /login. GET or POST /logout clears the cookie.
//!
//! Not gated: `/sentinel/api/health` (container health check) and `/internal/*` (the runtime's calls; those carry
//! their own runtime token and Sentinel checks it). The phone port is a separate app that is public by design.
//!
//! Mount: `Router::layer(axum::middleware::from_fn_with_state(Arc<Gate>, Gate::layer))` — `/login` and `/logout`
//! are answered inside the middleware, so the router needs no routes for them.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use axum::body::Body;
use axum::extract::{Request, State};
use axum::http::header::{
    HeaderMap, HeaderName, HeaderValue, ACCEPT_LANGUAGE, ALLOW, AUTHORIZATION, CACHE_CONTROL,
    CONTENT_TYPE, COOKIE, LOCATION, SET_COOKIE, UPGRADE, WWW_AUTHENTICATE,
};
use axum::http::{Method, StatusCode};
use axum::middleware::Next;
use axum::response::Response;
use base64::Engine;
use hmac::{Hmac, Mac};
use http_body_util::BodyExt;
use parking_lot::Mutex;
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;

use crate::templates::{LoginPage, Templates};
use omuse_common::util;

/// Paths that need no login.
pub const OPEN_PATHS: &[&str] = &["/sentinel/api/health"];
/// Path prefixes that need no login.
pub const OPEN_PREFIXES: &[&str] = &["/internal/"];
/// Seconds to wait after bad credentials (slows password guessing).
pub const FAIL_DELAY_S: f64 = 1.0;
/// The session cookie's name.
pub const COOKIE_NAME: &str = "omuse_session";
/// Default session lifetime.
pub const SESSION_DAYS: u32 = 180;
pub const LOGIN_PATH: &str = "/login";
pub const LOGOUT_PATH: &str = "/logout";
/// The login form body is read up to this many bytes.
const MAX_FORM: usize = 65536;

/// `verify(user, password) -> bool`: the real check (slow: a password hash).
pub type VerifyFn = Arc<dyn Fn(&str, &str) -> bool + Send + Sync>;
/// `version() -> int`: changes when the password does (sessions and the Basic cache are dropped then).
pub type VersionFn = Arc<dyn Fn() -> i64 + Send + Sync>;

/// The texts of the login page in one language.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Texts {
    pub title: &'static str,
    pub user_label: &'static str,
    pub pw_label: &'static str,
    pub button: &'static str,
    pub wrong: &'static str,
    pub hint: &'static str,
}

/// Page language, from `Accept-Language`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lang {
    Zh,
    En,
}

impl Lang {
    /// `"zh" if "zh" in accept_language.lower()[:40] else "en"`.
    pub fn detect(headers: &HeaderMap) -> Lang {
        let al = headers
            .get(ACCEPT_LANGUAGE)
            .map(|v| latin1(v.as_bytes()).to_lowercase())
            .unwrap_or_default();
        if util::take_chars(&al, 40).contains("zh") {
            Lang::Zh
        } else {
            Lang::En
        }
    }

    /// The `<html lang>` attribute.
    pub fn html_lang(self) -> &'static str {
        match self {
            Lang::Zh => "zh-CN",
            Lang::En => "en",
        }
    }
}

/// The bilingual `TEXT` table.
pub fn text(lang: Lang) -> &'static Texts {
    match lang {
        Lang::Zh => &Texts {
            title: "登录 Sign in",
            user_label: "登录名 User",
            pw_label: "密码 Password",
            button: "登录 Sign in",
            wrong: "登录名或密码不对 (wrong user or password)",
            hint: "登录后这个浏览器会保持登录状态；改密码后需要重新登录。",
        },
        Lang::En => &Texts {
            title: "Sign in",
            user_label: "User",
            pw_label: "Password",
            button: "Sign in",
            wrong: "Wrong user or password",
            hint: "This browser stays signed in; changing the password signs every browser out.",
        },
    }
}

/// The cookie signing key: created once, kept with the other Sentinel state so sessions outlive restarts.
pub fn load_session_key(path: &Path) -> std::io::Result<Vec<u8>> {
    if let Ok(raw) = std::fs::read(path) {
        let key = raw.trim_ascii();
        if key.len() >= 32 {
            return Ok(key.to_vec());
        }
    }
    let key = util::token_hex(32).into_bytes();
    util::write_atomic(path, &key, 0o600)?;
    Ok(key)
}

/// Only a path on this site (with query and fragment): never another host.
pub fn safe_next(v: &str) -> String {
    let v = v.trim();
    if !v.starts_with('/')
        || v.starts_with("//")
        || v.starts_with("/\\")
        || v.contains('\n')
        || v.contains('\r')
    {
        return "/".to_string();
    }
    v.to_string()
}

/// Python `urllib.parse.quote_plus`: unreserved `A-Za-z0-9_.-~` kept, space → `+`, the rest `%XX`.
pub fn quote_plus(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'_' | b'.' | b'-' | b'~' => {
                out.push(b as char)
            }
            b' ' => out.push('+'),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// Bytes as a Latin-1 string (how ASGI decodes header values).
fn latin1(b: &[u8]) -> String {
    b.iter().map(|&c| c as char).collect()
}

fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// `parse_qs(...).get(key)[0]`: the first non-blank value of `key` (parse_qs drops blank values).
fn first_value(form: &str, key: &str) -> Option<String> {
    form_urlencoded::parse(form.as_bytes())
        .find(|(k, v)| k == key && !v.is_empty())
        .map(|(_, v)| v.into_owned())
}

/// The login gate: an axum middleware ([`Gate::layer`]) plus the `/login` and `/logout` pages it answers itself.
///
/// The last accepted Basic credentials are remembered by digest, so the password hash runs once per sign-in, not
/// per request.
pub struct Gate {
    verify: VerifyFn,
    version: VersionFn,
    user: String,
    key: Vec<u8>,
    max_age: i64,
    fail_delay: Duration,
    proxy_headers: bool,
    /// (password version, sha256 of the accepted "user:password")
    ok: Mutex<Option<(i64, [u8; 32])>>,
    templates: Templates,
}

impl std::fmt::Debug for Gate {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Gate")
            .field("user", &self.user)
            .field("max_age", &self.max_age)
            .field("fail_delay", &self.fail_delay)
            .field("proxy_headers", &self.proxy_headers)
            .finish_non_exhaustive()
    }
}

impl Gate {
    /// Build a gate whose signing key is loaded from (or created at) `session_key_path` with mode 0600.
    pub fn new(
        verify: VerifyFn,
        version: VersionFn,
        user: String,
        session_key_path: PathBuf,
        session_days: u32,
        fail_delay_secs: f64,
    ) -> anyhow::Result<Gate> {
        let key = load_session_key(&session_key_path)
            .map_err(|e| anyhow::anyhow!("cannot create {}: {e}", session_key_path.display()))?;
        Ok(Self::with_key(
            verify,
            version,
            user,
            key,
            session_days,
            fail_delay_secs,
        ))
    }

    /// Build a gate with an explicit signing key (tests; the Python `session_key=` argument).
    pub fn with_key(
        verify: VerifyFn,
        version: VersionFn,
        user: String,
        key: Vec<u8>,
        session_days: u32,
        fail_delay_secs: f64,
    ) -> Gate {
        Gate {
            verify,
            version,
            user,
            key,
            max_age: i64::from(session_days) * 86400,
            fail_delay: Duration::from_secs_f64(fail_delay_secs.max(0.0)),
            proxy_headers: true,
            ok: Mutex::new(None),
            templates: Templates::new(),
        }
    }

    /// Whether `X-Forwarded-Proto` decides the request scheme (uvicorn `--proxy-headers`; default on).
    pub fn with_proxy_headers(mut self, trust: bool) -> Self {
        self.proxy_headers = trust;
        self
    }

    /// The signing key (`session.key` contents).
    pub fn key(&self) -> &[u8] {
        &self.key
    }

    /// The login name shown pre-filled on the page.
    pub fn user(&self) -> &str {
        &self.user
    }

    // ------------------------------------------------------------ session cookies

    /// `hmac(key, payload, sha256).hexdigest()[:40]`.
    pub fn sign(&self, payload: &str) -> String {
        let mut mac =
            Hmac::<Sha256>::new_from_slice(&self.key).expect("hmac accepts any key length");
        mac.update(payload.as_bytes());
        hex::encode(mac.finalize().into_bytes())[..40].to_string()
    }

    /// A fresh cookie value: `issued.version.nonce.sig`.
    pub fn issue_cookie(&self) -> String {
        let payload = format!(
            "{}.{}.{}",
            now_secs(),
            (self.version)(),
            util::token_urlsafe(9)
        );
        let sig = self.sign(&payload);
        format!("{payload}.{sig}")
    }

    /// Signed by us, same password version, not older than the session lifetime.
    pub fn cookie_valid(&self, value: &str) -> bool {
        let parts: Vec<&str> = value.split('.').collect();
        let [issued, ver, nonce, sig] = parts[..] else {
            return false;
        };
        let payload = format!("{issued}.{ver}.{nonce}");
        if self
            .sign(&payload)
            .as_bytes()
            .ct_eq(sig.as_bytes())
            .unwrap_u8()
            != 1
        {
            return false;
        }
        let (Ok(ver), Ok(issued)) = (ver.trim().parse::<i64>(), issued.trim().parse::<i64>())
        else {
            return false;
        };
        let age = now_secs() - issued;
        ver == (self.version)() && (0..=self.max_age).contains(&age)
    }

    /// Every `Cookie` header joined (HTTP/2 clients may send several), as name → value.
    fn cookies(headers: &HeaderMap) -> Vec<(String, String)> {
        let raw = headers
            .get_all(COOKIE)
            .iter()
            .map(|v| latin1(v.as_bytes()))
            .collect::<Vec<_>>()
            .join("; ");
        let mut out: Vec<(String, String)> = Vec::new();
        for part in raw.split(';') {
            let part = part.trim();
            let (k, v) = part.split_once('=').unwrap_or((part, ""));
            if k.is_empty() {
                continue;
            }
            match out.iter_mut().find(|(ek, _)| ek == k) {
                Some(e) => e.1 = v.to_string(),
                None => out.push((k.to_string(), v.to_string())),
            }
        }
        out
    }

    fn session_cookie(headers: &HeaderMap) -> Option<String> {
        Self::cookies(headers)
            .into_iter()
            .find(|(k, _)| k == COOKIE_NAME)
            .map(|(_, v)| v)
    }

    /// `http`, `https`, `ws`, `wss`: from `X-Forwarded-Proto` when proxy headers are trusted, else the request URI.
    fn scheme(&self, req: &Request) -> String {
        if self.proxy_headers {
            if let Some(v) = req.headers().get("x-forwarded-proto") {
                let s = latin1(v.as_bytes())
                    .split(',')
                    .next()
                    .unwrap_or("")
                    .trim()
                    .to_lowercase();
                if !s.is_empty() {
                    return s;
                }
            }
        }
        req.uri().scheme_str().unwrap_or("http").to_lowercase()
    }

    /// The `Set-Cookie` value; `clear` makes it expire at once.
    fn set_cookie(&self, value: &str, scheme: &str, clear: bool) -> HeaderValue {
        let secure = if scheme == "https" || scheme == "wss" {
            "; Secure"
        } else {
            ""
        };
        let age = if clear { 0 } else { self.max_age };
        HeaderValue::from_str(&format!(
            "{COOKIE_NAME}={value}; Path=/; Max-Age={age}; HttpOnly; SameSite=Lax{secure}"
        ))
        .expect("cookie header")
    }

    // ------------------------------------------------------------ who may pass

    /// `Authorization: Basic <base64 user:password>`, with the digest cache.
    pub fn basic_ok(&self, v: &[u8]) -> bool {
        let (scheme, cred) = match v.iter().position(|&b| b == b' ') {
            Some(i) => (&v[..i], &v[i + 1..]),
            None => (v, &b""[..]),
        };
        if !scheme.eq_ignore_ascii_case(b"basic") {
            return false;
        }
        let Ok(given) = base64::engine::general_purpose::STANDARD.decode(cred.trim_ascii()) else {
            return false;
        };
        let digest: [u8; 32] = Sha256::digest(&given).into();
        let ver = (self.version)();
        if let Some((okv, okd)) = *self.ok.lock() {
            if okv == ver && okd.ct_eq(&digest).unwrap_u8() == 1 {
                return true;
            }
        }
        let Ok(s) = std::str::from_utf8(&given) else {
            return false;
        };
        let (user, password) = s.split_once(':').unwrap_or((s, ""));
        if (self.verify)(user, password) {
            *self.ok.lock() = Some((ver, digest));
            return true;
        }
        false
    }

    /// Basic credentials when sent (then the cookie is not consulted), else a valid session cookie.
    pub fn authorized(&self, headers: &HeaderMap) -> bool {
        if let Some(a) = headers.get(AUTHORIZATION) {
            return self.basic_ok(a.as_bytes());
        }
        Self::session_cookie(headers).is_some_and(|c| !c.is_empty() && self.cookie_valid(&c))
    }

    // ------------------------------------------------------------ responses

    /// A response of the gate itself: always `Cache-Control: no-store`.
    fn respond(
        status: StatusCode,
        headers: Vec<(HeaderName, HeaderValue)>,
        body: impl Into<Body>,
    ) -> Response {
        let mut b = Response::builder().status(status);
        for (k, v) in headers {
            b = b.header(k, v);
        }
        b.header(CACHE_CONTROL, "no-store")
            .body(body.into())
            .expect("gate response")
    }

    fn html() -> (HeaderName, HeaderValue) {
        (
            CONTENT_TYPE,
            HeaderValue::from_static("text/html; charset=utf-8"),
        )
    }

    /// The login page in the request's language.
    pub fn page(&self, headers: &HeaderMap, next: &str, user: &str, error: &str) -> String {
        let lang = Lang::detect(headers);
        let t = text(lang);
        self.templates.render_login(&LoginPage {
            lang: lang.html_lang(),
            title: t.title,
            user_label: t.user_label,
            pw_label: t.pw_label,
            button: t.button,
            hint: t.hint,
            error,
            user: if user.is_empty() { &self.user } else { user },
            next,
        })
    }

    async fn fail_delay(&self) {
        if !self.fail_delay.is_zero() {
            tokio::time::sleep(self.fail_delay).await;
        }
    }

    /// The form body, at most `MAX_FORM` bytes (the rest is drained and dropped).
    async fn form_body(body: Body) -> Vec<u8> {
        let mut out = Vec::new();
        let mut body = body;
        while let Some(frame) = body.frame().await {
            let Ok(frame) = frame else { break };
            if let Ok(data) = frame.into_data() {
                let room = MAX_FORM.saturating_sub(out.len());
                out.extend_from_slice(&data[..data.len().min(room)]);
            }
        }
        out
    }

    async fn login(&self, req: Request) -> Response {
        let query = req.uri().query().unwrap_or("").to_string();
        let nxt = safe_next(&first_value(&query, "next").unwrap_or_default());
        let headers = req.headers().clone();
        let scheme = self.scheme(&req);
        if req.method() == Method::GET {
            if self.authorized(&headers) {
                return Self::respond(
                    StatusCode::SEE_OTHER,
                    vec![(LOCATION, header_value(&nxt))],
                    Body::empty(),
                );
            }
            return Self::respond(
                StatusCode::OK,
                vec![Self::html()],
                self.page(&headers, &nxt, "", ""),
            );
        }
        if req.method() != Method::POST {
            return Self::respond(
                StatusCode::METHOD_NOT_ALLOWED,
                vec![(ALLOW, HeaderValue::from_static("GET, POST"))],
                Body::empty(),
            );
        }
        let form = String::from_utf8_lossy(&Self::form_body(req.into_body()).await).into_owned();
        let user = first_value(&form, "user")
            .unwrap_or_default()
            .trim()
            .to_string();
        let password = first_value(&form, "password").unwrap_or_default();
        let nxt = first_value(&form, "next")
            .map(|n| safe_next(&n))
            .unwrap_or(nxt);
        if !(self.verify)(&user, &password) {
            self.fail_delay().await;
            let wrong = text(Lang::detect(&headers)).wrong;
            return Self::respond(
                StatusCode::UNAUTHORIZED,
                vec![Self::html()],
                self.page(&headers, &nxt, &user, wrong),
            );
        }
        Self::respond(
            StatusCode::SEE_OTHER,
            vec![
                (LOCATION, header_value(&nxt)),
                (
                    SET_COOKIE,
                    self.set_cookie(&self.issue_cookie(), &scheme, false),
                ),
            ],
            Body::empty(),
        )
    }

    fn logout(&self, req: &Request) -> Response {
        let scheme = self.scheme(req);
        Self::respond(
            StatusCode::SEE_OTHER,
            vec![
                (LOCATION, HeaderValue::from_static(LOGIN_PATH)),
                (SET_COOKIE, self.set_cookie("", &scheme, true)),
            ],
            Body::empty(),
        )
    }

    /// The middleware: `axum::middleware::from_fn_with_state(gate, Gate::layer)`.
    pub async fn layer(State(gate): State<Arc<Gate>>, req: Request, next: Next) -> Response {
        gate.handle(req, next).await
    }

    /// The Python `__call__`.
    pub async fn handle(&self, req: Request, next: Next) -> Response {
        let path = percent_encoding::percent_decode_str(req.uri().path())
            .decode_utf8_lossy()
            .into_owned();
        if path == LOGIN_PATH {
            return self.login(req).await;
        }
        if path == LOGOUT_PATH {
            return self.logout(&req);
        }
        let headers = req.headers();
        if OPEN_PATHS.contains(&path.as_str())
            || OPEN_PREFIXES.iter().any(|p| path.starts_with(p))
            || self.authorized(headers)
        {
            return next.run(req).await;
        }
        if headers.contains_key(AUTHORIZATION) {
            self.fail_delay().await;
        }
        let headers = req.headers();
        let is_ws = headers
            .get(UPGRADE)
            .is_some_and(|v| v.as_bytes().eq_ignore_ascii_case(b"websocket"));
        if is_ws {
            // the ASGI gate closed the socket with 1008 before accepting it; on the wire that is a refused handshake
            return Self::respond(StatusCode::FORBIDDEN, vec![], Body::empty());
        }
        let mode = headers
            .get("sec-fetch-mode")
            .map(|v| latin1(v.as_bytes()).to_lowercase())
            .unwrap_or_default();
        if mode == "navigate" && (req.method() == Method::GET || req.method() == Method::HEAD) {
            let here = match req.uri().query() {
                Some(q) if !q.is_empty() => format!("{path}?{q}"),
                _ => path,
            };
            let loc = if here != "/" {
                format!("{LOGIN_PATH}?next={}", quote_plus(&here))
            } else {
                LOGIN_PATH.to_string()
            };
            return Self::respond(
                StatusCode::FOUND,
                vec![(LOCATION, header_value(&loc))],
                Body::empty(),
            );
        }
        if !mode.is_empty() {
            // a browser's fetch / XHR / EventSource: no Basic challenge, so no native password dialog
            return Self::respond(
                StatusCode::UNAUTHORIZED,
                vec![(CONTENT_TYPE, HeaderValue::from_static("application/json"))],
                "{\"detail\": \"login required\"}",
            );
        }
        Self::respond(
            StatusCode::UNAUTHORIZED,
            vec![
                (
                    WWW_AUTHENTICATE,
                    HeaderValue::from_static("Basic realm=\"OMuse\", charset=\"UTF-8\""),
                ),
                (
                    CONTENT_TYPE,
                    HeaderValue::from_static("text/plain; charset=utf-8"),
                ),
            ],
            "login required",
        )
    }
}

/// A header value from text that may carry non-ASCII (a `next` URL): Latin-1 bytes like the ASGI server sent.
fn header_value(s: &str) -> HeaderValue {
    HeaderValue::from_str(s).unwrap_or_else(|_| {
        let bytes: Vec<u8> = s.bytes().filter(|b| *b >= 0x20 && *b != 0x7f).collect();
        HeaderValue::from_bytes(&bytes).unwrap_or_else(|_| HeaderValue::from_static("/"))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::to_bytes;
    use axum::routing::get;
    use axum::Router;
    use std::sync::atomic::{AtomicI64, AtomicUsize, Ordering};
    use tower::ServiceExt;

    fn b64(s: &str) -> String {
        base64::engine::general_purpose::STANDARD.encode(s)
    }

    fn app(gate: Arc<Gate>) -> Router {
        Router::new()
            .route("/", get(|| async { "app" }))
            .fallback(|| async { "app" })
            .layer(axum::middleware::from_fn_with_state(gate, Gate::layer))
    }

    struct Resp {
        status: u16,
        headers: HeaderMap,
        body: Vec<u8>,
    }

    impl Resp {
        fn h(&self, k: &str) -> Option<String> {
            self.headers
                .get(k)
                .map(|v| String::from_utf8_lossy(v.as_bytes()).into_owned())
        }
        fn text(&self) -> String {
            String::from_utf8_lossy(&self.body).into_owned()
        }
    }

    async fn call(
        app: &Router,
        method: &str,
        uri: &str,
        headers: &[(&str, &str)],
        body: &str,
    ) -> Resp {
        let mut b = Request::builder().method(method).uri(uri);
        for (k, v) in headers {
            b = b.header(*k, *v);
        }
        if !body.is_empty() {
            b = b.header("content-type", "application/x-www-form-urlencoded");
        }
        let r = app
            .clone()
            .oneshot(b.body(Body::from(body.to_string())).unwrap())
            .await
            .unwrap();
        let status = r.status().as_u16();
        let headers = r.headers().clone();
        let body = to_bytes(r.into_body(), usize::MAX).await.unwrap().to_vec();
        Resp {
            status,
            headers,
            body,
        }
    }

    fn gate_with(verify: VerifyFn, version: VersionFn, key: Vec<u8>) -> Arc<Gate> {
        Arc::new(Gate::with_key(
            verify,
            version,
            "omuse".into(),
            key,
            SESSION_DAYS,
            0.0,
        ))
    }

    const NAV: &[(&str, &str)] = &[
        ("sec-fetch-mode", "navigate"),
        ("accept-language", "zh-CN,zh;q=0.9"),
    ];
    const CORS: &[(&str, &str)] = &[("sec-fetch-mode", "cors")];

    #[test]
    fn safe_next_only_local_paths() {
        assert_eq!(safe_next("/#settings"), "/#settings");
        assert_eq!(safe_next("  /tasks?a=1 "), "/tasks?a=1");
        assert_eq!(safe_next("//evil.example/x"), "/");
        assert_eq!(safe_next("/\\evil.example"), "/");
        assert_eq!(safe_next("https://evil.example/"), "/");
        assert_eq!(safe_next("/a\r\nSet-Cookie: x"), "/");
        assert_eq!(safe_next(""), "/");
    }

    #[test]
    fn quote_plus_like_python() {
        assert_eq!(quote_plus("/tasks?a=1"), "%2Ftasks%3Fa%3D1");
        assert_eq!(quote_plus("/a b~*"), "%2Fa+b~%2A");
        assert_eq!(quote_plus("/#设置"), "%2F%23%E8%AE%BE%E7%BD%AE");
    }

    #[test]
    fn session_key_created_once_with_mode_600() {
        let tmp = tempfile::tempdir().unwrap();
        let p = tmp.path().join("sentinel").join("session.key");
        let k1 = load_session_key(&p).unwrap();
        assert_eq!(k1.len(), 64);
        assert!(k1.iter().all(|b| b.is_ascii_hexdigit()));
        assert_eq!(std::fs::read(&p).unwrap(), k1);
        assert!(!p.with_extension("key.tmp").exists());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&p).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
        assert_eq!(load_session_key(&p).unwrap(), k1);
        // whitespace around the key is ignored; a short key is replaced
        std::fs::write(&p, b"  abcdefghijklmnopqrstuvwxyz0123456789\n").unwrap();
        assert_eq!(
            load_session_key(&p).unwrap(),
            b"abcdefghijklmnopqrstuvwxyz0123456789"
        );
        std::fs::write(&p, b"short").unwrap();
        let k3 = load_session_key(&p).unwrap();
        assert_eq!(k3.len(), 64);
        assert_ne!(k3, k1);
        // Gate::new loads the same key
        let g = Gate::new(
            Arc::new(|_, _| false),
            Arc::new(|| 0),
            "omuse".into(),
            p.clone(),
            180,
            0.0,
        )
        .unwrap();
        assert_eq!(g.key(), &k3[..]);
    }

    #[test]
    fn signing_and_cookie_validity() {
        let ver = Arc::new(AtomicI64::new(1));
        let v = ver.clone();
        let g = gate_with(
            Arc::new(|_, _| false),
            Arc::new(move || v.load(Ordering::SeqCst)),
            b"k".repeat(32),
        );
        // HMAC-SHA256 hex[:40] of the payload with key "k"*32
        let sig = g.sign("1.2.abc");
        assert_eq!(sig.len(), 40);
        let mut mac = Hmac::<Sha256>::new_from_slice(&b"k".repeat(32)).unwrap();
        mac.update(b"1.2.abc");
        assert_eq!(sig, hex::encode(mac.finalize().into_bytes())[..40]);
        let c = g.issue_cookie();
        let parts: Vec<&str> = c.split('.').collect();
        assert_eq!(parts.len(), 4);
        assert_eq!(parts[1], "1");
        assert_eq!(parts[2].len(), 12);
        assert!(g.cookie_valid(&c));
        // forged, empty, expired, or from before a password change: no
        let mut forged = c.clone();
        let last = forged.pop().unwrap();
        forged.push(if last != '0' { '0' } else { '1' });
        assert!(!g.cookie_valid(&forged));
        assert!(!g.cookie_valid(""));
        assert!(!g.cookie_valid("a.b.c"));
        let old = format!("{}.1.{}", now_secs() - 181 * 86400, parts[2]);
        assert!(!g.cookie_valid(&format!("{old}.{}", g.sign(&old))));
        let future = format!("{}.1.{}", now_secs() + 5, parts[2]);
        assert!(!g.cookie_valid(&format!("{future}.{}", g.sign(&future))));
        ver.store(2, Ordering::SeqCst);
        assert!(!g.cookie_valid(&c));
        ver.store(1, Ordering::SeqCst);
        assert!(g.cookie_valid(&c));
        // a second gate with the same key (a restart) accepts it; another key does not
        let g2 = gate_with(Arc::new(|_, _| false), Arc::new(|| 1), b"k".repeat(32));
        assert!(g2.cookie_valid(&c));
        let g3 = gate_with(Arc::new(|_, _| false), Arc::new(|| 1), b"j".repeat(32));
        assert!(!g3.cookie_valid(&c));
    }

    #[tokio::test]
    async fn basic_auth_with_digest_cache() {
        let calls = Arc::new(AtomicUsize::new(0));
        let ver = Arc::new(AtomicI64::new(0));
        let (c, v) = (calls.clone(), ver.clone());
        let g = gate_with(
            Arc::new(move |u, p| {
                c.fetch_add(1, Ordering::SeqCst);
                u == "omuse" && p == "pä55"
            }),
            Arc::new(move || v.load(Ordering::SeqCst)),
            b"k".repeat(32),
        );
        let app = app(g.clone());
        let basic = |cred: &str| format!("Basic {}", b64(cred));
        let r = call(&app, "GET", "/", &[], "").await;
        assert_eq!(r.status, 401);
        assert_eq!(
            r.h("www-authenticate").as_deref(),
            Some("Basic realm=\"OMuse\", charset=\"UTF-8\"")
        );
        assert_eq!(
            r.h("content-type").as_deref(),
            Some("text/plain; charset=utf-8")
        );
        assert_eq!(r.h("cache-control").as_deref(), Some("no-store"));
        assert_eq!(r.text(), "login required");
        assert_eq!(
            call(
                &app,
                "GET",
                "/api/stream",
                &[("authorization", &basic("omuse:wrong"))],
                ""
            )
            .await
            .status,
            401
        );
        let bearer = format!("Bearer {}", b64("omuse:pä55"));
        assert_eq!(
            call(&app, "GET", "/", &[("authorization", &bearer)], "")
                .await
                .status,
            401
        );
        assert_eq!(
            call(
                &app,
                "GET",
                "/",
                &[("authorization", "Basic not*base64")],
                ""
            )
            .await
            .status,
            401
        );
        let r = call(
            &app,
            "GET",
            "/api/stream",
            &[("authorization", &basic("omuse:pä55"))],
            "",
        )
        .await;
        assert_eq!((r.status, r.text()), (200, "app".into()));
        assert_eq!(
            call(&app, "GET", "/sentinel/api/health", &[], "")
                .await
                .status,
            200
        ); // container health check
        assert_eq!(
            call(&app, "GET", "/internal/act", &[], "").await.status,
            200
        ); // runtime calls: own token
        assert_eq!(call(&app, "GET", "/internalx", &[], "").await.status, 401);
        let n = calls.load(Ordering::SeqCst);
        // accepted once: no second slow check
        assert_eq!(
            call(
                &app,
                "GET",
                "/api/stream",
                &[("authorization", &basic("omuse:pä55"))],
                ""
            )
            .await
            .status,
            200
        );
        assert_eq!(calls.load(Ordering::SeqCst), n);
        assert_eq!(
            call(
                &app,
                "GET",
                "/api/stream",
                &[("authorization", &basic("omuse:wrong"))],
                ""
            )
            .await
            .status,
            401
        );
        assert_eq!(calls.load(Ordering::SeqCst), n + 1);
        ver.store(2, Ordering::SeqCst); // password changed: cache dropped
        assert_eq!(
            call(
                &app,
                "GET",
                "/api/stream",
                &[("authorization", &basic("omuse:pä55"))],
                ""
            )
            .await
            .status,
            200
        );
        assert_eq!(calls.load(Ordering::SeqCst), n + 2);
        // the websocket handshake without credentials is refused
        let r = call(
            &app,
            "GET",
            "/api/ws",
            &[("upgrade", "websocket"), ("connection", "Upgrade")],
            "",
        )
        .await;
        assert_eq!(r.status, 403);
    }

    #[tokio::test]
    async fn cookie_login_flow() {
        let ver = Arc::new(AtomicI64::new(1));
        let v = ver.clone();
        let g = gate_with(
            Arc::new(|u, p| u == "omuse" && p == "secret-1"),
            Arc::new(move || v.load(Ordering::SeqCst)),
            b"k".repeat(32),
        );
        let app = app(g.clone());
        // a browser without a session: navigation -> login page (with the url to come back to), fetch -> 401 json
        let r = call(&app, "GET", "/", NAV, "").await;
        assert_eq!((r.status, r.h("location")), (302, Some("/login".into())));
        assert_eq!(r.h("cache-control").as_deref(), Some("no-store"));
        let r = call(&app, "GET", "/tasks?a=1", NAV, "").await;
        assert_eq!(
            (r.status, r.h("location")),
            (302, Some("/login?next=%2Ftasks%3Fa%3D1".into()))
        );
        let r = call(&app, "GET", "/api/stream", CORS, "").await;
        assert_eq!(r.status, 401);
        assert!(r.h("www-authenticate").is_none());
        assert_eq!(r.h("content-type").as_deref(), Some("application/json"));
        assert_eq!(r.text(), "{\"detail\": \"login required\"}");
        let r = call(&app, "POST", "/api/x", NAV, "").await; // a navigate POST is not redirected
        assert_eq!(
            (r.status, r.h("content-type")),
            (401, Some("application/json".into()))
        );
        let r = call(&app, "GET", "/api/stream", &[], "").await; // curl / httpx: the Basic challenge
        assert_eq!(r.status, 401);
        assert!(r.h("www-authenticate").unwrap().contains("Basic"));
        // the login page
        let r = call(&app, "GET", "/login?next=/%23settings", NAV, "").await;
        assert_eq!(r.status, 200);
        assert_eq!(
            r.h("content-type").as_deref(),
            Some("text/html; charset=utf-8")
        );
        assert_eq!(r.h("cache-control").as_deref(), Some("no-store"));
        let body = r.text();
        assert!(
            body.contains("name=\"password\"")
                && body.contains("value=\"/#settings\"")
                && body.contains("登录")
        );
        assert!(body.contains("<html lang=\"zh-CN\">") && body.contains("value=\"omuse\""));
        assert!(!body.contains("class=\"err\""));
        let r = call(
            &app,
            "GET",
            "/login",
            &[("sec-fetch-mode", "navigate"), ("accept-language", "en-US")],
            "",
        )
        .await;
        let body = r.text();
        assert!(r.status == 200 && body.contains("Sign in") && !body.contains("登录"));
        assert!(body.contains("<html lang=\"en\">") && body.contains("value=\"/\">"));
        let r = call(&app, "PUT", "/login", NAV, "").await;
        assert_eq!((r.status, r.h("allow")), (405, Some("GET, POST".into())));
        // wrong password: 401 page with the error, no cookie
        let r = call(
            &app,
            "POST",
            "/login",
            NAV,
            "user=omuse&password=wrong&next=/",
        )
        .await;
        assert_eq!(r.status, 401);
        assert!(r.h("set-cookie").is_none());
        assert!(r
            .text()
            .contains("<p class=\"err\">登录名或密码不对 (wrong user or password)</p>"));
        let r = call(
            &app,
            "POST",
            "/login",
            &[("accept-language", "en")],
            "user=me%3Cb%3E&password=wrong",
        )
        .await;
        assert!(
            r.text()
                .contains("<p class=\"err\">Wrong user or password</p>")
                && r.text().contains("value=\"me&lt;b&gt;\"")
        );
        // right password: 303 + cookie; off-site next ignored
        let r = call(
            &app,
            "POST",
            "/login",
            NAV,
            "user=omuse&password=secret-1&next=//evil.example/x",
        )
        .await;
        assert_eq!((r.status, r.h("location")), (303, Some("/".into())));
        let ck = r.h("set-cookie").unwrap();
        assert!(ck.starts_with("omuse_session="));
        assert!(ck.contains("; Path=/; Max-Age=15552000; HttpOnly; SameSite=Lax"));
        assert!(!ck.contains("Secure"));
        // behind HTTPS (Fly): Secure
        let r = call(
            &app,
            "POST",
            "/login",
            &[
                ("sec-fetch-mode", "navigate"),
                ("x-forwarded-proto", "https"),
            ],
            "user=omuse&password=secret-1&next=/%23settings",
        )
        .await;
        assert_eq!(
            (r.status, r.h("location")),
            (303, Some("/#settings".into()))
        );
        assert!(r
            .h("set-cookie")
            .unwrap()
            .ends_with("; SameSite=Lax; Secure"));
        let r = call(
            &app,
            "POST",
            "http://h/login",
            &[("sec-fetch-mode", "navigate")],
            "user=omuse&password=secret-1",
        )
        .await;
        assert!(!r.h("set-cookie").unwrap().contains("Secure"));
        let r = call(
            &app,
            "POST",
            "https://h/login",
            &[("sec-fetch-mode", "navigate")],
            "user=omuse&password=secret-1",
        )
        .await;
        assert!(r.h("set-cookie").unwrap().ends_with("; Secure"));
        let cookie = ck.split(';').next().unwrap().to_string();
        // the session works for navigations, fetches and the stream; the login page sends a signed-in user on
        for (path, hs) in [
            ("/", NAV),
            ("/api/stream", CORS),
            ("/sentinel/api/audit", &[][..]),
        ] {
            let mut h = hs.to_vec();
            let ck = format!("x=1; {cookie}");
            h.push(("cookie", &ck));
            let r = call(&app, "GET", path, &h, "").await;
            assert_eq!((r.status, r.text()), (200, "app".into()), "{path}");
        }
        // several Cookie headers (HTTP/2) are joined
        let r = call(
            &app,
            "GET",
            "/",
            &[("cookie", "x=1"), ("cookie", &cookie)],
            "",
        )
        .await;
        assert_eq!(r.status, 200);
        let r = call(
            &app,
            "GET",
            "/login?next=/%23tasks",
            &[("sec-fetch-mode", "navigate"), ("cookie", &cookie)],
            "",
        )
        .await;
        assert_eq!((r.status, r.h("location")), (303, Some("/#tasks".into())));
        // a restart: a new gate with the same key still accepts it
        let g2 = gate_with(Arc::new(|_, _| false), Arc::new(|| 1), b"k".repeat(32));
        assert!(g2.cookie_valid(cookie.split_once('=').unwrap().1));
        // a password change invalidates it
        ver.store(2, Ordering::SeqCst);
        let mut h = NAV.to_vec();
        h.push(("cookie", &cookie));
        assert_eq!(call(&app, "GET", "/", &h, "").await.status, 302);
        ver.store(1, Ordering::SeqCst);
        assert_eq!(call(&app, "GET", "/", &h, "").await.status, 200);
        // sign out clears the cookie
        let r = call(&app, "GET", "/logout", &h, "").await;
        assert_eq!((r.status, r.h("location")), (303, Some("/login".into())));
        assert_eq!(
            r.h("set-cookie").as_deref(),
            Some("omuse_session=; Path=/; Max-Age=0; HttpOnly; SameSite=Lax")
        );
        assert_eq!(call(&app, "POST", "/logout", &[], "").await.status, 303);
        // the health check and the runtime's calls stay open
        assert_eq!(
            call(&app, "GET", "/sentinel/api/health", NAV, "")
                .await
                .status,
            200
        );
        assert_eq!(
            call(&app, "GET", "/internal/act", NAV, "").await.status,
            200
        );
    }

    #[tokio::test]
    async fn login_is_answered_without_a_route_and_proxy_headers_can_be_ignored() {
        let g = Arc::new(
            Gate::with_key(
                Arc::new(|_, p| p == "pw"),
                Arc::new(|| 0),
                "omuse".into(),
                b"k".repeat(32),
                1,
                0.0,
            )
            .with_proxy_headers(false),
        );
        let app = Router::new()
            .route("/", get(|| async { "app" }))
            .layer(axum::middleware::from_fn_with_state(g, Gate::layer));
        assert_eq!(call(&app, "GET", "/login", NAV, "").await.status, 200);
        assert_eq!(call(&app, "GET", "/logout", NAV, "").await.status, 303);
        assert_eq!(call(&app, "GET", "/nope", NAV, "").await.status, 302);
        let r = call(
            &app,
            "POST",
            "/login",
            &[("x-forwarded-proto", "https")],
            "user=omuse&password=pw",
        )
        .await;
        let ck = r.h("set-cookie").unwrap();
        assert!(r.status == 303 && ck.contains("Max-Age=86400") && !ck.contains("Secure"));
    }

    #[tokio::test]
    async fn bad_credentials_wait() {
        let g = Arc::new(Gate::with_key(
            Arc::new(|_, _| false),
            Arc::new(|| 0),
            "omuse".into(),
            b"k".repeat(32),
            1,
            0.05,
        ));
        let app = app(g);
        let t = std::time::Instant::now();
        assert_eq!(
            call(
                &app,
                "GET",
                "/",
                &[("authorization", &format!("Basic {}", b64("a:b")))],
                ""
            )
            .await
            .status,
            401
        );
        assert!(t.elapsed() >= Duration::from_millis(50));
        let t = std::time::Instant::now();
        assert_eq!(
            call(&app, "POST", "/login", &[], "user=a&password=b")
                .await
                .status,
            401
        );
        assert!(t.elapsed() >= Duration::from_millis(50));
        let t = std::time::Instant::now();
        assert_eq!(call(&app, "GET", "/", &[], "").await.status, 401); // no credentials: no wait
        assert!(t.elapsed() < Duration::from_millis(50));
    }
}
