//! The text the agent gets for `market_data` and `stock_fundamentals`: `describe` / `describe_fundamentals` of
//! `app/sentinel/market.py`. Sentinel fetches the numbers (`/internal/market_data`, `/internal/fundamentals`); the
//! runtime only formats them, so the two formatters live here (the runtime crate does not depend on the connector
//! crates).

use omuse_common::util::{get_str, truthy};
use serde_json::Value;

use super::util::{fmt_g, pystr_opt};

fn f64_of(v: Option<&Value>) -> Option<f64> {
    v.and_then(omuse_common::util::as_f64)
}

/// Python `str(x)` of a JSON number or null the way the f-strings printed them.
fn num_str(v: Option<&Value>) -> String {
    pystr_opt(v)
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
    let day = f64_of(r.get("day_change_pct"));
    lines.push(format!(
        "最新价 last {} {}（{}{}）",
        num_str(r.get("last")),
        cur,
        get_str(r, "last_time"),
        day.map(|d| format!("，较前收 {:+.2}%", d))
            .unwrap_or_default()
    ));
    if let Some(ch) = f64_of(r.get("change_pct")) {
        let start = r.get("start").cloned().unwrap_or(Value::Null);
        lines.push(format!(
            "区间 {}：起 start {} ({}) → 现 now {}，涨跌 change {:+.2}%",
            get_str(r, "range"),
            num_str(start.get("close")),
            get_str(&start, "date"),
            num_str(r.get("last")),
            ch
        ));
    } else {
        lines.push(String::new());
    }
    let (hi, lo) = (
        r.get("high").cloned().unwrap_or(Value::Null),
        r.get("low").cloned().unwrap_or(Value::Null),
    );
    lines.push(format!(
        "区间最高 high {} ({})；最低 low {} ({})",
        num_str(hi.get("price")),
        get_str(&hi, "date"),
        num_str(lo.get("price")),
        get_str(&lo, "date")
    ));
    if let Some(w) = r.get("week52").and_then(Value::as_array) {
        if !w.is_empty() && w.iter().all(truthy) {
            lines.push(format!(
                "52 周区间 52-week range: {} – {}",
                num_str(w.first()),
                num_str(w.get(1))
            ));
        }
    }
    let step = match get_str(r, "interval").as_str() {
        "1d" => "日",
        "1wk" => "周",
        "1mo" => "月",
        "3mo" => "季",
        _ => "",
    };
    let series = r
        .get("series")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let closes: Vec<String> = series
        .iter()
        .map(|p| {
            let d = p.get(0).map(omuse_common::util::py_str).unwrap_or_default();
            let c = f64_of(p.get(1)).unwrap_or(0.0);
            format!("{} {}", d, fmt_g(c))
        })
        .collect();
    lines.push(format!(
        "收盘价序列（{}线, {} 点）closes: {}",
        step,
        series.len(),
        closes.join(", ")
    ));
    lines
        .into_iter()
        .filter(|x| !x.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

fn big(v: Option<&Value>) -> String {
    let Some(v) = f64_of(v) else {
        return "—".into();
    };
    for (n, u) in [(1e12, "T"), (1e9, "B"), (1e6, "M")] {
        if v.abs() >= n {
            return format!("{:.2}{}", v / n, u);
        }
    }
    fmt_g(v)
}

fn f2(v: Option<&Value>) -> String {
    match f64_of(v) {
        Some(x) => format!("{:.2}", x),
        None => "—".into(),
    }
}

/// Valuation and reported financials as text.
pub fn describe_fundamentals(r: &Value) -> String {
    let cur = get_str(r, "currency");
    let mut lines: Vec<String> = Vec::new();
    lines.push(format!(
        "{} · {} · 股价 price {} {}",
        get_str(r, "symbol"),
        get_str(r, "name"),
        num_str(r.get("price")),
        cur
    ));
    let dy = match f64_of(r.get("dividend_yield_pct")) {
        Some(d) => format!("{:.2}%", d),
        None => "—".into(),
    };
    lines.push(format!(
        "市值 market cap {} {}；市盈率 P/E (TTM) {}；预期市盈率 forward P/E {}；市净率 P/B {}；每股收益 EPS (TTM) {}；股息率 dividend yield {}",
        big(r.get("market_cap")),
        cur,
        f2(r.get("pe_ttm")),
        f2(r.get("pe_forward")),
        f2(r.get("pb")),
        f2(r.get("eps_ttm")),
        dy
    ));
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
            .filter(|o| !o.is_empty())
        else {
            continue;
        };
        let fc = rows
            .values()
            .next()
            .map(|x| get_str(x, "currency"))
            .filter(|c| !c.is_empty())
            .unwrap_or_else(|| get_str(r, "financial_currency"));
        lines.push(format!("{}（{}）：", label, fc));
        for (d, x) in rows {
            let (rev, gp, ni) = (
                f64_of(x.get("TotalRevenue")),
                f64_of(x.get("GrossProfit")),
                f64_of(x.get("NetIncome")),
            );
            let gm = match (rev, gp) {
                (Some(r), Some(g)) if r != 0.0 && g != 0.0 => format!("{:.1}%", g / r * 100.0),
                _ => "—".into(),
            };
            let nm = match (rev, ni) {
                (Some(r), Some(n)) if r != 0.0 => format!("{:.1}%", n / r * 100.0),
                _ => "—".into(),
            };
            let eps = match x.get("DilutedEPS") {
                Some(v) => omuse_common::util::py_str(v),
                None => "—".into(),
            };
            lines.push(format!(
                "  {}: 营收 revenue {}；毛利 gross profit {}（毛利率 gross margin {}）；经营利润 operating income {}；净利润 net income {}（净利率 net margin {}）；摊薄 EPS {}",
                d,
                big(x.get("TotalRevenue")),
                big(x.get("GrossProfit")),
                gm,
                big(x.get("OperatingIncome")),
                big(x.get("NetIncome")),
                nm,
                eps
            ));
        }
    }
    lines.join("\n")
}
