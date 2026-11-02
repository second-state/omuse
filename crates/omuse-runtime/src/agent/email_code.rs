//! The email verification step — "enter your email → send code → type the code" — as one reusable function for every
//! code machine with such a step (docs/json-tools.md, "Email verification codes"): the Taiwan Arrival Card and the UK
//! ETA today (`driver::CardSpec::email_code`), any sign-up later.
//!
//! A machine asks for it with a code call `email_code` whose arguments are a [`Spec`] (the page's fixed selectors and
//! the site's sender) and the user's email. The runtime runs it ([`run`]; `Runtime::email_code_step`):
//!
//! 1. code types the email into the page's email box;
//! 2. no mailbox connected: that is all — the result says so and the machine hands the page over (the user presses
//!    Send and types the code);
//! 3. a mailbox: code presses Send, then waits for the mail with a structured query (the site's sender, the last day,
//!    dated after the press — Sentinel's `/internal/mail_code`), polling up to a timeout;
//! 4. the mail's text is natural language: the MODEL reads the code out of it as JSON (`{"code": "…"}`, one more try
//!    with the planner model) — here, in the runtime, never inside a code tool;
//! 5. code checks the value's shape (4–10 letters or digits) and types it into the code box (as a secret: it never
//!    shows in a picture, the transcript or the result), then presses the verify button when the page has one.
//!
//! The result is JSON (`status`: typed · no_mailbox · timeout · no_code · …) and never carries the code.

use std::future::Future;
use std::time::Duration;

use omuse_common::util::{get_str, take_chars};
use serde_json::{json, Value};

use crate::llm::{ChatOpts, Llm};

/// The code call's name (a machine's call, never in the model's catalog).
pub const EMAIL_CODE: &str = "email_code";

/// A site's email verification step, as data: where the email goes, the button that sends the code, where the code
/// goes, the button that checks it, and the domain the mail comes from.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Spec {
    /// the email box (CSS selector)
    pub email_sel: &'static str,
    /// the text of the button that sends the code
    pub send_text: &'static str,
    /// the code box (CSS selector; its first visible match)
    pub code_sel: &'static str,
    /// the text of the button that checks the code ("" = none: the code box is enough)
    pub verify_text: &'static str,
    /// the sender's domain, for the mailbox query
    pub from: &'static str,
}

impl Spec {
    /// The `email_code` call's arguments for the user's `email`.
    pub fn args(&self, email: &str) -> Value {
        json!({"email": email, "email_sel": self.email_sel, "send_text": self.send_text,
               "code_sel": self.code_sel, "verify_text": self.verify_text, "from": self.from})
    }
}

/// How long to wait for the mail.
#[derive(Debug, Clone, Copy)]
pub struct Timing {
    /// between two mailbox reads
    pub poll: Duration,
    /// after the press, before giving up
    pub timeout: Duration,
}

/// The real step's waiting: a read every 10 s for two and a half minutes.
pub const TIMING: Timing = Timing {
    poll: Duration::from_secs(10),
    timeout: Duration::from_secs(150),
};

/// The mailbox query: mail from the site's sender in the last day (the press time narrows it further).
pub fn mail_query(from: &str) -> String {
    format!("from:{} newer_than:1d", from.trim())
}

/// The system prompt of the extraction.
pub const EXTRACT_SYSTEM: &str = "You read one email for a program. A website sent it after the user asked for a \
verification code. Answer with JSON only: {\"code\": \"<the verification code exactly as written in the email>\"}, or \
{\"code\": null} when the email holds no verification code. No other keys, no explanation.";

/// The user message of the extraction: the site and the mail as JSON.
pub fn extraction_prompt(site: &str, mail: &Value) -> String {
    json!({"site": site, "from": get_str(mail, "from"), "subject": get_str(mail, "subject"),
           "body": take_chars(&get_str(mail, "body"), 6000)})
    .to_string()
}

