//! A currency conversion the user's request asks for ("how much is 300 US dollars in dong?"), computed in code: the
//! amount and both currencies come from the request, the rate from a `market_data` result (code fetches the pair when
//! the run did not), and the line is appended to the answer — the model's own arithmetic or its doubts about the rate
//! never decide the figure (city_guides_real 2026-11-01: the answer gave no amount of dong at all).

use std::sync::LazyLock;

use omuse_common::util::get_str;
use regex::Regex;
use serde_json::{json, Value};

use super::util::{content_of, stored_call, tool_calls};
use crate::llm::ToolCall;

/// The currencies a request may name: ISO code, the words for it (longest first within the table's order: a more
/// specific name comes before a generic one), its symbol for the answer and its decimals.
const CURRENCIES: &[(&str, &str, &str, usize)] = &[
    ("SGD", r"singapore dollars?|sgd|s\$", "S$", 2),
    ("HKD", r"hong kong dollars?|hkd|hk\$", "HK$", 2),
    ("AUD", r"australian dollars?|aud|a\$", "A$", 2),
    (
        "TWD",
        r"new taiwan dollars?|taiwan dollars?|twd|nt\$",
        "NT$",
        0,
    ),
    ("CAD", r"canadian dollars?|cad|c\$", "C$", 2),
    (
        "USD",
        r"us dollars?|u\.s\. dollars?|american dollars?|usd|us\$|dollars?|\$|美元",
        "US$",
        2,
    ),
    ("VND", r"vietnamese dong|dong|đồng|vnd|₫|越南盾", "₫", 0),
    ("JPY", r"japanese yen|yen|jpy|円|日元", "¥", 0),
    (
        "CNY",
        r"chinese yuan|yuan|renminbi|rmb|cny|人民币",
        "CN¥",
        2,
    ),
    ("EUR", r"euros?|eur|€|欧元", "€", 2),
    (
        "GBP",
        r"british pounds?|pounds? sterling|pounds?|gbp|£|英镑",
        "£",
        2,
    ),
    ("THB", r"thai baht|baht|thb|฿|泰铢", "฿", 0),
    ("KRW", r"korean won|won|krw|₩|韩元", "₩", 0),
    ("MYR", r"malaysian ringgit|ringgit|myr|rm", "RM", 2),
    ("IDR", r"indonesian rupiah|rupiah|idr", "Rp", 0),
    ("PHP", r"philippine pesos?|php|₱", "₱", 2),
    ("INR", r"indian rupees?|rupees?|inr|₹", "₹", 0),
    ("CHF", r"swiss francs?|chf", "CHF ", 2),
];

/// One alternation of every currency word, each in its own named group order (`c0`, `c1`, …).
static CUR: LazyLock<Regex> = LazyLock::new(|| {
    let alts: Vec<String> = CURRENCIES
        .iter()
        .enumerate()
        .map(|(i, (_, w, _, _))| format!("(?P<c{i}>{w})"))
        .collect();
    // a word currency needs word edges; symbols ($, ₫) do not
    Regex::new(&format!(
        r"(?i)(?:^|[^\p{{L}}])(?:{})(?:$|[^\p{{L}}])",
        alts.join("|")
    ))
    .unwrap()
});
static AMOUNT: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(\d{1,3}(?:,\d{3})+|\d+)(?:\.(\d+))?").unwrap());
static INTO: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)(?:\b(?:in|into|to)\s+|换成|换算成|兑换成)$").unwrap());

/// The currency mentions of `text`: (start, end, ISO code).
fn mentions(text: &str) -> Vec<(usize, usize, &'static str)> {
    let mut out = Vec::new();
    let mut at = 0;
    while let Some(caps) = CUR.captures_at(text, at) {
        let Some((i, m)) =
            (0..CURRENCIES.len()).find_map(|i| caps.name(&format!("c{i}")).map(|m| (i, m)))
        else {
            break;
        };
        out.push((m.start(), m.end(), CURRENCIES[i].0));
        at = m.end().max(at + 1);
        if at >= text.len() {
            break;
        }
    }
    out
}

