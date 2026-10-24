//! Notion connector (official REST API, internal-integration token).
//!
//! The user creates an internal integration at notion.so/my-integrations, copies its secret (ntn_… / secret_…)
//! and shares the pages/databases OMuse may use with that integration ("Connections" menu on the page).

use std::future::Future;
use std::pin::Pin;
use std::sync::LazyLock;
use std::time::Duration;

use omuse_common::util::{get_str, get_truthy, py_str, take_chars, truthy};
use regex::Regex;
use serde_json::{json, Map, Value};

use crate::py::httpx_name;

/// `Notion-Version` header.
pub const NOTION_VERSION: &str = "2022-06-28";

/// Python `NotionError`: the message the agent / UI sees.
#[derive(Debug, Clone, thiserror::Error)]
#[error("{0}")]
pub struct NotionError(pub String);

fn obj(v: &Value) -> &Map<String, Value> {
    static EMPTY: LazyLock<Map<String, Value>> = LazyLock::new(Map::new);
    v.as_object().unwrap_or(&EMPTY)
}

fn arr(v: Option<&Value>) -> &[Value] {
    v.and_then(Value::as_array)
        .map(|a| a.as_slice())
        .unwrap_or(&[])
}

/// Accept a page/database id or a Notion URL; return the dashed uuid.
pub fn norm_id(reference: &str) -> Result<String, NotionError> {
    static HEX32_END: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"(?i)([0-9a-f]{32})$").unwrap());
    static HEX32_FULL: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"(?i)^[0-9a-f]{32}$").unwrap());
    let r = reference.trim();
    let raw = if r.starts_with("http://")
        || r.starts_with("https://")
        || r.contains("notion.so/")
        || r.contains("notion.site/")
    {
        let full = if r.contains("://") {
            r.to_string()
        } else {
            format!("https://{}", r)
        };
        let cand = match url::Url::parse(&full) {
            Ok(u) => {
                let p = u
                    .query_pairs()
                    .find(|(k, _)| k == "p")
                    .map(|(_, v)| v.to_string())
                    .unwrap_or_default();
                if p.is_empty() {
                    u.path()
                        .trim_end_matches('/')
                        .rsplit('/')
                        .next()
                        .unwrap_or("")
                        .to_string()
                } else {
                    p
                }
            }
            Err(_) => String::new(),
        };
        HEX32_END
            .captures(&cand.replace('-', ""))
            .map(|c| c[1].to_string())
    } else {
        let c = r.replace('-', "");
        HEX32_FULL.is_match(&c).then_some(c)
    };
    let raw = raw
        .ok_or_else(|| {
            NotionError(format!(
                "无法识别的 Notion 页面/数据库 ID：{} (invalid id)",
                take_chars(r, 80)
            ))
        })?
        .to_lowercase();
    Ok(format!(
        "{}-{}-{}-{}-{}",
        &raw[..8],
        &raw[8..12],
        &raw[12..16],
        &raw[16..20],
        &raw[20..]
    ))
}

// ------------------------------------------------------------------ rich text <-> markdown

/// Notion rich text → inline markdown.
pub fn rt_to_md(rts: &Value) -> String {
    let mut out = String::new();
    for r in arr(Some(rts)) {
        let mut t = get_str(r, "plain_text");
        if t.is_empty() {
            t = r
                .get("text")
                .map(|x| get_str(x, "content"))
                .unwrap_or_default();
        }
        let a = r.get("annotations").cloned().unwrap_or(Value::Null);
        if get_truthy(&a, "code") {
            t = format!("`{}`", t);
        }
        if get_truthy(&a, "bold") {
            t = format!("**{}**", t);
        }
        if get_truthy(&a, "italic") {
            t = format!("*{}*", t);
        }
        if get_truthy(&a, "strikethrough") {
            t = format!("~~{}~~", t);
        }
        let mut href = get_str(r, "href");
        if href.is_empty() {
            href = r
                .get("text")
                .and_then(|x| x.get("link"))
                .map(|l| get_str(l, "url"))
                .unwrap_or_default();
        }
        if !href.is_empty() {
            t = format!("[{}]({})", t, href);
        }
        out.push_str(&t);
    }
    out
}

