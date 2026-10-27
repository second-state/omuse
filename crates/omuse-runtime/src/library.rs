//! Research library: subjects the user wants researched in depth, each with a Markdown report in the workspace
//! (`library/<slug>/report.md`) that the agent writes, the user refines in a chat bound to the subject, and every task can
//! search (`library_search`; the top hits are handed to the agent at the start of a task, like memory facts).
//!
//! The report file is the source of truth (the user can read and edit it in the workspace); the FTS index is rebuilt
//! from it whenever a task bound to the subject finishes or the file is written through `files_write`.

use std::path::Path;
use std::sync::LazyLock;

use anyhow::Result;
use omuse_common::util::{get_str, new_id, now_ts, take_chars};
use regex::Regex;
use serde_json::{json, Value};
use sha1::{Digest, Sha1};

use crate::store::Store;

pub const CHUNK_CHARS: usize = 1200;
pub const STATUSES: [&str; 4] = ["researching", "ready", "empty", "failed"];

/// ASCII slug for the folder name. A title with non-Latin text (Chinese, say) keeps its Latin words plus a short hash of
/// the whole title, so "家用 NAS 选购" and "办公 NAS 选购" get different folders.
pub fn slugify(title: &str) -> String {
    static NON_ALNUM: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[^a-z0-9]+").unwrap());
    let t = title.trim();
    let s = NON_ALNUM.replace_all(&t.to_lowercase(), "-").into_owned();
    let mut s = take_chars(s.trim_matches('-'), 48).to_string();
    if !t.is_ascii() || s.chars().count() < 3 {
        let digest = hex::encode(Sha1::digest(t.as_bytes()));
        s = format!(
            "{}-{}",
            if s.is_empty() { "subject" } else { s.as_str() },
            &digest[..8]
        );
    }
    s
}

/// `library/<slug>/report.md`.
pub fn report_path(slug: &str) -> String {
    format!("library/{}/report.md", slug)
}

/// `(heading, text)` pieces of a Markdown report: a new piece at every heading; within a section, paragraphs (blank-line
/// separated) are packed into pieces of at most [`CHUNK_CHARS`]; a single longer paragraph is a piece of its own.
pub fn chunks(text: &str) -> Vec<(String, String)> {
    static HEADING: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^#{1,4}\s+\S").unwrap());
    let mut out: Vec<(String, String)> = Vec::new();
    let mut heading = String::new();
    let mut paras: Vec<String> = Vec::new();
    let mut cur: Vec<&str> = Vec::new();

    fn end_para(cur: &mut Vec<&str>, paras: &mut Vec<String>) {
        if !cur.is_empty() {
            let p = cur.join("\n").trim().to_string();
            if !p.is_empty() {
                paras.push(p);
            }
            cur.clear();
        }
    }

    fn flush(
        cur: &mut Vec<&str>,
        paras: &mut Vec<String>,
        heading: &str,
        out: &mut Vec<(String, String)>,
    ) {
        end_para(cur, paras);
        let mut piece: Vec<String> = Vec::new();
        let mut size = 0usize;
        for para in paras.iter() {
            let n = para.chars().count();
            if !piece.is_empty() && size + n + 2 > CHUNK_CHARS {
                out.push((heading.to_string(), piece.join("\n\n")));
                piece = Vec::new();
                size = 0;
            }
            piece.push(para.clone());
            size += n + 2;
        }
        if !piece.is_empty() {
            out.push((heading.to_string(), piece.join("\n\n")));
        }
        paras.clear();
    }

    for line in text.lines() {
        if HEADING.is_match(line) {
            flush(&mut cur, &mut paras, &heading, &mut out);
            heading = line.trim_start_matches('#').trim().to_string();
        } else if line.trim().is_empty() {
            end_para(&mut cur, &mut paras);
        } else {
            cur.push(line);
        }
    }
    flush(&mut cur, &mut paras, &heading, &mut out);
    out
}

/// The report text (at most 2,000,000 characters; "" when the file is missing).
pub fn read_report(workspace: &Path, slug: &str) -> String {
    let p = workspace.join(report_path(slug));
    match std::fs::read(&p) {
        Ok(bytes) => {
            let text = String::from_utf8_lossy(&bytes)
                .replace("\r\n", "\n")
                .replace('\r', "\n");
            take_chars(&text, 2_000_000).to_string()
        }
        Err(_) => String::new(),
    }
}

/// Rebuild the subject's search index from its report; sets status ready / empty. Returns the chunk count.
pub fn index_subject(store: &Store, workspace: &Path, subject: &Value) -> Result<usize> {
    let text = read_report(workspace, &get_str(subject, "slug"));
    let pieces = chunks(&text);
    let sid = get_str(subject, "id");
    store.library_replace(&sid, &pieces)?;
    store.update_subject(
        &sid,
        &json!({"status": if pieces.is_empty() { "empty" } else { "ready" }, "chars": text.chars().count()}),
    )?;
    Ok(pieces.len())
}

