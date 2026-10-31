//! Fast search / read without snapshot round trips (`web_search`, `web_read`): short-lived pages of the
//! persistent browser.

use std::time::Duration;

use omuse_common::api::ApiError;
use omuse_common::guard::check_url;
use omuse_common::util::{get_str, truthy};
use serde_json::{json, Value};

use crate::actions::BLOCK_JS;
use crate::broker::Broker;
use crate::textfmt::cut;

const SEARCH_JS: &str = r#"() => {
  const out = [];
  const dec = (h) => { try { const u = new URL(h, location.href); const g = u.searchParams.get('uddg');
                             return g ? decodeURIComponent(g) : u.href; } catch (e) { return h; } };
  for (const r of document.querySelectorAll('.result, .web-result')) {
    const a = r.querySelector('a.result__a'); if (!a) continue;
    const sn = r.querySelector('.result__snippet');
    out.push({title: a.innerText.trim(), url: dec(a.getAttribute('href')), snippet: sn ? sn.innerText.trim() : ''});
  }
  if (!out.length) for (const r of document.querySelectorAll('div.news-card')) {   // Bing News
    const a = r.querySelector('a.title'); if (!a) continue;
    const sn = r.querySelector('.snippet'); const age = r.querySelector('span[aria-label]');
    out.push({title: (r.getAttribute('data-title') || a.innerText).trim(), url: r.getAttribute('url') || a.href,
              snippet: sn ? sn.innerText.trim() : '', source: r.getAttribute('data-author') || '',
              age: age ? age.getAttribute('aria-label') : ''});
  }
  if (!out.length) for (const r of document.querySelectorAll('li.b_algo')) {   // Bing
    const a = r.querySelector('h2 a'); if (!a) continue;
    let url = a.href; const m = url.match(/[?&]u=a1([^&]+)/);
    if (m) { try { url = atob(m[1].replace(/-/g, '+').replace(/_/g, '/')); } catch (e) {} }
    const sn = r.querySelector('.b_caption p, p');
    out.push({title: a.innerText.trim(), url, snippet: sn ? sn.innerText.trim() : ''});
  }
  return out;
}"#;

const READ_JS: &str = r#"(limit) => {
  const pick = document.querySelector('article') || document.querySelector('main, [role=main]') || document.body;
  let t = (pick ? pick.innerText : '') || '';
  if (t.length < 400 && document.body) t = document.body.innerText || t;
  t = t.replace(/[ \t]+\n/g, '\n').replace(/\n{3,}/g, '\n\n').trim();
  return {title: document.title, url: location.href, text: t.slice(0, limit), length: t.length,
          scripts: document.scripts.length};
}"#;

/// `urllib.parse.quote_plus`.
pub fn quote_plus(s: &str) -> String {
    url::form_urlencoded::byte_serialize(s.as_bytes()).collect()
}

impl Broker {
    /// The search engines: DuckDuckGo html and Bing, or `[browser] search_engines = "name=url-prefix,…"` (tests).
    pub fn search_engines(&self, query: &str) -> Vec<(String, String)> {
        let q = quote_plus(query);
        let cfg = self.cfg.browser.search_engines.trim();
        if !cfg.is_empty() {
            return cfg
                .split(',')
                .filter(|e| e.contains('='))
                .map(|e| {
                    let (n, u) = e.split_once('=').unwrap();
                    (n.to_string(), format!("{}{}", u, q))
                })
                .collect();
        }
        vec![
            (
                "duckduckgo".into(),
                format!("https://html.duckduckgo.com/html/?q={}", q),
            ),
            (
                "bing".into(),
                format!("https://www.bing.com/search?setlang=en&q={}", q),
            ),
        ]
    }

    /// The engines' news searches for the last `days` days, in the browser (no search API): DuckDuckGo html with its
    /// date filter (`df` = d / w / m / y) and Bing News with its interval; the test engines with `&df=…` added.
    pub fn news_engines(&self, query: &str, days: i64) -> Vec<(String, String)> {
        let df = match days {
            ..=1 => "d",
            2..=7 => "w",
            8..=31 => "m",
            _ => "y",
        };
        let cfg = self.cfg.browser.search_engines.trim();
        if !cfg.is_empty() {
            return self
                .search_engines(query)
                .into_iter()
                .map(|(n, u)| (n, format!("{u}&df={df}")))
                .collect();
        }
        let q = quote_plus(query);
        let mut out = vec![(
            "duckduckgo".to_string(),
            format!("https://html.duckduckgo.com/html/?q={q}&df={df}"),
        )];
        let interval = match days {
            ..=1 => "&qft=interval%3d%227%22",
            2..=7 => "&qft=interval%3d%228%22",
            8..=31 => "&qft=interval%3d%229%22",
            _ => "",
        };
        out.push((
            "bing news".to_string(),
            format!("https://www.bing.com/news/search?setlang=en&q={q}{interval}"),
        ));
        out
    }

    /// `web_search(query, n)`.
    pub async fn web_search(&self, query: &str, n: i64) -> Result<Value, ApiError> {
        self.search_on(self.search_engines(query), query, n).await
    }