static INLINE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(\*\*[^*]+\*\*|`[^`]+`|\[[^\]]+\]\([^)\s]+\)|\*[^*]+\*)").unwrap()
});

fn char_slice(s: &str, from: usize, drop_end: usize) -> String {
    let n = s.chars().count();
    s.chars()
        .skip(from)
        .take(n.saturating_sub(from + drop_end))
        .collect()
}

/// Inline markdown → Notion rich-text objects (at most 100, 1900 characters each).
pub fn md_to_rt(text: &str) -> Vec<Value> {
    // re.split with one capturing group: the pieces between matches and the matches, in order
    let mut parts: Vec<&str> = Vec::new();
    let mut last = 0;
    for m in INLINE.find_iter(text) {
        parts.push(&text[last..m.start()]);
        parts.push(m.as_str());
        last = m.end();
    }
    parts.push(&text[last..]);
    let mut out = Vec::new();
    for part in parts {
        if part.is_empty() {
            continue;
        }
        let n = part.chars().count();
        let mut ann = Map::new();
        let mut link: Option<String> = None;
        let content: String;
        if part.starts_with("**") && part.ends_with("**") && n > 4 {
            ann.insert("bold".into(), json!(true));
            content = char_slice(part, 2, 2);
        } else if part.starts_with('`') && part.ends_with('`') && n > 2 {
            ann.insert("code".into(), json!(true));
            content = char_slice(part, 1, 1);
        } else if part.starts_with('[') && part.contains("](") && part.ends_with(')') {
            let i = part.find("](").unwrap_or(0);
            content = part[1..i].to_string();
            link = Some(part[i + 2..part.len() - 1].to_string());
        } else if part.starts_with('*') && part.ends_with('*') && n > 2 {
            ann.insert("italic".into(), json!(true));
            content = char_slice(part, 1, 1);
        } else {
            content = part.to_string();
        }
        let chars: Vec<char> = content.chars().collect();
        let mut i = 0;
        loop {
            let chunk: String = chars[i..(i + 1900).min(chars.len())].iter().collect();
            let mut o = json!({"type": "text", "text": {"content": chunk}});
            if let Some(l) = &link {
                if l.starts_with("http://") || l.starts_with("https://") {
                    o["text"]["link"] = json!({"url": l});
                }
            }
            if !ann.is_empty() {
                o["annotations"] = Value::Object(ann.clone());
            }
            out.push(o);
            i += 1900;
            if i >= chars.len() {
                break;
            }
        }
    }
    out.truncate(100);
    out
}

const CODE_LANGS: &[&str] = &[
    "python",
    "javascript",
    "json",
    "bash",
    "shell",
    "sql",
    "yaml",
    "markdown",
    "html",
    "css",
    "typescript",
    "go",
    "java",
];

fn block(kind: &str, text: &str, extra: Vec<(&str, Value)>) -> Value {
    let mut inner = Map::new();
    inner.insert("rich_text".into(), Value::Array(md_to_rt(text)));
    for (k, v) in extra {
        inner.insert(k.into(), v);
    }
    json!({"object": "block", "type": kind, kind: inner})
}

fn code_block(body: &str, lang: &str) -> Value {
    json!({"object": "block", "type": "code", "code": {
        "rich_text": [{"type": "text", "text": {"content": take_chars(body, 1900)}}], "language": lang}})
}

/// Markdown → Notion blocks (at most 100).
pub fn md_to_blocks(md: &str) -> Vec<Value> {
    static TODO: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[-*] \[( |x|X)\] ").unwrap());
    static NUM: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^\d+[.)] ").unwrap());
    static SEP: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^\|[\s:|-]+\|$").unwrap());
    static BREAK: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"^(#{1,3} |[-*•] |\d+[.)] |> |```|\||---)").unwrap());
    let text = md.replace("\r\n", "\n");
    let lines: Vec<&str> = text.split('\n').collect();
    let mut blocks = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        let s = lines[i].trim();
        if let Some(rest) = s.strip_prefix("```") {
            let lang = rest.trim();
            let lang = if lang.is_empty() { "plain text" } else { lang };
            let mut body = Vec::new();
            i += 1;
            while i < lines.len() && !lines[i].trim().starts_with("```") {
                body.push(lines[i]);
                i += 1;
            }
            let lang = if CODE_LANGS.contains(&lang) {
                lang
            } else {
                "plain text"
            };
            blocks.push(code_block(&body.join("\n"), lang));
        } else if s.is_empty() {
        } else if s == "---" || s == "***" {
            blocks.push(json!({"object": "block", "type": "divider", "divider": {}}));
        } else if let Some(t) = s.strip_prefix("### ") {
            blocks.push(block("heading_3", t, vec![]));
        } else if let Some(t) = s.strip_prefix("## ") {
            blocks.push(block("heading_2", t, vec![]));
        } else if let Some(t) = s.strip_prefix("# ") {
            blocks.push(block("heading_1", t, vec![]));
        } else if TODO.is_match(s) {
            let checked = s.as_bytes()[3].eq_ignore_ascii_case(&b'x');
            blocks.push(block("to_do", &s[6..], vec![("checked", json!(checked))]));
        } else if let Some(t) = s
            .strip_prefix("- ")
            .or_else(|| s.strip_prefix("* "))
            .or_else(|| s.strip_prefix("• "))
        {
            blocks.push(block("bulleted_list_item", t, vec![]));
        } else if NUM.is_match(s) {
            blocks.push(block("numbered_list_item", &NUM.replace(s, ""), vec![]));
        } else if let Some(t) = s.strip_prefix("> ") {
            blocks.push(block("quote", t, vec![]));
        } else if s.starts_with('|') && s.ends_with('|') {
            let mut rows = Vec::new();
            while i < lines.len() && lines[i].trim().starts_with('|') {
                let t = lines[i].trim();
                if !SEP.is_match(t) {
                    rows.push(t);
                }
                i += 1;
            }
            i -= 1;
            blocks.push(code_block(&rows.join("\n"), "plain text"));
        } else {
            let mut para = vec![s];
            while i + 1 < lines.len()
                && !lines[i + 1].trim().is_empty()
                && !BREAK.is_match(lines[i + 1].trim())
            {
                i += 1;
                para.push(lines[i].trim());
            }
            blocks.push(block("paragraph", &para.join(" "), vec![]));
        }
        i += 1;
    }
    blocks.truncate(100);
    blocks
}

