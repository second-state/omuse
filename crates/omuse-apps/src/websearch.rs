//! Web search through a search API — Exa's free MCP, Tavily, Brave Search or Exa — behind
//! `web_search` when the "Web search" connection has a key (the user's own, or the server's default from
//! `[search]`). All three come back in one shape: `{"query", "engine", "results": [{title, url, snippet, age?}]}`,
//! the same the browser's search engines produce, so nothing downstream changes.

use std::fmt;
use std::time::Duration;

use omuse_common::util::get_str;
use serde_json::{json, Value};

/// A search API failure (HTTP, a refused key, a malformed reply).
#[derive(Debug)]
pub struct SearchError(pub String);

impl fmt::Display for SearchError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for SearchError {}

/// The providers. [`Provider::ORDER`] is the order they are tried: Exa's free MCP, then Tavily, Brave, Exa —
/// the first that answers is used and remembered, so it comes first next time (see [`order`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Provider {
    Brave,
    Tavily,
    Exa,
    /// Exa's hosted MCP server, keyless on a rate-limited free tier: the first choice every install has.
    ExaMcp,
}

impl Provider {
    pub const ALL: [Provider; 4] = [
        Provider::Brave,
        Provider::Tavily,
        Provider::Exa,
        Provider::ExaMcp,
    ];
    /// The order the search APIs are tried in by `web_search`, after the browser's own engines (free, and as fast in
    /// the measurements of 2026-10-28): Brave, then Exa's free MCP, then an Exa key. Tavily is not in this chain: it
    /// answers `news_search` (its news topic gives dated articles with snippets).
    pub const ORDER: [Provider; 3] = [Provider::Brave, Provider::ExaMcp, Provider::Exa];
    /// Every provider as listed (keys looked up, the Settings card): the web chain in order, then Tavily for news.
    pub const LISTED: [Provider; 4] = [
        Provider::Brave,
        Provider::ExaMcp,
        Provider::Exa,
        Provider::Tavily,
    ];

    /// What a provider answers: "news" (Tavily) or "web".
    pub fn role(self) -> &'static str {
        if self == Provider::Tavily {
            "news"
        } else {
            "web"
        }
    }

    pub fn parse(s: &str) -> Option<Provider> {
        match s.trim().to_lowercase().as_str() {
            "brave" => Some(Provider::Brave),
            "tavily" => Some(Provider::Tavily),
            "exa" => Some(Provider::Exa),
            "exa-mcp" | "exa_mcp" => Some(Provider::ExaMcp),
            _ => None,
        }
    }

    pub fn id(self) -> &'static str {
        match self {
            Provider::Brave => "brave",
            Provider::Tavily => "tavily",
            Provider::Exa => "exa",
            Provider::ExaMcp => "exa-mcp",
        }
    }

    /// Whether the provider needs a key (the free Exa MCP does not).
    pub fn needs_key(self) -> bool {
        self != Provider::ExaMcp
    }

    /// The engine name in results (`engine`).
    pub fn engine(self) -> &'static str {
        match self {
            Provider::Brave => "brave",
            Provider::Tavily => "tavily",
            Provider::Exa => "exa",
            Provider::ExaMcp => "exa (free)",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Provider::Brave => "Brave Search",
            Provider::Tavily => "Tavily",
            Provider::Exa => "Exa",
            Provider::ExaMcp => "Exa (free MCP)",
        }
    }
}

