//! Exact table queries for the agent — counts, sums, averages, groups, percentiles, pivots, trends — over CSV / Excel
//! files and text logs, without running model-written code (`app/common/dataq.py`).
//!
//! Why: in the 2026-10-02 round-5 test the model "read" a 40-row survey and reported 9 promoters / 19 detractors (NPS −25);
//! the file has 14 / 17 (NPS −7.5). Models can't count rows reliably, so every number that comes from a table goes through
//! this module. A query is a small JSON object (where / derive / group_by + agg / pivot / trend / sort / limit / save) applied
//! in a fixed order; expressions in derive use the safe calculator (`calc.evaluate`, see `expr.rs`).

use std::cmp::Ordering;
use std::io::Read;
use std::path::Path;
use std::sync::LazyLock;

use chrono::{Datelike, NaiveDate};
use fancy_regex::Regex as FRegex;
use indexmap::IndexMap;
use regex::Regex;
use serde_json::{json, Map, Value};

use omuse_common::util::truthy;

use crate::expr::{self, Val};
use crate::pycsv;
use crate::pyfmt::{self, py_repr, py_str, take_chars};
use crate::xlsx;

/// Most rows read from a file.
pub const MAX_ROWS: usize = 500_000;
/// Largest text file read.
pub const MAX_BYTES: usize = 60 * 1024 * 1024;
/// Rows shown by [`table`].
pub const SHOW_ROWS: usize = 60;

/// One table row: column → cell (`""` for empty, numbers as numbers).
pub type Row = Map<String, Value>;

/// The Python `DataError` (`Data`) plus the other exceptions a query can raise (`ValueError`, `TypeError`,
/// `KeyError`, `OSError`, …) which the agent reports as `ERROR: <Type>: <message>`.
#[derive(Debug, Clone, thiserror::Error)]
pub enum DataError {
    /// `dataq.DataError(msg)`.
    #[error("{0}")]
    Data(String),
    /// Any other Python exception: `kind` is the class name, `msg` its `str()`.
    #[error("{kind}: {msg}")]
    Other { kind: &'static str, msg: String },
}

impl DataError {
    fn new(msg: impl Into<String>) -> Self {
        DataError::Data(msg.into())
    }

    fn other(kind: &'static str, msg: impl Into<String>) -> Self {
        DataError::Other {
            kind,
            msg: msg.into(),
        }
    }

    /// True for the Python `DataError` (the per-query errors the agent keeps going after).
    pub fn is_data(&self) -> bool {
        matches!(self, DataError::Data(_))
    }
}

type Result<T> = std::result::Result<T, DataError>;

// ----------------------------------------------------------------------------------------------------------- loading
static NUM: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^\s*(?:S\$|US\$|HK\$|RM|[$€£¥])?\s*([-+]?(?:\d{1,3}(?:,\d{3})+|\d+)?(?:\.\d+)?(?:[eE][-+]?\d+)?)\s*(%)?\s*$").unwrap()
});
static DIGIT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\d").unwrap());
static LEADING_ZERO: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^0\d").unwrap());
static NON_WORD: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\W+").unwrap());
static DATE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(\d{4})[-/.](\d{1,2})(?:[-/.](\d{1,2}))?(?:[T\s]+(\d{1,2}):(\d{2})(?::(\d{2}))?)?")
        .unwrap()
});
static PCTL: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^p(\d{1,2}(?:\.\d+)?)$").unwrap());

/// `'1,234.5' / 'S$ 80' / '12%' → float (12% → 12.0); anything else → None`.
pub fn to_num(v: &Value) -> Option<f64> {
    match v {
        Value::Null | Value::Bool(_) => return None,
        Value::Number(n) => return n.as_f64().filter(|f| !f.is_nan()),
        _ => {}
    }
    let s = py_str(v);
    let s = s.trim();
    if s.is_empty() || s.chars().count() > 40 {
        return None;
    }
    let m = NUM.captures(s)?;
    let g = m.get(1).map(|x| x.as_str()).unwrap_or("");
    if !DIGIT.is_match(g) {
        return None;
    }
    g.replace(',', "").parse::<f64>().ok()
}

/// Python `_clean`: `None` → `""`, numeric text → number (codes like 007 stay text), floats that are whole → ints.
pub fn clean(v: &Value) -> Value {
    match v {
        Value::Null => return Value::String(String::new()),
        Value::Number(n) => {
            if let Some(f) = n.as_f64() {
                if n.is_f64() && f.fract() == 0.0 && f.abs() < 1e15 {
                    return json!(f as i64);
                }
            }
            return v.clone();
        }
        Value::Bool(_) => return v.clone(),
        _ => {}
    }
    let s = py_str(v);
    let s = s.trim();
    if let Some(n) = to_num(&Value::String(s.to_string())) {
        if !LEADING_ZERO.is_match(s.trim_start_matches(['+', '-'])) {
            // keep codes like 007 / phone numbers as text
            return if n.fract() == 0.0 && !s.contains('%') && n.abs() < 1e15 {
                json!(n as i64)
            } else {
                pyfmt::float_value(n)
            };
        }
    }
    Value::String(s.to_string())
}

fn read_text(path: &str) -> Result<String> {
    let io = |e: std::io::Error| DataError::other("OSError", e.to_string());
    let mut f = std::fs::File::open(path).map_err(io)?;
    let mut raw = Vec::new();
    f.by_ref()
        .take(MAX_BYTES as u64 + 1)
        .read_to_end(&mut raw)
        .map_err(io)?;
    if raw.len() > MAX_BYTES {
        return Err(DataError::new(format!(
            "file larger than {} MB",
            MAX_BYTES / 1024 / 1024
        )));
    }
    let body = raw.strip_prefix(b"\xef\xbb\xbf").unwrap_or(&raw);
    if let Ok(s) = std::str::from_utf8(body) {
        return Ok(s.to_string());
    }
    if let Some(s) = encoding_rs::GB18030.decode_without_bom_handling_and_without_replacement(&raw)
    {
        return Ok(s.into_owned());
    }
    Ok(raw.iter().map(|b| *b as char).collect())
}

fn dedupe(cols: &[String]) -> Vec<String> {
    let mut out = Vec::new();
    let mut seen: IndexMap<String, usize> = IndexMap::new();
    for (i, c) in cols.iter().enumerate() {
        let mut c = c.trim().to_string();
        if c.is_empty() {
            c = format!("col{}", i + 1);
        }
        if let Some(n) = seen.get_mut(&c) {
            *n += 1;
            c = format!("{}_{}", c, n);
        } else {
            seen.insert(c.clone(), 1);
        }
        out.push(c);
    }
    out
}

/// Read a table. CSV/TSV (delimiter sniffed), .xlsx (first sheet or `sheet`), or any text file with `pattern`
/// (a regex with named groups: each matching line becomes a row).
pub fn load(
    path: &str,
    sheet: Option<&str>,
    pattern: Option<&str>,
    header: bool,
) -> Result<(Vec<String>, Vec<Row>)> {
    let low = path.to_lowercase();
    if let Some(pattern) = pattern.filter(|p| !p.is_empty()) {
        let rx = FRegex::new(pattern).map_err(|e| DataError::new(format!("bad pattern: {e}")))?;
        let cols: Vec<String> = rx
            .capture_names()
            .flatten()
            .map(|s| s.to_string())
            .collect();
        if cols.is_empty() {
            return Err(DataError::new(
                "pattern needs named groups, e.g. (?P<time>\\S+) (?P<status>\\d{3})",
            ));
        }
        let text = read_text(path)?;
        let mut rows = Vec::new();
        let mut skipped = 0;
        for line in pyfmt::splitlines(&text) {
            let m = match rx.captures(line) {
                Ok(Some(m)) => m,
                _ => {
                    if !line.trim().is_empty() {
                        skipped += 1;
                    }
                    continue;
                }
            };
            let mut row = Row::new();
            for c in &cols {
                let v = m
                    .name(c)
                    .map(|g| Value::String(g.as_str().to_string()))
                    .unwrap_or(Value::Null);
                row.insert(c.clone(), clean(&v));
            }
            rows.push(row);
            if rows.len() >= MAX_ROWS {
                break;
            }
        }
        if rows.is_empty() {
            return Err(DataError::new(format!(
                "pattern matched no lines ({skipped} non-empty lines did not match)"
            )));
        }
        return Ok((cols, rows));
    }
    let grid: Vec<Vec<Value>> = if low.ends_with(".xlsx") || low.ends_with(".xlsm") {
        xlsx::read_grid(path, sheet.filter(|s| !s.is_empty()), MAX_ROWS).map_err(DataError::new)?
    } else if low.ends_with(".csv") || low.ends_with(".tsv") || low.ends_with(".txt") {
        let text = read_text(path)?;
        let sample = take_chars(&text, 20000);
        let dialect = pycsv::sniff(sample, ",\t;|").unwrap_or_else(|| {
            if low.ends_with(".tsv") {
                pycsv::Dialect::excel_tab()
            } else {
                pycsv::Dialect::excel()
            }
        });
        pycsv::read_all(&text, &dialect)
            .into_iter()
            .filter(|r| r.iter().any(|c| !c.trim().is_empty()))
            .take(MAX_ROWS + 1)
            .map(|r| r.into_iter().map(Value::String).collect())
            .collect()
    } else {
        return Err(DataError::new(
            "supported: .csv .tsv .txt .xlsx (for logs or other text pass a regex `pattern`)",
        ));
    };
    if grid.is_empty() {
        return Err(DataError::new("the table is empty"));
    }
    let width = grid.iter().map(|r| r.len()).max().unwrap_or(0);
    let (cols, body): (Vec<String>, &[Vec<Value>]) = if header {
        let mut names: Vec<String> = grid[0]
            .iter()
            .map(|c| {
                if c.is_null() {
                    String::new()
                } else {
                    py_str(c)
                }
            })
            .collect();
        names.resize(width, String::new());
        (dedupe(&names), &grid[1..])
    } else {
        (
            (0..width).map(|i| format!("col{}", i + 1)).collect(),
            &grid[..],
        )
    };
    let rows = body
        .iter()
        .map(|r| {
            cols.iter()
                .enumerate()
                .map(|(i, c)| (c.clone(), clean(r.get(i).unwrap_or(&Value::Null))))
                .collect()
        })
        .collect();
    Ok((cols, rows))
}