/// Notion blocks (with `_children`) → markdown lines.
pub fn blocks_to_md(blocks: &[Value], depth: usize) -> Vec<String> {
    let ind = "  ".repeat(depth);
    let mut out = Vec::new();
    for bl in blocks {
        let t = get_str(bl, "type");
        let d = bl
            .get(&t)
            .filter(|v| truthy(v))
            .cloned()
            .unwrap_or(json!({}));
        let txt = rt_to_md(d.get("rich_text").unwrap_or(&Value::Null));
        let id = bl.get("id").map(py_str).unwrap_or_else(|| "None".into());
        let line = match t.as_str() {
            "paragraph" => txt,
            "heading_1" => format!("# {}", txt),
            "heading_2" => format!("## {}", txt),
            "heading_3" => format!("### {}", txt),
            "bulleted_list_item" => format!("- {}", txt),
            "numbered_list_item" => format!("1. {}", txt),
            "quote" => format!("> {}", txt),
            "to_do" => format!(
                "- [{}] {}",
                if get_truthy(&d, "checked") { "x" } else { " " },
                txt
            ),
            "toggle" => format!("▸ {}", txt),
            "callout" => format!("> 💡 {}", txt),
            "code" => format!("```\n{}\n```", txt),
            "divider" => "---".to_string(),
            "child_page" => format!("📄 子页面 child page: {} (id {})", get_str(&d, "title"), id),
            "child_database" => format!(
                "🗂 子数据库 child database: {} (id {})",
                get_str(&d, "title"),
                id
            ),
            "bookmark" => format!("🔖 {}", get_str(&d, "url")),
            "equation" => format!("$${}$$", get_str(&d, "expression")),
            "image" => "[图片 image]".to_string(),
            "file" => "[文件 file]".to_string(),
            "pdf" => "[PDF]".to_string(),
            "table" => "[表格 table]".to_string(),
            "table_row" => arr(d.get("cells"))
                .iter()
                .map(rt_to_md)
                .collect::<Vec<_>>()
                .join(" | "),
            other => format!("[{}]", other),
        };
        out.push(format!("{}{}", ind, line));
        if let Some(ch) = bl.get("_children").filter(|v| truthy(v)) {
            out.extend(blocks_to_md(arr(Some(ch)), depth + 1));
        }
    }
    out
}

