//! A thin, `imaplib`-shaped async IMAP4rev1 client.
//!
//! The Python connector drove `imaplib` and parsed the raw server responses itself (FETCH heads with
//! `X-GM-MSGID` / `X-GM-LABELS`, SEARCH results, LIST lines), so this module reproduces `imaplib`'s data
//! model instead of a typed protocol: a command returns its status (`OK` / `NO`; `BAD` is an error, like
//! `imaplib.IMAP4.error`) plus the untagged responses of the command's name as byte items, where a response
//! carrying a literal is `(head, literal)` and the trailer after the literal is a separate line item.
//! Client-side literals (`UID SEARCH CHARSET UTF-8 … {n}`, `APPEND`) and `AUTHENTICATE XOAUTH2` are supported.
//!
//! Transport: plain TCP, implicit TLS (IMAPS 993) or STARTTLS, all through [`connect`]; tests plug in an
//! in-memory stream (see [`fake`]).

use std::fmt;
use std::sync::Arc;
use std::time::Duration;

use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine;
use regex::bytes::Regex as BytesRegex;
use std::sync::LazyLock;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio_rustls::rustls::pki_types::ServerName;
use tokio_rustls::rustls::{ClientConfig, RootCertStore};
use tokio_rustls::TlsConnector;

/// Socket timeout per read / write (`imaplib` `timeout=TIMEOUT`).
pub const TIMEOUT: Duration = Duration::from_secs(30);

/// Anything we can speak IMAP over.
pub trait AsyncStream: AsyncRead + AsyncWrite + Unpin + Send {}
impl<T: AsyncRead + AsyncWrite + Unpin + Send> AsyncStream for T {}

/// A boxed stream (TCP, TLS or an in-memory test pipe).
pub type BoxStream = Box<dyn AsyncStream>;

/// `imaplib.IMAP4.error` (protocol / BAD / failed login) vs. socket errors (`OSError`).
#[derive(Debug, thiserror::Error)]
pub enum ImapError {
    /// Protocol-level failure (`BAD`, failed LOGIN, unexpected data).
    #[error("{0}")]
    Protocol(String),
    /// Socket / TLS failure.
    #[error("{0}")]
    Io(#[from] std::io::Error),
}

/// One untagged data item, like `imaplib` returns them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Item {
    /// A whole line (without CRLF).
    Line(Vec<u8>),
    /// A line that ended in `{n}` and the `n` bytes that followed.
    Literal { head: Vec<u8>, data: Vec<u8> },
}

impl Item {
    /// The line / head bytes.
    pub fn head(&self) -> &[u8] {
        match self {
            Item::Line(l) => l,
            Item::Literal { head, .. } => head,
        }
    }

    /// The literal bytes (`None` for a plain line).
    pub fn literal(&self) -> Option<&[u8]> {
        match self {
            Item::Line(_) => None,
            Item::Literal { data, .. } => Some(data),
        }
    }
}

/// A completed command: tagged status + the untagged items of the command's name.
#[derive(Debug, Clone, Default)]
pub struct Resp {
    /// `OK` or `NO`.
    pub typ: String,
    /// Text of the tagged status line.
    pub text: String,
    /// Untagged data for the command (e.g. the `SEARCH` numbers line, `FETCH` items, `LIST` lines).
    pub data: Vec<Item>,
}

impl Resp {
    /// `typ == "OK"`.
    pub fn ok(&self) -> bool {
        self.typ == "OK"
    }

    /// `data[0]` as bytes (`b""` when absent) — the `(data[0] or b"")` idiom.
    pub fn first(&self) -> &[u8] {
        self.data.first().map(|i| i.head()).unwrap_or(b"")
    }

    /// Whitespace-separated numbers of the first data line (SEARCH results).
    pub fn numbers(&self) -> Vec<String> {
        String::from_utf8_lossy(self.first())
            .split_whitespace()
            .map(str::to_string)
            .collect()
    }

    /// Python repr of the data list used in error messages (`[b'...']`).
    pub fn py_data(&self) -> String {
        format!(
            "[b'{}']",
            self.text.replace('\\', "\\\\").replace('\'', "\\'")
        )
    }
}