// -------------------------------------------------------------------------------------------------------- operations
/// A column name as an identifier for expressions (`金额(新元)` → `金额_新元`).
pub fn alias(col: &str) -> String {
    let a = NON_WORD.replace_all(col, "_");
    let a = a.trim_matches('_');
    let a = if a.is_empty() { "c" } else { a };
    if a.chars().next().map(|c| c.is_numeric()).unwrap_or(false) {
        format!("c_{a}")
    } else {
        a.to_string()
    }
}

fn col(cols: &[String], name: &Value) -> Result<String> {
    let n = if truthy(name) {
        py_str(name).trim().to_string()
    } else {
        String::new()
    };
    if cols.contains(&n) {
        return Ok(n);
    }
    let nl = n.to_lowercase();
    for c in cols {
        if alias(c) == n
            || c.to_lowercase() == nl
            || alias(c).to_lowercase() == alias(&n).to_lowercase()
        {
            return Ok(c.clone());
        }
    }
    Err(DataError::new(format!(
        "no column {}; columns: {}",
        py_repr(name),
        py_repr(&json!(cols))
    )))
}

fn col_str(cols: &[String], name: &str) -> Result<String> {
    col(cols, &Value::String(name.to_string()))
}

fn cell<'a>(r: &'a Row, c: &str) -> &'a Value {
    r.get(c).unwrap_or(&Value::Null)
}

fn is_empty(v: &Value) -> bool {
    matches!(v, Value::Null) || matches!(v, Value::String(s) if s.is_empty())
}

fn eq(a: &Value, b: &Value) -> bool {
    if let (Some(x), Some(y)) = (to_num(a), to_num(b)) {
        return (x - y).abs() < 1e-9;
    }
    py_str(a).trim().to_lowercase() == py_str(b).trim().to_lowercase()
}

fn cmp(a: &Value, op: &str, b: &Value) -> Result<bool> {
    let op = op.trim().to_lowercase();
    match op.as_str() {
        "empty" | "is_empty" => return Ok(is_empty(a)),
        "not_empty" | "notempty" => return Ok(!is_empty(a)),
        "in" | "not_in" => {
            let hit = match b {
                Value::Array(vals) => vals.iter().any(|v| eq(a, v)),
                v => eq(a, v),
            };
            return Ok(if op == "in" { hit } else { !hit });
        }
        "between" => {
            let Value::Array(lh) = b else {
                return Err(DataError::new("between needs [low, high]"));
            };
            if lh.len() != 2 {
                return Err(DataError::new("between needs [low, high]"));
            }
            return Ok(cmp(a, ">=", &lh[0])? && cmp(a, "<=", &lh[1])?);
        }
        "contains" | "not_contains" => {
            let hit = py_str(a).to_lowercase().contains(&py_str(b).to_lowercase());
            return Ok(if op == "contains" { hit } else { !hit });
        }
        "startswith" => return Ok(py_str(a).starts_with(&py_str(b))),
        "regex" => {
            let rx =
                FRegex::new(&py_str(b)).map_err(|e| DataError::new(format!("bad regex: {e}")))?;
            return Ok(rx.is_match(&py_str(a)).unwrap_or(false));
        }
        "==" | "=" | "eq" => return Ok(eq(a, b)),
        "!=" | "<>" | "ne" => return Ok(!eq(a, b)),
        _ => {}
    }
    let ord = match (to_num(a), to_num(b)) {
        (Some(x), Some(y)) => x.partial_cmp(&y).unwrap_or(Ordering::Equal),
        _ => py_str(a).cmp(&py_str(b)), // dates / text compare as strings (ISO dates sort correctly)
    };
    match op.as_str() {
        ">" | "gt" => Ok(ord == Ordering::Greater),
        ">=" | "ge" => Ok(ord != Ordering::Less),
        "<" | "lt" => Ok(ord == Ordering::Less),
        "<=" | "le" => Ok(ord != Ordering::Greater),
        _ => Err(DataError::new(format!(
            "unknown op {}",
            py_repr(&Value::String(op))
        ))),
    }
}

fn as_list(v: &Value) -> Vec<Value> {
    match v {
        Value::Array(a) => a.clone(),
        other => vec![other.clone()],
    }
}

/// Conditions are ANDed (any_of: ORed). A value that names a column — or {"col": name} — compares the two columns
/// row by row (e.g. 金额 > 预算).
fn where_(cols: &[String], rows: Vec<Row>, conds: &Value, any_of: bool) -> Result<Vec<Row>> {
    let mut cc: Vec<(String, String, Value, Option<String>)> = Vec::new();
    for c in as_list(conds) {
        let Value::Object(c) = c else {
            return Err(DataError::new(
                "each condition is an object {col, op, value}",
            ));
        };
        let v = c.get("value").cloned().unwrap_or(Value::Null);
        let mut other = None;
        if let Value::Object(o) = &v {
            if o.get("col").map(truthy).unwrap_or(false) {
                other = Some(col(cols, &o["col"])?);
            }
        } else if let Value::String(s) = &v {
            if cols.contains(s) && c.get("col") != Some(&v) {
                other = Some(s.clone());
            }
        }
        let op = match c.get("op") {
            Some(o) if truthy(o) => py_str(o),
            _ => "==".to_string(),
        };
        cc.push((
            col(cols, c.get("col").unwrap_or(&Value::Null))?,
            op,
            v,
            other,
        ));
    }
    let mut out = Vec::new();
    for r in rows {
        let mut keep = !any_of;
        for (c, op, v, o) in &cc {
            let rhs = match o {
                Some(o) => cell(&r, o),
                None => v,
            };
            let hit = cmp(cell(&r, c), op, rhs)?;
            if any_of && hit {
                keep = true;
                break;
            }
            if !any_of && !hit {
                keep = false;
                break;
            }
        }
        if keep {
            out.push(r);
        }
    }
    Ok(out)
}

fn date_part(v: &Value, part: &str) -> Result<Value> {
    let s = py_str(v);
    let Some(m) = DATE.captures(&s) else {
        return Ok(json!(""));
    };
    let y: i32 = m[1].parse().unwrap_or(0);
    let mo: u32 = m[2].parse().unwrap_or(0);
    let d: u32 = m
        .get(3)
        .map(|g| g.as_str().parse().unwrap_or(1))
        .unwrap_or(1);
    let h = m.get(4).map(|g| g.as_str().to_string());
    Ok(match part {
        "year" => json!(y),
        "month" => json!(format!("{y:04}-{mo:02}")),
        "quarter" => json!(format!("{y}-Q{}", (mo as i64 - 1).div_euclid(3) + 1)),
        "date" => json!(format!("{y:04}-{mo:02}-{d:02}")),
        "hour" => match h {
            Some(h) => json!(h.parse::<i64>().unwrap_or(0)),
            None => json!(""),
        },
        "weekday" => match NaiveDate::from_ymd_opt(y, mo, d) {
            Some(dt) => json!(dt.format("%a").to_string()),
            None => json!(""),
        },
        "week" => match NaiveDate::from_ymd_opt(y, mo, d) {
            Some(dt) => {
                let iw = dt.iso_week();
                json!(format!("{}-W{:02}", iw.year(), iw.week()))
            }
            None => json!(""),
        },
        _ => {
            return Err(DataError::new(
                "date_part: year / quarter / month / week / date / weekday / hour",
            ))
        }
    })
}