/// A page property → display text.
pub fn prop_to_text(p: &Value) -> String {
    let t = get_str(p, "type");
    let null = Value::Null;
    let v = p.get(&t).unwrap_or(&null);
    match t.as_str() {
        "title" | "rich_text" => rt_to_md(v),
        "select" | "status" => get_str(v, "name"),
        "multi_select" => arr(Some(v))
            .iter()
            .map(|x| get_str(x, "name"))
            .collect::<Vec<_>>()
            .join(", "),
        "date" => {
            let start = get_str(v, "start");
            let end = v.get("end").filter(|e| truthy(e)).map(py_str);
            match end {
                Some(e) => format!("{} → {}", start, e),
                None => start,
            }
        }
        "people" => arr(Some(v))
            .iter()
            .map(|x| {
                x.get("name")
                    .map(py_str)
                    .unwrap_or_else(|| get_str(x, "id"))
            })
            .collect::<Vec<_>>()
            .join(", "),
        "number" | "checkbox" | "url" | "email" | "phone_number" | "created_time"
        | "last_edited_time" => {
            if v.is_null() {
                String::new()
            } else {
                py_str(v)
            }
        }
        "formula" => {
            let kind = get_str(v, "type");
            v.get(&kind).map(py_str).unwrap_or_default()
        }
        "relation" => arr(Some(v))
            .iter()
            .map(|x| get_str(x, "id"))
            .collect::<Vec<_>>()
            .join(", "),
        _ => String::new(),
    }
}

const UNTITLED: &str = "(无标题 untitled)";

/// The page's (or database's) title.
pub fn page_title(page: &Value) -> String {
    for p in obj(page.get("properties").unwrap_or(&Value::Null)).values() {
        if get_str(p, "type") == "title" {
            let t = rt_to_md(p.get("title").unwrap_or(&Value::Null));
            return if t.is_empty() { UNTITLED.into() } else { t };
        }
    }
    if get_str(page, "object") == "database" {
        let t = rt_to_md(page.get("title").unwrap_or(&Value::Null));
        return if t.is_empty() { UNTITLED.into() } else { t };
    }
    UNTITLED.into()
}

/// A compact view of a page: id, object, title, url, last_edited_time, archived, non-empty properties.
pub fn page_brief(page: &Value) -> Value {
    let mut props = Map::new();
    for (k, v) in obj(page.get("properties").unwrap_or(&Value::Null)) {
        if get_str(v, "type") != "title" {
            let t = prop_to_text(v);
            if !t.is_empty() {
                props.insert(k.clone(), Value::String(t));
            }
        }
    }
    json!({
        "id": page.get("id").cloned().unwrap_or(Value::Null),
        "object": page.get("object").cloned().unwrap_or(Value::Null),
        "title": page_title(page),
        "url": page.get("url").cloned().unwrap_or(json!("")),
        "last_edited_time": page.get("last_edited_time").cloned().unwrap_or(json!("")),
        "archived": page.get("archived").cloned().unwrap_or(json!(false)),
        "properties": props,
    })
}

fn clamp(limit: i64, default: i64, max: i64) -> i64 {
    let l = if limit == 0 { default } else { limit };
    l.clamp(1, max)
}

/// The Notion API client (one integration token).
pub struct Notion {
    client: reqwest::Client,
    api: String,
    token: String,
    timeout: Duration,
}

type BoxFut<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

impl Notion {
    /// `api` is `cfg.endpoints.notion` (`https://api.notion.com`).
    pub fn new(client: reqwest::Client, api: &str, token: &str) -> Self {
        Self {
            client,
            api: api.trim_end_matches('/').to_string(),
            token: token.to_string(),
            timeout: Duration::from_secs(30),
        }
    }

    pub fn with_timeout(mut self, secs: f64) -> Self {
        self.timeout = Duration::from_secs_f64(secs);
        self
    }

    async fn req(
        &self,
        method: reqwest::Method,
        path: &str,
        body: Option<&Value>,
        params: &[(&str, String)],
    ) -> Result<Value, NotionError> {
        for attempt in 0..3 {
            let mut rb = self
                .client
                .request(method.clone(), format!("{}{}", self.api, path))
                .timeout(self.timeout)
                .header("Authorization", format!("Bearer {}", self.token))
                .header("Notion-Version", NOTION_VERSION)
                .header("Content-Type", "application/json");
            if !params.is_empty() {
                rb = rb.query(params);
            }
            if let Some(b) = body {
                rb = rb.json(b);
            }
            let r = rb.send().await.map_err(|e| {
                NotionError(format!("无法连接 Notion (network): {}", httpx_name(&e)))
            })?;
            let status = r.status().as_u16();
            if status == 429 && attempt < 2 {
                let wait = r
                    .headers()
                    .get("retry-after")
                    .and_then(|v| v.to_str().ok())
                    .filter(|s| !s.is_empty())
                    .and_then(|s| s.trim().parse::<f64>().ok())
                    .unwrap_or(1.0)
                    .min(5.0);
                tokio::time::sleep(Duration::from_secs_f64(wait.max(0.0))).await;
                continue;
            }
            let data: Value = r.json().await.unwrap_or_else(|_| json!({}));
            if status == 401 {
                return Err(NotionError(
                    "Notion 令牌无效或已被撤销 (invalid token)".into(),
                ));
            }
            if status == 404 {
                return Err(NotionError(
                    "找不到这个页面/数据库，或者还没有把它共享给 OMuse 集成 (not found — share the page with your integration via ••• → Connections)".into(),
                ));
            }
            if status >= 400 {
                let msg = data.get("message").map(py_str).unwrap_or_default();
                return Err(NotionError(format!(
                    "Notion 错误 {}: {}",
                    status,
                    take_chars(&msg, 300)
                )));
            }
            return Ok(data);
        }
        Err(NotionError("Notion 请求过于频繁 (rate limited)".into()))
    }

