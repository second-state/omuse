//! The facts the user must read, appended to the answer by code (docs/json-tools.md, "What the answer shows").
//!
//! A trusted code tool's JSON result (a workflow machine's `T_result`, a fact tool's result: `facts::FACT_TOOLS`,
//! `confirm_reference`) carries `show`: a list of structured items — a link, a file, a reference, an amount, a fact —
//! each with its label. After the model's reply (and after the review), code renders every item of this request as one
//! Markdown line and appends the block to the answer, always and in order: the reply's prose is never read and never
//! rewritten. The run's own structured facts (`agent/guards.rs`: the card catalog's date, an approved amount, …) are
//! appended the same way. The block opens with a Markdown rule ([`MARK`]), so a reader finds it after the reply; the
//! only comparison is string equality of code's own rendered lines against what follows the answer's last rule (so a
//! second append adds nothing).
//!
//! ```text
//! {"kind": "reference", "label": "Confirmation number", "value": "TH7D3K9Q2"}  →  - Confirmation number: **TH7D3K9Q2**
//! {"kind": "link", "label": "HTML edition", "text": "The Tidal Ledger", "url": "/artifacts/…html"}
//!                                                       →  - HTML edition: [The Tidal Ledger](/artifacts/…html)
//! {"kind": "file", "label": "PDF", "path": "reports/a.pdf"}   →  - PDF: [a.pdf](/api/files/raw?path=reports%2Fa.pdf)
//! {"kind": "amount", "label": "Amount charged", "value": 454, "currency": "SGD"}  →  - Amount charged: SGD 454.00
//! {"kind": "fact", "label": "Next edition", "value": "Monday 2 November 08:00"}  →  - Next edition: Monday 2 November 08:00
//! {"kind": "block", "value": "| Card | … |"}                  →  the Markdown as it is (a table)
//! ```
//!
//! A label is code's: `label` in English, `label_zh` for a Chinese answer (the language setting); a label taken from
//! structured data (a page's own field name) has no `label_zh` and is shown as it is.

use omuse_common::util::get_str;
use percent_encoding::{utf8_percent_encode, NON_ALPHANUMERIC};
use serde_json::{json, Map, Value};

use super::util::sget;

/// The kinds of item a result can show.
pub const KINDS: [&str; 6] = ["link", "file", "reference", "amount", "fact", "block"];

fn item(kind: &str, label: &str, label_zh: &str) -> Map<String, Value> {
    let mut m = Map::new();
    m.insert("kind".into(), json!(kind));
    m.insert("label".into(), json!(label));
    if !label_zh.is_empty() {
        m.insert("label_zh".into(), json!(label_zh));
    }
    m
}

/// A link: `- {label}: [{text}]({url})` (the url itself as the text when `text` is empty).
pub fn link(label: &str, label_zh: &str, text: &str, url: &str) -> Value {
    let mut m = item("link", label, label_zh);
    m.insert("url".into(), json!(url));
    if !text.is_empty() {
        m.insert("text".into(), json!(text));
    }
    Value::Object(m)
}

/// A file in the workspace: a link to `/api/files/raw?path=…` (the file's name as the text when `text` is empty).
pub fn file(label: &str, label_zh: &str, text: &str, path: &str) -> Value {
    let mut m = item("file", label, label_zh);
    m.insert("path".into(), json!(path));
    if !text.is_empty() {
        m.insert("text".into(), json!(text));
    }
    Value::Object(m)
}

/// A reference, a booking or confirmation number: `- {label}: **{value}**`.
pub fn reference(label: &str, label_zh: &str, value: &str) -> Value {
    let mut m = item("reference", label, label_zh);
    m.insert("value".into(), json!(value));
    Value::Object(m)
}

/// An amount: `- {label}: {currency} {value}` (a number with two decimals and thousands separators).
pub fn amount(label: &str, label_zh: &str, value: f64, currency: &str) -> Value {
    let mut m = item("amount", label, label_zh);
    m.insert("value".into(), json!(value));
    m.insert("currency".into(), json!(currency));
    Value::Object(m)
}

/// A fact rendered from structured data: `- {label}: {value}` (`- {value}` without a label).
pub fn fact(label: &str, label_zh: &str, value: &str) -> Value {
    let mut m = item("fact", label, label_zh);
    m.insert("value".into(), json!(value));
    Value::Object(m)
}

/// Markdown appended as it is (a table code rendered from data).
pub fn block(markdown: &str) -> Value {
    json!({"kind": "block", "value": markdown})
}

/// The `show` items of a result (those of a known kind with something to show).
pub fn items(result: &Value) -> Vec<Value> {
    result
        .get("show")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|x| KINDS.contains(&get_str(x, "kind").as_str()))
        .cloned()
        .collect()
}

