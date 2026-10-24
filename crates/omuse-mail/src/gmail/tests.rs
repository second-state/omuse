//! Port of the "gmail parsing (fake IMAP)", "unsubscribe" and "other email providers" sections of
//! `tests/test_units.py`, plus generic-mode flows of `tests/mail_generic_e2e.py` against a scripted server.

use std::sync::Arc;

use base64::Engine;
use parking_lot::Mutex;
use serde_json::json;

use super::*;
use crate::imap::fake::{dialer, fetch_line, Handler, Reply};

type Log = Arc<Mutex<Vec<(String, String, Option<Vec<u8>>)>>>;

const HDR1: &[u8] = b"From: John <john@acme.com>\r\nTo: lucas@x.com\r\nSubject: Meeting Tuesday?\r\nDate: Mon, 21 Sep 2026 10:00:00 +0800\r\nMessage-ID: <m1@acme>\r\nList-Unsubscribe: <mailto:u@list.acme.com?subject=unsub>,\r\n <https://acme.com/unsub/AbC123xyz>\r\nList-Unsubscribe-Post: List-Unsubscribe=One-Click\r\n\r\n";
const HDR2: &[u8] =
    b"From: Bank <no-reply@bank.com>\r\nSubject: Your verification code\r\nDate: Tue, 22 Sep 2026 10:00:00 +0800\r\n\r\n";

fn meta_fetch(gm: bool) -> Vec<u8> {
    let (g1, g2) = if gm {
        (
            r#"X-GM-MSGID 1790000000000001 X-GM-THRID 1790000000000001 X-GM-LABELS ("\\Inbox" "\\Important") "#,
            "X-GM-MSGID 1790000000000002 X-GM-THRID 1790000000000002 X-GM-LABELS () ",
        )
    } else {
        ("", "")
    };
    let mut out = fetch_line(
        1,
        &[
            (
                &format!(
                    "UID 101 {g1}FLAGS () BODY[HEADER.FIELDS (FROM TO CC SUBJECT DATE MESSAGE-ID)]"
                ),
                HDR1,
            ),
            ("BODY[TEXT]<0>", b"Hi Lucas, does Tuesday 3pm work for you?"),
        ],
    );
    out.extend(fetch_line(
        2,
        &[
            (&format!("UID 102 {g2}FLAGS (\\Seen) BODY[HEADER.FIELDS (FROM TO CC SUBJECT DATE MESSAGE-ID)]"), HDR2),
            ("BODY[TEXT]<0>", b"Your code is 123456"),
        ],
    ));
    out
}

/// `FakeIMAP` (Gmail) / `FakeIMAPNoUTF8` (generic, rejects SEARCH CHARSET UTF-8).
fn fake(log: Log, gm: bool) -> Handler {
    Box::new(move |verb, args, lit| {
        log.lock()
            .push((verb.to_string(), args.to_string(), lit.clone()));
        match verb {
            "LOGIN" => Reply::ok(vec![]),
            "CAPABILITY" => Reply::ok(if gm {
                b"* CAPABILITY IMAP4rev1 X-GM-EXT-1 UIDPLUS MOVE\r\n".to_vec()
            } else {
                b"* CAPABILITY IMAP4rev1 UIDPLUS\r\n".to_vec()
            }),
            "LIST" => Reply::ok(if gm {
                b"* LIST (\\HasNoChildren) \"/\" \"INBOX\"\r\n* LIST (\\All \\HasNoChildren) \"/\" \"[Gmail]/All Mail\"\r\n* LIST (\\Drafts \\HasNoChildren) \"/\" \"[Gmail]/Drafts\"\r\n".to_vec()
            } else {
                b"* LIST (\\HasNoChildren) \"/\" \"INBOX\"\r\n* LIST (\\HasNoChildren) \"/\" \"&g0l6P3ux-\"\r\n* LIST (\\HasNoChildren) \"/\" \"Sent Messages\"\r\n".to_vec()
            }),
            "SELECT" | "EXAMINE" => Reply::ok(b"* 2 EXISTS\r\n".to_vec()),
            "UID SEARCH" => {
                if !gm && args.contains("CHARSET") {
                    Reply::bad("[BADCHARSET] not supported")
                } else {
                    Reply::ok(b"* SEARCH 101 102\r\n".to_vec())
                }
            }
            "UID FETCH" if args.contains("HEADER.FIELDS") => Reply::ok(meta_fetch(gm)),
            _ => Reply::ok(vec![]),
        }
    })
}

