//! One checking layer for the JSON the model returns (docs/json-tools.md, "Checking the model's JSON"): every tool
//! call's arguments and every other JSON reply (`decide()` choices, the final review's verdict, …) go through here, at
//! the boundary where language becomes data, never in each machine.
//!
//! 1. [`extract`] / [`parse_args`] repair the syntax in code: a code fence or prose around the object, single quotes,
//!    unquoted keys, Python literals, trailing commas, a string / object / array left open (a reply cut short), numbers
//!    with absurd precision. What was repaired is reported (the run loop emits `json_repaired`).
//! 2. [`conform`] checks the arguments against the tool's JSON schema: required fields, types and enums. A value that
//!    can be fixed is fixed (`"2"` → `2`, `"true"` → `true`, a date in another format → ISO, an enum in another case),
//!    one that cannot is dropped and reported. The tool's own field checks follow (e.g. `booking::check`).
//! 3. Still wrong (unparseable, a required field missing or invalid): the runtime asks the planner model once for the
//!    JSON again ([`regen_prompt`], `json_regenerated`) and runs 1–2 on its answer; then the tool result says
//!    `invalid` for the executor's one more try, and after that `needs_input`.
//!
//! Everything here is pure code; the regeneration call itself is the runtime's (`Runtime::checked_workflow_args`).

use chrono::NaiveDate;
use omuse_common::util::{py_str, take_chars, truthy};
use serde_json::{json, Map, Value};

/// The key the parsed arguments carry their syntax repairs under until the run loop reports and removes it.
pub const REPAIRED_KEY: &str = "_repaired";
/// The key of arguments that could not be parsed at all (the raw text, cut to 2000 characters).
pub const UNPARSED_KEY: &str = "_unparsed";

/// The first JSON object or array in a model reply, repaired where its syntax is broken: the value and the repairs
/// made (empty when it parsed as it was).
pub fn extract(text: &str) -> Option<(Value, Vec<String>)> {
    let t = text.trim();
    if t.is_empty() {
        return None;
    }
    if let Ok(v) = serde_json::from_str::<Value>(t) {
        if v.is_object() || v.is_array() {
            let mut repairs = Vec::new();
            let v = tame_numbers(v, &mut repairs);
            return Some((v, repairs));
        }
    }
    let mut repairs = Vec::new();
    let body = strip_wrapping(t, &mut repairs)?;
    let fixed = repair_syntax(&body, &mut repairs);
    let v: Value = serde_json::from_str(&fixed).ok()?;
    let v = tame_numbers(v, &mut repairs);
    Some((v, repairs))
}

/// Tool-call arguments from whatever the model sent: an object as it is; JSON text repaired (the repairs under
/// [`REPAIRED_KEY`]); a non-object value as `{"value": …}`; nothing as `{}`; text that is no JSON at all as
/// `{"_unparsed": …}`.
pub fn parse_args(raw: &Value) -> Value {
    if raw.is_object() {
        return raw.clone();
    }
    if !truthy(raw) {
        return json!({});
    }
    let s = py_str(raw);
    match extract(&s) {
        Some((Value::Object(mut o), repairs)) => {
            if !repairs.is_empty() {
                o.insert(REPAIRED_KEY.into(), json!(repairs));
            }
            Value::Object(o)
        }
        Some((v, _)) => json!({"value": v}),
        None => json!({UNPARSED_KEY: take_chars(s.trim(), 2000)}),
    }
}

/// The repairs `parse_args` recorded, removed from the arguments.
pub fn take_repairs(args: &mut Value) -> Vec<String> {
    args.as_object_mut()
        .and_then(|o| o.remove(REPAIRED_KEY))
        .and_then(|v| v.as_array().cloned())
        .unwrap_or_default()
        .iter()
        .filter_map(|x| x.as_str().map(str::to_string))
        .collect()
}