/// The `show` items of every trusted code tool's result in this request (after the user's latest message), in order.
pub fn collect(transcript: &[Value]) -> Vec<Value> {
    let last_user = transcript
        .iter()
        .rposition(|m| get_str(m, "role") == "user")
        .unwrap_or(0);
    super::facts::trusted_results(&transcript[last_user..])
        .iter()
        .flat_map(items)
        .collect()
}

/// The Markdown lines a result's `show` items render to (what code appends to the reply for it).
pub fn lines(result: &Value, zh: bool) -> Vec<String> {
    items(result).iter().filter_map(|i| render(i, zh)).collect()
}

/// A url as a Markdown link target: spaces and parentheses escaped.
fn target(url: &str) -> String {
    url.trim()
        .replace(' ', "%20")
        .replace('(', "%28")
        .replace(')', "%29")
}

/// A link's text: the brackets that would end it escaped, on one line.
fn link_text(s: &str) -> String {
    one_line(s).replace('[', "\\[").replace(']', "\\]")
}

fn one_line(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// "1,753.60"; a whole number keeps its two decimals too.
fn money(v: &Value) -> String {
    match v {
        Value::Number(n) => n
            .as_f64()
            .map(super::claims::money)
            .unwrap_or_else(|| n.to_string()),
        other => one_line(&super::util::pystr_or_empty(other)),
    }
}

/// One item as its Markdown (a line; a block as it is). `None` for an item with nothing to show.
pub fn render(it: &Value, zh: bool) -> Option<String> {
    let label = match sget(it, "label_zh") {
        l if zh && !l.trim().is_empty() => l,
        _ => sget(it, "label"),
    };
    let label = one_line(&label);
    let line = |body: String| {
        Some(if label.is_empty() {
            format!("- {body}")
        } else {
            format!("- {label}: {body}")
        })
    };
    match get_str(it, "kind").as_str() {
        "link" => {
            let url = sget(it, "url");
            if url.trim().is_empty() {
                return None;
            }
            let text = match sget(it, "text") {
                t if t.trim().is_empty() => url.trim().to_string(),
                t => t,
            };
            line(format!("[{}]({})", link_text(&text), target(&url)))
        }
        "file" => {
            let path = sget(it, "path");
            let path = path.trim().trim_start_matches('/');
            if path.is_empty() {
                return None;
            }
            let text = match sget(it, "text") {
                t if t.trim().is_empty() => path.rsplit('/').next().unwrap_or(path).to_string(),
                t => t,
            };
            let url = format!(
                "/api/files/raw?path={}",
                utf8_percent_encode(path, NON_ALPHANUMERIC)
            );
            line(format!("[{}]({url})", link_text(&text)))
        }
        "reference" => {
            let v = one_line(&sget(it, "value"));
            (!v.is_empty()).then_some(())?;
            line(format!("**{v}**"))
        }
        "amount" => {
            let v = it.get("value").filter(|v| !v.is_null())?;
            let amount = money(v);
            if amount.is_empty() {
                return None;
            }
            let cur = one_line(&sget(it, "currency"));
            line(if cur.is_empty() {
                amount
            } else {
                format!("{cur} {amount}")
            })
        }
        "fact" => {
            let v = one_line(&sget(it, "value"));
            (!v.is_empty()).then_some(())?;
            line(v)
        }
        "block" => {
            let v = sget(it, "value").trim().to_string();
            (!v.is_empty()).then_some(v)
        }
        _ => None,
    }
}

/// The line that opens the appended block (a Markdown rule: `<hr>` on the page): the block is everything after the
/// last one, so a reader (the e2e helpers) finds it without reading the reply.
pub const MARK: &str = "---";

/// The answer with the items appended: after the reply, a rule ([`MARK`]) and the items, each once. Appending again is
/// a no-op: a line already in the answer's block (string equality of the code's own rendered line) is not added twice;
/// the reply's prose is never read.
pub fn append(answer: &str, items: &[Value], zh: bool) -> String {
    let block_now = answer
        .rsplit_once(&format!("\n{MARK}\n"))
        .map(|(_, b)| b)
        .unwrap_or("");
    let have: Vec<&str> = block_now.lines().map(str::trim).collect();
    let mut lines: Vec<String> = Vec::new();
    for r in items.iter().filter_map(|it| render(it, zh)) {
        let dup = if r.contains('\n') {
            block_now.contains(&r)
        } else {
            have.contains(&r.trim())
        };
        if !dup && !lines.contains(&r) {
            lines.push(r);
        }
    }
    if lines.is_empty() {
        return answer.to_string();
    }
    let mut out = answer.trim_end().to_string();
    out.push_str(&format!("\n\n{MARK}\n"));
    // a block (a table) stands apart from the list lines around it
    let mut prev_block = true;
    for l in &lines {
        let is_block = !l.starts_with("- ");
        out.push_str(if is_block || prev_block { "\n" } else { "" });
        out.push_str(l);
        out.push('\n');
        prev_block = is_block;
    }
    out.trim_end().to_string()
}

/// The run's own structured facts (agent/guards.rs) as items to show: a line each, a table as a block.
pub fn run_fact_items(facts: &[String]) -> Vec<Value> {
    facts
        .iter()
        .filter(|f| !f.trim().is_empty())
        .map(|f| {
            if f.contains('\n') {
                block(f)
            } else {
                fact("", "", f)
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_kind_renders_as_one_markdown_line() {
        let r = |v: Value| render(&v, false).unwrap();
        assert_eq!(
            r(reference("Confirmation number", "确认号", "TH7D3K9Q2")),
            "- Confirmation number: **TH7D3K9Q2**"
        );
        assert_eq!(
            render(
                &reference("Confirmation number", "确认号", "TH7D3K9Q2"),
                true
            )
            .unwrap(),
            "- 确认号: **TH7D3K9Q2**"
        );
        assert_eq!(
            r(link(
                "HTML edition",
                "",
                "The Tidal Ledger",
                "/artifacts/news/a b.html"
            )),
            "- HTML edition: [The Tidal Ledger](/artifacts/news/a%20b.html)"
        );
        assert_eq!(
            r(link("Official page", "", "", "https://x.example/p")),
            "- Official page: [https://x.example/p](https://x.example/p)"
        );
        assert_eq!(
            r(file("PDF", "", "", "reports/a b.pdf")),
            "- PDF: [a b.pdf](/api/files/raw?path=reports%2Fa%20b%2Epdf)"
        );
        assert_eq!(
            r(amount("Amount charged", "", 1454.0, "SGD")),
            "- Amount charged: SGD 1,454.00"
        );
        assert_eq!(
            r(fact("Next edition", "", "Monday 2 November\n08:00")),
            "- Next edition: Monday 2 November 08:00"
        );
        assert_eq!(r(fact("", "", "Terms change.")), "- Terms change.");
        assert_eq!(r(block("| a |\n|---|")), "| a |\n|---|");
        assert!(render(&reference("Ref", "", " "), false).is_none());
        assert!(render(&json!({"kind": "other", "value": "x"}), false).is_none());
    }

    #[test]
    fn the_block_goes_after_the_reply_once() {
        let items = vec![
            reference("Confirmation number", "", "TH7D3K9Q2"),
            amount("Refund", "", 120.0, "HKD"),
            reference("Confirmation number", "", "TH7D3K9Q2"),
        ];
        let out = append("Your card is submitted.", &items, false);
        assert_eq!(
            out,
            "Your card is submitted.\n\n---\n\n- Confirmation number: **TH7D3K9Q2**\n- Refund: HKD 120.00"
        );
        // appending again adds nothing; a new item goes into a block of its own after the last rule
        assert_eq!(append(&out, &items, false), out);
        let more = append(&out, &[fact("Deadline", "", "1 December 2026")], false);
        assert!(
            more.ends_with("\n\n---\n\n- Deadline: 1 December 2026"),
            "{more}"
        );
        // nothing to show: the reply as it is
        assert_eq!(append("ok", &[], false), "ok");
    }

    #[test]
    fn only_this_requests_trusted_results_are_collected() {
        let call = |id: &str, name: &str| {
            json!({"role": "assistant", "content": "", "tool_calls": [{"id": id, "type": "function",
                   "function": {"name": name, "arguments": "{}"}}]})
        };
        let tool = |id: &str, v: Value| json!({"role": "tool", "tool_call_id": id, "content": v.to_string()});
        let shown = |r: &str| json!({"status": "submitted", "show": [reference("Ref", "", r)]});
        let t = vec![
            json!({"role": "user", "content": "first"}),
            call("res_1", "arrival_card_fill_result"),
            tool("res_1", shown("OLD1")),
            json!({"role": "user", "content": "second"}),
            call("c1", "browser_read"),
            tool("c1", shown("PAGE")),
            call("res_2", "arrival_card_fill_result"),
            tool("res_2", shown("NEW2")),
        ];
        let got: Vec<String> = collect(&t).iter().map(|i| sget(i, "value")).collect();
        assert_eq!(got, vec!["NEW2"]);
    }
}
