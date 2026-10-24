//! A small, private port of `app/common/calc.py:evaluate` for `dataq` derive expressions
//! (`omuse_common::calc` is not available yet; swap this for `omuse_common::calc::evaluate` once it lands).
//!
//! Supports what row expressions use: numbers, column aliases, `+ - * / // % **`, unary signs, one comparison,
//! parentheses, lists and the pure functions of the calculator whitelist (abs, round, min, max, sum, len, int,
//! float, sqrt, log, log10, exp, ceil, floor, mean, median, stdev, pmt, fv, cagr, npv, irr, payback).
//! Results keep Python's int/float distinction. Errors carry the Python text (`CalcError` messages).

use indexmap::IndexMap;

use crate::pyfmt;

/// A Python value an expression can produce.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Val {
    Int(i64),
    Float(f64),
    Bool(bool),
    List(Vec<Val>),
}

impl Val {
    fn num(&self) -> Option<f64> {
        match self {
            Val::Int(i) => Some(*i as f64),
            Val::Float(f) => Some(*f),
            Val::Bool(b) => Some(if *b { 1.0 } else { 0.0 }),
            Val::List(_) => None,
        }
    }

    fn int(&self) -> Option<i64> {
        match self {
            Val::Int(i) => Some(*i),
            Val::Bool(b) => Some(*b as i64),
            _ => None,
        }
    }