/// Python `int(v)`.
fn py_int(v: &Value) -> Result<i64> {
    match v {
        Value::Number(n) => Ok(n
            .as_i64()
            .or_else(|| n.as_f64().map(|f| f.trunc() as i64))
            .unwrap_or(0)),
        Value::Bool(b) => Ok(*b as i64),
        Value::String(s) => s.trim().parse::<i64>().map_err(|_| {
            DataError::other(
                "ValueError",
                format!("invalid literal for int() with base 10: {}", py_repr(v)),
            )
        }),
        other => Err(DataError::other(
            "TypeError",
            format!(
                "int() argument must be a string, a bytes-like object or a real number, not '{}'",
                pyfmt::py_type(other)
            ),
        )),
    }
}

/// Python `float(v)`.
fn py_float(v: &Value) -> Result<f64> {
    match v {
        Value::Number(n) => Ok(n.as_f64().unwrap_or(0.0)),
        Value::Bool(b) => Ok(if *b { 1.0 } else { 0.0 }),
        Value::String(s) => s.trim().parse::<f64>().map_err(|_| {
            DataError::other(
                "ValueError",
                format!("could not convert string to float: {}", py_repr(v)),
            )
        }),
        other => Err(DataError::other(
            "TypeError",
            format!(
                "float() argument must be a string or a real number, not '{}'",
                pyfmt::py_type(other)
            ),
        )),
    }
}

/// Python `s[a:b]` with `None` / negative indices.
fn py_slice(s: &str, a: Option<i64>, b: Option<i64>) -> String {
    let chars: Vec<char> = s.chars().collect();
    let n = chars.len() as i64;
    let norm = |i: Option<i64>, default: i64| -> i64 {
        match i {
            None => default,
            Some(i) if i < 0 => (i + n).max(0),
            Some(i) => i.min(n),
        }
    };
    let (lo, hi) = (norm(a, 0), norm(b, n));
    if hi <= lo {
        return String::new();
    }
    chars[lo as usize..hi as usize].iter().collect()
}

fn slice_index(v: &Value) -> Result<Option<i64>> {
    match v {
        Value::Null => Ok(None),
        Value::Number(n) if n.as_i64().is_some() => Ok(n.as_i64()),
        Value::Bool(b) => Ok(Some(*b as i64)),
        _ => Err(DataError::other(
            "TypeError",
            "slice indices must be integers or None or have an __index__ method",
        )),
    }
}

fn obj<'a>(v: &'a Value, what: &str) -> Result<&'a Map<String, Value>> {
    match v {
        Value::Object(o) => Ok(o),
        other => Err(DataError::other(
            "AttributeError",
            format!(
                "'{}' object has no attribute '{}'",
                pyfmt::py_type(other),
                what
            ),
        )),
    }
}

/// A hashable Python-equality key for group / pivot dictionaries (`1 == 1.0 == True`).
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
enum Key {
    Num(u64),
    Str(String),
    Null,
}

fn key_of(v: &Value) -> Key {
    match v {
        Value::Null => Key::Null,
        Value::Bool(b) => Key::Num((if *b { 1.0f64 } else { 0.0 }).to_bits()),
        Value::Number(n) => {
            let f = n.as_f64().unwrap_or(0.0);
            Key::Num(if f == 0.0 {
                0.0f64.to_bits()
            } else {
                f.to_bits()
            })
        }
        Value::String(s) => Key::Str(s.clone()),
        other => Key::Str(py_repr(other)),
    }
}

/// Rows grouped by the values of `by`, in first-seen order: key → (first key values, row indexes).
fn group_rows(rows: &[Row], by: &[String]) -> IndexMap<Vec<Key>, (Vec<Value>, Vec<usize>)> {
    let mut groups: IndexMap<Vec<Key>, (Vec<Value>, Vec<usize>)> = IndexMap::new();
    for (i, r) in rows.iter().enumerate() {
        let k: Vec<Key> = by.iter().map(|b| key_of(cell(r, b))).collect();
        groups
            .entry(k)
            .or_insert_with(|| (by.iter().map(|b| cell(r, b).clone()).collect(), Vec::new()))
            .1
            .push(i);
    }
    groups
}

fn val_to_value(v: Val) -> Value {
    match v {
        Val::Int(i) => json!(i),
        Val::Float(f) => pyfmt::float_value(f),
        Val::Bool(b) => json!(b as i64),
        Val::List(l) => Value::Array(l.into_iter().map(val_to_value).collect()),
    }
}

fn derive(cols: &mut Vec<String>, rows: &mut [Row], specs: &Value) -> Result<()> {
    for s in as_list(specs) {
        let s = obj(&s, "get")?;
        let name = match s.get("as") {
            Some(v) if truthy(v) => py_str(v).trim().to_string(),
            _ => String::new(),
        };
        if name.is_empty() {
            return Err(DataError::new("derive needs \"as\" (the new column name)"));
        }
        if let Some(e) = s.get("expr") {
            let mut names: IndexMap<String, String> = IndexMap::new();
            for c in cols.iter() {
                names.insert(alias(c), c.clone());
            }
            let mut expr = py_str(e);
            let mut sorted_cols: Vec<&String> = cols.iter().collect();
            sorted_cols.sort_by_key(|c| std::cmp::Reverse(c.chars().count()));
            for c in sorted_cols {
                // 金额(新元) - 预算(新元) → 金额_新元 - 预算_新元
                let a = alias(c);
                if *c != a && expr.contains(c.as_str()) {
                    expr = expr.replace(c.as_str(), &a);
                }
            }
            let shown: Vec<Value> = names
                .keys()
                .take(20)
                .map(|k| Value::String(k.clone()))
                .collect();
            for r in rows.iter_mut() {
                let mut env: IndexMap<String, f64> = IndexMap::new();
                for (a, c) in &names {
                    env.insert(a.clone(), to_num(cell(r, c)).unwrap_or(0.0));
                }
                let v = expr::evaluate(&expr, &env).map_err(|e| {
                    DataError::new(format!(
                        "derive {name}: {e} (columns are referred to as {})",
                        py_repr(&Value::Array(shown.clone()))
                    ))
                })?;
                r.insert(name.clone(), val_to_value(v));
            }
        } else if let Some(b) = s.get("bins") {
            let src = col(cols, s.get("from").unwrap_or(&Value::Null))?;
            let edges: Vec<f64> = pyfmt::py_iter(b)
                .map_err(|m| DataError::other("TypeError", m))?
                .iter()
                .map(py_float)
                .collect::<Result<_>>()?;
            let labels: Vec<Value> = match s.get("labels") {
                Some(l) if truthy(l) => {
                    pyfmt::py_iter(l).map_err(|m| DataError::other("TypeError", m))?
                }
                _ => (0..edges.len().saturating_sub(1))
                    .map(|i| {
                        json!(format!(
                            "{}–{}",
                            pyfmt::general(edges[i], 6),
                            pyfmt::general(edges[i + 1], 6)
                        ))
                    })
                    .collect(),
            };
            if labels.len() + 1 != edges.len() {
                return Err(DataError::new("bins: labels must be one fewer than edges"));
            }
            let right = s.get("right").map(truthy).unwrap_or(false); // default: [low, high) ; right=true: (low, high]
            for r in rows.iter_mut() {
                let mut lab = json!("");
                if let Some(x) = to_num(cell(r, &src)) {
                    for i in 0..edges.len().saturating_sub(1) {
                        let (lo, hi) = (edges[i], edges[i + 1]);
                        if if right {
                            lo < x && x <= hi
                        } else {
                            lo <= x && x < hi
                        } {
                            lab = labels[i].clone();
                            break;
                        }
                    }
                }
                r.insert(name.clone(), lab);
            }
        } else if let Some(m) = s.get("map") {
            let src = col(cols, s.get("from").unwrap_or(&Value::Null))?;
            let mp = match m {
                Value::Object(o) => o.clone(),
                other => {
                    return Err(DataError::other(
                        "TypeError",
                        format!("'{}' object is not iterable", pyfmt::py_type(other)),
                    ))
                }
            };
            let keep = s.get("keep").map(truthy).unwrap_or(true);
            let default = s.get("default").cloned().unwrap_or(json!(""));
            for r in rows.iter_mut() {
                let cur = cell(r, &src).clone();
                let v = match mp.get(&py_str(&cur)) {
                    Some(v) => v.clone(),
                    None => {
                        if keep {
                            cur
                        } else {
                            default.clone()
                        }
                    }
                };
                r.insert(name.clone(), v);
            }
        } else if let Some(part) = s.get("date_part") {
            let src = col(cols, s.get("from").unwrap_or(&Value::Null))?;
            let part = py_str(part);
            for r in rows.iter_mut() {
                let v = date_part(cell(r, &src), &part)?;
                r.insert(name.clone(), v);
            }
        } else if let Some(sl) = s.get("slice") {
            let src = col(cols, s.get("from").unwrap_or(&Value::Null))?;
            let items = pyfmt::py_iter(sl).map_err(|m| DataError::other("TypeError", m))?;
            let a = slice_index(items.first().unwrap_or(&Value::Null))?;
            let b = slice_index(items.get(1).unwrap_or(&Value::Null))?;
            for r in rows.iter_mut() {
                let v = py_slice(&py_str(cell(r, &src)), a, b);
                r.insert(name.clone(), json!(v));
            }
        } else if let Some(kind) = ["zscore", "pct_change", "rank", "cumsum"]
            .iter()
            .find(|k| s.contains_key(**k))
        {
            let src = col(cols, &s[*kind])?;
            let by: Vec<String> = match s.get("by") {
                Some(b) if truthy(b) => pyfmt::py_iter(b)
                    .map_err(|m| DataError::other("TypeError", m))?
                    .iter()
                    .map(|b| col(cols, b))
                    .collect::<Result<_>>()?,
                _ => Vec::new(),
            };
            let groups = group_rows(rows, &by);
            for (_, (_, idx)) in groups {
                let xs: Vec<Option<f64>> =
                    idx.iter().map(|i| to_num(cell(&rows[*i], &src))).collect();
                match *kind {
                    "zscore" => {
                        let vals: Vec<f64> = xs.iter().flatten().copied().collect();
                        let mu = if vals.is_empty() {
                            0.0
                        } else {
                            pyfmt::fmean(&vals)
                        };
                        let sd = if vals.len() > 1 {
                            pyfmt::stdev(&vals)
                        } else {
                            0.0
                        };
                        for (i, x) in idx.iter().zip(&xs) {
                            let v = match x {
                                Some(x) if sd != 0.0 => {
                                    pyfmt::float_value(pyfmt::round_to((x - mu) / sd, 3))
                                }
                                _ => json!(""),
                            };
                            rows[*i].insert(name.clone(), v);
                        }
                    }
                    "pct_change" => {
                        let mut prev: Option<f64> = None;
                        for (i, x) in idx.iter().zip(&xs) {
                            let v = match (x, prev) {
                                (Some(x), Some(p)) if p != 0.0 => pyfmt::float_value(
                                    pyfmt::round_to((x - p) / p.abs() * 100.0, 2),
                                ),
                                _ => json!(""),
                            };
                            rows[*i].insert(name.clone(), v);
                            if x.is_some() {
                                prev = *x;
                            }
                        }
                    }
                    "cumsum" => {
                        let mut tot = 0.0;
                        for (i, x) in idx.iter().zip(&xs) {
                            tot += x.unwrap_or(0.0);
                            rows[*i].insert(name.clone(), pyfmt::float_value(tot));
                        }
                    }
                    _ => {
                        let mut order: Vec<usize> = (0..idx.len()).collect();
                        order.sort_by(|a, b| {
                            let ka = -xs[*a].unwrap_or(f64::NEG_INFINITY);
                            let kb = -xs[*b].unwrap_or(f64::NEG_INFINITY);
                            ka.partial_cmp(&kb).unwrap_or(Ordering::Equal)
                        });
                        for (k, i) in order.iter().enumerate() {
                            rows[idx[*i]].insert(name.clone(), json!(k + 1));
                        }
                    }
                }
            }
        } else {
            return Err(DataError::new("derive needs one of: expr, bins, map, date_part, slice, zscore, pct_change, rank, cumsum"));
        }
        if !cols.contains(&name) {
            cols.push(name);
        }
    }
    Ok(())
}