/// The conversion a request asks for: `(amount, from, to)` — an amount written next to a currency ("300 US dollars",
/// "US$300") and another currency after "in" / "into" / "to" ("in dong").
pub fn fx_request(goal: &str) -> Option<(f64, &'static str, &'static str)> {
    let ms = mentions(goal);
    let mut from: Option<(f64, &'static str)> = None;
    for &(s, e, code) in &ms {
        let before = goal[..s].trim_end();
        let after = goal[e..].trim_start();
        let num = |t: &str| -> Option<f64> { t.replace(',', "").parse::<f64>().ok() };
        // "300 US dollars": the number right before the currency
        let prev = AMOUNT
            .find_iter(before)
            .last()
            .filter(|m| m.end() == before.len())
            .and_then(|m| num(m.as_str()));
        // "US$300": the number right after it
        let next = AMOUNT
            .find(after)
            .filter(|m| m.start() == 0)
            .and_then(|m| num(m.as_str()));
        if let Some(a) = prev.or(next).filter(|a| *a > 0.0) {
            from = Some((a, code));
            break;
        }
    }
    let (amount, from) = from?;
    let to = ms
        .iter()
        .find(|&&(s, _, code)| code != from && INTO.is_match(&goal[..s]))
        .map(|&(_, _, c)| c)?;
    Some((amount, from, to))
}

/// The rate of `from` → `to` a `market_data` result in the run showed, with its date: the pair's own symbol
/// (`USDVND=X`) or the inverse one.
pub fn fx_rate(transcript: &[Value], from: &str, to: &str) -> Option<(f64, String)> {
    static HEAD: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?m)^([A-Z]{3})([A-Z]{3})=X · .*\n最新价 last ([\d.]+) [A-Z]*（([^）]*)）")
            .unwrap()
    });
    let ids: Vec<String> = transcript
        .iter()
        .flat_map(tool_calls)
        .filter(|tc| stored_call(tc).0 == "market_data")
        .map(|tc| get_str(tc, "id"))
        .collect();
    transcript
        .iter()
        .filter(|m| get_str(m, "role") == "tool" && ids.contains(&get_str(m, "tool_call_id")))
        .rev()
        .find_map(|m| {
            let c = content_of(m);
            HEAD.captures_iter(&c).find_map(|cap| {
                let last: f64 = cap[3].parse().ok().filter(|x: &f64| *x > 0.0)?;
                let date = cap[4].split('，').next().unwrap_or("").trim().to_string();
                if &cap[1] == from && &cap[2] == to {
                    Some((last, date))
                } else if &cap[1] == to && &cap[2] == from {
                    Some((1.0 / last, date))
                } else {
                    None
                }
            })
        })
}

/// The call that fetches the pair's rate, when the request asks for a conversion and no result shows the rate yet.
pub fn fx_rate_call(transcript: &[Value], goal: &str) -> Option<ToolCall> {
    let (_, from, to) = fx_request(goal)?;
    if fx_rate(transcript, from, to).is_some() {
        return None;
    }
    Some(ToolCall {
        id: "fx_rate".into(),
        name: "market_data".into(),
        args: json!({"symbols": [format!("{from}{to}=X")], "range": "5d"}),
    })
}

