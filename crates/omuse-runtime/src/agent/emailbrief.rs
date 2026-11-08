//! The email brief (skill `morning-email-brief`, a long task: agent/longtask.rs): the important recent emails, the
//! ones that need a reply with a suggested reply each, as one table.
//!
//! The model calls `email_brief` with the period (a day or a week) and, when the user asked for it on a timer, the
//! schedule. Code runs every step ([`next`]): ONE fixed mail search of the inbox without the promotions, social,
//! updates and forums categories; the first [`READS`] hits read in full in one turn; a subagent
//! ([`TRIAGE`], JSON in and out) sorts them into "needs a reply" (why, a suggested reply) and "for your information"
//! by their numbers; code lays out the table (From | Subject | Why | Suggested reply | message id), makes the schedule
//! asked for with the task's JSON, mails the brief to the user's own address on a scheduled run, and ends with the
//! JSON result (`show`: the table). Nothing is ever sent to anyone else; drafts are the user's next request.

use chrono::NaiveTime;
use omuse_common::util::{get_str, take_chars, truthy};
use serde_json::{json, Value};

use super::driver::{json_of, results_of, walk_messages, Step};
use super::longtask::{self, SubSpec, When};
use super::show as S;
use super::tools::fn_;
use super::util::{content_of, sget, tool_calls};
use super::workflow::{self, Checked};

/// The skill whose run asks through this tool.
pub const SKILL: &str = "morning-email-brief";
/// The id prefix of the machine's code calls.
pub const PREFIX: &str = "brf_";
pub const EMAIL_BRIEF: &str = "email_brief";
pub const TOOLS: [&str; 1] = [EMAIL_BRIEF];
/// The subagent that sorts the emails.
pub const TRIAGE: &str = "email_triage";
/// The emails read in full.
pub const READS: usize = 12;
const TEXT_CHARS: usize = 1500;

pub fn tool_schemas() -> Vec<Value> {
    vec![fn_(
        EMAIL_BRIEF,
        "The email brief: the important recent emails in the user's inbox, the ones that need a reply with a \
suggested reply each, and the rest for their information, as one table (\"summarise my new emails\", \"what needs a \
reply\", \"my morning email brief\", also every weekday at 8). Code searches the mail, reads it, sorts it and lays it \
out; nothing is sent. The JSON result has the table.",
        json!({
            "period": {"type": "string", "enum": ["day", "week"], "description": "day for today's / new mail (default), week for this week's"},
            "schedule": {"type": "object", "description": "only when the user asked for the brief on a timer (\"every weekday at 8 am\")", "properties": {
                "frequency": {"type": "string", "enum": ["daily", "weekdays", "weekly"]},
                "time": {"type": "string", "description": "HH:MM"},
                "weekday": {"type": "integer", "description": "weekly: 0 = Sunday … 6 = Saturday"}}},
            "email": {"type": "boolean", "description": "the user asked for the brief to be sent to them by email"}
        }),
        &[],
    )]
}

/// The validated arguments: the period, the schedule asked for (as a cron spec and its words), the mail flag.
pub fn check(args: &Value) -> Checked {
    let period = match sget(args, "period").to_lowercase().as_str() {
        "week" | "weekly" => "week",
        _ => "day",
    };
    let mut out = json!({"period": period, "email": args.get("email").is_some_and(truthy)});
    if let Some(s) = args.get("schedule").filter(|s| s.is_object()) {
        if let Ok(t) = NaiveTime::parse_from_str(sget(s, "time").trim(), "%H:%M") {
            let f = match sget(s, "frequency").as_str() {
                f @ ("daily" | "weekdays" | "weekly") => f.to_string(),
                _ => "daily".to_string(),
            };
            let wd = s.get("weekday").and_then(Value::as_u64).unwrap_or(1) as u32;
            let w = When::new(
                &f,
                chrono::Timelike::hour(&t),
                chrono::Timelike::minute(&t),
                wd,
            );
            out["schedule"] = json!({"spec": w.cron(), "every": w.every()});
        }
    }
    Checked::Valid(out)
}

/// The fixed search of the period: the inbox without the bulk categories.
pub fn query(period: &str) -> String {
    format!(
        "in:inbox newer_than:{} -category:promotions -category:social -category:updates -category:forums",
        if period == "week" { "7d" } else { "1d" }
    )
}

// ---------------------------------------------------------------- the subagent