impl fmt::Display for Resp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} {}", self.typ, self.text)
    }
}

static LITERAL_TAIL: LazyLock<BytesRegex> =
    LazyLock::new(|| BytesRegex::new(r"\{(\d+)\}$").unwrap());
static UNTAGGED: LazyLock<BytesRegex> =
    LazyLock::new(|| BytesRegex::new(r"^\* ([A-Z-]+)( (.*))?$").unwrap());
static UNTAGGED_STATUS: LazyLock<BytesRegex> =
    LazyLock::new(|| BytesRegex::new(r"^\* (\d+) ([A-Z-]+)( (.*))?$").unwrap());

enum Line {
    Tagged {
        tag: Vec<u8>,
        typ: String,
        text: String,
    },
    Continue,
    Untagged {
        typ: String,
        items: Vec<Item>,
    },
    Other,
}

/// The IMAP connection.
pub struct ImapClient<S> {
    stream: S,
    buf: Vec<u8>,
    tagn: u32,
    /// Upper-cased capability names (after [`ImapClient::capability`]).
    pub capabilities: Vec<String>,
    /// Untagged responses not consumed by a command (BYE text etc.).
    pub last_bye: Option<String>,
}

impl<S: AsyncStream> ImapClient<S> {
    /// Wrap a connected stream and read the server greeting.
    pub async fn new(stream: S) -> Result<Self, ImapError> {
        let mut c = Self {
            stream,
            buf: Vec::new(),
            tagn: 0,
            capabilities: Vec::new(),
            last_bye: None,
        };
        let line = c.read_line().await?;
        let txt = String::from_utf8_lossy(&line).to_string();
        if txt.starts_with("* OK") || txt.starts_with("* PREAUTH") {
            Ok(c)
        } else {
            Err(ImapError::Protocol(format!("unexpected greeting: {txt}")))
        }
    }

    /// Give the raw stream back (used to upgrade to TLS after STARTTLS).
    pub fn into_inner(self) -> S {
        self.stream
    }