/// Exa's MCP `web_search_exa` reply is text — blocks of `Title: … / URL: … / Published: … / Highlights: …` — turned
/// into the common rows.
pub fn parse_exa_mcp_text(text: &str, n: usize) -> Vec<Value> {
    let mut rows = Vec::new();
    let mut cur: Option<(String, String, String, Vec<String>)> = None; // title, url, date, snippet lines
    let mut in_high = false;
    let flush = |cur: &mut Option<(String, String, String, Vec<String>)>, rows: &mut Vec<Value>| {
        if let Some((t, u, d, h)) = cur.take() {
            if u.starts_with("http") {
                let snippet = h.join(" ").split_whitespace().collect::<Vec<_>>().join(" ");
                rows.push(row(
                    &t,
                    &u,
                    &snippet.chars().take(400).collect::<String>(),
                    &d,
                ));
            }
        }
    };
    for line in text.lines() {
        let l = line.trim();
        if let Some(t) = l.strip_prefix("Title:") {
            flush(&mut cur, &mut rows);
            cur = Some((
                t.trim().to_string(),
                String::new(),
                String::new(),
                Vec::new(),
            ));
            in_high = false;
        } else if let Some(c) = cur.as_mut() {
            if let Some(u) = l.strip_prefix("URL:") {
                c.1 = u.trim().to_string();
                in_high = false;
            } else if let Some(d) = l.strip_prefix("Published:") {
                let d = d.trim();
                c.2 = if d.eq_ignore_ascii_case("n/a") {
                    String::new()
                } else {
                    d.to_string()
                };
                in_high = false;
            } else if l.starts_with("Author:") || l.starts_with("Score:") || l.starts_with("ID:") {
                in_high = false;
            } else if let Some(rest) = l.strip_prefix("Highlights:") {
                in_high = true;
                if !rest.trim().is_empty() {
                    c.3.push(rest.trim().to_string());
                }
            } else if let Some(rest) = l
                .strip_prefix("Summary:")
                .or_else(|| l.strip_prefix("Text:"))
            {
                in_high = true;
                if !rest.trim().is_empty() {
                    c.3.push(rest.trim().to_string());
                }
            } else if in_high && !l.is_empty() {
                c.3.push(l.trim_start_matches('#').trim().to_string());
            }
        }
    }
    flush(&mut cur, &mut rows);
    rows.truncate(n.clamp(1, 20));
    rows
}

/// One `data:` payload of a streamable-HTTP MCP reply (or the plain JSON body).
fn mcp_payload(text: &str) -> Option<Value> {
    let t = text.trim();
    if t.starts_with('{') {
        return serde_json::from_str(t).ok();
    }
    let mut last = None;
    for line in t.lines() {
        if let Some(d) = line.strip_prefix("data:") {
            if let Ok(v) = serde_json::from_str::<Value>(d.trim()) {
                last = Some(v);
            }
        }
    }
    last
}

/// The providers to try, in order: the one that worked last time first (when it is still available), then
/// [`Provider::ORDER`] without it.
pub fn order(last_ok: Option<Provider>, available: &[Provider]) -> Vec<Provider> {
    let mut out: Vec<Provider> = Vec::new();
    if let Some(p) = last_ok.filter(|p| available.contains(p) && Provider::ORDER.contains(p)) {
        out.push(p);
    }
    for p in Provider::ORDER {
        if available.contains(&p) && !out.contains(&p) {
            out.push(p);
        }
    }
    out
}

/// A search client for one provider and key (the key is "" for the free Exa MCP, whose base is the MCP endpoint).
pub struct WebSearch {
    client: reqwest::Client,
    provider: Provider,
    base: String,
    key: String,
    timeout: Duration,
}

fn arr<'a>(v: &'a Value, path: &[&str]) -> &'a [Value] {
    let mut cur = v;
    for k in path {
        cur = cur.get(*k).unwrap_or(&Value::Null);
    }
    cur.as_array().map_or(&[], Vec::as_slice)
}

fn row(title: &str, url: &str, snippet: &str, age: &str) -> Value {
    let mut r = json!({"title": title.trim(), "url": url.trim(), "snippet": snippet.trim()});
    if !age.trim().is_empty() {
        r["age"] = json!(age.trim());
    }
    r
}

