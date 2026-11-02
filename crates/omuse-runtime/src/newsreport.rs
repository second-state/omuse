//! The news research report (`news_report`, skill `news-research`): the model researches and writes the stories and
//! insights; code checks them, lays them out in the skill's newspaper template (`skills/news-research/template.html`)
//! and decides the file names under `artifacts/` (served to the signed-in user at `/artifacts/…` by Sentinel).

use chrono::{DateTime, Local};
use omuse_common::util::{get_str, take_chars};
use serde_json::Value;

/// One story: a headline, what happened (the model's own words), and the article it comes from.
#[derive(Debug, Clone, PartialEq)]
pub struct Story {
    pub headline: String,
    pub summary: String,
    pub source: String,
    pub url: String,
    pub date: String,
}

/// A report as the model hands it over.
#[derive(Debug, Clone, PartialEq)]
pub struct Report {
    pub subject: String,
    pub title: String,
    pub period: String,
    pub stories: Vec<Story>,
    pub insights: Vec<String>,
}

fn story(v: &Value) -> Option<Story> {
    let s = Story {
        headline: take_chars(get_str(v, "headline").trim(), 200).to_string(),
        summary: take_chars(get_str(v, "summary").trim(), 1500).to_string(),
        source: take_chars(get_str(v, "source").trim(), 80).to_string(),
        url: get_str(v, "url").trim().to_string(),
        date: take_chars(get_str(v, "date").trim(), 40).to_string(),
    };
    // a story without its article is not reported: every story links to where it was read
    let linked = s.url.starts_with("https://") || s.url.starts_with("http://");
    (linked && !s.headline.is_empty() && !s.summary.is_empty()).then_some(s)
}

impl Report {
    /// From the tool's arguments: stories without a headline, a summary or an http(s) link are dropped, the same
    /// article is kept once; `Err` says what is missing.
    pub fn from_args(a: &Value) -> Result<Self, String> {
        let subject = take_chars(get_str(a, "subject").trim(), 120).to_string();
        if subject.is_empty() {
            return Err("subject is required (what the report is about)".into());
        }
        let mut stories: Vec<Story> = Vec::new();
        for s in a
            .get("stories")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(story)
        {
            if !stories.iter().any(|x| x.url == s.url) {
                stories.push(s);
            }
        }
        if stories.is_empty() {
            return Err(
                "no story with a headline, a summary and the article's url: give at least one"
                    .into(),
            );
        }
        let insights: Vec<String> = a
            .get("insights")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .map(|s| take_chars(s.trim(), 600).to_string())
            .filter(|s| !s.is_empty())
            .take(8)
            .collect();
        let title = take_chars(get_str(a, "title").trim(), 80).to_string();
        let period = take_chars(get_str(a, "period").trim(), 80).to_string();
        Ok(Self {
            title: if title.is_empty() {
                format!("The {subject} Report")
            } else {
                title
            },
            period: if period.is_empty() {
                "The latest news".into()
            } else {
                period
            },
            subject,
            stories: stories.into_iter().take(12).collect(),
            insights,
        })
    }
}

/// HTML text escaping (the template's values are text, never markup).
pub fn esc(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

/// The folder name of a subject: lowercase ASCII words joined by `-` (`news` when nothing is left).
pub fn slug(subject: &str) -> String {
    let mut out = String::new();
    for c in subject.to_lowercase().chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c);
        } else if !out.ends_with('-') && !out.is_empty() {
            out.push('-');
        }
    }
    let s = out.trim_matches('-');
    if s.is_empty() {
        "news".into()
    } else {
        take_chars(s, 60)
            .to_string()
            .trim_end_matches('-')
            .to_string()
    }
}

fn meta(s: &Story) -> String {
    let mut parts: Vec<String> = Vec::new();
    if !s.source.is_empty() {
        parts.push(esc(&s.source));
    }
    if !s.date.is_empty() {
        parts.push(esc(&s.date));
    }
    parts.push(format!(
        "<a href=\"{}\" rel=\"noopener\">Read the article</a>",
        esc(&s.url)
    ));
    format!("<div class=\"meta\">{}</div>", parts.join(" · "))
}