fn gmail_client(log: &Log) -> Gmail {
    let log = log.clone();
    Gmail::with_gmail_defaults("lucas@x.com", "abcd abcd abcd abcd")
        .with_dialer(dialer(move || fake(log.clone(), true)))
}

fn qq_client(log: &Log) -> Gmail {
    let log = log.clone();
    Gmail::new(
        "me@qq.com",
        "x",
        "imap.qq.com",
        "smtp.qq.com",
        "",
        GmailOptions {
            provider: "qq".into(),
            ..Default::default()
        },
    )
    .with_dialer(dialer(move || fake(log.clone(), false)))
}

fn new_log() -> Log {
    Arc::new(Mutex::new(Vec::new()))
}

#[tokio::test]
async fn gmail_search_parsing() {
    let log = new_log();
    let mut g = gmail_client(&log);
    assert_eq!(g.password, "abcdabcdabcdabcd"); // app password spaces removed
    let res = g.search("in:inbox newer_than:7d", 10).await.unwrap();
    assert_eq!(res.len(), 2);
    let (bank, john) = (&res[0], &res[1]); // sorted by date desc
    assert_eq!(john["id"], "1790000000000001");
    assert_eq!(john["thread_id"], "1790000000000001");
    assert_eq!(john["subject"], "Meeting Tuesday?");
    assert_eq!(john["unread"], true);
    assert!(vs(john, "snippet").contains("Tuesday 3pm"));
    assert!(john["labels"]
        .as_array()
        .unwrap()
        .contains(&json!("\\Important")));
    assert_eq!(bank["security_message"], true);
    assert!(!vs(bank, "snippet").contains("123456"));
    assert_eq!(john["unsubscribe"], "one-click");
    assert!(john.get("_unsub").is_none() && bank.get("unsubscribe").is_none());
    assert!(!serde_json::to_string(&res).unwrap().contains("AbC123xyz")); // unsubscribe URLs never reach the agent
    let cmds = log.lock().clone();
    assert!(cmds
        .iter()
        .any(|(v, a, _)| v == "EXAMINE" && a == "\"[Gmail]/All Mail\""));
    assert!(cmds
        .iter()
        .any(|(v, a, _)| v == "UID SEARCH" && a == "X-GM-RAW \"in:inbox newer_than:7d\""));
    // non-ascii query uses a literal
    let log2 = new_log();
    let mut g2 = gmail_client(&log2);
    g2.search("subject:会议", 5).await.unwrap();
    let lit = log2
        .lock()
        .iter()
        .find(|(v, _, _)| v == "UID SEARCH")
        .and_then(|x| x.2.clone());
    assert_eq!(lit.as_deref(), Some("subject:会议".as_bytes()));
    assert!(log2
        .lock()
        .iter()
        .any(|(v, a, _)| v == "UID SEARCH" && a == "CHARSET UTF-8 X-GM-RAW"));
}

#[tokio::test]
async fn generic_search_utf8_fallback() {
    let log = new_log();
    let mut g = qq_client(&log);
    let res = g.search("in:inbox Tuesday", 10).await.unwrap(); // ASCII: server-side TEXT search
    let ids: std::collections::HashSet<String> = res.iter().map(|r| vs(r, "id")).collect();
    assert_eq!(
        ids,
        ["i-101", "i-102"].iter().map(|s| s.to_string()).collect()
    );
    assert!(log
        .lock()
        .iter()
        .any(|(v, a, _)| v == "UID SEARCH" && a == "TEXT \"Tuesday\""));
    assert_eq!(res[1]["labels"], json!(["INBOX"]));
    let res = g.search("会议", 10).await.unwrap(); // server refuses UTF-8 -> local filter over newest mail
    assert!(res.is_empty());
    let res = g.search("verification", 10).await.unwrap();
    assert_eq!(res.len(), 2);
    let f = g.cached_folders().unwrap();
    assert_eq!(f.get("drafts"), Some("\"&g0l6P3ux-\""));
    assert_eq!(f.get("sent"), Some("\"Sent Messages\""));
    assert_eq!(f.display["\"&g0l6P3ux-\""], "草稿箱");
    assert_eq!(
        g.list_labels().await.unwrap(),
        vec!["INBOX", "草稿箱", "Sent Messages"]
    );
    let e = g.search("label:Nope", 5).await.unwrap_err();
    assert!(
        e.0.contains("Sent Messages") && e.0.starts_with("找不到文件夹 folder not found: Nope"),
        "{e}"
    );
    let e = g.search("in:trash", 5).await.unwrap_err();
    assert_eq!(e.0, "这个邮箱没有 trash 文件夹 (no trash folder)");
}

