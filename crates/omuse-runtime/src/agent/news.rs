//! The news research machine behind the workflow tools `news_gather`, `news_publish` and `news_mail`
//! (docs/json-tools.md; the protocol is [`super::workflow`]).
//!
//! The model reads the user's words and calls `news_gather` with JSON arguments (the subject, the window). [`check`]
//! validates them; then every step is code ([`next`]): ONE browser news search of the subject (a second, wider one only
//! when the first finds no article), the first [`ARTICLES`] distinct links of the results read in one turn, and a JSON
//! result with the articles (number, title, publication, date, url, text excerpt). The model writes the edition as
//! JSON from that result and calls `news_publish` (a title, one story per article by its number — headline and
//! summary —, insights, the schedule, whether to email it). [`check`] keeps only stories whose article number exists,
//! takes each story's link, publication and date from the gathered articles (never from the model) and adds every
//! article the model left out with its own headline and opening. The machine publishes with `news_report` (the
//! newspaper HTML and PDF under `/artifacts/`, `crate::newsreport`), makes the schedule the arguments ask for
//! (`schedule_create`, the goal written by code so that the scheduled run calls `news_gather` → `news_publish` again)
//! and mails the edition to the user's own connected address when asked. `news_mail` mails an edition published earlier
//! in the chat (its links, which the model copies from the earlier reply). Each run ends with [`Step::Done`]: a JSON
//! result with `must_say`; the model writes the reply from it. Nothing here reads the user's wording or calls a model.
//!
//! As in [`super::booking`], the next step is a function of the validated arguments and the transcript after the tool
//! call alone, so a run suspended for a takeover carries on where it was.

use std::sync::LazyLock;

use chrono::{Duration, NaiveDate};
use omuse_common::util::{get_str, take_chars, truthy};
use regex::Regex;
use serde_json::{json, Map, Value};

use super::driver::{json_of, Step};
use super::tools::fn_;
use super::util::{content_of, sget, stored_call, tool_calls};
use super::workflow::{self, Checked};
use crate::newsreport::{pick_articles, Ask};

/// The skill whose run asks through these tools.
pub const SKILL: &str = "news-research";
/// The id prefix of the machine's code calls.
pub const PREFIX: &str = "nws_";
pub const NEWS_GATHER: &str = "news_gather";
pub const NEWS_PUBLISH: &str = "news_publish";
pub const NEWS_MAIL: &str = "news_mail";
/// The workflow tools this machine runs.
pub const TOOLS: [&str; 3] = [NEWS_GATHER, NEWS_PUBLISH, NEWS_MAIL];
/// The layout tool the machine publishes with (code's own call; not offered to the model).
pub const REPORT_TOOL: &str = "news_report";
/// How many articles an edition reads (two `browser_read` calls of at most four pages, in one turn).
pub const ARTICLES: usize = 6;
/// `browser_read` takes at most four pages per call.
const READ_BATCH: usize = 4;
/// How much of each article the gathered result carries.
const ARTICLE_CHARS: usize = 3000;