/// The report in the template: `{{title}}`, `{{subject}}`, `{{date}}`, `{{period}}`, `{{edition}}`, `{{lead}}` (the
/// first story), `{{stories}}` (the others), `{{insights}}` and `{{sources}}`.
pub fn render(template: &str, r: &Report, issued: DateTime<Local>) -> String {
    let lead = &r.stories[0];
    let lead_html = format!(
        "<h2>{}</h2><p class=\"summary\">{}</p>{}",
        esc(&lead.headline),
        esc(&lead.summary),
        meta(lead)
    );
    let stories: String = r.stories[1..]
        .iter()
        .map(|s| {
            format!(
                "<article class=\"story\"><h4>{}</h4><p>{}</p>{}</article>",
                esc(&s.headline),
                esc(&s.summary),
                meta(s)
            )
        })
        .collect::<Vec<_>>()
        .join("\n    ");
    let insights = if r.insights.is_empty() {
        "<p class=\"meta\">No further insights this edition.</p>".to_string()
    } else {
        format!(
            "<ol>{}</ol>",
            r.insights
                .iter()
                .map(|i| format!("<li>{}</li>", esc(i)))
                .collect::<String>()
        )
    };
    let sources = format!(
        "<ol>{}</ol>",
        r.stories
            .iter()
            .map(|s| {
                let name = if s.source.is_empty() {
                    String::new()
                } else {
                    format!("{} — ", esc(&s.source))
                };
                format!(
                    "<li>{name}<a href=\"{u}\" rel=\"noopener\">{u}</a></li>",
                    u = esc(&s.url)
                )
            })
            .collect::<String>()
    );
    let edition = format!(
        "{} {}",
        r.stories.len(),
        if r.stories.len() == 1 {
            "story"
        } else {
            "stories"
        }
    );
    template
        .replace("{{title}}", &esc(&r.title))
        .replace("{{subject}}", &esc(&r.subject))
        .replace(
            "{{date}}",
            &esc(&issued.format("%A, %-d %B %Y").to_string()),
        )
        .replace("{{period}}", &esc(&r.period))
        .replace("{{edition}}", &edition)
        .replace("{{lead}}", &lead_html)
        .replace("{{stories}}", &stories)
        .replace("{{insights}}", &insights)
        .replace("{{sources}}", &sources)
}

/// The workspace-relative paths of an edition: `artifacts/news/<slug>/<YYYY-MM-DD-HHMM>.html` and `.pdf`.
pub fn paths(subject: &str, issued: DateTime<Local>) -> (String, String) {
    let stem = format!(
        "artifacts/news/{}/{}",
        slug(subject),
        issued.format("%Y-%m-%d-%H%M")
    );
    (format!("{stem}.html"), format!("{stem}.pdf"))
}

/// What the user asked for, as the request extraction gives it (the only part the model decides): the subject, how
/// far back, how often a new edition comes (`""` = once), the weekday / hour when it does, and whether each edition
/// is emailed.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Ask {
    pub subject: String,
    pub days: i64,
    pub frequency: String,
    pub weekday: u32,
    pub hour: u32,
    pub email: bool,
}

impl Ask {
    /// From the extraction's JSON: unknown values fall back to the defaults (7 days; Monday; 08:00).
    pub fn from_json(v: &Value) -> Option<Self> {
        let subject = take_chars(get_str(v, "subject").trim(), 120).to_string();
        if subject.is_empty() {
            return None;
        }
        let freq = get_str(v, "frequency").trim().to_lowercase();
        let frequency = match freq.as_str() {
            "daily" | "weekly" | "monthly" => freq,
            _ => String::new(),
        };
        let n = |k: &str| v.get(k).and_then(Value::as_i64);
        let days =
            n("days")
                .filter(|d| (1..=365).contains(d))
                .unwrap_or(match frequency.as_str() {
                    "daily" => 1,
                    "monthly" => 30,
                    _ => 7,
                });
        Some(Self {
            subject,
            days,
            frequency,
            weekday: n("weekday").filter(|d| (0..=6).contains(d)).unwrap_or(1) as u32,
            hour: n("hour").filter(|h| (0..=23).contains(h)).unwrap_or(8) as u32,
            email: v.get("email").and_then(Value::as_bool).unwrap_or(false),
        })
    }

    /// The schedule's cron spec (`None` for a one-off report): daily at the hour, weekly on the weekday (0 = Sunday),
    /// monthly on the 1st.
    pub fn cron(&self) -> Option<String> {
        match self.frequency.as_str() {
            "daily" => Some(format!("0 {} * * *", self.hour)),
            "weekly" => Some(format!("0 {} * * {}", self.hour, self.weekday)),
            "monthly" => Some(format!("0 {} 1 * *", self.hour)),
            _ => None,
        }
    }

    /// The news searches: the subject as given, then with "news" (each once).
    pub fn queries(&self) -> Vec<String> {
        let mut q = vec![self.subject.clone()];
        if !self.subject.to_lowercase().contains("news") {
            q.push(format!("{} news", self.subject));
        }
        q
    }
}

