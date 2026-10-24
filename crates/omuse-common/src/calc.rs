//! Safe calculator for the agent (no code execution): arithmetic, math functions and a few finance helpers.
//!
//! The model is bad at arithmetic (a 2026-10-02 test answered a S$3M / 25-year / 3.5% mortgage with S$15,155 a month and a
//! first-month interest of S$1,761; the right numbers are S$15,018.71 and S$8,750). Expressions are parsed like Python's
//! `ast` and only numbers, names from the whitelist below, the user's own variables and arithmetic operators are allowed.
//!
//! Port of `app/common/calc.py`. The Python version leaned on the interpreter for its semantics; this module reproduces
//! the parts the calculator can reach: Python's expression grammar (with `ast.dump` text for "not allowed" errors and the
//! CPython 3.12 syntax-error messages that matter), arbitrary-precision ints, float semantics and formatting, the
//! whitelisted builtins / `math` / `statistics` functions with their error texts, and the finance helpers.

use std::cmp::Ordering;
use std::fmt;

use indexmap::IndexMap;
use num_bigint::{BigInt, BigUint, Sign};
use num_integer::Integer;
use num_traits::{One, Signed, ToPrimitive, Zero};
use serde_json::Value;

// ---------------------------------------------------------------- errors and values

/// `CalcError`: the message the agent shows after `ERROR: `.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CalcError(pub String);

impl fmt::Display for CalcError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for CalcError {}

/// An exception raised while evaluating: our own `CalcError` or a Python built-in exception (type name, message).
#[derive(Debug, Clone)]
enum Exc {
    Calc(String),
    Py(&'static str, String),
}

type R<T> = Result<T, Exc>;

fn calc_err<T>(msg: impl Into<String>) -> R<T> {
    Err(Exc::Calc(msg.into()))
}
fn type_err<T>(msg: impl Into<String>) -> R<T> {
    Err(Exc::Py("TypeError", msg.into()))
}
fn value_err<T>(msg: impl Into<String>) -> R<T> {
    Err(Exc::Py("ValueError", msg.into()))
}
fn zero_div<T>(msg: impl Into<String>) -> R<T> {
    Err(Exc::Py("ZeroDivisionError", msg.into()))
}
fn overflow<T>(msg: impl Into<String>) -> R<T> {
    Err(Exc::Py("OverflowError", msg.into()))
}
fn stats_err<T>(msg: impl Into<String>) -> R<T> {
    Err(Exc::Py("StatisticsError", msg.into()))
}

/// A Python value the calculator can produce or receive as a variable.
#[derive(Debug, Clone, PartialEq)]
pub enum Val {
    Int(BigInt),
    Float(f64),
    Bool(bool),
    /// Only produced by a negative number raised to a fractional power (Python returns a complex then).
    Complex(f64, f64),
    Str(String),
    None,
    /// Python lists (tuples evaluate to lists as well, like `[_ev(x) for x in node.elts]`).
    List(Vec<Val>),
    /// The result dicts of `loan` / `invest` / `apr` (and JSON objects given as variables).
    Dict(IndexMap<String, Val>),
}

/// The variables of an evaluation (`env` in the Python).
pub type Env = IndexMap<String, Val>;

impl From<i64> for Val {
    fn from(v: i64) -> Self {
        Val::Int(BigInt::from(v))
    }
}
impl From<f64> for Val {
    fn from(v: f64) -> Self {
        Val::Float(v)
    }
}
impl From<bool> for Val {
    fn from(v: bool) -> Self {
        Val::Bool(v)
    }
}
impl From<BigInt> for Val {
    fn from(v: BigInt) -> Self {
        Val::Int(v)
    }
}

impl Val {
    /// `type(v).__name__`.
    pub fn type_name(&self) -> &'static str {
        match self {
            Val::Int(_) => "int",
            Val::Float(_) => "float",
            Val::Bool(_) => "bool",
            Val::Complex(..) => "complex",
            Val::Str(_) => "str",
            Val::None => "NoneType",
            Val::List(_) => "list",
            Val::Dict(_) => "dict",
        }
    }

    /// `isinstance(v, (int, float))` (bool is an int).
    pub fn is_number(&self) -> bool {
        matches!(self, Val::Int(_) | Val::Float(_) | Val::Bool(_))
    }

    /// The value as a float when it is an int / float / bool that fits.
    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Val::Int(i) => int_to_f64(i).ok(),
            Val::Float(f) => Some(*f),
            Val::Bool(b) => Some(if *b { 1.0 } else { 0.0 }),
            _ => None,
        }
    }

    /// `d[key]` for dict results.
    pub fn get(&self, key: &str) -> Option<&Val> {
        match self {
            Val::Dict(d) => d.get(key),
            _ => None,
        }
    }

    /// Python `repr(v)`.
    pub fn repr(&self) -> String {
        match self {
            Val::Int(i) => i.to_string(),
            Val::Float(f) => float_repr(*f),
            Val::Bool(true) => "True".into(),
            Val::Bool(false) => "False".into(),
            Val::Complex(re, im) => complex_repr(*re, *im),
            Val::Str(s) => crate::pyjson::py_repr(&Value::String(s.clone())),
            Val::None => "None".into(),
            Val::List(items) => format!(
                "[{}]",
                items.iter().map(Val::repr).collect::<Vec<_>>().join(", ")
            ),
            Val::Dict(d) => format!(
                "{{{}}}",
                d.iter()
                    .map(|(k, v)| format!(
                        "{}: {}",
                        crate::pyjson::py_repr(&Value::String(k.clone())),
                        v.repr()
                    ))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        }
    }

    /// Python `str(v)`.
    pub fn py_str(&self) -> String {
        match self {
            Val::Str(s) => s.clone(),
            other => other.repr(),
        }
    }

    /// The value as it arrives from `json.loads` (variables of the `calculate` tool).
    pub fn from_json(v: &Value) -> Val {
        match v {
            Value::Null => Val::None,
            Value::Bool(b) => Val::Bool(*b),
            Value::Number(n) => {
                if let Some(i) = n.as_i64() {
                    Val::from(i)
                } else if let Some(u) = n.as_u64() {
                    Val::Int(BigInt::from(u))
                } else {
                    Val::Float(n.as_f64().unwrap_or(f64::NAN))
                }
            }
            Value::String(s) => Val::Str(s.clone()),
            Value::Array(a) => Val::List(a.iter().map(Val::from_json).collect()),
            Value::Object(o) => Val::Dict(
                o.iter()
                    .map(|(k, v)| (k.clone(), Val::from_json(v)))
                    .collect(),
            ),
        }
    }

    /// JSON form (ints too large for i64 and non-finite floats become strings / null as `json` cannot hold them).
    pub fn to_json(&self) -> Value {
        match self {
            Val::Int(i) => match i.to_i64() {
                Some(v) => Value::from(v),
                None => Value::String(i.to_string()),
            },
            Val::Float(f) => serde_json::Number::from_f64(*f)
                .map(Value::Number)
                .unwrap_or(Value::Null),
            Val::Bool(b) => Value::Bool(*b),
            Val::Complex(..) => Value::String(self.repr()),
            Val::Str(s) => Value::String(s.clone()),
            Val::None => Value::Null,
            Val::List(items) => Value::Array(items.iter().map(Val::to_json).collect()),
            Val::Dict(d) => {
                Value::Object(d.iter().map(|(k, v)| (k.clone(), v.to_json())).collect())
            }
        }
    }
}

// ---------------------------------------------------------------- float / int helpers

/// Python `repr(float)`.
fn float_repr(f: f64) -> String {
    if f.is_nan() {
        "nan".into()
    } else if f.is_infinite() {
        if f > 0.0 {
            "inf".into()
        } else {
            "-inf".into()
        }
    } else {
        crate::pyjson::format_f64(f)
    }
}

/// A complex component as `repr(complex)` writes it (no `.0` for integral values).
fn complex_part(f: f64) -> String {
    let s = float_repr(f);
    match s.strip_suffix(".0") {
        Some(t) => t.to_string(),
        None => s,
    }
}

/// Python `repr(complex)`.
fn complex_repr(re: f64, im: f64) -> String {
    let ims = complex_part(im);
    if re == 0.0 && re.is_sign_positive() {
        return format!("{ims}j");
    }
    let sign = if ims.starts_with('-') { "" } else { "+" };
    format!("({}{}{}j)", complex_part(re), sign, ims)
}

/// `x * 2**k` without intermediate overflow/underflow (exact when the result is representable).
fn ldexp(mut x: f64, mut k: i64) -> f64 {
    let big = 2f64.powi(1000);
    let small = 2f64.powi(-1000);
    while k > 1000 {
        x *= big;
        k -= 1000;
        if x.is_infinite() {
            return x;
        }
    }
    while k < -1000 {
        x *= small;
        k += 1000;
        if x == 0.0 {
            return x;
        }
    }
    x * 2f64.powi(k as i32)
}

/// `n / d` correctly rounded to the nearest float (ties to even), `None` when it overflows. `d` must not be zero.
fn ratio_to_f64(n: &BigUint, d: &BigUint) -> Option<f64> {
    if n.is_zero() {
        return Some(0.0);
    }
    let a = n.bits() as i64;
    let b = d.bits() as i64;
    let shift = 55 + b - a; // q = floor(n * 2^shift / d) has 55 or 56 bits
    let (q, r) = if shift >= 0 {
        (n << shift as u64).div_rem(d)
    } else {
        n.div_rem(&(d << (-shift) as u64))
    };
    let sticky = !r.is_zero();
    let qbits = q.bits() as i64;
    let e = qbits - 1 - shift; // value in [2^e, 2^(e+1))
    if e > 1023 {
        return None;
    }
    let keep = if e >= -1022 { 53 } else { 53 - (-1022 - e) };
    if keep < 0 {
        return Some(0.0);
    }
    let drop = qbits - keep;
    let mut mant = (&q >> drop as u64).to_u64().unwrap_or(0);
    if drop > 0 {
        let rem = &q - (BigUint::from(mant) << drop as u64);
        let half = BigUint::one() << (drop - 1) as u64;
        let up = match rem.cmp(&half) {
            Ordering::Greater => true,
            Ordering::Equal => sticky || mant & 1 == 1,
            Ordering::Less => false,
        };
        if up {
            mant += 1;
        }
    }
    let v = ldexp(mant as f64, drop - shift);
    if v.is_infinite() {
        None
    } else {
        Some(v)
    }
}

/// `float(int)`: OverflowError when too large.
fn int_to_f64(i: &BigInt) -> R<f64> {
    if let Some(v) = i.to_i64() {
        if v.unsigned_abs() <= (1u64 << 53) {
            return Ok(v as f64);
        }
    }
    match ratio_to_f64(i.magnitude(), &BigUint::one()) {
        Some(v) => Ok(if i.sign() == Sign::Minus { -v } else { v }),
        None => overflow("int too large to convert to float"),
    }
}

/// Python `int / int`.
fn int_truediv(a: &BigInt, b: &BigInt) -> R<f64> {
    if b.is_zero() {
        return zero_div("division by zero");
    }
    if let (Some(x), Some(y)) = (a.to_i64(), b.to_i64()) {
        if x.unsigned_abs() <= (1u64 << 53) && y.unsigned_abs() <= (1u64 << 53) {
            return Ok(x as f64 / y as f64);
        }
    }
    let neg = (a.sign() == Sign::Minus) != (b.sign() == Sign::Minus);
    match ratio_to_f64(a.magnitude(), b.magnitude()) {
        Some(v) => Ok(if neg { -v } else { v }),
        None => overflow("integer division result too large for a float"),
    }
}

/// The exact integer value of an integral float.
fn f64_to_bigint(f: f64) -> BigInt {
    use num_traits::FromPrimitive;
    BigInt::from_f64(f).unwrap_or_default()
}

/// `int(float)` / `math.floor` / `round` conversions: errors for inf / nan like Python.
fn float_to_int_checked(f: f64) -> R<BigInt> {
    if f.is_nan() {
        return value_err("cannot convert float NaN to integer");
    }
    if f.is_infinite() {
        return overflow("cannot convert float infinity to integer");
    }
    Ok(f64_to_bigint(f.trunc()))
}

/// Exact comparison of an int with a float (`None` when the float is NaN).
fn cmp_int_float(i: &BigInt, f: f64) -> Option<Ordering> {
    if f.is_nan() {
        return None;
    }
    if f.is_infinite() {
        return Some(if f > 0.0 {
            Ordering::Less
        } else {
            Ordering::Greater
        });
    }
    let fl = f.floor();
    match i.cmp(&f64_to_bigint(fl)) {
        Ordering::Equal => Some(if f > fl {
            Ordering::Less
        } else {
            Ordering::Equal
        }),
        o => Some(o),
    }
}

/// Numeric comparison of two int/float/bool values (`None` for NaN; `Err` for non-numbers).
fn num_cmp(a: &Val, b: &Val) -> Result<Option<Ordering>, ()> {
    let ai = as_int(a);
    let bi = as_int(b);
    match (a, b, ai, bi) {
        (_, _, Some(x), Some(y)) => Ok(Some(x.cmp(&y))),
        (Val::Float(x), Val::Float(y), _, _) => Ok(x.partial_cmp(y)),
        (_, Val::Float(y), Some(x), _) => Ok(cmp_int_float(&x, *y)),
        (Val::Float(x), _, _, Some(y)) => Ok(cmp_int_float(&y, *x).map(Ordering::reverse)),
        _ => Err(()),
    }
}

/// The value as an int when it is an int or a bool.
fn as_int(v: &Val) -> Option<BigInt> {
    match v {
        Val::Int(i) => Some(i.clone()),
        Val::Bool(b) => Some(BigInt::from(*b as i64)),
        _ => None,
    }
}

/// Python `f"{v:,.Nf}"`.
fn format_fixed_commas(v: f64, prec: usize) -> String {
    if !v.is_finite() {
        return float_repr(v);
    }
    let s = format!("{:.*}", prec, v);
    let (sign, body) = match s.strip_prefix('-') {
        Some(b) => ("-", b),
        None => ("", s.as_str()),
    };
    let (int_part, frac) = match body.split_once('.') {
        Some((i, f)) => (i, Some(f)),
        None => (body, None),
    };
    let mut grouped = String::new();
    for (i, ch) in int_part.chars().enumerate() {
        if i > 0 && (int_part.len() - i) % 3 == 0 {
            grouped.push(',');
        }
        grouped.push(ch);
    }
    match frac {
        Some(f) => format!("{sign}{grouped}.{f}"),
        None => format!("{sign}{grouped}"),
    }
}

/// Python `f"{v:.{prec}g}"`.
fn format_g(v: f64, prec: usize) -> String {
    if !v.is_finite() {
        return float_repr(v);
    }
    let prec = prec.max(1);
    let sci = format!("{:.*e}", prec - 1, v);
    let (mant, exp) = sci.split_once('e').unwrap_or((&sci, "0"));
    let exp: i64 = exp.parse().unwrap_or(0);
    if exp < -4 || exp >= prec as i64 {
        let mant = if mant.contains('.') {
            mant.trim_end_matches('0').trim_end_matches('.')
        } else {
            mant
        };
        let sign = if exp < 0 { '-' } else { '+' };
        format!("{mant}e{sign}{:02}", exp.abs())
    } else {
        let decimals = (prec as i64 - 1 - exp).max(0) as usize;
        let s = format!("{:.*}", decimals, v);
        if s.contains('.') {
            s.trim_end_matches('0').trim_end_matches('.').to_string()
        } else {
            s
        }
    }
}

/// Python `f"{v:g}"`.
fn format_g_default(v: f64) -> String {
    format_g(v, 6)
}

// ---------------------------------------------------------------- tokenizer (CPython 3.12 rules for one expression)

#[derive(Debug, Clone, PartialEq)]
enum Num {
    Int(BigInt),
    Float(f64),
    Imag(f64),
}

#[derive(Debug, Clone, PartialEq)]
enum Tok {
    Name(String),
    Number(Num),
    /// A string literal: decoded value, bytes?, f-string?, `u` prefix?, raw body (for f-strings).
    Str {
        value: String,
        bytes: bool,
        fstring: bool,
        u: bool,
        raw: bool,
    },
    Op(&'static str),
    /// A character the grammar has no use for (`$`, `?`, `` ` ``): the parser fails on it with "invalid syntax".
    Unknown,
    Newline,
    End,
}

#[derive(Debug, Clone)]
struct Token {
    tok: Tok,
    /// Bracket nesting level after this token.
    level: usize,
}

const KEYWORDS: [&str; 35] = [
    "False", "None", "True", "and", "as", "assert", "async", "await", "break", "class", "continue",
    "def", "del", "elif", "else", "except", "finally", "for", "from", "global", "if", "import",
    "in", "is", "lambda", "nonlocal", "not", "or", "pass", "raise", "return", "try", "while",
    "with", "yield",
];

const OPERATORS: [&str; 48] = [
    "**=", "//=", ">>=", "<<=", "...", "!=", "==", "<=", ">=", "->", ":=", "**", "//", "<<", ">>",
    "+=", "-=", "*=", "/=", "%=", "&=", "|=", "^=", "@=", "<>", "+", "-", "*", "/", "%", "@", "&",
    "|", "^", "~", "<", ">", "(", ")", "[", "]", "{", "}", ",", ":", ";", ".", "=",
];

struct Lexer<'a> {
    chars: Vec<char>,
    pos: usize,
    line: usize,
    out: Vec<Token>,
    stack: Vec<(char, usize)>,
    _src: &'a str,
}

fn is_potential_ident_start(c: char) -> bool {
    c.is_ascii_alphabetic() || c == '_' || (c as u32) >= 128
}

fn is_potential_ident_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_' || (c as u32) >= 128
}

fn is_string_prefix(p: &str) -> bool {
    matches!(
        p.to_ascii_lowercase().as_str(),
        "u" | "b" | "r" | "f" | "br" | "rb" | "fr" | "rf"
    )
}

/// Tokenize `src`. `Err` carries the SyntaxError message of a tokenizer error.
fn tokenize(src: &str) -> Result<Vec<Token>, String> {
    let mut lx = Lexer {
        chars: src.chars().collect(),
        pos: 0,
        line: 1,
        out: Vec::new(),
        stack: Vec::new(),
        _src: src,
    };
    lx.run()?;
    Ok(lx.out)
}