/// The object or array in the text: think blocks, code fences and prose around it removed.
fn strip_wrapping(t: &str, repairs: &mut Vec<String>) -> Option<String> {
    let mut s = t.to_string();
    while let (Some(i), Some(j)) = (s.find("<think>"), s.find("</think>")) {
        if j < i {
            break;
        }
        s.replace_range(i..j + "</think>".len(), "");
    }
    if let Some(i) = s.find("```") {
        let rest = &s[i + 3..];
        let rest = rest.strip_prefix("json").unwrap_or(rest);
        let inner = match rest.find("```") {
            Some(j) => &rest[..j],
            None => rest,
        };
        s = inner.to_string();
        repairs.push("code fence removed".into());
    }
    let start = s.find(['{', '['])?;
    if start > 0 && !s[..start].trim().is_empty() {
        repairs.push("text before the JSON removed".into());
    }
    Some(s[start..].to_string())
}

/// The text repaired into JSON by a string-aware scan: single-quoted strings, unquoted keys, Python literals, trailing
/// commas, and whatever is left open at the end closed; text after the outermost value is dropped.
fn repair_syntax(s: &str, repairs: &mut Vec<String>) -> String {
    let chars: Vec<char> = s.chars().collect();
    let mut out = String::with_capacity(s.len() + 8);
    let mut stack: Vec<char> = Vec::new();
    let note = |r: &str, repairs: &mut Vec<String>| {
        if !repairs.iter().any(|x| x == r) {
            repairs.push(r.into());
        }
    };
    let mut i = 0;
    let mut ended = false;
    while i < chars.len() {
        let c = chars[i];
        match c {
            '"' | '\'' => {
                if c == '\'' {
                    note("single quotes", repairs);
                }
                // a string: copied with `"` delimiters, inner `"` escaped when the delimiter was `'`
                out.push('"');
                i += 1;
                let mut closed = false;
                while i < chars.len() {
                    let d = chars[i];
                    if d == '\\' && i + 1 < chars.len() {
                        if c == '\'' && chars[i + 1] == '\'' {
                            out.push('\'');
                        } else {
                            out.push(d);
                            out.push(chars[i + 1]);
                        }
                        i += 2;
                        continue;
                    }
                    if d == c {
                        closed = true;
                        i += 1;
                        break;
                    }
                    if d == '"' {
                        out.push_str("\\\"");
                    } else if d == '\n' {
                        out.push_str("\\n");
                    } else {
                        out.push(d);
                    }
                    i += 1;
                }
                if !closed {
                    note("an open string closed", repairs);
                }
                out.push('"');
                continue;
            }
            '{' | '[' => {
                stack.push(if c == '{' { '}' } else { ']' });
                out.push(c);
            }
            '}' | ']' => {
                trim_trailing_comma(&mut out, repairs);
                if stack.last() == Some(&c) {
                    stack.pop();
                }
                out.push(c);
                if stack.is_empty() {
                    ended = true;
                    if chars[i + 1..].iter().any(|x| !x.is_whitespace()) {
                        note("text after the JSON removed", repairs);
                    }
                    break;
                }
            }
            c if c.is_alphabetic() || c == '_' => {
                // a bare word: a key when a colon follows, else a literal
                let st = i;
                while i < chars.len() && (chars[i].is_alphanumeric() || chars[i] == '_') {
                    i += 1;
                }
                let word: String = chars[st..i].iter().collect();
                let mut k = i;
                while k < chars.len() && chars[k].is_whitespace() {
                    k += 1;
                }
                if k < chars.len() && chars[k] == ':' && stack.last() == Some(&'}') {
                    note("unquoted key", repairs);
                    out.push('"');
                    out.push_str(&word);
                    out.push('"');
                } else {
                    let lit = match word.as_str() {
                        "True" => "true",
                        "False" => "false",
                        "None" => "null",
                        w => w,
                    };
                    if lit != word {
                        note("Python literal", repairs);
                    }
                    out.push_str(lit);
                }
                continue;
            }
            _ => out.push(c),
        }
        i += 1;
    }
    if !ended && !stack.is_empty() {
        // a reply cut short: a dangling key or comma dropped, the rest closed
        let t = out.trim_end().to_string();
        out = t;
        if out.ends_with(':') {
            out.push_str(" null");
        }
        trim_trailing_comma(&mut out, repairs);
        while let Some(cl) = stack.pop() {
            out.push(cl);
        }
        note("open brackets closed", repairs);
    }
    out
}