/// The catalog entries of the workflow tools (JSON schemas; the descriptions say what the tool does).
pub fn tool_schemas() -> Vec<Value> {
    let s = json!({"type": "string"});
    let gather = fn_(
        NEWS_GATHER,
        "Research the latest news on a subject for a newspaper-style report (also a scheduled edition). Code runs the \
browser's news search, reads the top articles and returns them as JSON (article number, title, publication, date, url, \
text). Then write the edition from those articles and call news_publish. status nothing_found: no recent article — say \
so; needs_input: ask the user for the subject.",
        json!({
            "subject": {"type": "string", "description": "what the news is about — a company, topic, place or person — without words such as 'latest news', 'report' or 'weekly'"},
            "window": {"type": "string", "enum": ["day", "week", "month"], "description": "how far back the news goes: day for a daily report, month for a monthly one, else week"}
        }),
        &["subject"],
    );
    let publish = fn_(
        NEWS_PUBLISH,
        "Publish the edition written from news_gather's articles. Code adds each story's link, publication and date from \
the gathered articles, lays out the newspaper (masthead, lead, stories, sources) as HTML and PDF under /artifacts/, makes \
the schedule asked for and emails the edition to the user's own address when asked; the JSON result has the links, the \
schedule and the mail. Write the reply from it, with every fact in its must_say.",
        json!({
            "title": {"type": "string", "description": "the paper's name, short (e.g. \"The Tidal Weekly\")"},
            "stories": {"type": "array", "description": "one per article, every article once, the most important first (it is the lead)",
                "items": {"type": "object", "properties": {
                    "article": {"type": "integer", "description": "the article's number in news_gather's result"},
                    "headline": {"type": "string", "description": "in your own words"},
                    "summary": {"type": "string", "description": "2-4 sentences in your own words that keep the article's numbers, amounts, names, places and dates"}}}},
            "insights": {"type": "array", "items": s.clone(), "description": "3-5 short points across the stories (trends, what changed, what to watch)"},
            "schedule": {"type": "string", "enum": ["once", "daily", "weekly", "monthly"], "description": "a new edition on a timer when the user asked for one (\"every week\" = weekly); once = no schedule"},
            "weekday": {"type": "integer", "description": "weekly: the day the user named, 0 = Sunday … 6 = Saturday (default Monday)"},
            "hour": {"type": "integer", "description": "the hour (0-23) the user named for the editions (default 8)"},
            "email": {"type": "boolean", "description": "the user wants the edition (or each new edition) sent to them — by email, or \"send me\" a report / a new edition every week: it goes to their own connected address"}
        }),
        &["stories"],
    );
    let mail = fn_(
        NEWS_MAIL,
        "Email a news edition published earlier in this chat to the user's own connected address, with the PDF and the \
HTML attached (\"email it\" after a report). The links are the /artifacts/news/… ones of the earlier reply. Write the \
reply from the JSON result.",
        json!({
            "files": {"type": "array", "items": s.clone(), "description": "the edition's links from the earlier reply (/artifacts/news/….pdf and .html)"},
            "title": {"type": "string", "description": "the edition's title from the earlier reply"}
        }),
        &["files"],
    );
    vec![gather, publish, mail]
}

/// `ask_user` in a news-research run before its workflow tool was called: refused, as data (the tool's `needs_input`
/// says what only the user can give; the window and the schedule have defaults). `events` are the task's.
pub fn ask_before_tool(events: &[Value]) -> Option<Value> {
    let loaded = events
        .iter()
        .filter(|e| get_str(e, "type") == "tool_call")
        .filter(|e| get_str(&e["data"], "name") == "load_skill")
        .any(|e| get_str(&e["data"]["args"], "name") == SKILL);
    let called = events.iter().any(|e| get_str(e, "type") == "workflow_call");
    (loaded && !called).then(|| {
        json!({"refused": "call_the_tool_first", "tool": NEWS_GATHER,
               "why": format!("the {SKILL} skill asks through {NEWS_GATHER}: call it with the subject the user gave; its needs_input lists what only the user can give")})
    })
}

/// Whether the model is offered tool `name` on its next turn: `news_report` is code's own call; on the reply to a
/// machine's result the news tools are withdrawn, except `news_publish` right after the articles were gathered.
pub fn offered(transcript: &[Value], name: &str) -> bool {
    if name == REPORT_TOOL {
        return false;
    }
    if !TOOLS.contains(&name) || !workflow::reply_turn(transcript) {
        return true;
    }
    name == NEWS_PUBLISH
        && last_result_name(transcript).as_deref() == Some("news_gather_result")
        && gathered(transcript).is_some()
}

fn last_result_name(transcript: &[Value]) -> Option<String> {
    transcript
        .iter()
        .flat_map(tool_calls)
        .filter(|tc| get_str(tc, "id").starts_with(workflow::RESULT_PREFIX))
        .map(|tc| stored_call(tc).0)
        .next_back()
}

/// The latest `news_gather` result with articles (`status: gathered`) in the transcript.
pub fn gathered(transcript: &[Value]) -> Option<Value> {
    let want = format!("{NEWS_GATHER}{}", workflow::RESULT_SUFFIX);
    let id = transcript
        .iter()
        .flat_map(tool_calls)
        .filter(|tc| {
            get_str(tc, "id").starts_with(workflow::RESULT_PREFIX) && stored_call(tc).0 == want
        })
        .map(|tc| get_str(tc, "id"))
        .next_back()?;
    transcript
        .iter()
        .find(|m| get_str(m, "role") == "tool" && get_str(m, "tool_call_id") == id)
        .map(|m| serde_json::from_str::<Value>(&content_of(m)).unwrap_or(Value::Null))
        .filter(|v| get_str(v, "status") == "gathered")
}

// ---------------------------------------------------------------- validation (code, on the JSON)

/// What validation reads besides the arguments.
pub struct Ctx<'a> {
    /// the profile, with the connected mailbox's address as `mailbox` when one is connected
    pub profile: &'a Value,
    /// the latest `news_gather` result with articles ([`gathered`])
    pub gathered: Option<&'a Value>,
}