    async fn fill(&mut self) -> Result<(), ImapError> {
        let mut tmp = [0u8; 8192];
        let n = tokio::time::timeout(TIMEOUT, self.stream.read(&mut tmp))
            .await
            .map_err(|_| std::io::Error::new(std::io::ErrorKind::TimedOut, "timed out"))??;
        if n == 0 {
            return Err(ImapError::Io(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "connection closed",
            )));
        }
        self.buf.extend_from_slice(&tmp[..n]);
        Ok(())
    }

    async fn read_line(&mut self) -> Result<Vec<u8>, ImapError> {
        loop {
            if let Some(pos) = self.buf.windows(2).position(|w| w == b"\r\n") {
                let line = self.buf[..pos].to_vec();
                self.buf.drain(..pos + 2);
                return Ok(line);
            }
            self.fill().await?;
        }
    }

    async fn read_exact(&mut self, n: usize) -> Result<Vec<u8>, ImapError> {
        while self.buf.len() < n {
            self.fill().await?;
        }
        let out = self.buf[..n].to_vec();
        self.buf.drain(..n);
        Ok(out)
    }

    async fn write(&mut self, data: &[u8]) -> Result<(), ImapError> {
        tokio::time::timeout(TIMEOUT, async {
            self.stream.write_all(data).await?;
            self.stream.flush().await
        })
        .await
        .map_err(|_| std::io::Error::new(std::io::ErrorKind::TimedOut, "timed out"))??;
        Ok(())
    }

    /// Read one response (a line plus any literals and trailers it carries).
    async fn read_response(&mut self) -> Result<Line, ImapError> {
        let line = self.read_line().await?;
        if line.starts_with(b"+") {
            return Ok(Line::Continue);
        }
        if let Some(m) = UNTAGGED_STATUS.captures(&line) {
            let typ = String::from_utf8_lossy(&m[2]).to_string();
            let mut dat = m[1].to_vec();
            if let Some(d2) = m.get(4) {
                dat.push(b' ');
                dat.extend_from_slice(d2.as_bytes());
            }
            let items = self.read_literals(dat).await?;
            return Ok(Line::Untagged { typ, items });
        }
        if let Some(m) = UNTAGGED.captures(&line) {
            let typ = String::from_utf8_lossy(&m[1]).to_string();
            let dat = m.get(3).map(|d| d.as_bytes().to_vec()).unwrap_or_default();
            if typ == "BYE" {
                self.last_bye = Some(String::from_utf8_lossy(&dat).to_string());
            }
            let items = self.read_literals(dat).await?;
            return Ok(Line::Untagged { typ, items });
        }
        // tagged: "TAG OK text"
        let txt = String::from_utf8_lossy(&line).to_string();
        let mut parts = txt.splitn(3, ' ');
        let tag = parts.next().unwrap_or("").as_bytes().to_vec();
        let typ = parts.next().unwrap_or("").to_uppercase();
        let text = parts.next().unwrap_or("").to_string();
        if matches!(typ.as_str(), "OK" | "NO" | "BAD") {
            return Ok(Line::Tagged { tag, typ, text });
        }
        Ok(Line::Other)
    }

    /// imaplib: while the line ends in `{n}`, read the literal and the next line; the final trailer is a plain item.
    async fn read_literals(&mut self, mut dat: Vec<u8>) -> Result<Vec<Item>, ImapError> {
        let mut items = Vec::new();
        loop {
            let size = LITERAL_TAIL
                .captures(&dat)
                .and_then(|m| String::from_utf8_lossy(&m[1]).parse::<usize>().ok());
            match size {
                Some(n) => {
                    let data = self.read_exact(n).await?;
                    items.push(Item::Literal { head: dat, data });
                    dat = self.read_line().await?;
                }
                None => {
                    items.push(Item::Line(dat));
                    return Ok(items);
                }
            }
        }
    }

    fn next_tag(&mut self) -> Vec<u8> {
        self.tagn += 1;
        format!("A{:04}", self.tagn).into_bytes()
    }

    /// Send `name args…` (plus a trailing literal) and collect the untagged responses named `want`.
    /// `BAD` → `Err`, like `imaplib`; `NO` is returned as a normal [`Resp`].
    pub async fn command(
        &mut self,
        name: &str,
        args: &[&str],
        literal: Option<&[u8]>,
        want: &str,
    ) -> Result<Resp, ImapError> {
        let tag = self.next_tag();
        let mut line = tag.clone();
        line.push(b' ');
        line.extend_from_slice(name.as_bytes());
        for a in args {
            if a.is_empty() {
                continue;
            }
            line.push(b' ');
            line.extend_from_slice(a.as_bytes());
        }
        if let Some(lit) = literal {
            line.extend_from_slice(format!(" {{{}}}", lit.len()).as_bytes());
        }
        line.extend_from_slice(b"\r\n");
        self.write(&line).await?;
        let mut data: Vec<Item> = Vec::new();
        let mut pending_literal = literal;
        loop {
            match self.read_response().await? {
                Line::Continue => {
                    if let Some(lit) = pending_literal.take() {
                        let mut out = lit.to_vec();
                        out.extend_from_slice(b"\r\n");
                        self.write(&out).await?;
                    }
                }
                Line::Untagged { typ, items } => {
                    if typ == want {
                        data.extend(items);
                    }
                }
                Line::Tagged { tag: t, typ, text } => {
                    if t != tag {
                        continue; // a stale tag: keep reading
                    }
                    if typ == "BAD" {
                        return Err(ImapError::Protocol(format!(
                            "{name} command error: BAD [b'{text}']"
                        )));
                    }
                    return Ok(Resp { typ, text, data });
                }
                Line::Other => {}
            }
        }
    }

    /// `CAPABILITY` → refreshes [`ImapClient::capabilities`] (upper-cased).
    pub async fn capability(&mut self) -> Result<(), ImapError> {
        let r = self.command("CAPABILITY", &[], None, "CAPABILITY").await?;
        if let Some(last) = r.data.last() {
            let txt = String::from_utf8_lossy(last.head()).to_uppercase();
            self.capabilities = txt.split_whitespace().map(str::to_string).collect();
        }
        Ok(())
    }

    /// Is a capability advertised?
    pub fn has(&self, cap: &str) -> bool {
        self.capabilities.iter().any(|c| c == cap)
    }

    /// `LOGIN user password` (quoted). A `NO` is an error with the server's text (`imaplib.IMAP4.error`).
    pub async fn login(&mut self, user: &str, password: &str) -> Result<(), ImapError> {
        let r = self
            .command("LOGIN", &[&quote(user), &quote(password)], None, "LOGIN")
            .await?;
        if !r.ok() {
            return Err(ImapError::Protocol(format!("b'{}'", r.text)));
        }
        Ok(())
    }

    /// `AUTHENTICATE XOAUTH2` with the SASL initial response sent after the server's `+`.
    pub async fn authenticate_xoauth2(&mut self, sasl: &str) -> Result<(), ImapError> {
        let tag = self.next_tag();
        let mut line = tag.clone();
        line.extend_from_slice(b" AUTHENTICATE XOAUTH2\r\n");
        self.write(&line).await?;
        let mut first = true;
        loop {
            match self.read_response().await? {
                Line::Continue => {
                    // the first challenge gets the credentials; a second one is an error blob: finish with an empty line
                    let payload = if first {
                        format!("{}\r\n", B64.encode(sasl.as_bytes()))
                    } else {
                        "\r\n".to_string()
                    };
                    first = false;
                    self.write(payload.as_bytes()).await?;
                }
                Line::Tagged { tag: t, typ, text } if t == tag => {
                    if typ == "OK" {
                        return Ok(());
                    }
                    return Err(ImapError::Protocol(format!("b'{text}'")));
                }
                _ => {}
            }
        }
    }

    /// `STARTTLS` handshake request (the caller then wraps the stream in TLS).
    pub async fn starttls(&mut self) -> Result<(), ImapError> {
        let r = self.command("STARTTLS", &[], None, "STARTTLS").await?;
        if !r.ok() {
            return Err(ImapError::Protocol(format!("STARTTLS refused: {}", r.text)));
        }
        Ok(())
    }

    /// `LIST "" *`.
    pub async fn list(&mut self) -> Result<Resp, ImapError> {
        self.command("LIST", &["\"\"", "*"], None, "LIST").await
    }

    /// `SELECT` / `EXAMINE` (readonly) a folder.
    pub async fn select(&mut self, folder: &str, readonly: bool) -> Result<Resp, ImapError> {
        let name = if readonly { "EXAMINE" } else { "SELECT" };
        self.command(name, &[folder], None, "EXISTS").await
    }

    /// `UID <cmd> args…` with an optional trailing literal; data of `SEARCH` or (for every other command) `FETCH`.
    pub async fn uid(
        &mut self,
        cmd: &str,
        args: &[&str],
        literal: Option<&[u8]>,
    ) -> Result<Resp, ImapError> {
        let cmd = cmd.to_uppercase();
        let mut all: Vec<&str> = vec![cmd.as_str()];
        all.extend_from_slice(args);
        let want = if matches!(cmd.as_str(), "SEARCH" | "SORT" | "THREAD") {
            cmd.clone()
        } else {
            "FETCH".to_string()
        };
        self.command("UID", &all, literal, &want).await
    }

    /// `APPEND folder (flags) "date" {n}`.
    pub async fn append(
        &mut self,
        folder: &str,
        flags: Option<&str>,
        date_time: &str,
        message: &[u8],
    ) -> Result<Resp, ImapError> {
        let flags = flags.map(|f| {
            if f.starts_with('(') && f.ends_with(')') {
                f.to_string()
            } else {
                format!("({f})")
            }
        });
        let mut args = vec![folder];
        if let Some(f) = flags.as_deref() {
            args.push(f);
        }
        if !date_time.is_empty() {
            args.push(date_time);
        }
        self.command("APPEND", &args, Some(message), "APPEND").await
    }

    /// `CREATE name`.
    pub async fn create(&mut self, name: &str) -> Result<Resp, ImapError> {
        self.command("CREATE", &[name], None, "CREATE").await
    }

    /// `ID (…)` (RFC 2971) — NetEase refuses SELECT until the client identifies itself.
    pub async fn id(&mut self, params: &str) -> Result<Resp, ImapError> {
        self.command("ID", &[params], None, "ID").await
    }

    /// `LOGOUT`; errors are ignored.
    pub async fn logout(&mut self) {
        let _ = tokio::time::timeout(
            Duration::from_secs(5),
            self.command("LOGOUT", &[], None, "BYE"),
        )
        .await;
    }
}

