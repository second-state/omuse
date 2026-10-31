//! Links the model writes, repaired in code. Booking and compare pages carry encoded addresses (a Google Flights
//! itinerary's `tfs`, a Google Hotels stay's `ts`) that a model tends to "tidy" or mistype: on 2026-10-30 a real model
//! dropped a hotel page's `&ts=…` and the page opened on other dates. The model sees every link as it is (it must know
//! where a link goes: with opaque stand-ins it guessed addresses, and a long stand-in mistyped by one letter could not
//! be recovered), and wherever its words leave the agent — the answer and the arguments of a tool call — each link is
//! checked against the links the run's tool results actually showed ([`seen`]) and repaired ([`repair`]):
//! - a link a result showed stays as it is;
//! - a link cut short — the same host and path as a shown link, with some of its query parameters missing — becomes
//!   that shown link;
//! - a link whose path is one or two characters off a shown link's on the same host (a letter dropped while copying),
//!   with no query or the same or fewer query parameters, becomes that shown link;
//! - anything else (another page, other query values such as dates the model chose) stays as the model wrote it.
//!
//! Older conversations may still hold the stand-ins earlier versions handed out (`https://link.omuse.local/<base64>~<crc>`);
//! [`decode`] turns those back into their links.

use std::collections::BTreeSet;
use std::sync::LazyLock;

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine as _;
use regex::Regex;
use serde_json::Value;

/// The address of the stand-ins earlier versions handed out.
pub const BASE: &str = "https://link.omuse.local/";
static URL: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"https?://[^\s"'<>\\)\]\[|`]+"#).unwrap());
static STAND_IN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"https://link\.omuse\.local/([A-Za-z0-9_-]{8,}(?: [A-Za-z0-9_-]+){0,3})~([0-9a-f]{6})",
    )
    .unwrap()
});
static LEFTOVER_MD: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\[([^\]\n]*)\]\(https://link\.omuse\.local/[^)\n]*\)").unwrap());
static LEFTOVER: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"<?https://link\.omuse\.local/[A-Za-z0-9_~-]*>?").unwrap());

/// The FNV-1a checksum the old stand-ins carried.
fn check(url: &str) -> String {
    let mut h: u32 = 0x811c9dc5;
    for b in url.bytes() {
        h ^= u32::from(b);
        h = h.wrapping_mul(0x0100_0193);
    }
    format!("{:06x}", h & 0xff_ffff)
}

/// `text` with every old stand-in decoded into its link (only a whole one, its checksum matching).
pub fn decode(text: &str) -> String {
    if !text.contains(BASE) {
        return text.to_string();
    }
    STAND_IN
        .replace_all(text, |c: &regex::Captures| {
            URL_SAFE_NO_PAD
                .decode(c[1].replace(' ', ""))
                .ok()
                .and_then(|b| String::from_utf8(b).ok())
                .filter(|u| {
                    (u.starts_with("https://") || u.starts_with("http://")) && check(u) == c[2]
                })
                .unwrap_or_else(|| c[0].to_string())
        })
        .into_owned()
}

/// What is left of an old stand-in that cannot be decoded is taken out of an answer (a Markdown link keeps its label).
pub fn decode_answer(text: &str) -> String {
    let d = decode(text);
    if !d.contains(BASE) {
        return d;
    }
    let d = LEFTOVER_MD.replace_all(&d, "$1");
    LEFTOVER.replace_all(&d, "").into_owned()
}

/// Every string in `v` decoded (old stand-ins in a tool call's arguments).
pub fn decode_value(v: &mut Value) {
    match v {
        Value::String(s) if s.contains(BASE) => *s = decode(s),
        Value::Array(xs) => xs.iter_mut().for_each(decode_value),
        Value::Object(m) => m.values_mut().for_each(decode_value),
        _ => {}
    }
}

/// A link as found in text: trailing sentence punctuation is not part of it.
fn trimmed(u: &str) -> &str {
    u.trim_end_matches(['.', ',', ';', ':', '!', '?', '*', '_'])
}