fn trim_trailing_comma(out: &mut String, repairs: &mut Vec<String>) {
    let t = out.trim_end().len();
    if out[..t].ends_with(',') {
        out.truncate(t - 1);
        if !repairs.iter().any(|x| x == "trailing comma") {
            repairs.push("trailing comma".into());
        }
    }
}

/// Numbers with absurd precision (`0.036000000000000004`, a run of digits cut short) cut to their value.
fn tame_numbers(v: Value, repairs: &mut Vec<String>) -> Value {
    match v {
        Value::Number(n) if n.is_f64() => {
            let f = n.as_f64().unwrap_or(0.0);
            let s = n.to_string();
            let decimals = s.split('.').nth(1).map(str::len).unwrap_or(0);
            if decimals > 8 {
                let r = format!("{f:.8}");
                let r = r.trim_end_matches('0').trim_end_matches('.');
                if let Ok(x) = r.parse::<f64>() {
                    if !repairs.iter().any(|x| x == "number precision") {
                        repairs.push("number precision".into());
                    }
                    return json!(x);
                }
            }
            Value::Number(n)
        }
        Value::Array(a) => Value::Array(a.into_iter().map(|x| tame_numbers(x, repairs)).collect()),
        Value::Object(o) => Value::Object(
            o.into_iter()
                .map(|(k, x)| (k, tame_numbers(x, repairs)))
                .collect(),
        ),
        x => x,
    }
}

/// What [`conform`] made of the arguments.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Conformed {
    /// the values fixed ("party: \"2\" → 2")
    pub fixed: Vec<String>,
    /// the values dropped or missing that the model can fix
    pub problems: Vec<String>,
    /// a required field missing or invalid (a reason to regenerate)
    pub required_bad: bool,
}

/// The arguments checked against the tool's parameters schema (`{"type": "object", "properties", "required"}`): types
/// fixed where they can be, enums matched in any case, dates in other formats made ISO; a value that cannot be fixed
/// is dropped and reported; keys the schema does not know are dropped.
pub fn conform(params: &Value, args: &mut Value) -> Conformed {
    let mut c = Conformed::default();
    let Some(o) = args.as_object_mut() else {
        c.problems
            .push("the arguments are not a JSON object".into());
        c.required_bad = true;
        return c;
    };
    let props = params
        .get("properties")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    let keys: Vec<String> = o.keys().cloned().collect();
    for k in keys {
        let Some(schema) = props.get(&k) else {
            if !k.starts_with('_') {
                o.remove(&k);
                c.fixed
                    .push(format!("{k}: not a field of the tool, dropped"));
            }
            continue;
        };
        let v = o[&k].clone();
        if v.is_null() || v.as_str().is_some_and(|s| s.trim().is_empty()) {
            o.remove(&k);
            continue;
        }
        match coerce(&k, schema, &v) {
            Ok(Some(nv)) => {
                if nv != v {
                    c.fixed.push(format!("{k}: {v} → {nv}"));
                }
                o.insert(k, nv);
            }
            Ok(None) => {
                o.remove(&k);
            }
            Err(why) => {
                o.remove(&k);
                c.problems.push(format!("{k}: {why}"));
            }
        }
    }
    for r in params
        .get("required")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
    {
        if !o.contains_key(r) {
            c.required_bad = true;
            if !c.problems.iter().any(|p| p.starts_with(&format!("{r}:"))) {
                c.problems.push(format!("{r}: required"));
            }
        } else if c.problems.iter().any(|p| p.starts_with(&format!("{r}:"))) {
            c.required_bad = true;
        }
    }
    c
}