/// [`check`] with its context read from the transcript.
pub fn check_in(tool: &str, args: &Value, transcript: &[Value], profile: &Value) -> Checked {
    let g = gathered(transcript);
    check(
        tool,
        args,
        &Ctx {
            profile,
            gathered: g.as_ref(),
        },
    )
}

/// Validates a workflow tool's arguments: the arguments to run with (normalised), the problems the model can fix, or
/// the fields only the user can give.
pub fn check(tool: &str, args: &Value, cx: &Ctx) -> Checked {
    match tool {
        NEWS_PUBLISH => check_publish(args, cx),
        NEWS_MAIL => check_mail(args, cx),
        _ => check_gather(args),
    }
}

fn s_arg(a: &Value, k: &str) -> String {
    match a.get(k) {
        Some(Value::String(s)) => s.trim().to_string(),
        Some(Value::Number(n)) => n.to_string(),
        _ => String::new(),
    }
}

fn i_arg(a: &Value, k: &str) -> Option<i64> {
    match a.get(k)? {
        Value::Number(n) => n.as_i64(),
        Value::String(s) => s.trim().parse().ok(),
        _ => None,
    }
}

/// The window's days: 1 for a day, 30 for a month, else 7.
pub fn window_days(window: &str) -> i64 {
    match window {
        "day" => 1,
        "month" => 30,
        _ => 7,
    }
}

fn check_gather(a: &Value) -> Checked {
    let subject = s_arg(a, "subject")
        .trim_matches(|c: char| matches!(c, '"' | '\'' | '“' | '”' | '‘' | '’'))
        .trim()
        .to_string();
    if subject.is_empty() {
        return Checked::NeedsInput(vec!["subject".into()]);
    }
    let window = match s_arg(a, "window").to_lowercase().as_str() {
        "day" | "daily" => "day",
        "month" | "monthly" => "month",
        _ => "week",
    };
    Checked::Valid(json!({"subject": take_chars(&subject, 120), "window": window}))
}

fn check_publish(a: &Value, cx: &Ctx) -> Checked {
    let Some(g) = cx.gathered else {
        return Checked::Invalid(vec![format!(
            "no articles gathered: call {NEWS_GATHER} first, then {NEWS_PUBLISH} with the stories written from its articles"
        )]);
    };
    let arts = g
        .get("articles")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let n = arts.len();
    let mut taken: Vec<(usize, String, String)> = Vec::new();
    let mut problems: Vec<String> = Vec::new();
    for (i, s) in a
        .get("stories")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .enumerate()
    {
        let k = i_arg(s, "article").unwrap_or(0);
        if k < 1 || k as usize > n {
            problems.push(format!(
                "stories[{i}].article: {k} is not an article of {NEWS_GATHER} (1–{n})"
            ));
            continue;
        }
        let idx = k as usize - 1;
        if taken.iter().any(|(j, _, _)| *j == idx) {
            problems.push(format!("stories[{i}].article: article {k} twice"));
            continue;
        }
        let headline = take_chars(&s_arg(s, "headline"), 200).to_string();
        let summary = take_chars(&s_arg(s, "summary"), 1500).to_string();
        if summary.is_empty() {
            problems.push(format!("stories[{i}].summary: empty"));
            continue;
        }
        taken.push((idx, headline, summary));
    }
    if taken.is_empty() {
        problems.insert(
            0,
            format!("stories: none names an article of {NEWS_GATHER} (1–{n}) with a summary"),
        );
        return Checked::Invalid(problems);
    }
    // every article read is in the edition: one the model left out gets its own headline and opening
    for i in 0..n {
        if !taken.iter().any(|(j, _, _)| *j == i) {
            taken.push((i, String::new(), String::new()));
        }
    }
    let stories: Vec<Value> = taken
        .into_iter()
        .map(|(i, headline, summary)| {
            let art = &arts[i];
            let (headline, summary) = if summary.is_empty() {
                (get_str(art, "title"), opening(art))
            } else if headline.is_empty() {
                (get_str(art, "title"), summary)
            } else {
                (headline, summary)
            };
            json!({"article": i + 1, "headline": headline, "summary": summary,
                   "source": get_str(art, "publication"), "url": get_str(art, "url"), "date": get_str(art, "date")})
        })
        .collect();
    let insights: Vec<String> = a
        .get("insights")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(|s| take_chars(s.trim(), 600).to_string())
        .filter(|s| !s.is_empty())
        .take(6)
        .collect();
    let schedule = match s_arg(a, "schedule").to_lowercase().as_str() {
        s @ ("daily" | "weekly" | "monthly") => s.to_string(),
        _ => "once".to_string(),
    };
    let mut out = Map::new();
    out.insert("subject".into(), json!(get_str(g, "subject")));
    out.insert("period".into(), json!(get_str(g, "period")));
    out.insert("window".into(), json!(get_str(g, "window")));
    out.insert("title".into(), json!(take_chars(&s_arg(a, "title"), 60)));
    out.insert("stories".into(), json!(stories));
    out.insert("insights".into(), json!(insights));
    out.insert("schedule".into(), json!(schedule));
    out.insert(
        "weekday".into(),
        json!(i_arg(a, "weekday")
            .filter(|d| (0..=6).contains(d))
            .unwrap_or(1)),
    );
    out.insert(
        "hour".into(),
        json!(i_arg(a, "hour")
            .filter(|h| (0..=23).contains(h))
            .unwrap_or(8)),
    );
    out.insert("email".into(), json!(a.get("email").is_some_and(truthy)));
    Checked::Valid(Value::Object(out))
}