fn b64_subject(s: &str) -> String {
    format!(
        "=?utf-8?b?{}?=",
        base64::engine::general_purpose::STANDARD.encode(s.as_bytes())
    )
}

#[tokio::test]
async fn generic_search_or_of_chinese_terms() {
    // The colleague's exact query: an OR of Chinese phrases must return a mail matching ANY one of them.
    let make = || -> Handler {
        Box::new(|verb, args, _lit| match verb {
            "CAPABILITY" => Reply::ok(b"* CAPABILITY IMAP4rev1\r\n".to_vec()),
            "LIST" => Reply::ok(b"* LIST (\\HasNoChildren) \"/\" \"INBOX\"\r\n".to_vec()),
            "UID SEARCH" if args.contains("CHARSET") => Reply::bad("BADCHARSET"),
            "UID SEARCH" => Reply::ok(b"* SEARCH 201 202\r\n".to_vec()),
            "UID FETCH" => {
                let hdr = |subj: &str, frm: &str| {
                    format!(
                        "From: {frm}\r\nTo: me@qq.com\r\nSubject: {}\r\nDate: Mon, 05 Oct 2026 10:00:00 +0800\r\nMessage-ID: <x>\r\n\r\n",
                        b64_subject(subj)
                    )
                    .into_bytes()
                };
                let h1 = hdr("笔试通知：下周二在线笔试", "hr@company.com");
                let h2 = hdr("本周周报汇总", "boss@company.com");
                let mut out = fetch_line(
                    1,
                    &[
                        ("UID 201 FLAGS () BODY[HEADER.FIELDS (FROM)]", &h1),
                        ("BODY[TEXT]<0>", "请准时参加".as_bytes()),
                    ],
                );
                out.extend(fetch_line(
                    2,
                    &[
                        ("UID 202 FLAGS (\\Seen) BODY[HEADER.FIELDS (FROM)]", &h2),
                        ("BODY[TEXT]<0>", "请填写".as_bytes()),
                    ],
                ));
                Reply::ok(out)
            }
            _ => Reply::ok(vec![]),
        })
    };
    let mut g = Gmail::new(
        "me@qq.com",
        "x",
        "imap.qq.com",
        "smtp.qq.com",
        "",
        GmailOptions {
            provider: "qq".into(),
            ..Default::default()
        },
    )
    .with_dialer(dialer(make));
    let res = g
        .search(
            "newer_than:30d (\"笔试\" OR \"笔试通知\" OR \"在线测评\" OR \"测评链接\")",
            10,
        )
        .await
        .unwrap();
    let subs: Vec<String> = res.iter().map(|r| vs(r, "subject")).collect();
    assert!(subs.iter().any(|s| s.contains("笔试")), "{subs:?}");
    assert!(!subs.iter().any(|s| s.contains("周报")), "{subs:?}");
}

#[test]
fn body_text_html_and_attachments() {
    let raw = b"MIME-Version: 1.0\r\nContent-Type: multipart/mixed; boundary=XX\r\n\r\n--XX\r\nContent-Type: text/html; charset=utf-8\r\n\r\n<html><body><p>Hello <b>Lucas</b></p><script>evil()</script></body></html>\r\n--XX\r\nContent-Type: application/pdf\r\nContent-Disposition: attachment; filename=\"a.pdf\"\r\nContent-Transfer-Encoding: base64\r\n\r\nJVBERi0=\r\n--XX--\r\n";
    let (body, att) = body_text(raw);
    assert!(
        body.contains("Hello") && body.contains("Lucas") && !body.contains("evil"),
        "{body}"
    );
    assert_eq!(att[0]["filename"], "a.pdf");
    assert_eq!(att[0]["type"], "application/pdf");
    assert_eq!(att[0]["size"], 5); // "%PDF-"
    let (name, ctype, data) = pick_attachment(raw, "A.PDF").unwrap();
    assert_eq!(
        (name.as_str(), ctype.as_str(), data.as_slice()),
        ("a.pdf", "application/pdf", &b"%PDF-"[..])
    );
    let e = pick_attachment(raw, "x.doc").unwrap_err();
    assert_eq!(
        e.0,
        "找不到附件 attachment not found: x.doc. Attachments: a.pdf"
    );
    // plain text wins; quoted lines dropped; 8-bit UTF-8 decoded
    let raw2 = "Content-Type: text/plain; charset=utf-8\r\n\r\n请在月底前提交发票报销单。\r\n> old quote\r\nbye\r\n".as_bytes();
    assert_eq!(body_text(raw2).0, "请在月底前提交发票报销单。\nbye");
    // quoted-printable + gb2312
    let raw3 = b"Content-Type: text/plain; charset=gb2312\r\nContent-Transfer-Encoding: quoted-printable\r\n\r\n=D6=D0=B9=FA=\r\nx\r\n";
    assert_eq!(body_text(raw3).0, "中国x");
}