fn info(code: &str) -> (&'static str, usize) {
    CURRENCIES
        .iter()
        .find(|c| c.0 == code)
        .map(|c| (c.2, c.3))
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

/// The answer with the conversion the request asks for, computed from the request's amount and the market rate.
pub fn with_fx_conversion(transcript: &[Value], goal: &str, answer: &str, zh: bool) -> String {
    let Some((amount, from, to)) = fx_request(goal) else {
        return answer.to_string();
    };
    let Some((rate, date)) = fx_rate(transcript, from, to) else {
        return answer.to_string();
    };
    let ((fs, fdp), (ts, tdp)) = (info(from), info(to));
    let a = format!(
        "{fs}{}",
        money(amount, if amount.fract() == 0.0 { 0 } else { fdp })
    );
    let b = format!("{ts}{}", money(amount * rate, tdp));
    let r = money(rate, if rate >= 100.0 { 1 } else { 4 });
    let line = if zh {
        format!("**{a} ≈ {b}**（按 {from}/{to} {r} 计算，行情数据 {date}；兑换时银行或兑换点的汇率和手续费会不同）")
    } else {
        format!("**{a} ≈ {b}** (at {from}/{to} {r}, market data of {date}; a bank or exchange counter gives its own rate and fees)")
    };
    format!("{}\n\n{line}", answer.trim_end())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn market(id: &str, text: &str) -> Vec<Value> {
        vec![
            json!({"role": "assistant", "content": "", "tool_calls": [{"id": id, "type": "function",
                   "function": {"name": "market_data", "arguments": "{\"symbols\": [\"USDVND=X\"]}"}}]}),
            json!({"role": "tool", "tool_call_id": id, "content": text}),
        ]
    }

    const GOAL: &str = "I'm visiting Hanoi for a week: where should I stay, how do I get around, and how much is 300 US \
dollars in dong right now?";

    #[test]
    fn the_request_names_the_amount_and_both_currencies() {
        assert_eq!(fx_request(GOAL), Some((300.0, "USD", "VND")));
        assert_eq!(
            fx_request("What is US$1,250.50 in yen?"),
            Some((1250.5, "USD", "JPY"))
        );
        assert_eq!(
            fx_request("convert 50 euros to Singapore dollars"),
            Some((50.0, "EUR", "SGD"))
        );
        assert_eq!(fx_request("Where should I stay in Hanoi?"), None);
        assert_eq!(fx_request("Is 300 dollars a day enough?"), None);
    }

    #[test]
    fn the_conversion_is_appended_from_the_market_rate() {
        // city_guides_real 2026-11-01: the market rate was read, the model called it anomalous and gave no figure
        let t = market(
            "m1",
            "USDVND=X · USD/VND · HKSE · VND\n最新价 last 39486.5 VND（2026-10-31）\n区间 5d：起 start 26150.0",
        );
        let out = with_fx_conversion(&t, GOAL, "Stay in Hoàn Kiếm.", false);
        assert!(
            out.starts_with("Stay in Hoàn Kiếm.\n\n**US$300 ≈ ₫11,845,950**"),
            "{out}"
        );
        assert!(
            out.contains("USD/VND 39,486.5, market data of 2026-10-31"),
            "{out}"
        );
        // the inverse pair works too
        let inv = market(
            "m2",
            "VNDUSD=X · VND/USD · CCY · USD\n最新价 last 0.00004 USD（2026-10-31）",
        );
        assert!(with_fx_conversion(&inv, GOAL, "x", false).contains("≈ ₫7,500,000"));
        // no rate, no request: nothing added
        assert_eq!(with_fx_conversion(&[], GOAL, "x", false), "x");
        assert_eq!(
            with_fx_conversion(&t, "Where should I stay?", "x", false),
            "x"
        );
    }

    #[test]
    fn the_rate_is_fetched_only_when_no_result_shows_it() {
        let call = fx_rate_call(&[], GOAL).unwrap();
        assert_eq!(call.name, "market_data");
        assert_eq!(call.args["symbols"][0], "USDVND=X");
        let t = market(
            "m1",
            "USDVND=X · USD/VND · HKSE · VND\n最新价 last 39486.5 VND（2026-10-31）",
        );
        assert!(fx_rate_call(&t, GOAL).is_none());
        assert!(fx_rate_call(&[], "Where should I stay?").is_none());
    }
}