/// An edition's file as a workspace path (`artifacts/news/…​.pdf|.html`), from a link the model copied (`/artifacts/…`,
/// a full URL on this host, or the path itself); `None` for anything else.
fn edition_path(link: &str) -> Option<String> {
    static PATH: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"^artifacts/news/[A-Za-z0-9._/-]+\.(?:pdf|html)$").unwrap());
    let l = link.trim();
    let i = l.find("artifacts/news/")?;
    let p = l[i..].split(['?', '#']).next().unwrap_or("");
    (PATH.is_match(p) && !p.contains("..")).then(|| p.to_string())
}

fn check_mail(a: &Value, cx: &Ctx) -> Checked {
    let raw: Vec<String> = match a.get("files") {
        Some(Value::Array(v)) => v
            .iter()
            .filter_map(Value::as_str)
            .map(str::to_string)
            .collect(),
        Some(Value::String(s)) => s.split([',', ' ']).map(str::to_string).collect(),
        _ => Vec::new(),
    };
    let mut files: Vec<String> = Vec::new();
    for f in raw.iter().filter_map(|r| edition_path(r)) {
        if !files.contains(&f) {
            files.push(f);
        }
    }
    if files.is_empty() {
        return Checked::Invalid(vec![
            "files: the edition's /artifacts/news/….pdf and .html links from the earlier reply"
                .into(),
        ]);
    }
    let to = get_str(cx.profile, "mailbox");
    if to.is_empty() {
        return Checked::NeedsInput(vec![
            "a connected mailbox (the user connects one on the Connections page)".into(),
        ]);
    }
    files.sort_by_key(|p| !p.ends_with(".pdf")); // the PDF first
    Checked::Valid(json!({"files": files, "title": take_chars(&s_arg(a, "title"), 80), "to": to}))
}

// ---------------------------------------------------------------- the articles (structured results and pages)

/// The machine's own calls of `tool` in `t`, each with its result message.
fn own<'a>(t: &'a [Value], tool: &str) -> Vec<(Value, &'a Value)> {
    let mut out = Vec::new();
    for tc in t.iter().flat_map(tool_calls) {
        let id = get_str(tc, "id");
        let (name, args) = stored_call(tc);
        if name != tool || !id.starts_with(PREFIX) {
            continue;
        }
        if let Some(m) = t
            .iter()
            .find(|m| get_str(m, "role") == "tool" && get_str(m, "tool_call_id") == id)
        {
            out.push((args, m));
        }
    }
    out
}

fn failed(m: &Value) -> bool {
    let c = content_of(m);
    c.starts_with("ERROR") || c.starts_with("DENIED")
}

/// A search engine's ad or redirect link: no article.
fn ad_link(url: &str) -> bool {
    let lc = url.to_lowercase();
    lc.contains("duckduckgo.com/y.js")
        || lc.contains("ad_domain=")
        || lc.contains("bing.com/aclick")
        || lc.contains("google.com/aclk")
        || lc.contains("ad_provider=")
}

/// The rows of the machine's news searches, in the engines' order.
fn search_rows(t: &[Value]) -> Vec<Value> {
    own(t, "browser_news_search")
        .into_iter()
        .flat_map(|(_, m)| {
            let v = json_of(m);
            v.get("results")
                .or_else(|| v.get("content").and_then(|c| c.get("results")))
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default()
        })
        .collect()
}