fn triage_output() -> Value {
    json!({"type": "object", "properties": {
        "summary": {"type": "string", "description": "one or two sentences on what came in"},
        "needs_reply": {"type": "array", "items": {"type": "object", "properties": {
            "email": {"type": "integer", "description": "the email's number in the input"},
            "why": {"type": "string", "description": "why it matters, at most 20 words"},
            "reply": {"type": "string", "description": "a suggested reply in 1-2 sentences"}}}},
        "fyi": {"type": "array", "items": {"type": "object", "properties": {
            "email": {"type": "integer"}, "line": {"type": "string", "description": "one line"}}}}},
        "required": ["needs_reply", "fyi"]})
}

fn numbers(input: &Value) -> usize {
    input["emails"].as_array().map_or(0, Vec::len)
}

fn triage_check(input: &Value, out: &Value) -> Result<Value, Vec<String>> {
    if !out.is_object() {
        return Err(vec!["the answer is not a JSON object".into()]);
    }
    let n = numbers(input) as i64;
    let mut seen: Vec<i64> = Vec::new();
    let mut problems = Vec::new();
    let mut pick = |key: &str, fields: &[&str]| -> Vec<Value> {
        let mut rows = Vec::new();
        for (i, r) in out[key].as_array().into_iter().flatten().enumerate() {
            let k = r.get("email").and_then(Value::as_i64).unwrap_or(0);
            if k < 1 || k > n {
                problems.push(format!(
                    "{key}[{i}].email: {k} is not an email of the input (1–{n})"
                ));
                continue;
            }
            if seen.contains(&k) {
                continue;
            }
            seen.push(k);
            let mut row = json!({"email": k});
            for f in fields {
                row[*f] = json!(take_chars(sget(r, f).trim(), 400));
            }
            rows.push(row);
        }
        rows
    };
    let needs = pick("needs_reply", &["why", "reply"]);
    let fyi = pick("fyi", &["line"]);
    if !problems.is_empty() && needs.is_empty() && fyi.is_empty() {
        return Err(problems);
    }
    Ok(
        json!({"summary": take_chars(sget(out, "summary").trim(), 400), "needs_reply": needs, "fyi": fyi}),
    )
}

/// The default: every email for the user's information, by its subject.
fn triage_fallback(input: &Value) -> Value {
    let fyi: Vec<Value> = input["emails"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|e| json!({"email": e["email"], "line": get_str(e, "subject")}))
        .collect();
    json!({"summary": "", "needs_reply": [], "fyi": fyi})
}

pub fn triage_spec() -> SubSpec {
    SubSpec {
        kind: TRIAGE,
        instruction: "You sort a user's recent emails for their morning brief. needs_reply: an email in which a person \
asks the user a question, asks them to act or waits for their decision — say why it matters and suggest a reply of one \
or two sentences in the email's language. fyi: what the user should know about but need not answer, one line each. \
Leave out newsletters, receipts, notifications and no-reply senders. Each email at most once, by its number; the most \
urgent first.",
        output: triage_output,
        tools: &[],
        check: triage_check,
        fallback: triage_fallback,
    }
}

// ---------------------------------------------------------------- the machine

pub struct Machine<'a> {
    /// the profile, with the connected mailbox's address as `mailbox`
    pub profile: &'a Value,
    /// a schedule's run: the brief is mailed to the user's own address, no schedule is made or offered
    pub scheduled: bool,
    pub first: usize,
    /// the schedule offer this chat run ends with (agent/longtask.rs; `None`: scheduled, offered already, or a run)
    pub offer: Option<Value>,
}

fn failed(m: &Value) -> bool {
    content_of(m).trim_start().starts_with("ERROR")
}

fn data_of(m: &Value) -> Value {
    let v = json_of(m);
    match v.get("content") {
        Some(c) if c.is_object() || c.is_array() => c.clone(),
        _ => v,
    }
}

fn cell(s: &str) -> String {
    take_chars(&s.replace('|', "/").replace('\n', " "), 300).to_string()
}