    /// `news_search(query, n, days)`: the news of the last `days` days from the engines' own pages.
    pub async fn news_search(&self, query: &str, n: i64, days: i64) -> Result<Value, ApiError> {
        let mut r = self
            .search_on(self.news_engines(query, days), query, n)
            .await?;
        r["days"] = json!(days);
        Ok(r)
    }

    async fn search_on(
        &self,
        engines: Vec<(String, String)>,
        query: &str,
        n: i64,
    ) -> Result<Value, ApiError> {
        let mut last = String::new();
        for (name, url) in engines {
            let p = self.temp_page().await?;
            let res: Result<Value, String> = async {
                p.navigate(&url, Duration::from_secs(25))
                    .await
                    .map_err(|e| e.to_string())?;
                tokio::time::sleep(Duration::from_millis(800)).await;
                p.eval(None, SEARCH_JS, vec![])
                    .await
                    .map_err(|e| crate::pages::err_text(&e))
            }
            .await;
            self.close_temp(&p).await;
            let mut rows: Vec<Value> = match res {
                Ok(Value::Array(a)) => a,
                Ok(_) => vec![],
                Err(e) => {
                    last = format!("{}: {}", name, cut(&e, 120));
                    vec![]
                }
            };
            rows.retain(|r| {
                let u = get_str(r, "url");
                u.starts_with("http") && check_url(&u).0
            });
            if !rows.is_empty() {
                let keep = n.clamp(1, 10) as usize;
                rows.truncate(keep);
                return Ok(json!({"query": query, "engine": name, "results": rows}));
            }
            if last.is_empty() {
                last = format!("{}: no results", name);
            }
        }
        Ok(json!({"query": query, "engine": "", "results": [], "error": last}))
    }

    /// `web_read(urls, limit, wait)`: up to four pages, read in parallel, each `wait` after it loads (scripts that draw
    /// the page, e.g. Google Maps, need longer than the default).
    pub async fn web_read(
        &self,
        urls: &[String],
        limit: i64,
        wait: Duration,
    ) -> Result<Value, ApiError> {
        let futs = urls
            .iter()
            .take(4)
            .map(|u| self.read_one(u.clone(), limit, wait));
        let pages = futures::future::join_all(futs).await;
        Ok(json!({"pages": pages}))
    }

    async fn read_one(&self, u: String, limit: i64, wait: Duration) -> Value {
        let (ok, why) = check_url(&u);
        if !ok {
            return json!({"url": u, "error": why});
        }
        let p = match self.temp_page().await {
            Ok(p) => p,
            Err(e) => {
                return json!({"url": u, "error": format!("打开失败 could not open: {}", cut(&e.detail_text(), 160))})
            }
        };
        let out: Result<Value, String> = async {
            self.pace(&u).await;
            let status = p
                .navigate(&u, Duration::from_secs(25))
                .await
                .map_err(|e| e.to_string())?;
            tokio::time::sleep(wait).await;
            let mut r = p
                .eval(None, READ_JS, vec![json!(limit)])
                .await
                .map_err(|e| crate::pages::err_text(&e))?;
            let blk = p
                .eval(None, BLOCK_JS, vec![json!(status)])
                .await
                .ok()
                .filter(truthy);
            if let Some(o) = r.as_object_mut() {
                if let Some(b) = blk {
                    o.insert("blocked".into(), b);
                    self.note_block(&u);
                }
                o.insert("status".into(), json!(status));
            }
            Ok(r)
        }
        .await;
        self.close_temp(&p).await;
        match out {
            Ok(v) => v,
            Err(msg) => {
                if msg.contains("ERR_BLOCKED_BY_CLIENT") {
                    json!({"url": u, "error": "该地址被安全策略拦截 (blocked by egress policy)"})
                } else {
                    json!({"url": u, "error": format!("打开失败 could not open: {}", cut(&msg, 160))})
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quote_plus_like_python() {
        assert_eq!(quote_plus("olares one"), "olares+one");
        assert_eq!(quote_plus("a&b=c"), "a%26b%3Dc");
    }
}

#[cfg(test)]
mod news_tests {
    #[test]
    fn days_pick_the_engines_date_filters() {
        let b = super::Broker::new(std::sync::Arc::new(
            omuse_common::Config::parse("").unwrap(),
        ));
        let urls = |d| -> Vec<String> {
            b.news_engines("solid state battery", d)
                .into_iter()
                .map(|(_, u)| u)
                .collect()
        };
        let week = urls(7);
        assert_eq!(
            week[0],
            "https://html.duckduckgo.com/html/?q=solid+state+battery&df=w"
        );
        assert_eq!(week[1], "https://www.bing.com/news/search?setlang=en&q=solid+state+battery&qft=interval%3d%228%22");
        assert!(urls(1)[0].ends_with("&df=d"));
        assert!(urls(30)[0].ends_with("&df=m"));
        assert!(urls(200)[1].ends_with("q=solid+state+battery"));
    }
}