/// One article of the edition: what the search result and the page read say about it.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Article {
    pub url: String,
    pub title: String,
    pub source: String,
    pub date: String,
    pub snippet: String,
    pub text: String,
    /// the article's first paragraph, read by its element (`browser_read`'s `lead`)
    pub lead: String,
}

/// A date as an edition shows it: an ISO date or timestamp ("2026-10-31T09:00:00.0000000") as "31 October 2026"; any
/// other text (an engine's "2 days ago") as it is.
fn shown_date(s: &str) -> String {
    let s = s.trim();
    match s
        .get(..10)
        .and_then(|d| NaiveDate::parse_from_str(d, "%Y-%m-%d").ok())
    {
        Some(d) => d.format("%-d %B %Y").to_string(),
        None => s.to_string(),
    }
}

fn host_of(u: &str) -> String {
    let rest = u.split("://").nth(1).unwrap_or(u);
    let host = rest.split(['/', '?', '#']).next().unwrap_or("");
    let host = host.split(':').next().unwrap_or("");
    host.trim_start_matches("www.").to_string()
}

/// The articles to read: the first [`ARTICLES`] distinct http(s) links of the search results (no ads).
pub fn articles(t: &[Value]) -> Vec<Article> {
    let rows: Vec<Value> = search_rows(t)
        .into_iter()
        .filter(|r| !ad_link(&get_str(r, "url")))
        .collect();
    pick_articles(&rows, ARTICLES)
        .into_iter()
        .map(|r| {
            // the engine's structured fields: the publication (Bing News' card author), the date (DuckDuckGo's
            // timestamp, Bing News' age)
            let date = get_str(&r, "date");
            Article {
                title: get_str(&r, "title").trim().to_string(),
                source: get_str(&r, "source").trim().to_string(),
                date: shown_date(
                    if date.trim().is_empty() {
                        get_str(&r, "age")
                    } else {
                        date
                    }
                    .as_str(),
                ),
                snippet: get_str(&r, "snippet"),
                url: get_str(&r, "url"),
                text: String::new(),
                lead: String::new(),
            }
        })
        .collect()
}

/// One page the machine's reads returned, with its parts read by their elements.
struct PageRead {
    url: String,
    title: String,
    text: String,
    lead: String,
    site_name: String,
    published: String,
}

/// The pages the machine's reads returned.
fn pages_read(t: &[Value]) -> Vec<PageRead> {
    own(t, "browser_read")
        .into_iter()
        .flat_map(|(_, m)| {
            json_of(m)
                .get("pages")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default()
        })
        .filter(|p| p.get("error").is_none() && p.get("blocked").is_none())
        .map(|p| PageRead {
            url: get_str(&p, "url"),
            title: get_str(&p, "title"),
            text: get_str(&p, "text"),
            lead: get_str(&p, "lead"),
            site_name: get_str(&p, "site_name"),
            published: get_str(&p, "published"),
        })
        .collect()
}

/// The articles with the text of their pages (the snippet when a page could not be read), the publication and the
/// date filled from the page's elements when the search result had none (`og:site_name`, else the page title's
/// "Headline | Publication", else the host; the published time).
pub fn articles_read(t: &[Value]) -> Vec<Article> {
    let pages = pages_read(t);
    articles(t)
        .into_iter()
        .map(|mut a| {
            let same = |u: &str| u.trim_end_matches('/') == a.url.trim_end_matches('/');
            if let Some(p) = pages.iter().find(|p| same(&p.url)) {
                a.text = p.text.trim().to_string();
                a.lead = p.lead.trim().to_string();
                if a.source.is_empty() {
                    a.source = p.site_name.trim().to_string();
                }
                if a.date.is_empty() {
                    a.date = shown_date(&p.published);
                }
                if a.source.is_empty() {
                    // the page title's "Headline | Publication"
                    if let Some((_, s)) = p
                        .title
                        .rsplit_once(" | ")
                        .or_else(|| p.title.rsplit_once(" - "))
                    {
                        a.source = s.trim().to_string();
                    }
                }
            }
            if a.text.is_empty() {
                a.text = a.snippet.clone();
            }
            if a.source.is_empty() {
                a.source = host_of(&a.url);
            }
            a
        })
        .collect()
}