fn pct(vals: &[f64], p: f64) -> f64 {
    let mut s = vals.to_vec();
    s.sort_by(|a, b| a.partial_cmp(b).unwrap_or(Ordering::Equal));
    if s.is_empty() {
        return f64::NAN;
    }
    let k = (s.len() - 1) as f64 * p / 100.0;
    let (f, c) = (k.floor(), k.ceil());
    if f == c {
        s[f as usize]
    } else {
        s[f as usize] + (s[c as usize] - s[f as usize]) * (k - f)
    }
}

fn nonempty(vals: &[Value]) -> impl Iterator<Item = &Value> {
    vals.iter().filter(|v| !is_empty(v))
}

fn agg(vals_raw: &[Value], fn_name: &str, total: Option<f64>) -> Result<Value> {
    let f = fn_name.trim().to_lowercase();
    let nums: Vec<f64> = vals_raw.iter().filter_map(to_num).collect();
    match f.as_str() {
        "count" => return Ok(json!(vals_raw.len())),
        "count_nonempty" | "nonempty" => return Ok(json!(nonempty(vals_raw).count())),
        "count_distinct" | "nunique" | "distinct" => {
            let set: std::collections::HashSet<String> = nonempty(vals_raw).map(py_str).collect();
            return Ok(json!(set.len()));
        }
        "mode" => {
            let mut counts: IndexMap<String, usize> = IndexMap::new();
            for v in nonempty(vals_raw) {
                *counts.entry(py_str(v)).or_insert(0) += 1;
            }
            let best = counts
                .iter()
                .fold(None::<(&String, usize)>, |b, (k, n)| match b {
                    Some((_, bn)) if bn >= *n => b,
                    _ => Some((k, *n)),
                });
            return Ok(json!(best.map(|(k, _)| k.clone()).unwrap_or_default()));
        }
        "first" | "last" => {
            return Ok(if vals_raw.is_empty() {
                json!("")
            } else if f == "first" {
                vals_raw[0].clone()
            } else {
                vals_raw[vals_raw.len() - 1].clone()
            });
        }
        "list" | "values" => {
            let mut seen: IndexMap<String, ()> = IndexMap::new();
            for v in nonempty(vals_raw) {
                seen.entry(py_str(v)).or_insert(());
            }
            let joined = seen.keys().cloned().collect::<Vec<_>>().join(", ");
            return Ok(json!(take_chars(&joined, 300)));
        }
        _ => {}
    }
    if nums.is_empty() {
        return Ok(json!(""));
    }
    let fv = pyfmt::float_value;
    let sum: f64 = nums.iter().fold(0.0, |a, b| a + b);
    let maxv = nums.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
    let minv = nums.iter().cloned().fold(f64::INFINITY, f64::min);
    Ok(match f.as_str() {
        "sum" => fv(sum),
        "mean" | "avg" | "average" => fv(pyfmt::fmean(&nums)),
        "median" => fv(pyfmt::median(&nums)),
        "min" => fv(minv),
        "max" => fv(maxv),
        "std" | "stdev" => fv(if nums.len() > 1 {
            pyfmt::stdev(&nums)
        } else {
            0.0
        }),
        "pstd" | "pstdev" => fv(pyfmt::pstdev(&nums)),
        "var" | "variance" => fv(if nums.len() > 1 {
            pyfmt::variance(&nums)
        } else {
            0.0
        }),
        "range" => fv(maxv - minv),
        "share" | "sum_share" => match total {
            Some(t) if t != 0.0 => fv(sum / t * 100.0),
            _ => json!(""),
        },
        _ => {
            if let Some(m) = PCTL.captures(&f) {
                let p: f64 = m[1].parse().unwrap_or(0.0);
                return Ok(fv(pct(&nums, p)));
            }
            return Err(DataError::new(format!(
                "unknown agg fn {} (count, count_distinct, sum, mean, median, min, max, std, var, range, p50/p90/p95/p99, share, count_share, mode, first, last, list)",
                py_repr(&Value::String(f))
            )));
        }
    })
}