/// IMAP quoted string.
pub fn quote(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}

// ------------------------------------------------------------------ connecting

/// A factory for raw streams (tests inject an in-memory pipe; production dials TCP/TLS).
pub type Dialer = Arc<
    dyn Fn(
            String,
            u16,
            String,
        ) -> futures::future::BoxFuture<'static, Result<ImapClient<BoxStream>, ImapError>>
        + Send
        + Sync,
>;

fn tls_connector() -> TlsConnector {
    static CFG: LazyLock<Arc<ClientConfig>> = LazyLock::new(|| {
        let roots = RootCertStore::from_iter(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
        // explicit provider: never depends on which rustls crypto features other crates enable
        let provider = Arc::new(tokio_rustls::rustls::crypto::ring::default_provider());
        let cfg = ClientConfig::builder_with_provider(provider)
            .with_safe_default_protocol_versions()
            .expect("TLS protocol versions")
            .with_root_certificates(roots)
            .with_no_client_auth();
        Arc::new(cfg)
    });
    TlsConnector::from(CFG.clone())
}

async fn tcp(host: &str, port: u16) -> Result<TcpStream, ImapError> {
    let s = tokio::time::timeout(TIMEOUT, TcpStream::connect((host, port)))
        .await
        .map_err(|_| std::io::Error::new(std::io::ErrorKind::TimedOut, "timed out"))??;
    let _ = s.set_nodelay(true);
    Ok(s)
}

async fn wrap_tls(
    host: &str,
    s: TcpStream,
) -> Result<tokio_rustls::client::TlsStream<TcpStream>, ImapError> {
    let name = ServerName::try_from(host.to_string())
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidInput, format!("{e}")))?;
    let tls = tokio::time::timeout(TIMEOUT, tls_connector().connect(name, s))
        .await
        .map_err(|_| std::io::Error::new(std::io::ErrorKind::TimedOut, "timed out"))??;
    Ok(tls)
}