impl Lexer<'_> {
    fn peek(&self, off: usize) -> Option<char> {
        self.chars.get(self.pos + off).copied()
    }

    fn push(&mut self, tok: Tok) {
        self.out.push(Token {
            tok,
            level: self.stack.len(),
        });
    }

    fn run(&mut self) -> Result<(), String> {
        while let Some(c) = self.peek(0) {
            match c {
                ' ' | '\t' | '\x0c' => self.pos += 1,
                '#' => {
                    while let Some(c) = self.peek(0) {
                        if c == '\n' || c == '\r' {
                            break;
                        }
                        self.pos += 1;
                    }
                }
                '\\' => {
                    let next = self.peek(1);
                    if next == Some('\n') {
                        self.pos += 2;
                        self.line += 1;
                    } else if next == Some('\r') {
                        self.pos += if self.peek(2) == Some('\n') { 3 } else { 2 };
                        self.line += 1;
                    } else {
                        return Err("unexpected character after line continuation character".into());
                    }
                }
                '\n' | '\r' => {
                    self.pos += if c == '\r' && self.peek(1) == Some('\n') {
                        2
                    } else {
                        1
                    };
                    if self.stack.is_empty()
                        && !self.out.is_empty()
                        && !matches!(self.out.last().map(|t| &t.tok), Some(Tok::Newline))
                    {
                        self.push(Tok::Newline);
                    }
                    self.line += 1;
                }
                c if c.is_ascii_digit()
                    || (c == '.' && self.peek(1).is_some_and(|d| d.is_ascii_digit())) =>
                {
                    self.number()?
                }
                c if is_potential_ident_start(c) => self.name_or_string()?,
                '"' | '\'' => self.string(String::new())?,
                _ => self.operator()?,
            }
        }
        if !self.out.is_empty()
            && !matches!(self.out.last().map(|t| &t.tok), Some(Tok::Newline))
            && self.stack.is_empty()
        {
            self.push(Tok::Newline);
        }
        self.push(Tok::End);
        Ok(())
    }

    fn operator(&mut self) -> Result<(), String> {
        for op in OPERATORS {
            let n = op.chars().count();
            if (0..n).all(|i| self.peek(i) == op.chars().nth(i)) {
                self.pos += n;
                match op {
                    "(" | "[" | "{" => {
                        if self.stack.len() >= 200 {
                            return Err("too many nested parentheses".into());
                        }
                        self.stack
                            .push((op.chars().next().unwrap_or('('), self.line));
                    }
                    ")" | "]" | "}" => {
                        let close = op.chars().next().unwrap_or(')');
                        match self.stack.pop() {
                            None => return Err(format!("unmatched '{close}'")),
                            Some((open, line)) => {
                                let want = match open {
                                    '(' => ')',
                                    '[' => ']',
                                    _ => '}',
                                };
                                if want != close {
                                    return Err(if line != self.line {
                                        format!("closing parenthesis '{close}' does not match opening parenthesis '{open}' on line {line}")
                                    } else {
                                        format!("closing parenthesis '{close}' does not match opening parenthesis '{open}'")
                                    });
                                }
                            }
                        }
                    }
                    _ => {}
                }
                self.push(Tok::Op(op));
                return Ok(());
            }
        }
        // `!` alone, `$`, `?`, `` ` `` and control characters
        self.pos += 1;
        self.push(Tok::Unknown);
        Ok(())
    }

    fn name_or_string(&mut self) -> Result<(), String> {
        let start = self.pos;
        while let Some(c) = self.peek(0) {
            if !is_potential_ident_char(c) {
                break;
            }
            self.pos += 1;
            // string prefix directly followed by a quote
            let so_far: String = self.chars[start..self.pos].iter().collect();
            if matches!(self.peek(0), Some('"') | Some('\'')) && is_string_prefix(&so_far) {
                return self.string(so_far);
            }
        }
        let raw: String = self.chars[start..self.pos].iter().collect();
        // verify_identifier: every character must be XID (the first XID_Start or '_')
        for (i, ch) in raw.chars().enumerate() {
            let ok = if i == 0 {
                ch == '_' || unicode_ident::is_xid_start(ch)
            } else {
                unicode_ident::is_xid_continue(ch)
            };
            if !ok {
                return Err(format!("invalid character '{}' (U+{:04X})", ch, ch as u32));
            }
        }
        let name = if raw.is_ascii() {
            raw
        } else {
            use unicode_normalization::UnicodeNormalization;
            raw.nfkc().collect()
        };
        self.push(Tok::Name(name));
        Ok(())
    }

    /// `verify_end_of_number`.
    fn verify_end(&mut self, kind: &str) -> Result<(), String> {
        let c = match self.peek(0) {
            Some(c) => c,
            None => return Ok(()),
        };
        let rest = |s: &str| {
            s.chars()
                .enumerate()
                .all(|(i, ch)| self.peek(1 + i) == Some(ch))
        };
        let kw = match c {
            'a' => rest("nd"),
            'e' => rest("lse"),
            'f' => rest("or"),
            'i' => matches!(self.peek(1), Some('f') | Some('n') | Some('s')),
            'o' => rest("r"),
            'n' => rest("ot"),
            _ => false,
        };
        if !kw && (c as u32) < 128 && is_potential_ident_char(c) {
            return Err(format!("invalid {kind} literal"));
        }
        Ok(())
    }

    /// `tok_decimal_tail`, the first digit already consumed.
    fn decimal_tail(&mut self) -> Result<(), String> {
        loop {
            while self.peek(0).is_some_and(|c| c.is_ascii_digit()) {
                self.pos += 1;
            }
            if self.peek(0) != Some('_') {
                return Ok(());
            }
            self.pos += 1;
            if !self.peek(0).is_some_and(|c| c.is_ascii_digit()) {
                return Err("invalid decimal literal".into());
            }
        }
    }

    fn radix_digits(&mut self, radix: u32, kind: &str) -> Result<(), String> {
        loop {
            if self.peek(0) == Some('_') {
                self.pos += 1;
            }
            match self.peek(0) {
                Some(c) if c.is_digit(radix) => {}
                Some(c) if radix < 10 && c.is_ascii_digit() => {
                    return Err(format!("invalid digit '{c}' in {kind} literal"))
                }
                _ => return Err(format!("invalid {kind} literal")),
            }
            while self.peek(0).is_some_and(|c| c.is_digit(radix)) {
                self.pos += 1;
            }
            if self.peek(0) != Some('_') {
                break;
            }
        }
        if radix < 10 {
            if let Some(c) = self.peek(0).filter(|c| c.is_ascii_digit()) {
                return Err(format!("invalid digit '{c}' in {kind} literal"));
            }
        }
        self.verify_end(kind)
    }

    fn number(&mut self) -> Result<(), String> {
        let start = self.pos;
        let text = |lx: &Self, a: usize, b: usize| -> String {
            lx.chars[a..b].iter().filter(|c| **c != '_').collect()
        };
        let c0 = self.peek(0).unwrap_or('0');
        if c0 == '.' {
            self.pos += 1;
            return self.fraction(start, true);
        }
        if c0 == '0' {
            if let Some(x) = self.peek(1) {
                let radix = match x {
                    'x' | 'X' => Some((16, "hexadecimal")),
                    'o' | 'O' => Some((8, "octal")),
                    'b' | 'B' => Some((2, "binary")),
                    _ => None,
                };
                if let Some((radix, kind)) = radix {
                    self.pos += 2;
                    self.radix_digits(radix, kind)?;
                    let digits = text(self, start + 2, self.pos);
                    let v = BigInt::parse_bytes(digits.as_bytes(), radix).unwrap_or_default();
                    self.push(Tok::Number(Num::Int(v)));
                    return Ok(());
                }
            }
            // "0", "00", "0_0" … ; a later non-zero digit is an old-style octal
            self.pos += 1;
            loop {
                if self.peek(0) == Some('_') {
                    self.pos += 1;
                    if !self.peek(0).is_some_and(|c| c.is_ascii_digit()) {
                        return Err("invalid decimal literal".into());
                    }
                }
                if self.peek(0) != Some('0') {
                    break;
                }
                self.pos += 1;
            }
            let mut nonzero = false;
            if self.peek(0).is_some_and(|c| c.is_ascii_digit()) {
                nonzero = true;
                self.pos += 1;
                self.decimal_tail()?;
            }
            match self.peek(0) {
                Some('.') => {
                    self.pos += 1;
                    return self.fraction(start, false);
                }
                Some('e') | Some('E') => return self.exponent(start),
                Some('j') | Some('J') => return self.imaginary(start),
                _ if nonzero => {
                    return Err("leading zeros in decimal integer literals are not permitted; use an 0o prefix for octal integers".into())
                }
                _ => {}
            }
            self.verify_end("decimal")?;
            self.push(Tok::Number(Num::Int(BigInt::zero())));
            return Ok(());
        }
        self.pos += 1;
        self.decimal_tail()?;
        match self.peek(0) {
            Some('.') => {
                self.pos += 1;
                self.fraction(start, false)
            }
            Some('e') | Some('E') => self.exponent(start),
            Some('j') | Some('J') => self.imaginary(start),
            _ => {
                self.verify_end("decimal")?;
                let digits = text(self, start, self.pos);
                let v = BigInt::parse_bytes(digits.as_bytes(), 10).unwrap_or_default();
                self.push(Tok::Number(Num::Int(v)));
                Ok(())
            }
        }
    }

    fn fraction(&mut self, start: usize, _leading_dot: bool) -> Result<(), String> {
        if self.peek(0).is_some_and(|c| c.is_ascii_digit()) {
            self.pos += 1;
            self.decimal_tail()?;
        }
        match self.peek(0) {
            Some('e') | Some('E') => self.exponent(start),
            Some('j') | Some('J') => self.imaginary(start),
            _ => {
                self.verify_end("decimal")?;
                self.push_float(start, self.pos, false);
                Ok(())
            }
        }
    }

    fn exponent(&mut self, start: usize) -> Result<(), String> {
        let e_pos = self.pos;
        self.pos += 1; // the 'e'
        match self.peek(0) {
            Some('+') | Some('-') => {
                self.pos += 1;
                if !self.peek(0).is_some_and(|c| c.is_ascii_digit()) {
                    return Err("invalid decimal literal".into());
                }
            }
            Some(c) if c.is_ascii_digit() => {}
            _ => {
                // "1else": the number ends before the 'e'
                self.pos = e_pos;
                self.verify_end("decimal")?;
                self.push_number_text(start, e_pos);
                return Ok(());
            }
        }
        self.pos += 1;
        self.decimal_tail()?;
        if matches!(self.peek(0), Some('j') | Some('J')) {
            return self.imaginary(start);
        }
        self.verify_end("decimal")?;
        self.push_float(start, self.pos, false);
        Ok(())
    }

    fn imaginary(&mut self, start: usize) -> Result<(), String> {
        let end = self.pos;
        self.pos += 1; // the 'j'
        self.verify_end("imaginary")?;
        self.push_float(start, end, true);
        Ok(())
    }

    fn push_number_text(&mut self, start: usize, end: usize) {
        let t: String = self.chars[start..end]
            .iter()
            .filter(|c| **c != '_')
            .collect();
        if t.contains('.') {
            self.push_float(start, end, false);
        } else {
            let v = BigInt::parse_bytes(t.as_bytes(), 10).unwrap_or_default();
            self.push(Tok::Number(Num::Int(v)));
        }
    }

    fn push_float(&mut self, start: usize, end: usize, imag: bool) {
        let t: String = self.chars[start..end]
            .iter()
            .filter(|c| **c != '_')
            .collect();
        let v = t.parse::<f64>().unwrap_or(f64::NAN);
        self.push(Tok::Number(if imag { Num::Imag(v) } else { Num::Float(v) }));
    }

    fn string(&mut self, prefix: String) -> Result<(), String> {
        let p = prefix.to_ascii_lowercase();
        let raw = p.contains('r');
        let bytes = p.contains('b');
        let fstring = p.contains('f');
        let u = p == "u";
        let q = self.peek(0).unwrap_or('"');
        let triple = self.peek(1) == Some(q) && self.peek(2) == Some(q);
        let qn = if triple { 3 } else { 1 };
        self.pos += qn;
        let body_start = self.pos;
        loop {
            match self.peek(0) {
                None => {
                    return Err(if triple {
                        format!(
                            "unterminated triple-quoted string literal (detected at line {})",
                            self.line
                        )
                    } else {
                        format!(
                            "unterminated string literal (detected at line {})",
                            self.line
                        )
                    })
                }
                Some('\n') | Some('\r') if !triple => {
                    return Err(format!(
                        "unterminated string literal (detected at line {})",
                        self.line
                    ));
                }
                Some('\\') => {
                    if matches!(self.peek(1), Some('\n')) {
                        self.line += 1;
                    }
                    self.pos += 2;
                }
                Some(c)
                    if c == q
                        && (!triple || (self.peek(1) == Some(q) && self.peek(2) == Some(q))) =>
                {
                    break
                }
                Some(c) => {
                    if c == '\n' {
                        self.line += 1;
                    }
                    self.pos += 1;
                }
            }
        }
        let body: String = self.chars[body_start..self.pos].iter().collect();
        self.pos += qn;
        let value = if raw || fstring {
            body.clone()
        } else {
            decode_escapes(&body, bytes)
        };
        self.push(Tok::Str {
            value,
            bytes,
            fstring,
            u,
            raw,
        });
        Ok(())
    }
}

/// Backslash escapes of a non-raw string literal (unknown escapes stay as written, like Python).
fn decode_escapes(body: &str, bytes: bool) -> String {
    let chars: Vec<char> = body.chars().collect();
    let mut out = String::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c != '\\' || i + 1 >= chars.len() {
            out.push(c);
            i += 1;
            continue;
        }
        let n = chars[i + 1];
        i += 2;
        let simple = match n {
            '\n' => Some(None),
            '\\' => Some(Some('\\')),
            '\'' => Some(Some('\'')),
            '"' => Some(Some('"')),
            'a' => Some(Some('\x07')),
            'b' => Some(Some('\x08')),
            'f' => Some(Some('\x0c')),
            'n' => Some(Some('\n')),
            'r' => Some(Some('\r')),
            't' => Some(Some('\t')),
            'v' => Some(Some('\x0b')),
            _ => None,
        };
        if let Some(ch) = simple {
            if let Some(ch) = ch {
                out.push(ch);
            }
            continue;
        }
        let hex = |len: usize, i: usize| -> Option<u32> {
            if i + len > chars.len() {
                return None;
            }
            let s: String = chars[i..i + len].iter().collect();
            u32::from_str_radix(&s, 16).ok()
        };
        match n {
            '0'..='7' => {
                let mut v = n.to_digit(8).unwrap_or(0);
                let mut k = 0;
                while k < 2 && i < chars.len() && chars[i].is_digit(8) {
                    v = v * 8 + chars[i].to_digit(8).unwrap_or(0);
                    i += 1;
                    k += 1;
                }
                out.push(char::from_u32(v).unwrap_or('\u{fffd}'));
            }
            'x' => match hex(2, i) {
                Some(v) => {
                    out.push(char::from_u32(v).unwrap_or('\u{fffd}'));
                    i += 2;
                }
                None => out.push_str("\\x"),
            },
            'u' | 'U' if !bytes => {
                let len = if n == 'u' { 4 } else { 8 };
                match hex(len, i).and_then(char::from_u32) {
                    Some(ch) => {
                        out.push(ch);
                        i += len;
                    }
                    None => {
                        out.push('\\');
                        out.push(n);
                    }
                }
            }
            _ => {
                out.push('\\');
                out.push(n);
            }
        }
    }
    out
}

// ---------------------------------------------------------------- AST (the parts of Python's `ast` the calculator sees)

#[derive(Debug, Clone)]
enum Const {
    Int(BigInt),
    Float(f64),
    Imag(f64),
    Str(String, bool),
    Bytes(String),
    True,
    False,
    None,
    Ellipsis,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Ctx {
    Load,
    Store,
}

impl Ctx {
    fn dump(self) -> &'static str {
        match self {
            Ctx::Load => "Load()",
            Ctx::Store => "Store()",
        }
    }
}