/// A gathered article's own opening, for an article the model wrote no story for: its first paragraph as its element
/// gives it (`lead`), else its text without the headline line, cut at a sentence end (at most ~600 characters).
pub fn opening(art: &Value) -> String {
    let title = get_str(art, "title");
    let lead = get_str(art, "lead");
    let text = if lead.trim().is_empty() {
        get_str(art, "text")
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty() && *l != title)
            .collect::<Vec<_>>()
            .join(" ")
    } else {
        lead
    };
    let text = text.trim_end_matches('…').trim().to_string();
    if text.chars().count() <= 600 {
        return text;
    }
    let cut = take_chars(&text, 600);
    match cut.rfind(['.', '!', '?']) {
        Some(i) if i > 100 => cut[..=i].to_string(),
        _ => format!("{}…", cut.trim_end()),
    }
}

/// The edition's period line: "1 November 2026" for a day, else "26 October 2026 – 1 November 2026".
pub fn period(days: i64, today: NaiveDate) -> String {
    let day = |d: NaiveDate| d.format("%-d %B %Y").to_string();
    if days <= 1 {
        day(today)
    } else {
        format!("{} – {}", day(today - Duration::days(days - 1)), day(today))
    }
}

// ---------------------------------------------------------------- the machine

/// What the machine reads besides the validated arguments and the transcript.
pub struct Machine<'a> {
    /// the profile, with the connected mailbox's address as `mailbox` when one is connected
    pub profile: &'a Value,
    pub today: NaiveDate,
    /// this run is a schedule's (no new schedule is made)
    pub scheduled: bool,
    /// the user's language is Chinese (the mail's body)
    pub zh: bool,
    /// the machine's code calls in the transcript before this run's tool call: the ids go on from there
    pub first: usize,
}

fn next_id(t: &[Value], first: usize) -> usize {
    first
        + t.iter()
            .flat_map(tool_calls)
            .filter(|tc| get_str(tc, "id").starts_with(PREFIX))
            .count()
        + 1
}

/// The next step of a run of workflow tool `tool` with its validated arguments `args`; `t` is the transcript from the
/// tool call on.
pub fn next(tool: &str, args: &Value, t: &[Value], m: &Machine) -> Step {
    let n = next_id(t, m.first);
    match tool {
        NEWS_PUBLISH => next_publish(args, t, m, n),
        NEWS_MAIL => next_mail(args, t, n),
        _ => next_gather(args, t, m, n),
    }
}

fn next_gather(args: &Value, t: &[Value], m: &Machine, n: usize) -> Step {
    let subject = sget(args, "subject");
    let window = match sget(args, "window").as_str() {
        w @ ("day" | "month") => w.to_string(),
        _ => "week".to_string(),
    };
    let days = window_days(&window);
    let period_line = period(days, m.today);
    // 1. the search: the subject; a second, wider one only when the first found no article
    let searches = own(t, "browser_news_search");
    let arts = articles(t);
    if arts.is_empty() {
        let wider = if subject.to_lowercase().contains("news") {
            subject.clone()
        } else {
            format!("{subject} news")
        };
        return match searches.len() {
            0 => Step::Calls(vec![workflow::code_call(
                PREFIX,
                n,
                "browser_news_search",
                json!({"query": subject, "days": days, "max_results": 10}),
            )]),
            1 => Step::Calls(vec![workflow::code_call(
                PREFIX,
                n,
                "browser_news_search",
                json!({"query": wider, "days": days.max(30), "max_results": 10}),
            )]),
            _ => {
                let searched: Vec<String> =
                    searches.iter().map(|(a, _)| sget(a, "query")).collect();
                Step::Done(json!({
                    "status": "nothing_found", "subject": subject, "window": window, "period": period_line,
                    "searched": searched,
                    "must_say": [format!("the browser's news search found no recent articles on {subject}, so no edition was published")]
                }))
            }
        };
    }
    // 2. the articles, read in one turn
    if own(t, "browser_read").is_empty() {
        let urls: Vec<String> = arts.iter().map(|a| a.url.clone()).collect();
        return Step::Calls(
            urls.chunks(READ_BATCH)
                .enumerate()
                .map(|(j, c)| {
                    workflow::code_call(PREFIX, n + j, "browser_read", json!({"urls": c}))
                })
                .collect(),
        );
    }
    // 3. the articles as JSON, for the model to write the edition from
    let list: Vec<Value> = articles_read(t)
        .iter()
        .enumerate()
        .map(|(i, a)| {
            json!({"article": i + 1, "title": a.title, "publication": a.source, "date": a.date, "url": a.url,
                   "lead": a.lead, "text": take_chars(&a.text, ARTICLE_CHARS)})
        })
        .collect();
    Step::Done(json!({
        "status": "gathered", "subject": subject, "window": window, "period": period_line, "articles": list,
        "next_tool": NEWS_PUBLISH, "must_say": []
    }))
}