#[test]
fn html_and_snippet_helpers() {
    assert_eq!(
        html_to_text("<head><title>t</title></head><p>a&amp;b</p><div>c</div>"),
        "a&b\n c"
    );
    assert_eq!(
        snippet(b"Hello=\r\n world\r\n--boundary1234567890 junk\nContent-Type: text/plain\nok"),
        "Hello world ok"
    );
    assert_eq!(splitlines("a\r\nb\rc\n"), vec!["a", "b", "c"]);
}

#[test]
fn compose_reply_headers_and_attachments() {
    let g = Gmail::new(
        "lucas@x.com",
        "abcdabcdabcdabcd",
        "imap.gmail.com",
        "smtp.gmail.com",
        "Lucas Lu",
        GmailOptions::default(),
    );
    let orig = json!({"subject": "Meeting", "message_id_header": "<m1@acme>", "references": "<a@x>\r\n <b@x>"});
    let m = g
        .compose("john@acme.com", "", "OK", "", Some(&orig), &[])
        .unwrap();
    assert_eq!(m.subject, "Re: Meeting");
    assert_eq!(m.in_reply_to, "<m1@acme>");
    assert_eq!(m.references, "<a@x> <b@x> <m1@acme>");
    assert!(m.from.contains("Lucas Lu"));
    let txt = String::from_utf8_lossy(&m.bytes).to_string();
    assert!(
        txt.contains("In-Reply-To: <m1@acme>\r\n")
            && txt.contains("From: Lucas Lu <lucas@x.com>\r\n"),
        "{txt}"
    );
    assert!(txt.contains("Message-ID: <") && txt.contains("@x.com>") && txt.contains("\r\nDate: "));
    let again = g
        .compose(
            "john@acme.com",
            "",
            "OK",
            "",
            Some(&json!({"subject": "RE: x"})),
            &[],
        )
        .unwrap();
    assert_eq!(again.subject, "RE: x");
    assert_eq!(
        g.compose("a@b.c", "", "x", "", None, &[]).unwrap().subject,
        "(no subject)"
    );
    // attachments (test_gmail_attachments_in_message)
    let g = Gmail::new(
        "me@example.com",
        "pw",
        "i",
        "s",
        "",
        GmailOptions::default(),
    );
    let m = g
        .compose(
            "you@example.com",
            "Slip",
            "attached",
            "",
            None,
            &[(
                "slip.pdf".into(),
                "application/pdf".into(),
                b"%PDF-1.4 x".to_vec(),
            )],
        )
        .unwrap();
    let (name, ctype, data) = pick_attachment(&m.bytes, "").unwrap();
    assert_eq!(
        (name.as_str(), ctype.as_str(), data.as_slice()),
        ("slip.pdf", "application/pdf", &b"%PDF-1.4 x"[..])
    );
    let (body, att) = body_text(&m.bytes);
    assert_eq!(body, "attached");
    assert_eq!(att.len(), 1);
    // non-ASCII subject is RFC 2047 encoded and decodes back
    let m = g
        .compose("you@example.com", "发票报销", "x", "", None, &[])
        .unwrap();
    let h = mime::parse_headers(&m.bytes);
    assert_eq!(dec(h.get("Subject")), "发票报销");
}

#[test]
fn reply_defaults_all() {
    let orig = json!({"from": "Boss <boss@acme.com>", "to": "me@x.com, Ann <ann@acme.com>", "cc": "\"Lu, L\" <l@x.com>", "subject": "Plan"});
    let d = reply_defaults_from(&orig, "ME@x.com", true);
    assert_eq!(
        d,
        json!({"to": "Boss <boss@acme.com>", "cc": "Ann <ann@acme.com>, \"Lu, L\" <l@x.com>", "subject": "Re: Plan"})
    );
    assert_eq!(reply_defaults_from(&orig, "me@x.com", false)["cc"], "");
}