#[derive(Debug, Clone)]
enum Node {
    Const(Const),
    Name(String, Ctx),
    /// left, operator class name (`Add`, `Pow`, …), right
    BinOp(Box<Node>, &'static str, Box<Node>),
    UnaryOp(&'static str, Box<Node>),
    Compare(Box<Node>, Vec<&'static str>, Vec<Node>),
    Call(Box<Node>, Vec<Node>, Vec<(Option<String>, Node)>),
    List(Vec<Node>, Ctx),
    Tuple(Vec<Node>, Ctx),
    Starred(Box<Node>, Ctx),
    Attribute(Box<Node>, String, Ctx),
    Subscript(Box<Node>, Box<Node>, Ctx),
    /// Any other node: never evaluated, only shown (its `ast.dump` text).
    Other(String),
}

fn py_str_repr(s: &str) -> String {
    crate::pyjson::py_repr(&Value::String(s.to_string()))
}

fn bytes_repr(s: &str) -> String {
    let quote = if s.contains('\'') && !s.contains('"') {
        '"'
    } else {
        '\''
    };
    let mut out = String::from("b");
    out.push(quote);
    for ch in s.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '\t' => out.push_str("\\t"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            c if c == quote => {
                out.push('\\');
                out.push(c);
            }
            c if (c as u32) < 0x20 || (c as u32) >= 0x7f => {
                out.push_str(&format!("\\x{:02x}", c as u32 & 0xff))
            }
            c => out.push(c),
        }
    }
    out.push(quote);
    out
}

fn dump_list(items: &[Node]) -> String {
    format!(
        "[{}]",
        items.iter().map(Node::dump).collect::<Vec<_>>().join(", ")
    )
}

impl Const {
    fn dump(&self) -> String {
        let (value, kind) = match self {
            Const::Int(i) => (i.to_string(), ""),
            Const::Float(f) => (float_repr(*f), ""),
            Const::Imag(f) => (complex_repr(0.0, *f), ""),
            Const::Str(s, u) => (py_str_repr(s), if *u { ", kind='u'" } else { "" }),
            Const::Bytes(b) => (bytes_repr(b), ""),
            Const::True => ("True".into(), ""),
            Const::False => ("False".into(), ""),
            Const::None => ("None".into(), ""),
            Const::Ellipsis => ("Ellipsis".into(), ""),
        };
        format!("Constant(value={value}{kind})")
    }
}

impl Node {
    /// `ast.dump(node)` (CPython 3.12 layout).
    fn dump(&self) -> String {
        match self {
            Node::Const(c) => c.dump(),
            Node::Name(id, ctx) => format!("Name(id={}, ctx={})", py_str_repr(id), ctx.dump()),
            Node::BinOp(l, op, r) => {
                format!("BinOp(left={}, op={}(), right={})", l.dump(), op, r.dump())
            }
            Node::UnaryOp(op, v) => format!("UnaryOp(op={}(), operand={})", op, v.dump()),
            Node::Compare(l, ops, cs) => format!(
                "Compare(left={}, ops=[{}], comparators={})",
                l.dump(),
                ops.iter()
                    .map(|o| format!("{o}()"))
                    .collect::<Vec<_>>()
                    .join(", "),
                dump_list(cs)
            ),
            Node::Call(f, args, kws) => format!(
                "Call(func={}, args={}, keywords=[{}])",
                f.dump(),
                dump_list(args),
                kws.iter()
                    .map(|(k, v)| match k {
                        Some(k) => format!("keyword(arg={}, value={})", py_str_repr(k), v.dump()),
                        None => format!("keyword(value={})", v.dump()),
                    })
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            Node::List(e, ctx) => format!("List(elts={}, ctx={})", dump_list(e), ctx.dump()),
            Node::Tuple(e, ctx) => format!("Tuple(elts={}, ctx={})", dump_list(e), ctx.dump()),
            Node::Starred(v, ctx) => format!("Starred(value={}, ctx={})", v.dump(), ctx.dump()),
            Node::Attribute(v, a, ctx) => {
                format!(
                    "Attribute(value={}, attr={}, ctx={})",
                    v.dump(),
                    py_str_repr(a),
                    ctx.dump()
                )
            }
            Node::Subscript(v, s, ctx) => format!(
                "Subscript(value={}, slice={}, ctx={})",
                v.dump(),
                s.dump(),
                ctx.dump()
            ),
            Node::Other(s) => s.clone(),
        }
    }

    /// The node as an assignment target (comprehension `for` targets, `:=`).
    fn into_store(self) -> Node {
        match self {
            Node::Name(n, _) => Node::Name(n, Ctx::Store),
            Node::List(e, _) => {
                Node::List(e.into_iter().map(Node::into_store).collect(), Ctx::Store)
            }
            Node::Tuple(e, _) => {
                Node::Tuple(e.into_iter().map(Node::into_store).collect(), Ctx::Store)
            }
            Node::Starred(v, _) => Node::Starred(Box::new(v.into_store()), Ctx::Store),
            Node::Attribute(v, a, _) => Node::Attribute(v, a, Ctx::Store),
            Node::Subscript(v, s, _) => Node::Subscript(v, s, Ctx::Store),
            other => other,
        }
    }
}

// ---------------------------------------------------------------- parser (Python 3.12 expression grammar, `eval` mode)

#[derive(Debug, Clone)]
enum PErr {
    /// "invalid syntax"
    Generic,
    /// A specific SyntaxError message.
    Msg(String),
}

type PR<T> = Result<T, PErr>;

const SOFT_KEYWORDS: [&str; 4] = ["_", "case", "match", "type"];

struct Parser {
    toks: Vec<Token>,
    pos: usize,
    /// > 0 while speculatively parsing (CPython's `expression_without_invalid`): no specific errors.
    quiet: usize,
}

fn is_keyword(s: &str) -> bool {
    KEYWORDS.contains(&s)
}

impl Parser {
    fn tok(&self) -> &Tok {
        &self.toks[self.pos.min(self.toks.len() - 1)].tok
    }

    fn tok_at(&self, off: usize) -> &Tok {
        &self.toks[(self.pos + off).min(self.toks.len() - 1)].tok
    }

    fn bump(&mut self) {
        if self.pos < self.toks.len() - 1 {
            self.pos += 1;
        }
    }

    fn at_op(&self, op: &str) -> bool {
        matches!(self.tok(), Tok::Op(o) if *o == op)
    }

    fn at_kw(&self, kw: &str) -> bool {
        matches!(self.tok(), Tok::Name(n) if n == kw)
    }

    fn eat_op(&mut self, op: &str) -> bool {
        if self.at_op(op) {
            self.bump();
            true
        } else {
            false
        }
    }

    fn expect_op(&mut self, op: &str) -> PR<()> {
        if self.eat_op(op) {
            Ok(())
        } else {
            Err(PErr::Generic)
        }
    }

    fn specific<T>(&self, msg: &str) -> PR<T> {
        if self.quiet > 0 {
            Err(PErr::Generic)
        } else {
            Err(PErr::Msg(msg.to_string()))
        }
    }

    fn level_before(&self) -> usize {
        if self.pos == 0 {
            0
        } else {
            self.toks[self.pos - 1].level
        }
    }

    /// Speculatively parse `f` from the current position; the position is restored.
    fn try_parse<T>(&mut self, f: impl FnOnce(&mut Self) -> PR<T>) -> Option<(T, usize)> {
        let save = self.pos;
        self.quiet += 1;
        let r = f(self);
        self.quiet -= 1;
        let end_level = self.level_before();
        self.pos = save;
        r.ok().map(|v| (v, end_level))
    }

    /// eval: expressions NEWLINE* ENDMARKER
    fn parse_eval(&mut self) -> PR<Node> {
        let body = self.expressions(false)?;
        while matches!(self.tok(), Tok::Newline) {
            self.bump();
        }
        if !matches!(self.tok(), Tok::End) {
            return Err(PErr::Generic);
        }
        Ok(body)
    }

    /// expressions: expression (',' expression)* [','] ; with `star`: star_expressions.
    fn expressions(&mut self, star: bool) -> PR<Node> {
        let first = if star {
            self.star_expression()?
        } else {
            self.expression()?
        };
        if !self.at_op(",") {
            return Ok(first);
        }
        let mut elts = vec![first];
        while self.eat_op(",") {
            if !self.starts_expression(star) {
                break;
            }
            elts.push(if star {
                self.star_expression()?
            } else {
                self.expression()?
            });
        }
        Ok(Node::Tuple(elts, Ctx::Load))
    }

    fn star_expression(&mut self) -> PR<Node> {
        if self.eat_op("*") {
            let v = self.bitor()?;
            return Ok(Node::Starred(Box::new(v), Ctx::Load));
        }
        self.expression()
    }

    /// Can the current token begin an expression?
    fn starts_expression(&self, star: bool) -> bool {
        match self.tok() {
            Tok::Name(n) => {
                !is_keyword(n)
                    || matches!(
                        n.as_str(),
                        "not" | "lambda" | "await" | "True" | "False" | "None"
                    )
            }
            Tok::Number(_) | Tok::Str { .. } => true,
            Tok::Op(o) => {
                matches!(*o, "(" | "[" | "{" | "-" | "+" | "~" | "...") || (star && *o == "*")
            }
            _ => false,
        }
    }

    /// expression: invalid_expression | invalid_legacy_expression | disjunction 'if' disjunction 'else' expression
    ///           | disjunction | lambdef
    fn expression(&mut self) -> PR<Node> {
        if self.at_kw("lambda") {
            return self.lambdef();
        }
        let start = self.pos;
        let start_tok = self.tok().clone();
        let a = self.disjunction()?;
        if self.at_kw("if") {
            self.bump();
            let test = self.disjunction()?;
            if !self.at_kw("else") {
                if self.at_op(":") {
                    return Err(PErr::Generic);
                }
                return self.specific("expected 'else' after 'if' expression");
            }
            self.bump();
            let orelse = self.expression()?;
            return Ok(Node::Other(format!(
                "IfExp(test={}, body={}, orelse={})",
                test.dump(),
                a.dump(),
                orelse.dump()
            )));
        }
        if self.quiet == 0 && self.starts_expression(false) {
            let single_name = self.pos == start + 1 && matches!(start_tok, Tok::Name(_));
            let legacy = match (&start_tok, single_name) {
                (Tok::Name(n), true) if n == "print" || n == "exec" => Some(n.clone()),
                _ => None,
            };
            let name_then_string = single_name && matches!(self.tok(), Tok::Str { .. });
            let soft = matches!(&start_tok, Tok::Name(n) if SOFT_KEYWORDS.contains(&n.as_str()));
            if legacy.is_none() && !name_then_string && !soft {
                if let Some((_, level)) = self.try_parse(|p| p.expression()) {
                    if level > 0 {
                        return self.specific("invalid syntax. Perhaps you forgot a comma?");
                    }
                }
            }
            if let Some(n) = legacy {
                if !self.at_op("(") && self.try_parse(|p| p.expressions(true)).is_some() {
                    return self.specific(&format!(
                        "Missing parentheses in call to '{n}'. Did you mean {n}(...)?"
                    ));
                }
            }
        }
        Ok(a)
    }

    fn disjunction(&mut self) -> PR<Node> {
        let first = self.conjunction()?;
        if !self.at_kw("or") {
            return Ok(first);
        }
        let mut values = vec![first];
        while self.at_kw("or") {
            self.bump();
            values.push(self.conjunction()?);
        }
        Ok(Node::Other(format!(
            "BoolOp(op=Or(), values={})",
            dump_list(&values)
        )))
    }

    fn conjunction(&mut self) -> PR<Node> {
        let first = self.inversion()?;
        if !self.at_kw("and") {
            return Ok(first);
        }
        let mut values = vec![first];
        while self.at_kw("and") {
            self.bump();
            values.push(self.inversion()?);
        }
        Ok(Node::Other(format!(
            "BoolOp(op=And(), values={})",
            dump_list(&values)
        )))
    }

    fn inversion(&mut self) -> PR<Node> {
        let mut nots = 0;
        while self.at_kw("not") {
            self.bump();
            nots += 1;
        }
        let mut node = self.comparison()?;
        for _ in 0..nots {
            node = Node::UnaryOp("Not", Box::new(node));
        }
        Ok(node)
    }

    fn compare_op(&mut self) -> Option<&'static str> {
        let op = match self.tok() {
            Tok::Op("==") => "Eq",
            Tok::Op("!=") => "NotEq",
            Tok::Op("<") => "Lt",
            Tok::Op("<=") => "LtE",
            Tok::Op(">") => "Gt",
            Tok::Op(">=") => "GtE",
            Tok::Name(n) if n == "in" => "In",
            Tok::Name(n) if n == "is" => {
                if matches!(self.tok_at(1), Tok::Name(m) if m == "not") {
                    self.bump();
                    "IsNot"
                } else {
                    "Is"
                }
            }
            Tok::Name(n) if n == "not" && matches!(self.tok_at(1), Tok::Name(m) if m == "in") => {
                self.bump();
                "NotIn"
            }
            _ => return None,
        };
        self.bump();
        Some(op)
    }

    fn comparison(&mut self) -> PR<Node> {
        let left = self.bitor()?;
        let mut ops = Vec::new();
        let mut comps = Vec::new();
        while let Some(op) = self.compare_op() {
            ops.push(op);
            comps.push(self.bitor()?);
        }
        if ops.is_empty() {
            Ok(left)
        } else {
            Ok(Node::Compare(Box::new(left), ops, comps))
        }
    }

    fn bitor(&mut self) -> PR<Node> {
        self.binary(1)
    }

    fn binop_info(&self) -> Option<(&'static str, u8)> {
        match self.tok() {
            Tok::Op("|") => Some(("BitOr", 1)),
            Tok::Op("^") => Some(("BitXor", 2)),
            Tok::Op("&") => Some(("BitAnd", 3)),
            Tok::Op("<<") => Some(("LShift", 4)),
            Tok::Op(">>") => Some(("RShift", 4)),
            Tok::Op("+") => Some(("Add", 5)),
            Tok::Op("-") => Some(("Sub", 5)),
            Tok::Op("*") => Some(("Mult", 6)),
            Tok::Op("/") => Some(("Div", 6)),
            Tok::Op("//") => Some(("FloorDiv", 6)),
            Tok::Op("%") => Some(("Mod", 6)),
            Tok::Op("@") => Some(("MatMult", 6)),
            _ => None,
        }
    }

    /// Left-associative binary operators by precedence climbing (bitwise_or … term).
    fn binary(&mut self, min_prec: u8) -> PR<Node> {
        let mut left = self.factor()?;
        while let Some((op, prec)) = self.binop_info() {
            if prec < min_prec {
                break;
            }
            self.bump();
            let right = self.binary(prec + 1)?;
            left = Node::BinOp(Box::new(left), op, Box::new(right));
        }
        Ok(left)
    }

    /// factor: ('+' | '-' | '~') factor | power
    fn factor(&mut self) -> PR<Node> {
        let mut ops = Vec::new();
        loop {
            let op = match self.tok() {
                Tok::Op("-") => "USub",
                Tok::Op("+") => "UAdd",
                Tok::Op("~") => "Invert",
                _ => break,
            };
            ops.push(op);
            self.bump();
        }
        let mut node = self.power()?;
        for op in ops.into_iter().rev() {
            node = Node::UnaryOp(op, Box::new(node));
        }
        Ok(node)
    }

    /// power: await_primary '**' factor | await_primary
    fn power(&mut self) -> PR<Node> {
        let base = if self.at_kw("await") {
            self.bump();
            let v = self.primary()?;
            Node::Other(format!("Await(value={})", v.dump()))
        } else {
            self.primary()?
        };
        if self.eat_op("**") {
            let exp = self.factor()?;
            return Ok(Node::BinOp(Box::new(base), "Pow", Box::new(exp)));
        }
        Ok(base)
    }

    fn primary(&mut self) -> PR<Node> {
        let mut node = self.atom()?;
        loop {
            if self.eat_op(".") {
                match self.tok().clone() {
                    Tok::Name(n) if !is_keyword(&n) => {
                        self.bump();
                        node = Node::Attribute(Box::new(node), n, Ctx::Load);
                    }
                    _ => return Err(PErr::Generic),
                }
            } else if self.at_op("(") {
                self.bump();
                node = self.call_args(node)?;
            } else if self.at_op("[") {
                self.bump();
                let s = self.slices()?;
                self.expect_op("]")?;
                node = Node::Subscript(Box::new(node), Box::new(s), Ctx::Load);
            } else {
                return Ok(node);
            }
        }
    }

    /// The arguments of a call, after '('.
    fn call_args(&mut self, func: Node) -> PR<Node> {
        let mut args: Vec<Node> = Vec::new();
        let mut kws: Vec<(Option<String>, Node)> = Vec::new();
        let mut seen_kw = false;
        let mut seen_kw_unpack = false;
        let mut genexp = false;
        let mut count = 0usize;
        while !self.at_op(")") {
            count += 1;
            if self.at_op("*") {
                self.bump();
                if self.at_op(")") || self.at_op(",") {
                    return self.specific("Invalid star expression");
                }
                let v = self.expression()?;
                args.push(Node::Starred(Box::new(v), Ctx::Load));
            } else if self.at_op("**") {
                self.bump();
                let v = self.expression()?;
                kws.push((None, v));
                seen_kw_unpack = true;
            } else if matches!(self.tok(), Tok::Name(n) if !is_keyword(n))
                && matches!(self.tok_at(1), Tok::Op("="))
            {
                let name = match self.tok().clone() {
                    Tok::Name(n) => n,
                    _ => String::new(),
                };
                self.bump();
                self.bump();
                if self.at_op(",") || self.at_op(")") {
                    return self.specific("expected argument value expression");
                }
                let v = self.expression()?;
                kws.push((Some(name), v));
                seen_kw = true;
            } else {
                let e = self.named_expression()?;
                if self.at_op("=") {
                    return self.specific(
                        "expression cannot contain assignment, perhaps you meant \"==\"?",
                    );
                }
                let e = if self.at_kw("for")
                    || (self.at_kw("async") && matches!(self.tok_at(1), Tok::Name(n) if n == "for"))
                {
                    let gens = self.comprehensions()?;
                    genexp = true;
                    Node::Other(format!(
                        "GeneratorExp(elt={}, generators={})",
                        e.dump(),
                        gens
                    ))
                } else {
                    e
                };
                if seen_kw_unpack {
                    return self.specific("positional argument follows keyword argument unpacking");
                }
                if seen_kw {
                    return self.specific("positional argument follows keyword argument");
                }
                args.push(e);
            }
            if genexp && count > 1 {
                return self.specific("Generator expression must be parenthesized");
            }
            if !self.eat_op(",") {
                break;
            }
            if genexp {
                return self.specific("Generator expression must be parenthesized");
            }
        }
        self.expect_op(")")?;
        Ok(Node::Call(Box::new(func), args, kws))
    }

    /// slices: slice !',' | ','.(slice | starred_expression)+ [',']
    fn slices(&mut self) -> PR<Node> {
        let first = self.slice_item()?;
        if !self.at_op(",") {
            return Ok(first);
        }
        let mut elts = vec![first];
        while self.eat_op(",") {
            if self.at_op("]") {
                break;
            }
            elts.push(self.slice_item()?);
        }
        Ok(Node::Tuple(elts, Ctx::Load))
    }

    fn slice_item(&mut self) -> PR<Node> {
        if self.eat_op("*") {
            let v = self.bitor()?;
            return Ok(Node::Starred(Box::new(v), Ctx::Load));
        }
        let lower = if self.at_op(":") {
            None
        } else {
            Some(self.named_expression()?)
        };
        if !self.eat_op(":") {
            return lower.ok_or(PErr::Generic);
        }
        let upper = if self.at_op(":") || self.at_op("]") || self.at_op(",") {
            None
        } else {
            Some(self.expression()?)
        };
        let mut step = None;
        if self.eat_op(":") && !(self.at_op("]") || self.at_op(",")) {
            step = Some(self.expression()?);
        }
        let mut fields = Vec::new();
        if let Some(l) = lower {
            fields.push(format!("lower={}", l.dump()));
        }
        if let Some(u) = upper {
            fields.push(format!("upper={}", u.dump()));
        }
        if let Some(s) = step {
            fields.push(format!("step={}", s.dump()));
        }
        Ok(Node::Other(format!("Slice({})", fields.join(", "))))
    }

    /// named_expression: NAME ':=' expression | expression !':='
    fn named_expression(&mut self) -> PR<Node> {
        if let (Tok::Name(n), Tok::Op(":=")) = (self.tok().clone(), self.tok_at(1).clone()) {
            if !is_keyword(&n) {
                self.bump();
                self.bump();
                let v = self.expression()?;
                return Ok(Node::Other(format!(
                    "NamedExpr(target={}, value={})",
                    Node::Name(n, Ctx::Store).dump(),
                    v.dump()
                )));
            }
        }
        let e = self.expression()?;
        if self.at_op(":=") {
            return Err(PErr::Generic);
        }
        Ok(e)
    }

    /// star_named_expression: '*' bitwise_or | named_expression ; returns (node, starred?)
    fn star_named(&mut self) -> PR<(Node, bool)> {
        if self.eat_op("*") {
            let v = self.bitor()?;
            return Ok((Node::Starred(Box::new(v), Ctx::Load), true));
        }
        Ok((self.named_expression()?, false))
    }

    fn at_for(&self) -> bool {
        self.at_kw("for")
            || (self.at_kw("async") && matches!(self.tok_at(1), Tok::Name(n) if n == "for"))
    }

    /// for_if_clauses, dumped as the `generators=[...]` list.
    fn comprehensions(&mut self) -> PR<String> {
        let mut gens = Vec::new();
        while self.at_for() {
            let is_async = if self.at_kw("async") {
                self.bump();
                1
            } else {
                0
            };
            self.bump(); // for
            let target = self.star_targets()?;
            if !self.at_kw("in") {
                return Err(PErr::Generic);
            }
            self.bump();
            let iter = self.disjunction()?;
            let mut ifs = Vec::new();
            while self.at_kw("if") {
                self.bump();
                ifs.push(self.disjunction()?);
            }
            gens.push(format!(
                "comprehension(target={}, iter={}, ifs={}, is_async={})",
                target.dump(),
                iter.dump(),
                dump_list(&ifs),
                is_async
            ));
        }
        Ok(format!("[{}]", gens.join(", ")))
    }

    fn star_targets(&mut self) -> PR<Node> {
        let one = |p: &mut Self| -> PR<Node> {
            if p.eat_op("*") {
                let v = p.bitor()?;
                Ok(Node::Starred(Box::new(v), Ctx::Load))
            } else {
                p.bitor()
            }
        };
        let first = one(self)?;
        if !self.at_op(",") {
            return Ok(first.into_store());
        }
        let mut elts = vec![first];
        while self.eat_op(",") {
            if self.at_kw("in") {
                break;
            }
            elts.push(one(self)?);
        }
        Ok(Node::Tuple(elts, Ctx::Load).into_store())
    }

    fn lambdef(&mut self) -> PR<Node> {
        self.bump(); // lambda
        let mut posonly: Vec<String> = Vec::new();
        let mut args: Vec<String> = Vec::new();
        let mut defaults: Vec<Node> = Vec::new();
        let mut vararg: Option<String> = None;
        let mut kwonly: Vec<String> = Vec::new();
        let mut kw_defaults: Vec<Option<Node>> = Vec::new();
        let mut kwarg: Option<String> = None;
        let mut star_seen = false;
        let name = |p: &mut Self| -> PR<String> {
            match p.tok().clone() {
                Tok::Name(n) if !is_keyword(&n) => {
                    p.bump();
                    Ok(n)
                }
                _ => Err(PErr::Generic),
            }
        };
        while !self.at_op(":") {
            if self.eat_op("/") {
                posonly.append(&mut args);
            } else if self.eat_op("**") {
                kwarg = Some(name(self)?);
            } else if self.eat_op("*") {
                star_seen = true;
                if !self.at_op(",") && !self.at_op(":") {
                    vararg = Some(name(self)?);
                }
            } else {
                let n = name(self)?;
                let d = if self.eat_op("=") {
                    Some(self.expression()?)
                } else {
                    None
                };
                if star_seen {
                    kwonly.push(n);
                    kw_defaults.push(d);
                } else {
                    args.push(n);
                    if let Some(d) = d {
                        defaults.push(d);
                    }
                }
            }
            if !self.eat_op(",") {
                break;
            }
        }
        self.expect_op(":")?;
        let body = self.expression()?;
        let arg = |n: &String| format!("arg(arg={})", py_str_repr(n));
        let list = |v: &[String]| format!("[{}]", v.iter().map(arg).collect::<Vec<_>>().join(", "));
        let mut fields = vec![
            format!("posonlyargs={}", list(&posonly)),
            format!("args={}", list(&args)),
        ];
        if let Some(v) = &vararg {
            fields.push(format!("vararg={}", arg(v)));
        }
        fields.push(format!("kwonlyargs={}", list(&kwonly)));
        fields.push(format!(
            "kw_defaults=[{}]",
            kw_defaults
                .iter()
                .map(|d| d.as_ref().map(Node::dump).unwrap_or_else(|| "None".into()))
                .collect::<Vec<_>>()
                .join(", ")
        ));
        if let Some(k) = &kwarg {
            fields.push(format!("kwarg={}", arg(k)));
        }
        fields.push(format!("defaults={}", dump_list(&defaults)));
        Ok(Node::Other(format!(
            "Lambda(args=arguments({}), body={})",
            fields.join(", "),
            body.dump()
        )))
    }

    fn atom(&mut self) -> PR<Node> {
        match self.tok().clone() {
            Tok::Name(n) => {
                let node = match n.as_str() {
                    "True" => Node::Const(Const::True),
                    "False" => Node::Const(Const::False),
                    "None" => Node::Const(Const::None),
                    k if is_keyword(k) => return Err(PErr::Generic),
                    _ => Node::Name(n.clone(), Ctx::Load),
                };
                self.bump();
                Ok(node)
            }
            Tok::Number(num) => {
                self.bump();
                Ok(Node::Const(match num {
                    Num::Int(i) => Const::Int(i),
                    Num::Float(f) => Const::Float(f),
                    Num::Imag(f) => Const::Imag(f),
                }))
            }
            Tok::Str { .. } => self.strings(),
            Tok::Op("...") => {
                self.bump();
                Ok(Node::Const(Const::Ellipsis))
            }
            Tok::Op("(") => {
                self.bump();
                self.paren()
            }
            Tok::Op("[") => {
                self.bump();
                self.bracket()
            }
            Tok::Op("{") => {
                self.bump();
                self.brace()
            }
            _ => Err(PErr::Generic),
        }
    }

    fn paren(&mut self) -> PR<Node> {
        if self.eat_op(")") {
            return Ok(Node::Tuple(Vec::new(), Ctx::Load));
        }
        if self.at_kw("yield") {
            self.bump();
            let node = if self.at_kw("from") {
                self.bump();
                let v = self.expression()?;
                format!("YieldFrom(value={})", v.dump())
            } else if self.at_op(")") {
                "Yield()".to_string()
            } else {
                let v = self.expressions(true)?;
                format!("Yield(value={})", v.dump())
            };
            self.expect_op(")")?;
            return Ok(Node::Other(node));
        }
        let (first, starred) = self.star_named()?;
        if !starred && self.at_for() {
            let gens = self.comprehensions()?;
            self.expect_op(")")?;
            return Ok(Node::Other(format!(
                "GeneratorExp(elt={}, generators={})",
                first.dump(),
                gens
            )));
        }
        if self.eat_op(")") {
            if starred {
                return self.specific("cannot use starred expression here");
            }
            return Ok(first);
        }
        if !self.at_op(",") {
            return Err(PErr::Generic);
        }
        let mut elts = vec![first];
        while self.eat_op(",") {
            if self.at_op(")") {
                break;
            }
            elts.push(self.star_named()?.0);
        }
        self.expect_op(")")?;
        Ok(Node::Tuple(elts, Ctx::Load))
    }

    fn bracket(&mut self) -> PR<Node> {
        if self.eat_op("]") {
            return Ok(Node::List(Vec::new(), Ctx::Load));
        }
        let (first, starred) = self.star_named()?;
        if !starred && self.at_for() {
            let gens = self.comprehensions()?;
            self.expect_op("]")?;
            return Ok(Node::Other(format!(
                "ListComp(elt={}, generators={})",
                first.dump(),
                gens
            )));
        }
        let mut elts = vec![first];
        while self.eat_op(",") {
            if self.at_op("]") {
                break;
            }
            elts.push(self.star_named()?.0);
        }
        self.expect_op("]")?;
        Ok(Node::List(elts, Ctx::Load))
    }

    fn brace(&mut self) -> PR<Node> {
        if self.eat_op("}") {
            return Ok(Node::Other("Dict(keys=[], values=[])".into()));
        }
        let mut keys: Vec<String> = Vec::new();
        let mut values: Vec<String> = Vec::new();
        let is_dict;
        let mut set_elts: Vec<Node> = Vec::new();
        if self.eat_op("**") {
            let v = self.bitor()?;
            keys.push("None".into());
            values.push(v.dump());
            is_dict = true;
        } else {
            let (first, starred) = self.star_named()?;
            if !starred && self.eat_op(":") {
                let v = self.expression()?;
                if self.at_for() {
                    let gens = self.comprehensions()?;
                    self.expect_op("}")?;
                    return Ok(Node::Other(format!(
                        "DictComp(key={}, value={}, generators={})",
                        first.dump(),
                        v.dump(),
                        gens
                    )));
                }
                keys.push(first.dump());
                values.push(v.dump());
                is_dict = true;
            } else {
                if !starred && self.at_for() {
                    let gens = self.comprehensions()?;
                    self.expect_op("}")?;
                    return Ok(Node::Other(format!(
                        "SetComp(elt={}, generators={})",
                        first.dump(),
                        gens
                    )));
                }
                set_elts.push(first);
                is_dict = false;
            }
        }
        while self.eat_op(",") {
            if self.at_op("}") {
                break;
            }
            if is_dict {
                if self.eat_op("**") {
                    let v = self.bitor()?;
                    keys.push("None".into());
                    values.push(v.dump());
                } else {
                    let k = self.expression()?;
                    self.expect_op(":")?;
                    let v = self.expression()?;
                    keys.push(k.dump());
                    values.push(v.dump());
                }
            } else {
                set_elts.push(self.star_named()?.0);
            }
        }
        self.expect_op("}")?;
        if is_dict {
            Ok(Node::Other(format!(
                "Dict(keys=[{}], values=[{}])",
                keys.join(", "),
                values.join(", ")
            )))
        } else {
            Ok(Node::Other(format!("Set(elts={})", dump_list(&set_elts))))
        }
    }

    /// One or more adjacent string literals (implicit concatenation).
    fn strings(&mut self) -> PR<Node> {
        let mut parts: Vec<(String, bool, bool, bool, bool)> = Vec::new();
        while let Tok::Str {
            value,
            bytes,
            fstring,
            u,
            raw,
        } = self.tok().clone()
        {
            parts.push((value, bytes, fstring, u, raw));
            self.bump();
        }
        let any_bytes = parts.iter().any(|p| p.1);
        if any_bytes && parts.iter().any(|p| !p.1) {
            return self.specific("cannot mix bytes and nonbytes literals");
        }
        let u = parts.first().map(|p| p.3).unwrap_or(false);
        if any_bytes {
            let joined: String = parts.iter().map(|p| p.0.as_str()).collect();
            if !joined.is_ascii() {
                return self.specific("bytes can only contain ASCII literal characters");
            }
            return Ok(Node::Const(Const::Bytes(joined)));
        }
        if !parts.iter().any(|p| p.2) {
            let joined: String = parts.iter().map(|p| p.0.as_str()).collect();
            return Ok(Node::Const(Const::Str(joined, u)));
        }
        // f-string: JoinedStr(values=[Constant | FormattedValue, …]) with adjacent constants merged
        let mut values: Vec<String> = Vec::new();
        let mut pending = String::new();
        for (value, _, fstring, _, raw) in &parts {
            if !fstring {
                pending.push_str(value);
                continue;
            }
            for piece in fstring_pieces(value, *raw)? {
                match piece {
                    FPiece::Text(t) => pending.push_str(&t),
                    FPiece::Field(dump) => {
                        if !pending.is_empty() {
                            values.push(Const::Str(std::mem::take(&mut pending), false).dump());
                        }
                        values.push(dump);
                    }
                }
            }
        }
        if !pending.is_empty() {
            values.push(Const::Str(pending, false).dump());
        }
        Ok(Node::Other(format!(
            "JoinedStr(values=[{}])",
            values.join(", ")
        )))
    }
}

enum FPiece {
    Text(String),
    /// The `FormattedValue(...)` dump of a replacement field.
    Field(String),
}

/// Split the body of an f-string into literal text and replacement fields.
fn fstring_pieces(body: &str, raw: bool) -> PR<Vec<FPiece>> {
    let chars: Vec<char> = body.chars().collect();
    let mut out = Vec::new();
    let mut text = String::new();
    let mut i = 0;
    let flush = |text: &mut String, out: &mut Vec<FPiece>| {
        if !text.is_empty() {
            let t = if raw {
                std::mem::take(text)
            } else {
                decode_escapes(&std::mem::take(text), false)
            };
            out.push(FPiece::Text(t));
        }
    };
    while i < chars.len() {
        let c = chars[i];
        if c == '{' && chars.get(i + 1) == Some(&'{') {
            text.push('{');
            i += 2;
        } else if c == '}' && chars.get(i + 1) == Some(&'}') {
            text.push('}');
            i += 2;
        } else if c == '}' {
            return Err(PErr::Msg("f-string: single '}' is not allowed".into()));
        } else if c == '{' {
            flush(&mut text, &mut out);
            // find the end of the expression part
            let mut depth = 0i32;
            let mut j = i + 1;
            let mut quote: Option<char> = None;
            let mut expr_end = None;
            while j < chars.len() {
                let d = chars[j];
                if let Some(q) = quote {
                    if d == q {
                        quote = None;
                    }
                } else if d == '\'' || d == '"' {
                    quote = Some(d);
                } else if "([{".contains(d) {
                    depth += 1;
                } else if ")]}".contains(d) {
                    if depth == 0 {
                        break;
                    }
                    depth -= 1;
                } else if depth == 0 && (d == ':' || (d == '!' && chars.get(j + 1) != Some(&'='))) {
                    break;
                }
                j += 1;
            }
            if j >= chars.len() {
                return Err(PErr::Msg("f-string: expecting '}'".into()));
            }
            expr_end.get_or_insert(j);
            let expr_src: String = chars[i + 1..j].iter().collect();
            let expr = parse_source(expr_src.trim())
                .map_err(|_| PErr::Msg("f-string: invalid syntax".into()))?;
            let mut conversion = -1i32;
            if chars[j] == '!' {
                conversion = match chars.get(j + 1) {
                    Some('r') => 114,
                    Some('s') => 115,
                    Some('a') => 97,
                    _ => {
                        return Err(PErr::Msg(
                            "f-string: invalid conversion character: expected 's', 'r', or 'a'"
                                .into(),
                        ))
                    }
                };
                j += 2;
            }
            let mut spec: Option<String> = None;
            if j < chars.len() && chars[j] == ':' {
                let mut k = j + 1;
                let mut d = 0;
                while k < chars.len() {
                    if chars[k] == '{' {
                        d += 1;
                    } else if chars[k] == '}' {
                        if d == 0 {
                            break;
                        }
                        d -= 1;
                    }
                    k += 1;
                }
                let spec_src: String = chars[j + 1..k.min(chars.len())].iter().collect();
                let pieces = fstring_pieces(&spec_src, raw)?;
                let vals: Vec<String> = pieces
                    .into_iter()
                    .map(|p| match p {
                        FPiece::Text(t) => Const::Str(t, false).dump(),
                        FPiece::Field(f) => f,
                    })
                    .collect();
                spec = Some(format!("JoinedStr(values=[{}])", vals.join(", ")));
                j = k;
            }
            if j >= chars.len() || chars[j] != '}' {
                return Err(PErr::Msg("f-string: expecting '}'".into()));
            }
            let mut dump = format!(
                "FormattedValue(value={}, conversion={}",
                expr.dump(),
                conversion
            );
            if let Some(s) = spec {
                dump.push_str(&format!(", format_spec={s}"));
            }
            dump.push(')');
            out.push(FPiece::Field(dump));
            i = j + 1;
        } else {
            text.push(c);
            i += 1;
        }
    }
    flush(&mut text, &mut out);
    Ok(out)
}

/// `ast.parse(s, mode="eval").body`, or the SyntaxError message.
fn parse_source(s: &str) -> Result<Node, String> {
    let toks = tokenize(s)?;
    // an unclosed bracket at the end of the input wins over parser errors
    let mut stack: Vec<char> = Vec::new();
    for t in &toks {
        match t.tok {
            Tok::Op("(") => stack.push('('),
            Tok::Op("[") => stack.push('['),
            Tok::Op("{") => stack.push('{'),
            Tok::Op(")") | Tok::Op("]") | Tok::Op("}") => {
                stack.pop();
            }
            _ => {}
        }
    }
    if let Some(open) = stack.last() {
        return Err(format!("'{open}' was never closed"));
    }
    let mut p = Parser {
        toks,
        pos: 0,
        quiet: 0,
    };
    p.parse_eval().map_err(|e| match e {
        PErr::Generic => "invalid syntax".to_string(),
        PErr::Msg(m) => m,
    })
}

// ---------------------------------------------------------------- Python operators on values

fn op_symbol(op: &str) -> &'static str {
    match op {
        "Add" => "+",
        "Sub" => "-",
        "Mult" => "*",
        "Div" => "/",
        "FloorDiv" => "//",
        "Mod" => "%",
        _ => "** or pow()",
    }
}

fn unsupported<T>(op: &str, a: &Val, b: &Val) -> R<T> {
    type_err(format!(
        "unsupported operand type(s) for {}: '{}' and '{}'",
        op_symbol(op),
        a.type_name(),
        b.type_name()
    ))
}

/// Python float `divmod` (CPython `float_divmod`): (floor quotient, remainder).
fn float_divmod(vx: f64, wx: f64) -> (f64, f64) {
    let mut m = vx % wx; // fmod
    let mut div = (vx - m) / wx;
    if m != 0.0 {
        if (wx < 0.0) != (m < 0.0) {
            m += wx;
            div -= 1.0;
        }
    } else {
        m = 0f64.copysign(wx);
    }
    let floordiv = if div != 0.0 {
        let mut f = div.floor();
        if div - f > 0.5 {
            f += 1.0;
        }
        f
    } else {
        0f64.copysign(vx / wx)
    };
    (floordiv, m)
}

fn is_odd_integer(f: f64) -> bool {
    f.is_finite() && f == f.trunc() && (f / 2.0).fract() != 0.0
}

/// CPython `float_pow`.
fn float_pow(iv: f64, iw: f64) -> R<Val> {
    if iw == 0.0 {
        return Ok(Val::Float(1.0));
    }
    if iv.is_nan() {
        return Ok(Val::Float(iv));
    }
    if iw.is_nan() {
        return Ok(Val::Float(if iv == 1.0 { 1.0 } else { iw }));
    }
    if iw.is_infinite() {
        let a = iv.abs();
        if a == 1.0 {
            return Ok(Val::Float(1.0));
        }
        return Ok(Val::Float(if (iw > 0.0) == (a > 1.0) {
            iw.abs()
        } else {
            0.0
        }));
    }
    if iv.is_infinite() {
        let odd = is_odd_integer(iw);
        if iv > 0.0 {
            return Ok(Val::Float(if iw > 0.0 { iv } else { 0.0 }));
        }
        return Ok(Val::Float(if iw > 0.0 {
            if odd {
                iv
            } else {
                -iv
            }
        } else if odd {
            0f64.copysign(iv)
        } else {
            0.0
        }));
    }
    if iv == 0.0 {
        if iw < 0.0 {
            return zero_div("0.0 cannot be raised to a negative power");
        }
        return Ok(Val::Float(if is_odd_integer(iw) { iv } else { 0.0 }));
    }
    let mut negate = false;
    let mut base = iv;
    if iv < 0.0 {
        if iw != iw.floor() {
            // a negative number to a fractional power is complex
            let (re, im) = complex_pow((iv, 0.0), (iw, 0.0))?;
            return Ok(Val::Complex(re, im));
        }
        base = -iv;
        negate = is_odd_integer(iw);
    }
    if base == 1.0 {
        return Ok(Val::Float(if negate { -1.0 } else { 1.0 }));
    }
    let r = base.powf(iw);
    if r.is_infinite() {
        return overflow("(34, 'Result too large')");
    }
    Ok(Val::Float(if negate { -r } else { r }))
}

fn c_mul(a: (f64, f64), b: (f64, f64)) -> (f64, f64) {
    (a.0 * b.0 - a.1 * b.1, a.0 * b.1 + a.1 * b.0)
}

/// CPython `_Py_c_quot`.
fn c_quot(a: (f64, f64), b: (f64, f64)) -> R<(f64, f64)> {
    let abs_br = b.0.abs();
    let abs_bi = b.1.abs();
    if abs_br >= abs_bi {
        if abs_br == 0.0 {
            return zero_div("complex division by zero");
        }
        let ratio = b.1 / b.0;
        let denom = b.0 + b.1 * ratio;
        Ok(((a.0 + a.1 * ratio) / denom, (a.1 - a.0 * ratio) / denom))
    } else if abs_bi >= abs_br {
        let ratio = b.0 / b.1;
        let denom = b.0 * ratio + b.1;
        Ok(((a.0 * ratio + a.1) / denom, (a.1 * ratio - a.0) / denom))
    } else {
        Ok((f64::NAN, f64::NAN))
    }
}

/// CPython `complex_pow`.
fn complex_pow(a: (f64, f64), b: (f64, f64)) -> R<(f64, f64)> {
    if b.1 == 0.0 && b.0 == b.0.floor() && b.0.abs() <= 100.0 {
        let n = b.0 as i64;
        // c_powi: c_powu for n > 0, 1 / c_powu(-n) for n < 0
        let powu = |x: (f64, f64), mut n: u64| {
            let mut r = (1.0, 0.0);
            let mut mask = 1u64;
            let mut x = x;
            while mask > 0 && n >= mask {
                if n & mask != 0 {
                    r = c_mul(r, x);
                }
                mask <<= 1;
                x = c_mul(x, x);
            }
            n = 0;
            let _ = n;
            r
        };
        return if n > 0 {
            Ok(powu(a, n as u64))
        } else {
            c_quot((1.0, 0.0), powu(a, n.unsigned_abs()))
        };
    }
    if b.0 == 0.0 && b.1 == 0.0 {
        return Ok((1.0, 0.0));
    }
    if a.0 == 0.0 && a.1 == 0.0 {
        if b.1 != 0.0 || b.0 < 0.0 {
            return zero_div("0.0 to a negative or complex power");
        }
        return Ok((0.0, 0.0));
    }
    let vabs = a.0.hypot(a.1);
    let mut len = vabs.powf(b.0);
    let at = a.1.atan2(a.0);
    let mut phase = at * b.0;
    if b.1 != 0.0 {
        len /= (at * b.1).exp();
        phase += b.1 * vabs.ln();
    }
    let r = (len * phase.cos(), len * phase.sin());
    if (!r.0.is_finite() || !r.1.is_finite()) && len.is_infinite() {
        return overflow("complex exponentiation");
    }
    Ok(r)
}

fn to_complex(v: &Val) -> R<(f64, f64)> {
    match v {
        Val::Complex(re, im) => Ok((*re, *im)),
        Val::Int(i) => Ok((int_to_f64(i)?, 0.0)),
        Val::Float(f) => Ok((*f, 0.0)),
        Val::Bool(b) => Ok((if *b { 1.0 } else { 0.0 }, 0.0)),
        _ => type_err("not a number"),
    }
}

/// Python `a <op> b` for `+ - * / // % **`.
fn py_binop(op: &str, a: &Val, b: &Val) -> R<Val> {
    // sequences
    match (a, b, op) {
        (Val::List(x), Val::List(y), "Add") => {
            return Ok(Val::List(x.iter().chain(y.iter()).cloned().collect()))
        }
        (Val::List(_), other, "Add") => {
            return type_err(format!(
                "can only concatenate list (not \"{}\") to list",
                other.type_name()
            ))
        }
        (Val::Str(x), Val::Str(y), "Add") => return Ok(Val::Str(format!("{x}{y}"))),
        (Val::Str(_), other, "Add") => {
            return type_err(format!(
                "can only concatenate str (not \"{}\") to str",
                other.type_name()
            ))
        }
        (Val::List(items), n, "Mult") | (n, Val::List(items), "Mult") if as_int(n).is_some() => {
            let k = as_int(n).and_then(|k| k.to_i64()).unwrap_or(0).max(0) as usize;
            if items.len().saturating_mul(k) > 10_000_000 {
                return Err(Exc::Py("MemoryError", String::new()));
            }
            return Ok(Val::List(
                items
                    .iter()
                    .cloned()
                    .cycle()
                    .take(items.len() * k)
                    .collect(),
            ));
        }
        (Val::List(_), n, "Mult") | (n, Val::List(_), "Mult") => {
            return type_err(format!(
                "can't multiply sequence by non-int of type '{}'",
                n.type_name()
            ))
        }
        _ => {}
    }
    if !(a.is_number() || matches!(a, Val::Complex(..)))
        || !(b.is_number() || matches!(b, Val::Complex(..)))
    {
        return unsupported(op, a, b);
    }
    if matches!(a, Val::Complex(..)) || matches!(b, Val::Complex(..)) {
        if op == "FloorDiv" || op == "Mod" {
            return unsupported(op, a, b);
        }
        let x = to_complex(a)?;
        let y = to_complex(b)?;
        let r = match op {
            "Add" => (x.0 + y.0, x.1 + y.1),
            "Sub" => (x.0 - y.0, x.1 - y.1),
            "Mult" => c_mul(x, y),
            "Div" => c_quot(x, y)?,
            _ => complex_pow(x, y)?,
        };
        return Ok(Val::Complex(r.0, r.1));
    }
    if let (Some(x), Some(y)) = (as_int(a), as_int(b)) {
        return Ok(match op {
            "Add" => Val::Int(x + y),
            "Sub" => Val::Int(x - y),
            "Mult" => Val::Int(x * y),
            "Div" => Val::Float(int_truediv(&x, &y)?),
            "FloorDiv" => {
                if y.is_zero() {
                    return zero_div("integer division or modulo by zero");
                }
                Val::Int(x.div_floor(&y))
            }
            "Mod" => {
                if y.is_zero() {
                    return zero_div("integer modulo by zero");
                }
                Val::Int(x.mod_floor(&y))
            }
            _ => {
                if y.sign() == Sign::Minus {
                    return float_pow(int_to_f64(&x)?, int_to_f64(&y)?);
                }
                let big_base = x.magnitude() > &BigUint::one();
                match y.to_u32() {
                    Some(e) if !big_base || (e as u64) * x.bits() <= 64_000_000 => {
                        Val::Int(num_traits::pow::Pow::pow(&x, e))
                    }
                    _ if !big_base => Val::Int(if x.is_zero() {
                        BigInt::zero()
                    } else if x.sign() == Sign::Minus && y.is_odd() {
                        -BigInt::one()
                    } else {
                        BigInt::one()
                    }),
                    _ => return Err(Exc::Py("MemoryError", String::new())),
                }
            }
        });
    }
    let x = match a {
        Val::Float(f) => *f,
        other => int_to_f64(&as_int(other).unwrap_or_default())?,
    };
    let y = match b {
        Val::Float(f) => *f,
        other => int_to_f64(&as_int(other).unwrap_or_default())?,
    };
    Ok(Val::Float(match op {
        "Add" => x + y,
        "Sub" => x - y,
        "Mult" => x * y,
        "Div" => {
            if y == 0.0 {
                return zero_div("float division by zero");
            }
            x / y
        }
        "FloorDiv" => {
            if y == 0.0 {
                return zero_div("float floor division by zero");
            }
            float_divmod(x, y).0
        }
        "Mod" => {
            if y == 0.0 {
                return zero_div("float modulo");
            }
            float_divmod(x, y).1
        }
        _ => return float_pow(x, y),
    }))
}

fn py_neg(v: &Val) -> R<Val> {
    match v {
        Val::Int(i) => Ok(Val::Int(-i)),
        Val::Bool(b) => Ok(Val::from(-(*b as i64))),
        Val::Float(f) => Ok(Val::Float(-f)),
        Val::Complex(re, im) => Ok(Val::Complex(-re, -im)),
        other => type_err(format!(
            "bad operand type for unary -: '{}'",
            other.type_name()
        )),
    }
}

fn py_pos(v: &Val) -> R<Val> {
    match v {
        Val::Int(_) | Val::Float(_) | Val::Complex(..) => Ok(v.clone()),
        Val::Bool(b) => Ok(Val::from(*b as i64)),
        other => type_err(format!(
            "bad operand type for unary +: '{}'",
            other.type_name()
        )),
    }
}

fn py_abs(v: &Val) -> R<Val> {
    match v {
        Val::Int(i) => Ok(Val::Int(i.abs())),
        Val::Bool(b) => Ok(Val::from(*b as i64)),
        Val::Float(f) => Ok(Val::Float(f.abs())),
        Val::Complex(re, im) => {
            let r = re.hypot(*im);
            if r.is_infinite() && re.is_finite() && im.is_finite() {
                return overflow("absolute value too large");
            }
            Ok(Val::Float(r))
        }
        other => type_err(format!(
            "bad operand type for abs(): '{}'",
            other.type_name()
        )),
    }
}

fn cmp_symbol(op: &str) -> &'static str {
    match op {
        "Lt" => "<",
        "LtE" => "<=",
        "Gt" => ">",
        "GtE" => ">=",
        "Eq" => "==",
        _ => "!=",
    }
}

