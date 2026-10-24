//! Market data (quotes and price history) from Yahoo Finance's public chart API — no key, no browser.
//!
//! Covers stocks (AAPL, 0700.HK, 9988.HK, 600519.SS, 7203.T), indices (^GSPC, ^IXIC, ^DJI, ^HSI, ^N225, ^STI),
//! FX (HKD=X = USD/HKD, CNY=X = USD/CNY, SGD=X, JPY=X, EURUSD=X), commodities (GC=F gold, CL=F oil) and crypto
//! (BTC-USD, ETH-USD). Names ("tencent", "nvidia") are looked up with Yahoo's search.

use std::collections::BTreeMap;
use std::sync::LazyLock;
use std::time::Duration;

use omuse_common::pyjson::format_f64;
use omuse_common::util::{as_f64, get_str, now_ts, py_str, take_chars, truthy};
use parking_lot::Mutex;
use regex::Regex;
use serde_json::{json, Map, Value};

use crate::py::{httpx_name, pyg, pyround};

/// A full browser UA string gets 429 from this API; the short one is served.
pub const UA: &str = "Mozilla/5.0";

/// Allowed ranges and their bar interval (in this order).
pub const RANGES: &[(&str, &str)] = &[
    ("5d", "1d"),
    ("1mo", "1d"),
    ("3mo", "1d"),
    ("6mo", "1wk"),
    ("ytd", "1wk"),
    ("1y", "1wk"),
    ("2y", "1mo"),
    ("3y", "1mo"),
    ("5y", "1mo"),
    ("10y", "3mo"),
    ("max", "3mo"),
];

/// Range spellings the model uses.
pub const ALIASES: &[(&str, &str)] = &[
    ("1m", "1mo"),
    ("3m", "3mo"),
    ("6m", "6mo"),
    ("12m", "1y"),
    ("1yr", "1y"),
    ("year", "1y"),
    ("2yr", "2y"),
    ("3yr", "3y"),
    ("5yr", "5y"),
    ("10yr", "10y"),
    ("1w", "5d"),
    ("week", "5d"),
    ("month", "1mo"),
];

/// Common names the model may pass instead of tickers.
pub const NAMES: &[(&str, &str)] = &[
    ("海峡时报指数", "^STI"),
    ("新加坡海峡时报指数", "^STI"),
    ("STI", "^STI"),
    ("星展", "D05.SI"),
    ("星展银行", "D05.SI"),
    ("DBS", "D05.SI"),
    ("华侨银行", "O39.SI"),
    ("OCBC", "O39.SI"),
    ("大华银行", "U11.SI"),
    ("UOB", "U11.SI"),
    ("新电信", "Z74.SI"),
    ("Singtel", "Z74.SI"),
    ("CapitaLand Integrated Commercial Trust", "C38U.SI"),
    ("CICT", "C38U.SI"),
    ("凯德综合商业信托", "C38U.SI"),
    ("Mapletree Pan Asia Commercial Trust", "N2IU.SI"),
    ("MPACT", "N2IU.SI"),
    ("丰树泛亚商业信托", "N2IU.SI"),
    ("Frasers Centrepoint Trust", "J69U.SI"),
    ("FCT", "J69U.SI"),
    ("CapitaLand Ascendas REIT", "A17U.SI"),
    ("Ascendas REIT", "A17U.SI"),
    ("Mapletree Logistics Trust", "M44U.SI"),
    ("Mapletree Industrial Trust", "ME8U.SI"),
    ("Keppel REIT", "K71U.SI"),
    ("Suntec REIT", "T82U.SI"),
    ("Sea", "SE"),
    ("Grab", "GRAB"),
    ("腾讯", "0700.HK"),
    ("汇丰", "0005.HK"),
    ("渣打", "2888.HK"),
    ("小米", "1810.HK"),
    ("比亚迪", "1211.HK"),
    ("阿里巴巴", "9988.HK"),
    ("美团", "3690.HK"),
    ("宁德时代", "300750.SZ"),
    ("茅台", "600519.SS"),
    ("贵州茅台", "600519.SS"),
    ("恒生指数", "^HSI"),
    ("恒指", "^HSI"),
    ("标普500", "^GSPC"),
    ("标普", "^GSPC"),
    ("纳指", "^IXIC"),
    ("纳斯达克", "^IXIC"),
    ("道指", "^DJI"),
    ("日经", "^N225"),
    ("黄金", "GC=F"),
    ("比特币", "BTC-USD"),
    ("英伟达", "NVDA"),
    ("苹果", "AAPL"),
    ("微软", "MSFT"),
    ("特斯拉", "TSLA"),
    ("谷歌", "GOOGL"),
    ("亚马逊", "AMZN"),
    ("港币", "HKD=X"),
    ("港元", "HKD=X"),
    ("美元兑港币", "HKD=X"),
    ("美元兑港元", "HKD=X"),
    ("USD/HKD", "HKD=X"),
    ("人民币", "CNY=X"),
    ("美元兑人民币", "CNY=X"),
    ("USD/CNY", "CNY=X"),
    ("新元", "SGD=X"),
    ("新加坡元", "SGD=X"),
    ("美元兑新元", "SGD=X"),
    ("USD/SGD", "SGD=X"),
    ("日元", "JPY=X"),
    ("美元兑日元", "JPY=X"),
    ("USD/JPY", "JPY=X"),
    ("欧元", "EURUSD=X"),
    ("EUR/USD", "EURUSD=X"),
    ("原油", "CL=F"),
    ("海天国际", "1882.HK"),
    ("中国移动", "0941.HK"),
    ("建设银行", "0939.HK"),
    ("工商银行", "1398.HK"),
    ("友邦", "1299.HK"),
    ("港交所", "0388.HK"),
    ("京东", "9618.HK"),
    ("百度", "9888.HK"),
    ("网易", "9999.HK"),
    ("快手", "1024.HK"),
    ("理想汽车", "2015.HK"),
    ("蔚来", "NIO"),
    ("小鹏", "9868.HK"),
    ("台积电", "TSM"),
    ("Meta", "META"),
    ("奈飞", "NFLX"),
    ("伯克希尔", "BRK-B"),
    ("新加坡交易所", "S68.SI"),
];

