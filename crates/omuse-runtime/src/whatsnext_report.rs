//! The "What's next" report (agent/whatsnext.rs): one self-contained HTML page under `artifacts/whats-next/`, laid out
//! in code from the machine's data — today's items in time order with their research (flight status and backups, the
//! arrival card, the tax refund, the map, the people, the meeting's background), the next two days in brief, the local
//! news and this evening's events. Every value is escaped; only `https://` links are linked.

use chrono::{DateTime, Local};
use serde::{Deserialize, Serialize};

use crate::newsreport::esc;

/// A link with its text.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Link {
    pub text: String,
    pub url: String,
    /// the source or a short note shown after it
    #[serde(default)]
    pub note: String,
}

/// A person met today, with what the web says about them (external participants only).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Person {
    pub name: String,
    #[serde(default)]
    pub organization: String,
    #[serde(default)]
    pub profile: String,
    #[serde(default)]
    pub links: Vec<Link>,
}

/// One item of the plan (a flight, a meeting, a hotel, a place to be).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Entry {
    pub kind: String,
    pub title: String,
    pub date: String,
    /// "HH:MM" or "HH:MM–HH:MM" (local), "" all day
    #[serde(default)]
    pub time: String,
    #[serde(default)]
    pub place: String,
    #[serde(default)]
    pub map_url: String,
    /// the model's short write-up (status, background, what to do)
    #[serde(default)]
    pub notes: String,
    /// facts code adds (the arrival card, the tax refund, the flight's status): label → text
    #[serde(default)]
    pub facts: Vec<(String, String)>,
    /// links code adds (the official pages)
    #[serde(default)]
    pub links: Vec<Link>,
    /// later flights on the same route (a delayed or cancelled flight)
    #[serde(default)]
    pub backups: Vec<String>,
    #[serde(default)]
    pub people: Vec<Person>,
    /// earlier emails with the participants (subject — sender, date)
    #[serde(default)]
    pub background: Vec<String>,
    /// added by the user today (and put in the calendar)
    #[serde(default)]
    pub added: bool,
}

/// The whole report.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Report {
    pub title: String,
    /// "YYYY-MM-DD"
    pub date: String,
    /// where the user is today ("Tokyo, Japan")
    #[serde(default)]
    pub location: String,
    /// the model's two or three sentences on the day
    #[serde(default)]
    pub summary: String,
    pub today: Vec<Entry>,
    #[serde(default)]
    pub later: Vec<Entry>,
    #[serde(default)]
    pub news: Vec<Link>,
    /// the city the news is from
    #[serde(default)]
    pub news_city: String,
    /// this evening's events and films (only when the evening is free)
    #[serde(default)]
    pub evening: Vec<Link>,
}

fn link(l: &Link) -> String {
    let note = if l.note.trim().is_empty() {
        String::new()
    } else {
        format!(" <span class=\"muted\">· {}</span>", esc(&l.note))
    };
    if l.url.starts_with("https://") {
        format!(
            "<a href=\"{}\" target=\"_blank\" rel=\"noopener\">{}</a>{note}",
            esc(&l.url),
            esc(if l.text.trim().is_empty() {
                &l.url
            } else {
                &l.text
            })
        )
    } else {
        format!("{}{note}", esc(&l.text))
    }
}

fn icon(kind: &str) -> &'static str {
    match kind {
        "flight" => "✈️",
        "hotel" => "🏨",
        "meeting" => "👥",
        "reservation" => "🍽",
        "event" => "🎟",
        "train" => "🚄",
        _ => "📍",
    }
}

