//! Small, dependency-free Markdown -> HTML for local PDF export (`app/common/mdhtml.py`).
//!
//! Covers what agent reports use: headings, paragraphs, bold/italic/strike, inline code, fenced code, links, images,
//! bullet/numbered lists (nested by indent), blockquotes, GFM tables and rules. Any raw HTML in the source is escaped,
//! so the output never contains scripts or tags the author didn't get from this converter.

use std::sync::LazyLock;

use fancy_regex::Regex as FRegex;
use regex::Regex;

use crate::pyfmt::html_escape;

/// Resolves an image `src` to a `data:` URL (or `None` when the image is missing / not allowed).
pub type ImageResolver<'a> = &'a dyn Fn(&str) -> Option<String>;

static CJK: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[⺀-鿿가-힯＀-￯　-〿]").unwrap());
static LIST_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(\s*)([-*+]|\d{1,3}[.)])\s+(.*)$").unwrap());
static TABLE_SEP: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\s*\|?\s*:?-{2,}:?\s*(\|\s*:?-{2,}:?\s*)*\|?\s*$").unwrap());
static CODE_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"`([^`]+)`").unwrap());
static IMAGE_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"!\[([^\]]*)\]\(([^)\s]+)(?:\s+&quot;[^)]*&quot;)?\)").unwrap());
static LINK_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\[([^\]]+)\]\(([^)\s]+)\)").unwrap());
static URL_OK: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)^(https?:|mailto:)").unwrap());
static STRONG_RE: LazyLock<FRegex> =
    LazyLock::new(|| FRegex::new(r"(\*\*|__)(?=\S)(.+?)(?<=\S)\1").unwrap());
static EM_RE: LazyLock<FRegex> =
    LazyLock::new(|| FRegex::new(r"(?<![\*\w])\*(?=\S)(.+?)(?<=\S)\*(?!\*)").unwrap());