fn py_eq(a: &Val, b: &Val) -> bool {
    if let Ok(o) = num_cmp(a, b) {
        return o == Some(Ordering::Equal);
    }
    match (a, b) {
        (Val::Complex(r1, i1), Val::Complex(r2, i2)) => r1 == r2 && i1 == i2,
        (Val::Complex(re, im), n) | (n, Val::Complex(re, im)) if n.is_number() => {
            *im == 0.0 && num_cmp(&Val::Float(*re), n).ok().flatten() == Some(Ordering::Equal)
        }
        (Val::List(x), Val::List(y)) => {
            x.len() == y.len() && x.iter().zip(y).all(|(p, q)| py_eq(p, q))
        }
        (Val::Str(x), Val::Str(y)) => x == y,
        (Val::None, Val::None) => true,
        (Val::Dict(x), Val::Dict(y)) => {
            x.len() == y.len() && x.iter().all(|(k, v)| y.get(k).is_some_and(|w| py_eq(v, w)))
        }
        _ => false,
    }
}

/// Python `a <op> b` for one comparison operator.
fn py_compare(op: &str, a: &Val, b: &Val) -> R<bool> {
    match op {
        "Eq" => return Ok(py_eq(a, b)),
        "NotEq" => return Ok(!py_eq(a, b)),
        _ => {}
    }
    let test = |o: Ordering| match op {
        "Lt" => o == Ordering::Less,
        "LtE" => o != Ordering::Greater,
        "Gt" => o == Ordering::Greater,
        _ => o != Ordering::Less,
    };
    if let Ok(o) = num_cmp(a, b) {
        return Ok(o.is_some_and(test));
    }
    match (a, b) {
        (Val::Str(x), Val::Str(y)) => Ok(test(x.cmp(y))),
        (Val::List(x), Val::List(y)) => {
            for (p, q) in x.iter().zip(y) {
                if !py_eq(p, q) {
                    return py_compare(op, p, q);
                }
            }
            Ok(test(x.len().cmp(&y.len())))
        }
        _ => type_err(format!(
            "'{}' not supported between instances of '{}' and '{}'",
            cmp_symbol(op),
            a.type_name(),
            b.type_name()
        )),
    }
}