/// Open an IMAP connection: `security` is `"ssl"` (implicit TLS), `"starttls"` or `"none"` (plain).
/// Returns the client with the greeting consumed (not logged in).
pub async fn connect(
    host: &str,
    port: u16,
    security: &str,
) -> Result<ImapClient<BoxStream>, ImapError> {
    let s = tcp(host, port).await?;
    match security {
        "ssl" => {
            let tls = wrap_tls(host, s).await?;
            ImapClient::new(Box::new(tls) as BoxStream).await
        }
        "starttls" => {
            let mut plain = ImapClient::new(s).await?;
            plain.starttls().await?;
            let tls = wrap_tls(host, plain.into_inner()).await?;
            let mut c = ImapClient {
                stream: Box::new(tls) as BoxStream,
                buf: Vec::new(),
                tagn: 0,
                capabilities: Vec::new(),
                last_bye: None,
            };
            c.tagn = 1;
            Ok(c)
        }
        _ => ImapClient::new(Box::new(s) as BoxStream).await,
    }
}

/// The production dialer (TCP + TLS per `security`).
pub fn default_dialer() -> Dialer {
    Arc::new(|host, port, security| Box::pin(async move { connect(&host, port, &security).await }))
}

// ------------------------------------------------------------------ scripted server for tests

/// An in-memory IMAP server driven by a handler closure (the Rust twin of `tests/test_units.py::FakeIMAP`).
#[cfg(test)]
pub mod fake {
    use super::*;
    use tokio::io::DuplexStream;

    /// What the handler answers for one command.
    pub struct Reply {
        /// Raw untagged bytes written before the tagged line (may contain literals).
        pub untagged: Vec<u8>,
        /// `OK` / `NO` / `BAD`.
        pub status: &'static str,
        pub text: String,
    }

    impl Reply {
        pub fn ok(untagged: Vec<u8>) -> Self {
            Reply {
                untagged,
                status: "OK",
                text: "done".into(),
            }
        }
        pub fn no(text: &str) -> Self {
            Reply {
                untagged: vec![],
                status: "NO",
                text: text.into(),
            }
        }
        pub fn bad(text: &str) -> Self {
            Reply {
                untagged: vec![],
                status: "BAD",
                text: text.into(),
            }
        }
    }