#[test]
fn list_unsubscribe_parsing() {
    let p = parse_list_unsubscribe;
    assert_eq!(
        p("<https://x.com/u/1>", "List-Unsubscribe=One-Click")["method"],
        "one-click"
    );
    assert_eq!(
        p("<mailto:a@b.com>, <https://x.com/u/1>", "")["method"],
        "email"
    );
    assert_eq!(p("<https://x.com/u/1>", "")["method"], "link");
    assert_eq!(p("<http://x.com/u/1>", "")["method"], ""); // plain http not used
    assert_eq!(p("", "")["method"], "");
    let full = p(
        "<mailto:u@list.acme.com?subject=unsub>,\r\n <https://acme.com/unsub/AbC123xyz>",
        "List-Unsubscribe=One-Click",
    );
    assert_eq!(
        full,
        json!({"mailto": "mailto:u@list.acme.com?subject=unsub", "https": "https://acme.com/unsub/AbC123xyz", "one_click": true, "method": "one-click"})
    );
}

#[test]
fn or_term_dedupe() {
    assert_eq!(
        dedupe_or_terms("conference OR conference OR conference OR conference"),
        "conference"
    );
    assert_eq!(dedupe_or_terms("a OR b OR A OR c OR b"), "a OR b OR c");
    assert_eq!(dedupe_or_terms("a OR a OR b"), "a OR a OR b"); // fewer than 3 ORs: untouched
}

#[test]
fn mailto_parsing() {
    assert_eq!(
        parse_mailto("mailto:leave@list.acme.com?subject=remove%20me"),
        (
            "leave@list.acme.com".into(),
            "remove me".into(),
            "unsubscribe".into()
        )
    );
    assert_eq!(
        parse_mailto("mailto:a%40b.com?body=x+y&subject="),
        ("a@b.com".into(), "unsubscribe".into(), "x y".into())
    );
}

#[tokio::test]
async fn unsubscribe_flows() {
    let (port, sink_log) = crate::smtp::tests::sink().await;
    let calls: Arc<Mutex<Vec<(bool, String)>>> = Arc::new(Mutex::new(Vec::new()));
    let c2 = calls.clone();
    let http: UnsubHttp = Arc::new(move |post, url| {
        c2.lock().push((post, url));
        Box::pin(async { Ok(200) })
    });
    let mut g = Gmail::new(
        "tester@example.com",
        "secretpass123",
        "127.0.0.1",
        "127.0.0.1",
        "",
        GmailOptions {
            smtp_port: port,
            smtp_security: "none".into(),
            ..Default::default()
        },
    )
    .with_unsub_http(http);
    let r = g
        .do_unsub(&parse_list_unsubscribe(
            "<https://acme.com/u/1>",
            "List-Unsubscribe=One-Click",
        ))
        .await;
    assert_eq!(r["status"], "done");
    assert_eq!(r["via"], "one-click");
    assert_eq!(
        calls.lock().last().cloned(),
        Some((true, "https://acme.com/u/1".to_string()))
    );
    let r = g
        .do_unsub(&parse_list_unsubscribe(
            "<mailto:leave@list.acme.com?subject=remove%20me>",
            "",
        ))
        .await;
    assert_eq!(r["status"], "done", "{r}");
    assert_eq!(r["note"], "已发送退订邮件给 list.acme.com");
    assert!(sink_log
        .lock()
        .iter()
        .any(|l| l == "RCPT TO:<leave@list.acme.com>"));
    assert!(sink_log
        .lock()
        .iter()
        .any(|l| l.starts_with("data:") && l.contains("Subject: remove me")));
    let r = g
        .do_unsub(&parse_list_unsubscribe("<https://acme.com/u/2>", ""))
        .await;
    assert_eq!(r["status"], "link_opened");
    assert_eq!(
        calls.lock().last().cloned(),
        Some((false, "https://acme.com/u/2".to_string()))
    );
    let r = g
        .do_unsub(&parse_list_unsubscribe("<https://127.0.0.1/u/2>", ""))
        .await;
    assert_eq!(r["status"], "failed"); // private addresses blocked
    assert_eq!(calls.lock().len(), 2);
    let r = g.do_unsub(&json!({"method": "", "one_click": false})).await;
    assert_eq!(
        r,
        json!({"status": "failed", "note": "unsupported unsubscribe method"})
    );
}