/// Ticker for a well-known name (`NAMES[name]`).
pub fn name_to_symbol(name: &str) -> Option<&'static str> {
    NAMES.iter().find(|(k, _)| *k == name).map(|(_, v)| *v)
}

/// Python `MarketError`.
#[derive(Debug, Clone, thiserror::Error)]
#[error("{0}")]
pub struct MarketError(pub String);

/// Internal: a `MarketError` or another exception the Python caught by class name.
#[derive(Debug)]
enum Fail {
    Market(MarketError),
    Other(&'static str),
}

impl From<MarketError> for Fail {
    fn from(e: MarketError) -> Self {
        Fail::Market(e)
    }
}

impl From<reqwest::Error> for Fail {
    fn from(e: reqwest::Error) -> Self {
        if e.is_decode() {
            Fail::Other("JSONDecodeError")
        } else {
            Fail::Other(httpx_name(&e))
        }
    }
}

fn arr(v: Option<&Value>) -> &[Value] {
    v.and_then(Value::as_array)
        .map(|a| a.as_slice())
        .unwrap_or(&[])
}

/// Normalize a range (`"1y"`, aliases) or fail with the bilingual list.
pub fn norm_range(r: &str) -> Result<&'static str, MarketError> {
    let r = if r.trim().is_empty() {
        "1y".to_string()
    } else {
        r.trim().to_lowercase()
    };
    let r = ALIASES
        .iter()
        .find(|(k, _)| *k == r)
        .map(|(_, v)| v.to_string())
        .unwrap_or(r);
    match RANGES.iter().find(|(k, _)| *k == r) {
        Some((k, _)) => Ok(k),
        None => {
            let all = RANGES
                .iter()
                .map(|(k, _)| *k)
                .collect::<Vec<_>>()
                .join(", ");
            Err(MarketError(format!(
                "range 应为 {} 之一 (range must be one of {})",
                all, all
            )))
        }
    }
}

fn interval(rng: &str) -> &'static str {
    RANGES
        .iter()
        .find(|(k, _)| *k == rng)
        .map(|(_, v)| *v)
        .unwrap_or("1wk")
}

/// Round prices for display: 4 decimals below 20 (FX, small caps), else 2.
pub fn round_price(v: f64) -> f64 {
    if v.abs() < 20.0 {
        pyround(v, 4)
    } else {
        pyround(v, 2)
    }
}

fn day(ts: i64, gmtoff: i64) -> String {
    chrono::DateTime::from_timestamp(ts + gmtoff, 0)
        .map(|d| d.format("%Y-%m-%d").to_string())
        .unwrap_or_default()
}

/// If `s` needs no search: the ticker (`NAMES`, a ticker-looking string, `700` → `0700.HK`), else `None`.
pub fn resolve_local(s: &str) -> Option<String> {
    static TICKER: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"^[\^A-Za-z0-9.\-=]{1,15}$").unwrap());
    static WORD: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[A-Za-z]{6,}$").unwrap());
    static HK: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^\d{1,4}$").unwrap());
    if let Some(t) = name_to_symbol(s) {
        return Some(t.to_string());
    }
    if TICKER.is_match(s) && !WORD.is_match(s) {
        if HK.is_match(s) {
            return Some(format!("{:0>4}.HK", s));
        }
        return Some(if s.starts_with('^') {
            s.to_string()
        } else {
            s.to_uppercase()
        });
    }
    None
}

#[derive(Default)]
struct Crumb {
    value: Option<String>,
    cookies: String,
    at: f64,
}

static CRUMB: LazyLock<Mutex<Crumb>> = LazyLock::new(|| Mutex::new(Crumb::default()));

/// Reported-financials fields.
pub const TS_FIELDS: &[&str] = &[
    "TotalRevenue",
    "GrossProfit",
    "OperatingIncome",
    "NetIncome",
    "DilutedEPS",
];

/// The Yahoo Finance client (hosts from `cfg.market_data_hosts()`).
pub struct Market {
    client: reqwest::Client,
    hosts: Vec<String>,
}

impl Market {
    pub fn new(client: reqwest::Client, hosts: Vec<String>) -> Self {
        Self { client, hosts }
    }

    /// GET from Yahoo's API, trying the next host when one is rate-limited or answers with a non-JSON page.
    async fn get_json(&self, path: &str, params: &[(&str, String)]) -> Result<Value, Fail> {
        let mut last: Option<u16> = None;
        for host in &self.hosts {
            let r = self
                .client
                .get(format!("{}{}", host, path))
                .query(params)
                .header("User-Agent", UA)
                .timeout(Duration::from_secs(20))
                .send()
                .await?;
            let status = r.status().as_u16();
            if status == 404 {
                return Ok(json!({"_status": 404}));
            }
            let ct = r
                .headers()
                .get("content-type")
                .and_then(|v| v.to_str().ok())
                .unwrap_or("")
                .to_string();
            if status == 200 && ct.contains("json") {
                let bytes = r.bytes().await?;
                return serde_json::from_slice(&bytes).map_err(|_| Fail::Other("JSONDecodeError"));
            }
            last = Some(status);
        }
        let last = last.map(|s| s.to_string()).unwrap_or_else(|| "None".into());
        Err(MarketError(format!("数据源暂时不可用 data source busy (HTTP {}) — try again shortly or use a finance website", last)).into())
    }

    async fn resolve_inner(&self, sym: &str) -> Result<String, Fail> {
        let s = sym.trim();
        if s.is_empty() {
            return Err(MarketError("空的代码 empty symbol".into()).into());
        }
        if let Some(t) = resolve_local(s) {
            return Ok(t);
        }
        let d = self
            .get_json(
                "/v1/finance/search",
                &[
                    ("q", s.to_string()),
                    ("quotesCount", "5".into()),
                    ("newsCount", "0".into()),
                ],
            )
            .await?;
        let first = arr(d.get("quotes"))
            .iter()
            .find(|q| q.get("symbol").is_some_and(truthy))
            .map(|q| get_str(q, "symbol"));
        first.ok_or_else(|| {
            MarketError(format!("找不到「{}」的代码 no ticker found for {} — give a ticker like AAPL, 0700.HK, ^HSI", s, omuse_common::pyjson::py_repr(&json!(s)))).into()
        })
    }