    /// `(verb, args, literal)` → reply. `verb` is e.g. `LOGIN`, `LIST`, `EXAMINE`, `UID SEARCH`, `UID FETCH`.
    pub type Handler = Box<dyn FnMut(&str, &str, Option<Vec<u8>>) -> Reply + Send>;

    /// Build an untagged FETCH response with literals: `* n FETCH (head1 {len}\r\n<lit1> head2 {len}\r\n<lit2>)\r\n`.
    pub fn fetch_line(seq: u32, parts: &[(&str, &[u8])]) -> Vec<u8> {
        let mut out = format!("* {seq} FETCH (").into_bytes();
        for (i, (head, lit)) in parts.iter().enumerate() {
            if i > 0 {
                out.push(b' ');
            }
            out.extend_from_slice(head.as_bytes());
            out.extend_from_slice(format!(" {{{}}}\r\n", lit.len()).as_bytes());
            out.extend_from_slice(lit);
        }
        out.extend_from_slice(b")\r\n");
        out
    }

    async fn read_line(s: &mut DuplexStream, buf: &mut Vec<u8>) -> Option<Vec<u8>> {
        loop {
            if let Some(pos) = buf.windows(2).position(|w| w == b"\r\n") {
                let line = buf[..pos].to_vec();
                buf.drain(..pos + 2);
                return Some(line);
            }
            let mut tmp = [0u8; 4096];
            let n = s.read(&mut tmp).await.ok()?;
            if n == 0 {
                return None;
            }
            buf.extend_from_slice(&tmp[..n]);
        }
    }

    async fn read_exact(s: &mut DuplexStream, buf: &mut Vec<u8>, n: usize) -> Option<Vec<u8>> {
        while buf.len() < n {
            let mut tmp = [0u8; 4096];
            let k = s.read(&mut tmp).await.ok()?;
            if k == 0 {
                return None;
            }
            buf.extend_from_slice(&tmp[..k]);
        }
        let out = buf[..n].to_vec();
        buf.drain(..n);
        Some(out)
    }

    /// Spawn the server task; returns the client end of the pipe.
    pub fn serve(mut handler: Handler) -> DuplexStream {
        let (client, mut server) = tokio::io::duplex(1 << 20);
        tokio::spawn(async move {
            let mut buf = Vec::new();
            if server.write_all(b"* OK fake ready\r\n").await.is_err() {
                return;
            }
            while let Some(line) = read_line(&mut server, &mut buf).await {
                let txt = String::from_utf8_lossy(&line).to_string();
                let Some((tag, rest)) = txt.split_once(' ') else {
                    continue;
                };
                let mut rest = rest.to_string();
                let mut literal: Option<Vec<u8>> = None;
                if let Some(m) = LITERAL_TAIL.captures(rest.as_bytes()) {
                    let n: usize = String::from_utf8_lossy(&m[1]).parse().unwrap_or(0);
                    let cut = m.get(0).map(|x| x.start()).unwrap_or(rest.len());
                    rest = rest[..cut].trim_end().to_string();
                    let _ = server.write_all(b"+ go ahead\r\n").await;
                    let Some(lit) = read_exact(&mut server, &mut buf, n).await else {
                        return;
                    };
                    let _ = read_line(&mut server, &mut buf).await; // the CRLF after the literal
                    literal = Some(lit);
                }
                let upper = rest.to_uppercase();
                let (verb, args) = if upper.starts_with("UID ") {
                    let after = &rest[4..];
                    let (v, a) = after.split_once(' ').unwrap_or((after, ""));
                    (format!("UID {}", v.to_uppercase()), a.to_string())
                } else {
                    let (v, a) = rest.split_once(' ').unwrap_or((&rest, ""));
                    (v.to_uppercase(), a.to_string())
                };
                if verb == "AUTHENTICATE" {
                    let _ = server.write_all(b"+ \r\n").await;
                    let Some(resp) = read_line(&mut server, &mut buf).await else {
                        return;
                    };
                    let decoded = B64.decode(&resp).unwrap_or_default();
                    let r = handler("AUTHENTICATE", &args, Some(decoded));
                    let _ = server.write_all(&r.untagged).await;
                    let _ = server
                        .write_all(format!("{tag} {} {}\r\n", r.status, r.text).as_bytes())
                        .await;
                    continue;
                }
                if verb == "LOGOUT" {
                    let _ = server
                        .write_all(format!("* BYE\r\n{tag} OK bye\r\n").as_bytes())
                        .await;
                    return;
                }
                let r = handler(&verb, &args, literal);
                if server.write_all(&r.untagged).await.is_err() {
                    return;
                }
                if server
                    .write_all(format!("{tag} {} {}\r\n", r.status, r.text).as_bytes())
                    .await
                    .is_err()
                {
                    return;
                }
            }
        });
        client
    }