/// The task text that starts (or refreshes) the research of a subject; the deep-research skill does the rest.
pub fn research_goal(subject: &Value, refresh: bool, language: &str) -> String {
    let slug = get_str(subject, "slug");
    let path = report_path(&slug);
    let title = get_str(subject, "title");
    let brief = get_str(subject, "brief");
    if language == "en" {
        let head = format!(
            "RESEARCH SUBJECT: {}\nBrief: {}\nReport file: {} (Markdown; sources go in library/{}/sources.md).\n",
            title,
            if brief.is_empty() { "(none)" } else { brief.as_str() },
            path,
            slug
        );
        if refresh {
            return head + "This is a REFRESH: read the existing report first (files_read), look for what is new or changed since, update the affected sections in place and append a dated '## Changelog' entry at the end. Call load_skill('deep-research') for the method.";
        }
        return head + "Research this subject thoroughly and write the report to that file. Call load_skill('deep-research') first and follow it.";
    }
    let head = format!(
        "研究课题 RESEARCH SUBJECT: {}\n要求 Brief: {}\n报告文件 Report file: {}（Markdown；来源写在 library/{}/sources.md）。\n",
        title,
        if brief.is_empty() { "(无)" } else { brief.as_str() },
        path,
        slug
    );
    if refresh {
        return head + "这是一次刷新 REFRESH：先用 files_read 读现有报告，查找此后的新情况或变化，就地更新受影响的章节，并在末尾追加带日期的「## Changelog」条目。先调用 load_skill('deep-research') 按其方法做。";
    }
    head + "请对这个课题做全面研究，把报告写到该文件。先调用 load_skill('deep-research') 并按其方法做。"
}

/// Executor prompt section for a conversation bound to a subject: the report is the thing being discussed.
pub fn chat_context(subject: &Value, report: &str, language: &str) -> String {
    let path = report_path(&get_str(subject, "slug"));
    let title = get_str(subject, "title");
    let mut body = report.trim().to_string();
    if body.is_empty() {
        body = if language == "en" {
            "(the report is still empty)".to_string()
        } else {
            "（报告还是空的）".to_string()
        };
    }
    if body.chars().count() > 24_000 {
        body = format!(
            "{}\n…(truncated; files_read the file for the rest)",
            take_chars(&body, 24_000)
        );
    }
    if language == "en" {
        return format!(
            "\n## Research subject: {}\nThis conversation is about a research subject in the user's library. Its report is the file {}; the current text is below. When the user asks a question, answer from the report (and research further with web_search / browser_read when it lacks the answer). When the user asks for a change, addition or correction, edit the report file with files_write (rewrite the affected section; keep the rest), append a dated entry to its '## Changelog' section, and tell the user what changed. Keep the report's structure.\n### Current report\n{}\n",
            title, path, body
        );
    }
    format!(
        "\n## 研究课题 Research subject: {}\n这个对话围绕用户研究库里的一个课题。报告文件是 {}，当前内容在下面。用户提问时依据报告回答（报告里没有的再用 web_search / browser_read 查）；用户要求修改、补充或纠正时，用 files_write 改报告文件（重写受影响的章节，其余保留），在「## Changelog」末尾追加带日期的条目，并告诉用户改了什么。保持报告结构。\n### 当前报告 Current report\n{}\n",
        title, path, body
    )
}

/// Executor prompt section listing the library passages that match this task's request.
pub fn hits_context(hits: &[Value], language: &str) -> String {
    if hits.is_empty() {
        return String::new();
    }
    let lines: Vec<String> = hits
        .iter()
        .map(|h| {
            let heading = get_str(h, "heading");
            format!(
                "- [{}] {}{}",
                get_str(h, "title"),
                if heading.is_empty() {
                    String::new()
                } else {
                    format!("{}: ", heading)
                },
                take_chars(&get_str(h, "text"), 500)
            )
        })
        .collect();
    if language == "en" {
        return format!(
            "\n## From your research library\nPassages from reports the user had researched earlier that match this request (library_search finds more; library_get reads a whole report):\n{}\n",
            lines.join("\n")
        );
    }
    format!(
        "\n## 来自研究库 From the research library\n用户之前研究过的报告里与本请求相关的段落（library_search 可查更多，library_get 读整份报告）：\n{}\n",
        lines.join("\n")
    )
}