/// The articles to read from the searches' results, in the engines' order: http(s) links, each once, at most `n`.
pub fn pick_articles(results: &[Value], n: usize) -> Vec<Value> {
    let mut out: Vec<Value> = Vec::new();
    for r in results {
        let u = get_str(r, "url");
        if !(u.starts_with("https://") || u.starts_with("http://")) {
            continue;
        }
        if out.iter().any(|x| get_str(x, "url") == u) {
            continue;
        }
        out.push(r.clone());
        if out.len() >= n {
            break;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use serde_json::json;

    fn args() -> Value {
        json!({"subject": "Solid-state batteries", "period": "This week",
               "stories": [
                   {"headline": "Maker <b>ships</b> cells", "summary": "First cars get them & more.", "source": "Reuters",
                    "url": "https://example.com/a?x=1&y=2", "date": "2 Nov 2026"},
                   {"headline": "No link", "summary": "dropped", "url": ""},
                   {"headline": "Same article", "summary": "dup", "url": "https://example.com/a?x=1&y=2"},
                   {"headline": "Price falls", "summary": "Costs drop 20%.", "source": "FT", "url": "https://ft.example/b"}],
               "insights": ["Expect wider use by 2028.", ""]})
    }

    #[test]
    fn stories_need_a_link_and_are_kept_once() {
        let r = Report::from_args(&args()).unwrap();
        assert_eq!(r.stories.len(), 2);
        assert_eq!(r.title, "The Solid-state batteries Report");
        assert_eq!(r.insights, ["Expect wider use by 2028."]);
        assert!(Report::from_args(
            &json!({"subject": "x", "stories": [{"headline": "h", "summary": "s"}]})
        )
        .is_err());
        assert!(Report::from_args(&json!({"stories": []})).is_err());
    }

    #[test]
    fn the_template_gets_escaped_text_and_every_source() {
        let r = Report::from_args(&args()).unwrap();
        let when = Local.with_ymd_and_hms(2026, 11, 2, 8, 5, 0).unwrap();
        let t = "<h1>{{title}}</h1>{{date}}|{{edition}}|{{lead}}|{{stories}}|{{insights}}|{{sources}}|{{period}}";
        let html = render(t, &r, when);
        assert!(html.contains("<h2>Maker &lt;b&gt;ships&lt;/b&gt; cells</h2>"));
        assert!(html.contains("Monday, 2 November 2026"));
        assert!(html.contains("2 stories"));
        assert!(html.contains("href=\"https://example.com/a?x=1&amp;y=2\""));
        assert!(html.contains("<article class=\"story\"><h4>Price falls</h4>"));
        assert!(html.contains("<li>Expect wider use by 2028.</li>"));
        assert!(html.contains("FT — <a href=\"https://ft.example/b\""));
        assert!(!html.contains("{{"));
        assert_eq!(
            paths("Solid-state batteries!", when),
            (
                "artifacts/news/solid-state-batteries/2026-11-02-0805.html".into(),
                "artifacts/news/solid-state-batteries/2026-11-02-0805.pdf".into()
            )
        );
        assert_eq!(slug("固态电池"), "news");
    }

    #[test]
    fn the_skill_template_has_every_placeholder() {
        let t = include_str!("../../../skills/news-research/template.html");
        for k in [
            "{{title}}",
            "{{subject}}",
            "{{date}}",
            "{{period}}",
            "{{edition}}",
            "{{lead}}",
            "{{stories}}",
            "{{insights}}",
            "{{sources}}",
        ] {
            assert!(t.contains(k), "{k}");
        }
    }

    #[test]
    fn the_request_gives_the_schedule_and_the_searches() {
        let a =
            Ask::from_json(&json!({"subject": "PCCW stock", "frequency": "weekly", "email": true}))
                .unwrap();
        assert_eq!(
            (a.days, a.cron().as_deref(), a.email),
            (7, Some("0 8 * * 1"), true)
        );
        assert_eq!(a.queries(), ["PCCW stock", "PCCW stock news"]);
        let d = Ask::from_json(&json!({"subject": "Lumora news", "frequency": "daily", "hour": 7}))
            .unwrap();
        assert_eq!((d.days, d.cron().as_deref()), (1, Some("0 7 * * *")));
        assert_eq!(d.queries(), ["Lumora news"]);
        let once = Ask::from_json(&json!({"subject": "x", "frequency": "sometimes", "days": 999}))
            .unwrap();
        assert_eq!((once.days, once.cron()), (7, None));
        assert!(Ask::from_json(&json!({"frequency": "daily"})).is_none());
    }

    #[test]
    fn articles_are_kept_once_in_order() {
        let r = vec![
            json!({"url": "https://a"}),
            json!({"url": "ftp://b"}),
            json!({"url": "https://a"}),
            json!({"url": "http://c"}),
        ];
        let got: Vec<String> = pick_articles(&r, 5)
            .iter()
            .map(|x| get_str(x, "url"))
            .collect();
        assert_eq!(got, ["https://a", "http://c"]);
        assert_eq!(pick_articles(&r, 1).len(), 1);
    }
}