/// Brave's `web.results` or Tavily's / Exa's `results` → the common rows.
pub fn compact(provider: Provider, data: &Value, n: usize) -> Vec<Value> {
    let rows: Vec<Value> = match provider {
        Provider::Brave => arr(data, &["web", "results"])
            .iter()
            .map(|r| {
                let mut snippet = get_str(r, "description");
                let extra: Vec<String> = arr(r, &["extra_snippets"])
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::to_string)
                    .collect();
                if !extra.is_empty() {
                    snippet = format!("{} {}", snippet, extra.join(" "));
                }
                row(
                    &get_str(r, "title"),
                    &get_str(r, "url"),
                    &snippet,
                    &get_str(r, "age"),
                )
            })
            .collect(),
        Provider::Tavily => arr(data, &["results"])
            .iter()
            .map(|r| {
                row(
                    &get_str(r, "title"),
                    &get_str(r, "url"),
                    &get_str(r, "content"),
                    &get_str(r, "published_date"),
                )
            })
            .collect(),
        Provider::Exa => arr(data, &["results"])
            .iter()
            .map(|r| {
                let mut snippet: Vec<String> = arr(r, &["highlights"])
                    .iter()
                    .filter_map(Value::as_str)
                    .map(|s| s.trim().to_string())
                    .collect();
                if snippet.is_empty() {
                    let s = get_str(r, "summary");
                    let t = get_str(r, "text");
                    let pick = if !s.is_empty() { s } else { t };
                    snippet.push(pick.chars().take(400).collect());
                }
                row(
                    &get_str(r, "title"),
                    &get_str(r, "url"),
                    &snippet.join(" "),
                    &get_str(r, "publishedDate"),
                )
            })
            .collect(),
        Provider::ExaMcp => Vec::new(), // its text reply is parsed by `parse_exa_mcp_text`
    };
    rows.into_iter()
        .filter(|r| get_str(r, "url").starts_with("http"))
        .take(n.clamp(1, 20))
        .collect()
}

impl WebSearch {
    pub fn new(client: reqwest::Client, provider: Provider, base: &str, key: &str) -> Self {
        Self {
            client,
            provider,
            base: base.trim_end_matches('/').to_string(),
            key: key.to_string(),
            timeout: Duration::from_secs(30),
        }
    }

    pub fn provider(&self) -> Provider {
        self.provider
    }

    async fn finish(&self, r: reqwest::Response) -> Result<Value, SearchError> {
        let status = r.status().as_u16();
        let text = r
            .text()
            .await
            .map_err(|e| SearchError(format!("{}: {e}", self.provider.label())))?;
        let body: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
        if status == 401 || status == 403 || status == 422 && self.provider == Provider::Brave {
            return Err(SearchError(format!(
                "{} refused the key (HTTP {status})",
                self.provider.label()
            )));
        }
        if status >= 400 {
            let msg = body
                .get("error")
                .map(|e| match e {
                    Value::String(s) => s.clone(),
                    other => get_str(other, "message"),
                })
                .filter(|s| !s.is_empty())
                .or_else(|| Some(get_str(&body, "detail")).filter(|s| !s.is_empty()))
                .unwrap_or_else(|| text.chars().take(200).collect());
            return Err(SearchError(format!(
                "{} HTTP {status}: {msg}",
                self.provider.label()
            )));
        }
        if !body.is_object() {
            return Err(SearchError(format!(
                "{}: unexpected reply",
                self.provider.label()
            )));
        }
        if let Some(e) = body.get("error").and_then(Value::as_str) {
            return Err(SearchError(format!("{}: {e}", self.provider.label())));
        }
        Ok(body)
    }

    /// One search: up to `n` rows in the common shape. A 429 (Brave's free plan allows one request a second) is
    /// retried once after the wait the server asks for, at most three seconds.
    pub async fn search(&self, query: &str, n: usize) -> Result<Value, SearchError> {
        let q = query.trim();
        if q.is_empty() {
            return Err(SearchError("empty query".into()));
        }
        let n = n.clamp(1, 20);
        if self.provider == Provider::ExaMcp {
            return self.search_exa_mcp(q, n).await;
        }
        let mut tries = 0;
        loop {
            tries += 1;
            let r = self
                .request(q, n)
                .send()
                .await
                .map_err(|e| SearchError(format!("{}: {e}", self.provider.label())))?;
            if r.status().as_u16() == 429 && tries == 1 {
                let wait = r
                    .headers()
                    .get("retry-after")
                    .and_then(|v| v.to_str().ok())
                    .and_then(|v| v.trim().parse::<f64>().ok())
                    .unwrap_or(1.2)
                    .clamp(0.5, 3.0);
                tokio::time::sleep(Duration::from_secs_f64(wait)).await;
                continue;
            }
            let data = self.finish(r).await?;
            let results = compact(self.provider, &data, n);
            return Ok(json!({"query": q, "engine": self.provider.engine(), "results": results}));
        }
    }