    // ---------------------------------------------------------------- read

    /// `GET /v1/users/me` — the integration's bot user.
    pub async fn me(&self) -> Result<Value, NotionError> {
        self.req(reqwest::Method::GET, "/v1/users/me", None, &[])
            .await
    }

    /// Search pages/databases shared with the integration; `kind` is `page`, `database` or "".
    pub async fn search(
        &self,
        query: &str,
        kind: &str,
        limit: i64,
    ) -> Result<Vec<Value>, NotionError> {
        let mut body = json!({"page_size": clamp(limit, 10, 50)});
        if !query.is_empty() {
            body["query"] = json!(query);
        }
        if kind == "page" || kind == "database" {
            body["filter"] = json!({"property": "object", "value": kind});
        }
        body["sort"] = json!({"direction": "descending", "timestamp": "last_edited_time"});
        let d = self
            .req(reqwest::Method::POST, "/v1/search", Some(&body), &[])
            .await?;
        Ok(arr(d.get("results")).iter().map(page_brief).collect())
    }

    /// Child blocks of a block/page, one level of nesting, at most ~300 blocks.
    pub async fn children(&self, block_id: &str) -> Result<Vec<Value>, NotionError> {
        let mut budget = 300i64;
        self.children_rec(block_id.to_string(), 0, &mut budget)
            .await
    }