/// `iter(v)` materialised.
fn py_iter(v: &Val) -> R<Vec<Val>> {
    match v {
        Val::List(items) => Ok(items.clone()),
        Val::Dict(d) => Ok(d.keys().map(|k| Val::Str(k.clone())).collect()),
        Val::Str(s) => Ok(s.chars().map(|c| Val::Str(c.to_string())).collect()),
        other => type_err(format!("'{}' object is not iterable", other.type_name())),
    }
}

fn py_len(v: &Val) -> R<usize> {
    match v {
        Val::List(items) => Ok(items.len()),
        Val::Dict(d) => Ok(d.len()),
        Val::Str(s) => Ok(s.chars().count()),
        other => type_err(format!(
            "object of type '{}' has no len()",
            other.type_name()
        )),
    }
}

/// Python's `float(str)` grammar.
fn parse_py_float(s: &str) -> Option<f64> {
    let t = s.trim();
    let low = t.to_ascii_lowercase();
    let (sign, body) = match low.strip_prefix('-') {
        Some(b) => (-1.0, b),
        None => (1.0, low.strip_prefix('+').unwrap_or(&low)),
    };
    match body {
        "inf" | "infinity" => return Some(sign * f64::INFINITY),
        "nan" => return Some(f64::NAN),
        _ => {}
    }
    if body.is_empty() || body.starts_with('_') || body.ends_with('_') || body.contains("__") {
        return None;
    }
    let ok = body
        .chars()
        .all(|c| c.is_ascii_digit() || matches!(c, '.' | 'e' | '+' | '-' | '_'));
    if !ok {
        return None;
    }
    body.replace('_', "").parse::<f64>().ok().map(|v| sign * v)
}

/// `float(v)`.
fn py_float(v: &Val) -> R<f64> {
    match v {
        Val::Int(i) => int_to_f64(i),
        Val::Float(f) => Ok(*f),
        Val::Bool(b) => Ok(if *b { 1.0 } else { 0.0 }),
        Val::Str(s) => parse_py_float(s).map(Ok).unwrap_or_else(|| {
            value_err(format!(
                "could not convert string to float: {}",
                py_str_repr(s)
            ))
        }),
        other => type_err(format!(
            "float() argument must be a string or a real number, not '{}'",
            other.type_name()
        )),
    }
}

/// `int(v)`.
fn py_int(v: &Val) -> R<BigInt> {
    match v {
        Val::Int(i) => Ok(i.clone()),
        Val::Bool(b) => Ok(BigInt::from(*b as i64)),
        Val::Float(f) => float_to_int_checked(*f),
        Val::Str(s) => {
            let t = s.trim();
            let (neg, body) = match t.strip_prefix('-') {
                Some(b) => (true, b),
                None => (false, t.strip_prefix('+').unwrap_or(t)),
            };
            let valid = !body.is_empty()
                && !body.starts_with('_')
                && !body.ends_with('_')
                && !body.contains("__")
                && body.chars().all(|c| c.is_ascii_digit() || c == '_');
            match valid
                .then(|| BigInt::parse_bytes(body.replace('_', "").as_bytes(), 10))
                .flatten()
            {
                Some(n) => Ok(if neg { -n } else { n }),
                None => value_err(format!(
                    "invalid literal for int() with base 10: {}",
                    py_str_repr(s)
                )),
            }
        }
        other => type_err(format!(
            "int() argument must be a string, a bytes-like object or a real number, not '{}'",
            other.type_name()
        )),
    }
}

/// `math` functions' argument conversion (`PyFloat_AsDouble`).
fn math_float(v: &Val) -> R<f64> {
    match v {
        Val::Int(i) => int_to_f64(i),
        Val::Float(f) => Ok(*f),
        Val::Bool(b) => Ok(if *b { 1.0 } else { 0.0 }),
        other => type_err(format!("must be real number, not {}", other.type_name())),
    }
}

/// `operator.index(v)` for `round`'s ndigits.
fn py_index(v: &Val) -> R<BigInt> {
    match as_int(v) {
        Some(i) => Ok(i),
        None => type_err(format!(
            "'{}' object cannot be interpreted as an integer",
            v.type_name()
        )),
    }
}

/// Round the decimal digit string `digits` (an integer) to a multiple of 10^k, half to even; `sticky` = a nonzero
/// fraction followed. Returns the kept leading digits.
fn round_digits_half_even(digits: &str, k: usize, sticky: bool) -> BigInt {
    let n = digits.len();
    let (head, tail) = if k >= n {
        ("0", digits)
    } else {
        digits.split_at(n - k)
    };
    let mut q = BigInt::parse_bytes(head.as_bytes(), 10).unwrap_or_default();
    let tail_padded = if k > n {
        format!("{}{}", "0".repeat(k - n), tail)
    } else {
        tail.to_string()
    };
    let half = format!("5{}", "0".repeat(k.saturating_sub(1)));
    let up = match tail_padded.cmp(&half) {
        Ordering::Greater => true,
        Ordering::Equal => sticky || q.is_odd(),
        Ordering::Less => false,
    };
    if k > 0 && up {
        q += 1;
    }
    q
}

/// `round(x[, ndigits])`.
fn py_round(x: &Val, nd: Option<&Val>) -> R<Val> {
    let nd = nd.filter(|v| !matches!(v, Val::None));
    match x {
        Val::Int(_) | Val::Bool(_) => {
            let i = as_int(x).unwrap_or_default();
            let Some(nd) = nd else { return Ok(Val::Int(i)) };
            let nd = py_index(nd)?;
            if nd.sign() != Sign::Minus {
                return Ok(Val::Int(i));
            }
            let k = nd.magnitude().to_usize().unwrap_or(usize::MAX);
            let digits = i.magnitude().to_string();
            if k > digits.len() + 1 {
                return Ok(Val::Int(BigInt::zero()));
            }
            let q = round_digits_half_even(&digits, k, false);
            let r = q * num_traits::pow::Pow::pow(&BigInt::from(10), k as u32);
            Ok(Val::Int(if i.sign() == Sign::Minus { -r } else { r }))
        }
        Val::Float(f) => {
            let f = *f;
            let Some(nd) = nd else {
                return Ok(Val::Int(float_to_int_checked(f.round_ties_even())?));
            };
            let nd = py_index(nd)?;
            let nd = nd.to_i64().unwrap_or(if nd.sign() == Sign::Minus {
                i64::MIN
            } else {
                i64::MAX
            });
            if nd > 323 {
                return Ok(Val::Float(f));
            }
            if nd < -308 {
                return Ok(Val::Float(0.0 * f));
            }
            if f == 0.0 || !f.is_finite() {
                return Ok(Val::Float(f));
            }
            if nd >= 0 {
                let s = format!("{:.*}", nd as usize, f);
                return Ok(Val::Float(s.parse::<f64>().unwrap_or(f)));
            }
            let k = (-nd) as usize;
            let int_digits = format!("{:.0}", f.abs().trunc());
            let q = round_digits_half_even(&int_digits, k, f.fract() != 0.0);
            let v: f64 = format!("{}e{}", q, k).parse().unwrap_or(f64::INFINITY);
            if v.is_infinite() {
                return overflow("rounded value too large to represent");
            }
            Ok(Val::Float(if f < 0.0 { -v } else { v }))
        }
        other => type_err(format!(
            "type {} doesn't define __round__ method",
            other.type_name()
        )),
    }
}

// ---------------------------------------------------------------- exact helpers for math.fsum and statistics

/// `math.fsum` (Shewchuk / CPython algorithm, including its special-value rules).
fn fsum(items: &[Val]) -> R<f64> {
    let mut p: Vec<f64> = Vec::new();
    let mut special_sum = 0.0f64;
    let mut inf_sum = 0.0f64;
    for item in items {
        let mut x = math_float(item)?;
        let xsave = x;
        let mut i = 0;
        for j in 0..p.len() {
            let mut y = p[j];
            if x.abs() < y.abs() {
                std::mem::swap(&mut x, &mut y);
            }
            let hi = x + y;
            let yr = hi - x;
            let lo = y - yr;
            if lo != 0.0 {
                p[i] = lo;
                i += 1;
            }
            x = hi;
        }
        p.truncate(i);
        if x != 0.0 {
            if !x.is_finite() {
                if xsave.is_finite() {
                    return overflow("intermediate overflow in fsum");
                }
                if xsave.is_infinite() {
                    inf_sum += xsave;
                }
                special_sum += xsave;
                p.clear();
            } else {
                p.push(x);
            }
        }
    }
    if special_sum != 0.0 || special_sum.is_nan() {
        if inf_sum.is_nan() {
            return value_err("-inf + inf in fsum");
        }
        return Ok(special_sum);
    }
    let mut hi = 0.0;
    let mut n = p.len();
    if n > 0 {
        n -= 1;
        hi = p[n];
        let mut lo = 0.0;
        while n > 0 {
            let x = hi;
            n -= 1;
            let y = p[n];
            hi = x + y;
            let yr = hi - x;
            lo = y - yr;
            if lo != 0.0 {
                break;
            }
        }
        if n > 0 && ((lo < 0.0 && p[n - 1] < 0.0) || (lo > 0.0 && p[n - 1] > 0.0)) {
            let y = lo * 2.0;
            let x = hi + y;
            let yr = x - hi;
            if y == yr {
                hi = x;
            }
        }
    }
    Ok(hi)
}

/// A fraction of big ints in lowest terms (denominator > 0).
#[derive(Debug, Clone)]
struct Frac {
    n: BigInt,
    d: BigInt,
}

impl Frac {
    fn new(n: BigInt, d: BigInt) -> Frac {
        let (n, d) = if d.sign() == Sign::Minus {
            (-n, -d)
        } else {
            (n, d)
        };
        let g = n.gcd(&d);
        if g.is_zero() || g.is_one() {
            Frac { n, d }
        } else {
            Frac {
                n: n / &g,
                d: d / &g,
            }
        }
    }
    fn int(i: BigInt) -> Frac {
        Frac {
            n: i,
            d: BigInt::one(),
        }
    }
    fn add(&self, o: &Frac) -> Frac {
        Frac::new(&self.n * &o.d + &o.n * &self.d, &self.d * &o.d)
    }
    fn sub(&self, o: &Frac) -> Frac {
        Frac::new(&self.n * &o.d - &o.n * &self.d, &self.d * &o.d)
    }
    fn mul(&self, o: &Frac) -> Frac {
        Frac::new(&self.n * &o.n, &self.d * &o.d)
    }
    fn div_int(&self, k: &BigInt) -> Frac {
        Frac::new(self.n.clone(), &self.d * k)
    }
}

/// `x.as_integer_ratio()` of a finite float.
fn float_ratio(f: f64) -> (BigInt, BigInt) {
    if f == 0.0 {
        return (BigInt::zero(), BigInt::one());
    }
    let bits = f.to_bits();
    let sign = if bits >> 63 == 1 { -1 } else { 1 };
    let exp = ((bits >> 52) & 0x7ff) as i64;
    let frac = bits & ((1u64 << 52) - 1);
    let (mant, e) = if exp == 0 {
        (frac, -1074)
    } else {
        (frac | (1u64 << 52), exp - 1075)
    };
    let mant = BigInt::from(mant) * sign;
    let f = if e >= 0 {
        Frac::new(mant << e as u64, BigInt::one())
    } else {
        Frac::new(mant, BigInt::one() << (-e) as u64)
    };
    (f.n, f.d)
}

/// `statistics._exact_ratio`: `Ok(Ok(fraction))`, `Ok(Err(x))` for inf / nan.
fn exact_ratio(v: &Val) -> R<Result<Frac, f64>> {
    match v {
        Val::Int(i) => Ok(Ok(Frac::int(i.clone()))),
        Val::Bool(b) => Ok(Ok(Frac::int(BigInt::from(*b as i64)))),
        Val::Float(f) if f.is_finite() => {
            let (n, d) = float_ratio(*f);
            Ok(Ok(Frac { n, d }))
        }
        Val::Float(f) => Ok(Err(*f)),
        other => type_err(format!(
            "can't convert type '{}' to numerator/denominator",
            other.type_name()
        )),
    }
}

/// `statistics._float_sqrt_of_frac(n, m)`: sqrt(n/m) correctly rounded.
fn float_sqrt_of_frac(n: &BigInt, m: &BigInt) -> R<f64> {
    let rto = |n: &BigInt, m: &BigInt| -> BigInt {
        let a = (n / m).sqrt();
        let exact = &a * &a * m == *n;
        if exact {
            a
        } else {
            a | BigInt::one()
        }
    };
    let q = (n.bits() as i64 - m.bits() as i64 - 109).div_euclid(2);
    if q >= 0 {
        let num = rto(n, &(m << (2 * q) as u64)) << q as u64;
        int_truediv(&num, &BigInt::one())
    } else {
        let num = rto(&(n << (-2 * q) as u64), m);
        int_truediv(&num, &(BigInt::one() << (-q) as u64))
    }
}

/// `statistics.stdev(data, xbar=None)`.
fn stdev(data: &Val, xbar: Option<&Val>) -> R<Val> {
    let xbar = xbar.filter(|v| !matches!(v, Val::None));
    let items = py_iter(data)?;
    let (ss, n): (Result<Frac, f64>, usize) = if let Some(c) = xbar {
        let mut total = Frac::int(BigInt::zero());
        let mut special: Option<f64> = None;
        for x in &items {
            let d = py_binop("Sub", x, c)?;
            match exact_ratio(&py_binop("Mult", &d, &d)?)? {
                Ok(f) => total = total.add(&f),
                Err(x) => special = Some(special.unwrap_or(0.0) + x),
            }
        }
        (special.map(Err).unwrap_or(Ok(total)), items.len())
    } else {
        let mut sx = Frac::int(BigInt::zero());
        let mut sxx = Frac::int(BigInt::zero());
        let mut special: Option<f64> = None;
        for x in &items {
            match exact_ratio(x)? {
                Ok(f) => {
                    sxx = sxx.add(&f.mul(&f));
                    sx = sx.add(&f);
                }
                Err(v) => special = Some(special.unwrap_or(0.0) + v),
            }
        }
        let count = items.len();
        let ss = match special {
            Some(s) => Err(s),
            None if count == 0 => Ok(Frac::int(BigInt::zero())),
            None => {
                let c = BigInt::from(count);
                Ok(Frac::int(c.clone()).mul(&sxx).sub(&sx.mul(&sx)).div_int(&c))
            }
        };
        (ss, count)
    };
    if n < 2 {
        return stats_err("stdev requires at least two data points");
    }
    match ss {
        Ok(ss) => {
            let mss = ss.div_int(&BigInt::from(n - 1));
            Ok(Val::Float(float_sqrt_of_frac(&mss.n, &mss.d)?))
        }
        Err(_) => Err(Exc::Py(
            "AttributeError",
            "'float' object has no attribute 'numerator'".into(),
        )),
    }
}