/// A date in another format as ISO (`2026/12/04`, `04.12.2026`, `4 December 2026`, `December 4, 2026`, an ISO
/// datetime); `None` when it is no date with a year.
pub fn iso_date(s: &str) -> Option<String> {
    let t = s.trim();
    if let Ok(d) = NaiveDate::parse_from_str(t, "%Y-%m-%d") {
        return Some(d.format("%Y-%m-%d").to_string());
    }
    if t.len() > 10
        && t.as_bytes()
            .get(10)
            .is_some_and(|b| *b == b'T' || *b == b' ')
    {
        if let Ok(d) = NaiveDate::parse_from_str(&t[..10], "%Y-%m-%d") {
            return Some(d.format("%Y-%m-%d").to_string());
        }
    }
    let cleaned = t.replace(',', "");
    for f in [
        "%Y/%m/%d",
        "%Y.%m.%d",
        "%d.%m.%Y",
        "%d %B %Y",
        "%d %b %Y",
        "%B %d %Y",
        "%b %d %Y",
        "%A %d %B %Y",
        "%a %d %b %Y",
        "%Y年%m月%d日",
    ] {
        if let Ok(d) = NaiveDate::parse_from_str(&cleaned, f) {
            return Some(d.format("%Y-%m-%d").to_string());
        }
    }
    None
}

fn is_date_field(key: &str, schema: &Value) -> bool {
    matches!(
        key,
        "date" | "date_to" | "return_date" | "check_out" | "check_in"
    ) || schema
        .get("description")
        .and_then(Value::as_str)
        .is_some_and(|d| d.contains("YYYY-MM-DD"))
}

/// One value to the schema's type: `Ok(Some(v))` (fixed or as it was), `Ok(None)` (empty, dropped quietly),
/// `Err(why)` (dropped and reported).
fn coerce(key: &str, schema: &Value, v: &Value) -> Result<Option<Value>, String> {
    let ty = schema.get("type").and_then(Value::as_str).unwrap_or("");
    let num = |v: &Value| -> Option<f64> {
        match v {
            Value::Number(n) => n.as_f64(),
            Value::String(s) => {
                let t: String = s
                    .trim()
                    .chars()
                    .filter(|c| c.is_ascii_digit() || *c == '.' || *c == '-')
                    .collect();
                t.parse().ok()
            }
            _ => None,
        }
    };
    match ty {
        "integer" => match num(v) {
            Some(x) if x.fract() == 0.0 => Ok(Some(json!(x as i64))),
            Some(x) => Err(format!("{x} is not a whole number")),
            None => Err(format!("{v} is not a number")),
        },
        "number" => num(v)
            .map(|x| Some(json!(x)))
            .ok_or_else(|| format!("{v} is not a number")),
        "boolean" => match v {
            Value::Bool(_) => Ok(Some(v.clone())),
            Value::String(s) => match s.trim().to_lowercase().as_str() {
                "true" | "yes" | "1" => Ok(Some(json!(true))),
                "false" | "no" | "0" => Ok(Some(json!(false))),
                _ => Err(format!("{v} is not true or false")),
            },
            Value::Number(n) => Ok(Some(json!(n.as_f64() != Some(0.0)))),
            _ => Err(format!("{v} is not true or false")),
        },
        "array" => {
            let items = match v {
                Value::Array(a) => a.clone(),
                Value::String(s) => s
                    .split(',')
                    .map(|x| json!(x.trim()))
                    .filter(|x| x.as_str().is_some_and(|s| !s.is_empty()))
                    .collect(),
                x => vec![x.clone()],
            };
            let item_schema = schema.get("items").cloned().unwrap_or(json!({}));
            let mut out = Vec::new();
            for it in items {
                match coerce(key, &item_schema, &it) {
                    Ok(Some(x)) => out.push(x),
                    Ok(None) => {}
                    Err(why) => return Err(why),
                }
            }
            Ok((!out.is_empty()).then_some(Value::Array(out)))
        }
        "object" => match v {
            Value::Object(m) => {
                let props = schema.get("properties").and_then(Value::as_object);
                let mut out = Map::new();
                for (k, x) in m {
                    match props.and_then(|p| p.get(k)) {
                        Some(s) => {
                            if let Ok(Some(nx)) = coerce(k, s, x) {
                                out.insert(k.clone(), nx);
                            }
                        }
                        None if props.is_none() => {
                            out.insert(k.clone(), x.clone());
                        }
                        None => {}
                    }
                }
                Ok((!out.is_empty()).then_some(Value::Object(out)))
            }
            _ => Err(format!("{v} is not an object")),
        },
        _ => {
            // a string (the default): numbers and booleans as text, an enum matched in any case, a date made ISO
            let s = match v {
                Value::String(s) => s.trim().to_string(),
                Value::Number(n) => n.to_string(),
                Value::Bool(b) => b.to_string(),
                _ => return Err(format!("{v} is not text")),
            };
            if let Some(opts) = schema.get("enum").and_then(Value::as_array) {
                let norm = |x: &str| x.to_lowercase().replace(['-', ' '], "_");
                return opts
                    .iter()
                    .filter_map(Value::as_str)
                    .find(|o| norm(o) == norm(&s))
                    .map(|o| Some(json!(o)))
                    .ok_or_else(|| {
                        format!(
                            "\"{s}\" is not one of {}",
                            opts.iter()
                                .filter_map(Value::as_str)
                                .collect::<Vec<_>>()
                                .join(", ")
                        )
                    });
            }
            if is_date_field(key, schema) {
                return iso_date(&s)
                    .map(|d| Some(json!(d)))
                    .ok_or_else(|| format!("\"{s}\" is not a date: give YYYY-MM-DD"));
            }
            Ok(Some(json!(s)))
        }
    }
}