    /// A dialer that serves a fresh scripted connection per call.
    pub fn dialer<F>(make: F) -> Dialer
    where
        F: Fn() -> Handler + Send + Sync + 'static,
    {
        let make = Arc::new(make);
        Arc::new(move |_h, _p, _s| {
            let make = make.clone();
            Box::pin(async move {
                let stream = serve(make());
                ImapClient::new(Box::new(stream) as BoxStream).await
            })
        })
    }
}

#[cfg(test)]
mod tests {
    use super::fake::{fetch_line, serve, Handler, Reply};
    use super::*;

    type Log = Arc<parking_lot::Mutex<Vec<(String, String, Option<Vec<u8>>)>>>;

    fn handler(log: Log) -> Handler {
        Box::new(move |verb, args, lit| {
            log.lock()
                .push((verb.to_string(), args.to_string(), lit.clone()));
            match verb {
                "LOGIN" => Reply::ok(vec![]),
                "CAPABILITY" => Reply::ok(b"* CAPABILITY IMAP4rev1 X-GM-EXT-1 MOVE UIDPLUS\r\n".to_vec()),
                "LIST" => Reply::ok(
                    b"* LIST (\\HasNoChildren) \"/\" \"INBOX\"\r\n* LIST (\\HasNoChildren) \"/\" {5}\r\nSales\r\n".to_vec(),
                ),
                "EXAMINE" => Reply::ok(b"* 3 EXISTS\r\n* 0 RECENT\r\n".to_vec()),
                "UID SEARCH" => {
                    if args.contains("CHARSET") {
                        Reply::bad("BADCHARSET")
                    } else {
                        Reply::ok(b"* SEARCH 101 102\r\n".to_vec())
                    }
                }
                "UID FETCH" => {
                    let mut out = fetch_line(
                        1,
                        &[("UID 101 FLAGS () BODY[HEADER.FIELDS (FROM)]", b"From: a@b.c\r\n\r\n"), ("BODY[TEXT]<0>", b"hi")],
                    );
                    out.extend(fetch_line(2, &[("UID 102 FLAGS (\\Seen) BODY[HEADER.FIELDS (FROM)]", b"From: x@y.z\r\n\r\n")]));
                    Reply::ok(out)
                }
                "APPEND" => Reply::ok(vec![]),
                "UID STORE" => Reply::ok(b"* 1 FETCH (FLAGS (\\Seen))\r\n".to_vec()),
                _ => Reply::no("nope"),
            }
        })
    }