/// `statistics.fmean(data, weights=None)`.
fn fmean(data: &Val, weights: Option<&Val>) -> R<Val> {
    let weights = weights.filter(|v| !matches!(v, Val::None));
    let Some(w) = weights else {
        let items = py_iter(data)?;
        let total = fsum(&items)?;
        if items.is_empty() {
            return stats_err("fmean requires at least one data point");
        }
        return Ok(Val::Float(total / items.len() as f64));
    };
    let w = match w {
        Val::List(items) => items.clone(),
        other => py_iter(other)?,
    };
    let d = py_iter(data)?;
    if d.len() != w.len() {
        return stats_err("data and weights must be the same length");
    }
    let num = sumprod(&d, &w)?;
    let den = fsum(&w)?;
    if den == 0.0 {
        return stats_err("sum of weights must be non-zero");
    }
    py_binop("Div", &num, &Val::Float(den))
}

/// `math.sumprod(p, q)` for equal-length inputs.
fn sumprod(p: &[Val], q: &[Val]) -> R<Val> {
    if p.iter().chain(q).all(|v| as_int(v).is_some()) {
        let mut total = BigInt::zero();
        for (a, b) in p.iter().zip(q) {
            total += as_int(a).unwrap_or_default() * as_int(b).unwrap_or_default();
        }
        return Ok(Val::Int(total));
    }
    if p.iter().chain(q).all(Val::is_number) {
        let mut parts = Vec::with_capacity(p.len() * 2);
        for (a, b) in p.iter().zip(q) {
            let x = math_float(a)?;
            let y = math_float(b)?;
            let prod = x * y;
            parts.push(Val::Float(prod));
            if prod.is_finite() {
                parts.push(Val::Float(x.mul_add(y, -prod)));
            }
        }
        return Ok(Val::Float(fsum(&parts)?));
    }
    let mut total = Val::from(0);
    for (a, b) in p.iter().zip(q) {
        total = py_binop("Add", &total, &py_binop("Mult", a, b)?)?;
    }
    Ok(total)
}

/// `sorted(items)` with Python's `<` (binary insertion sort after the initial run, like CPython for short lists).
fn py_sorted(mut v: Vec<Val>) -> R<Vec<Val>> {
    let n = v.len();
    if n < 2 {
        return Ok(v);
    }
    let lt = |a: &Val, b: &Val| py_compare("Lt", a, b);
    let mut run = 2;
    if lt(&v[1], &v[0])? {
        while run < n && lt(&v[run], &v[run - 1])? {
            run += 1;
        }
        v[..run].reverse();
    } else {
        while run < n && !lt(&v[run], &v[run - 1])? {
            run += 1;
        }
    }
    for i in run..n {
        let pivot = v[i].clone();
        let (mut l, mut r) = (0, i);
        while l < r {
            let m = l + (r - l) / 2;
            if lt(&pivot, &v[m])? {
                r = m;
            } else {
                l = m + 1;
            }
        }
        let item = v.remove(i);
        v.insert(l, item);
    }
    Ok(v)
}

/// `statistics.median(data)`.
fn median(data: &Val) -> R<Val> {
    let items = py_sorted(py_iter(data)?)?;
    let n = items.len();
    if n == 0 {
        return stats_err("no median for empty data");
    }
    if n % 2 == 1 {
        return Ok(items[n / 2].clone());
    }
    let s = py_binop("Add", &items[n / 2 - 1], &items[n / 2])?;
    py_binop("Div", &s, &Val::from(2))
}

/// Builtin `sum(iterable, start)` with CPython 3.12's fast paths (int, then compensated float).
fn py_sum(items: Vec<Val>, start: Val) -> R<Val> {
    if matches!(start, Val::Str(_)) {
        return type_err("sum() can't sum strings [use ''.join(seq) instead]");
    }
    let mut result = start;
    let mut it = items.into_iter().peekable();
    if let Val::Int(i) = &result {
        if let Some(mut acc) = i.to_i64() {
            loop {
                let Some(item) = it.next() else {
                    return Ok(Val::from(acc));
                };
                let small = as_int(&item)
                    .and_then(|b| b.to_i64())
                    .and_then(|b| acc.checked_add(b));
                match small {
                    Some(s) => acc = s,
                    None => {
                        result = py_binop("Add", &Val::from(acc), &item)?;
                        break;
                    }
                }
            }
        }
    }
    if let Val::Float(f0) = result {
        let mut f = f0;
        let mut c = 0.0f64;
        loop {
            let Some(item) = it.next() else {
                if c != 0.0 && c.is_finite() {
                    f += c;
                }
                return Ok(Val::Float(f));
            };
            match &item {
                Val::Float(x) => {
                    let x = *x;
                    let t = f + x;
                    if f.abs() >= x.abs() {
                        c += (f - t) + x;
                    } else {
                        c += (x - t) + f;
                    }
                    f = t;
                }
                other if as_int(other).and_then(|b| b.to_i64()).is_some() => {
                    f += as_int(other).and_then(|b| b.to_i64()).unwrap_or(0) as f64;
                }
                other => {
                    result = py_binop("Add", &Val::Float(f), other)?;
                    break;
                }
            }
        }
    }
    for item in it {
        result = py_binop("Add", &result, &item)?;
    }
    Ok(result)
}

// ---------------------------------------------------------------- argument binding

type Kwargs = IndexMap<String, Val>;

fn plural(n: usize) -> &'static str {
    if n == 1 {
        ""
    } else {
        "s"
    }
}

/// Bind the arguments of a Python `def` function (CPython's error texts). Returns the parameter slots and `*args`.
fn bind_def(
    fname: &str,
    params: &[&str],
    ndefaults: usize,
    varargs: bool,
    mut args: Vec<Val>,
    kws: Kwargs,
) -> R<(Vec<Option<Val>>, Vec<Val>)> {
    let n = params.len();
    let npos = args.len();
    let extra = if npos > n {
        args.split_off(n)
    } else {
        Vec::new()
    };
    let mut slots: Vec<Option<Val>> = args.into_iter().map(Some).collect();
    slots.resize(n, None);
    for (k, v) in kws {
        match params.iter().position(|p| *p == k) {
            None => {
                return type_err(format!(
                    "{fname}() got an unexpected keyword argument '{k}'"
                ))
            }
            Some(i) if slots[i].is_some() => {
                return type_err(format!("{fname}() got multiple values for argument '{k}'"))
            }
            Some(i) => slots[i] = Some(v),
        }
    }
    let required = n - ndefaults;
    if !varargs && npos > n {
        let takes = if ndefaults > 0 {
            format!("from {required} to {n} positional arguments")
        } else {
            format!("{n} positional argument{}", plural(n))
        };
        let was = if npos == 1 { "was" } else { "were" };
        return type_err(format!("{fname}() takes {takes} but {npos} {was} given"));
    }
    let missing: Vec<String> = (0..required)
        .filter(|i| slots[*i].is_none())
        .map(|i| format!("'{}'", params[i]))
        .collect();
    if !missing.is_empty() {
        let names = match missing.len() {
            1 => missing[0].clone(),
            2 => format!("{} and {}", missing[0], missing[1]),
            k => format!("{}, and {}", missing[..k - 1].join(", "), missing[k - 1]),
        };
        return type_err(format!(
            "{fname}() missing {} required positional argument{}: {names}",
            missing.len(),
            plural(missing.len())
        ));
    }
    Ok((slots, extra))
}

/// Bind the arguments of an Argument-Clinic builtin (`round`, `sum`, `int`). `posonly` leading parameters cannot be
/// given by keyword; the first `minargs` are required.
fn bind_clinic(
    fname: &str,
    params: &[&str],
    posonly: usize,
    minargs: usize,
    args: Vec<Val>,
    kws: &Kwargs,
) -> R<Vec<Option<Val>>> {
    let nargs = args.len();
    let total = nargs + kws.len();
    if total > params.len() {
        return type_err(format!(
            "{fname}() takes at most {} argument{} ({total} given)",
            params.len(),
            plural(params.len())
        ));
    }
    if posonly > 0 && nargs < minargs.min(posonly) {
        let k = minargs.min(posonly);
        return type_err(format!(
            "{fname}() takes at least {k} positional argument{} ({nargs} given)",
            plural(k)
        ));
    }
    let mut slots: Vec<Option<Val>> = args.into_iter().map(Some).collect();
    slots.resize(params.len(), None);
    for i in nargs..params.len() {
        if i >= posonly {
            if let Some(v) = kws.get(params[i]) {
                slots[i] = Some(v.clone());
                continue;
            }
        }
        if i < minargs {
            return type_err(format!(
                "{fname}() missing required argument '{}' (pos {})",
                params[i],
                i + 1
            ));
        }
    }
    for k in kws.keys() {
        match params.iter().position(|p| p == k) {
            Some(i) if i >= posonly && i < nargs => {
                return type_err(format!(
                    "argument for {fname}() given by name ('{k}') and position ({})",
                    i + 1
                ))
            }
            Some(i) if i >= posonly => {}
            _ => {
                return type_err(format!(
                    "'{k}' is an invalid keyword argument for {fname}()"
                ))
            }
        }
    }
    Ok(slots)
}

fn no_kwargs(qual: &str, kws: &Kwargs) -> R<()> {
    if kws.is_empty() {
        Ok(())
    } else {
        type_err(format!("{qual} takes no keyword arguments"))
    }
}

fn one_arg(qual: &str, args: &[Val]) -> R<()> {
    if args.len() == 1 {
        Ok(())
    } else {
        type_err(format!(
            "{qual} takes exactly one argument ({} given)",
            args.len()
        ))
    }
}

/// `math.log` / `math.log10` argument handling (`loghelper`): big ints are handled via their bit length.
fn loghelper(v: &Val, f: fn(f64) -> R<f64>) -> R<f64> {
    if let Some(i) = as_int(v) {
        if i.sign() != Sign::Plus {
            return value_err("math domain error");
        }
        if let Ok(x) = int_to_f64(&i) {
            return f(x);
        }
        // x = m * 2**e with 0.5 <= m < 1
        let e = i.bits() as i64;
        let m = ratio_to_f64(i.magnitude(), &(BigUint::one() << e as u64)).unwrap_or(0.5);
        return Ok(f(m)? + f(2.0)? * e as f64);
    }
    f(math_float(v)?)
}

fn m_log(x: f64) -> R<f64> {
    if x.is_finite() {
        if x > 0.0 {
            Ok(x.ln())
        } else {
            value_err("math domain error")
        }
    } else if x.is_nan() || x > 0.0 {
        Ok(x)
    } else {
        value_err("math domain error")
    }
}

fn m_log10(x: f64) -> R<f64> {
    m_log(x).map(|_| x.log10())
}

// ---------------------------------------------------------------- the whitelisted functions

/// `FUNCS` of the Python module (names the expressions may call).
pub const FUNCS: [&str; 26] = [
    "abs", "round", "min", "max", "sum", "len", "int", "float", "sqrt", "log", "log10", "exp",
    "ceil", "floor", "mean", "median", "stdev", "pmt", "fv", "cagr", "loan", "invest", "npv",
    "irr", "apr", "payback",
];

fn call_func(name: &str, args: Vec<Val>, kws: Kwargs) -> R<Val> {
    let opt = |slots: &[Option<Val>], i: usize, default: Val| {
        slots.get(i).cloned().flatten().unwrap_or(default)
    };
    match name {
        "abs" => {
            no_kwargs("abs()", &kws)?;
            one_arg("abs()", &args)?;
            py_abs(&args[0])
        }
        "len" => {
            no_kwargs("len()", &kws)?;
            one_arg("len()", &args)?;
            Ok(Val::from(py_len(&args[0])? as i64))
        }
        "sqrt" | "log10" | "exp" | "ceil" | "floor" => {
            let qual = format!("math.{name}()");
            no_kwargs(&qual, &kws)?;
            one_arg(&qual, &args)?;
            let a = &args[0];
            match name {
                "sqrt" => {
                    let x = math_float(a)?;
                    if x < 0.0 {
                        return value_err("math domain error");
                    }
                    Ok(Val::Float(x.sqrt()))
                }
                "log10" => Ok(Val::Float(loghelper(a, m_log10)?)),
                "exp" => {
                    let x = math_float(a)?;
                    let r = x.exp();
                    if r.is_infinite() && x.is_finite() {
                        return overflow("math range error");
                    }
                    Ok(Val::Float(r))
                }
                _ => {
                    if let Some(i) = as_int(a) {
                        return Ok(Val::Int(i));
                    }
                    let x = math_float(a)?;
                    let r = if name == "ceil" { x.ceil() } else { x.floor() };
                    Ok(Val::Int(float_to_int_checked(r)?))
                }
            }
        }
        "log" => {
            no_kwargs("math.log()", &kws)?;
            if args.is_empty() {
                return type_err("log expected at least 1 argument, got 0");
            }
            if args.len() > 2 {
                return type_err(format!(
                    "log expected at most 2 arguments, got {}",
                    args.len()
                ));
            }
            let num = loghelper(&args[0], m_log)?;
            if args.len() == 2 {
                let den = loghelper(&args[1], m_log)?;
                if den == 0.0 {
                    return zero_div("float division by zero");
                }
                return Ok(Val::Float(num / den));
            }
            Ok(Val::Float(num))
        }
        "round" => {
            let s = bind_clinic("round", &["number", "ndigits"], 0, 1, args, &kws)?;
            py_round(s[0].as_ref().unwrap_or(&Val::None), s[1].as_ref())
        }
        "int" => {
            let has_x = !args.is_empty();
            let s = bind_clinic("int", &["x", "base"], 1, 0, args, &kws)?;
            match (&s[0], &s[1]) {
                (None, None) => Ok(Val::from(0)),
                (None, Some(_)) => type_err("int() missing string argument"),
                (Some(x), None) => Ok(Val::Int(py_int(x)?)),
                (Some(x), Some(base)) => {
                    let _ = has_x;
                    let Val::Str(text) = x else {
                        return type_err("int() can't convert non-string with explicit base");
                    };
                    let b = py_index(base)?.to_u32().unwrap_or(0);
                    let t = text.trim().replace('_', "");
                    let (neg, body) = match t.strip_prefix('-') {
                        Some(r) => (true, r.to_string()),
                        None => (false, t.trim_start_matches('+').to_string()),
                    };
                    match (2..=36)
                        .contains(&b)
                        .then(|| BigInt::parse_bytes(body.as_bytes(), b))
                        .flatten()
                    {
                        Some(v) => Ok(Val::Int(if neg { -v } else { v })),
                        None => value_err(format!(
                            "invalid literal for int() with base {b}: {}",
                            py_str_repr(text)
                        )),
                    }
                }
            }
        }
        "float" => {
            no_kwargs("float()", &kws)?;
            if args.len() > 1 {
                return type_err(format!(
                    "float expected at most 1 argument, got {}",
                    args.len()
                ));
            }
            match args.first() {
                None => Ok(Val::Float(0.0)),
                Some(v) => Ok(Val::Float(py_float(v)?)),
            }
        }
        "min" | "max" => {
            if args.is_empty() {
                return type_err(format!("{name} expected at least 1 argument, got 0"));
            }
            let mut key: Option<Val> = None;
            let mut default: Option<Val> = None;
            for (k, v) in &kws {
                match k.as_str() {
                    "key" => key = Some(v.clone()),
                    "default" => default = Some(v.clone()),
                    _ => {
                        return type_err(format!(
                            "'{k}' is an invalid keyword argument for {name}()"
                        ))
                    }
                }
            }
            let positional = args.len() > 1;
            if positional && default.is_some() {
                return type_err(format!(
                    "Cannot specify a default for {name}() with multiple positional arguments"
                ));
            }
            let items = if positional { args } else { py_iter(&args[0])? };
            let key = key.filter(|k| !matches!(k, Val::None));
            let op = if name == "max" { "Gt" } else { "Lt" };
            let mut best: Option<Val> = None;
            for item in items {
                if let Some(k) = &key {
                    return type_err(format!("'{}' object is not callable", k.type_name()));
                }
                best = match best {
                    None => Some(item),
                    Some(b) => Some(if py_compare(op, &item, &b)? { item } else { b }),
                };
            }
            match (best, default) {
                (Some(b), _) => Ok(b),
                (None, Some(d)) => Ok(d),
                (None, None) => value_err(format!("{name}() iterable argument is empty")),
            }
        }
        "sum" => {
            let s = bind_clinic("sum", &["iterable", "start"], 1, 1, args, &kws)?;
            let items = py_iter(s[0].as_ref().unwrap_or(&Val::None))?;
            py_sum(items, opt(&s, 1, Val::from(0)))
        }
        "mean" => {
            let (s, _) = bind_def("fmean", &["data", "weights"], 1, false, args, kws)?;
            fmean(s[0].as_ref().unwrap_or(&Val::None), s[1].as_ref())
        }
        "median" => {
            let (s, _) = bind_def("median", &["data"], 0, false, args, kws)?;
            median(s[0].as_ref().unwrap_or(&Val::None))
        }
        "stdev" => {
            let (s, _) = bind_def("stdev", &["data", "xbar"], 1, false, args, kws)?;
            stdev(s[0].as_ref().unwrap_or(&Val::None), s[1].as_ref())
        }
        "pmt" => {
            let (s, _) = bind_def("pmt", &["rate", "nper", "pv", "fv"], 1, false, args, kws)?;
            f_pmt(
                &opt(&s, 0, Val::None),
                &opt(&s, 1, Val::None),
                &opt(&s, 2, Val::None),
                &opt(&s, 3, Val::Float(0.0)),
            )
        }
        "fv" => {
            let (s, _) = bind_def("fv", &["rate", "nper", "pmt_", "pv"], 2, false, args, kws)?;
            f_fv(
                &opt(&s, 0, Val::None),
                &opt(&s, 1, Val::None),
                &opt(&s, 2, Val::Float(0.0)),
                &opt(&s, 3, Val::Float(0.0)),
            )
        }
        "cagr" => {
            let (s, _) = bind_def("cagr", &["start", "end", "years"], 0, false, args, kws)?;
            f_cagr(
                &opt(&s, 0, Val::None),
                &opt(&s, 1, Val::None),
                &opt(&s, 2, Val::None),
            )
        }
        "loan" => {
            let (s, _) = bind_def(
                "loan",
                &["principal", "annual_rate_pct", "years", "rows", "per_year"],
                2,
                false,
                args,
                kws,
            )?;
            f_loan(
                &opt(&s, 0, Val::None),
                &opt(&s, 1, Val::None),
                &opt(&s, 2, Val::None),
                &opt(&s, 3, Val::from(12)),
                &opt(&s, 4, Val::from(12)),
            )
        }
        "invest" => {
            let (s, _) = bind_def(
                "invest",
                &["monthly", "annual_rate_pct", "years", "initial", "per_year"],
                2,
                false,
                args,
                kws,
            )?;
            f_invest(
                &opt(&s, 0, Val::None),
                &opt(&s, 1, Val::None),
                &opt(&s, 2, Val::None),
                &opt(&s, 3, Val::Float(0.0)),
                &opt(&s, 4, Val::from(12)),
            )
        }
        "npv" => {
            let (s, more) = bind_def("npv", &["rate_pct", "flows"], 0, true, args, kws)?;
            f_npv(&opt(&s, 0, Val::None), &opt(&s, 1, Val::None), more)
        }
        "irr" => {
            let (s, more) = bind_def("irr", &["flows"], 0, true, args, kws)?;
            f_irr(&opt(&s, 0, Val::None), more)
        }
        "apr" => {
            let (s, _) = bind_def(
                "apr",
                &["principal", "payment", "nper", "per_year"],
                1,
                false,
                args,
                kws,
            )?;
            f_apr(
                &opt(&s, 0, Val::None),
                &opt(&s, 1, Val::None),
                &opt(&s, 2, Val::None),
                &opt(&s, 3, Val::from(12)),
            )
        }
        "payback" => {
            let (s, _) = bind_def(
                "payback",
                &["cost", "saving_per_period"],
                0,
                false,
                args,
                kws,
            )?;
            f_payback(&opt(&s, 0, Val::None), &opt(&s, 1, Val::None))
        }
        _ => calc_err(format!("unknown name {}", py_str_repr(name))),
    }
}