/// The planner's request to write a tool call's JSON arguments again: (system, user).
pub fn regen_prompt(
    tool: &str,
    schema: &Value,
    user_message: &str,
    last_answer: &str,
    rejected: &str,
    problems: &[String],
    today: &str,
) -> (String, String) {
    let system = format!(
        "You write the JSON arguments of one tool call. Answer with ONE JSON object only — no prose, no code fence — that \
matches the tool's parameters schema exactly (field names, types, enums; dates as YYYY-MM-DD, relative dates resolved \
from today {today}). Take every value from the user's words; leave out a field the user did not give."
    );
    let user = format!(
        "Tool: {tool}\nParameters schema:\n{}\n\nThe user's message:\n{}\n\nThe assistant's last answer before it:\n{}\n\n\
The arguments that were rejected:\n{}\n\nThe problems found:\n- {}",
        schema,
        take_chars(user_message, 3000),
        if last_answer.trim().is_empty() {
            "(none)"
        } else {
            take_chars(last_answer, 2000)
        },
        take_chars(rejected, 2000),
        problems.join("\n- ")
    );
    (system, user)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixed(s: &str) -> (Value, Vec<String>) {
        extract(s).unwrap_or_else(|| panic!("not repaired: {s}"))
    }

    #[test]
    fn valid_json_is_taken_as_it_is() {
        let (v, r) = fixed(r#"{"a": 1, "b": "x"}"#);
        assert_eq!((v, r.len()), (json!({"a": 1, "b": "x"}), 0));
    }

    #[test]
    fn a_fence_and_prose_around_the_object_are_removed() {
        let (v, r) = fixed("Sure, here it is:\n```json\n{\"kind\": \"flight\"}\n```\nThanks");
        assert_eq!(v, json!({"kind": "flight"}));
        assert!(r.iter().any(|x| x.contains("fence")));
        let (v, r) = fixed("call with {\"q\": \"x\"} please");
        assert_eq!(v, json!({"q": "x"}));
        assert!(r.iter().any(|x| x.contains("after")));
    }

    #[test]
    fn a_trailing_comma_is_removed() {
        let (v, r) = fixed(r#"{"a": [1, 2,], "b": 3,}"#);
        assert_eq!(v, json!({"a": [1, 2], "b": 3}));
        assert_eq!(r, vec!["trailing comma"]);
    }

    #[test]
    fn single_quotes_and_unquoted_keys_and_python_literals_are_fixed() {
        let (v, r) = fixed(r#"{kind: 'flight', nonstop: True, note: 'say "hi"', x: None}"#);
        assert_eq!(
            v,
            json!({"kind": "flight", "nonstop": true, "note": "say \"hi\"", "x": null})
        );
        for want in ["unquoted key", "single quotes", "Python literal"] {
            assert!(r.iter().any(|x| x == want), "{want} in {r:?}");
        }
    }

    #[test]
    fn a_reply_cut_short_is_closed() {
        let (v, r) = fixed(r#"{"kind": "hotel", "city": "Tok"#);
        assert_eq!(v, json!({"kind": "hotel", "city": "Tok"}));
        assert!(r.iter().any(|x| x == "an open string closed"));
        assert!(r.iter().any(|x| x == "open brackets closed"));
        let (v, _) = fixed(r#"{"airlines": ["SQ", "NH""#);
        assert_eq!(v, json!({"airlines": ["SQ", "NH"]}));
        let (v, _) = fixed(r#"{"kind": "hotel", "city":"#);
        assert_eq!(v, json!({"kind": "hotel", "city": null}));
        let (v, _) = fixed(r#"{"kind": "hotel", "#);
        assert_eq!(v, json!({"kind": "hotel"}));
    }

    #[test]
    fn absurd_precision_is_cut_to_the_value() {
        let (v, r) = fixed(r#"{"max_price": 0.036000000000000004, "n": 2}"#);
        assert_eq!(v, json!({"max_price": 0.036, "n": 2}));
        assert_eq!(r, vec!["number precision"]);
    }

    #[test]
    fn think_blocks_are_ignored() {
        let (v, _) = fixed("<think>{\"no\": 1}</think> {\"yes\": 2}");
        assert_eq!(v, json!({"yes": 2}));
    }

    #[test]
    fn no_json_at_all_is_unparsed() {
        assert!(extract("garbage").is_none());
        assert_eq!(
            parse_args(&json!("garbage")),
            json!({"_unparsed": "garbage"})
        );
        assert_eq!(parse_args(&json!("")), json!({}));
        assert_eq!(parse_args(&json!("[1]")), json!({"value": [1]}));
        let mut a = parse_args(&json!("{'a': 1}"));
        assert_eq!(take_repairs(&mut a), vec!["single quotes"]);
        assert_eq!(a, json!({"a": 1}));
    }

    fn params() -> Value {
        json!({"type": "object", "required": ["kind"], "properties": {
            "kind": {"type": "string", "enum": ["flight", "hotel"]},
            "party": {"type": "integer"}, "max_price": {"type": "number"}, "nonstop": {"type": "boolean"},
            "date": {"type": "string"}, "cabin": {"type": "string", "enum": ["economy", "premium_economy"]},
            "airlines": {"type": "array", "items": {"type": "string"}},
            "contact": {"type": "object", "properties": {"email": {"type": "string"}}}}})
    }

    #[test]
    fn values_are_fixed_to_the_schema() {
        let mut a = json!({"kind": "Flight", "party": "2", "max_price": "S$1,500", "nonstop": "yes",
                           "date": "4 December 2026", "cabin": "Premium Economy", "airlines": "SQ, NH",
                           "contact": {"email": "a@b.co", "fax": "1"}, "extra": 1});
        let c = conform(&params(), &mut a);
        assert_eq!(
            a,
            json!({"kind": "flight", "party": 2, "max_price": 1500.0, "nonstop": true, "date": "2026-12-04",
                   "cabin": "premium_economy", "airlines": ["SQ", "NH"], "contact": {"email": "a@b.co"}})
        );
        assert!(c.problems.is_empty() && !c.required_bad, "{c:?}");
        assert!(c.fixed.iter().any(|f| f.starts_with("party")));
    }

    #[test]
    fn a_value_that_cannot_be_fixed_is_dropped_and_reported() {
        let mut a = json!({"kind": "train", "party": 2.5, "date": "next Friday"});
        let c = conform(&params(), &mut a);
        assert_eq!(a, json!({}));
        assert!(c.required_bad);
        assert_eq!(c.problems.len(), 3, "{c:?}");
        let mut a = json!({"party": 2});
        let c = conform(&params(), &mut a);
        assert_eq!(c.problems, vec!["kind: required"]);
        assert!(c.required_bad);
    }

    #[test]
    fn other_date_formats_become_iso() {
        for s in [
            "2026-12-04",
            "2026/12/04",
            "04.12.2026",
            "December 4, 2026",
            "Dec 4 2026",
            "2026-12-04T10:00:00",
        ] {
            assert_eq!(iso_date(s).as_deref(), Some("2026-12-04"), "{s}");
        }
        assert_eq!(iso_date("Friday"), None);
    }
}