fn group(
    cols: &[String],
    rows: &[Row],
    by: &Value,
    aggs: &Value,
) -> Result<(Vec<String>, Vec<Row>)> {
    let by: Vec<String> = match by {
        Value::Array(a) => a.iter().map(|b| col(cols, b)).collect::<Result<_>>()?,
        b if truthy(b) => vec![col(cols, b)?],
        _ => Vec::new(),
    };
    let mut aggs = as_list(aggs);
    if aggs.is_empty() {
        aggs = vec![json!({"fn": "count"})];
    }
    let mut plan: Vec<(Option<String>, String, String)> = Vec::new();
    for a in &aggs {
        let a = obj(a, "get")?;
        let f = match a.get("fn") {
            Some(v) if truthy(v) => py_str(v),
            _ => "count".to_string(),
        };
        let c = match a.get("col") {
            Some(v) if !is_empty(v) && *v != json!("*") => Some(col(cols, v)?),
            _ => None,
        };
        if c.is_none() && !matches!(f.to_lowercase().as_str(), "count" | "count_share") {
            return Err(DataError::new(format!("agg {f} needs \"col\"")));
        }
        let name = match a.get("as") {
            Some(v) if truthy(v) => py_str(v),
            _ => match &c {
                Some(c) => format!("{f}({c})"),
                None => f.clone(),
            },
        };
        plan.push((c, f, name));
    }
    let groups = group_rows(rows, &by);
    let mut totals: IndexMap<String, f64> = IndexMap::new();
    for (c, f, _) in &plan {
        if let Some(c) = c {
            if matches!(f.to_lowercase().as_str(), "share" | "sum_share") {
                let t = rows
                    .iter()
                    .filter_map(|r| to_num(cell(r, c)))
                    .fold(0.0, |a, b| a + b);
                totals.insert(c.clone(), t);
            }
        }
    }
    let mut out_cols: Vec<String> = by.clone();
    out_cols.extend(plan.iter().map(|p| p.2.clone()));
    let mut out = Vec::new();
    for (_, (keyvals, idx)) in groups {
        let mut o = Row::new();
        for (b, v) in by.iter().zip(keyvals) {
            o.insert(b.clone(), v);
        }
        for (c, f, name) in &plan {
            let v = if f.to_lowercase() == "count_share" {
                if rows.is_empty() {
                    json!("")
                } else {
                    pyfmt::float_value(idx.len() as f64 / rows.len() as f64 * 100.0)
                }
            } else {
                match c {
                    Some(c) => {
                        let vals: Vec<Value> =
                            idx.iter().map(|i| cell(&rows[*i], c).clone()).collect();
                        agg(&vals, f, totals.get(c).copied())?
                    }
                    None => json!(idx.len()),
                }
            };
            o.insert(name.clone(), v);
        }
        out.push(o);
    }
    Ok((out_cols, out))
}

fn pivot(cols: &[String], rows: &[Row], spec: &Value) -> Result<(Vec<String>, Vec<Row>)> {
    let spec = obj(spec, "get")?;
    let rc = col(cols, spec.get("rows").unwrap_or(&Value::Null))?;
    let cc = col(cols, spec.get("cols").unwrap_or(&Value::Null))?;
    let vc = match spec.get("value") {
        Some(v) if truthy(v) => Some(col(cols, v)?),
        _ => None,
    };
    let f = match spec.get("fn") {
        Some(v) if truthy(v) => py_str(v),
        _ => if vc.is_some() { "sum" } else { "count" }.to_string(),
    };
    let uniq = |c: &str| -> Vec<Value> {
        let mut seen: IndexMap<Key, Value> = IndexMap::new();
        for r in rows {
            let v = cell(r, c);
            seen.entry(key_of(v)).or_insert_with(|| v.clone());
        }
        seen.into_values().collect()
    };
    let (mut rkeys, mut ckeys) = (uniq(&rc), uniq(&cc));
    if spec.get("sort").map(truthy).unwrap_or(true) {
        rkeys.sort_by_key(py_str);
        ckeys.sort_by_key(py_str);
    }
    let mut cells: IndexMap<(Key, Key), Vec<Value>> = IndexMap::new();
    for r in rows {
        let v = match &vc {
            Some(vc) => cell(r, vc).clone(),
            None => json!(1),
        };
        cells
            .entry((key_of(cell(r, &rc)), key_of(cell(r, &cc))))
            .or_default()
            .push(v);
    }
    let totals = spec.get("totals").map(truthy).unwrap_or(true);
    let mut out_cols = vec![rc.clone()];
    out_cols.extend(ckeys.iter().map(py_str));
    if totals {
        out_cols.push("Total".to_string());
    }
    let mut out = Vec::new();
    for rk in &rkeys {
        let mut o = Row::new();
        o.insert(rc.clone(), rk.clone());
        let mut allv: Vec<Value> = Vec::new();
        for ck in &ckeys {
            let vs = cells
                .get(&(key_of(rk), key_of(ck)))
                .cloned()
                .unwrap_or_default();
            allv.extend(vs.iter().cloned());
            let v = if !vs.is_empty() {
                agg(&vs, &f, None)?
            } else if f == "sum" || f == "count" {
                json!(0)
            } else {
                json!("")
            };
            o.insert(py_str(ck), v);
        }
        if totals {
            o.insert("Total".to_string(), agg(&allv, &f, None)?);
        }
        out.push(o);
    }
    if totals {
        let mut t = Row::new();
        t.insert(rc.clone(), json!("Total"));
        for ck in &ckeys {
            let kc = key_of(ck);
            let vs: Vec<Value> = cells
                .iter()
                .filter(|((_, b), _)| *b == kc)
                .flat_map(|(_, vs)| vs.iter().cloned())
                .collect();
            t.insert(py_str(ck), agg(&vs, &f, None)?);
        }
        let all: Vec<Value> = cells.values().flat_map(|vs| vs.iter().cloned()).collect();
        t.insert("Total".to_string(), agg(&all, &f, None)?);
        out.push(t);
    }
    Ok((out_cols, out))
}

/// Least-squares line y = a + b·x per group; x defaults to the row order 1..n (e.g. months in order).
fn trend(cols: &[String], rows: &[Row], spec: &Value) -> Result<(Vec<String>, Vec<Row>)> {
    let spec = obj(spec, "get")?;
    let yc = col(
        cols,
        spec.get("y")
            .ok_or_else(|| DataError::other("KeyError", "'y'"))?,
    )?;
    let xc = match spec.get("x") {
        Some(v) if truthy(v) => Some(col(cols, v)?),
        _ => None,
    };
    let by: Vec<String> = match spec.get("by") {
        Some(b) if truthy(b) => pyfmt::py_iter(b)
            .map_err(|m| DataError::other("TypeError", m))?
            .iter()
            .map(|b| col(cols, b))
            .collect::<Result<_>>()?,
        _ => Vec::new(),
    };
    let ahead = match spec.get("ahead") {
        Some(v) if truthy(v) => py_int(v)?,
        Some(_) => 0,
        None => 1,
    }
    .clamp(0, 24) as usize;
    let groups = group_rows(rows, &by);
    let mut out_cols = by.clone();
    out_cols.extend(
        ["n", "slope", "intercept", "r2", "last_y"]
            .iter()
            .map(|s| s.to_string()),
    );
    out_cols.extend((1..=ahead).map(|i| format!("forecast_{i}")));
    let mut out = Vec::new();
    for (_, (keyvals, idx)) in groups {
        let mut pts: Vec<(f64, f64)> = Vec::new();
        for (n, i) in idx.iter().enumerate() {
            let r = &rows[*i];
            let x = match &xc {
                Some(xc) => to_num(cell(r, xc)),
                None => Some((n + 1) as f64),
            };
            let y = to_num(cell(r, &yc));
            if let (Some(x), Some(y)) = (x, y) {
                pts.push((x, y));
            }
        }
        let mut o = Row::new();
        for (b, v) in by.iter().zip(keyvals) {
            o.insert(b.clone(), v);
        }
        o.insert("n".to_string(), json!(pts.len()));
        if pts.len() < 2 {
            out.push(o);
            continue;
        }
        let mx = pyfmt::fmean(&pts.iter().map(|p| p.0).collect::<Vec<_>>());
        let my = pyfmt::fmean(&pts.iter().map(|p| p.1).collect::<Vec<_>>());
        let sxx = pts.iter().fold(0.0, |a, p| a + (p.0 - mx).powi(2));
        let b = if sxx != 0.0 {
            pts.iter().fold(0.0, |a, p| a + (p.0 - mx) * (p.1 - my)) / sxx
        } else {
            0.0
        };
        let a = my - b * mx;
        let ss_tot = pts.iter().fold(0.0, |s, p| s + (p.1 - my).powi(2));
        let ss_res = pts
            .iter()
            .fold(0.0, |s, p| s + (p.1 - (a + b * p.0)).powi(2));
        let fv = pyfmt::float_value;
        o.insert("slope".to_string(), fv(b));
        o.insert("intercept".to_string(), fv(a));
        o.insert(
            "r2".to_string(),
            fv(if ss_tot != 0.0 {
                1.0 - ss_res / ss_tot
            } else {
                1.0
            }),
        );
        o.insert("last_y".to_string(), fv(pts[pts.len() - 1].1));
        let step = if pts.len() > 1 {
            (pts[pts.len() - 1].0 - pts[0].0) / (pts.len() - 1) as f64
        } else {
            1.0
        };
        for i in 1..=ahead {
            o.insert(
                format!("forecast_{i}"),
                fv(a + b * (pts[pts.len() - 1].0 + step * i as f64)),
            );
        }
        out.push(o);
    }
    Ok((out_cols, out))
}