    fn type_name(&self) -> &'static str {
        match self {
            Val::Int(_) => "int",
            Val::Float(_) => "float",
            Val::Bool(_) => "bool",
            Val::List(_) => "list",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
enum Tok {
    Num(Val),
    Name(String),
    Op(&'static str),
    LParen,
    RParen,
    LBracket,
    RBracket,
    Comma,
}

fn tokenize(s: &str) -> Result<Vec<Tok>, String> {
    let chars: Vec<char> = s.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c.is_whitespace() {
            i += 1;
            continue;
        }
        if c.is_ascii_digit()
            || (c == '.'
                && chars
                    .get(i + 1)
                    .map(|d| d.is_ascii_digit())
                    .unwrap_or(false))
        {
            let start = i;
            let mut is_float = false;
            while i < chars.len() && (chars[i].is_ascii_digit() || chars[i] == '_') {
                i += 1;
            }
            if i < chars.len() && chars[i] == '.' {
                is_float = true;
                i += 1;
                while i < chars.len() && (chars[i].is_ascii_digit() || chars[i] == '_') {
                    i += 1;
                }
            }
            if i < chars.len() && (chars[i] == 'e' || chars[i] == 'E') {
                let mut j = i + 1;
                if j < chars.len() && (chars[j] == '+' || chars[j] == '-') {
                    j += 1;
                }
                if j < chars.len() && chars[j].is_ascii_digit() {
                    is_float = true;
                    i = j;
                    while i < chars.len() && chars[i].is_ascii_digit() {
                        i += 1;
                    }
                }
            }
            let text: String = chars[start..i].iter().filter(|c| **c != '_').collect();
            if is_float {
                let f: f64 = text.parse().map_err(|_| "invalid syntax".to_string())?;
                out.push(Tok::Num(Val::Float(f)));
            } else {
                match text.parse::<i64>() {
                    Ok(n) => out.push(Tok::Num(Val::Int(n))),
                    Err(_) => {
                        let f: f64 = text.parse().map_err(|_| "invalid syntax".to_string())?;
                        out.push(Tok::Num(Val::Float(f)));
                    }
                }
            }
            continue;
        }
        if c.is_alphabetic() || c == '_' {
            let start = i;
            while i < chars.len() && (chars[i].is_alphanumeric() || chars[i] == '_') {
                i += 1;
            }
            out.push(Tok::Name(chars[start..i].iter().collect()));
            continue;
        }
        let two: String = chars[i..chars.len().min(i + 2)].iter().collect();
        let op: Option<&'static str> = match two.as_str() {
            "**" => Some("**"),
            "//" => Some("//"),
            "<=" => Some("<="),
            ">=" => Some(">="),
            "==" => Some("=="),
            "!=" => Some("!="),
            _ => None,
        };
        if let Some(op) = op {
            out.push(Tok::Op(op));
            i += 2;
            continue;
        }
        let tok = match c {
            '+' => Tok::Op("+"),
            '-' => Tok::Op("-"),
            '*' => Tok::Op("*"),
            '/' => Tok::Op("/"),
            '%' => Tok::Op("%"),
            '<' => Tok::Op("<"),
            '>' => Tok::Op(">"),
            '(' => Tok::LParen,
            ')' => Tok::RParen,
            '[' => Tok::LBracket,
            ']' => Tok::RBracket,
            ',' => Tok::Comma,
            _ => return Err("invalid syntax".into()),
        };
        out.push(tok);
        i += 1;
    }
    Ok(out)
}

/// Parsed expression tree (a subset of Python's `ast`).
#[derive(Debug, Clone)]
enum Node {
    Const(Val),
    Name(String),
    Bin(&'static str, Box<Node>, Box<Node>),
    Unary(&'static str, Box<Node>),
    Cmp(&'static str, Box<Node>, Box<Node>),
    List(Vec<Node>),
    Call(String, Vec<Node>),
}

struct Parser {
    toks: Vec<Tok>,
    pos: usize,
}

impl Parser {
    fn peek(&self) -> Option<&Tok> {
        self.toks.get(self.pos)
    }

    fn next(&mut self) -> Option<Tok> {
        let t = self.toks.get(self.pos).cloned();
        self.pos += 1;
        t
    }

    fn expect(&mut self, t: Tok) -> Result<(), String> {
        if self.peek() == Some(&t) {
            self.pos += 1;
            Ok(())
        } else {
            Err("invalid syntax".into())
        }
    }

    fn parse_expr(&mut self) -> Result<Node, String> {
        let left = self.parse_arith()?;
        if let Some(Tok::Op(op)) = self.peek().cloned() {
            if matches!(op, "<" | "<=" | ">" | ">=" | "==" | "!=") {
                self.pos += 1;
                let right = self.parse_arith()?;
                if let Some(Tok::Op(op2)) = self.peek() {
                    if matches!(*op2, "<" | "<=" | ">" | ">=" | "==" | "!=") {
                        return Err("not allowed: Compare(".into());
                    }
                }
                return Ok(Node::Cmp(op, Box::new(left), Box::new(right)));
            }
        }
        Ok(left)
    }

    fn parse_arith(&mut self) -> Result<Node, String> {
        let mut left = self.parse_term()?;
        while let Some(Tok::Op(op)) = self.peek().cloned() {
            if op == "+" || op == "-" {
                self.pos += 1;
                let right = self.parse_term()?;
                left = Node::Bin(op, Box::new(left), Box::new(right));
            } else {
                break;
            }
        }
        Ok(left)
    }

    fn parse_term(&mut self) -> Result<Node, String> {
        let mut left = self.parse_unary()?;
        while let Some(Tok::Op(op)) = self.peek().cloned() {
            if matches!(op, "*" | "/" | "//" | "%") {
                self.pos += 1;
                let right = self.parse_unary()?;
                left = Node::Bin(op, Box::new(left), Box::new(right));
            } else {
                break;
            }
        }
        Ok(left)
    }

    fn parse_unary(&mut self) -> Result<Node, String> {
        if let Some(Tok::Op(op)) = self.peek().cloned() {
            if op == "-" || op == "+" {
                self.pos += 1;
                let operand = self.parse_unary()?;
                return Ok(Node::Unary(op, Box::new(operand)));
            }
        }
        self.parse_power()
    }

    fn parse_power(&mut self) -> Result<Node, String> {
        let base = self.parse_atom()?;
        if self.peek() == Some(&Tok::Op("**")) {
            self.pos += 1;
            let exp = self.parse_unary()?; // right associative, unary allowed
            return Ok(Node::Bin("**", Box::new(base), Box::new(exp)));
        }
        Ok(base)
    }

    fn parse_atom(&mut self) -> Result<Node, String> {
        match self.next() {
            Some(Tok::Num(v)) => Ok(Node::Const(v)),
            Some(Tok::Name(n)) => {
                if self.peek() == Some(&Tok::LParen) {
                    self.pos += 1;
                    let mut args = Vec::new();
                    if self.peek() != Some(&Tok::RParen) {
                        loop {
                            args.push(self.parse_expr()?);
                            match self.peek() {
                                Some(Tok::Comma) => {
                                    self.pos += 1;
                                    if self.peek() == Some(&Tok::RParen) {
                                        break;
                                    }
                                }
                                _ => break,
                            }
                        }
                    }
                    self.expect(Tok::RParen)?;
                    Ok(Node::Call(n, args))
                } else {
                    Ok(Node::Name(n))
                }
            }
            Some(Tok::LParen) => {
                let e = self.parse_expr()?;
                if self.peek() == Some(&Tok::Comma) {
                    // a tuple
                    let mut items = vec![e];
                    while self.peek() == Some(&Tok::Comma) {
                        self.pos += 1;
                        if self.peek() == Some(&Tok::RParen) {
                            break;
                        }
                        items.push(self.parse_expr()?);
                    }
                    self.expect(Tok::RParen)?;
                    return Ok(Node::List(items));
                }
                self.expect(Tok::RParen)?;
                Ok(e)
            }
            Some(Tok::LBracket) => {
                let mut items = Vec::new();
                while self.peek() != Some(&Tok::RBracket) {
                    items.push(self.parse_expr()?);
                    if self.peek() == Some(&Tok::Comma) {
                        self.pos += 1;
                    } else {
                        break;
                    }
                }
                self.expect(Tok::RBracket)?;
                Ok(Node::List(items))
            }
            _ => Err("invalid syntax".into()),
        }
    }
}

const FUNCS: &[&str] = &[
    "abs", "round", "min", "max", "sum", "len", "int", "float", "sqrt", "log", "log10", "exp",
    "ceil", "floor", "mean", "median", "stdev", "pmt", "fv", "cagr", "npv", "irr", "payback",
];

fn type_err(op: &str, a: &Val, b: &Val) -> String {
    format!(
        "TypeError: unsupported operand type(s) for {}: '{}' and '{}'",
        op,
        a.type_name(),
        b.type_name()
    )
}

fn arith(op: &str, a: &Val, b: &Val) -> Result<Val, String> {
    let (Some(x), Some(y)) = (a.num(), b.num()) else {
        return Err("arithmetic works on numbers only".into());
    };
    let ints = match (a.int(), b.int()) {
        (Some(i), Some(j)) => Some((i, j)),
        _ => None,
    };
    let both_int = ints.is_some() && !matches!(a, Val::Float(_)) && !matches!(b, Val::Float(_));
    match op {
        "+" => Ok(match ints {
            Some((i, j)) if both_int => i.checked_add(j).map(Val::Int).unwrap_or(Val::Float(x + y)),
            _ => Val::Float(x + y),
        }),
        "-" => Ok(match ints {
            Some((i, j)) if both_int => i.checked_sub(j).map(Val::Int).unwrap_or(Val::Float(x - y)),
            _ => Val::Float(x - y),
        }),
        "*" => Ok(match ints {
            Some((i, j)) if both_int => i.checked_mul(j).map(Val::Int).unwrap_or(Val::Float(x * y)),
            _ => Val::Float(x * y),
        }),
        "/" => {
            if y == 0.0 {
                return Err(if both_int {
                    "ZeroDivisionError: division by zero"
                } else {
                    "ZeroDivisionError: float division by zero"
                }
                .into());
            }
            Ok(Val::Float(x / y))
        }
        "//" => {
            if y == 0.0 {
                return Err(if both_int {
                    "ZeroDivisionError: integer division or modulo by zero"
                } else {
                    "ZeroDivisionError: float floor division by zero"
                }
                .into());
            }
            Ok(match ints {
                Some((i, j)) if both_int => Val::Int(
                    i.div_euclid(j)
                        - if (i.rem_euclid(j) != 0) && (j < 0) {
                            1
                        } else {
                            0
                        },
                ),
                _ => Val::Float((x / y).floor()),
            })
        }
        "%" => {
            if y == 0.0 {
                return Err(if both_int {
                    "ZeroDivisionError: integer modulo by zero"
                } else {
                    "ZeroDivisionError: float modulo"
                }
                .into());
            }
            Ok(match ints {
                Some((i, j)) if both_int => Val::Int(((i % j) + j) % j),
                _ => Val::Float(x - y * (x / y).floor()),
            })
        }
        "**" => {
            if y.abs() > 10000.0 {
                return Err("exponent too large".into());
            }
            if both_int {
                let (i, j) = ints.unwrap();
                if j >= 0 {
                    if let Some(p) = i.checked_pow(j as u32) {
                        return Ok(Val::Int(p));
                    }
                    return Ok(Val::Float(x.powf(y)));
                }
                if i == 0 {
                    return Err(
                        "ZeroDivisionError: 0.0 cannot be raised to a negative power".into(),
                    );
                }
                return Ok(Val::Float(x.powf(y)));
            }
            if x == 0.0 && y < 0.0 {
                return Err("ZeroDivisionError: 0.0 cannot be raised to a negative power".into());
            }
            let r = x.powf(y);
            if r.is_infinite() && x.is_finite() && y.is_finite() {
                return Err("OverflowError: (34, 'Numerical result out of range')".into());
            }
            if r.is_nan() && !x.is_nan() && !y.is_nan() {
                return Err("ValueError: math domain error".into());
            }
            Ok(Val::Float(r))
        }
        _ => Err(type_err(op, a, b)),
    }
}

fn compare(op: &str, a: &Val, b: &Val) -> Result<Val, String> {
    match (a, b) {
        (Val::List(x), Val::List(y)) => {
            let mut ord = std::cmp::Ordering::Equal;
            for (p, q) in x.iter().zip(y.iter()) {
                let lt = matches!(compare("<", p, q)?, Val::Bool(true));
                let gt = matches!(compare(">", p, q)?, Val::Bool(true));
                if lt {
                    ord = std::cmp::Ordering::Less;
                    break;
                }
                if gt {
                    ord = std::cmp::Ordering::Greater;
                    break;
                }
            }
            if ord == std::cmp::Ordering::Equal {
                ord = x.len().cmp(&y.len());
            }
            Ok(Val::Bool(match op {
                "<" => ord.is_lt(),
                "<=" => ord.is_le(),
                ">" => ord.is_gt(),
                ">=" => ord.is_ge(),
                "==" => ord.is_eq(),
                _ => ord.is_ne(),
            }))
        }
        _ => {
            let (Some(x), Some(y)) = (a.num(), b.num()) else {
                if op == "==" {
                    return Ok(Val::Bool(false));
                }
                if op == "!=" {
                    return Ok(Val::Bool(true));
                }
                return Err(format!(
                    "TypeError: '{}' not supported between instances of '{}' and '{}'",
                    op,
                    a.type_name(),
                    b.type_name()
                ));
            };
            Ok(Val::Bool(match op {
                "<" => x < y,
                "<=" => x <= y,
                ">" => x > y,
                ">=" => x >= y,
                "==" => x == y,
                _ => x != y,
            }))
        }
    }
}

fn flatten_args(args: &[Val]) -> Result<Vec<Val>, String> {
    if args.len() == 1 {
        if let Val::List(l) = &args[0] {
            return Ok(l.clone());
        }
        return Err(format!(
            "TypeError: '{}' object is not iterable",
            args[0].type_name()
        ));
    }
    Ok(args.to_vec())
}

fn floats(vals: &[Val]) -> Result<Vec<f64>, String> {
    vals.iter()
        .map(|v| {
            v.num()
                .ok_or_else(|| "TypeError: must be real number".to_string())
        })
        .collect()
}

fn per_period(rate: f64) -> f64 {
    if rate.abs() >= 1.0 {
        rate / 100.0
    } else {
        rate
    }
}

fn call(name: &str, args: Vec<Val>) -> Result<Val, String> {
    let n = args.len();
    let arg = |i: usize| -> Result<f64, String> {
        args.get(i)
            .and_then(|v| v.num())
            .ok_or_else(|| format!("TypeError: {name}() takes real number arguments"))
    };
    let argc = |min: usize, max: usize| -> Result<(), String> {
        if n < min || n > max {
            Err(format!("TypeError: {name}() takes from {min} to {max} positional arguments but {n} were given"))
        } else {
            Ok(())
        }
    };
    match name {
        "abs" => {
            argc(1, 1)?;
            Ok(match &args[0] {
                Val::Int(i) => Val::Int(i.abs()),
                Val::Bool(b) => Val::Int(*b as i64),
                Val::Float(f) => Val::Float(f.abs()),
                Val::List(_) => return Err("TypeError: bad operand type for abs(): 'list'".into()),
            })
        }
        "round" => {
            argc(1, 2)?;
            let x = arg(0)?;
            if n == 1 {
                return Ok(match &args[0] {
                    Val::Int(i) => Val::Int(*i),
                    Val::Bool(b) => Val::Int(*b as i64),
                    _ => {
                        if !x.is_finite() {
                            return Err(if x.is_nan() {
                                "ValueError: cannot convert float NaN to integer"
                            } else {
                                "OverflowError: cannot convert float infinity to integer"
                            }
                            .into());
                        }
                        Val::Int(x.round_ties_even() as i64)
                    }
                });
            }
            let d = args[1]
                .int()
                .ok_or("TypeError: 'float' object cannot be interpreted as an integer")?;
            let m = 10f64.powi(d.unsigned_abs().min(300) as i32);
            Ok(match &args[0] {
                Val::Int(_) | Val::Bool(_) if d >= 0 => Val::Int(args[0].int().unwrap_or(0)),
                Val::Int(i) => Val::Int(((*i as f64 / m).round_ties_even() * m) as i64),
                _ if d >= 0 => Val::Float(pyfmt::round_to(x, d as usize)),
                _ => Val::Float((x / m).round_ties_even() * m),
            })
        }
        "min" | "max" => {
            if n == 0 {
                return Err(format!(
                    "TypeError: {name} expected at least 1 argument, got 0"
                ));
            }
            let items = flatten_args(&args)?;
            if items.is_empty() {
                return Err(format!("ValueError: {name}() iterable argument is empty"));
            }
            let mut best = items[0].clone();
            for it in &items[1..] {
                let better = if name == "min" {
                    compare("<", it, &best)?
                } else {
                    compare(">", it, &best)?
                };
                if better == Val::Bool(true) {
                    best = it.clone();
                }
            }
            Ok(best)
        }
        "sum" => {
            argc(1, 2)?;
            let items = match &args[0] {
                Val::List(l) => l.clone(),
                other => {
                    return Err(format!(
                        "TypeError: '{}' object is not iterable",
                        other.type_name()
                    ))
                }
            };
            let mut acc = if n == 2 { args[1].clone() } else { Val::Int(0) };
            for it in &items {
                acc = arith("+", &acc, it)?;
            }
            Ok(acc)
        }
        "len" => {
            argc(1, 1)?;
            match &args[0] {
                Val::List(l) => Ok(Val::Int(l.len() as i64)),
                other => Err(format!(
                    "TypeError: object of type '{}' has no len()",
                    other.type_name()
                )),
            }
        }
        "int" => {
            argc(0, 1)?;
            if n == 0 {
                return Ok(Val::Int(0));
            }
            let x = arg(0)?;
            if x.is_nan() {
                return Err("ValueError: cannot convert float NaN to integer".into());
            }
            if x.is_infinite() {
                return Err("OverflowError: cannot convert float infinity to integer".into());
            }
            Ok(Val::Int(x.trunc() as i64))
        }
        "float" => {
            argc(0, 1)?;
            Ok(Val::Float(if n == 0 { 0.0 } else { arg(0)? }))
        }
        "sqrt" => {
            argc(1, 1)?;
            let x = arg(0)?;
            if x < 0.0 {
                return Err("ValueError: math domain error".into());
            }
            Ok(Val::Float(x.sqrt()))
        }
        "log" | "log10" => {
            argc(1, 2)?;
            let x = arg(0)?;
            if x <= 0.0 {
                return Err("ValueError: math domain error".into());
            }
            if name == "log10" {
                return Ok(Val::Float(x.log10()));
            }
            if n == 2 {
                let b = arg(1)?;
                if b <= 0.0 || b == 1.0 {
                    return Err(if b <= 0.0 {
                        "ValueError: math domain error"
                    } else {
                        "ZeroDivisionError: float division by zero"
                    }
                    .into());
                }
                return Ok(Val::Float(x.ln() / b.ln()));
            }
            Ok(Val::Float(x.ln()))
        }
        "exp" => {
            argc(1, 1)?;
            let r = arg(0)?.exp();
            if r.is_infinite() {
                return Err("OverflowError: math range error".into());
            }
            Ok(Val::Float(r))
        }
        "ceil" | "floor" => {
            argc(1, 1)?;
            let x = arg(0)?;
            if x.is_nan() {
                return Err("ValueError: cannot convert float NaN to integer".into());
            }
            if x.is_infinite() {
                return Err("OverflowError: cannot convert float infinity to integer".into());
            }
            Ok(Val::Int(
                if name == "ceil" { x.ceil() } else { x.floor() } as i64
            ))
        }
        "mean" | "median" | "stdev" => {
            argc(1, 1)?;
            let items = match &args[0] {
                Val::List(l) => floats(l)?,
                other => {
                    return Err(format!(
                        "TypeError: '{}' object is not iterable",
                        other.type_name()
                    ))
                }
            };
            match name {
                "mean" => {
                    if items.is_empty() {
                        return Err(
                            "StatisticsError: fmean requires at least one data point".into()
                        );
                    }
                    Ok(Val::Float(pyfmt::fmean(&items)))
                }
                "median" => {
                    if items.is_empty() {
                        return Err("StatisticsError: no median for empty data".into());
                    }
                    Ok(Val::Float(pyfmt::median(&items)))
                }
                _ => {
                    if items.len() < 2 {
                        return Err(
                            "StatisticsError: variance requires at least two data points".into(),
                        );
                    }
                    Ok(Val::Float(pyfmt::stdev(&items)))
                }
            }
        }
        "pmt" => {
            argc(3, 4)?;
            let (rate, nper, pv) = (per_period(arg(0)?), arg(1)?, arg(2)?.abs());
            let fv = if n == 4 { arg(3)?.abs() } else { 0.0 };
            if rate == 0.0 {
                if nper == 0.0 {
                    return Err("ZeroDivisionError: float division by zero".into());
                }
                return Ok(Val::Float((pv + fv) / nper));
            }
            let g = (1.0 + rate).powf(nper);
            if g - 1.0 == 0.0 {
                return Err("ZeroDivisionError: float division by zero".into());
            }
            Ok(Val::Float((pv * g + fv) * rate / (g - 1.0)))
        }
        "fv" => {
            argc(2, 4)?;
            let (rate, nper) = (per_period(arg(0)?), arg(1)?);
            let pmt = if n >= 3 { arg(2)?.abs() } else { 0.0 };
            let pv = if n >= 4 { arg(3)?.abs() } else { 0.0 };
            if rate == 0.0 {
                return Ok(Val::Float(pv + pmt * nper));
            }
            let g = (1.0 + rate).powf(nper);
            Ok(Val::Float(pv * g + pmt * (g - 1.0) / rate))
        }
        "cagr" => {
            argc(3, 3)?;
            let (s, e, y) = (arg(0)?, arg(1)?, arg(2)?);
            if s == 0.0 || y == 0.0 {
                return Err("ZeroDivisionError: float division by zero".into());
            }
            Ok(Val::Float(((e / s).powf(1.0 / y) - 1.0) * 100.0))
        }
        "npv" => {
            if n < 2 {
                return Err("TypeError: npv() missing required positional arguments".into());
            }
            let r = arg(0)? / 100.0;
            let mut flows = match &args[1] {
                Val::List(l) => floats(l)?,
                other => vec![other
                    .num()
                    .ok_or("cash flows must be numbers, e.g. [-50000, 12000, 12000]")?],
            };
            flows.extend(floats(&args[2..])?);
            if flows.is_empty() {
                return Err("cash flows must be numbers, e.g. [-50000, 12000, 12000]".into());
            }
            let mut acc = Val::Int(0);
            for (t, c) in flows.iter().enumerate() {
                acc = arith("+", &acc, &Val::Float(c / (1.0 + r).powi(t as i32)))?;
            }
            Ok(acc)
        }
        "irr" => {
            if n < 1 {
                return Err("TypeError: irr() missing required positional argument".into());
            }
            let mut flows = match &args[0] {
                Val::List(l) => floats(l)?,
                other => vec![other
                    .num()
                    .ok_or("cash flows must be numbers, e.g. [-50000, 12000, 12000]")?],
            };
            flows.extend(floats(&args[1..])?);
            if !(flows.iter().any(|c| *c < 0.0) && flows.iter().any(|c| *c > 0.0)) {
                return Err(
                    "irr needs both negative (money out) and positive (money in) cash flows".into(),
                );
            }
            let f = |r: f64| {
                flows
                    .iter()
                    .enumerate()
                    .fold(0.0, |a, (t, c)| a + c / (1.0 + r).powi(t as i32))
            };
            let (mut lo, mut hi) = (-0.9999f64, 10.0f64);
            if f(lo) * f(hi) > 0.0 {
                return Err("no IRR between -100% and 1000%".into());
            }
            for _ in 0..200 {
                let mid = (lo + hi) / 2.0;
                if f(lo) * f(mid) <= 0.0 {
                    hi = mid;
                } else {
                    lo = mid;
                }
            }
            Ok(Val::Float((lo + hi) / 2.0 * 100.0))
        }
        "payback" => {
            argc(2, 2)?;
            let (cost, sav) = (arg(0)?, arg(1)?);
            if sav <= 0.0 {
                return Err("saving per period must be positive".into());
            }
            Ok(Val::Float(cost.abs() / sav))
        }
        _ => Err(format!(
            "not allowed: Call(func=Name(id='{name}', ctx=Load()), args=["
        )),
    }
}

fn eval_node(node: &Node, env: &IndexMap<String, f64>) -> Result<Val, String> {
    match node {
        Node::Const(v) => Ok(v.clone()),
        Node::Name(n) => {
            if let Some(v) = env.get(n) {
                return Ok(Val::Float(*v));
            }
            match n.as_str() {
                "pi" => Ok(Val::Float(std::f64::consts::PI)),
                "e" => Ok(Val::Float(std::f64::consts::E)),
                _ => Err(format!("unknown name '{n}'")),
            }
        }
        Node::Bin(op, a, b) => {
            let (x, y) = (eval_node(a, env)?, eval_node(b, env)?);
            arith(op, &x, &y)
        }
        Node::Unary(op, a) => {
            let x = eval_node(a, env)?;
            match (op, &x) {
                (&"-", Val::Int(i)) => Ok(i
                    .checked_neg()
                    .map(Val::Int)
                    .unwrap_or(Val::Float(-(*i as f64)))),
                (&"-", Val::Bool(b)) => Ok(Val::Int(-(*b as i64))),
                (&"-", Val::Float(f)) => Ok(Val::Float(-f)),
                (&"+", Val::Bool(b)) => Ok(Val::Int(*b as i64)),
                (&"+", v @ (Val::Int(_) | Val::Float(_))) => Ok(v.clone()),
                (o, Val::List(_)) => {
                    Err(format!("TypeError: bad operand type for unary {o}: 'list'"))
                }
                _ => Err("not allowed".into()),
            }
        }
        Node::Cmp(op, a, b) => compare(op, &eval_node(a, env)?, &eval_node(b, env)?),
        Node::List(items) => {
            if items.len() > 10000 {
                return Err("list too long".into());
            }
            Ok(Val::List(
                items
                    .iter()
                    .map(|i| eval_node(i, env))
                    .collect::<Result<Vec<_>, _>>()?,
            ))
        }
        Node::Call(name, args) => {
            if !FUNCS.contains(&name.as_str()) {
                return Err(pyfmt::take_chars(
                    &format!("not allowed: Call(func=Name(id='{name}', ctx=Load()), args=["),
                    72,
                )
                .to_string());
            }
            let vals = args
                .iter()
                .map(|a| eval_node(a, env))
                .collect::<Result<Vec<_>, _>>()?;
            call(name, vals)
        }
    }
}

/// `calc.evaluate(expr, env)`: the message of the error is the `CalcError` text.
pub(crate) fn evaluate(expr: &str, env: &IndexMap<String, f64>) -> Result<Val, String> {
    let s = expr
        .trim()
        .replace('^', "**")
        .replace('×', "*")
        .replace('÷', "/")
        .replace('，', ",");
    if s.is_empty() || s.chars().count() > 2000 {
        return Err("empty or too long expression".into());
    }
    let toks = tokenize(&s).map_err(|e| format!("syntax error: {e}"))?;
    let mut p = Parser { toks, pos: 0 };
    let tree = p.parse_expr().map_err(|e| {
        if e.starts_with("not allowed") {
            e
        } else {
            format!("syntax error: {e}")
        }
    })?;
    if p.pos < p.toks.len() {
        return Err("syntax error: invalid syntax".into());
    }
    eval_node(&tree, env)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ev(s: &str) -> Val {
        evaluate(s, &IndexMap::new()).unwrap()
    }

    #[test]
    fn python_semantics() {
        assert_eq!(ev("1 + 2 * 3"), Val::Int(7));
        assert_eq!(ev("7 / 2"), Val::Float(3.5));
        assert_eq!(ev("-7 // 2"), Val::Int(-4));
        assert_eq!(ev("-7 % 3"), Val::Int(2));
        assert_eq!(ev("2 ** 10"), Val::Int(1024));
        assert_eq!(ev("2 ^ -1"), Val::Float(0.5));
        assert_eq!(ev("-2 ** 2"), Val::Int(-4));
        assert_eq!(ev("round(2.5)"), Val::Int(2));
        assert_eq!(ev("round(2.71828, 2)"), Val::Float(2.72));
        assert_eq!(ev("max(1, 2.5, 2)"), Val::Float(2.5));
        assert_eq!(ev("sum([1, 2, 3])"), Val::Int(6));
        assert_eq!(ev("mean([1, 2, 3, 4])"), Val::Float(2.5));
        assert_eq!(ev("3 >= 2"), Val::Bool(true));
        assert_eq!(ev("abs(-3)"), Val::Int(3));
        let mut env = IndexMap::new();
        env.insert("status".to_string(), 503.0);
        assert_eq!(evaluate("status >= 500", &env).unwrap(), Val::Bool(true));
        assert_eq!(evaluate("status * 2", &env).unwrap(), Val::Float(1006.0));
        assert_eq!(
            evaluate("1/0", &env).unwrap_err(),
            "ZeroDivisionError: division by zero"
        );
        assert_eq!(evaluate("foo + 1", &env).unwrap_err(), "unknown name 'foo'");
        assert_eq!(
            evaluate("", &env).unwrap_err(),
            "empty or too long expression"
        );
        assert!(evaluate("1 +", &env)
            .unwrap_err()
            .starts_with("syntax error"));
        assert!((ev("pmt(0.035/12, 300, 3e6)").num().unwrap() - 15018.707108).abs() < 1e-6);
    }
}