/// A new subject record (not yet stored; the caller sets `conv_id`).
pub fn new_subject(title: &str, brief: &str) -> Value {
    json!({
        "id": new_id("subj"), "slug": slugify(title), "title": take_chars(title.trim(), 120),
        "brief": take_chars(brief.trim(), 4000), "status": "researching", "conv_id": "",
        "created_at": now_ts(), "updated_at": now_ts(), "last_task": "", "runs": 0, "chars": 0, "error": "",
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slugs() {
        assert_eq!(slugify("Home NAS 2026"), "home-nas-2026");
        assert_eq!(slugify("  Home  NAS!! "), "home-nas");
        let a = slugify("家用 NAS 选购");
        let b = slugify("办公 NAS 选购");
        assert!(a.starts_with("nas-") && b.starts_with("nas-") && a != b);
        assert_eq!(a.len(), "nas-".len() + 8);
        assert!(slugify("").starts_with("subject-"));
        assert!(slugify("ab").starts_with("ab-"));
        assert_eq!(
            report_path("home-nas-2026"),
            "library/home-nas-2026/report.md"
        );
    }

    #[test]
    fn chunking() {
        let text = "# Home NAS\n\nintro line one\nintro line two\n\n## Candidates\n\nUGREEN DXP4800 runs Docker.\n\nSynology DS923+.\n### Deep\n\nx\n";
        let c = chunks(text);
        assert_eq!(
            c,
            vec![
                (
                    "Home NAS".to_string(),
                    "intro line one\nintro line two".to_string()
                ),
                (
                    "Candidates".to_string(),
                    "UGREEN DXP4800 runs Docker.\n\nSynology DS923+.".to_string()
                ),
                ("Deep".to_string(), "x".to_string()),
            ]
        );
        assert!(chunks("").is_empty());
        // paragraphs are packed up to CHUNK_CHARS; a longer one stands alone
        let big = "a".repeat(1300);
        let c = chunks(&format!("## H\n\n{}\n\nshort\n\nshort2\n", big));
        assert_eq!(c.len(), 2);
        assert_eq!(c[0].1, big);
        assert_eq!(c[1].1, "short\n\nshort2");
    }

    #[test]
    fn prompt_sections() {
        let sub = json!({"slug": "home-nas-2026", "title": "Home NAS 2026", "brief": ""});
        let g = research_goal(&sub, false, "zh");
        assert!(g.starts_with("研究课题 RESEARCH SUBJECT: Home NAS 2026\n要求 Brief: (无)\n"));
        assert!(g.ends_with("先调用 load_skill('deep-research') 并按其方法做。"));
        let g = research_goal(&sub, true, "en");
        assert!(g.contains("Brief: (none)\n") && g.contains("This is a REFRESH"));
        let c = chat_context(&sub, "", "en");
        assert!(c.contains("### Current report\n(the report is still empty)\n"));
        let c = chat_context(&sub, "  body  ", "zh");
        assert!(c.contains("### 当前报告 Current report\nbody\n"));
        assert_eq!(hits_context(&[], "en"), "");
        let h = hits_context(
            &[
                json!({"title": "Home NAS 2026", "heading": "Candidates", "text": "UGREEN"}),
                json!({"title": "T", "heading": "", "text": "no heading"}),
            ],
            "en",
        );
        assert!(h.contains("- [Home NAS 2026] Candidates: UGREEN\n- [T] no heading\n"));
        let s = new_subject("  Home NAS 2026 ", "brief");
        assert_eq!(get_str(&s, "slug"), "home-nas-2026");
        assert_eq!(get_str(&s, "title"), "Home NAS 2026");
        assert_eq!(s["runs"], json!(0));
        assert!(get_str(&s, "id").starts_with("subj_"));
    }

    #[test]
    fn index_from_file() {
        let d = tempfile::tempdir().unwrap();
        let st = Store::new(
            d.path(),
            std::sync::Arc::new(omuse_common::Config::parse("").unwrap()),
        )
        .unwrap();
        let ws = d.path().join("ws");
        let mut rec = new_subject("Home NAS 2026", "b");
        rec["conv_id"] = json!(st.create_conv("Home NAS 2026", "research", None).unwrap());
        let sub = st.create_subject(&rec).unwrap();
        assert_eq!(index_subject(&st, &ws, &sub).unwrap(), 0);
        assert_eq!(
            get_str(
                &st.subject(&get_str(&sub, "id")).unwrap().unwrap(),
                "status"
            ),
            "empty"
        );
        let p = ws.join(report_path("home-nas-2026"));
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(
            &p,
            "# Home NAS\r\n\r\n## Candidates\r\n\r\nUGREEN DXP4800 runs Docker.\r\n",
        )
        .unwrap();
        assert_eq!(
            read_report(&ws, "home-nas-2026"),
            "# Home NAS\n\n## Candidates\n\nUGREEN DXP4800 runs Docker.\n"
        );
        assert_eq!(index_subject(&st, &ws, &sub).unwrap(), 1);
        let s2 = st.subject(&get_str(&sub, "id")).unwrap().unwrap();
        assert_eq!(get_str(&s2, "status"), "ready");
        let hits = st.library_search("Docker", 10).unwrap();
        assert!(hits.iter().any(|h| get_str(h, "heading") == "Candidates"));
    }
}