/// A small stateful generic server: INBOX / Sent / Drafts, folders created on demand.
fn generic(log: Log, created: Arc<Mutex<Vec<String>>>, login_ok: bool) -> Handler {
    Box::new(move |verb, args, lit| {
        log.lock()
            .push((verb.to_string(), args.to_string(), lit.clone()));
        match verb {
            "LOGIN" if !login_ok => Reply::no("[AUTHENTICATIONFAILED] Authentication failed."),
            "CAPABILITY" => Reply::ok(b"* CAPABILITY IMAP4rev1 UIDPLUS ID\r\n".to_vec()),
            "LIST" => {
                let mut out = b"* LIST (\\HasNoChildren) \"/\" \"INBOX\"\r\n* LIST (\\HasNoChildren \\Sent) \"/\" \"Sent\"\r\n* LIST (\\HasNoChildren \\Drafts) \"/\" \"Drafts\"\r\n* LIST (\\Noselect) \"/\" \"Hidden\"\r\n".to_vec();
                for c in created.lock().iter() {
                    out.extend(format!("* LIST (\\HasNoChildren) \"/\" {c}\r\n").as_bytes());
                }
                Reply::ok(out)
            }
            "CREATE" => {
                created.lock().push(args.to_string());
                Reply::ok(vec![])
            }
            "UID SEARCH" if args == "UNSEEN" => Reply::ok(b"* SEARCH 2 3 5\r\n".to_vec()),
            "UID SEARCH" => Reply::ok(b"* SEARCH 4\r\n".to_vec()),
            "UID FETCH" if args.contains("BODY.PEEK[]") => {
                let raw = b"From: Boss <boss@acme.com>\r\nSubject: Re: Quarterly plan\r\nMessage-ID: <q2@acme.com>\r\nContent-Type: multipart/mixed; boundary=BB\r\n\r\n--BB\r\nContent-Type: text/plain\r\n\r\nAttached the numbers. Your verification code is 482913.\r\n--BB\r\nContent-Type: text/csv\r\nContent-Disposition: attachment; filename=\"numbers.csv\"\r\n\r\na,b\r\n1,2\r\n--BB--\r\n";
                Reply::ok(fetch_line(1, &[("UID 4 BODY[]", raw)]))
            }
            "UID FETCH" if args.contains("REPLY-TO") => Reply::ok(fetch_line(
                1,
                &[("UID 4 BODY[HEADER.FIELDS (FROM REPLY-TO TO CC SUBJECT MESSAGE-ID REFERENCES)]",
                   b"From: Boss <boss@acme.com>\r\nSubject: Re: Quarterly plan\r\nMessage-ID: <q2@acme.com>\r\nReferences: <q1@acme.com>\r\n\r\n")],
            )),
            "UID FETCH" => Reply::ok(fetch_line(
                1,
                &[("UID 4 FLAGS (\\Seen \\Flagged) BODY[HEADER.FIELDS (FROM)]",
                   b"From: Boss <boss@acme.com>\r\nSubject: Re: Quarterly plan\r\nDate: Mon, 21 Sep 2026 10:00:00 +0800\r\n\r\n")],
            )),
            _ => Reply::ok(vec![]),
        }
    })
}

fn custom_client(
    log: &Log,
    created: &Arc<Mutex<Vec<String>>>,
    login_ok: bool,
    provider: &str,
) -> Gmail {
    let (log, created) = (log.clone(), created.clone());
    Gmail::new(
        "tester@example.com",
        "secretpass123",
        "127.0.0.1",
        "127.0.0.1",
        "Tester",
        GmailOptions {
            provider: provider.into(),
            imap_port: 1143,
            imap_security: "none".into(),
            ..Default::default()
        },
    )
    .with_dialer(dialer(move || {
        generic(log.clone(), created.clone(), login_ok)
    }))
}