/// The `news_report` call's arguments from the validated `news_publish` arguments.
fn report_args(args: &Value) -> Value {
    let stories: Vec<Value> = args
        .get("stories")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .map(|s| {
            json!({"headline": get_str(s, "headline"), "summary": get_str(s, "summary"), "source": get_str(s, "source"),
                   "url": get_str(s, "url"), "date": get_str(s, "date")})
        })
        .collect();
    json!({"subject": sget(args, "subject"), "title": sget(args, "title"), "period": sget(args, "period"),
           "stories": stories, "insights": args.get("insights").cloned().unwrap_or_else(|| json!([]))})
}

/// The request's schedule as `newsreport::Ask` (`None`: a one-off edition).
fn ask_of(args: &Value) -> Option<Ask> {
    let schedule = sget(args, "schedule");
    if !matches!(schedule.as_str(), "daily" | "weekly" | "monthly") {
        return None;
    }
    Ask::from_json(
        &json!({"subject": sget(args, "subject"), "frequency": schedule,
                           "weekday": args.get("weekday"), "hour": args.get("hour"),
                           "email": args.get("email").is_some_and(truthy)}),
    )
}

/// "every Monday at 08:00", "every day at 08:00", "on the 1st of every month at 08:00".
fn cadence(ask: &Ask) -> String {
    const DAYS: [&str; 7] = [
        "Sunday",
        "Monday",
        "Tuesday",
        "Wednesday",
        "Thursday",
        "Friday",
        "Saturday",
    ];
    let at = format!("{:02}:00", ask.hour);
    match ask.frequency.as_str() {
        "daily" => format!("every day at {at}"),
        "monthly" => format!("on the 1st of every month at {at}"),
        _ => format!("every {} at {at}", DAYS[ask.weekday as usize % 7]),
    }
}

/// The scheduled run's goal (natural language for the model of that run): it calls `news_gather` with the same
/// subject and window, then `news_publish` — with `email: true` when the editions are mailed.
pub fn schedule_goal(subject: &str, window: &str, email: bool) -> String {
    format!(
        "News report on {subject}: research the latest news with the {SKILL} skill — {NEWS_GATHER} with subject \"{subject}\" \
and window \"{window}\" — and publish the newspaper edition with {NEWS_PUBLISH}{}",
        if email {
            " (email: true — email it to me)"
        } else {
            ""
        }
    )
}