fn sort(cols: &[String], rows: &mut [Row], spec: &Value) -> Result<()> {
    for s in as_list(spec).iter().rev() {
        let (c, desc) = match s {
            Value::String(s) => (
                col_str(cols, s.trim_start_matches('-'))?,
                s.starts_with('-'),
            ),
            other => {
                let o = obj(other, "get")?;
                let c = col(
                    cols,
                    o.get("col")
                        .ok_or_else(|| DataError::other("KeyError", "'col'"))?,
                )?;
                (c, o.get("desc").map(truthy).unwrap_or(false))
            }
        };
        let num = rows
            .iter()
            .filter(|r| !is_empty(cell(r, &c)))
            .all(|r| to_num(cell(r, &c)).is_some());
        if num {
            let sign = if desc { -1.0 } else { 1.0 };
            rows.sort_by(|a, b| {
                let ka = (
                    is_empty(cell(a, &c)),
                    sign * to_num(cell(a, &c)).unwrap_or(0.0),
                );
                let kb = (
                    is_empty(cell(b, &c)),
                    sign * to_num(cell(b, &c)).unwrap_or(0.0),
                );
                ka.0.cmp(&kb.0)
                    .then(ka.1.partial_cmp(&kb.1).unwrap_or(Ordering::Equal))
            });
        } else {
            rows.sort_by(|a, b| {
                let o = py_str(cell(a, &c)).cmp(&py_str(cell(b, &c)));
                if desc {
                    o.reverse()
                } else {
                    o
                }
            });
        }
    }
    Ok(())
}

/// The keys a query object may use.
pub const QUERY_KEYS: &[&str] = &[
    "sheet",
    "pattern",
    "header",
    "derive",
    "where",
    "where_any",
    "group_by",
    "agg",
    "pivot",
    "trend",
    "having",
    "sort",
    "select",
    "totals",
    "limit",
    "save",
];

fn get_truthy<'a>(q: &'a Map<String, Value>, k: &str) -> Option<&'a Value> {
    q.get(k).filter(|v| truthy(v))
}

/// Apply one query. Returns (columns, rows, info) with info = `{"source_rows", "columns", "matched_rows"}`.
pub fn run(path: &str, q: &Value) -> Result<(Vec<String>, Vec<Row>, Value)> {
    let q = obj(q, "keys")?;
    let mut bad: Vec<&String> = q
        .keys()
        .filter(|k| !QUERY_KEYS.contains(&k.as_str()))
        .collect();
    bad.sort();
    if !bad.is_empty() {
        // 2026-10-02 R10-09: conditions nested under "queries" inside a query were silently ignored → whole table
        let mut keys: Vec<&str> = QUERY_KEYS.to_vec();
        keys.sort();
        return Err(DataError::new(format!(
            "unknown query key(s) {}; a query object uses only {} — e.g. {{\"where\": [{{\"col\": \"status\", \"op\": \">=\", \"value\": 500}}], \"group_by\": [\"path\"], \"agg\": [{{\"fn\": \"count\"}}]}}",
            py_repr(&json!(bad)),
            py_repr(&json!(keys))
        )));
    }
    let sheet = get_truthy(q, "sheet").map(py_str);
    let pattern = get_truthy(q, "pattern").map(py_str);
    let header = q.get("header").map(truthy).unwrap_or(true);
    let (mut cols, mut rows) = load(path, sheet.as_deref(), pattern.as_deref(), header)?;
    let mut info = json!({"source_rows": rows.len(), "columns": cols});
    if let Some(d) = get_truthy(q, "derive") {
        // first, so where / group_by can use derived columns (z-scores see the whole table)
        derive(&mut cols, &mut rows, d)?;
    }
    if let Some(w) = get_truthy(q, "where") {
        rows = where_(&cols, rows, w, false)?;
    }
    if let Some(w) = get_truthy(q, "where_any") {
        rows = where_(&cols, rows, w, true)?;
    }
    info["matched_rows"] = json!(rows.len());
    if let Some(p) = get_truthy(q, "pivot") {
        (cols, rows) = pivot(&cols, &rows, p)?;
    } else if let Some(t) = get_truthy(q, "trend") {
        (cols, rows) = trend(&cols, &rows, t)?;
    } else if get_truthy(q, "group_by").is_some() || get_truthy(q, "agg").is_some() {
        let by = get_truthy(q, "group_by").cloned().unwrap_or(json!([]));
        let aggs = get_truthy(q, "agg")
            .cloned()
            .unwrap_or(json!([{"fn": "count"}]));
        (cols, rows) = group(&cols, &rows, &by, &aggs)?;
    }
    if let Some(h) = get_truthy(q, "having") {
        // filter the grouped / pivoted result
        rows = where_(&cols, rows, h, false)?;
    }
    if let Some(s) = get_truthy(q, "sort") {
        sort(&cols, &mut rows, s)?;
    }
    if let Some(sel) = get_truthy(q, "select") {
        cols = pyfmt::py_iter(sel)
            .map_err(|m| DataError::other("TypeError", m))?
            .iter()
            .map(|c| col(&cols, c))
            .collect::<Result<_>>()?;
    }
    if get_truthy(q, "totals").is_some() && get_truthy(q, "pivot").is_none() {
        let mut t = Row::new();
        for c in &cols {
            let nums: Vec<Option<f64>> = rows
                .iter()
                .map(|r| to_num(r.get(c).unwrap_or(&Value::Null)))
                .collect();
            let v = if !nums.is_empty() && nums.iter().all(|x| x.is_some()) {
                pyfmt::float_value(nums.iter().flatten().fold(0.0, |a, b| a + b))
            } else {
                json!("")
            };
            t.insert(c.clone(), v);
        }
        if let Some(c0) = cols.first() {
            t.insert(c0.clone(), json!("Total"));
        }
        rows.push(t);
    }
    if let Some(l) = get_truthy(q, "limit") {
        let n = py_int(l)?.max(1) as usize;
        rows.truncate(n);
    }
    Ok((cols, rows, info))
}

/// A summary of the table: row/column counts, per-column statistics and the first rows.
pub fn describe(
    path: &str,
    sheet: Option<&str>,
    pattern: Option<&str>,
    header: bool,
) -> Result<String> {
    let (cols, rows) = load(path, sheet, pattern, header)?;
    let mut lines = vec![format!(
        "{} rows × {} columns. Refer to columns by name (or by the alias shown).",
        rows.len(),
        cols.len()
    )];
    for c in &cols {
        let vals: Vec<&Value> = rows.iter().map(|r| cell(r, c)).collect();
        let nonempty: Vec<&Value> = vals.iter().copied().filter(|v| !is_empty(v)).collect();
        let nums: Vec<f64> = nonempty.iter().filter_map(|v| to_num(v)).collect();
        let al = alias(c);
        let head = format!(
            "- {c}{}",
            if al != *c {
                format!(" (alias {al})")
            } else {
                String::new()
            }
        );
        if !nonempty.is_empty() && nums.len() == nonempty.len() {
            let minv = nums.iter().cloned().fold(f64::INFINITY, f64::min);
            let maxv = nums.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
            let sum = nums.iter().fold(0.0, |a, b| a + b);
            lines.push(format!(
                "{head}: number, {} values, min {}, max {}, mean {}, sum {}",
                nums.len(),
                fmt(&pyfmt::float_value(minv)),
                fmt(&pyfmt::float_value(maxv)),
                fmt(&pyfmt::float_value(pyfmt::fmean(&nums))),
                fmt(&pyfmt::float_value(sum))
            ));
        } else {
            let mut distinct: IndexMap<String, usize> = IndexMap::new();
            for v in &nonempty {
                *distinct.entry(py_str(v)).or_insert(0) += 1;
            }
            let mut top: Vec<(&String, &usize)> = distinct.iter().collect();
            top.sort_by_key(|kv| std::cmp::Reverse(*kv.1));
            let top: Vec<String> = top
                .iter()
                .take(6)
                .map(|(k, n)| format!("{} ({n})", take_chars(k, 30)))
                .collect();
            lines.push(format!(
                "{head}: text, {} non-empty, {} distinct; top: {}",
                nonempty.len(),
                distinct.len(),
                top.join(", ")
            ));
        }
    }
    lines.push("First rows:".to_string());
    lines.push(table(&cols, &rows[..rows.len().min(5)]));
    Ok(lines.join("\n"))
}

/// A cell as text for a Markdown table (`dataq.fmt`).
pub fn fmt(v: &Value) -> String {
    if let Value::Number(n) = v {
        if let Some(i) = n.as_i64() {
            return if i.abs() >= 10000 {
                pyfmt::int_commas(i)
            } else {
                i.to_string()
            };
        }
        if let Some(f) = n.as_f64() {
            if f.is_nan() {
                return String::new();
            }
            if f.fract() == 0.0 && f.abs() < 1e15 {
                return pyfmt::int_commas(f as i64);
            }
            return if f.abs() >= 0.0001 {
                pyfmt::fixed_commas(f, 4)
                    .trim_end_matches('0')
                    .trim_end_matches('.')
                    .to_string()
            } else {
                pyfmt::general(f, 4)
            };
        }
    }
    py_str(v).replace('|', "/").replace('\n', " ")
}