/// What the model's answer came to.
#[derive(Debug, Clone, PartialEq)]
pub enum Code {
    /// a code of the right shape (spaces and hyphens taken out)
    Code(String),
    /// the model says the mail holds none
    NoCode,
    /// anything else (prose, a wrong shape, JSON without the key)
    Invalid(String),
}

/// The code in a model reply, through the JSON layer (`json_in::extract` repairs the syntax): `{"code": "<4–10 letters
/// or digits>"}` (a number is taken as its digits); `{"code": null}` or `""` is no code.
pub fn parse_code(reply: &str) -> Code {
    let Some((v, _)) = super::json_in::extract(reply) else {
        return Code::Invalid("not JSON".into());
    };
    let raw = match v.get("code") {
        None => return Code::Invalid("no \"code\" key".into()),
        Some(Value::Null) => return Code::NoCode,
        Some(Value::Number(n)) => n.to_string(),
        Some(Value::String(s)) => s.clone(),
        Some(other) => return Code::Invalid(format!("not a string: {other}")),
    };
    let code: String = raw.chars().filter(|c| !matches!(c, ' ' | '-')).collect();
    if code.is_empty() {
        return Code::NoCode;
    }
    if (4..=10).contains(&code.chars().count()) && code.chars().all(|c| c.is_ascii_alphanumeric()) {
        Code::Code(code)
    } else {
        Code::Invalid(format!("not a code: {}", take_chars(&raw, 40)))
    }
}

/// The model reads the code out of `mail`: one try with the default model, one more with `retry_model` (the planner)
/// when the first answer is not a code. `Ok(None)`: the mail holds none.
pub async fn read_code(
    llm: &Llm,
    site: &str,
    mail: &Value,
    task_id: &str,
    retry_model: Option<String>,
) -> Result<Option<String>, String> {
    let msgs = [
        json!({"role": "system", "content": EXTRACT_SYSTEM}),
        json!({"role": "user", "content": extraction_prompt(site, mail)}),
    ];
    let mut why = String::new();
    for model in [None, retry_model] {
        let o = ChatOpts {
            max_tokens: Some(80),
            temperature: Some(0.0),
            no_think: true,
            purpose: "email_code".into(),
            task_id: task_id.to_string(),
            model,
            ..Default::default()
        };
        let r = llm
            .chat(&msgs, None, &o)
            .await
            .map_err(|e| take_chars(&e.to_string(), 200).to_string())?;
        match parse_code(&r.content) {
            Code::Code(c) => return Ok(Some(c)),
            Code::NoCode => return Ok(None),
            Code::Invalid(w) => why = w,
        }
    }
    Err(why)
}

/// Whether a `browser_fill_form` result says every op worked.
fn all_ok(r: &Result<Value, String>) -> bool {
    let Ok(v) = r else {
        return false;
    };
    let filled = v
        .get("filled")
        .or_else(|| v.get("result").and_then(|x| x.get("filled")))
        .and_then(Value::as_array);
    filled.is_some_and(|f| {
        !f.is_empty()
            && f.iter()
                .all(|x| x.get("ok").and_then(Value::as_bool) == Some(true))
    })
}

