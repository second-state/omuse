//! SMTP submission (`Gmail._smtp` + `send_message` in the Python), on lettre.
//!
//! Security `"ssl"` = implicit TLS (465), `"starttls"` = upgrade required (587), `"none"` = plain (loopback only).
//! Authentication: XOAUTH2 when a token is given (lettre's mechanism sends exactly
//! `user=…\x01auth=Bearer …\x01\x01`), otherwise PLAIN / LOGIN when a password is set or the link is encrypted
//! (Python: `elif self.password or self.smtp_security != "none": s.login(...)`).

use std::time::Duration;

use lettre::address::Envelope;
use lettre::transport::smtp::authentication::{Credentials, Mechanism};
use lettre::transport::smtp::client::{Tls, TlsParameters};
use lettre::{Address, AsyncSmtpTransport, AsyncTransport, Tokio1Executor};

/// Where and how to submit.
#[derive(Debug, Clone)]
pub struct SmtpSettings {
    pub host: String,
    pub port: u16,
    pub security: String,
    pub username: String,
    pub password: String,
}

/// Why a submission failed (`SMTPAuthenticationError` vs. every other `SMTPException` / `OSError`).
#[derive(Debug, Clone, thiserror::Error, PartialEq, Eq)]
pub enum SmtpFail {
    /// Login refused: text like Python's `str(SMTPAuthenticationError)`, `(535, b'…')`.
    #[error("{0}")]
    Auth(String),
    /// Anything else (connection, TLS, rejected recipient…).
    #[error("{0}")]
    Other(String),
}

const AUTH_CODES: &[&str] = &["530", "534", "535", "538", "454"];

fn classify(e: &lettre::transport::smtp::Error) -> SmtpFail {
    let text = e.to_string();
    if let Some(code) = e.status() {
        let code = code.to_string();
        if AUTH_CODES.contains(&code.as_str()) {
            let msg = text
                .split_once("): ")
                .map(|(_, m)| m)
                .unwrap_or(&text)
                .trim()
                .to_string();
            return SmtpFail::Auth(format!("({code}, b'{}')", msg.replace('\'', "\\'")));
        }
    }
    SmtpFail::Other(text)
}

/// Parse envelope addresses.
fn addr(a: &str) -> Result<Address, SmtpFail> {
    a.trim()
        .parse::<Address>()
        .map_err(|e| SmtpFail::Other(format!("invalid address {a}: {e}")))
}