pub fn next(args: &Value, t: &[Value], m: &Machine) -> Step {
    let n = m.first
        + t.iter()
            .flat_map(tool_calls)
            .filter(|tc| get_str(tc, "id").starts_with(PREFIX))
            .count()
        + 1;
    let period = sget(args, "period");
    // 1. the search
    let searches = results_of(t, "gmail_search");
    let Some((_, sm)) = searches.last() else {
        return Step::Calls(vec![workflow::code_call(
            PREFIX,
            n,
            "gmail_search",
            json!({"query": query(&period), "max_results": 25}),
        )]);
    };
    if failed(sm) {
        let why = take_chars(content_of(sm).trim_start_matches("ERROR:").trim(), 200).to_string();
        return Step::Done(json!({"status": "error", "why": why,
            "show": [S::fact("Mail not read", "邮件未读取", &why)], "hints": []}));
    }
    let mut hits = Vec::new();
    walk_messages(&json_of(sm), &mut hits);
    let ids: Vec<String> = hits
        .iter()
        .map(|h| get_str(h, "id"))
        .filter(|id| !id.is_empty())
        .fold(Vec::new(), |mut v, id| {
            if !v.contains(&id) {
                v.push(id);
            }
            v
        })
        .into_iter()
        .take(READS)
        .collect();
    // 2. the first hits in full, one turn
    let reads = results_of(t, "gmail_get_message");
    if reads.is_empty() && !ids.is_empty() {
        return Step::Calls(
            ids.iter()
                .enumerate()
                .map(|(i, id)| {
                    workflow::code_call(
                        PREFIX,
                        n + i,
                        "gmail_get_message",
                        json!({"message_id": id}),
                    )
                })
                .collect(),
        );
    }
    let emails: Vec<Value> = reads
        .iter()
        .filter(|(_, msg)| !failed(msg))
        .enumerate()
        .map(|(i, (a, msg))| {
            let v = data_of(msg);
            let text = [
                get_str(&v, "body"),
                get_str(&v, "text"),
                get_str(&v, "snippet"),
            ]
            .into_iter()
            .find(|x| !x.is_empty())
            .unwrap_or_default();
            json!({"email": i + 1, "message_id": sget(a, "message_id"), "from": get_str(&v, "from"),
                   "subject": get_str(&v, "subject"), "date": get_str(&v, "date"),
                   "injection_warning": v.get("injection_warning").is_some_and(truthy),
                   "text": take_chars(&text, TEXT_CHARS)})
        })
        .collect();
    let mut show: Vec<Value> = Vec::new();
    let mut out = json!({"status": "brief", "period": period, "emails": emails.len()});
    if emails.is_empty() {
        show.push(S::fact(
            "Email brief",
            "邮件简报",
            &format!(
                "no new email in your inbox {}",
                if period == "week" {
                    "this week"
                } else {
                    "today"
                }
            ),
        ));
        out["status"] = json!("empty");
    } else {
        // 3. the subagent sorts them
        let Some(tri) = longtask::sub_answer(t, TRIAGE) else {
            let input: Vec<Value> = emails
                .iter()
                .map(|e| json!({"email": e["email"], "from": e["from"], "subject": e["subject"], "date": e["date"], "text": e["text"]}))
                .collect();
            return Step::Calls(vec![longtask::sub_call(
                PREFIX,
                n,
                TRIAGE,
                json!({"emails": input}),
            )]);
        };
        let by = |k: i64| emails.iter().find(|e| e["email"].as_i64() == Some(k));
        let mut table = String::from(
            "| From | Subject | Why | Suggested reply | message_id |\n|---|---|---|---|---|\n",
        );
        let needs = tri["needs_reply"].as_array().cloned().unwrap_or_default();
        for r in &needs {
            let Some(e) = r["email"].as_i64().and_then(by) else {
                continue;
            };
            table.push_str(&format!(
                "| {} | {}{} | {} | {} | {} |\n",
                cell(&get_str(e, "from")),
                cell(&get_str(e, "subject")),
                if e["injection_warning"] == json!(true) {
                    " ⚠️ suspicious"
                } else {
                    ""
                },
                cell(&get_str(r, "why")),
                cell(&get_str(r, "reply")),
                get_str(e, "message_id")
            ));
        }
        let mut md = if needs.is_empty() {
            "**Needs a reply:** nothing.\n".to_string()
        } else {
            format!("**Needs a reply**\n\n{table}")
        };
        let fyi: Vec<String> = tri["fyi"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|r| {
                let e = r["email"].as_i64().and_then(by)?;
                Some(format!(
                    "- {} — {}",
                    cell(&get_str(e, "from")),
                    cell(&get_str(r, "line"))
                ))
            })
            .collect();
        if !fyi.is_empty() {
            md.push_str(&format!(
                "\n**For your information**\n\n{}\n",
                fyi.join("\n")
            ));
        }
        show.push(S::block(&md));
        out["summary"] = json!(get_str(&tri, "summary"));
        out["needs_reply"] = json!(needs.len());
        out["fyi"] = json!(fyi.len());
        out["brief"] = json!(md);
    }
    // 4. the schedule asked for, with the task's JSON (never from a schedule's own run)
    let sched = args
        .get("schedule")
        .filter(|s| s.is_object() && !m.scheduled);
    let made = results_of(t, "schedule_create");
    if let (Some(s), None) = (sched, made.last()) {
        let name = "Email brief";
        return Step::Calls(vec![workflow::code_call(
            PREFIX,
            n,
            "schedule_create",
            json!({"name": name, "kind": "cron", "spec": sget(s, "spec"), "goal": name,
                   "task": longtask::task_call(EMAIL_BRIEF, args)}),
        )]);
    }
    if let (Some(s), Some((_, sm))) = (sched, made.last()) {
        if failed(sm) && !content_of(sm).contains("already exists") {
            show.push(S::fact(
                "Not scheduled",
                "未定时",
                take_chars(content_of(sm).trim(), 200),
            ));
        } else {
            show.push(S::fact(
                "Scheduled",
                "已定时",
                &format!("the email brief {}", sget(s, "every")),
            ));
            out["schedule"] = s.clone();
        }
    }
    // 5. mailed to the user's own address on a schedule's run, or when asked
    let mailbox = get_str(m.profile, "mailbox");
    let mails = results_of(t, "gmail_send");
    let send = (m.scheduled || args.get("email").is_some_and(truthy)) && !emails.is_empty();
    if send && !mailbox.is_empty() && mails.is_empty() {
        let body = format!("{}\n\n{}", get_str(&out, "summary"), get_str(&out, "brief"));
        return Step::Calls(vec![workflow::code_call(
            PREFIX,
            n,
            "gmail_send",
            json!({"to": mailbox, "subject": "Your email brief", "body": body.trim()}),
        )]);
    }
    match mails.last() {
        Some((a, mm)) if !failed(mm) => {
            show.push(S::fact("Emailed to", "已发送到", &sget(a, "to")));
            out["mailed"] = json!(sget(a, "to"));
        }
        Some((_, mm)) => show.push(S::fact(
            "Not emailed",
            "邮件未发送",
            take_chars(content_of(mm).trim(), 200),
        )),
        None => {}
    }
    // 6. the schedule offer (a chat run only, when none was asked for)
    if let Some(o) = m.offer.as_ref().filter(|_| sched.is_none()) {
        show.push(longtask::offer_item(o));
        out[longtask::OFFER] = o.clone();
        out["next_question"] = o["question"].clone();
    }
    out["show"] = json!(show);
    out["hints"] = json!([]);
    Step::Done(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn valid(c: Checked) -> Value {
        match c {
            Checked::Valid(v) => v,
            other => panic!("{other:?}"),
        }
    }

    fn answer(name: &str, a: &Value) -> String {
        match name {
            "gmail_search" => json!({"messages": [
                {"id": "m1", "subject": "Contract question", "from": "ana@acme.test"},
                {"id": "m2", "subject": "Weekly digest", "from": "news@digest.test"}]}).to_string(),
            "gmail_get_message" if sget(a, "message_id") == "m1" => json!({"id": "m1", "from": "Ana <ana@acme.test>",
                "subject": "Contract question", "date": "Sat, 7 Nov 2026", "body": "Can you confirm clause 4 by Monday?"}).to_string(),
            "gmail_get_message" => json!({"id": "m2", "from": "news@digest.test", "subject": "Weekly digest",
                "body": "This week in tech"}).to_string(),
            "subagent_json" => {
                assert_eq!(sget(a, "kind"), TRIAGE);
                assert_eq!(a["input"]["emails"].as_array().unwrap().len(), 2);
                json!({"kind": TRIAGE, "answer": triage_check(&a["input"], &json!({
                    "summary": "One question from Acme.",
                    "needs_reply": [{"email": 1, "why": "Ana needs clause 4 confirmed by Monday", "reply": "Yes, clause 4 is fine."}],
                    "fyi": [{"email": 2, "line": "a tech digest"}, {"email": 9, "line": "made up"}]})).unwrap()}).to_string()
            }
            "schedule_create" => "已创建定时任务 schedule created: id=sch_1, next run 2026-11-09 08:00".into(),
            "gmail_send" => json!({"sent": true}).to_string(),
            other => panic!("unexpected {other}"),
        }
    }

    fn run(args: &Value, m: &Machine) -> (Vec<Value>, Value) {
        let mut t: Vec<Value> = Vec::new();
        for _ in 0..10 {
            match next(args, &t, m) {
                Step::Calls(c) => {
                    t.push(super::super::runtime::assistant_with_calls("", &c));
                    for call in &c {
                        assert!(call.id.starts_with(PREFIX));
                        t.push(json!({"role": "tool", "tool_call_id": call.id, "content": answer(&call.name, &call.args)}));
                    }
                }
                Step::Done(v) => return (t, v),
            }
        }
        panic!("did not end")
    }

    fn names(t: &[Value]) -> Vec<String> {
        t.iter()
            .flat_map(tool_calls)
            .map(|tc| super::super::util::stored_call(tc).0)
            .collect()
    }

    #[test]
    fn the_brief_is_one_search_one_read_turn_one_subagent_and_a_table() {
        let p = json!({"mailbox": "me@x.test"});
        let args = valid(check(&json!({"period": "today"})));
        assert_eq!(args["period"], "day");
        let offer = longtask::offer(EMAIL_BRIEF, &args);
        let m = Machine {
            profile: &p,
            scheduled: false,
            first: 0,
            offer: offer.clone(),
        };
        let (t, r) = run(&args, &m);
        assert_eq!(
            names(&t),
            [
                "gmail_search",
                "gmail_get_message",
                "gmail_get_message",
                "subagent_json"
            ]
        );
        let q = t
            .iter()
            .flat_map(tool_calls)
            .map(|tc| super::super::util::stored_call(tc).1)
            .next()
            .unwrap();
        assert!(sget(&q, "query").starts_with("in:inbox newer_than:1d -category:promotions"));
        let md = r["show"][0]["value"].as_str().unwrap();
        assert!(md.contains("| Ana <ana@acme.test> | Contract question | Ana needs clause 4 confirmed by Monday | Yes, clause 4 is fine. | m1 |"), "{md}");
        assert!(md.contains("- news@digest.test — a tech digest") && !md.contains("made up"));
        // asked in the chat: nothing mailed, the schedule offered
        assert!(r.get("mailed").is_none());
        assert_eq!(r[longtask::OFFER]["every"], "every weekday at 08:00");
    }

    #[test]
    fn every_weekday_at_8_makes_the_schedule_with_the_task_and_a_scheduled_run_mails_the_brief() {
        let p = json!({"mailbox": "me@x.test"});
        let args = valid(check(
            &json!({"schedule": {"frequency": "weekdays", "time": "08:00"}}),
        ));
        let m = Machine {
            profile: &p,
            scheduled: false,
            first: 0,
            offer: longtask::offer(EMAIL_BRIEF, &args),
        };
        let (t, r) = run(&args, &m);
        let made = t
            .iter()
            .flat_map(tool_calls)
            .map(super::super::util::stored_call)
            .find(|(n, _)| n == "schedule_create")
            .unwrap()
            .1;
        assert_eq!(made["spec"], "0 8 * * 1-5");
        assert_eq!(
            made["task"],
            json!({"tool": "email_brief", "args": {"period": "day"}})
        );
        assert!(
            r.get(longtask::OFFER).is_none(),
            "asked for a schedule: none offered"
        );
        // the schedule's run: mailed to the user's own address, no schedule, no offer
        let m = Machine {
            profile: &p,
            scheduled: true,
            first: 0,
            offer: None,
        };
        let (t, r) = run(&made["task"]["args"], &m);
        assert_eq!(names(&t).last().unwrap(), "gmail_send");
        assert_eq!(r["mailed"], "me@x.test");
    }

    #[test]
    fn the_triage_default_lists_every_email_for_information() {
        let input = json!({"emails": [{"email": 1, "subject": "A"}, {"email": 2, "subject": "B"}]});
        assert!(triage_check(&input, &json!("prose")).is_err());
        assert!(triage_check(&input, &json!({"needs_reply": [{"email": 7}], "fyi": []})).is_err());
        assert_eq!(triage_fallback(&input)["fyi"].as_array().unwrap().len(), 2);
    }
}