// ---------------------------------------------------------------- finance helpers (written over Python values)

fn add(a: &Val, b: &Val) -> R<Val> {
    py_binop("Add", a, b)
}
fn sub(a: &Val, b: &Val) -> R<Val> {
    py_binop("Sub", a, b)
}
fn mul(a: &Val, b: &Val) -> R<Val> {
    py_binop("Mult", a, b)
}
fn div(a: &Val, b: &Val) -> R<Val> {
    py_binop("Div", a, b)
}
fn pow(a: &Val, b: &Val) -> R<Val> {
    py_binop("Pow", a, b)
}
fn round2(v: &Val, nd: i64) -> R<Val> {
    py_round(v, Some(&Val::from(nd)))
}
fn is_zero(v: &Val) -> R<bool> {
    py_compare("Eq", v, &Val::from(0))
}

/// pmt / fv take the rate per period as a fraction (0.035/12). A value of 1 or more can only be a percentage
/// (no loan or deposit earns 100%+ per period), so 4 is read as 4% = 0.04.
fn per_period(rate: &Val) -> R<Val> {
    let r = py_float(rate)?;
    Ok(Val::Float(if r.abs() >= 1.0 { r / 100.0 } else { r }))
}

/// Payment per period for a loan (positive number), like Excel PMT without the sign.
fn f_pmt(rate: &Val, nper: &Val, pv: &Val, fv: &Val) -> R<Val> {
    let rate = per_period(rate)?;
    let pv = py_abs(pv)?;
    let fv = py_abs(fv)?;
    if is_zero(&rate)? {
        return div(&add(&pv, &fv)?, nper);
    }
    let one_r = add(&Val::from(1), &rate)?;
    let num = mul(&add(&mul(&pv, &pow(&one_r, nper)?)?, &fv)?, &rate)?;
    div(&num, &sub(&pow(&one_r, nper)?, &Val::from(1))?)
}

/// A rate below 1 is almost always a fraction typed by mistake (0.04 meant 4%); say so instead of guessing.
fn rate_note(annual_rate_pct: &Val) -> R<String> {
    let a = py_abs(annual_rate_pct)?;
    if py_compare("Lt", &Val::from(0), &a)? && py_compare("Lt", &a, &Val::from(1))? {
        let g = |v: &Val| {
            v.as_f64()
                .map(format_g_default)
                .unwrap_or_else(|| v.py_str())
        };
        let x = g(annual_rate_pct);
        let x100 = g(&mul(annual_rate_pct, &Val::from(100))?);
        return Ok(format!(
            "annual_rate_pct={x} is read as {x}% a year. If you meant {x100}%, call again with {x100}."
        ));
    }
    Ok(String::new())
}

/// Level-payment loan: payment, totals and the first `rows` lines of the amortization schedule.
fn f_loan(
    principal: &Val,
    annual_rate_pct: &Val,
    years: &Val,
    rows: &Val,
    per_year: &Val,
) -> R<Val> {
    let r = div(&div(annual_rate_pct, &Val::from(100))?, per_year)?;
    let n = Val::Int(py_int(&py_round(&mul(years, per_year)?, None)?)?);
    let pay = f_pmt(&r, &n, principal, &Val::Float(0.0))?;
    let mut bal = Val::Float(py_float(principal)?);
    let mut sched = Vec::new();
    let rows_i = py_int(rows)?;
    let n_i = as_int(&n).unwrap_or_default();
    let limit = rows_i.min(n_i).min(BigInt::from(600));
    let limit = limit.to_i64().unwrap_or(0);
    for i in 1..=limit.max(0) {
        let it = mul(&bal, &r)?;
        let pr = sub(&pay, &it)?;
        bal = sub(&bal, &pr)?;
        let shown = if py_compare("Gt", &Val::Float(0.0), &bal)? {
            Val::Float(0.0)
        } else {
            bal.clone()
        };
        let mut row = IndexMap::new();
        row.insert("period".to_string(), Val::from(i));
        row.insert("payment".to_string(), round2(&pay, 2)?);
        row.insert("principal".to_string(), round2(&pr, 2)?);
        row.insert("interest".to_string(), round2(&it, 2)?);
        row.insert("balance".to_string(), round2(&shown, 2)?);
        sched.push(Val::Dict(row));
    }
    let total = mul(&pay, &n)?;
    let mut out = IndexMap::new();
    out.insert("payment".to_string(), round2(&pay, 2)?);
    out.insert("periods".to_string(), n.clone());
    out.insert("total_paid".to_string(), round2(&total, 2)?);
    out.insert(
        "total_interest".to_string(),
        round2(&sub(&total, principal)?, 2)?,
    );
    out.insert("schedule".to_string(), Val::List(sched));
    let note = rate_note(annual_rate_pct)?;
    if !note.is_empty() {
        out.insert("note".to_string(), Val::Str(note));
    }
    Ok(Val::Dict(out))
}

/// Future value of a lump sum `pv` plus regular deposits `pmt_`. Signs are ignored (Excel writes deposits as
/// negative numbers; models copy that), so the result is always the positive balance.
fn f_fv(rate: &Val, nper: &Val, pmt_: &Val, pv: &Val) -> R<Val> {
    let pmt_ = py_abs(pmt_)?;
    let pv = py_abs(pv)?;
    let rate = per_period(rate)?;
    if is_zero(&rate)? {
        return add(&pv, &mul(&pmt_, nper)?);
    }
    let g = pow(&add(&Val::from(1), &rate)?, nper)?;
    add(
        &mul(&pv, &g)?,
        &div(&mul(&pmt_, &sub(&g, &Val::from(1))?)?, &rate)?,
    )
}

/// Regular saving / investing (定投): balance at the end of each year, money put in, and the gain.
fn f_invest(
    monthly: &Val,
    annual_rate_pct: &Val,
    years: &Val,
    initial: &Val,
    per_year: &Val,
) -> R<Val> {
    let per_year_i = py_int(per_year)?;
    if per_year_i < BigInt::one() || per_year_i > BigInt::from(365) {
        return calc_err("per_year must be 1–365 (1 = yearly, 12 = monthly, 52 = weekly)");
    }
    let py_ = Val::Int(per_year_i.clone());
    let r = div(&div(annual_rate_pct, &Val::from(100))?, &py_)?;
    let n = py_int(&py_round(&mul(years, &py_)?, None)?)?;
    if n <= BigInt::zero() || n > BigInt::from(1200) {
        return calc_err("years must give 1–1200 periods");
    }
    let n = n.to_i64().unwrap_or(0);
    let per = per_year_i.to_i64().unwrap_or(12);
    let init = Val::Float(py_float(initial)?.abs());
    let mut bal = init.clone();
    let mut put = init;
    let mut rows = Vec::new();
    for i in 1..=n {
        let m = py_abs(monthly)?;
        bal = add(&mul(&bal, &add(&Val::from(1), &r)?)?, &m)?;
        put = add(&put, &py_abs(monthly)?)?;
        if i % per == 0 || i == n {
            let mut row = IndexMap::new();
            row.insert("year".to_string(), round2(&div(&Val::from(i), &py_)?, 2)?);
            row.insert("contributed".to_string(), round2(&put, 2)?);
            row.insert("balance".to_string(), round2(&bal, 2)?);
            row.insert("gain".to_string(), round2(&sub(&bal, &put)?, 2)?);
            rows.push(Val::Dict(row));
        }
    }
    let mut out = IndexMap::new();
    out.insert("balance".to_string(), round2(&bal, 2)?);
    out.insert("contributed".to_string(), round2(&put, 2)?);
    out.insert("gain".to_string(), round2(&sub(&bal, &put)?, 2)?);
    out.insert("years".to_string(), Val::List(rows));
    let note = rate_note(annual_rate_pct)?;
    if !note.is_empty() {
        out.insert("note".to_string(), Val::Str(note));
    }
    Ok(Val::Dict(out))
}

/// Compound annual growth rate in percent.
fn f_cagr(start: &Val, end: &Val, years: &Val) -> R<Val> {
    let g = pow(&div(end, start)?, &div(&Val::from(1), years)?)?;
    mul(&sub(&g, &Val::from(1))?, &Val::from(100))
}

fn flows_of(flows: &Val, more: Vec<Val>) -> R<Vec<Val>> {
    let mut out = match flows {
        Val::List(items) => items.clone(),
        other => vec![other.clone()],
    };
    out.extend(more);
    if out.is_empty() || !out.iter().all(Val::is_number) {
        return calc_err("cash flows must be numbers, e.g. [-50000, 12000, 12000]");
    }
    Ok(out)
}

/// sum(c / (1 + r) ** t for t, c in enumerate(flows))
fn discounted(flows: &[Val], r: &Val) -> R<Val> {
    let base = add(&Val::from(1), r)?;
    let mut terms = Vec::with_capacity(flows.len());
    for (t, c) in flows.iter().enumerate() {
        terms.push(div(c, &pow(&base, &Val::from(t as i64))?)?);
    }
    py_sum(terms, Val::from(0))
}

/// Net present value at `rate_pct`% per period; flows[0] happens now.
fn f_npv(rate_pct: &Val, flows: &Val, more: Vec<Val>) -> R<Val> {
    let flows = flows_of(flows, more)?;
    let r = div(rate_pct, &Val::from(100))?;
    discounted(&flows, &r)
}

/// Internal rate of return in percent per period for cash flows [-invest, in1, in2, …] (bisection).
fn f_irr(flows: &Val, more: Vec<Val>) -> R<Val> {
    let flows = flows_of(flows, more)?;
    let mut neg = false;
    let mut posv = false;
    for c in &flows {
        neg = neg || py_compare("Lt", c, &Val::from(0))?;
        posv = posv || py_compare("Gt", c, &Val::from(0))?;
    }
    if !(neg && posv) {
        return calc_err("irr needs both negative (money out) and positive (money in) cash flows");
    }
    let f = |r: f64| -> R<Val> { discounted(&flows, &Val::Float(r)) };
    let mut lo = -0.9999f64;
    let mut hi = 10.0f64;
    if py_compare("Gt", &mul(&f(lo)?, &f(hi)?)?, &Val::from(0))? {
        return calc_err("no IRR between -100% and 1000%");
    }
    for _ in 0..200 {
        let mid = (lo + hi) / 2.0;
        if py_compare("LtE", &mul(&f(lo)?, &f(mid)?)?, &Val::from(0))? {
            hi = mid;
        } else {
            lo = mid;
        }
    }
    Ok(Val::Float((lo + hi) / 2.0 * 100.0))
}

/// True interest rate of an instalment plan / loan: nominal annual rate (APR) and effective annual rate (EIR), in percent.
fn f_apr(principal: &Val, payment: &Val, nper: &Val, per_year: &Val) -> R<Val> {
    let principal = py_abs(principal)?;
    let payment = py_abs(payment)?;
    let total = mul(&payment, nper)?;
    let mut out = IndexMap::new();
    if py_compare("LtE", &total, &principal)? {
        out.insert("apr_pct".to_string(), Val::Float(0.0));
        out.insert("effective_pct".to_string(), Val::Float(0.0));
        out.insert(
            "total_interest".to_string(),
            round2(&sub(&total, &principal)?, 2)?,
        );
        return Ok(Val::Dict(out));
    }
    let k = py_int(nper)?.to_usize().unwrap_or(0);
    if k > 10_000_000 {
        return Err(Exc::Py("MemoryError", String::new()));
    }
    let mut flows = vec![py_neg(&principal)?];
    flows.extend(std::iter::repeat_n(payment.clone(), k));
    let r = div(&f_irr(&Val::List(flows), Vec::new())?, &Val::from(100))?;
    out.insert(
        "apr_pct".to_string(),
        round2(&mul(&mul(&r, per_year)?, &Val::from(100))?, 3)?,
    );
    let eff = mul(
        &sub(&pow(&add(&Val::from(1), &r)?, per_year)?, &Val::from(1))?,
        &Val::from(100),
    )?;
    out.insert("effective_pct".to_string(), round2(&eff, 3)?);
    out.insert(
        "period_rate_pct".to_string(),
        round2(&mul(&r, &Val::from(100))?, 4)?,
    );
    out.insert(
        "total_interest".to_string(),
        round2(&sub(&total, &principal)?, 2)?,
    );
    Ok(Val::Dict(out))
}

/// Periods until savings repay the cost (cost / saving per period).
fn f_payback(cost: &Val, saving: &Val) -> R<Val> {
    if py_compare("LtE", saving, &Val::from(0))? {
        return calc_err("saving per period must be positive");
    }
    div(&py_abs(cost)?, saving)
}

// ---------------------------------------------------------------- evaluator and public API

/// `CONSTS` of the Python module.
pub const CONSTS: [(&str, f64); 2] = [("pi", std::f64::consts::PI), ("e", std::f64::consts::E)];

fn not_allowed<T>(node: &Node) -> R<T> {
    calc_err(format!(
        "not allowed: {}",
        crate::util::take_chars(&node.dump(), 60)
    ))
}

/// `_ev(node, env)`.
fn ev(node: &Node, env: &Env, depth: usize) -> R<Val> {
    if depth > 950 {
        return Err(Exc::Py(
            "RecursionError",
            "maximum recursion depth exceeded".into(),
        ));
    }
    let d = depth + 1;
    match node {
        Node::Const(Const::Int(i)) => Ok(Val::Int(i.clone())),
        Node::Const(Const::Float(f)) => Ok(Val::Float(*f)),
        Node::Name(id, Ctx::Load) => {
            if let Some(v) = env.get(id) {
                return Ok(v.clone());
            }
            if let Some((_, c)) = CONSTS.iter().find(|(k, _)| k == id) {
                return Ok(Val::Float(*c));
            }
            calc_err(format!("unknown name {}", py_str_repr(id)))
        }
        Node::BinOp(l, op, r)
            if matches!(
                *op,
                "Add" | "Sub" | "Mult" | "Div" | "FloorDiv" | "Mod" | "Pow"
            ) =>
        {
            let a = ev(l, env, d)?;
            let b = ev(r, env, d)?;
            if !a.is_number() || !b.is_number() {
                return calc_err("arithmetic works on numbers only");
            }
            if *op == "Pow" && py_compare("Gt", &py_abs(&b)?, &Val::from(10000))? {
                return calc_err("exponent too large");
            }
            py_binop(op, &a, &b)
        }
        Node::UnaryOp("USub", v) => py_neg(&ev(v, env, d)?),
        Node::UnaryOp("UAdd", v) => py_pos(&ev(v, env, d)?),
        Node::Compare(l, ops, cs)
            if ops.len() == 1 && matches!(ops[0], "Lt" | "LtE" | "Gt" | "GtE" | "Eq" | "NotEq") =>
        {
            let a = ev(l, env, d)?;
            let b = ev(&cs[0], env, d)?;
            Ok(Val::Bool(py_compare(ops[0], &a, &b)?))
        }
        Node::List(elts, Ctx::Load) | Node::Tuple(elts, Ctx::Load) => {
            if elts.len() > 10000 {
                return calc_err("list too long");
            }
            Ok(Val::List(
                elts.iter().map(|x| ev(x, env, d)).collect::<R<Vec<_>>>()?,
            ))
        }
        Node::Call(f, args, kws) => match f.as_ref() {
            Node::Name(name, _) if FUNCS.contains(&name.as_str()) => {
                let args = args.iter().map(|a| ev(a, env, d)).collect::<R<Vec<_>>>()?;
                let mut kw = Kwargs::new();
                let mut bad_key = false;
                for (k, v) in kws {
                    let v = ev(v, env, d)?;
                    match k {
                        Some(k) => {
                            kw.insert(k.clone(), v);
                        }
                        None => bad_key = true,
                    }
                }
                if bad_key {
                    return type_err("keywords must be strings");
                }
                call_func(name, args, kw)
            }
            _ => not_allowed(node),
        },
        _ => not_allowed(node),
    }
}

fn exc_to_error(e: Exc) -> CalcError {
    match e {
        Exc::Calc(m) => CalcError(m),
        Exc::Py(kind, msg) => CalcError(format!("{kind}: {msg}")),
    }
}

/// Python's `str.strip()` whitespace set.
fn py_strip(s: &str) -> &str {
    s.trim_matches(|c: char| c.is_whitespace() || matches!(c, '\x1c'..='\x1f'))
}

/// `calc.evaluate(expr, env)`: evaluate one expression with the given variables.
pub fn evaluate(expr: &str, env: &Env) -> Result<Val, CalcError> {
    let s = py_strip(expr)
        .replace('^', "**")
        .replace('×', "*")
        .replace('÷', "/")
        .replace('，', ",");
    if s.is_empty() || s.chars().count() > 2000 {
        return Err(CalcError("empty or too long expression".into()));
    }
    let work = || -> Result<Val, CalcError> {
        let tree = parse_source(&s).map_err(|m| CalcError(format!("syntax error: {m}")))?;
        ev(&tree, env, 0).map_err(exc_to_error)
    };
    if s.chars().count() <= 150 {
        return work();
    }
    // long inputs can nest deeply (200 brackets, ~1000 unary operators): parse and evaluate them on a thread with a
    // stack large enough for the recursion, whatever stack the caller runs on
    std::thread::scope(|scope| {
        std::thread::Builder::new()
            .name("calc".into())
            .stack_size(64 << 20)
            .spawn_scoped(scope, work)
            .map_err(|e| CalcError(format!("RuntimeError: {e}")))?
            .join()
            .unwrap_or_else(|_| {
                Err(CalcError(
                    "RecursionError: maximum recursion depth exceeded".into(),
                ))
            })
    })
}

/// `str.isidentifier()`.
fn is_identifier(s: &str) -> bool {
    let mut chars = s.chars();
    match chars.next() {
        Some(c) if c == '_' || unicode_ident::is_xid_start(c) => {
            chars.all(unicode_ident::is_xid_continue)
        }
        _ => false,
    }
}

/// `calc.run(expressions, variables)`: evaluate the variables in order (each may use the earlier ones), then each
/// expression (at most 50). Each row is `(str(expression), value)`; a failed expression's value is the string
/// `"ERROR: <message>"`, exactly like the Python. A bad variable fails the whole call.
pub fn run(
    expressions: &Value,
    variables: &serde_json::Map<String, Value>,
) -> Result<Vec<(String, Val)>, CalcError> {
    let mut env = Env::new();
    for (k, v) in variables {
        if !is_identifier(k) || FUNCS.contains(&k.as_str()) {
            return Err(CalcError(format!("bad variable name {}", py_str_repr(k))));
        }
        let val = match v {
            Value::String(s) => evaluate(s, &env)?,
            other => Val::from_json(other),
        };
        env.insert(k.clone(), val);
    }
    let list: Vec<Value> = match expressions {
        Value::String(s) => vec![Value::String(s.clone())],
        Value::Array(a) => a.clone(),
        Value::Object(o) => o.keys().map(|k| Value::String(k.clone())).collect(),
        Value::Null | Value::Bool(false) => Vec::new(),
        Value::Number(n) if n.as_f64() == Some(0.0) => Vec::new(),
        other => {
            return Err(CalcError(format!(
                "TypeError: '{}' object is not iterable",
                Val::from_json(other).type_name()
            )))
        }
    };
    Ok(list
        .iter()
        .take(50)
        .map(|x| {
            let text = Val::from_json(x).py_str();
            let v = evaluate(&text, &env).unwrap_or_else(|e| Val::Str(format!("ERROR: {e}")));
            (text, v)
        })
        .collect())
}

/// Call one of the whitelisted functions with positional arguments (`calc.loan(3e6, 3.5, 25, 12)` …).
pub fn call(name: &str, args: Vec<Val>) -> Result<Val, CalcError> {
    if !FUNCS.contains(&name) {
        return Err(CalcError(format!("unknown name {}", py_str_repr(name))));
    }
    call_func(name, args, Kwargs::new()).map_err(exc_to_error)
}

fn as_float_result(v: Result<Val, CalcError>) -> Result<f64, CalcError> {
    let v = v?;
    v.as_f64()
        .ok_or_else(|| CalcError(format!("not a real number: {}", v.repr())))
}