/// Send one already-composed RFC 5322 message. `token` = XOAUTH2 access token (Outlook).
pub async fn send(
    s: &SmtpSettings,
    token: Option<&str>,
    from: &str,
    rcpts: &[String],
    message: &[u8],
) -> Result<(), SmtpFail> {
    let tls_params = || {
        TlsParameters::new_rustls(s.host.clone()).map_err(|e| SmtpFail::Other(format!("TLS: {e}")))
    };
    let tls = match s.security.as_str() {
        "ssl" => Tls::Wrapper(tls_params()?),
        "starttls" => Tls::Required(tls_params()?),
        _ => Tls::None,
    };
    let mut b = AsyncSmtpTransport::<Tokio1Executor>::builder_dangerous(s.host.clone())
        .port(s.port)
        .tls(tls)
        .timeout(Some(Duration::from_secs(30)));
    if let Some(tok) = token {
        b = b
            .credentials(Credentials::new(s.username.clone(), tok.to_string()))
            .authentication(vec![Mechanism::Xoauth2]);
    } else if !s.password.is_empty() || s.security != "none" {
        b = b
            .credentials(Credentials::new(s.username.clone(), s.password.clone()))
            .authentication(vec![Mechanism::Plain, Mechanism::Login]);
    }
    let transport = b.build();
    let to: Vec<Address> = rcpts.iter().map(|r| addr(r)).collect::<Result<_, _>>()?;
    let envelope =
        Envelope::new(Some(addr(from)?), to).map_err(|e| SmtpFail::Other(e.to_string()))?;
    transport
        .send_raw(&envelope, message)
        .await
        .map(|_| ())
        .map_err(|e| classify(&e))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use base64::engine::general_purpose::STANDARD as B64;
    use base64::Engine;
    use std::sync::Arc;
    use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};

    /// A tiny SMTP sink like the e2e test's aiosmtpd: AUTH PLAIN/LOGIN for one user, XOAUTH2 for tokens "at-*".
    pub(crate) async fn sink() -> (u16, Arc<parking_lot::Mutex<Vec<String>>>) {
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = l.local_addr().unwrap().port();
        let log = Arc::new(parking_lot::Mutex::new(Vec::new()));
        let log2 = log.clone();
        tokio::spawn(async move {
            while let Ok((sock, _)) = l.accept().await {
                let log = log2.clone();
                tokio::spawn(async move {
                    let (r, mut w) = sock.into_split();
                    let mut r = BufReader::new(r);
                    w.write_all(b"220 sink\r\n").await.unwrap();
                    let mut line = String::new();
                    loop {
                        line.clear();
                        if r.read_line(&mut line).await.unwrap_or(0) == 0 {
                            return;
                        }
                        let cmd = line.trim_end().to_string();
                        let up = cmd.to_uppercase();
                        let reply: String = if up.starts_with("EHLO") {
                            "250-sink\r\n250 AUTH PLAIN LOGIN XOAUTH2\r\n".into()
                        } else if let Some(rest) = cmd.strip_prefix("AUTH XOAUTH2 ") {
                            let raw = String::from_utf8(B64.decode(rest).unwrap()).unwrap();
                            let ok = raw.starts_with("user=me@outlook.com\x01auth=Bearer at-")
                                && raw.ends_with("\x01\x01");
                            log.lock().push(format!("xoauth:{raw}"));
                            if ok {
                                "235 ok\r\n".into()
                            } else {
                                "535 5.7.8 bad token\r\n".into()
                            }
                        } else if let Some(rest) = cmd.strip_prefix("AUTH PLAIN ") {
                            let raw = String::from_utf8(B64.decode(rest).unwrap()).unwrap();
                            if raw == "\0tester@example.com\0secretpass123" {
                                "235 ok\r\n".into()
                            } else {
                                "535 5.7.8 Authentication failed\r\n".into()
                            }
                        } else if up.starts_with("MAIL FROM") || up.starts_with("RCPT TO") {
                            log.lock().push(cmd.clone());
                            "250 ok\r\n".into()
                        } else if up == "DATA" {
                            w.write_all(b"354 go\r\n").await.unwrap();
                            let mut data = Vec::new();
                            loop {
                                let mut b = [0u8; 1];
                                r.read_exact(&mut b).await.unwrap();
                                data.push(b[0]);
                                if data.ends_with(b"\r\n.\r\n") {
                                    break;
                                }
                            }
                            log.lock()
                                .push(format!("data:{}", String::from_utf8_lossy(&data)));
                            "250 OK\r\n".into()
                        } else if up == "QUIT" {
                            let _ = w.write_all(b"221 bye\r\n").await;
                            return;
                        } else {
                            "250 ok\r\n".into()
                        };
                        w.write_all(reply.as_bytes()).await.unwrap();
                    }
                });
            }
        });
        (port, log)
    }

    fn settings(port: u16, user: &str, pw: &str) -> SmtpSettings {
        SmtpSettings {
            host: "127.0.0.1".into(),
            port,
            security: "none".into(),
            username: user.into(),
            password: pw.into(),
        }
    }

    #[tokio::test]
    async fn login_xoauth2_and_failures() {
        let (port, log) = sink().await;
        let msg = b"From: tester@example.com\r\nTo: boss@acme.com\r\nSubject: Hi\r\n\r\nBody\r\n";
        send(
            &settings(port, "tester@example.com", "secretpass123"),
            None,
            "tester@example.com",
            &["boss@acme.com".into()],
            msg,
        )
        .await
        .unwrap();
        assert!(log.lock().iter().any(|l| l == "RCPT TO:<boss@acme.com>"));
        assert!(log
            .lock()
            .iter()
            .any(|l| l.starts_with("data:") && l.contains("Subject: Hi")));
        let e = send(
            &settings(port, "tester@example.com", "wrong"),
            None,
            "tester@example.com",
            &["a@b.com".into()],
            msg,
        )
        .await
        .unwrap_err();
        assert!(
            matches!(e, SmtpFail::Auth(ref m) if m.starts_with("(535, b'") && m.contains("Authentication failed")),
            "{e:?}"
        );
        send(
            &settings(port, "me@outlook.com", ""),
            Some("at-7"),
            "me@outlook.com",
            &["friend@example.com".into()],
            msg,
        )
        .await
        .unwrap();
        assert!(log
            .lock()
            .iter()
            .any(|l| l == "xoauth:user=me@outlook.com\x01auth=Bearer at-7\x01\x01"));
        let e = send(
            &settings(port, "me@outlook.com", ""),
            Some("nope"),
            "me@outlook.com",
            &["f@e.com".into()],
            msg,
        )
        .await
        .unwrap_err();
        assert!(matches!(e, SmtpFail::Auth(_)), "{e:?}");
        let e = send(
            &settings(1, "x", "y"),
            None,
            "x@y.com",
            &["a@b.com".into()],
            msg,
        )
        .await
        .unwrap_err();
        assert!(matches!(e, SmtpFail::Other(_)));
    }
}