    #[tokio::test]
    async fn protocol_roundtrip() {
        let log = Arc::new(parking_lot::Mutex::new(Vec::new()));
        let stream = serve(handler(log.clone()));
        let mut c = ImapClient::new(Box::new(stream) as BoxStream)
            .await
            .unwrap();
        c.login("me", "p\"w").await.unwrap();
        assert_eq!(log.lock()[0].1, "\"me\" \"p\\\"w\"");
        c.capability().await.unwrap();
        assert!(c.has("X-GM-EXT-1") && c.has("MOVE"));
        let l = c.list().await.unwrap();
        assert_eq!(l.data.len(), 3);
        assert_eq!(
            l.data[0],
            Item::Line(b"(\\HasNoChildren) \"/\" \"INBOX\"".to_vec())
        );
        assert_eq!(
            l.data[1],
            Item::Literal {
                head: b"(\\HasNoChildren) \"/\" {5}".to_vec(),
                data: b"Sales".to_vec()
            }
        );
        assert_eq!(l.data[2], Item::Line(b"".to_vec()));
        assert!(c.select("INBOX", true).await.unwrap().ok());
        let s = c.uid("SEARCH", &["ALL"], None).await.unwrap();
        assert_eq!(s.numbers(), vec!["101", "102"]);
        // a literal argument: command line ends in {n}, literal sent after "+"
        let err = c
            .uid(
                "SEARCH",
                &["CHARSET", "UTF-8", "TEXT"],
                Some("会议".as_bytes()),
            )
            .await
            .unwrap_err();
        assert!(
            matches!(err, ImapError::Protocol(ref m) if m.contains("BADCHARSET")),
            "{err}"
        );
        assert_eq!(
            log.lock().last().unwrap().2.as_deref(),
            Some("会议".as_bytes())
        );
        let f = c
            .uid("FETCH", &["101,102", "(UID FLAGS)"], None)
            .await
            .unwrap();
        assert_eq!(f.data.len(), 5);
        assert!(f.data[0].head().starts_with(b"1 (UID 101"));
        assert_eq!(f.data[0].literal(), Some(&b"From: a@b.c\r\n\r\n"[..]));
        assert_eq!(f.data[1].head(), b" BODY[TEXT]<0> {2}");
        assert_eq!(f.data[1].literal(), Some(&b"hi"[..]));
        assert_eq!(f.data[2], Item::Line(b")".to_vec()));
        assert!(f.data[3].head().starts_with(b"2 (UID 102"));
        let a = c
            .append(
                "\"Drafts\"",
                Some("\\Draft \\Seen"),
                "\"1-Jan-2026 00:00:00 +0000\"",
                b"Subject: x\r\n\r\nbody",
            )
            .await
            .unwrap();
        assert!(a.ok());
        let (verb, args, lit) = log.lock().last().cloned().unwrap();
        assert_eq!(
            (verb.as_str(), args.as_str()),
            (
                "APPEND",
                "\"Drafts\" (\\Draft \\Seen) \"1-Jan-2026 00:00:00 +0000\""
            )
        );
        assert_eq!(lit.as_deref(), Some(&b"Subject: x\r\n\r\nbody"[..]));
        let no = c.uid("COPY", &["1", "X"], None).await.unwrap();
        assert_eq!(no.typ, "NO");
        assert_eq!(no.py_data(), "[b'nope']");
        c.logout().await;
    }

    /// Live check of the TLS paths (needs network): `cargo test -p omuse-mail -- --ignored`.
    #[tokio::test]
    #[ignore]
    async fn live_tls_greeting() {
        let mut c = connect("imap.gmail.com", 993, "ssl").await.unwrap();
        c.capability().await.unwrap();
        assert!(c.has("IMAP4REV1"));
        c.logout().await;
        let e = crate::smtp::send(
            &crate::smtp::SmtpSettings {
                host: "smtp.gmail.com".into(),
                port: 587,
                security: "starttls".into(),
                username: "nobody@gmail.com".into(),
                password: "wrong".into(),
            },
            None,
            "nobody@gmail.com",
            &["nobody@gmail.com".into()],
            b"Subject: x\r\n\r\nx\r\n",
        )
        .await
        .unwrap_err();
        assert!(matches!(e, crate::smtp::SmtpFail::Auth(_)), "{e:?}");
    }

    #[tokio::test]
    async fn xoauth2_exchange() {
        let seen = Arc::new(parking_lot::Mutex::new(Vec::new()));
        let s2 = seen.clone();
        let stream = serve(Box::new(move |verb, _a, lit| {
            if verb == "AUTHENTICATE" {
                s2.lock().push(lit.unwrap_or_default());
                return Reply::ok(vec![]);
            }
            Reply::no("x")
        }));
        let mut c = ImapClient::new(Box::new(stream) as BoxStream)
            .await
            .unwrap();
        c.authenticate_xoauth2("user=me@outlook.com\x01auth=Bearer at-1\x01\x01")
            .await
            .unwrap();
        assert_eq!(
            seen.lock()[0],
            b"user=me@outlook.com\x01auth=Bearer at-1\x01\x01"
        );
    }
}