fn next_publish(args: &Value, t: &[Value], m: &Machine, n: usize) -> Step {
    let subject = sget(args, "subject");
    // 1. published by code
    let reports = own(t, REPORT_TOOL);
    let Some((_, rm)) = reports.last() else {
        return Step::Calls(vec![workflow::code_call(
            PREFIX,
            n,
            REPORT_TOOL,
            report_args(args),
        )]);
    };
    if failed(rm) {
        let why = take_chars(content_of(rm).trim_start_matches("ERROR:").trim(), 200).to_string();
        return Step::Done(json!({
            "status": "error", "subject": subject, "why": why,
            "must_say": [format!("the report on {subject} could not be published: {why}")]
        }));
    }
    let rep = json_of(rm);
    let title = get_str(&rep, "title");
    let (html, pdf) = (get_str(&rep, "html_url"), get_str(&rep, "pdf_url"));
    // 2. the schedule the arguments ask for (never from a schedule's own run)
    let ask = ask_of(args).filter(|_| !m.scheduled);
    let schedules = own(t, "schedule_create");
    if let (Some(a), None) = (&ask, schedules.last()) {
        return Step::Calls(vec![workflow::code_call(
            PREFIX,
            n,
            "schedule_create",
            json!({"name": format!("News report: {subject}"), "kind": "cron", "spec": a.cron().unwrap_or_default(),
                   "goal": schedule_goal(&subject, &sget(args, "window"), a.email)}),
        )]);
    }
    // 3. the mail to the user's own address, when asked
    let mailbox = get_str(m.profile, "mailbox");
    let email = args.get("email").is_some_and(truthy);
    let mails = own(t, "gmail_send");
    if email && !mailbox.is_empty() && mails.is_empty() {
        let n_stories = rep.get("stories").and_then(Value::as_u64).unwrap_or(0);
        let body = if m.zh {
            format!("你的新一期《{title}》已经出来了（{n_stories} 条新闻）。PDF 和 HTML 版见附件。")
        } else {
            format!("Your new edition of {title} is ready ({n_stories} stories). The PDF and HTML editions are attached.")
        };
        let mut atts = vec![get_str(&rep, "pdf_path"), get_str(&rep, "html_path")];
        atts.retain(|x| !x.is_empty());
        return Step::Calls(vec![workflow::code_call(
            PREFIX,
            n,
            "gmail_send",
            json!({"to": mailbox, "subject": title, "body": body, "attachments": atts}),
        )]);
    }
    // 4. the result
    let stories: Vec<Value> = args
        .get("stories")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .map(|s| json!({"headline": get_str(s, "headline"), "publication": get_str(s, "source"), "url": get_str(s, "url")}))
        .collect();
    let mut must: Vec<String> = Vec::new();
    if let Some(lead) = stories.first() {
        must.push(format!(
            "the lead: {} ({})",
            get_str(lead, "headline"),
            get_str(lead, "publication")
        ));
    }
    must.push(format!("the HTML edition: {html}"));
    if !pdf.is_empty() {
        must.push(format!("the PDF edition: {pdf}"));
    }
    let mut out = json!({
        "status": "published", "subject": subject, "title": title, "period": sget(args, "period"),
        "stories": stories, "html_url": html, "pdf_url": pdf, "schedule": null, "mailed": null,
    });
    if pdf.is_empty() {
        out["pdf_error"] = json!(get_str(&rep, "pdf_error"));
    }
    if let (Some(a), Some((_, sm))) = (&ask, schedules.last()) {
        if failed(sm) {
            let why =
                take_chars(content_of(sm).trim_start_matches("ERROR:").trim(), 200).to_string();
            out["schedule"] = json!({"error": why});
            must.push(format!("the schedule could not be made: {why}"));
        } else {
            static NEXT: LazyLock<Regex> =
                LazyLock::new(|| Regex::new(r"next run ([0-9:\- ]+)").unwrap());
            let next_run = NEXT
                .captures(&content_of(sm))
                .map(|c| c[1].trim().to_string())
                .unwrap_or_default();
            let every = cadence(a);
            out["schedule"] =
                json!({"every": every, "spec": a.cron(), "next_run": next_run, "email": a.email});
            must.push(if next_run.is_empty() {
                format!("a new edition {every}")
            } else {
                format!("a new edition {every} — the next one {next_run}")
            });
        }
    }
    if email {
        match mails.last() {
            Some((ma, mm)) if !failed(mm) => {
                let to = sget(ma, "to");
                out["mailed"] = json!({"to": to});
                must.push(format!(
                    "this edition was emailed to {to} with the PDF attached"
                ));
            }
            Some((_, mm)) => {
                let why =
                    take_chars(content_of(mm).trim_start_matches("ERROR:").trim(), 200).to_string();
                out["mail_error"] = json!(why);
                must.push(format!("the edition could not be emailed: {why}"));
            }
            None => {
                out["mail_error"] = json!("no mailbox is connected");
                must.push("the edition was not emailed: no mailbox is connected (connect one on the Connections page)".into());
            }
        }
    } else if !mailbox.is_empty() && !m.scheduled {
        out["next_question"] = json!(format!("email this edition (PDF attached) to {mailbox}?"));
    }
    out["must_say"] = json!(must);
    Step::Done(out)
}

fn next_mail(args: &Value, t: &[Value], n: usize) -> Step {
    let files: Vec<String> = args
        .get("files")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::to_string)
        .collect();
    let to = sget(args, "to");
    let title = match sget(args, "title") {
        s if s.is_empty() => "Your news report".to_string(),
        s => s,
    };
    let mails = own(t, "gmail_send");
    let Some((_, mm)) = mails.last() else {
        return Step::Calls(vec![workflow::code_call(
            PREFIX,
            n,
            "gmail_send",
            json!({"to": to, "subject": title,
                   "body": format!("Here is the latest edition of {title}. The PDF and HTML editions are attached."),
                   "attachments": files}),
        )]);
    };
    if failed(mm) {
        let why = take_chars(content_of(mm).trim_start_matches("ERROR:").trim(), 200).to_string();
        return Step::Done(
            json!({"status": "error", "to": to, "files": files, "why": why,
                                 "must_say": [format!("the edition could not be emailed: {why}")]}),
        );
    }
    Step::Done(
        json!({"status": "mailed", "to": to, "files": files, "title": title,
                      "must_say": [format!("the edition was emailed to {to} with the PDF attached")]}),
    )
}

#[cfg(test)]
mod tests;