/// A Markdown table of the first [`SHOW_ROWS`] rows (`dataq.table(cols, rows)`).
pub fn table(cols: &[String], rows: &[Row]) -> String {
    table_limit(cols, rows, SHOW_ROWS)
}

/// A Markdown table of the first `limit` rows (`dataq.table(cols, rows, limit)`).
pub fn table_limit(cols: &[String], rows: &[Row], limit: usize) -> String {
    if rows.is_empty() {
        return "(no rows)".to_string();
    }
    let mut out = vec![
        format!("| {} |", cols.join(" | ")),
        format!("|{}", "---|".repeat(cols.len())),
    ];
    for r in rows.iter().take(limit) {
        let cells: Vec<String> = cols
            .iter()
            .map(|c| fmt(r.get(c).unwrap_or(&json!(""))))
            .collect();
        out.push(format!("| {} |", cells.join(" | ")));
    }
    if rows.len() > limit {
        out.push(format!(
            "… {} more rows (use save to get them all, or group/limit)",
            rows.len() - limit
        ));
    }
    out.join("\n")
}

/// Write the result as .csv (UTF-8 with BOM) or .xlsx.
pub fn save(cols: &[String], rows: &[Row], path: &str) -> Result<()> {
    let io = |e: std::io::Error| DataError::other("OSError", e.to_string());
    let dir = Path::new(path)
        .parent()
        .filter(|d| !d.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    std::fs::create_dir_all(dir).map_err(io)?;
    if path.to_lowercase().ends_with(".xlsx") {
        let data: Vec<Vec<Value>> = rows
            .iter()
            .map(|r| {
                cols.iter()
                    .map(|c| r.get(c).cloned().unwrap_or(json!("")))
                    .collect()
            })
            .collect();
        xlsx::make(
            path,
            &[json!({"name": "Data", "columns": cols, "rows": data})],
            false,
        )
        .map_err(|e| DataError::other("XlsxError", e.0))?;
        return Ok(());
    }
    let mut out = String::from("\u{feff}");
    pycsv::write_row(&mut out, cols);
    for r in rows {
        let fields: Vec<String> = cols
            .iter()
            .map(|c| match r.get(c) {
                None => String::new(),
                Some(Value::Null) => String::new(),
                Some(v) => py_str(v),
            })
            .collect();
        pycsv::write_row(&mut out, &fields);
    }
    std::fs::write(path, out).map_err(io)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str, content: &str) -> String {
        let dir = std::env::temp_dir().join(format!("omuse-docs-dataq-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join(name);
        std::fs::write(&p, content).unwrap();
        p.to_string_lossy().into_owned()
    }

    #[test]
    fn exact_tables() {
        let p = tmp(
            "t.csv",
            "姓名,年龄,金额(新元)\nA,25,\"1,200\"\nB,35,S$800\nC,45,300\nD,55,\n",
        );
        let (_c, rows, _i) = run(
            &p,
            &json!({"agg": [{"col": "金额(新元)", "fn": "sum"}, {"fn": "count"}]}),
        )
        .unwrap();
        assert_eq!(rows[0]["sum(金额(新元))"].as_f64(), Some(2300.0));
        assert_eq!(rows[0]["count"], json!(4));
        let (_c, rows, _i) = run(
            &p,
            &json!({"derive": [{"as": "x2", "expr": "金额_新元 * 2"}], "where": [{"col": "年龄", "op": "between", "value": [30, 50]}]}),
        )
        .unwrap();
        assert_eq!(
            rows.iter()
                .map(|r| r["x2"].as_f64().unwrap())
                .collect::<Vec<_>>(),
            vec![1600.0, 600.0]
        );
        let (_c, rows, _i) = run(
            &p,
            &json!({"derive": [{"as": "g", "from": "年龄", "bins": [0, 30, 50, 200], "labels": ["<30", "30-49", "50+"]}],
                    "group_by": ["g"], "agg": [{"fn": "count"}], "sort": ["g"]}),
        )
        .unwrap();
        let got: Vec<(String, i64)> = rows
            .iter()
            .map(|r| {
                (
                    r["g"].as_str().unwrap().to_string(),
                    r["count"].as_i64().unwrap(),
                )
            })
            .collect();
        assert_eq!(
            got,
            vec![("30-49".into(), 2), ("50+".into(), 1), ("<30".into(), 1)]
        );
        assert_eq!(to_num(&json!("12%")), Some(12.0));
        assert_eq!(to_num(&json!("007")), Some(7.0));
        assert_eq!(to_num(&json!("2026-10-01")), None);
        assert_eq!(clean(&json!("007")), json!("007"));
        assert_eq!(clean(&json!(" 1,200 ")), json!(1200));
        assert_eq!(clean(&json!("12%")), json!(12.0));
        assert_eq!(clean(&json!(3.0)), json!(3));
        let err = run(&p, &json!({"group_by": ["missing"]})).unwrap_err();
        assert_eq!(
            err.to_string(),
            "no column 'missing'; columns: ['姓名', '年龄', '金额(新元)']"
        );
        assert!(err.is_data());
        let bad = tmp("b.xlsx", "not a zip");
        let err = load(&bad, None, None, true).unwrap_err();
        assert!(
            err.to_string()
                .starts_with("cannot open workbook (damaged or not a real .xlsx?): "),
            "{err}"
        );
    }

    #[test]
    fn derive_first_column_compare_and_having() {
        // 2026-10-02 R5-02: where on a derived column, "金额(新元) - 预算(新元)" in expr, value naming another column
        let p = tmp(
            "e.csv",
            "月份,金额(新元),预算(新元)\n1,120,100\n1,90,100\n2,130,100\n2,150,100\n",
        );
        let (_c, rows, _i) = run(
            &p,
            &json!({"derive": [{"as": "diff", "expr": "金额(新元) - 预算(新元)"}],
                    "where": [{"col": "diff", "op": ">", "value": 0}], "group_by": ["月份"],
                    "agg": [{"fn": "count", "as": "n"}], "having": [{"col": "n", "op": ">=", "value": 2}]}),
        )
        .unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(
            (rows[0]["月份"].clone(), rows[0]["n"].clone()),
            (json!(2), json!(2))
        );
        let (_c, rows, _i) =
            run(&p, &json!({"where": [{"col": "金额(新元)", "op": ">", "value": "预算(新元)"}], "agg": [{"fn": "count"}]})).unwrap();
        assert_eq!(rows[0]["count"], json!(3));
    }

    #[test]
    fn rejects_unknown_keys() {
        let p = tmp("x.csv", "a,b\n1,2\n");
        let err = run(
            &p,
            &json!({"queries": [{"col": "a", "op": ">", "value": 0}]}),
        )
        .unwrap_err();
        assert!(err.to_string().starts_with("unknown query key(s) ['queries']; a query object uses only ['agg', 'derive', 'group_by', 'having', 'header', 'limit', 'pattern', 'pivot', 'save', 'select', 'sheet', 'sort', 'totals', 'trend', 'where', 'where_any'] — e.g. {\"where\""), "{err}");
    }

    #[test]
    fn dataqtest_scenario() {
        // the DATAQTEST scenario of tests/fake_llm.py and the assertions of tests/market_e2e.py
        let csv: String = "id,age,nps\n".to_string()
            + &(0..40)
                .map(|i| format!("u{i},{},{}", 20 + i, [2, 5, 7, 8, 9, 10][i % 6]))
                .collect::<Vec<_>>()
                .join("\n");
        let p = tmp("s.csv", &csv);
        let d = describe(&p, None, None, true).unwrap();
        assert!(d.starts_with("40 rows × 3 columns"), "{d}");
        assert!(
            d.contains("- nps: number, 40 values, min 2, max 10, mean 6.7, sum 268"),
            "{d}"
        );
        assert!(
            d.contains("- id: text, 40 non-empty, 40 distinct; top: u0 (1), u1 (1)"),
            "{d}"
        );
        let (cols, rows, info) = run(
            &p,
            &json!({"derive": [{"as": "seg", "from": "nps", "bins": [0, 7, 9, 11], "labels": ["det", "pas", "pro"]}],
                    "group_by": ["seg"], "agg": [{"fn": "count"}, {"fn": "count_share", "as": "pct"}], "sort": ["seg"]}),
        )
        .unwrap();
        assert_eq!(info["matched_rows"], json!(40));
        let t = table(&cols, &rows);
        assert_eq!(t, "| seg | count | pct |\n|---|---|---|\n| det | 14 | 35 |\n| pas | 14 | 35 |\n| pro | 12 | 30 |");
        let (cols, rows, info) =
            run(&p, &json!({"where": [{"col": "age", "op": ">=", "value": 50}], "agg": [{"col": "nps", "fn": "mean"}], "save": "x"})).unwrap();
        assert_eq!(
            (info["matched_rows"].clone(), info["source_rows"].clone()),
            (json!(10), json!(40))
        );
        assert_eq!(table(&cols, &rows), "| mean(nps) |\n|---|\n| 6.3 |");
        let out = tmp("out.csv", "");
        save(&cols, &rows, &out).unwrap();
        assert_eq!(
            std::fs::read_to_string(&out).unwrap(),
            "\u{feff}mean(nps)\r\n6.3\r\n"
        );
        let err = run(&p, &json!({"group_by": ["nope"]})).unwrap_err();
        assert_eq!(
            err.to_string(),
            "no column 'nope'; columns: ['id', 'age', 'nps']"
        );
        let log = [
            (8, "a", 200, 100),
            (8, "b", 500, 300),
            (9, "b", 503, 500),
            (9, "a", 200, 120),
        ]
        .iter()
        .map(|(h, p, st, ms)| {
            format!("2026-10-01T{h:02}:00:00 1.1.1.1 \"GET /api/{p}\" {st} {ms}ms")
        })
        .collect::<Vec<_>>()
        .join("\n");
        let lp = tmp("app.log", &log);
        let q = json!({"pattern": r#"^(?P<time>\S+) \S+ "GET (?P<path>[^"]+)" (?P<status>\d{3}) (?P<ms>\d+)ms"#,
                       "derive": [{"as": "err", "expr": "status >= 500"}], "group_by": ["path"],
                       "agg": [{"col": "err", "fn": "sum", "as": "errors"}, {"col": "ms", "fn": "p95"}],
                       "sort": [{"col": "errors", "desc": true}]});
        let (cols, rows, _i) = run(&lp, &q).unwrap();
        assert_eq!(table(&cols, &rows), "| path | errors | p95(ms) |\n|---|---|---|\n| /api/b | 2 | 490 |\n| /api/a | 0 | 119 |");
    }

    #[test]
    fn pivot_trend_and_more_derives() {
        let p = tmp("pv.csv", "m,region,sales\n2026-01-15,N,10\n2026-01-20,S,20\n2026-02-10,N,30\n2026-02-11,S,25\n2026-03-05,N,50\n");
        let (cols, rows, _) = run(&p, &json!({"derive": [{"as": "mon", "from": "m", "date_part": "month"}], "pivot": {"rows": "mon", "cols": "region", "value": "sales"}})).unwrap();
        assert_eq!(cols, vec!["mon", "N", "S", "Total"]);
        assert_eq!(table(&cols, &rows), "| mon | N | S | Total |\n|---|---|---|---|\n| 2026-01 | 10 | 20 | 30 |\n| 2026-02 | 30 | 25 | 55 |\n| 2026-03 | 50 | 0 | 50 |\n| Total | 90 | 45 | 135 |");
        let (cols, rows, _) = run(&p, &json!({"where": [{"col": "region", "op": "=", "value": "N"}], "trend": {"y": "sales", "ahead": 2}})).unwrap();
        assert_eq!(
            cols,
            vec![
                "n",
                "slope",
                "intercept",
                "r2",
                "last_y",
                "forecast_1",
                "forecast_2"
            ]
        );
        assert_eq!(table(&cols, &rows), "| n | slope | intercept | r2 | last_y | forecast_1 | forecast_2 |\n|---|---|---|---|---|---|---|\n| 3 | 20 | -10 | 1 | 50 | 70 | 90 |");
        let (_c, rows, _) = run(&p, &json!({"derive": [
            {"as": "rk", "rank": "sales"}, {"as": "cs", "cumsum": "sales", "by": ["region"]},
            {"as": "pc", "pct_change": "sales"}, {"as": "z", "zscore": "sales"},
            {"as": "wd", "from": "m", "date_part": "weekday"}, {"as": "yy", "from": "m", "slice": [0, 4]},
            {"as": "rg", "from": "region", "map": {"N": "North"}, "keep": false, "default": "?"}],
            "select": ["rk", "cs", "pc", "z", "wd", "yy", "rg"], "limit": 2})).unwrap();
        let pick = |r: &Row| -> Value {
            Value::Object(
                r.iter()
                    .filter(|(k, _)| {
                        ["rk", "cs", "pc", "z", "wd", "yy", "rg"].contains(&k.as_str())
                    })
                    .map(|(k, v)| (k.clone(), v.clone()))
                    .collect(),
            )
        };
        assert_eq!(rows.len(), 2); // limit; select only narrows the shown columns, rows keep every key
        assert_eq!(
            pick(&rows[0]),
            json!({"rk": 5, "cs": 10.0, "pc": "", "z": -1.146, "wd": "Thu", "yy": "2026", "rg": "North"})
        );
        assert_eq!(
            pick(&rows[1]),
            json!({"rk": 4, "cs": 20.0, "pc": 100.0, "z": -0.472, "wd": "Tue", "yy": "2026", "rg": "?"})
        );
        let (cols, rows, _) = run(&p, &json!({"group_by": "region", "agg": [{"col": "sales", "fn": "share"}, {"col": "sales", "fn": "list"}, {"col": "sales", "fn": "median"}], "totals": true})).unwrap();
        assert_eq!(table(&cols, &rows), "| region | share(sales) | list(sales) | median(sales) |\n|---|---|---|---|\n| N | 66.6667 | 10, 30, 50 | 30 |\n| S | 33.3333 | 20, 25 | 22.5 |\n| Total | 100 |  | 52.5 |");
        assert_eq!(
            run(&p, &json!({"agg": [{"fn": "sum"}]}))
                .unwrap_err()
                .to_string(),
            "agg sum needs \"col\""
        );
        assert_eq!(run(&p, &json!({"agg": [{"col": "sales", "fn": "wat"}]})).unwrap_err().to_string(), "unknown agg fn 'wat' (count, count_distinct, sum, mean, median, min, max, std, var, range, p50/p90/p95/p99, share, count_share, mode, first, last, list)");
        assert_eq!(
            run(
                &p,
                &json!({"where": [{"col": "sales", "op": "~", "value": 1}]})
            )
            .unwrap_err()
            .to_string(),
            "unknown op '~'"
        );
        assert_eq!(fmt(&json!(1234567.891)), "1,234,567.891");
        assert_eq!(fmt(&json!(0.00001234)), "1.234e-05");
        assert_eq!(fmt(&json!(15000)), "15,000");
        assert_eq!(fmt(&json!("a|b\nc")), "a/b c");
        assert_eq!(alias("金额(新元)"), "金额_新元");
        assert_eq!(alias("2024 sales"), "c_2024_sales");
        assert_eq!(alias("()"), "c");
    }

    #[test]
    fn semicolon_csv_header_less_and_xlsx_roundtrip() {
        let p = tmp("semi.csv", "a;b;c\n1;2;3\n4;5;6\n");
        let (cols, rows) = load(&p, None, None, false).unwrap();
        assert_eq!(cols, vec!["col1", "col2", "col3"]);
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[1]["col2"], json!(2));
        let x = tmp("rt.xlsx", "");
        save(
            &["k".to_string(), "v".to_string()],
            &rows[1..]
                .iter()
                .map(|r| {
                    let mut m = Row::new();
                    m.insert("k".into(), r["col1"].clone());
                    m.insert("v".into(), r["col3"].clone());
                    m
                })
                .collect::<Vec<_>>(),
            &x,
        )
        .unwrap();
        let (cols, rows) = load(&x, None, None, true).unwrap();
        assert_eq!(cols, vec!["k", "v"]);
        assert_eq!(rows[0]["v"], json!(3));
        assert_eq!(
            load(&x, Some("Nope"), None, true).unwrap_err().to_string(),
            "no sheet 'Nope'; sheets: ['Data']"
        );
        let e = tmp("empty.csv", "\n\n");
        assert_eq!(
            load(&e, None, None, true).unwrap_err().to_string(),
            "the table is empty"
        );
        assert_eq!(
            load(&tmp("x.bin", "x"), None, None, true)
                .unwrap_err()
                .to_string(),
            "supported: .csv .tsv .txt .xlsx (for logs or other text pass a regex `pattern`)"
        );
        assert_eq!(
            load(&e, None, Some("(?P<a>x)"), true)
                .unwrap_err()
                .to_string(),
            "pattern matched no lines (0 non-empty lines did not match)"
        );
        assert_eq!(
            load(&e, None, Some("(x)"), true).unwrap_err().to_string(),
            "pattern needs named groups, e.g. (?P<time>\\S+) (?P<status>\\d{3})"
        );
    }
}