fn entry(e: &Entry, full: bool) -> String {
    let mut h = String::new();
    h.push_str("<article class=\"item\">");
    h.push_str(&format!(
        "<div class=\"when\">{}</div><div class=\"body\"><h3>{} {}{}</h3>",
        esc(if e.time.is_empty() {
            "all day"
        } else {
            &e.time
        }),
        icon(&e.kind),
        esc(&e.title),
        if e.added {
            " <span class=\"tag\">added</span>"
        } else {
            ""
        }
    ));
    if !e.place.is_empty() {
        let place = if e.map_url.starts_with("https://") {
            format!(
                "<a href=\"{}\" target=\"_blank\" rel=\"noopener\">{}</a>",
                esc(&e.map_url),
                esc(&e.place)
            )
        } else {
            esc(&e.place)
        };
        h.push_str(&format!("<p class=\"place\">📍 {place}</p>"));
    }
    if full && !e.notes.trim().is_empty() {
        h.push_str(&format!("<p>{}</p>", esc(e.notes.trim())));
    }
    if !e.facts.is_empty() {
        h.push_str("<dl>");
        for (k, v) in &e.facts {
            h.push_str(&format!("<dt>{}</dt><dd>{}</dd>", esc(k), esc(v)));
        }
        h.push_str("</dl>");
    }
    if full && !e.backups.is_empty() {
        h.push_str("<p class=\"label\">Later flights on the same route</p><ul>");
        for b in &e.backups {
            h.push_str(&format!("<li>{}</li>", esc(b)));
        }
        h.push_str("</ul>");
    }
    if full && !e.people.is_empty() {
        h.push_str("<p class=\"label\">Who you will meet</p><ul class=\"people\">");
        for p in &e.people {
            let org = if p.organization.is_empty() {
                String::new()
            } else {
                format!(" — {}", esc(&p.organization))
            };
            let links: Vec<String> = p.links.iter().map(link).collect();
            h.push_str(&format!(
                "<li><b>{}</b>{org}{}{}</li>",
                esc(&p.name),
                if p.profile.is_empty() {
                    String::new()
                } else {
                    format!("<br>{}", esc(&p.profile))
                },
                if links.is_empty() {
                    String::new()
                } else {
                    format!("<br><span class=\"small\">{}</span>", links.join(" · "))
                }
            ));
        }
        h.push_str("</ul>");
    }
    if full && !e.background.is_empty() {
        h.push_str("<p class=\"label\">Earlier emails</p><ul class=\"small\">");
        for b in &e.background {
            h.push_str(&format!("<li>{}</li>", esc(b)));
        }
        h.push_str("</ul>");
    }
    if !e.links.is_empty() {
        let l: Vec<String> = e.links.iter().map(link).collect();
        h.push_str(&format!("<p class=\"small\">{}</p>", l.join(" · ")));
    }
    h.push_str("</div></article>");
    h
}