#[tokio::test]
async fn generic_mailbox_flows() {
    let log = new_log();
    let created = Arc::new(Mutex::new(Vec::new()));
    let mut g = custom_client(&log, &created, true, "custom");
    let t = g.test().await.unwrap();
    assert_eq!(
        t,
        json!({"ok": true, "provider": "custom", "drafts": "Drafts", "sent": "Sent", "unread_inbox": 3})
    );
    assert_eq!(g.gm, Some(false));
    assert!(log
        .lock()
        .iter()
        .any(|(v, a, _)| v == "ID" && a.contains("OMuse")));
    // message + attachment + secrets redacted
    let full = g.get_message("i-4").await.unwrap();
    assert!(
        vs(&full, "body").contains("numbers") && !vs(&full, "body").contains("482913"),
        "{full}"
    );
    assert_eq!(full["attachments"][0]["filename"], "numbers.csv");
    assert_eq!(full["labels"], json!(["INBOX", "\\Starred"]));
    assert_eq!(full["unread"], false);
    let (name, _, data) = g.get_attachment("i-4", "numbers.csv").await.unwrap();
    assert!(name == "numbers.csv" && String::from_utf8_lossy(&data).contains("1,2"));
    assert_eq!(
        g.get_message("bogus").await.unwrap_err().0,
        "无效的 message_id: bogus"
    );
    // draft reply appended to Drafts
    let d = g
        .create_draft("boss@acme.com", "", "Draft reply", "", Some("i-4"), &[])
        .await
        .unwrap();
    assert_eq!(
        d,
        json!({"saved": true, "to": "boss@acme.com", "subject": "Re: Quarterly plan", "folder": "Drafts"})
    );
    let (_, args, lit) = log
        .lock()
        .iter()
        .rev()
        .find(|(v, _, _)| v == "APPEND")
        .cloned()
        .unwrap();
    assert!(args.starts_with("\"Drafts\" (\\Draft \\Seen) \""), "{args}");
    let lit = String::from_utf8(lit.unwrap()).unwrap();
    assert!(
        lit.contains("In-Reply-To: <q2@acme.com>")
            && lit.contains("References: <q1@acme.com> <q2@acme.com>"),
        "{lit}"
    );
    // archive: no Archive folder -> created, then COPY + \Deleted + UID EXPUNGE (no MOVE capability)
    let a = g.archive(&["i-4".into()]).await.unwrap();
    assert_eq!(a, json!({"archived": 1}));
    assert_eq!(created.lock().as_slice(), ["\"Archive\""]);
    let cmds: Vec<(String, String)> = log
        .lock()
        .iter()
        .map(|(v, a, _)| (v.clone(), a.clone()))
        .collect();
    assert!(cmds.contains(&("UID COPY".into(), "4 \"Archive\"".into())));
    assert!(cmds.contains(&("UID STORE".into(), "4 +FLAGS (\\Deleted)".into())));
    assert!(cmds.contains(&("UID EXPUNGE".into(), "4".into())));
    assert_eq!(
        g.archive(&["s-4".into()]).await.unwrap(),
        json!({"archived": 0})
    ); // not in the inbox
       // labels: star + UTF-7 folder copy
    let lab = g
        .label(&["i-4".into()], &["STARRED".into(), "工作".into()], &[])
        .await
        .unwrap();
    assert_eq!(lab, json!({"added": 2}));
    assert!(created.lock().contains(&"\"&XeVPXA-\"".to_string()));
    assert!(log
        .lock()
        .iter()
        .any(|(v, a, _)| v == "UID COPY" && a == "4 \"&XeVPXA-\""));
    assert!(g.list_labels().await.unwrap().contains(&"工作".to_string()));
    let lab = g
        .label(&["i-4".into()], &[], &["STARRED".into()])
        .await
        .unwrap();
    assert_eq!(lab, json!({"removed": 1}));
    assert!(log
        .lock()
        .iter()
        .any(|(v, a, _)| v == "UID STORE" && a == "4 -FLAGS (\\Flagged)"));
    assert_eq!(
        g.mark_read(&["i-4".into()], true).await.unwrap(),
        json!({"updated": 1})
    );
    // ids in other folders select the folder by key
    let crc_id = format!("{}-4", crc("\"&XeVPXA-\""));
    g.get_message(&crc_id).await.unwrap();
    assert!(log
        .lock()
        .iter()
        .any(|(v, a, _)| v == "EXAMINE" && a == "\"&XeVPXA-\""));
    // unsubscribe targets: no header -> manual
    let tg = g.unsubscribe(&["i-4".into(), "zz".into()]).await.unwrap();
    assert_eq!(tg[0]["status"], "manual");
    assert_eq!(
        tg[1],
        json!({"id": "zz", "from": "", "subject": "", "method": "", "status": "failed", "note": "无效的 message_id: zz"})
    );
    // thread via References across folders
    let th = g.get_thread("i-4").await.unwrap();
    assert!(!th.is_empty());
    assert!(log.lock().iter().any(|(v, a, _)| v == "UID SEARCH"
        && a == "OR OR HEADER Message-ID \"<q1@acme.com>\" HEADER References \"<q1@acme.com>\" HEADER In-Reply-To \"<q1@acme.com>\""));
}