/// The step (module doc). `fill(fills)` runs one `browser_fill_form` plan on the task's page and returns its result;
/// `mail(query, since)` is Sentinel's `/internal/mail_code`; `read(mail)` is the model's extraction ([`read_code`]);
/// `now()` the epoch seconds; `pause(d)` waits `d` between two mailbox reads and says whether the user cancelled the
/// task meanwhile (the wait then ends at once with `cancelled`). The result never carries the code.
#[allow(clippy::too_many_arguments)]
pub async fn run<F, FF, M, MF, R, RF, P, PF>(
    args: &Value,
    mail_ready: bool,
    timing: Timing,
    now: impl Fn() -> f64,
    pause: P,
    fill: F,
    mail: M,
    read: R,
) -> Value
where
    P: Fn(Duration) -> PF,
    PF: Future<Output = bool>,
    F: Fn(Value) -> FF,
    FF: Future<Output = Result<Value, String>>,
    M: Fn(String, f64) -> MF,
    MF: Future<Output = Result<Value, String>>,
    R: Fn(Value) -> RF,
    RF: Future<Output = Result<Option<String>, String>>,
{
    let s = |k: &str| get_str(args, k).trim().to_string();
    let (email, from) = (s("email"), s("from"));
    let base = |status: &str, filled: bool, sent: bool| json!({"status": status, "email_filled": filled, "sent": sent, "from": from});
    if email.is_empty() || s("email_sel").is_empty() {
        return base("no_email", false, false);
    }
    let typed =
        fill(json!([{"op": "text", "sel": s("email_sel"), "value": email, "field": "email"}]))
            .await;
    if !all_ok(&typed) {
        return base("no_email_field", false, false);
    }
    if !mail_ready || from.is_empty() {
        return base("no_mailbox", true, false);
    }
    // the press time, less a minute for the clocks of the site's mail server
    let since = now() - 60.0;
    let pressed = fill(
        json!([{"op": "button", "sel": "button", "text": s("send_text"), "field": "send_code"}]),
    )
    .await;
    if !all_ok(&pressed) {
        return base("no_send_button", true, false);
    }
    let query = mail_query(&from);
    let started = now();
    let found = loop {
        if pause(timing.poll).await {
            return base("cancelled", true, true);
        }
        match mail(query.clone(), since).await {
            Ok(v) if v.get("mailbox").and_then(Value::as_bool) == Some(false) => {
                return base("no_mailbox", true, true);
            }
            Ok(v) if v.get("found").and_then(Value::as_bool) == Some(true) => {
                break v["mail"].clone()
            }
            _ => {}
        }
        if now() - started >= timing.timeout.as_secs_f64() {
            return base("timeout", true, true);
        }
    };
    let code = match read(found).await {
        Ok(Some(c)) => c,
        Ok(None) | Err(_) => return base("no_code", true, true),
    };
    let mut ops = vec![
        json!({"op": "text", "sel": s("code_sel"), "value": code, "secret": true, "field": "verification_code"}),
    ];
    if !s("verify_text").is_empty() {
        ops.push(json!({"op": "button", "sel": "button", "text": s("verify_text"), "field": "verify_code"}));
    }
    if all_ok(&fill(Value::Array(ops)).await) {
        base("typed", true, true)
    } else {
        base("code_not_typed", true, true)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use parking_lot::Mutex;

    use super::*;

    const TW: Spec = Spec {
        email_sel: "input[name=email]",
        send_text: "SEND VERIFICATION CODE",
        code_sel: "input[type=text]",
        verify_text: "VERIFY",
        from: "immigration.gov.tw",
    };

    fn fast() -> Timing {
        Timing {
            poll: Duration::from_millis(1),
            timeout: Duration::from_millis(30),
        }
    }

    fn ok_fill(fills: &Value) -> Value {
        let n = fills.as_array().map(Vec::len).unwrap_or(0);
        json!({"url": "https://twac.immigration.gov.tw/submit", "filled": vec![json!({"ok": true}); n]})
    }

    /// One scripted run: the fills seen, the mailbox reads, the result.
    async fn scripted(
        mail_ready: bool,
        mails: Vec<Value>,
        code: Result<Option<String>, String>,
    ) -> (Vec<Value>, Vec<(String, f64)>, Value) {
        let fills = Arc::new(Mutex::new(Vec::new()));
        let reads = Arc::new(Mutex::new(Vec::new()));
        let mails = Arc::new(Mutex::new(mails));
        let clock = Arc::new(Mutex::new(1_000_000.0));
        let (f2, r2, m2, c2, c3) = (
            fills.clone(),
            reads.clone(),
            mails.clone(),
            clock.clone(),
            clock.clone(),
        );
        let res = run(
            &TW.args("michael@michaelyuan.com"),
            mail_ready,
            fast(),
            move || {
                let mut c = c2.lock();
                *c += 0.01;
                *c
            },
            |d: Duration| async move {
                tokio::time::sleep(d).await;
                false
            },
            move |v: Value| {
                f2.lock().push(v.clone());
                async move { Ok(ok_fill(&v)) }
            },
            move |q: String, since: f64| {
                r2.lock().push((q, since));
                let next = {
                    let mut m = m2.lock();
                    if m.is_empty() {
                        json!({"mailbox": true, "found": false})
                    } else {
                        m.remove(0)
                    }
                };
                async move { Ok(next) }
            },
            move |_mail: Value| {
                let c = code.clone();
                async move { c }
            },
        )
        .await;
        let _ = c3;
        let f = fills.lock().clone();
        let r = reads.lock().clone();
        (f, r, res)
    }

    fn the_mail() -> Value {
        json!({"mailbox": true, "found": true,
               "mail": {"from": "TWAC <noreply@immigration.gov.tw>", "subject": "Verification code", "body": "Your code is 481 207."}})
    }

    #[tokio::test]
    async fn with_a_mailbox_the_code_is_read_from_the_mail_and_typed_as_a_secret() {
        let (fills, reads, res) = scripted(
            true,
            vec![json!({"mailbox": true, "found": false}), the_mail()],
            Ok(Some("481207".into())),
        )
        .await;
        assert_eq!(res["status"], "typed", "{res}");
        // the email, the press, the code (secret) and the verify button — in that order
        assert_eq!(fills.len(), 3, "{fills:?}");
        assert_eq!(fills[0][0]["value"], "michael@michaelyuan.com");
        assert_eq!(fills[0][0]["sel"], "input[name=email]");
        assert_eq!(fills[1][0]["text"], "SEND VERIFICATION CODE");
        assert_eq!(fills[2][0]["value"], "481207");
        assert_eq!(fills[2][0]["secret"], true);
        assert_eq!(fills[2][1]["text"], "VERIFY");
        // the structured query, polled until the mail came, dated after the press (less the clock margin)
        assert_eq!(reads.len(), 2);
        assert_eq!(reads[0].0, "from:immigration.gov.tw newer_than:1d");
        assert!(reads[0].1 < 1_000_000.1 && reads[0].1 > 999_900.0);
        // the code is never in the result
        assert!(!res.to_string().contains("481207"));
    }

    #[tokio::test]
    async fn without_a_mailbox_only_the_email_is_filled_and_nothing_is_sent() {
        let (fills, reads, res) = scripted(false, vec![], Ok(Some("1234".into()))).await;
        assert_eq!(
            (
                res["status"].as_str(),
                res["email_filled"].as_bool(),
                res["sent"].as_bool()
            ),
            (Some("no_mailbox"), Some(true), Some(false))
        );
        assert_eq!(fills.len(), 1, "only the email: {fills:?}");
        assert!(reads.is_empty());
    }

    #[tokio::test]
    async fn no_mail_before_the_timeout_or_no_code_in_it_hands_over() {
        let (fills, reads, res) = scripted(true, vec![], Ok(Some("1234".into()))).await;
        assert_eq!(res["status"], "timeout");
        assert_eq!(
            fills.len(),
            2,
            "the email and the press, no code: {fills:?}"
        );
        assert!(!reads.is_empty());
        let (fills, _, res) = scripted(true, vec![the_mail()], Ok(None)).await;
        assert_eq!(res["status"], "no_code");
        assert_eq!(fills.len(), 2);
        let (_, _, res) = scripted(true, vec![the_mail()], Err("prose".into())).await;
        assert_eq!(res["status"], "no_code");
        // the mailbox went away after the press
        let (_, _, res) = scripted(
            true,
            vec![json!({"mailbox": false, "found": false})],
            Ok(None),
        )
        .await;
        assert_eq!(
            (res["status"].as_str(), res["sent"].as_bool()),
            (Some("no_mailbox"), Some(true))
        );
    }

    #[tokio::test]
    async fn a_cancelled_task_stops_waiting_for_the_mail() {
        let reads = Arc::new(Mutex::new(0));
        let r2 = reads.clone();
        let res = run(
            &TW.args("michael@michaelyuan.com"),
            true,
            Timing {
                poll: Duration::from_millis(1),
                timeout: Duration::from_secs(3600),
            },
            || 1_000_000.0,
            |_d: Duration| async { true },
            |v: Value| async move { Ok(ok_fill(&v)) },
            move |_q: String, _s: f64| {
                *r2.lock() += 1;
                async { Ok(json!({"mailbox": true, "found": false})) }
            },
            |_m: Value| async { Ok(Some("1234".to_string())) },
        )
        .await;
        assert_eq!(res["status"], "cancelled");
        assert_eq!(*reads.lock(), 0, "no mailbox read after the cancel");
    }

    #[test]
    fn the_code_is_checked_json() {
        assert_eq!(
            parse_code(r#"{"code": "481207"}"#),
            Code::Code("481207".into())
        );
        assert_eq!(
            parse_code(r#"{"code": "481 207"}"#),
            Code::Code("481207".into())
        );
        assert_eq!(
            parse_code("```json\n{'code': 'AB12CD'}\n```"),
            Code::Code("AB12CD".into())
        );
        assert_eq!(
            parse_code(r#"{"code": 481207}"#),
            Code::Code("481207".into())
        );
        assert_eq!(parse_code(r#"{"code": null}"#), Code::NoCode);
        assert_eq!(parse_code(r#"{"code": ""}"#), Code::NoCode);
        for bad in [
            "Your code is 481207.",
            r#"{"otp": "481207"}"#,
            r#"{"code": "12"}"#,
            r#"{"code": "https://x.test/verify?t=abc"}"#,
            r#"{"code": "12345678901"}"#,
        ] {
            assert!(matches!(parse_code(bad), Code::Invalid(_)), "{bad}");
        }
    }

    fn answer(content: &str) -> Result<crate::llm::HttpResponse, String> {
        crate::llm::tests::resp(
            200,
            json!({"choices": [{"message": {"role": "assistant", "content": content}}], "usage": {}}),
            None,
        )
    }

    #[tokio::test]
    async fn the_model_reads_the_code_as_json_with_one_more_try() {
        let mail = json!({"from": "noreply@immigration.gov.tw", "subject": "Code", "body": "Your verification code: 481207"});
        let (llm, tr, _, _) =
            crate::llm::tests::scripted_llm(vec![answer(r#"{"code": "481207"}"#)]);
        assert_eq!(
            read_code(&llm, "twac.immigration.gov.tw", &mail, "t1", None).await,
            Ok(Some("481207".into()))
        );
        let sent = format!("{:?}", tr.requests.lock()[0]);
        assert!(sent.contains("verification code") && sent.contains("481207"));
        // prose first: the planner model's try is taken
        let (llm, tr, _, _) = crate::llm::tests::scripted_llm(vec![
            answer("The code is 481207."),
            answer(r#"{"code": "481207"}"#),
        ]);
        assert_eq!(
            read_code(&llm, "x", &mail, "t1", Some("planner-m".into())).await,
            Ok(Some("481207".into()))
        );
        assert!(format!("{:?}", tr.requests.lock()[1]).contains("planner-m"));
        // twice wrong: an error (the step hands over), never a guess
        let (llm, _, _, _) =
            crate::llm::tests::scripted_llm(vec![answer("no idea"), answer("still none")]);
        assert!(read_code(&llm, "x", &mail, "t1", None).await.is_err());
    }
}