/// The links the run's tool results showed (`role: tool` messages), in order, without repeats.
pub fn seen(transcript: &[Value]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut have = BTreeSet::new();
    for m in transcript {
        if m.get("role").and_then(Value::as_str) != Some("tool") {
            continue;
        }
        let text = match m.get("content") {
            Some(Value::String(s)) => s.replace("\\/", "/"),
            Some(other) => other.to_string(),
            None => continue,
        };
        for u in URL.find_iter(&decode(&text)) {
            let u = trimmed(u.as_str()).to_string();
            if have.insert(u.clone()) {
                out.push(u);
            }
        }
    }
    out
}

/// A link's query as `(name, value)` pairs.
type Query = Vec<(String, String)>;

/// `(host, path, query pairs)` of a link, lower-cased host, without a fragment.
fn parts(u: &str) -> Option<(String, String, Query)> {
    let rest = u
        .strip_prefix("https://")
        .or_else(|| u.strip_prefix("http://"))?;
    let rest = rest.split('#').next().unwrap_or("");
    let (hp, query) = rest.split_once('?').unwrap_or((rest, ""));
    let (host, path) = hp.split_once('/').unwrap_or((hp, ""));
    let pairs = query
        .split('&')
        .filter(|p| !p.is_empty())
        .map(|p| {
            let (k, v) = p.split_once('=').unwrap_or((p, ""));
            (k.to_string(), v.to_string())
        })
        .collect();
    Some((
        host.to_lowercase(),
        path.trim_end_matches('/').to_string(),
        pairs,
    ))
}

/// The edit distance of two short strings, or `limit + 1` once it is certainly above `limit`.
fn distance(a: &str, b: &str, limit: usize) -> usize {
    omuse_common::util::edit_distance(a, b, limit)
}

/// The shown link `u` stands for (see the module docs), or `None` to keep `u` as written.
fn mend<'a>(u: &str, seen: &'a [String]) -> Option<&'a String> {
    if seen.iter().any(|s| s == u) {
        return None;
    }
    let (host, path, query) = parts(u)?;
    let subset = |q: &[(String, String)], of: &[(String, String)]| q.iter().all(|p| of.contains(p));
    let mut best: Option<(usize, &String)> = None;
    let mut tie = false;
    for s in seen {
        let Some((h, p, q)) = parts(s) else {
            continue;
        };
        if h != host || !subset(&query, &q) || (query.len() == q.len() && p == path) {
            continue;
        }
        let d = if p == path {
            0
        } else {
            match distance(&path, &p, 2) {
                d if d <= 2 && path.chars().count() >= 4 => d,
                _ => continue,
            }
        };
        match best {
            Some((bd, bs)) if d == bd && bs != s => tie = true,
            Some((bd, _)) if d >= bd => {}
            _ => {
                best = Some((d, s));
                tie = false;
            }
        }
    }
    best.filter(|_| !tie).map(|(_, s)| s)
}

/// `text` with every link repaired against the links the run showed (module docs) and old stand-ins decoded.
pub fn repair(text: &str, seen: &[String]) -> String {
    let text = decode(text);
    if seen.is_empty() {
        return text;
    }
    URL.replace_all(&text, |c: &regex::Captures| {
        let whole = &c[0];
        let u = trimmed(whole);
        match mend(u, seen) {
            Some(s) => format!("{s}{}", &whole[u.len()..]),
            None => whole.to_string(),
        }
    })
    .into_owned()
}

/// The answer the user reads: links repaired, and what is left of an old stand-in that cannot be decoded taken out.
pub fn repair_answer(text: &str, seen: &[String]) -> String {
    decode_answer(&repair(text, seen))
}