    /// Name or ticker → Yahoo symbol.
    pub async fn resolve(&self, sym: &str) -> Result<String, MarketError> {
        self.resolve_inner(sym).await.map_err(|e| match e {
            Fail::Market(m) => m,
            Fail::Other(n) => MarketError(format!("数据源暂时不可用 data source error ({})", n)),
        })
    }

    async fn history_inner(&self, sym: &str, rng: &str) -> Result<Value, Fail> {
        let rng = norm_range(rng)?;
        let iv = interval(rng);
        let j = self
            .get_json(
                &format!("/v8/finance/chart/{}", sym),
                &[
                    ("range", rng.into()),
                    ("interval", iv.into()),
                    ("includePrePost", "false".into()),
                ],
            )
            .await?;
        if j.get("_status") == Some(&json!(404)) {
            return Err(MarketError(format!("代码不存在 unknown symbol: {}", sym)).into());
        }
        let d = j
            .get("chart")
            .filter(|c| truthy(c))
            .cloned()
            .unwrap_or(json!({}));
        if d.get("error").is_some_and(truthy) || !d.get("result").is_some_and(truthy) {
            let desc = d
                .get("error")
                .filter(|e| truthy(e))
                .map(|e| get_str(e, "description"))
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| "no data".into());
            return Err(MarketError(format!("{}: {}", sym, desc)).into());
        }
        let res = &d["result"][0];
        let meta = res
            .get("meta")
            .filter(|m| truthy(m))
            .cloned()
            .unwrap_or(json!({}));
        let off = meta
            .get("gmtoffset")
            .and_then(omuse_common::util::as_i64)
            .unwrap_or(0);
        let ind = res
            .get("indicators")
            .filter(|i| truthy(i))
            .cloned()
            .unwrap_or(json!({}));
        let q = arr(ind.get("quote")).first().cloned().unwrap_or(json!({}));
        let adj = arr(ind.get("adjclose"))
            .first()
            .and_then(|a| a.get("adjclose"))
            .cloned()
            .unwrap_or(Value::Null);
        let closes = arr(q.get("close"));
        let highs = arr(q.get("high"));
        let lows = arr(q.get("low"));
        let mut pts: Vec<(String, f64, f64, f64)> = Vec::new();
        for (i, ts) in arr(res.get("timestamp")).iter().enumerate() {
            let Some(c) = closes.get(i).and_then(as_f64) else {
                continue;
            };
            let pick = |list: &[Value]| -> f64 {
                if list.is_empty() {
                    return c;
                }
                list.get(i)
                    .and_then(as_f64)
                    .filter(|v| *v != 0.0)
                    .unwrap_or(c)
            };
            let ts = ts
                .as_i64()
                .or_else(|| ts.as_f64().map(|f| f as i64))
                .unwrap_or(0);
            pts.push((
                day(ts, off),
                round_price(c),
                round_price(pick(highs)),
                round_price(pick(lows)),
            ));
        }
        if pts.is_empty() {
            return Err(
                MarketError(format!("{}: 这个区间没有数据 no prices in this range", sym)).into(),
            );
        }
        let last = meta
            .get("regularMarketPrice")
            .filter(|v| truthy(v))
            .and_then(as_f64)
            .unwrap_or(pts[pts.len() - 1].1);
        let prev = if rng == "5d" || rng == "1mo" {
            meta.get("chartPreviousClose")
                .cloned()
                .unwrap_or(Value::Null)
        } else {
            Value::Null
        };
        let mut hi = &pts[0];
        let mut lo = &pts[0];
        for p in &pts {
            if p.2 > hi.2 {
                hi = p;
            }
            if p.3 < lo.3 {
                lo = p;
            }
        }
        let first = pts[0].1;
        let first_str =
            |k: &str| -> Option<String> { meta.get(k).filter(|v| truthy(v)).map(py_str) };
        let last_time = match meta.get("regularMarketTime").filter(|v| truthy(v)) {
            Some(t) => day(omuse_common::util::as_i64(t).unwrap_or(0), off),
            None => pts[pts.len() - 1].0.clone(),
        };
        let day_change = meta
            .get("previousClose")
            .filter(|v| truthy(v))
            .and_then(as_f64)
            .map(|pc| json!(pyround((last / pc - 1.0) * 100.0, 2)))
            .unwrap_or(Value::Null);
        Ok(json!({
            "symbol": first_str("symbol").unwrap_or_else(|| sym.to_string()),
            "name": first_str("longName").or_else(|| first_str("shortName")).unwrap_or_else(|| sym.to_string()),
            "exchange": first_str("fullExchangeName").or_else(|| first_str("exchangeName")).unwrap_or_default(),
            "currency": first_str("currency").unwrap_or_default(),
            "range": rng, "interval": iv, "last": round_price(last), "last_time": last_time,
            "day_change_pct": day_change,
            "start": {"date": pts[0].0, "close": first},
            "change_pct": if first != 0.0 { json!(pyround((last / first - 1.0) * 100.0, 2)) } else { Value::Null },
            "high": {"date": hi.0, "price": hi.2}, "low": {"date": lo.0, "price": lo.3},
            "week52": [meta.get("fiftyTwoWeekLow").cloned().unwrap_or(Value::Null), meta.get("fiftyTwoWeekHigh").cloned().unwrap_or(Value::Null)],
            "prev_close": prev,
            "series": pts.iter().map(|p| json!([p.0, p.1])).collect::<Vec<_>>(),
            "adjusted": truthy(&adj),
        }))
    }

    /// Price history of one resolved symbol.
    pub async fn history(&self, sym: &str, rng: &str) -> Result<Value, MarketError> {
        self.history_inner(sym, rng).await.map_err(|e| match e {
            Fail::Market(m) => m,
            Fail::Other(n) => MarketError(format!("数据源暂时不可用 data source error ({})", n)),
        })
    }

    /// Quotes + history for up to 8 symbols: `{results, errors, source}`.
    pub async fn fetch(&self, symbols: &[String], rng: &str) -> Value {
        let mut out = Vec::new();
        let mut errors = Vec::new();
        for s in symbols.iter().take(8) {
            let r = async {
                let sym = self.resolve_inner(s).await?;
                self.history_inner(&sym, rng).await
            }
            .await;
            match r {
                Ok(v) => out.push(v),
                Err(Fail::Market(e)) => errors.push(format!("{}: {}", s, e)),
                Err(Fail::Other(n)) => {
                    errors.push(format!("{}: 数据源暂时不可用 data source error ({})", s, n))
                }
            }
        }
        json!({"results": out, "errors": errors, "source": "Yahoo Finance"})
    }

    // ---------------------------------------------------------------- valuations and reported financials
    // Quote fields (P/E, market cap, dividend yield) need Yahoo's cookie + "crumb"; the reported-financials timeseries does not.

    async fn quote_crumb(&self) -> (Option<String>, String) {
        {
            let c = CRUMB.lock();
            if c.value.is_some() && now_ts() - c.at < 3600.0 {
                return (c.value.clone(), c.cookies.clone());
            }
        }
        let mut cookies: Vec<String> = Vec::new();
        if self
            .hosts
            .first()
            .is_some_and(|h| h.starts_with("https://query"))
        {
            // real Yahoo: the session cookie comes from fc.yahoo.com (answers 404 but sets it)
            if let Ok(r) = self
                .client
                .get("https://fc.yahoo.com")
                .header("User-Agent", UA)
                .timeout(Duration::from_secs(15))
                .send()
                .await
            {
                for v in r.headers().get_all("set-cookie") {
                    if let Some(kv) = v.to_str().ok().and_then(|s| s.split(';').next()) {
                        cookies.push(kv.trim().to_string());
                    }
                }
            }
        }
        let jar = cookies.join("; ");
        let mut crumb = None;
        for host in &self.hosts {
            let mut rb = self
                .client
                .get(format!("{}/v1/test/getcrumb", host))
                .header("User-Agent", UA)
                .timeout(Duration::from_secs(15));
            if !jar.is_empty() {
                rb = rb.header("Cookie", &jar);
            }
            let Ok(r) = rb.send().await else { continue };
            let status = r.status().as_u16();
            let Ok(text) = r.text().await else { continue };
            let t = text.trim();
            if status == 200 && !t.is_empty() && t.chars().count() < 40 && !text.contains('<') {
                crumb = Some(t.to_string());
                break;
            }
        }
        let mut c = CRUMB.lock();
        *c = Crumb {
            value: crumb.clone(),
            cookies: jar.clone(),
            at: now_ts(),
        };
        (crumb, jar)
    }

    async fn quotes(&self, syms: &[String]) -> Result<Map<String, Value>, Fail> {
        let (crumb, jar) = self.quote_crumb().await;
        let mut params = vec![("symbols", syms.join(","))];
        if let Some(c) = crumb {
            params.push(("crumb", c));
        }
        for host in &self.hosts {
            let mut rb = self
                .client
                .get(format!("{}/v7/finance/quote", host))
                .query(&params)
                .header("User-Agent", UA)
                .timeout(Duration::from_secs(20));
            if !jar.is_empty() {
                rb = rb.header("Cookie", &jar);
            }
            let r = rb.send().await?;
            let status = r.status().as_u16();
            let ct = r
                .headers()
                .get("content-type")
                .and_then(|v| v.to_str().ok())
                .unwrap_or("")
                .to_string();
            if status == 200 && ct.contains("json") {
                let bytes = r.bytes().await?;
                let j: Value =
                    serde_json::from_slice(&bytes).map_err(|_| Fail::Other("JSONDecodeError"))?;
                let res = j
                    .get("quoteResponse")
                    .filter(|x| truthy(x))
                    .map(|x| arr(x.get("result")).to_vec())
                    .unwrap_or_default();
                return Ok(res
                    .into_iter()
                    .filter(|q| q.get("symbol").is_some_and(truthy))
                    .map(|q| (get_str(&q, "symbol"), q))
                    .collect());
            }
            if status == 401 || status == 403 {
                CRUMB.lock().at = 0.0; // stale crumb: fetch a new one next time
            }
        }
        Ok(Map::new())
    }

    async fn financials(&self, sym: &str) -> Result<Value, Fail> {
        let types: Vec<String> = TS_FIELDS
            .iter()
            .map(|f| format!("quarterly{}", f))
            .chain(TS_FIELDS.iter().map(|f| format!("annual{}", f)))
            .collect();
        let now = now_ts() as i64;
        let d = self
            .get_json(
                &format!("/ws/fundamentals-timeseries/v1/finance/timeseries/{}", sym),
                &[
                    ("type", types.join(",")),
                    ("period1", (now - 6 * 365 * 86400).to_string()),
                    ("period2", (now + 86400).to_string()),
                ],
            )
            .await?;
        let mut out: BTreeMap<&str, BTreeMap<String, Map<String, Value>>> = BTreeMap::new();
        out.insert("quarterly", BTreeMap::new());
        out.insert("annual", BTreeMap::new());
        let results = d
            .get("timeseries")
            .filter(|t| truthy(t))
            .map(|t| arr(t.get("result")).to_vec())
            .unwrap_or_default();
        for r in &results {
            let t = r
                .get("meta")
                .and_then(|m| m.get("type"))
                .and_then(|t| t.as_array())
                .and_then(|a| a.first())
                .map(py_str)
                .unwrap_or_default();
            let kind = if t.starts_with("quarterly") {
                "quarterly"
            } else if t.starts_with("annual") {
                "annual"
            } else {
                continue;
            };
            let field = t[kind.len()..].to_string();
            for x in arr(r.get(&t)) {
                let raw = x
                    .get("reportedValue")
                    .and_then(|v| v.get("raw"))
                    .filter(|v| !v.is_null());
                if let (Some(date), Some(raw)) = (x.get("asOfDate").filter(|v| truthy(v)), raw) {
                    let row = out.get_mut(kind).unwrap().entry(py_str(date)).or_default();
                    row.insert(field.clone(), raw.clone());
                    let cur = x
                        .get("currencyCode")
                        .filter(|v| truthy(v))
                        .map(py_str)
                        .unwrap_or_default();
                    row.insert("currency".into(), json!(cur));
                }
            }
        }
        let mut v = Map::new();
        for kind in ["quarterly", "annual"] {
            // newest last, keep the last 4 periods
            let rows = &out[kind];
            let skip = rows.len().saturating_sub(4);
            let m: Map<String, Value> = rows
                .iter()
                .skip(skip)
                .map(|(k, r)| (k.clone(), Value::Object(r.clone())))
                .collect();
            v.insert(kind.into(), Value::Object(m));
        }
        Ok(Value::Object(v))
    }

    /// Valuations + reported financials for up to 6 symbols: `{results, errors, source}`.
    pub async fn fundamentals(&self, symbols: &[String]) -> Value {
        let mut out = Vec::new();
        let mut errors: Vec<String> = Vec::new();
        let mut syms: Vec<(String, String)> = Vec::new();
        for s in symbols.iter().take(6) {
            match self.resolve_inner(s).await {
                Ok(y) => syms.push((s.clone(), y)),
                Err(Fail::Market(e)) => errors.push(format!("{}: {}", s, e)),
                Err(Fail::Other(n)) => {
                    errors.push(format!("{}: 数据源暂时不可用 data source error ({})", s, n))
                }
            }
        }
        let quotes = if syms.is_empty() {
            Map::new()
        } else {
            match self
                .quotes(&syms.iter().map(|(_, y)| y.clone()).collect::<Vec<_>>())
                .await
            {
                Ok(q) => q,
                Err(e) => {
                    let n = match e {
                        Fail::Other(n) => n,
                        Fail::Market(_) => "MarketError",
                    };
                    errors.push(format!(
                        "估值数据暂时不可用 valuation data unavailable ({})",
                        n
                    ));
                    Map::new()
                }
            }
        };
        for (orig, sym) in &syms {
            let q = quotes.get(sym).cloned().unwrap_or(json!({}));
            let fin = if !sym.starts_with('^') && !sym.contains('=') {
                match self.financials(sym).await {
                    Ok(f) => f,
                    Err(e) => {
                        let msg = match e {
                            Fail::Market(m) => m.0,
                            Fail::Other(n) => n.to_string(),
                        };
                        errors.push(
                            take_chars(
                                &format!(
                                    "{}: 财报数据暂时不可用 financials unavailable ({})",
                                    orig, msg
                                ),
                                200,
                            )
                            .to_string(),
                        );
                        json!({})
                    }
                }
            } else {
                json!({})
            };
            let has_fin =
                fin.get("quarterly").is_some_and(truthy) || fin.get("annual").is_some_and(truthy);
            if !truthy(&q) && !has_fin {
                errors.push(format!("{}: 没有数据 no data for {}", orig, sym));
                continue;
            }
            let g = |k: &str| q.get(k).cloned().unwrap_or(Value::Null);
            let s_or = |a: &str, b: &str, d: &str| -> Value {
                q.get(a)
                    .filter(|v| truthy(v))
                    .or_else(|| q.get(b).filter(|v| truthy(v)))
                    .cloned()
                    .unwrap_or(json!(d))
            };
            let dy = match q.get("dividendYield").filter(|v| !v.is_null()) {
                Some(v) => v.clone(),
                None => match q
                    .get("trailingAnnualDividendYield")
                    .filter(|v| truthy(v))
                    .and_then(as_f64)
                {
                    Some(t) => json!(pyround(t * 100.0, 2)),
                    None => Value::Null,
                },
            };
            let earnings = q
                .get("earningsTimestamp")
                .filter(|v| truthy(v))
                .and_then(omuse_common::util::as_i64)
                .map(|t| json!(day(t, 0)))
                .unwrap_or(Value::Null);
            out.push(json!({
                "symbol": sym, "name": s_or("longName", "shortName", sym), "currency": s_or("currency", "", ""),
                "financial_currency": s_or("financialCurrency", "", ""), "price": g("regularMarketPrice"),
                "market_cap": g("marketCap"), "pe_ttm": g("trailingPE"), "pe_forward": g("forwardPE"),
                "eps_ttm": g("epsTrailingTwelveMonths"), "pb": g("priceToBook"),
                "dividend_yield_pct": dy,
                "week52": [g("fiftyTwoWeekLow"), g("fiftyTwoWeekHigh")],
                "earnings_date": earnings,
                "quarterly": fin.get("quarterly").filter(|v| truthy(v)).cloned().unwrap_or(json!({})),
                "annual": fin.get("annual").filter(|v| truthy(v)).cloned().unwrap_or(json!({})),
            }));
        }
        json!({"results": out, "errors": errors, "source": "Yahoo Finance"})
    }
}