#[tokio::test]
async fn bad_password_hint() {
    let log = new_log();
    let created = Arc::new(Mutex::new(Vec::new()));
    let mut bad = custom_client(&log, &created, false, "qq");
    let e = bad.test().await.unwrap_err();
    assert!(
        e.0.starts_with(
            "IMAP 登录失败 (login failed): b'[AUTHENTICATIONFAILED] Authentication failed.'. "
        ),
        "{e}"
    );
    assert!(e.0.contains("授权码"));
    // unreachable server
    let mut g = Gmail::new(
        "a@b.com",
        "x",
        "127.0.0.1",
        "127.0.0.1",
        "",
        GmailOptions {
            provider: "custom".into(),
            imap_port: 1,
            imap_security: "none".into(),
            ..Default::default()
        },
    );
    let e = g.test().await.unwrap_err();
    assert!(e.0.starts_with("无法连接 127.0.0.1:1: "), "{e}");
}

#[tokio::test]
async fn xoauth2_login_and_token_errors() {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let s2 = seen.clone();
    let make = move || -> Handler {
        let s3 = s2.clone();
        Box::new(move |verb, _args, lit| {
            if verb == "AUTHENTICATE" {
                s3.lock()
                    .push(String::from_utf8(lit.unwrap_or_default()).unwrap());
            }
            match verb {
                "CAPABILITY" => Reply::ok(b"* CAPABILITY IMAP4rev1\r\n".to_vec()),
                "LIST" => Reply::ok(b"* LIST () \"/\" \"INBOX\"\r\n".to_vec()),
                "UID SEARCH" => Reply::ok(b"* SEARCH\r\n".to_vec()),
                _ => Reply::ok(vec![]),
            }
        })
    };
    let tok: TokenFn = Arc::new(|| Box::pin(async { Ok("at-1".to_string()) }));
    let mut g = Gmail::new(
        "me@outlook.com",
        "",
        "127.0.0.1",
        "127.0.0.1",
        "",
        GmailOptions {
            provider: "outlook".into(),
            token_fn: Some(tok),
            ..Default::default()
        },
    )
    .with_dialer(dialer(make.clone()));
    let t = g.test().await.unwrap();
    assert_eq!(t["unread_inbox"], 0);
    assert_eq!(t["drafts"], Value::Null);
    assert_eq!(
        seen.lock()[0],
        "user=me@outlook.com\x01auth=Bearer at-1\x01\x01"
    );
    let bad: TokenFn = Arc::new(|| {
        Box::pin(async {
            Err(OAuthError(
                "微软授权已失效，请在「连接」页重新登录 (refresh failed): invalid_grant: x".into(),
            ))
        })
    });
    g.token_fn = Some(bad);
    let e = g.test().await.unwrap_err();
    assert!(e.0.contains("重新登录"));
}

#[test]
fn make_client_fields() {
    let ms = MsLogin::new("http://127.0.0.1:1");
    let acc = json!({"id": "g3", "email": "me@qq.com", "provider": "qq", "imap_host": "imap.qq.com", "imap_port": 993,
        "imap_security": "ssl", "smtp_host": "smtp.qq.com", "smtp_port": "465", "smtp_security": "ssl", "username": ""});
    let g = make_client(&acc, &json!({"app_password": "a b"}), None, &ms);
    assert_eq!(
        (
            g.account_id.as_str(),
            g.username.as_str(),
            g.password.as_str()
        ),
        ("g3", "me@qq.com", "a b")
    );
    assert!(g.token_fn.is_none());
    let g = make_client(
        &json!({"email": "x@outlook.com"}),
        &json!({"oauth": {"client_id": "c", "refresh_token": "r"}}),
        None,
        &ms,
    );
    assert!(g.token_fn.is_some());
    assert_eq!(
        (
            g.account_id.as_str(),
            g.imap_host.as_str(),
            g.provider.as_str()
        ),
        ("g1", "imap.gmail.com", "gmail")
    );
}

#[test]
fn crc_and_ids() {
    assert_eq!(crc32(b"hello"), 0x3610a686);
    assert!(GEN_ID.is_match("x0123abcd-42") && !GEN_ID.is_match("q-1"));
}
