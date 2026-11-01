//! `fx_convert{amount, from, to}`: a currency conversion computed in code. The model reads the user's words and calls
//! the tool with ISO codes; code checks the codes against its table, fetches the pair's market rate (Sentinel's
//! `/internal/market_data`, the structured result: `symbol`, `last`, `last_time`) and returns the figure as JSON with
//! the line the reply must carry (`must_say`) — the model's own arithmetic or its doubts about the rate never decide
//! the figure (city_guides_real 2026-11-01: the answer gave no amount of dong at all).

use omuse_common::util::get_str;
use serde_json::{json, Value};

/// The currencies the tool converts: ISO code, its symbol for the reply and its decimals.
const CURRENCIES: &[(&str, &str, usize)] = &[
    ("SGD", "S$", 2),
    ("HKD", "HK$", 2),
    ("AUD", "A$", 2),
    ("TWD", "NT$", 0),
    ("CAD", "C$", 2),
    ("USD", "US$", 2),
    ("VND", "₫", 0),
    ("JPY", "¥", 0),
    ("CNY", "CN¥", 2),
    ("EUR", "€", 2),
    ("GBP", "£", 2),
    ("THB", "฿", 0),
    ("KRW", "₩", 0),
    ("MYR", "RM", 2),
    ("IDR", "Rp", 0),
    ("PHP", "₱", 2),
    ("INR", "₹", 0),
    ("CHF", "CHF ", 2),
    ("NZD", "NZ$", 2),
];

/// The ISO code of a currency argument when the table knows it ("usd" → "USD"); `None` otherwise.
pub fn code(arg: &str) -> Option<&'static str> {
    let a = arg.trim().to_uppercase();
    CURRENCIES.iter().map(|c| c.0).find(|c| *c == a)
}

/// The arguments checked: `(amount, from, to)` (the amount defaults to 1, a rate), or the problems the model can fix.
pub fn check(args: &Value) -> Result<(f64, &'static str, &'static str), Vec<String>> {
    let mut problems = Vec::new();
    let amount = match args.get("amount") {
        None | Some(Value::Null) => Some(1.0),
        Some(v) => omuse_common::util::as_f64(v).filter(|a| *a > 0.0),
    };
    if amount.is_none() {
        problems.push("amount: a positive number".to_string());
    }
    let known = CURRENCIES
        .iter()
        .map(|c| c.0)
        .collect::<Vec<_>>()
        .join(", ");
    let from = code(&get_str(args, "from"));
    let to = code(&get_str(args, "to"));
    if from.is_none() {
        problems.push(format!("from: one of {known}"));
    }
    if to.is_none() {
        problems.push(format!("to: one of {known}"));
    }
    match (amount, from, to) {
        (Some(a), Some(f), Some(t)) if f != t => Ok((a, f, t)),
        (Some(_), Some(_), Some(_)) => Err(vec!["from and to are the same currency".into()]),
        _ => Err(problems),
    }
}

/// The market symbols of a pair: its own (`USDVND=X`) and the inverse one.
pub fn symbols(from: &str, to: &str) -> [String; 2] {
    [format!("{from}{to}=X"), format!("{to}{from}=X")]
}

/// The rate `from` → `to` and its date from Sentinel's structured market results: the pair's own symbol, else the
/// inverse one (1 / last).
pub fn rate_in(results: &[Value], from: &str, to: &str) -> Option<(f64, String)> {
    let [own, inv] = symbols(from, to);
    let last = |r: &Value| {
        r.get("last")
            .and_then(omuse_common::util::as_f64)
            .filter(|x| *x > 0.0)
    };
    let date = |r: &Value| {
        get_str(r, "last_time")
            .split([' ', 'T'])
            .next()
            .unwrap_or("")
            .to_string()
    };
    results
        .iter()
        .find(|r| get_str(r, "symbol") == own)
        .and_then(|r| Some((last(r)?, date(r))))
        .or_else(|| {
            results
                .iter()
                .find(|r| get_str(r, "symbol") == inv)
                .and_then(|r| Some((1.0 / last(r)?, date(r))))
        })
}