/// `calc.pmt(rate, nper, pv, fv)`.
pub fn pmt(rate: f64, nper: f64, pv: f64, fv: f64) -> Result<f64, CalcError> {
    as_float_result(call(
        "pmt",
        vec![rate.into(), nper.into(), pv.into(), fv.into()],
    ))
}

/// `calc.fv(rate, nper, pmt_, pv)`.
pub fn fv(rate: f64, nper: f64, pmt_: f64, pv: f64) -> Result<f64, CalcError> {
    as_float_result(call(
        "fv",
        vec![rate.into(), nper.into(), pmt_.into(), pv.into()],
    ))
}

/// `calc.loan(principal, annual_rate_pct, years, rows, per_year)` → the result dict.
pub fn loan(
    principal: f64,
    annual_rate_pct: f64,
    years: f64,
    rows: i64,
    per_year: i64,
) -> Result<Val, CalcError> {
    call(
        "loan",
        vec![
            principal.into(),
            annual_rate_pct.into(),
            years.into(),
            rows.into(),
            per_year.into(),
        ],
    )
}

/// `calc.invest(monthly, annual_rate_pct, years, initial, per_year)` → the result dict.
pub fn invest(
    monthly: f64,
    annual_rate_pct: f64,
    years: f64,
    initial: f64,
    per_year: i64,
) -> Result<Val, CalcError> {
    call(
        "invest",
        vec![
            monthly.into(),
            annual_rate_pct.into(),
            years.into(),
            initial.into(),
            per_year.into(),
        ],
    )
}

/// `fmt(v)` thousands-separated with two decimals (`f"{x:,.2f}"`) for a numeric value.
fn money(v: Option<&Val>) -> String {
    match v.and_then(Val::as_f64) {
        Some(f) => format_fixed_commas(f, 2),
        None => v.map(Val::py_str).unwrap_or_default(),
    }
}

/// `calc.fmt(v)`: the text the `calculate` tool shows after `expression = `.
pub fn fmt(v: &Val) -> String {
    let note = |d: &IndexMap<String, Val>| match d.get("note") {
        Some(n) if crate::util::truthy(&n.to_json()) => format!("\nNOTE: {}", n.py_str()),
        _ => String::new(),
    };
    if let Val::Dict(d) = v {
        if d.contains_key("years") && d.contains_key("contributed") {
            let head = format!(
                "final balance {}; contributed {}; gain {}{}",
                money(d.get("balance")),
                money(d.get("contributed")),
                money(d.get("gain")),
                note(d)
            );
            let rows: Vec<String> = match d.get("years") {
                Some(Val::List(rs)) => rs
                    .iter()
                    .map(|r| {
                        let year = r
                            .get("year")
                            .and_then(Val::as_f64)
                            .map(format_g_default)
                            .unwrap_or_default();
                        format!(
                            "| {} | {} | {} | {} |",
                            year,
                            money(r.get("contributed")),
                            money(r.get("balance")),
                            money(r.get("gain"))
                        )
                    })
                    .collect(),
                _ => Vec::new(),
            };
            return format!(
                "{head}\n| year | contributed | balance | gain |\n|---|---|---|---|\n{}",
                rows.join("\n")
            );
        }
    }
    if let Val::Float(f) = v {
        return if f.abs() < 1e15 {
            let s = format_fixed_commas(*f, 6);
            s.trim_end_matches('0').trim_end_matches('.').to_string()
        } else {
            format_g(*f, 6)
        };
    }
    if let Val::Dict(d) = v {
        if d.contains_key("schedule") {
            let head = format!(
                "payment {} × {} periods; total paid {}; total interest {}{}",
                money(d.get("payment")),
                d.get("periods").map(Val::py_str).unwrap_or_default(),
                money(d.get("total_paid")),
                money(d.get("total_interest")),
                note(d)
            );
            let rows: Vec<String> = match d.get("schedule") {
                Some(Val::List(rs)) => rs
                    .iter()
                    .map(|r| {
                        format!(
                            "| {} | {} | {} | {} | {} |",
                            r.get("period").map(Val::py_str).unwrap_or_default(),
                            money(r.get("payment")),
                            money(r.get("principal")),
                            money(r.get("interest")),
                            money(r.get("balance"))
                        )
                    })
                    .collect(),
                _ => Vec::new(),
            };
            return format!(
                "{head}\n| # | payment | principal | interest | balance |\n|---|---|---|---|---|\n{}",
                rows.join("\n")
            );
        }
    }
    v.py_str()
}

/// The `calculate` tool of the agent (`app/runtime/agent.py`): parses `expressions` / `expression` and `variables`
/// from the tool arguments the way the Python did and returns the tool result text.
pub fn calculate_tool(a: &Value) -> String {
    let pick = |k: &str| a.get(k).filter(|v| crate::util::truthy(v)).cloned();
    let mut exprs = pick("expressions")
        .or_else(|| pick("expression"))
        .unwrap_or(Value::Array(Vec::new()));
    if let Value::String(s) = &exprs {
        exprs = if s.trim().starts_with('[') {
            serde_json::from_str(s).unwrap_or_else(|_| Value::Array(vec![Value::String(s.clone())]))
        } else {
            Value::Array(vec![Value::String(s.clone())])
        };
    }
    let mut vars = pick("variables").unwrap_or(Value::Object(Default::default()));
    if let Value::String(s) = &vars {
        match serde_json::from_str::<Value>(s) {
            Ok(v) => vars = v,
            Err(_) => {
                return "ERROR: variables 应为对象 must be an object like {\"r\": \"0.035/12\"}"
                    .to_string()
            }
        }
    }
    let empty = serde_json::Map::new();
    let vars = vars.as_object().unwrap_or(&empty);
    let rows = match run(&exprs, vars) {
        Ok(rows) => rows,
        Err(e) => return format!("ERROR: {e}"),
    };
    if rows.is_empty() {
        return "ERROR: 需要 expressions (give one or more expressions)".to_string();
    }
    rows.iter()
        .map(|(e, v)| format!("{e} = {}", fmt(v)))
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn ev_str(e: &str) -> String {
        match evaluate(e, &Env::new()) {
            Ok(v) => fmt(&v),
            Err(err) => format!("ERROR: {err}"),
        }
    }

    fn num(v: &Val) -> f64 {
        v.as_f64().unwrap()
    }

    fn round_to(x: f64, nd: i64) -> f64 {
        num(&py_round(&Val::Float(x), Some(&Val::from(nd))).unwrap())
    }

    /// tests/test_units.py::test_calculator_is_exact_and_safe
    #[test]
    fn calculator_is_exact_and_safe() {
        let rows = run(
            &json!([
                "pmt(r, 300, 3e6)",
                "283.8/4",
                "2^10",
                "__import__('os')",
                "[1]*10**9",
                "10**100000",
                "x+1"
            ]),
            json!({"r": "0.035/12"}).as_object().unwrap(),
        )
        .unwrap();
        let r: IndexMap<String, Val> = rows.into_iter().collect();
        assert_eq!(round_to(num(&r["pmt(r, 300, 3e6)"]), 2), 15018.71);
        assert_eq!(r["283.8/4"], Val::Float(70.95));
        assert_eq!(r["2^10"], Val::from(1024));
        for k in ["__import__('os')", "[1]*10**9", "10**100000", "x+1"] {
            assert!(r[k].py_str().starts_with("ERROR"), "{k}");
        }
        assert_eq!(
            r["__import__('os')"].py_str(),
            "ERROR: not allowed: Call(func=Name(id='__import__', ctx=Load()), args=[Constant("
        );
        assert_eq!(r["x+1"].py_str(), "ERROR: unknown name 'x'");
        let ln = loan(3e6, 3.5, 25.0, 12, 12).unwrap();
        assert_eq!(ln.get("payment"), Some(&Val::Float(15018.71)));
        let Some(Val::List(sched)) = ln.get("schedule") else {
            panic!()
        };
        assert_eq!(sched[0].get("interest"), Some(&Val::Float(8750.0)));
        assert_eq!(sched.len(), 12);
        assert!(fmt(&ln).contains("| 1 | 15,018.71 | 6,268.71 | 8,750.00 |"));
    }

    /// tests/test_units.py::test_invest_schedule_and_sign_free_fv
    #[test]
    fn invest_schedule_and_sign_free_fv() {
        let v = invest(1000.0, 4.0, 10.0, 0.0, 12).unwrap();
        assert_eq!(v.get("balance"), Some(&Val::Float(147249.8)));
        assert_eq!(v.get("contributed").and_then(Val::as_f64), Some(120000.0));
        assert!(matches!(v.get("years"), Some(Val::List(y)) if y.len() == 10));
        assert_eq!(
            round_to(fv(0.04 / 12.0, 120.0, -1000.0, 0.0).unwrap(), 2),
            147249.8
        );
        assert!(fmt(&v).contains("final balance 147,249.80"));
    }

    /// tests/test_units.py::test_calc_finance_helpers
    #[test]
    fn calc_finance_helpers() {
        let e = |s: &str| evaluate(s, &Env::new()).unwrap();
        assert_eq!(
            round_to(
                num(&e("npv(6, [-50000, 12000, 12000, 12000, 12000, 12000])")),
                2
            ),
            548.37
        );
        assert_eq!(
            round_to(
                num(&e("irr([-50000, 12000, 12000, 12000, 12000, 12000])")),
                2
            ),
            6.40
        );
        let a = e("apr(3000, 265, 12)");
        assert_eq!(a.get("apr_pct"), Some(&Val::Float(10.896)));
        assert_eq!(a.get("effective_pct"), Some(&Val::Float(11.457)));
        assert_eq!(a.get("total_interest"), Some(&Val::from(180)));
        assert_eq!(e("payback(4500, 800)"), Val::Float(5.625));
        let v = e("invest(1358.5, 4, 10, 0, 1)");
        assert_eq!(v.get("balance"), Some(&Val::Float(16310.3)));
        assert!(v.get("note").is_none());
        let v = e("invest(100, 0.04, 10)");
        assert!(v
            .get("note")
            .unwrap()
            .py_str()
            .contains("call again with 4"));
        assert!(fmt(&v).contains("NOTE"));
        assert_eq!(round_to(num(&e("fv(4, 10, 1358.5)")), 1), 16310.3);
        assert_eq!(round_to(num(&e("pmt(0.035/12, 240, -300000)")), 2), 1739.88);
    }

    /// The `calculate` tool text (tests/test_units.py::test_ungrounded_numbers quotes it).
    #[test]
    fn calculate_tool_text() {
        let out = calculate_tool(
            &json!({"expressions": ["6.5*365 - 6.5*3*52", "invest(1358.5, 4, 10, 0, 1)"]}),
        );
        assert!(out.starts_with(
            "6.5*365 - 6.5*3*52 = 1,358.5\ninvest(1358.5, 4, 10, 0, 1) = final balance 16,310.30"
        ));
        assert_eq!(calculate_tool(&json!({"expression": "1+1"})), "1+1 = 2");
        assert_eq!(
            calculate_tool(&json!({"expressions": "[\"2*3\", \"7/2\"]"})),
            "2*3 = 6\n7/2 = 3.5"
        );
        assert_eq!(
            calculate_tool(&json!({})),
            "ERROR: 需要 expressions (give one or more expressions)"
        );
        assert_eq!(
            calculate_tool(&json!({"expression": "x", "variables": "{bad"})),
            "ERROR: variables 应为对象 must be an object like {\"r\": \"0.035/12\"}"
        );
        assert_eq!(
            calculate_tool(&json!({"expression": "x", "variables": {"pmt": 1}})),
            "ERROR: bad variable name 'pmt'"
        );
        assert_eq!(
            calculate_tool(&json!({"expression": "y", "variables": {"y": "1/0"}})),
            "ERROR: ZeroDivisionError: division by zero"
        );
    }

    /// Results and messages checked against CPython 3.12.
    #[test]
    fn matches_python() {
        let cases = [
            ("1,358.5", "[1, 358.5]"),
            ("2**100", "1267650600228229401496703205376"),
            ("6/3", "2"),
            ("1e20", "1e+20"),
            ("1e15", "1e+15"),
            ("123456789012345.678", "123,456,789,012,345.671875"),
            ("0.1+0.2", "0.3"),
            ("1/3", "0.333333"),
            ("round(2.5)", "2"),
            ("round(2.675, 2)", "2.67"),
            ("round(1234.5, -2)", "1,200"),
            ("round(-0.0, 2)", "-0"),
            ("[1, 2.5, True]", "ERROR: not allowed: Constant(value=True)"),
            ("stdev([2,4,4,4,5,5,7,9])", "2.13809"),
            ("sum([0.1,0.2,0.3])", "0.6"),
            ("sum([1e100, 1, -1e100, 1])", "1"),
            ("mean([0.1, 0.2, 0.3])", "0.2"),
            ("median([1, 3])", "2"),
            ("1.0 % 0", "ERROR: ZeroDivisionError: float modulo"),
            ("1//0", "ERROR: ZeroDivisionError: integer division or modulo by zero"),
            ("2**10000.0", "ERROR: OverflowError: (34, 'Result too large')"),
            ("10**400/3", "ERROR: OverflowError: integer division result too large for a float"),
            ("(-2) ** 0.5", "(8.659560562354934e-17+1.4142135623730951j)"),
            ("cagr(100, -200, 2)", "(-99.99999999999999+141.4213562373095j)"),
            ("sqrt(-1)", "ERROR: ValueError: math domain error"),
            ("log(10**400, 10)", "400"),
            ("mean([])", "ERROR: StatisticsError: fmean requires at least one data point"),
            ("min([], default=3)", "3"),
            ("max(1, [1])", "ERROR: TypeError: '>' not supported between instances of 'list' and 'int'"),
            ("round(x=1)", "ERROR: TypeError: round() missing required argument 'number' (pos 1)"),
            ("round(1, number=2)", "ERROR: TypeError: argument for round() given by name ('number') and position (1)"),
            ("sum()", "ERROR: TypeError: sum() takes at least 1 positional argument (0 given)"),
            ("cagr()", "ERROR: TypeError: cagr() missing 3 required positional arguments: 'start', 'end', and 'years'"),
            ("loan(1,2,3,4,5,6)", "ERROR: TypeError: loan() takes from 3 to 5 positional arguments but 6 were given"),
            ("pmt(1, 2, 3, rate=4)", "ERROR: TypeError: pmt() got multiple values for argument 'rate'"),
            ("pmt(**[1])", "ERROR: TypeError: keywords must be strings"),
            ("irr([1,2])", "ERROR: irr needs both negative (money out) and positive (money in) cash flows"),
            ("invest(1, 2, 0)", "ERROR: years must give 1–1200 periods"),
            ("npv(6, [1, [2]])", "ERROR: cash flows must be numbers, e.g. [-50000, 12000, 12000]"),
            ("", "ERROR: empty or too long expression"),
            ("1 +", "ERROR: syntax error: invalid syntax"),
            ("(1", "ERROR: syntax error: '(' was never closed"),
            ("1]", "ERROR: syntax error: unmatched ']'"),
            ("(1]", "ERROR: syntax error: closing parenthesis ']' does not match opening parenthesis '('"),
            ("f(2 3)", "ERROR: syntax error: invalid syntax. Perhaps you forgot a comma?"),
            ("1 if 2", "ERROR: syntax error: expected 'else' after 'if' expression"),
            ("print 1", "ERROR: syntax error: Missing parentheses in call to 'print'. Did you mean print(...)?"),
            ("012", "ERROR: syntax error: leading zeros in decimal integer literals are not permitted; use an 0o prefix for octal integers"),
            ("1a", "ERROR: syntax error: invalid decimal literal"),
            ("0b2", "ERROR: syntax error: invalid digit '2' in binary literal"),
            ("¥3", "ERROR: syntax error: invalid character '¥' (U+00A5)"),
            ("\"abc", "ERROR: syntax error: unterminated string literal (detected at line 1)"),
            ("f(a=1, 2)", "ERROR: syntax error: positional argument follows keyword argument"),
            ("f(1=2)", "ERROR: syntax error: expression cannot contain assignment, perhaps you meant \"==\"?"),
            ("lambda: 1", "ERROR: not allowed: Lambda(args=arguments(posonlyargs=[], args=[], kwonlyargs=[]"),
            ("[x for x in y]", "ERROR: not allowed: ListComp(elt=Name(id='x', ctx=Load()), generators=[comprehen"),
            ("1 < 2 < 3", "ERROR: not allowed: Compare(left=Constant(value=1), ops=[Lt(), Lt()], comparator"),
            ("a１", "ERROR: unknown name 'a1'"),
            ("3 × 4 ÷ 2", "6"),
            ("1_000 + 0x10 + 0o7 + 0b1", "1024"),
            ("9007199254740993.0", "9.0072e+15"),
            ("0.0000001", "0"),
            ("-0.0000001", "-0"),
        ];
        for (e, want) in cases {
            assert_eq!(ev_str(e), want, "{e}");
        }
        let deep = format!("{}1", "-".repeat(975));
        assert_eq!(
            ev_str(&deep),
            "ERROR: RecursionError: maximum recursion depth exceeded"
        );
        assert_eq!(ev_str(&format!("{}1", "-".repeat(900))), "1");
        assert_eq!(
            ev_str(&format!("{}1{}", "(".repeat(200), ")".repeat(200))),
            "1"
        );
        assert_eq!(
            ev_str(&"[".repeat(201)),
            "ERROR: syntax error: too many nested parentheses"
        );
    }

    #[test]
    fn loan_and_invest_formatting() {
        let ln = evaluate("loan(300000, 0.5, 2, 3)", &Env::new()).unwrap();
        assert_eq!(
            fmt(&ln),
            "payment 12,565.21 × 24 periods; total paid 301,565.00; total interest 1,565.00\nNOTE: annual_rate_pct=0.5 is read as 0.5% a year. If you meant 50%, call again with 50.\n| # | payment | principal | interest | balance |\n|---|---|---|---|---|\n| 1 | 12,565.21 | 12,440.21 | 125.00 | 287,559.79 |\n| 2 | 12,565.21 | 12,445.39 | 119.82 | 275,114.40 |\n| 3 | 12,565.21 | 12,450.58 | 114.63 | 262,663.82 |"
        );
        let empty = evaluate("loan(3e6, 3.5, 25, 0)", &Env::new()).unwrap();
        assert!(fmt(&empty).ends_with("|---|---|---|---|---|\n"));
        let v = evaluate("invest(100, 4, 1.5, 50, 2)", &Env::new()).unwrap();
        assert_eq!(
            fmt(&v),
            "final balance 359.10; contributed 350.00; gain 9.10\n| year | contributed | balance | gain |\n|---|---|---|---|\n| 1 | 250.00 | 254.02 | 4.02 |\n| 1.5 | 350.00 | 359.10 | 9.10 |"
        );
        let a = evaluate("apr(3000, 265, 12)", &Env::new()).unwrap();
        assert_eq!(fmt(&a), "{'apr_pct': 10.896, 'effective_pct': 11.457, 'period_rate_pct': 0.908, 'total_interest': 180}");
    }

    /// dataq calls `evaluate(expr, env)` with float column values.
    #[test]
    fn evaluate_with_env() {
        let mut env = Env::new();
        env.insert("金额_新元".into(), Val::Float(800.0));
        env.insert("x".into(), Val::from(3));
        assert_eq!(evaluate("金额_新元 * 2", &env).unwrap(), Val::Float(1600.0));
        assert_eq!(evaluate("x > 2", &env).unwrap(), Val::Bool(true));
        assert_eq!(evaluate("x // 2", &env).unwrap(), Val::from(1));
        assert_eq!(evaluate("y", &env).unwrap_err().0, "unknown name 'y'");
    }

    #[test]
    fn number_helpers() {
        assert_eq!(format_g(1e15, 6), "1e+15");
        assert_eq!(format_g(9007199254740992.0, 6), "9.0072e+15");
        assert_eq!(format_g(0.0001, 6), "0.0001");
        assert_eq!(format_g(0.00001, 6), "1e-05");
        assert_eq!(format_fixed_commas(-1234567.891, 2), "-1,234,567.89");
        assert_eq!(
            int_truediv(&BigInt::from(10).pow(30u32), &BigInt::from(7)).unwrap(),
            1.4285714285714285e29
        );
        assert_eq!(complex_repr(0.0, 1.5), "1.5j");
        assert_eq!(complex_repr(1.0, -2.0), "(1-2j)");
    }
}