/// Python `str(float)` of a JSON number (ints as ints), `None` for null.
fn num_str(v: &Value) -> String {
    match v {
        Value::Number(n) if n.is_f64() => format_f64(n.as_f64().unwrap_or(0.0)),
        other => py_str(other),
    }
}

fn f64_of(v: &Value) -> f64 {
    as_f64(v).unwrap_or(0.0)
}

/// Compact text for the agent: headline numbers plus the series as date close pairs.
pub fn describe(r: &Value) -> String {
    let cur = get_str(r, "currency");
    let mut lines: Vec<String> = Vec::new();
    lines.push(format!(
        "{} · {} · {} · {}",
        get_str(r, "symbol"),
        get_str(r, "name"),
        get_str(r, "exchange"),
        cur
    ));
    let dc = match r.get("day_change_pct").filter(|v| !v.is_null()) {
        Some(v) => format!("，较前收 {:+.2}%", f64_of(v)),
        None => String::new(),
    };
    lines.push(format!(
        "最新价 last {} {}（{}{}）",
        num_str(&r["last"]),
        cur,
        get_str(r, "last_time"),
        dc
    ));
    lines.push(match r.get("change_pct").filter(|v| !v.is_null()) {
        Some(v) => format!(
            "区间 {}：起 start {} ({}) → 现 now {}，涨跌 change {:+.2}%",
            get_str(r, "range"),
            num_str(&r["start"]["close"]),
            get_str(&r["start"], "date"),
            num_str(&r["last"]),
            f64_of(v)
        ),
        None => String::new(),
    });
    lines.push(format!(
        "区间最高 high {} ({})；最低 low {} ({})",
        num_str(&r["high"]["price"]),
        get_str(&r["high"], "date"),
        num_str(&r["low"]["price"]),
        get_str(&r["low"], "date")
    ));
    let w = arr(r.get("week52"));
    if !w.is_empty() && w.iter().all(truthy) {
        lines.push(format!(
            "52 周区间 52-week range: {} – {}",
            num_str(&w[0]),
            num_str(&w[1])
        ));
    }
    let step = match get_str(r, "interval").as_str() {
        "1d" => "日",
        "1wk" => "周",
        "1mo" => "月",
        "3mo" => "季",
        _ => "",
    };
    let series = arr(r.get("series"));
    lines.push(format!(
        "收盘价序列（{}线, {} 点）closes: {}",
        step,
        series.len(),
        series
            .iter()
            .map(|p| format!("{} {}", py_str(&p[0]), pyg(f64_of(&p[1]))))
            .collect::<Vec<_>>()
            .join(", ")
    ));
    lines
        .into_iter()
        .filter(|x| !x.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

fn big(v: &Value) -> String {
    let Some(v) = as_f64(v).filter(|_| !v.is_null()) else {
        return "—".into();
    };
    for (n, u) in [(1e12, "T"), (1e9, "B"), (1e6, "M")] {
        if v.abs() >= n {
            return format!("{:.2}{}", v / n, u);
        }
    }
    pyg(v)
}

fn f2(v: &Value) -> String {
    if v.is_null() {
        "—".into()
    } else {
        format!("{:.2}", f64_of(v))
    }
}

/// Text for the agent from one `fundamentals` result.
pub fn describe_fundamentals(r: &Value) -> String {
    let cur = get_str(r, "currency");
    let dy = if r["dividend_yield_pct"].is_null() {
        "—".to_string()
    } else {
        format!("{:.2}%", f64_of(&r["dividend_yield_pct"]))
    };
    let mut lines = vec![
        format!("{} · {} · 股价 price {} {}", get_str(r, "symbol"), get_str(r, "name"), num_str(&r["price"]), cur),
        format!(
            "市值 market cap {} {}；市盈率 P/E (TTM) {}；预期市盈率 forward P/E {}；市净率 P/B {}；每股收益 EPS (TTM) {}；股息率 dividend yield {}",
            big(&r["market_cap"]),
            cur,
            f2(&r["pe_ttm"]),
            f2(&r["pe_forward"]),
            f2(&r["pb"]),
            f2(&r["eps_ttm"]),
            dy
        ),
    ];
    if r.get("earnings_date").is_some_and(truthy) {
        lines.push(format!(
            "最近/下次财报日 earnings date {}",
            get_str(r, "earnings_date")
        ));
    }
    for (kind, label) in [
        ("quarterly", "季度财报 quarterly"),
        ("annual", "年度财报 annual"),
    ] {
        let Some(rows) = r
            .get(kind)
            .and_then(Value::as_object)
            .filter(|m| !m.is_empty())
        else {
            continue;
        };
        let mut fc = rows
            .values()
            .next()
            .map(|x| get_str(x, "currency"))
            .unwrap_or_default();
        if fc.is_empty() {
            fc = get_str(r, "financial_currency");
        }
        lines.push(format!("{}（{}）：", label, fc));
        for (d, x) in rows {
            let null = Value::Null;
            let rev = x.get("TotalRevenue").unwrap_or(&null);
            let gp = x.get("GrossProfit").unwrap_or(&null);
            let ni = x.get("NetIncome").unwrap_or(&null);
            let gm = if truthy(rev) && truthy(gp) {
                format!("{:.1}%", f64_of(gp) / f64_of(rev) * 100.0)
            } else {
                "—".into()
            };
            let nm = if truthy(rev) && !ni.is_null() {
                format!("{:.1}%", f64_of(ni) / f64_of(rev) * 100.0)
            } else {
                "—".into()
            };
            let eps = x
                .get("DilutedEPS")
                .map(num_str)
                .unwrap_or_else(|| "—".into());
            lines.push(format!(
                "  {}: 营收 revenue {}；毛利 gross profit {}（毛利率 gross margin {}）；经营利润 operating income {}；净利润 net income {}（净利率 net margin {}）；摊薄 EPS {}",
                d,
                big(rev),
                big(gp),
                gm,
                big(x.get("OperatingIncome").unwrap_or(&null)),
                big(ni),
                nm,
                eps
            ));
        }
    }
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::extract::{Path, Query};
    use axum::http::{header, StatusCode};
    use axum::response::IntoResponse;
    use axum::routing::get;
    use axum::{Json, Router};
    use std::collections::HashMap;

    #[test]
    fn names_ranges_symbols() {
        assert_eq!(
            name_to_symbol("Mapletree Pan Asia Commercial Trust"),
            Some("N2IU.SI")
        );
        assert_eq!(name_to_symbol("CICT"), Some("C38U.SI"));
        assert_eq!(norm_range("").unwrap(), "1y");
        assert_eq!(norm_range("12M").unwrap(), "1y");
        assert_eq!(norm_range("week").unwrap(), "5d");
        assert_eq!(
            norm_range("7y").unwrap_err().0,
            "range 应为 5d, 1mo, 3mo, 6mo, ytd, 1y, 2y, 3y, 5y, 10y, max 之一 (range must be one of 5d, 1mo, 3mo, 6mo, ytd, 1y, 2y, 3y, 5y, 10y, max)"
        );
        assert_eq!(resolve_local("700").as_deref(), Some("0700.HK"));
        assert_eq!(resolve_local("aapl").as_deref(), Some("AAPL"));
        assert_eq!(resolve_local("^hsi").as_deref(), Some("^hsi"));
        assert_eq!(resolve_local("腾讯").as_deref(), Some("0700.HK"));
        assert_eq!(resolve_local("tencent"), None);
        assert_eq!(resolve_local("NOPE.XX").as_deref(), Some("NOPE.XX"));
        assert_eq!(round_price(7.123456), 7.1235);
        assert_eq!(round_price(404.006), 404.01);
    }

    #[test]
    fn describe_text() {
        let r = json!({"symbol": "0700.HK", "name": "Tencent", "exchange": "HKSE", "currency": "HKD", "range": "1y", "interval": "1wk",
            "last": 404.0, "last_time": "2025-09-22", "day_change_pct": null, "start": {"date": "2024-09-30", "close": 400.0},
            "change_pct": 1.0, "high": {"date": "2025-03-31", "price": 520.0}, "low": {"date": "2024-09-30", "price": 399.0},
            "week52": [400.0, 515.0], "series": [["2024-09-30", 400.0], ["2024-10-07", 404.5]]});
        assert_eq!(
            describe(&r),
            "0700.HK · Tencent · HKSE · HKD\n最新价 last 404.0 HKD（2025-09-22）\n区间 1y：起 start 400.0 (2024-09-30) → 现 now 404.0，涨跌 change +1.00%\n\
区间最高 high 520.0 (2025-03-31)；最低 low 399.0 (2024-09-30)\n52 周区间 52-week range: 400.0 – 515.0\n收盘价序列（周线, 2 点）closes: 2024-09-30 400, 2024-10-07 404.5"
        );
        let mut r2 = r.clone();
        r2["change_pct"] = Value::Null;
        r2["day_change_pct"] = json!(-1.234);
        let t = describe(&r2);
        assert!(t.contains("（2025-09-22，较前收 -1.23%）") && !t.contains("区间 1y"));
    }

    #[test]
    fn fundamentals_text() {
        let r = json!({"symbol": "TSLA", "name": "Tesla, Inc.", "currency": "USD", "financial_currency": "USD", "price": 354.11,
            "market_cap": 1.4e12, "pe_ttm": 334.07, "pe_forward": 167.035, "eps_ttm": 1.06, "pb": 3.1, "dividend_yield_pct": null,
            "week52": [1, 2], "earnings_date": "2026-08-28",
            "quarterly": {"2026-06-30": {"TotalRevenue": 25e9, "currency": "USD", "GrossProfit": 4.75e9, "OperatingIncome": 1e9, "NetIncome": 1.1e9, "DilutedEPS": 0.32}},
            "annual": {}});
        let t = describe_fundamentals(&r);
        assert!(t.starts_with("TSLA · Tesla, Inc. · 股价 price 354.11 USD\n市值 market cap 1.40T USD；市盈率 P/E (TTM) 334.07；预期市盈率 forward P/E 167.03"), "{}", t);
        assert!(t.contains("股息率 dividend yield —\n最近/下次财报日 earnings date 2026-08-28\n季度财报 quarterly（USD）："));
        assert!(t.contains("  2026-06-30: 营收 revenue 25.00B；毛利 gross profit 4.75B（毛利率 gross margin 19.0%）；经营利润 operating income 1.00B；净利润 net income 1.10B（净利率 net margin 4.4%）；摊薄 EPS 0.32"));
        assert!(!t.contains("年度财报"));
        assert_eq!(big(&json!(512.5)), "512.5");
        assert_eq!(big(&Value::Null), "—");
    }

    // ---------------------------------------------------------------- fake Yahoo (tests/fake_apps.py): host 1 is rate-limited
    fn mkt(sym: &str) -> Option<(&'static str, &'static str, f64)> {
        match sym {
            "0700.HK" => Some(("Tencent Holdings Limited", "HKD", 400.0)),
            "0005.HK" => Some(("HSBC Holdings plc", "HKD", 100.0)),
            _ => None,
        }
    }

    async fn serve() -> String {
        let app = Router::new()
            .route("/yahoo1/{*rest}", get(|| async { (StatusCode::TOO_MANY_REQUESTS, [(header::CONTENT_TYPE, "text/html")], "busy") }))
            .route(
                "/yahoo2/v1/finance/search",
                get(|Query(q): Query<HashMap<String, String>>| async move {
                    let ql = q.get("q").cloned().unwrap_or_default().to_lowercase();
                    let hits: Vec<Value> = ["0700.HK", "0005.HK"]
                        .iter()
                        .filter(|s| mkt(s).unwrap().0.to_lowercase().contains(&ql))
                        .map(|s| json!({"symbol": s}))
                        .collect();
                    Json(json!({"quotes": hits}))
                }),
            )
            .route(
                "/yahoo2/v8/finance/chart/{sym}",
                get(|Path(sym): Path<String>| async move {
                    let Some((name, cur, p0)) = mkt(&sym) else {
                        return (StatusCode::NOT_FOUND, Json(json!({"chart": {"result": null, "error": {"description": "No data found"}}}))).into_response();
                    };
                    let n = 52i64;
                    let step = 7 * 86400;
                    let t0 = 1759104000 - n * step;
                    let closes: Vec<f64> =
                        (0..n).map(|i| pyround(p0 * (1.0 + 0.01 * i as f64) + if i == n / 2 { 15.0 } else { 0.0 }, 2)).collect();
                    let lo = closes.iter().cloned().fold(f64::MAX, f64::min);
                    let hi = closes.iter().cloned().fold(f64::MIN, f64::max);
                    Json(json!({"chart": {"result": [{"meta": {"symbol": sym, "longName": name, "currency": cur, "fullExchangeName": "HKSE",
                        "regularMarketPrice": closes[closes.len() - 1], "regularMarketTime": t0 + (n - 1) * step, "gmtoffset": 28800,
                        "fiftyTwoWeekLow": lo, "fiftyTwoWeekHigh": hi},
                        "timestamp": (0..n).map(|i| t0 + i * step).collect::<Vec<_>>(),
                        "indicators": {"quote": [{"close": closes, "high": closes.iter().map(|c| c + 1.0).collect::<Vec<_>>(),
                            "low": closes.iter().map(|c| c - 1.0).collect::<Vec<_>>()}]}}], "error": null}}))
                    .into_response()
                }),
            )
            .route("/yahoo2/v1/test/getcrumb", get(|| async { "Crumb123" }))
            .route(
                "/yahoo2/v7/finance/quote",
                get(|Query(q): Query<HashMap<String, String>>| async move {
                    if q.get("crumb").map(String::as_str) != Some("Crumb123") {
                        return (StatusCode::UNAUTHORIZED, Json(json!({}))).into_response();
                    }
                    let out: Vec<Value> = q["symbols"]
                        .split(',')
                        .filter(|s| *s == "TSLA")
                        .map(|s| json!({"symbol": s, "longName": "Tesla, Inc.", "currency": "USD", "regularMarketPrice": 354.11,
                            "marketCap": 1.4e12, "trailingPE": 334.07, "dividendYield": null, "trailingAnnualDividendYield": 0.0054,
                            "earningsTimestamp": 1787904600}))
                        .collect();
                    Json(json!({"quoteResponse": {"result": out}})).into_response()
                }),
            )
            .route(
                "/yahoo2/ws/fundamentals-timeseries/v1/finance/timeseries/{sym}",
                get(|Path(sym): Path<String>, Query(q): Query<HashMap<String, String>>| async move {
                    if sym != "TSLA" {
                        return Json(json!({"timeseries": {"result": []}}));
                    }
                    let mut res = Vec::new();
                    for t in q["type"].split(',') {
                        let kind = if t.starts_with("quarterly") { "quarterly" } else { "annual" };
                        let dates: &[&str] = if kind == "quarterly" {
                            &["2025-06-30", "2025-09-30", "2025-12-31", "2026-03-31", "2026-06-30"]
                        } else {
                            &["2024-12-31", "2025-12-31"]
                        };
                        let base = match &t[kind.len()..] {
                            "TotalRevenue" => 25e9,
                            "GrossProfit" => 4.75e9,
                            "OperatingIncome" => 1e9,
                            "NetIncome" => 1.1e9,
                            _ => 0.32,
                        };
                        res.push(json!({"meta": {"type": [t]}, t: dates.iter().map(|d| json!({"asOfDate": d, "currencyCode": "USD", "reportedValue": {"raw": base}})).collect::<Vec<_>>()}));
                    }
                    Json(json!({"timeseries": {"result": res}}))
                }),
            );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        format!("http://{}", addr)
    }

    #[tokio::test]
    async fn fetch_with_fallback_host_search_and_unknown() {
        let base = serve().await;
        let m = Market::new(
            reqwest::Client::new(),
            vec![format!("{}/yahoo1", base), format!("{}/yahoo2", base)],
        );
        let r = m
            .fetch(
                &[
                    "腾讯".into(),
                    "HSBC Holdings".into(),
                    "NOPE.XX".into(),
                    "7y".into(),
                ],
                "2y",
            )
            .await;
        let res = r["results"].as_array().unwrap();
        assert_eq!(res.len(), 2, "{}", r);
        assert_eq!(res[0]["symbol"], json!("0700.HK"));
        assert_eq!(res[1]["name"], json!("HSBC Holdings plc"));
        assert_eq!(res[0]["series"].as_array().unwrap().len(), 52);
        assert_eq!(
            res[0]["high"],
            json!({"date": "2025-09-22", "price": 605.0})
        );
        assert_eq!(
            r["errors"][0],
            json!("NOPE.XX: 代码不存在 unknown symbol: NOPE.XX")
        );
        assert_eq!(r["errors"][1], json!("7y: 代码不存在 unknown symbol: 7Y"));
        let text = describe(&res[0]);
        assert!(text.starts_with("0700.HK · Tencent Holdings Limited · HKSE · HKD\n最新价 last 604.0 HKD（2025-09-22）"), "{}", text);
        assert!(
            text.contains("涨跌 change +51.00%")
                && text.contains("区间最高 high 605.0 (20")
                && text.contains("closes: 2024-09-30 400, ")
        );
        let bad = m.fetch(&["0700.HK".into()], "7y").await;
        assert!(bad["errors"][0]
            .as_str()
            .unwrap()
            .starts_with("0700.HK: range 应为"));
    }

    #[tokio::test]
    async fn fundamentals_crumb_flow() {
        let base = serve().await;
        let m = Market::new(
            reqwest::Client::new(),
            vec![format!("{}/yahoo1", base), format!("{}/yahoo2", base)],
        );
        let r = m
            .fundamentals(&["特斯拉".into(), "NOPE.XX".into(), "^HSI".into()])
            .await;
        let res = r["results"].as_array().unwrap();
        assert_eq!(res.len(), 1, "{}", r);
        let t = &res[0];
        assert_eq!(t["dividend_yield_pct"], json!(0.54));
        assert_eq!(t["earnings_date"], json!("2026-08-28"));
        let q = t["quarterly"].as_object().unwrap();
        assert_eq!(
            q.keys().collect::<Vec<_>>(),
            ["2025-09-30", "2025-12-31", "2026-03-31", "2026-06-30"]
        );
        assert_eq!(
            q["2026-06-30"].as_object().unwrap().keys().next().unwrap(),
            "TotalRevenue"
        );
        let text = describe_fundamentals(t);
        assert!(
            text.contains("市盈率 P/E (TTM) 334.07")
                && text.contains("股息率 dividend yield 0.54%")
                && text.contains("市值 market cap 1.40T USD")
        );
        assert!(text.contains("毛利率 gross margin 19.0%") && text.contains("年度财报 annual"));
        let errs: Vec<&str> = r["errors"]
            .as_array()
            .unwrap()
            .iter()
            .map(|e| e.as_str().unwrap())
            .collect();
        assert_eq!(
            errs,
            [
                "NOPE.XX: 没有数据 no data for NOPE.XX",
                "^HSI: 没有数据 no data for ^HSI"
            ]
        );
    }
}