/// Every string in `v` repaired (a tool call's arguments).
pub fn repair_value(v: &mut Value, seen: &[String]) {
    match v {
        Value::String(s) => {
            if s.contains("http") {
                *s = repair(s, seen);
            }
        }
        Value::Array(xs) => xs.iter_mut().for_each(|x| repair_value(x, seen)),
        Value::Object(m) => m.values_mut().for_each(|x| repair_value(x, seen)),
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const HOTEL: &str = "https://www.google.com/travel/hotels/entity/ChoI69b9xYKY0NjuARoNL2cvMTFmNGxjd2w0dBAB?hl=en&curr=SGD&ts=CAEaIAoCGgASGhIUCgcI6g8QCxgdEgcI6g8QDBgCGAMyAhAAKgkKBToDU0dEGgA";
    const ICA: &str = "https://eservices.ica.gov.sg/sgarrivalcard/";

    fn shown() -> Vec<String> {
        seen(&[
            json!({"role": "tool", "content": format!("{{\"booking_url\": \"{HOTEL}\", \"see\": \"{ICA}.\"}}")}),
            json!({"role": "tool", "content": "Compare: https://www.kayak.com/flights/SIN-HND,NRT/2026-11-30?sort=price_a"}),
            json!({"role": "user", "content": "https://never.example/shown"}),
        ])
    }

    #[test]
    fn the_links_results_showed_are_collected() {
        let s = shown();
        assert_eq!(
            s,
            vec![
                HOTEL.to_string(),
                ICA.to_string(),
                "https://www.kayak.com/flights/SIN-HND,NRT/2026-11-30?sort=price_a".to_string()
            ]
        );
    }

    #[test]
    fn a_tidied_or_mistyped_link_is_restored_and_others_are_left_alone() {
        let s = shown();
        // shown as is: untouched
        assert_eq!(
            repair(&format!("[Book]({HOTEL})"), &s),
            format!("[Book]({HOTEL})")
        );
        // the stay's ts dropped (2026-10-30): the whole link again
        let tidied = HOTEL.split("&ts=").next().unwrap();
        assert_eq!(
            repair(&format!("[Book]({tidied})"), &s),
            format!("[Book]({HOTEL})")
        );
        // a letter dropped while copying (arrival_address2_real 2026-10-30)
        assert_eq!(
            repair("Open https://eservices.ica.gov.sg/sgarrvalcard/.", &s),
            format!("Open {ICA}.")
        );
        // other dates the model chose, another page, another site: as written
        let other_dates = "https://www.kayak.com/flights/SIN-HND,NRT/2026-12-01?sort=price_a";
        assert_eq!(repair(other_dates, &s), other_dates);
        let other_page = "https://www.kayak.com/hotels/Tokyo/2026-11-30/2026-12-03";
        assert_eq!(repair(other_page, &s), other_page);
        assert_eq!(
            repair("https://www.ica.gov.sg/", &s),
            "https://www.ica.gov.sg/"
        );
        // in a tool call's arguments too
        let mut args =
            json!({"url": tidied, "list": ["https://eservices.ica.gov.sg/sgarrvalcard"]});
        repair_value(&mut args, &s);
        assert_eq!(args, json!({"url": HOTEL, "list": [ICA]}));
        // nothing shown yet: nothing to repair against
        assert_eq!(repair(tidied, &[]), tidied);
    }

    #[test]
    fn two_equally_close_links_leave_it_as_written() {
        let s = vec![
            "https://shop.test/item?id=1&colour=red".to_string(),
            "https://shop.test/item?id=1&colour=blue".to_string(),
        ];
        assert_eq!(
            repair("https://shop.test/item?id=1", &s),
            "https://shop.test/item?id=1"
        );
    }

    #[test]
    fn old_stand_ins_still_decode() {
        let old = format!("{BASE}{}~{}", URL_SAFE_NO_PAD.encode(ICA), check(ICA));
        assert_eq!(decode(&format!("[Card]({old})")), format!("[Card]({ICA})"));
        let cut = &old[..old.len() - 12];
        assert_eq!(
            decode_answer(&format!("Book · [Card]({cut}) now")),
            "Book · Card now"
        );
        let mut args = json!({"url": old});
        decode_value(&mut args);
        assert_eq!(args, json!({"url": ICA}));
    }
}