fn info(code: &str) -> (&'static str, usize) {
    CURRENCIES
        .iter()
        .find(|c| c.0 == code)
        .map(|c| (c.1, c.2))
        .unwrap_or(("", 2))
}

/// `x` with thousands separators and `dp` decimals.
fn money(x: f64, dp: usize) -> String {
    let s = format!("{x:.dp$}");
    let (int, frac) = s.split_once('.').map_or((s.as_str(), ""), |(a, b)| (a, b));
    let digits: Vec<char> = int.chars().collect();
    let mut out = String::new();
    for (i, ch) in digits.iter().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(*ch);
    }
    if frac.is_empty() {
        out
    } else {
        format!("{out}.{frac}")
    }
}

/// The tool's JSON result: the converted amount at the market rate of `date`, and the line the reply must carry.
pub fn conversion(amount: f64, from: &str, to: &str, rate: f64, date: &str, zh: bool) -> Value {
    let ((fs, fdp), (ts, tdp)) = (info(from), info(to));
    let a = format!(
        "{fs}{}",
        money(amount, if amount.fract() == 0.0 { 0 } else { fdp })
    );
    let converted = amount * rate;
    let b = format!(
        "{ts}{}",
        money(converted, if converted < 10.0 { 4 } else { tdp })
    );
    let r = money(rate, if rate >= 100.0 { 1 } else { 4 });
    let line = if zh {
        format!("{a} ≈ {b} {to}（按 {from}/{to} {r} 计算，行情数据 {date}；兑换时银行或兑换点的汇率和手续费会不同）")
    } else {
        format!("{a} ≈ {b} {to} (at {from}/{to} {r}, market data of {date}; a bank or exchange counter gives its own rate and fees)")
    };
    json!({"status": "ok", "amount": amount, "from": from, "to": to, "rate": rate, "as_of": date,
           "converted": (converted * 10000.0).round() / 10000.0, "source": "market data (Yahoo Finance)",
           "must_say": [line]})
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_arguments_are_iso_codes_and_a_positive_amount() {
        assert_eq!(
            check(&json!({"amount": 300, "from": "usd", "to": "VND"})),
            Ok((300.0, "USD", "VND"))
        );
        assert_eq!(
            check(&json!({"from": "SGD", "to": "JPY"})),
            Ok((1.0, "SGD", "JPY"))
        );
        let bad = check(&json!({"amount": -1, "from": "dollars", "to": "VND"})).unwrap_err();
        assert!(
            bad.iter().any(|p| p.starts_with("amount"))
                && bad.iter().any(|p| p.starts_with("from"))
        );
    }

    #[test]
    fn the_conversion_comes_from_the_market_rate_with_its_date() {
        // city_guides_real 2026-11-01: the market rate was read, the model called it anomalous and gave no figure
        let res = [json!({"symbol": "USDVND=X", "last": 39486.5, "last_time": "2026-10-31 16:00"})];
        let (rate, date) = rate_in(&res, "USD", "VND").unwrap();
        let v = conversion(300.0, "USD", "VND", rate, &date, false);
        let line = v["must_say"][0].as_str().unwrap();
        assert!(line.starts_with("US$300 ≈ ₫11,845,950 VND"), "{line}");
        assert!(
            line.contains("USD/VND 39,486.5, market data of 2026-10-31"),
            "{line}"
        );
        // the inverse pair works too
        let inv = [json!({"symbol": "VNDUSD=X", "last": 0.00004, "last_time": "2026-10-31"})];
        let (rate, _) = rate_in(&inv, "USD", "VND").unwrap();
        assert!(
            conversion(300.0, "USD", "VND", rate, "x", false)["must_say"][0]
                .as_str()
                .unwrap()
                .contains("≈ ₫7,500,000")
        );
        assert!(rate_in(&[], "USD", "VND").is_none());
    }
}