    /// News: Tavily's `news` topic for the last `days` days (dated articles with snippets). Only Tavily does this.
    pub async fn news(&self, query: &str, n: usize, days: u32) -> Result<Value, SearchError> {
        if self.provider != Provider::Tavily {
            return Err(SearchError("news search needs Tavily".into()));
        }
        let q = query.trim();
        if q.is_empty() {
            return Err(SearchError("empty query".into()));
        }
        let n = n.clamp(1, 20);
        let r = self
            .client
            .post(format!("{}/search", self.base))
            .bearer_auth(&self.key)
            .json(&json!({"query": q, "max_results": n, "search_depth": "basic", "topic": "news", "days": days.clamp(1, 365)}))
            .timeout(self.timeout)
            .send()
            .await
            .map_err(|e| SearchError(format!("Tavily: {e}")))?;
        let data = self.finish(r).await?;
        let results = compact(Provider::Tavily, &data, n);
        Ok(json!({"query": q, "engine": "tavily news", "days": days, "results": results}))
    }

    /// Exa's MCP server: `initialize`, then `tools/call web_search_exa` (streamable HTTP, no session, no key).
    async fn search_exa_mcp(&self, q: &str, n: usize) -> Result<Value, SearchError> {
        let post = |body: Value| {
            self.client
                .post(&self.base)
                .header("Content-Type", "application/json")
                .header("Accept", "application/json, text/event-stream")
                .json(&body)
                .timeout(self.timeout)
                .send()
        };
        let init = post(json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {"protocolVersion": "2025-03-26",
                               "capabilities": {}, "clientInfo": {"name": "omuse", "version": "1"}}}))
            .await
            .map_err(|e| SearchError(format!("Exa MCP: {e}")))?;
        match init.status().as_u16() {
            429 => {
                return Err(SearchError(
                    "Exa MCP: rate limit of the free tier reached".into(),
                ))
            }
            s if s >= 400 => return Err(SearchError(format!("Exa MCP HTTP {s}"))),
            _ => {}
        }
        let r = post(json!({"jsonrpc": "2.0", "id": 2, "method": "tools/call",
                            "params": {"name": "web_search_exa", "arguments": {"query": q, "numResults": n}}}))
            .await
            .map_err(|e| SearchError(format!("Exa MCP: {e}")))?;
        let status = r.status().as_u16();
        let text = r
            .text()
            .await
            .map_err(|e| SearchError(format!("Exa MCP: {e}")))?;
        if status == 429 {
            return Err(SearchError(
                "Exa MCP: rate limit of the free tier reached".into(),
            ));
        }
        if status >= 400 {
            return Err(SearchError(format!(
                "Exa MCP HTTP {status}: {}",
                text.chars().take(160).collect::<String>()
            )));
        }
        let msg =
            mcp_payload(&text).ok_or_else(|| SearchError("Exa MCP: unexpected reply".into()))?;
        if let Some(e) = msg.get("error") {
            return Err(SearchError(format!("Exa MCP: {}", get_str(e, "message"))));
        }
        let content = msg["result"]["content"]
            .as_array()
            .map(|a| {
                a.iter()
                    .map(|c| get_str(c, "text"))
                    .collect::<Vec<_>>()
                    .join("\n")
            })
            .unwrap_or_default();
        if msg["result"]["isError"].as_bool().unwrap_or(false) {
            return Err(SearchError(format!(
                "Exa MCP: {}",
                content.chars().take(160).collect::<String>()
            )));
        }
        Ok(
            json!({"query": q, "engine": self.provider.engine(), "results": parse_exa_mcp_text(&content, n)}),
        )
    }

    fn request(&self, q: &str, n: usize) -> reqwest::RequestBuilder {
        let req = match self.provider {
            Provider::Brave => self
                .client
                .get(format!("{}/web/search", self.base))
                .query(&[("q", q), ("count", &n.to_string())])
                .header("X-Subscription-Token", &self.key)
                .header("Accept", "application/json"),
            Provider::Tavily => self
                .client
                .post(format!("{}/search", self.base))
                .bearer_auth(&self.key)
                .json(&json!({"query": q, "max_results": n, "search_depth": "basic"})),
            Provider::Exa => self
                .client
                .post(format!("{}/search", self.base))
                .header("x-api-key", &self.key)
                .json(&json!({"query": q, "numResults": n, "type": "auto",
                              "contents": {"highlights": {"maxCharacters": 400, "highlightsPerUrl": 1}}})),
            Provider::ExaMcp => self.client.post(&self.base), // served by search_exa_mcp
        };
        req.timeout(self.timeout)
    }

    /// The key check when it is saved: one small search.
    pub async fn test(&self) -> Result<Value, SearchError> {
        let r = self.search("OMuse personal agent", 3).await?;
        Ok(
            json!({"ok": true, "provider": self.provider.id(), "engine": self.provider.engine(),
                  "count": r["results"].as_array().map(Vec::len).unwrap_or(0)}),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn providers_parse_and_results_compact() {
        assert_eq!(Provider::parse("Brave"), Some(Provider::Brave));
        assert_eq!(Provider::parse("google"), None);
        assert_eq!(Provider::parse("bing"), None);
        let brave = json!({"web": {"results": [
            {"title": "A", "url": "https://a.example/", "description": "first", "age": "2 days ago", "extra_snippets": ["more"]},
            {"title": "B", "url": "ftp://b", "description": "skipped"},
            {"title": "C", "url": "https://c.example/", "description": "third"}]}});
        let rows = compact(Provider::Brave, &brave, 10);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0]["snippet"], "first more");
        assert_eq!(rows[0]["age"], "2 days ago");
        let tavily = json!({"results": [{"title": "T", "url": "https://t.example/", "content": "tav", "published_date": null}]});
        let rows = compact(Provider::Tavily, &tavily, 10);
        assert_eq!(rows[0]["snippet"], "tav");
        assert!(rows[0].get("age").is_none());
        let exa = json!({"results": [{"title": "E", "url": "https://e.example/", "publishedDate": "2026-10-01", "highlights": ["one ", "two"]},
                                     {"title": "F", "url": "https://f.example/", "summary": "sum"}]});
        let rows = compact(Provider::Exa, &exa, 10);
        assert_eq!(rows[0]["snippet"], "one two");
        assert_eq!(rows[0]["age"], "2026-10-01");
        assert_eq!(rows[1]["snippet"], "sum");
        assert_eq!(Provider::parse("Exa"), Some(Provider::Exa));
        // the API order after the browser: Brave, the free Exa MCP, Exa — the last one that worked comes first; Tavily is
        // never in it (it answers news_search)
        let all = Provider::ALL;
        assert_eq!(
            order(None, &all),
            vec![Provider::Brave, Provider::ExaMcp, Provider::Exa]
        );
        assert_eq!(
            order(Some(Provider::Exa), &all),
            vec![Provider::Exa, Provider::Brave, Provider::ExaMcp]
        );
        assert_eq!(
            order(Some(Provider::Tavily), &all),
            vec![Provider::Brave, Provider::ExaMcp, Provider::Exa]
        );
        assert_eq!(
            order(Some(Provider::Exa), &[Provider::Brave, Provider::Tavily]),
            vec![Provider::Brave]
        );
        assert!(order(None, &[]).is_empty());
        let txt = "Title: Second State\nURL: https://github.com/second-state\nPublished: N/A\nAuthor: N/A\nHighlights:\n# Second State\n\nFast, safe, portable.\n\nTitle: Other\nURL: https://o.example/\nPublished: 2026-01-02\nHighlights:\nsome text\n";
        let rows = parse_exa_mcp_text(txt, 10);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0]["url"], "https://github.com/second-state");
        assert_eq!(rows[0]["snippet"], "Second State Fast, safe, portable.");
        assert!(rows[0].get("age").is_none());
        assert_eq!(rows[1]["age"], "2026-01-02");
        assert!(!Provider::ExaMcp.needs_key() && Provider::Exa.needs_key());
        assert_eq!(
            mcp_payload("event: message\ndata: {\"a\": 1}\n\n").unwrap()["a"],
            1
        );
        assert_eq!(mcp_payload("{\"b\": 2}").unwrap()["b"], 2);
    }
}