const STYLE: &str = r#"
:root{color-scheme:light dark;--bg:#fafaf7;--fg:#1d1d1b;--muted:#6b6b66;--line:#e2e2dc;--card:#fff;--accent:#1F6F5C}
@media (prefers-color-scheme:dark){:root{--bg:#161615;--fg:#ecece8;--muted:#a3a39c;--line:#33332f;--card:#1f1f1d;--accent:#5fbfa3}}
*{box-sizing:border-box}body{margin:0;background:var(--bg);color:var(--fg);font:16px/1.55 system-ui,-apple-system,"Segoe UI",sans-serif}
main{max-width:760px;margin:0 auto;padding:24px 16px 48px}h1{font-size:26px;margin:0}h2{font-size:18px;margin:28px 0 10px;border-bottom:1px solid var(--line);padding-bottom:6px}
h3{font-size:16px;margin:0 0 4px}p{margin:6px 0}.muted,.small{color:var(--muted)}.small{font-size:13.5px}a{color:var(--accent)}
.lead{font-size:17px;margin:12px 0 4px}.item{display:flex;gap:14px;background:var(--card);border:1px solid var(--line);border-radius:10px;padding:12px 14px;margin:10px 0}
.when{min-width:92px;font-weight:600;font-variant-numeric:tabular-nums}.body{flex:1;min-width:0}.place{color:var(--muted)}
dl{display:grid;grid-template-columns:max-content 1fr;gap:2px 12px;margin:8px 0;font-size:14.5px}dt{color:var(--muted)}dd{margin:0}
.label{font-weight:600;margin-top:10px}ul{margin:4px 0 4px 18px;padding:0}.tag{font-size:12px;border:1px solid var(--line);border-radius:6px;padding:0 6px;color:var(--muted)}
@media (max-width:560px){.item{flex-direction:column;gap:4px}.when{min-width:0}}
"#;

/// The page.
pub fn render(r: &Report, issued: DateTime<Local>) -> String {
    let mut h = format!(
        "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width,initial-scale=1\">\
<meta name=\"referrer\" content=\"no-referrer\"><title>{}</title><style>{STYLE}</style></head><body><main>",
        esc(&r.title)
    );
    h.push_str(&format!("<h1>{}</h1>", esc(&r.title)));
    h.push_str(&format!(
        "<p class=\"muted\">{}{} · made {}</p>",
        esc(&r.date),
        if r.location.is_empty() {
            String::new()
        } else {
            format!(" · {}", esc(&r.location))
        },
        issued.format("%Y-%m-%d %H:%M")
    ));
    if !r.summary.trim().is_empty() {
        h.push_str(&format!("<p class=\"lead\">{}</p>", esc(r.summary.trim())));
    }
    h.push_str("<h2>Today</h2>");
    if r.today.is_empty() {
        h.push_str("<p class=\"muted\">Nothing found in your mail or calendar for today.</p>");
    }
    for e in &r.today {
        h.push_str(&entry(e, true));
    }
    if !r.evening.is_empty() {
        h.push_str("<h2>This evening is free</h2><ul>");
        for l in &r.evening {
            h.push_str(&format!("<li>{}</li>", link(l)));
        }
        h.push_str("</ul>");
    }
    if !r.news.is_empty() {
        h.push_str(&format!(
            "<h2>Local news{}</h2><ul>",
            if r.news_city.is_empty() {
                String::new()
            } else {
                format!(" — {}", esc(&r.news_city))
            }
        ));
        for l in &r.news {
            h.push_str(&format!("<li>{}</li>", link(l)));
        }
        h.push_str("</ul>");
    }
    if !r.later.is_empty() {
        h.push_str("<h2>Next two days</h2>");
        let mut day = String::new();
        for e in &r.later {
            if e.date != day {
                day = e.date.clone();
                h.push_str(&format!("<p class=\"label\">{}</p>", esc(&day)));
            }
            h.push_str(&entry(e, false));
        }
    }
    h.push_str("<p class=\"small\" style=\"margin-top:32px\">From your mail and calendar, researched by OMuse. Check times with the airline or the organiser before you leave.</p>");
    h.push_str("</main></body></html>\n");
    h
}

/// Where a report is written: `artifacts/whats-next/<date>-<HHMM>.html`.
pub fn path(issued: DateTime<Local>) -> String {
    format!(
        "artifacts/whats-next/{}.html",
        issued.format("%Y-%m-%d-%H%M")
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_page_escapes_everything_and_links_only_https() {
        let r = Report {
            title: "What's next — Wed 4 Nov".into(),
            date: "2026-11-04".into(),
            location: "Tokyo, Japan".into(),
            summary: "A meeting at 10, then <b>the</b> flight home.".into(),
            today: vec![Entry {
                kind: "meeting".into(),
                title: "Q4 review <script>".into(),
                date: "2026-11-04".into(),
                time: "10:00–11:00".into(),
                place: "Roppongi Hills".into(),
                map_url: "https://www.google.com/maps/search/?api=1&query=Roppongi+Hills".into(),
                facts: vec![(
                    "Arrival card".into(),
                    "SG Arrival Card, within 3 days".into(),
                )],
                links: vec![
                    Link {
                        text: "ICA".into(),
                        url: "https://www.ica.gov.sg/".into(),
                        note: String::new(),
                    },
                    Link {
                        text: "bad".into(),
                        url: "javascript:alert(1)".into(),
                        note: String::new(),
                    },
                ],
                people: vec![Person {
                    name: "Aiko Tanaka".into(),
                    organization: "Acme KK".into(),
                    ..Default::default()
                }],
                ..Default::default()
            }],
            evening: vec![Link {
                text: "Jazz at Blue Note".into(),
                url: "https://www.bluenote.co.jp/".into(),
                note: "19:00".into(),
            }],
            ..Default::default()
        };
        let h = render(&r, Local::now());
        assert!(
            h.contains("Q4 review &lt;script&gt;") && !h.contains("<script>"),
            "{h}"
        );
        assert!(h.contains("&lt;b&gt;the&lt;/b&gt;"));
        assert!(h.contains("href=\"https://www.ica.gov.sg/\""));
        assert!(!h.contains("javascript:"), "{h}");
        assert!(
            h.contains("Roppongi+Hills")
                && h.contains("Aiko Tanaka")
                && h.contains("This evening is free")
        );
        assert!(path(Local::now()).starts_with("artifacts/whats-next/"));
    }
}