static DEL_RE: LazyLock<FRegex> = LazyLock::new(|| FRegex::new(r"~~(?=\S)(.+?)(?<=\S)~~").unwrap());
static PLACEHOLDER_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new("\x00(\\d+)\x00").unwrap());
static HEADING_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(#{1,6})\s+(.*?)\s*#*\s*$").unwrap());
static HR_RE: LazyLock<FRegex> = LazyLock::new(|| FRegex::new(r"^([-*_])(\s*\1){2,}$").unwrap());
static QUOTE_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^\s*>\s?").unwrap());
static TASK_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^\[([ xX])\]\s+(.*)$").unwrap());

/// Python `html.unescape` (named, decimal and hex entities).
fn html_unescape(s: &str) -> String {
    html_escape::decode_html_entities(s).into_owned()
}

fn inline(text: &str, img: Option<ImageResolver<'_>>) -> String {
    let mut codes: Vec<String> = Vec::new();
    let text = CODE_RE.replace_all(text, |caps: &regex::Captures| {
        codes.push(format!("<code>{}</code>", html_escape(&caps[1], true)));
        format!("\x00{}\x00", codes.len() - 1)
    });
    let t = html_escape(&text, false);
    let t = IMAGE_RE.replace_all(&t, |caps: &regex::Captures| {
        let alt = &caps[1];
        let src = html_unescape(&caps[2]);
        let src = src.trim();
        let data = img.and_then(|f| f(src)).filter(|d| !d.is_empty());
        match data {
            Some(d) => format!("<img alt=\"{}\" src=\"{}\">", html_escape(alt, true), d),
            None => format!(
                "<span class=\"missing\">[{}]</span>",
                if alt.is_empty() { "image" } else { alt }
            ),
        }
    });
    let t = LINK_RE.replace_all(&t, |caps: &regex::Captures| {
        let label = &caps[1];
        let url = html_unescape(&caps[2]);
        let url = url.trim();
        if URL_OK.is_match(url) {
            format!("<a href=\"{}\">{}</a>", html_escape(url, true), label)
        } else {
            label.to_string()
        }
    });
    let t = STRONG_RE.replace_all(&t, |caps: &fancy_regex::Captures| {
        format!("<strong>{}</strong>", &caps[2])
    });
    let t = EM_RE.replace_all(&t, |caps: &fancy_regex::Captures| {
        format!("<em>{}</em>", &caps[1])
    });
    let t = DEL_RE.replace_all(&t, |caps: &fancy_regex::Captures| {
        format!("<del>{}</del>", &caps[1])
    });
    PLACEHOLDER_RE
        .replace_all(&t, |caps: &regex::Captures| {
            let i: usize = caps[1].parse().unwrap_or(usize::MAX);
            codes.get(i).cloned().unwrap_or_default()
        })
        .into_owned()
}

fn join(lines: &[&str]) -> String {
    let mut out = String::new();
    for ln in lines {
        let ln = ln.trim();
        if !out.is_empty() {
            let last = out
                .chars()
                .next_back()
                .map(|c| c.to_string())
                .unwrap_or_default();
            let first = ln
                .chars()
                .next()
                .map(|c| c.to_string())
                .unwrap_or_else(|| " ".to_string());
            if !(CJK.is_match(&last) && CJK.is_match(&first)) {
                out.push(' ');
            }
        }
        out.push_str(ln);
    }
    out
}

/// Split a table row on `|` that are not escaped with a backslash.
fn cells(row: &str) -> Vec<String> {
    let mut row = row.trim();
    if let Some(r) = row.strip_prefix('|') {
        row = r;
    }
    if row.ends_with('|') && !row.ends_with("\\|") {
        row = &row[..row.len() - 1];
    }
    let mut parts = Vec::new();
    let mut cur = String::new();
    let mut prev_backslash = false;
    for ch in row.chars() {
        if ch == '|' && !prev_backslash {
            parts.push(std::mem::take(&mut cur));
        } else {
            cur.push(ch);
        }
        prev_backslash = ch == '\\';
    }
    parts.push(cur);
    parts.iter().map(|c| c.trim().replace("\\|", "|")).collect()
}

/// A list item: (indent, tag, start, html).
struct Item {
    indent: usize,
    tag: &'static str,
    start: i64,
    html: String,
}

/// Convert Markdown to HTML. `img` resolves image sources to `data:` URLs (see `app/browser/main.py:_img_data`).
pub fn md_to_html(src: &str, img: Option<ImageResolver<'_>>) -> String {
    let text = src.replace("\r\n", "\n").replace('\t', "    ");
    let lines: Vec<&str> = text.split('\n').collect();
    let n = lines.len();
    let mut out: Vec<String> = Vec::new();
    let mut para: Vec<&str> = Vec::new();
    let mut i = 0;

    fn flush(para: &mut Vec<&str>, out: &mut Vec<String>, img: Option<ImageResolver<'_>>) {
        if !para.is_empty() {
            out.push(format!("<p>{}</p>", inline(&join(para), img)));
            para.clear();
        }
    }

    while i < n {
        let ln = lines[i];
        let s = ln.trim();
        if s.is_empty() {
            flush(&mut para, &mut out, img);
            i += 1;
            continue;
        }
        if s.starts_with("```") || s.starts_with("~~~") {
            flush(&mut para, &mut out, img);
            let fence = &s[..3];
            let mut body: Vec<&str> = Vec::new();
            i += 1;
            while i < n && !lines[i].trim().starts_with(fence) {
                body.push(lines[i]);
                i += 1;
            }
            i += 1;
            out.push(format!(
                "<pre><code>{}</code></pre>",
                html_escape(&body.join("\n"), true)
            ));
            continue;
        }
        if let Some(m) = HEADING_RE.captures(s) {
            flush(&mut para, &mut out, img);
            let lv = m[1].len();
            out.push(format!("<h{lv}>{}</h{lv}>", inline(&m[2], img)));
            i += 1;
            continue;
        }
        if HR_RE.is_match(s).unwrap_or(false) {
            flush(&mut para, &mut out, img);
            out.push("<hr>".to_string());
            i += 1;
            continue;
        }
        if s.contains('|') && i + 1 < n && TABLE_SEP.is_match(lines[i + 1]) && !LIST_RE.is_match(ln)
        {
            flush(&mut para, &mut out, img);
            let head = cells(s);
            let aligns: Vec<&str> = cells(lines[i + 1])
                .iter()
                .map(|c| {
                    if c.starts_with(':') && c.ends_with(':') {
                        "center"
                    } else if c.ends_with(':') {
                        "right"
                    } else {
                        ""
                    }
                })
                .collect();
            i += 2;
            let mut rows: Vec<Vec<String>> = Vec::new();
            while i < n && lines[i].contains('|') && !lines[i].trim().is_empty() {
                rows.push(cells(lines[i]));
                i += 1;
            }
            let st = |k: usize| -> String {
                if k < aligns.len() && !aligns[k].is_empty() {
                    format!(" style=\"text-align:{}\"", aligns[k])
                } else {
                    String::new()
                }
            };
            let mut t = String::from("<table><thead><tr>");
            for (k, c) in head.iter().enumerate() {
                t.push_str(&format!("<th{}>{}</th>", st(k), inline(c, img)));
            }
            t.push_str("</tr></thead><tbody>");
            let width = head.len().max(1);
            for r in rows {
                let mut r = r;
                r.resize(r.len() + head.len(), String::new());
                r.truncate(width);
                t.push_str("<tr>");
                for (k, c) in r.iter().enumerate() {
                    t.push_str(&format!("<td{}>{}</td>", st(k), inline(c, img)));
                }
                t.push_str("</tr>");
            }
            t.push_str("</tbody></table>");
            out.push(t);
            continue;
        }
        if s.starts_with('>') {
            flush(&mut para, &mut out, img);
            let mut q: Vec<String> = Vec::new();
            while i < n && lines[i].trim().starts_with('>') {
                q.push(QUOTE_RE.replace(lines[i], "").into_owned());
                i += 1;
            }
            out.push(format!(
                "<blockquote>{}</blockquote>",
                md_to_html(&q.join("\n"), img)
            ));
            continue;
        }
        if LIST_RE.is_match(ln) {
            flush(&mut para, &mut out, img);
            let mut items: Vec<Item> = Vec::new();
            while i < n {
                let Some(lm) = LIST_RE.captures(lines[i]) else {
                    // an indented line continues the previous item; anything else ends the list
                    if !lines[i].trim().is_empty()
                        && lines[i].starts_with("  ")
                        && !items.is_empty()
                    {
                        let extra = inline(lines[i].trim(), img);
                        let last = items.last_mut().unwrap();
                        last.html.push(' ');
                        last.html.push_str(&extra);
                        i += 1;
                        continue;
                    }
                    break;
                };
                let marker = &lm[2];
                let mut body = lm[3].to_string();
                if let Some(task) = TASK_RE.captures(&body.clone()) {
                    let done = task[1].to_lowercase() == "x";
                    body = format!("{}{}", if done { "☑ " } else { "☐ " }, &task[2]);
                }
                let numbered = marker
                    .chars()
                    .next()
                    .map(|c| c.is_ascii_digit())
                    .unwrap_or(false);
                items.push(Item {
                    indent: lm[1].chars().count(),
                    tag: if numbered { "ol" } else { "ul" },
                    start: if numbered {
                        marker[..marker.len() - 1].parse().unwrap_or(1)
                    } else {
                        1
                    },
                    html: inline(&body, img),
                });
                i += 1;
            }
            let mut k = 0;
            let mut parts = String::new();
            while k < items.len() {
                let (h, nk) = build_list(&items, k);
                parts.push_str(&h);
                k = nk;
            }
            out.push(parts);
            continue;
        }
        para.push(ln);
        i += 1;
    }
    flush(&mut para, &mut out, img);
    out.join("\n")
}

fn build_list(items: &[Item], mut k: usize) -> (String, usize) {
    let ind0 = items[k].indent;
    let tag = items[k].tag;
    let st = if tag == "ol" && items[k].start != 1 {
        format!(" start=\"{}\"", items[k].start)
    } else {
        String::new()
    };
    let mut parts = format!("<{tag}{st}>");
    while k < items.len() && items[k].indent == ind0 {
        parts.push_str("<li>");
        parts.push_str(&items[k].html);
        k += 1;
        while k < items.len() && items[k].indent > ind0 {
            // deeper items: a nested list inside this <li>
            let (sub, nk) = build_list(items, k);
            parts.push_str(&sub);
            k = nk;
        }
        parts.push_str("</li>");
    }
    parts.push_str(&format!("</{tag}>"));
    (parts, k)
}

/// The print stylesheet for PDF export.
pub const CSS: &str = r#"
@page { size: A4; margin: 18mm 16mm 20mm 16mm; }
html { -webkit-print-color-adjust: exact; print-color-adjust: exact; }
body { font-family: "Noto Sans CJK SC", "WenQuanYi Zen Hei", "PingFang SC", "Microsoft YaHei", "Helvetica Neue", Arial, sans-serif;
       font-size: 11pt; line-height: 1.65; color: #1f2328; }
h1 { font-size: 20pt; margin: 0 0 12pt; padding-bottom: 6pt; border-bottom: 2px solid #1f6f5c; }
h2 { font-size: 15pt; margin: 18pt 0 8pt; color: #1f6f5c; }
h3 { font-size: 12.5pt; margin: 14pt 0 6pt; }
h4, h5, h6 { font-size: 11pt; margin: 12pt 0 4pt; }
h1, h2, h3, h4 { break-after: avoid; }
p { margin: 0 0 8pt; }
ul, ol { margin: 0 0 8pt; padding-left: 20pt; }
li { margin: 2pt 0; }
table { border-collapse: collapse; width: 100%; margin: 8pt 0 12pt; font-size: 10pt; }
th, td { border: 1px solid #d0d7de; padding: 5pt 7pt; vertical-align: top; text-align: left; }
th { background: #eef4f2; font-weight: 600; }
tr { break-inside: avoid; }
code { font-family: "WenQuanYi Zen Hei Mono", "DejaVu Sans Mono", monospace; font-size: 9.5pt; background: #f3f4f6; padding: 1px 4px; border-radius: 3px; }
pre { background: #f6f8fa; border: 1px solid #e5e7eb; border-radius: 6px; padding: 8pt 10pt; white-space: pre-wrap; word-break: break-word; }
pre code { background: none; padding: 0; }
blockquote { margin: 8pt 0; padding: 4pt 12pt; border-left: 3px solid #1f6f5c; color: #4b5563; background: #f7faf9; }
hr { border: none; border-top: 1px solid #d0d7de; margin: 14pt 0; }
img { max-width: 100%; }
a { color: #1f6f5c; }
.missing { color: #9ca3af; }
"#;

/// A complete HTML document around `body_html` (`mdhtml.page`).
pub fn page(title: &str, body_html: &str) -> String {
    format!(
        "<!doctype html><html><head><meta charset=\"utf-8\"><title>{}</title><style>{}</style></head><body>{}</body></html>",
        html_escape(if title.is_empty() { "Document" } else { title }, true),
        CSS,
        body_html
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blocks_and_inline() {
        let html = md_to_html("# Title #\n\nSome **bold** and *em* and ~~del~~ with `co<de>`.\n\n- a\n- b\n  - c\n1. x\n\n> q1\n> q2\n\n---\n\n| h1 | h2 |\n|---|---:|\n| 1 | 2 |\n", None);
        assert_eq!(
            html,
            "<h1>Title</h1>\n<p>Some <strong>bold</strong> and <em>em</em> and <del>del</del> with <code>co&lt;de&gt;</code>.</p>\n<ul><li>a</li><li>b<ul><li>c</li></ul></li><li>x</li></ul>\n<blockquote><p>q1 q2</p></blockquote>\n<hr>\n<table><thead><tr><th>h1</th><th style=\"text-align:right\">h2</th></tr></thead><tbody><tr><td>1</td><td style=\"text-align:right\">2</td></tr></tbody></table>"
        );
    }

    #[test]
    fn html_is_escaped_and_links_filtered() {
        assert_eq!(
            md_to_html("<script>x</script>", None),
            "<p>&lt;script&gt;x&lt;/script&gt;</p>"
        );
        assert_eq!(
            md_to_html("[a](javascript:alert(1)) [b](https://x.y/?q=1&r=2)", None),
            "<p>a) <a href=\"https://x.y/?q=1&amp;r=2\">b</a></p>"
        );
        assert_eq!(
            md_to_html("![pic](charts/a.png)", None),
            "<p><span class=\"missing\">[pic]</span></p>"
        );
        let resolver = |src: &str| {
            if src == "charts/a.png" {
                Some("data:image/png;base64,AA==".to_string())
            } else {
                None
            }
        };
        assert_eq!(
            md_to_html("![p \"q\"](charts/a.png)", Some(&resolver)),
            "<p><img alt=\"p &quot;q&quot;\" src=\"data:image/png;base64,AA==\"></p>"
        );
    }

    #[test]
    fn cjk_lines_join_without_spaces_and_fences_keep_text() {
        assert_eq!(
            md_to_html("中文\n段落\nthen english\n", None),
            "<p>中文段落 then english</p>"
        );
        assert_eq!(
            md_to_html("```\na <b>\n  c\n```\nafter", None),
            "<pre><code>a &lt;b&gt;\n  c</code></pre>\n<p>after</p>"
        );
        assert_eq!(
            md_to_html("3. three\n4. four\n- [x] done\n- [ ] todo", None),
            "<ol start=\"3\"><li>three</li><li>four</li><li>☑ done</li><li>☐ todo</li></ol>"
        );
        assert_eq!(
            md_to_html("- item\n  continued here", None),
            "<ul><li>item continued here</li></ul>"
        );
    }

    #[test]
    fn page_wraps_body() {
        let p = page("", "<p>x</p>");
        assert!(p.starts_with("<!doctype html><html><head><meta charset=\"utf-8\"><title>Document</title><style>\n@page"));
        assert!(p.ends_with("</style></head><body><p>x</p></body></html>"));
        assert!(page("a<b", "").contains("<title>a&lt;b</title>"));
    }
}