    fn children_rec<'a>(
        &'a self,
        block_id: String,
        depth: u32,
        budget: &'a mut i64,
    ) -> BoxFut<'a, Result<Vec<Value>, NotionError>> {
        Box::pin(async move {
            let mut out = Vec::new();
            let mut cursor: Option<String> = None;
            while *budget > 0 {
                let mut params = vec![("page_size", "100".to_string())];
                if let Some(c) = &cursor {
                    params.push(("start_cursor", c.clone()));
                }
                let data = self
                    .req(
                        reqwest::Method::GET,
                        &format!("/v1/blocks/{}/children", block_id),
                        None,
                        &params,
                    )
                    .await?;
                for bl in arr(data.get("results")) {
                    let mut bl = bl.clone();
                    *budget -= 1;
                    let t = get_str(&bl, "type");
                    if get_truthy(&bl, "has_children")
                        && depth < 1
                        && t != "child_page"
                        && t != "child_database"
                        && *budget > 0
                    {
                        let kids = self
                            .children_rec(get_str(&bl, "id"), depth + 1, budget)
                            .await?;
                        bl["_children"] = Value::Array(kids);
                    }
                    out.push(bl);
                }
                if !get_truthy(&data, "has_more") {
                    break;
                }
                cursor = data.get("next_cursor").filter(|c| truthy(c)).map(py_str);
                if cursor.is_none() {
                    break;
                }
            }
            Ok(out)
        })
    }

    /// A page as a brief plus its content as markdown (`content`, at most 40000 characters).
    pub async fn get_page(&self, reference: &str) -> Result<Value, NotionError> {
        let pid = norm_id(reference)?;
        let page = self
            .req(
                reqwest::Method::GET,
                &format!("/v1/pages/{}", pid),
                None,
                &[],
            )
            .await?;
        let md = blocks_to_md(&self.children(&pid).await?, 0).join("\n");
        let mut out = page_brief(&page);
        out["content"] = json!(take_chars(&md, 40000));
        Ok(out)
    }

    /// `GET /v1/databases/{id}`.
    pub async fn database(&self, reference: &str) -> Result<Value, NotionError> {
        self.req(
            reqwest::Method::GET,
            &format!("/v1/databases/{}", norm_id(reference)?),
            None,
            &[],
        )
        .await
    }

    /// Query a database: `{database, database_id, schema, rows, has_more}`.
    pub async fn query_database(
        &self,
        reference: &str,
        filter: Option<&Value>,
        sorts: Option<&Value>,
        limit: i64,
    ) -> Result<Value, NotionError> {
        let db = self.database(reference).await?;
        let mut body = json!({"page_size": clamp(limit, 20, 100)});
        if let Some(f) = filter.filter(|f| truthy(f)) {
            body["filter"] = f.clone();
        }
        if let Some(s) = sorts.filter(|s| truthy(s)) {
            body["sorts"] = s.clone();
        }
        let db_id = get_str(&db, "id");
        let res = self
            .req(
                reqwest::Method::POST,
                &format!("/v1/databases/{}/query", db_id),
                Some(&body),
                &[],
            )
            .await?;
        let schema: Map<String, Value> = obj(db.get("properties").unwrap_or(&Value::Null))
            .iter()
            .map(|(k, v)| (k.clone(), v.get("type").cloned().unwrap_or(Value::Null)))
            .collect();
        Ok(json!({
            "database": page_title(&db), "database_id": db.get("id").cloned().unwrap_or(Value::Null), "schema": schema,
            "rows": arr(res.get("results")).iter().map(page_brief).collect::<Vec<_>>(),
            "has_more": res.get("has_more").cloned().unwrap_or(json!(false)),
        }))
    }

    // ---------------------------------------------------------------- write

    fn props_for(&self, db: &Value, props: &Value) -> Result<Value, NotionError> {
        let schema = obj(db.get("properties").unwrap_or(&Value::Null));
        let mut out = Map::new();
        for (k, v) in obj(props) {
            let Some(col) = schema.get(k) else {
                return Err(NotionError(format!(
                    "数据库里没有「{}」这一列 (unknown property). 可用 columns: {}",
                    k,
                    schema.keys().cloned().collect::<Vec<_>>().join(", ")
                )));
            };
            let t = get_str(col, "type");
            let empty = v.is_null() || v.as_str() == Some("");
            let val = match t.as_str() {
                "title" => json!({"title": md_to_rt(&py_str(v))}),
                "rich_text" => json!({"rich_text": md_to_rt(&py_str(v))}),
                "select" | "status" => {
                    json!({&t: if empty { Value::Null } else { json!({"name": py_str(v)}) }})
                }
                "multi_select" => {
                    let vals: Vec<String> = match v.as_array() {
                        Some(a) => a.iter().map(py_str).collect(),
                        None => py_str(v)
                            .split(',')
                            .map(str::trim)
                            .filter(|x| !x.is_empty())
                            .map(String::from)
                            .collect(),
                    };
                    json!({"multi_select": vals.iter().map(|x| json!({"name": x})).collect::<Vec<_>>()})
                }
                "date" => {
                    if v.is_object() {
                        json!({"date": v})
                    } else if truthy(v) {
                        json!({"date": {"start": py_str(v)}})
                    } else {
                        json!({"date": null})
                    }
                }
                "checkbox" => {
                    let b = match v {
                        Value::Bool(b) => *b,
                        other => ["true", "yes", "1", "是", "✓"]
                            .contains(&py_str(other).to_lowercase().as_str()),
                    };
                    json!({"checkbox": b})
                }
                "number" => {
                    if empty {
                        json!({"number": null})
                    } else {
                        let f = omuse_common::util::as_f64(v).ok_or_else(|| {
                            NotionError(format!(
                                "could not convert string to float: {}",
                                omuse_common::pyjson::py_repr(v)
                            ))
                        })?;
                        json!({"number": f})
                    }
                }
                "url" | "email" | "phone_number" => {
                    let s = py_str(v);
                    json!({&t: if s.is_empty() { Value::Null } else { Value::String(s) }})
                }
                _ => {
                    return Err(NotionError(format!(
                        "暂不支持写入「{}」({}) 类型的列 (unsupported property type)",
                        k, t
                    )))
                }
            };
            out.insert(k.clone(), val);
        }
        Ok(Value::Object(out))
    }

    /// Create a page under a page or as a database row (typed `properties` only for rows).
    pub async fn create_page(
        &self,
        parent: &str,
        title: &str,
        content: &str,
        properties: Option<&Value>,
    ) -> Result<Value, NotionError> {
        let pid = norm_id(parent)?;
        let blocks = md_to_blocks(content);
        let db = self
            .req(
                reqwest::Method::GET,
                &format!("/v1/databases/{}", pid),
                None,
                &[],
            )
            .await
            .ok();
        let props_in = properties.cloned().unwrap_or(json!({}));
        let mut body = if let Some(db) = db.filter(truthy) {
            let mut props = self.props_for(&db, &props_in)?;
            let tkey = obj(db.get("properties").unwrap_or(&Value::Null))
                .iter()
                .find(|(_, v)| get_str(v, "type") == "title")
                .map(|(k, _)| k.clone())
                .ok_or_else(|| {
                    NotionError("数据库没有标题列 (database has no title property)".into())
                })?;
            if props.get(&tkey).is_none() {
                props[&tkey] = json!({"title": md_to_rt(title)});
            }
            json!({"parent": {"database_id": pid}, "properties": props})
        } else {
            if truthy(&props_in) {
                return Err(NotionError(
                    "父级是普通页面时不能设置属性列 (properties only for database rows)".into(),
                ));
            }
            json!({"parent": {"page_id": pid}, "properties": {"title": {"title": md_to_rt(title)}}})
        };
        if !blocks.is_empty() {
            body["children"] = Value::Array(blocks);
        }
        let page = self
            .req(reqwest::Method::POST, "/v1/pages", Some(&body), &[])
            .await?;
        let mut out = json!({"created": true});
        for (k, v) in obj(&page_brief(&page)) {
            out[k] = v.clone();
        }
        Ok(out)
    }

    /// Append markdown as blocks to a page.
    pub async fn append(&self, reference: &str, content: &str) -> Result<Value, NotionError> {
        let pid = norm_id(reference)?;
        let blocks = md_to_blocks(content);
        if blocks.is_empty() {
            return Err(NotionError("没有要追加的内容 (empty content)".into()));
        }
        let n = blocks.len();
        self.req(
            reqwest::Method::PATCH,
            &format!("/v1/blocks/{}/children", pid),
            Some(&json!({"children": blocks})),
            &[],
        )
        .await?;
        Ok(json!({"appended_blocks": n, "page_id": pid}))
    }

    /// Update properties / title / archived flag of a page.
    pub async fn update_page(
        &self,
        reference: &str,
        properties: Option<&Value>,
        archived: Option<bool>,
        title: Option<&str>,
    ) -> Result<Value, NotionError> {
        let pid = norm_id(reference)?;
        let page = self
            .req(
                reqwest::Method::GET,
                &format!("/v1/pages/{}", pid),
                None,
                &[],
            )
            .await?;
        let mut body = Map::new();
        let parent = page.get("parent").cloned().unwrap_or(json!({}));
        if let Some(props) = properties.filter(|p| truthy(p)) {
            if get_str(&parent, "type") != "database_id" {
                return Err(NotionError(
                    "只有数据库里的行才能改属性列 (properties only for database rows)".into(),
                ));
            }
            let db = self.database(&get_str(&parent, "database_id")).await?;
            body.insert("properties".into(), self.props_for(&db, props)?);
        }
        if let Some(t) = title.filter(|t| !t.is_empty()) {
            let tkey = obj(page.get("properties").unwrap_or(&Value::Null))
                .iter()
                .find(|(_, v)| get_str(v, "type") == "title")
                .map(|(k, _)| k.clone())
                .unwrap_or_else(|| "title".into());
            let props = body.entry("properties").or_insert_with(|| json!({}));
            props[&tkey] = json!({"title": md_to_rt(t)});
        }
        if let Some(a) = archived {
            body.insert("archived".into(), json!(a));
        }
        if body.is_empty() {
            return Err(NotionError("没有要修改的内容 (nothing to update)".into()));
        }
        let page = self
            .req(
                reqwest::Method::PATCH,
                &format!("/v1/pages/{}", pid),
                Some(&Value::Object(body)),
                &[],
            )
            .await?;
        let mut out = json!({"updated": true});
        for (k, v) in obj(&page_brief(&page)) {
            out[k] = v.clone();
        }
        Ok(out)
    }

    // ---------------------------------------------------------------- watch

    /// Rows edited on/after `since` (Notion timestamps are rounded to the minute, so callers dedupe):
    /// `(database title, raw pages)`.
    pub async fn edited_since(
        &self,
        reference: &str,
        since: &str,
    ) -> Result<(String, Vec<Value>), NotionError> {
        let db = self.database(reference).await?;
        let body = json!({"page_size": 50, "sorts": [{"timestamp": "last_edited_time", "direction": "ascending"}],
                "filter": {"timestamp": "last_edited_time", "last_edited_time": {"on_or_after": since}}});
        let res = self
            .req(
                reqwest::Method::POST,
                &format!("/v1/databases/{}/query", get_str(&db, "id")),
                Some(&body),
                &[],
            )
            .await?;
        Ok((page_title(&db), arr(res.get("results")).to_vec()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_from_urls_and_raw() {
        assert_eq!(
            norm_id("https://www.notion.so/Q4-Plan-aaaaaaaabbbb4ccc8dddeeeeeeeeeeee").unwrap(),
            "aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee"
        );
        assert_eq!(
            norm_id("11111111-2222-4333-8444-555555555555").unwrap(),
            "11111111-2222-4333-8444-555555555555"
        );
        assert_eq!(
            norm_id("www.notion.so/x?p=AAAAAAAABBBB4CCC8DDDEEEEEEEEEEEE&v=1").unwrap(),
            "aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee"
        );
        let e = norm_id("nope").unwrap_err();
        assert!(e.0.contains("invalid id") && e.0.contains("nope"));
    }

    #[test]
    fn markdown_to_blocks_and_back() {
        let b = md_to_blocks("# Hi\n- a\n- [ ] b\n- [x] c\n1. one\n> q\n```json\n{}\n```\n|a|b|\n|-|-|\n|1|2|\n\npara line\ncontinued\n---\n• dot");
        let kinds: Vec<&str> = b.iter().map(|x| x["type"].as_str().unwrap()).collect();
        assert_eq!(
            kinds,
            [
                "heading_1",
                "bulleted_list_item",
                "to_do",
                "to_do",
                "numbered_list_item",
                "quote",
                "code",
                "code",
                "paragraph",
                "divider",
                "bulleted_list_item"
            ]
        );
        assert_eq!(b[2]["to_do"]["checked"], json!(false));
        assert_eq!(b[3]["to_do"]["checked"], json!(true));
        assert_eq!(b[6]["code"]["language"], json!("json"));
        assert_eq!(
            b[7]["code"]["rich_text"][0]["text"]["content"],
            json!("|a|b|\n|1|2|")
        );
        assert_eq!(
            b[8]["paragraph"]["rich_text"][0]["text"]["content"],
            json!("para line continued")
        );
        assert_eq!(
            b[10]["bulleted_list_item"]["rich_text"][0]["text"]["content"],
            json!("dot")
        );
        let md = blocks_to_md(&b, 0).join("\n");
        assert!(md.starts_with("# Hi\n- a\n- [ ] b\n- [x] c\n1. one\n> q\n```\n{}\n```"));
    }

    #[test]
    fn rich_text_inline() {
        let rt = md_to_rt("plain **bold** `code` [link](https://x.test/a) *it* [bad](ftp://x)");
        assert_eq!(rt.len(), 10);
        assert_eq!(rt[1]["annotations"], json!({"bold": true}));
        assert_eq!(rt[1]["text"]["content"], json!("bold"));
        assert_eq!(rt[3]["annotations"], json!({"code": true}));
        assert_eq!(
            rt[5]["text"],
            json!({"content": "link", "link": {"url": "https://x.test/a"}})
        );
        assert_eq!(rt[7]["annotations"], json!({"italic": true}));
        assert_eq!(rt[9]["text"], json!({"content": "bad"}));
        let back = rt_to_md(
            &json!([{"plain_text": "b", "annotations": {"bold": true}, "href": "https://y"}, {"text": {"content": "c"}}]),
        );
        assert_eq!(back, "[**b**](https://y)c");
        let long = md_to_rt(&"字".repeat(4000));
        assert_eq!(long.len(), 3);
        assert_eq!(
            long[0]["text"]["content"].as_str().unwrap().chars().count(),
            1900
        );
    }

    #[test]
    fn briefs_and_properties() {
        let page = json!({"object": "page", "id": "p1", "url": "u", "last_edited_time": "t", "archived": false,
            "properties": {"Name": {"type": "title", "title": [{"plain_text": "Write launch post"}]},
                           "Status": {"type": "status", "status": {"name": "Doing"}},
                           "Due": {"type": "date", "date": {"start": "2026-10-01", "end": "2026-10-02"}},
                           "N": {"type": "number", "number": 1.5}, "Empty": {"type": "rich_text", "rich_text": []},
                           "Done": {"type": "checkbox", "checkbox": true}}});
        let b = page_brief(&page);
        assert_eq!(b["title"], json!("Write launch post"));
        assert_eq!(
            b["properties"],
            json!({"Status": "Doing", "Due": "2026-10-01 → 2026-10-02", "N": "1.5", "Done": "True"})
        );
        assert_eq!(
            page_title(&json!({"object": "database", "title": []})),
            "(无标题 untitled)"
        );
        assert_eq!(
            omuse_common::pyjson::dumps(&b)
                .matches("\"id\": \"p1\", \"object\": \"page\"")
                .count(),
            1
        );
    }
}
