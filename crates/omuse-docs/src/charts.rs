//! Charts and diagrams as SVG, in pure Rust (no plotting library) — `app/common/charts.py`. The browser container
//! turns the SVG into a PNG (JavaScript off, no network), so Chinese text uses the system CJK fonts.
//!
//! Types: line (trends, several series, highest/lowest marked), bar (grouped; horizontal for rankings), pie (shares),
//! gantt (schedules) and flow (flowcharts / architecture: boxes and arrows).

use std::f64::consts::PI;
use std::sync::LazyLock;

use chrono::{Datelike, Days, NaiveDate};
use indexmap::IndexMap;
use regex::Regex;
use serde_json::{json, Value};

use omuse_common::util::truthy;

use crate::pyfmt::{self, fixed, float_str, html_escape, py_iter, py_repr, py_str};

/// Default chart width.
pub const W: i64 = 960;
/// Default chart height.
pub const H: i64 = 540;
/// Font stack of the SVG.
pub const FONT: &str = "'Noto Sans CJK SC','Noto Sans SC','Source Han Sans SC','PingFang SC','Microsoft YaHei','WenQuanYi Micro Hei',sans-serif";
/// Series colours.
pub const PALETTE: [&str; 10] = [
    "#2563eb", "#f97316", "#16a34a", "#dc2626", "#9333ea", "#0891b2", "#ca8a04", "#db2777",
    "#4b5563", "#65a30d",
];
const INK: &str = "#111827";
const MUTED: &str = "#6b7280";
const GRID: &str = "#e5e7eb";
const BG: &str = "#ffffff";
/// The chart types `render` accepts.
pub const TYPES: [&str; 5] = ["line", "bar", "pie", "gantt", "flow"];

/// `ChartError` of the Python module (also carries the `KeyError` / `TypeError` texts the agent reports the same way).
#[derive(Debug, Clone, thiserror::Error)]
#[error("{0}")]
pub struct ChartError(pub String);

type Result<T> = std::result::Result<T, ChartError>;

fn err<T>(msg: impl Into<String>) -> Result<T> {
    Err(ChartError(msg.into()))
}

/// A Python number whose text form depends on whether it is an `int` or a `float` (the SVG prints a few of them raw).
#[derive(Clone, Copy, Debug)]
enum Num {
    I(i64),
    F(f64),
}

impl Num {
    fn f(self) -> f64 {
        match self {
            Num::I(i) => i as f64,
            Num::F(f) => f,
        }
    }

    /// Python `str()`.
    fn s(self) -> String {
        match self {
            Num::I(i) => i.to_string(),
            Num::F(f) => float_str(f),
        }
    }

    fn add(self, o: Num) -> Num {
        match (self, o) {
            (Num::I(a), Num::I(b)) => Num::I(a + b),
            _ => Num::F(self.f() + o.f()),
        }
    }

    fn mul_i(self, k: i64) -> Num {
        match self {
            Num::I(a) => Num::I(a * k),
            Num::F(a) => Num::F(a * k as f64),
        }
    }

    /// Python `min(limit, x)`: the int `limit` unless `x` is smaller.
    fn capped(limit: i64, x: Num) -> Num {
        if x.f() < limit as f64 {
            x
        } else {
            Num::I(limit)
        }
    }
}

/// Python `max(nums)`: the first largest element.
fn num_max(nums: &[Num]) -> Num {
    let mut best = nums[0];
    for n in &nums[1..] {
        if n.f() > best.f() {
            best = *n;
        }
    }
    best
}

fn e(s: &str) -> String {
    html_escape(s, true)
}

fn f1(x: f64) -> String {
    fixed(x, 1)
}

/// Rough text width: CJK glyphs are ~1 em wide, Latin ~0.58 em (an int 0 for the empty string, like Python's `sum`).
fn tw(s: &str, size: f64) -> Num {
    if s.is_empty() {
        return Num::I(0);
    }
    Num::F(s.chars().fold(0.0, |a, ch| {
        a + size * if (ch as u32) > 0x2e80 { 1.0 } else { 0.58 }
    }))
}

fn num(v: &Value) -> Result<Option<f64>> {
    match v {
        Value::Null => return Ok(None),
        Value::String(s) if s.is_empty() => return Ok(None),
        Value::Number(n) => return Ok(n.as_f64().filter(|f| !f.is_nan())),
        _ => {}
    }
    let s = py_str(v)
        .trim()
        .replace([',', '%', '$', '¥'], "")
        .replace("HK", "");
    match s.parse::<f64>() {
        Ok(f) if !s.contains('_') => Ok(Some(f)),
        _ => err(format!("不是数字 not a number: {}", py_repr(v))),
    }
}

fn strip0(s: String) -> String {
    s.trim_end_matches('0').trim_end_matches('.').to_string()
}

fn fmt(v: f64, unit: &str) -> String {
    let a = v.abs();
    let s = if a >= 1e12 {
        format!("{}T", fixed(v / 1e12, 2))
    } else if a >= 1e9 {
        format!("{}B", fixed(v / 1e9, 2))
    } else if a >= 1e6 {
        format!("{}M", fixed(v / 1e6, 2))
    } else if a >= 1e4 {
        pyfmt::fixed_commas(v, 0)
    } else if a >= 100.0 {
        strip0(pyfmt::fixed_commas(v, 1))
    } else {
        strip0(fixed(v, 2))
    };
    s + unit
}

fn nice_ticks(lo: f64, hi: f64) -> Vec<f64> {
    let n = 5.0;
    let (mut lo, mut hi) = (lo, hi);
    if hi == lo {
        let (h0, l0) = (hi, lo);
        hi = h0 + (if h0.abs() != 0.0 { h0.abs() } else { 1.0 }) * 0.5;
        lo = l0 - (if l0.abs() != 0.0 { l0.abs() } else { 1.0 }) * 0.5;
    }
    let raw = (hi - lo) / n;
    let mag = 10f64.powf(raw.log10().floor());
    let step = [1.0, 2.0, 2.5, 5.0, 10.0]
        .iter()
        .map(|m| m * mag)
        .find(|s| *s >= raw)
        .unwrap_or(10.0 * mag);
    let start = (lo / step).floor() * step;
    let mut ticks = Vec::new();
    let mut t = start;
    while t <= hi + step * 0.5 {
        ticks.push(pyfmt::round_to(t, 10));
        t += step;
        if ticks.len() > 10_000 {
            break;
        }
    }
    ticks
}

fn frame(title: &Value, subtitle: &Value, source: &Value, body: &str, w: i64, h: i64) -> String {
    let mut head = if truthy(title) {
        format!(
            "<text x=\"32\" y=\"40\" font-size=\"22\" font-weight=\"700\" fill=\"{INK}\">{}</text>",
            e(&py_str(title))
        )
    } else {
        String::new()
    };
    if truthy(subtitle) {
        head.push_str(&format!(
            "<text x=\"32\" y=\"64\" font-size=\"14\" fill=\"{MUTED}\">{}</text>",
            e(&py_str(subtitle))
        ));
    }
    let foot = if truthy(source) {
        format!(
            "<text x=\"32\" y=\"{}\" font-size=\"12\" fill=\"{MUTED}\">{}</text>",
            h - 14,
            e(&format!("来源 Source: {}", py_str(source)))
        )
    } else {
        String::new()
    };
    format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{w}\" height=\"{h}\" viewBox=\"0 0 {w} {h}\" font-family=\"{FONT}\"><rect width=\"{w}\" height=\"{h}\" fill=\"{BG}\"/>{head}{body}{foot}</svg>"
    )
}

fn legend(names: &[String], x: i64, y: i64, maxw: i64) -> String {
    let mut out = String::new();
    let (mut cx, mut cy) = (Num::I(x), y);
    for (i, n) in names.iter().enumerate() {
        let wd = Num::I(22).add(tw(n, 13.0)).add(Num::I(18));
        if cx.add(wd).f() > (x + maxw) as f64 && cx.f() > x as f64 {
            cx = Num::I(x);
            cy += 20;
        }
        let c = PALETTE[i % PALETTE.len()];
        out.push_str(&format!(
            "<rect x=\"{}\" y=\"{}\" width=\"14\" height=\"10\" rx=\"2\" fill=\"{c}\"/><text x=\"{}\" y=\"{cy}\" font-size=\"13\" fill=\"{INK}\">{}</text>",
            cx.s(),
            cy - 10,
            cx.add(Num::I(20)).s(),
            e(n)
        ));
        cx = cx.add(wd);
    }
    out
}

struct Series {
    name: String,
    values: Vec<Option<f64>>,
}

fn get<'a>(spec: &'a Value, key: &str) -> &'a Value {
    spec.get(key).unwrap_or(&Value::Null)
}

fn get_str(spec: &Value, key: &str) -> String {
    let v = get(spec, key);
    if truthy(v) {
        py_str(v)
    } else {
        String::new()
    }
}

/// `str(d.get(key, ''))`: the value's text when the key exists (even `None`), else "".
fn get_present_str(d: &Value, key: &str) -> String {
    match d.get(key) {
        Some(v) => py_str(v),
        None => String::new(),
    }
}

fn items(v: &Value) -> Result<Vec<Value>> {
    py_iter(v).map_err(ChartError)
}

fn dict<'a>(v: &'a Value, attr: &str) -> Result<&'a serde_json::Map<String, Value>> {
    match v {
        Value::Object(o) => Ok(o),
        other => err(format!(
            "'{}' object has no attribute '{attr}'",
            pyfmt::py_type(other)
        )),
    }
}

fn series(spec: &Value, n: usize) -> Result<Vec<Series>> {
    let mut ser = get(spec, "series").clone();
    if !truthy(&ser) && spec.get("values").map(|v| !v.is_null()).unwrap_or(false) {
        ser = json!([{"name": get_str(spec, "name"), "values": spec["values"]}]);
    }
    if !truthy(&ser) {
        return err("需要 series（[{name, values}]）或 values");
    }
    let mut out = Vec::new();
    for s in items(&ser)?.iter().take(10) {
        dict(s, "get")?;
        let raw = get(s, "values");
        let vals: Vec<Option<f64>> = if truthy(raw) {
            items(raw)?.iter().map(num).collect::<Result<_>>()?
        } else {
            Vec::new()
        };
        if vals.len() != n {
            return err(format!(
                "series「{}」有 {} 个数值，但 labels 有 {n} 个 (values must match labels)",
                get_present_str(s, "name"),
                vals.len()
            ));
        }
        out.push(Series {
            name: get_str(s, "name"),
            values: vals,
        });
    }
    Ok(out)
}

fn plot_area(has_legend: bool, has_sub: bool) -> (i64, i64, i64, i64) {
    let mut top = if has_sub { 92 } else { 76 };
    top += if has_legend { 22 } else { 0 };
    (78, top, W - 40, H - 70) // left, top, right, bottom
}

fn labels_of(spec: &Value) -> Result<Vec<String>> {
    let v = [get(spec, "labels"), get(spec, "x")]
        .into_iter()
        .find(|v| truthy(v));
    match v {
        Some(v) => Ok(items(v)?.iter().map(py_str).collect()),
        None => Ok(Vec::new()),
    }
}

fn has_key_truthy(spec: &Value, key: &str, default: bool) -> bool {
    spec.get(key).map(truthy).unwrap_or(default)
}

fn finish(spec: &Value, body: &str, w: i64, h: i64) -> String {
    frame(
        get(spec, "title"),
        get(spec, "subtitle"),
        get(spec, "source"),
        body,
        w,
        h,
    )
}

// ----------------------------------------------------------------------------------------------- line
/// A line chart.
pub fn line(spec: &Value) -> Result<String> {
    let labels = labels_of(spec)?;
    let n = labels.len();
    if n < 2 {
        return err("折线图至少需要 2 个点 (line chart needs labels with at least 2 points)");
    }
    let ser = series(spec, n)?;
    let unit = get_str(spec, "unit");
    let allv: Vec<f64> = ser
        .iter()
        .flat_map(|s| s.values.iter().flatten().copied())
        .collect();
    if allv.is_empty() {
        return err("没有数值 (no values)");
    }
    let multi = ser.len() > 1;
    let (l, t, r, b) = plot_area(multi, truthy(get(spec, "subtitle")));
    let (vmin, vmax) = (
        allv.iter().cloned().fold(f64::INFINITY, f64::min),
        allv.iter().cloned().fold(f64::NEG_INFINITY, f64::max),
    );
    let pad = {
        let a = (vmax - vmin) * 0.08;
        let c = vmax.abs() * 0.05;
        if a != 0.0 {
            a
        } else if c != 0.0 {
            c
        } else {
            1.0
        }
    };
    let ticks = nice_ticks(vmin - pad, vmax + pad);
    let (lo, hi) = (ticks[0], ticks[ticks.len() - 1]);
    let xs = |i: usize| l as f64 + ((r - l) * i as i64) as f64 / (n - 1) as f64;
    let ys = |v: f64| b as f64 - ((b - t) as f64 * (v - lo)) / (hi - lo);
    let mut g = String::new();
    for tk in &ticks {
        let y = ys(*tk);
        g.push_str(&format!(
            "<line x1=\"{l}\" y1=\"{}\" x2=\"{r}\" y2=\"{}\" stroke=\"{GRID}\"/><text x=\"{}\" y=\"{}\" font-size=\"12\" fill=\"{MUTED}\" text-anchor=\"end\">{}</text>",
            f1(y),
            f1(y),
            l - 8,
            f1(y + 4.0),
            e(&fmt(*tk, &unit))
        ));
    }
    let step = ((n as f64 / 10.0).ceil() as usize).max(1);
    for i in (0..n).step_by(step) {
        g.push_str(&format!(
            "<text x=\"{}\" y=\"{}\" font-size=\"12\" fill=\"{MUTED}\" text-anchor=\"middle\">{}</text>",
            f1(xs(i)),
            b + 20,
            e(&labels[i])
        ));
    }
    if (n - 1) % step != 0 {
        g.push_str(&format!(
            "<text x=\"{}\" y=\"{}\" font-size=\"12\" fill=\"{MUTED}\" text-anchor=\"middle\">{}</text>",
            f1(xs(n - 1)),
            b + 20,
            e(&labels[n - 1])
        ));
    }
    g.push_str(&format!(
        "<line x1=\"{l}\" y1=\"{b}\" x2=\"{r}\" y2=\"{b}\" stroke=\"#9ca3af\"/>"
    ));
    for (k, s) in ser.iter().enumerate() {
        let c = PALETTE[k % PALETTE.len()];
        let mut pts: Vec<String> = Vec::new();
        let mut segs: Vec<Vec<String>> = Vec::new();
        for (i, v) in s.values.iter().enumerate() {
            match v {
                None => {
                    if !pts.is_empty() {
                        segs.push(std::mem::take(&mut pts));
                    }
                }
                Some(v) => pts.push(format!("{},{}", f1(xs(i)), f1(ys(*v)))),
            }
        }
        if !pts.is_empty() {
            segs.push(pts);
        }
        for p in segs {
            g.push_str(&format!(
                "<polyline points=\"{}\" fill=\"none\" stroke=\"{c}\" stroke-width=\"2.5\" stroke-linejoin=\"round\"/>",
                p.join(" ")
            ));
        }
        if n <= 24 {
            for (i, v) in s.values.iter().enumerate() {
                if let Some(v) = v {
                    g.push_str(&format!(
                        "<circle cx=\"{}\" cy=\"{}\" r=\"3\" fill=\"{c}\"/>",
                        f1(xs(i)),
                        f1(ys(*v))
                    ));
                }
            }
        }
    }
    if has_key_truthy(spec, "mark_extremes", ser.len() == 1) {
        for (k, s) in ser.iter().take(2).enumerate() {
            let vals: Vec<(f64, usize)> = s
                .values
                .iter()
                .enumerate()
                .filter_map(|(i, v)| v.map(|v| (v, i)))
                .collect();
            if vals.is_empty() {
                continue;
            }
            let cmpf = |a: &(f64, usize), b: &(f64, usize)| {
                a.0.partial_cmp(&b.0)
                    .unwrap_or(std::cmp::Ordering::Equal)
                    .then(a.1.cmp(&b.1))
            };
            let hi_pt = *vals.iter().max_by(|a, b| cmpf(a, b)).unwrap();
            let lo_pt = *vals.iter().min_by(|a, b| cmpf(a, b)).unwrap();
            for ((v, i), word, dy0) in [(hi_pt, "最高 High", -12i64), (lo_pt, "最低 Low", 22)] {
                let c = PALETTE[k % PALETTE.len()];
                let (x, y) = (xs(i), ys(v));
                let mut dy = dy0;
                if y + dy as f64 > (b - 6) as f64 || y + (dy as f64) < (t + 4) as f64 {
                    // keep the note off the axis / title
                    dy = if dy > 0 { -dy } else { 22 };
                }
                let txt = format!("{word} {} ({})", fmt(v, &unit), labels[i]);
                let half = tw(&txt, 12.0).f() / 2.0;
                let anchor = if x > r as f64 - half {
                    "end"
                } else if x < l as f64 + half {
                    "start"
                } else {
                    "middle"
                };
                g.push_str(&format!(
                    "<circle cx=\"{}\" cy=\"{}\" r=\"5.5\" fill=\"none\" stroke=\"{c}\" stroke-width=\"2\"/><text x=\"{}\" y=\"{}\" font-size=\"12\" font-weight=\"700\" fill=\"{c}\" text-anchor=\"{anchor}\">{}</text>",
                    f1(x),
                    f1(y),
                    f1(x),
                    f1(y + dy as f64),
                    e(&txt)
                ));
            }
        }
    }
    if truthy(get(spec, "y_label")) {
        let mid = float_str((t + b) as f64 / 2.0);
        g.push_str(&format!(
            "<text x=\"18\" y=\"{mid}\" font-size=\"12\" fill=\"{MUTED}\" transform=\"rotate(-90 18 {mid})\" text-anchor=\"middle\">{}</text>",
            e(&py_str(get(spec, "y_label")))
        ));
    }
    if multi {
        g.push_str(&legend(
            &ser.iter().map(|s| s.name.clone()).collect::<Vec<_>>(),
            l,
            t - 14,
            r - l,
        ));
    }
    Ok(finish(spec, &g, W, H))
}

// ----------------------------------------------------------------------------------------------- bar
/// A bar chart (grouped; horizontal when `horizontal` is set).
pub fn bar(spec: &Value) -> Result<String> {
    let mut labels = labels_of(spec)?;
    if labels.is_empty() {
        return err("柱状图需要 labels (bar chart needs labels)");
    }
    let mut ser = series(spec, labels.len())?;
    let unit = get_str(spec, "unit");
    let sort = get_str(spec, "sort");
    if (sort == "desc" || sort == "asc") && get(spec, "sort").is_string() && ser.len() == 1 {
        let mut order: Vec<usize> = (0..labels.len()).collect();
        let key = |i: usize| ser[0].values[i].unwrap_or(0.0);
        order.sort_by(|a, b| {
            let o = key(*a)
                .partial_cmp(&key(*b))
                .unwrap_or(std::cmp::Ordering::Equal);
            if sort == "desc" {
                o.reverse()
            } else {
                o
            }
        });
        labels = order.iter().map(|i| labels[*i].clone()).collect();
        ser[0].values = order.iter().map(|i| ser[0].values[*i]).collect();
    }
    let mut allv: Vec<f64> = ser
        .iter()
        .flat_map(|s| s.values.iter().flatten().copied())
        .collect();
    allv.push(0.0);
    let (vmin, vmax) = (
        allv.iter().cloned().fold(f64::INFINITY, f64::min),
        allv.iter().cloned().fold(f64::NEG_INFINITY, f64::max),
    );
    let ticks = nice_ticks(vmin, vmax);
    let (lo, hi) = (ticks[0], ticks[ticks.len() - 1]);
    let multi = ser.len() > 1;
    let color_each = !multi && has_key_truthy(spec, "color_by_sign", allv.iter().any(|v| *v < 0.0));
    let nser = ser.len() as f64;
    let mut g = String::new();
    let n = labels.len();
    let has_sub = truthy(get(spec, "subtitle"));
    let colour = |v: f64, k: usize| {
        if color_each {
            if v >= 0.0 {
                "#16a34a"
            } else {
                "#dc2626"
            }
        } else {
            PALETTE[k % PALETTE.len()]
        }
    };
    if truthy(get(spec, "horizontal")) {
        let lw = Num::capped(
            260,
            num_max(&labels.iter().map(|x| tw(x, 13.0)).collect::<Vec<_>>()).add(Num::I(16)),
        );
        let (_l0, t, _r0, b) = plot_area(multi, has_sub);
        let l = Num::I(32).add(lw);
        let r = W - 90;
        let band = (b - t) as f64 / n as f64;
        let bh = (band * 0.72 / nser).min(34.0);
        let xs = |v: f64| l.f() + ((r as f64 - l.f()) * (v - lo)) / (hi - lo);
        for tk in &ticks {
            g.push_str(&format!(
                "<line x1=\"{}\" y1=\"{t}\" x2=\"{}\" y2=\"{b}\" stroke=\"{GRID}\"/><text x=\"{}\" y=\"{}\" font-size=\"12\" fill=\"{MUTED}\" text-anchor=\"middle\">{}</text>",
                f1(xs(*tk)),
                f1(xs(*tk)),
                f1(xs(*tk)),
                b + 18,
                e(&fmt(*tk, &unit))
            ));
        }
        for (i, lab) in labels.iter().enumerate() {
            let yc = t as f64 + band * (i as f64 + 0.5);
            g.push_str(&format!(
                "<text x=\"{}\" y=\"{}\" font-size=\"13\" fill=\"{INK}\" text-anchor=\"end\">{}</text>",
                l.add(Num::I(-10)).s(),
                f1(yc + 4.0),
                e(lab)
            ));
            for (k, s) in ser.iter().enumerate() {
                let Some(v) = s.values[i] else { continue };
                let y = yc - bh * nser / 2.0 + k as f64 * bh;
                let (x0, x1) = {
                    let (a, bb) = (xs(0.0), xs(v));
                    if bb < a {
                        (bb, a)
                    } else {
                        (a, bb)
                    }
                };
                let c = colour(v, k);
                g.push_str(&format!(
                    "<rect x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\" rx=\"3\" fill=\"{c}\"/><text x=\"{}\" y=\"{}\" font-size=\"12\" fill=\"{INK}\" text-anchor=\"{}\">{}</text>",
                    f1(x0),
                    f1(y),
                    f1((x1 - x0).max(1.0)),
                    f1(bh - 2.0),
                    f1(if v >= 0.0 { x1 + 6.0 } else { x0 - 6.0 }),
                    f1(y + bh / 2.0 + 3.0),
                    if v >= 0.0 { "start" } else { "end" },
                    e(&fmt(v, &unit))
                ));
            }
        }
        g.push_str(&format!(
            "<line x1=\"{}\" y1=\"{t}\" x2=\"{}\" y2=\"{b}\" stroke=\"#9ca3af\"/>",
            f1(xs(0.0)),
            f1(xs(0.0))
        ));
    } else {
        let (l, t, r, b) = plot_area(multi, has_sub);
        let band = (r - l) as f64 / n as f64;
        let bw = (band * 0.75 / nser).min(56.0);
        let ys = |v: f64| b as f64 - ((b - t) as f64 * (v - lo)) / (hi - lo);
        for tk in &ticks {
            g.push_str(&format!(
                "<line x1=\"{l}\" y1=\"{}\" x2=\"{r}\" y2=\"{}\" stroke=\"{GRID}\"/><text x=\"{}\" y=\"{}\" font-size=\"12\" fill=\"{MUTED}\" text-anchor=\"end\">{}</text>",
                f1(ys(*tk)),
                f1(ys(*tk)),
                l - 8,
                f1(ys(*tk) + 4.0),
                e(&fmt(*tk, &unit))
            ));
        }
        let rot =
            n > 8 || num_max(&labels.iter().map(|x| tw(x, 12.0)).collect::<Vec<_>>()).f() > band;
        for (i, lab) in labels.iter().enumerate() {
            let xc = l as f64 + band * (i as f64 + 0.5);
            if rot {
                g.push_str(&format!(
                    "<text x=\"{}\" y=\"{}\" font-size=\"12\" fill=\"{INK}\" text-anchor=\"end\" transform=\"rotate(-30 {} {})\">{}</text>",
                    f1(xc),
                    b + 14,
                    f1(xc),
                    b + 14,
                    e(lab)
                ));
            } else {
                g.push_str(&format!(
                    "<text x=\"{}\" y=\"{}\" font-size=\"12\" fill=\"{INK}\" text-anchor=\"middle\">{}</text>",
                    f1(xc),
                    b + 20,
                    e(lab)
                ));
            }
            for (k, s) in ser.iter().enumerate() {
                let Some(v) = s.values[i] else { continue };
                let x = xc - bw * nser / 2.0 + k as f64 * bw;
                let (y0, y1) = {
                    let (a, bb) = (ys(0.0), ys(v));
                    if bb < a {
                        (bb, a)
                    } else {
                        (a, bb)
                    }
                };
                let c = colour(v, k);
                g.push_str(&format!(
                    "<rect x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\" rx=\"3\" fill=\"{c}\"/>",
                    f1(x + 1.0),
                    f1(y0),
                    f1(bw - 2.0),
                    f1((y1 - y0).max(1.0))
                ));
                if n * ser.len() <= 24 {
                    let ty = if v >= 0.0 { y0 - 6.0 } else { y1 + 14.0 };
                    g.push_str(&format!(
                        "<text x=\"{}\" y=\"{}\" font-size=\"11\" fill=\"{INK}\" text-anchor=\"middle\">{}</text>",
                        f1(x + bw / 2.0),
                        f1(ty),
                        e(&fmt(v, &unit))
                    ));
                }
            }
        }
        g.push_str(&format!(
            "<line x1=\"{l}\" y1=\"{}\" x2=\"{r}\" y2=\"{}\" stroke=\"#9ca3af\"/>",
            f1(ys(0.0)),
            f1(ys(0.0))
        ));
    }
    if multi {
        g.push_str(&legend(
            &ser.iter().map(|s| s.name.clone()).collect::<Vec<_>>(),
            78,
            (if has_sub { 92 } else { 76 }) - 14,
            W - 120,
        ));
    }
    Ok(finish(spec, &g, W, H))
}

// ----------------------------------------------------------------------------------------------- pie
/// A pie / donut chart.
pub fn pie(spec: &Value) -> Result<String> {
    let labels: Vec<String> = if truthy(get(spec, "labels")) {
        items(get(spec, "labels"))?.iter().map(py_str).collect()
    } else {
        Vec::new()
    };
    let raw_vals: Vec<Value> = if truthy(get(spec, "values")) {
        items(get(spec, "values"))?
    } else {
        let ser = get(spec, "series");
        let first = if truthy(ser) {
            match ser {
                Value::Array(a) => a[0].clone(),
                Value::Object(_) => return err("0"),
                other => {
                    return err(format!(
                        "'{}' object is not subscriptable",
                        pyfmt::py_type(other)
                    ))
                }
            }
        } else {
            json!({})
        };
        dict(&first, "get")?;
        let v = get(&first, "values");
        if truthy(v) {
            items(v)?
        } else {
            Vec::new()
        }
    };
    let vals: Vec<Option<f64>> = raw_vals.iter().map(num).collect::<Result<_>>()?;
    if labels.is_empty() || labels.len() != vals.len() {
        return err(
            "饼图需要等长的 labels 和 values (pie needs labels and values of the same length)",
        );
    }
    if vals.iter().any(|v| v.map(|v| v < 0.0).unwrap_or(true))
        || vals.iter().flatten().fold(0.0, |a, b| a + b) <= 0.0
    {
        return err("饼图的数值必须是正数 (pie values must be positive)");
    }
    let vals: Vec<f64> = vals.into_iter().flatten().collect();
    let tot = vals.iter().fold(0.0, |a, b| a + b);
    let (cx, cy, r) = (330i64, H as f64 / 2.0 + 14.0, 175i64);
    let inner = if has_key_truthy(spec, "donut", true) {
        Num::F(r as f64 * 0.55)
    } else {
        Num::I(0)
    };
    let cys = float_str(cy);
    let mut g = String::new();
    let mut a0 = -PI / 2.0;
    for (i, v) in vals.iter().enumerate() {
        let a1 = a0 + 2.0 * PI * v / tot;
        let c = PALETTE[i % PALETTE.len()];
        if v / tot >= 0.9999 {
            g.push_str(&format!(
                "<circle cx=\"{cx}\" cy=\"{cys}\" r=\"{r}\" fill=\"{c}\"/>"
            ));
        } else {
            let large = if a1 - a0 > PI { 1 } else { 0 };
            let rf = r as f64;
            let p = format!(
                "M{},{} A{r},{r} 0 {large} 1 {},{} L{cx},{cys} Z",
                f1(cx as f64 + rf * a0.cos()),
                f1(cy + rf * a0.sin()),
                f1(cx as f64 + rf * a1.cos()),
                f1(cy + rf * a1.sin())
            );
            g.push_str(&format!(
                "<path d=\"{p}\" fill=\"{c}\" stroke=\"#fff\" stroke-width=\"2\"/>"
            ));
        }
        let am = (a0 + a1) / 2.0;
        if v / tot >= 0.04 {
            let rr = if inner.f() != 0.0 {
                (r as f64 + inner.f()) / 2.0
            } else {
                r as f64 * 0.62
            };
            g.push_str(&format!(
                "<text x=\"{}\" y=\"{}\" font-size=\"14\" font-weight=\"700\" fill=\"#fff\" text-anchor=\"middle\">{}%</text>",
                f1(cx as f64 + rr * am.cos()),
                f1(cy + rr * am.sin() + 5.0),
                fixed(v / tot * 100.0, 0)
            ));
        }
        a0 = a1;
    }
    if inner.f() != 0.0 {
        g.push_str(&format!(
            "<circle cx=\"{cx}\" cy=\"{cys}\" r=\"{}\" fill=\"#fff\"/>",
            inner.s()
        ));
        if truthy(get(spec, "center_text")) {
            g.push_str(&format!(
                "<text x=\"{cx}\" y=\"{}\" font-size=\"18\" font-weight=\"700\" fill=\"{INK}\" text-anchor=\"middle\">{}</text>",
                float_str(cy + 6.0),
                e(&py_str(get(spec, "center_text")))
            ));
        }
    }
    let unit = get_str(spec, "unit");
    let (lx, ly) = (560i64, cy - (labels.len() * 15) as f64);
    for (i, (lab, v)) in labels.iter().zip(&vals).enumerate() {
        let y = ly + (i * 30) as f64;
        let pct = format!("{}%", fixed(v / tot * 100.0, 1));
        let txt = if unit == "%" {
            pct
        } else {
            format!("{}  ·  {pct}", fmt(*v, &unit))
        };
        g.push_str(&format!(
            "<rect x=\"{lx}\" y=\"{}\" width=\"16\" height=\"16\" rx=\"3\" fill=\"{}\"/><text x=\"{}\" y=\"{}\" font-size=\"15\" fill=\"{INK}\">{}</text><text x=\"{}\" y=\"{}\" font-size=\"15\" fill=\"{MUTED}\" text-anchor=\"end\">{}</text>",
            float_str(y - 12.0),
            PALETTE[i % PALETTE.len()],
            lx + 26,
            float_str(y + 1.0),
            e(lab),
            W - 40,
            float_str(y + 1.0),
            e(&txt)
        ));
    }
    Ok(finish(spec, &g, W, H))
}

// ----------------------------------------------------------------------------------------------- gantt
fn date(v: &Value) -> Result<NaiveDate> {
    let s = py_str(v);
    let s = s.trim();
    for f in ["%Y-%m-%d", "%Y/%m/%d", "%Y.%m.%d"] {
        if let Ok(d) = NaiveDate::parse_from_str(s, f) {
            return Ok(d);
        }
    }
    err(format!(
        "日期格式应为 YYYY-MM-DD (date must be YYYY-MM-DD): {}",
        py_repr(&Value::String(s.to_string()))
    ))
}

fn add_days(d: NaiveDate, n: i64) -> NaiveDate {
    if n >= 0 {
        d.checked_add_days(Days::new(n as u64)).unwrap_or(d)
    } else {
        d.checked_sub_days(Days::new(n.unsigned_abs())).unwrap_or(d)
    }
}

/// A Gantt chart of `tasks` (`[{name, start, end or days/weeks}]`).
pub fn gantt(spec: &Value) -> Result<String> {
    let tasks = if truthy(get(spec, "tasks")) {
        items(get(spec, "tasks"))?
    } else {
        Vec::new()
    };
    if tasks.is_empty() {
        return err("甘特图需要 tasks（[{name, start, end 或 days/weeks}]）");
    }
    let mut rows: Vec<(String, NaiveDate, NaiveDate)> = Vec::new();
    let mut prev_end: Option<NaiveDate> = None;
    for t in tasks.iter().take(30) {
        dict(t, "get")?;
        let st = if truthy(get(t, "start")) {
            date(get(t, "start"))?
        } else {
            prev_end.unwrap_or_else(|| chrono::Local::now().date_naive())
        };
        let mut en = if truthy(get(t, "end")) {
            date(get(t, "end"))?
        } else {
            let days = match num(get(t, "days"))? {
                Some(d) if d != 0.0 => d,
                _ => match num(get(t, "weeks"))? {
                    Some(w) if w != 0.0 => w * 7.0,
                    _ => 7.0,
                },
            };
            add_days(st, days.trunc() as i64)
        };
        if en == st {
            // a one-day item or a deadline (2026-10-02 R10-04: "due 10/03" gave start = end)
            en = add_days(st, 1);
        }
        if en < st {
            return err(format!(
                "「{}」的结束日期要晚于开始日期 (end must be after start)",
                get_present_str(t, "name")
            ));
        }
        rows.push((get_str(t, "name"), st, en));
        prev_end = Some(en);
    }
    let d0 = rows.iter().map(|r| r.1).min().unwrap();
    let d1 = rows.iter().map(|r| r.2).max().unwrap();
    let span = {
        let s = (d1 - d0).num_days();
        if s == 0 {
            1
        } else {
            s
        }
    };
    let lw = Num::capped(
        240,
        num_max(&rows.iter().map(|r| tw(&r.0, 13.0)).collect::<Vec<_>>()).add(Num::I(20)),
    );
    let l = Num::I(32).add(lw);
    let t = 116i64;
    let r = W - 40;
    let b0 = H - 60;
    let band = Num::capped(46, Num::F((b0 - t) as f64 / rows.len() as f64));
    let b = Num::I(t).add(band.mul_i(rows.len() as i64));
    let xs = |d: NaiveDate| l.f() + ((r as f64 - l.f()) * (d - d0).num_days() as f64) / span as f64;
    let mut g = String::new();
    let step = if span <= 120 { 7 } else { 30 };
    let mut d = d0;
    while d <= d1 {
        let x = xs(d);
        g.push_str(&format!(
            "<line x1=\"{}\" y1=\"{}\" x2=\"{}\" y2=\"{}\" stroke=\"{GRID}\"/><text x=\"{}\" y=\"{}\" font-size=\"11\" fill=\"{MUTED}\" text-anchor=\"middle\">{}/{}</text>",
            f1(x),
            t - 6,
            f1(x),
            b.s(),
            f1(x),
            t - 12,
            d.month(),
            d.day()
        ));
        d = add_days(d, step);
    }
    for (i, (name, st, en)) in rows.iter().enumerate() {
        let y = Num::I(t).add(band.mul_i(i as i64));
        let c = PALETTE[i % PALETTE.len()];
        let (x0, x1) = (xs(*st), xs(*en));
        let ty = f1(y.f() + band.f() / 2.0 + 4.0);
        g.push_str(&format!(
            "<text x=\"{}\" y=\"{ty}\" font-size=\"13\" fill=\"{INK}\" text-anchor=\"end\">{}</text><rect x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\" rx=\"4\" fill=\"{c}\"/>",
            l.add(Num::I(-10)).s(),
            e(name),
            f1(x0),
            f1(y.f() + band.f() * 0.2),
            f1(x1 - x0),
            f1(band.f() * 0.6)
        ));
        let lab = format!(
            "{}/{}–{}/{} · {}天",
            st.month(),
            st.day(),
            en.month(),
            en.day(),
            (*en - *st).num_days()
        );
        if x1 - x0 > tw(&lab, 11.0).f() + 10.0 {
            g.push_str(&format!(
                "<text x=\"{}\" y=\"{ty}\" font-size=\"11\" fill=\"#fff\" text-anchor=\"middle\">{}</text>",
                f1((x0 + x1) / 2.0),
                e(&lab)
            ));
        } else {
            g.push_str(&format!(
                "<text x=\"{}\" y=\"{ty}\" font-size=\"11\" fill=\"{MUTED}\">{}</text>",
                f1(x1 + 6.0),
                e(&lab)
            ));
        }
    }
    let h = (b.f() + 60.0).trunc() as i64;
    let sub = if truthy(get(spec, "subtitle")) {
        get(spec, "subtitle").clone()
    } else {
        json!(format!(
            "{} → {}（共 {span} 天 {span} days）",
            d0.format("%Y-%m-%d"),
            d1.format("%Y-%m-%d")
        ))
    };
    Ok(frame(
        get(spec, "title"),
        &sub,
        get(spec, "source"),
        &g,
        W,
        h.max(300),
    ))
}

// ----------------------------------------------------------------------------------------------- flow
static WRAP_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"[\u2e80-\u{ffff}]|[^\s\u2e80-\u{ffff}]+|\s+").unwrap());

fn wrap(s: &str, width: f64, size: f64) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut cur = String::new();
    for tok in WRAP_RE.find_iter(s).map(|m| m.as_str()) {
        if tw(&format!("{cur}{tok}"), size).f() > width && !cur.trim().is_empty() {
            out.push(cur.trim().to_string());
            cur = tok.trim_start().to_string();
        } else {
            cur.push_str(tok);
        }
    }
    if !cur.trim().is_empty() {
        out.push(cur.trim().to_string());
    }
    out.truncate(4);
    if out.is_empty() {
        out.push(String::new());
    }
    out
}

/// A flowchart / architecture diagram (`nodes` + `edges`, or `steps`).
pub fn flow(spec: &Value) -> Result<String> {
    let mut nodes = if truthy(get(spec, "nodes")) {
        items(get(spec, "nodes"))?
    } else {
        Vec::new()
    };
    let mut edges = if truthy(get(spec, "edges")) {
        items(get(spec, "edges"))?
    } else {
        Vec::new()
    };
    if nodes.is_empty() && truthy(get(spec, "steps")) {
        let steps = items(get(spec, "steps"))?;
        nodes = steps
            .iter()
            .enumerate()
            .map(|(i, s)| json!({"id": format!("s{i}"), "label": s}))
            .collect();
        edges = (0..nodes.len().saturating_sub(1))
            .map(|i| json!({"from": format!("s{i}"), "to": format!("s{}", i + 1)}))
            .collect();
    }
    if nodes.is_empty() {
        return err(
            "流程图需要 nodes（[{id, label}]）和 edges（[{from, to, label}]），或者 steps（[...]）",
        );
    }
    let mut ids: Vec<String> = Vec::new();
    let mut lab: IndexMap<String, String> = IndexMap::new();
    for (i, n) in nodes.iter().take(40).enumerate() {
        let nid = if n.is_object() {
            let nid = match n.get("id") {
                Some(id) if !id.is_null() => py_str(id),
                _ => {
                    if truthy(get(n, "label")) {
                        py_str(get(n, "label"))
                    } else {
                        format!("n{i}")
                    }
                }
            };
            let l = if truthy(get(n, "label")) {
                py_str(get(n, "label"))
            } else {
                nid.clone()
            };
            lab.insert(nid.clone(), l);
            nid
        } else {
            let nid = py_str(n);
            lab.insert(nid.clone(), nid.clone());
            nid
        };
        if !ids.contains(&nid) {
            ids.push(nid);
        }
    }
    let mut es: Vec<(String, String, String)> = Vec::new();
    for ed in edges.iter().take(80) {
        dict(ed, "get")?;
        let (a, b) = (py_str(get(ed, "from")), py_str(get(ed, "to")));
        if !lab.contains_key(&a) || !lab.contains_key(&b) {
            return err(format!(
                "连线指向不存在的节点 edge refers to an unknown node: {a} → {b}"
            ));
        }
        es.push((a, b, get_str(ed, "label")));
    }
    // layers = longest path from the sources, after dropping back edges (loops such as "rejected → resubmit")
    let adj: IndexMap<&str, Vec<&str>> = ids
        .iter()
        .map(|n| {
            (
                n.as_str(),
                es.iter()
                    .filter(|(a, _, _)| a == n)
                    .map(|(_, b, _)| b.as_str())
                    .collect(),
            )
        })
        .collect();
    let mut state: IndexMap<&str, u8> = IndexMap::new();
    let mut back: Vec<(String, String)> = Vec::new();
    fn dfs<'a>(
        u: &'a str,
        adj: &IndexMap<&'a str, Vec<&'a str>>,
        state: &mut IndexMap<&'a str, u8>,
        back: &mut Vec<(String, String)>,
    ) {
        state.insert(u, 1);
        if let Some(vs) = adj.get(u) {
            for v in vs {
                match state.get(v).copied().unwrap_or(0) {
                    1 => back.push((u.to_string(), v.to_string())),
                    0 => dfs(v, adj, state, back),
                    _ => {}
                }
            }
        }
        state.insert(u, 2);
    }
    for n in &ids {
        if state.get(n.as_str()).copied().unwrap_or(0) == 0 {
            dfs(n.as_str(), &adj, &mut state, &mut back);
        }
    }
    let mut layer: IndexMap<&str, i64> = ids.iter().map(|n| (n.as_str(), 0)).collect();
    for _ in 0..ids.len() {
        for (a, b, _) in &es {
            if !back.contains(&(a.clone(), b.clone())) && layer[b.as_str()] < layer[a.as_str()] + 1
            {
                let v = layer[a.as_str()] + 1;
                layer.insert(b.as_str(), v);
            }
        }
    }
    let nl = layer.values().copied().max().unwrap_or(0) + 1;
    let groups: Vec<Vec<&str>> = (0..nl)
        .map(|k| {
            ids.iter()
                .filter(|n| layer[n.as_str()] == k)
                .map(|n| n.as_str())
                .collect()
        })
        .collect();
    let maxg = groups.iter().map(|g| g.len()).max().unwrap_or(1) as i64;
    let direction = get_str(spec, "direction").to_uppercase();
    let lr = direction == "LR" || (nl <= 6 && maxg <= 2 && direction != "TB");
    let (bw, bh) = (170i64, 64i64);
    let (w, h);
    let mut pos: IndexMap<&str, (f64, f64)> = IndexMap::new();
    if lr {
        let gapx = ((W - 80 - nl * bw) as f64 / (nl - 1).max(1) as f64).clamp(60.0, 110.0);
        w = ((80 + nl * bw) as f64 + (nl - 1) as f64 * gapx)
            .max(W as f64)
            .trunc() as i64;
        h = (260 + maxg * (bh + 46)).max(420);
        for (k, g) in groups.iter().enumerate() {
            let tot = (g.len() as i64) * bh + (g.len() as i64 - 1) * 46;
            for (j, n) in g.iter().enumerate() {
                pos.insert(
                    n,
                    (
                        40.0 + k as f64 * (bw as f64 + gapx),
                        (h - 40 - tot) as f64 / 2.0 + 40.0 + (j as i64 * (bh + 46)) as f64,
                    ),
                );
            }
        }
    } else {
        let gapy = 56i64;
        w = (160 + maxg * (bw + 40)).max(W);
        h = 120 + nl * (bh + gapy);
        for (k, g) in groups.iter().enumerate() {
            let tot = (g.len() as i64) * bw + (g.len() as i64 - 1) * 40;
            for (j, n) in g.iter().enumerate() {
                pos.insert(
                    n,
                    (
                        (w - tot) as f64 / 2.0 + (j as i64 * (bw + 40)) as f64,
                        (90 + k as i64 * (bh + gapy)) as f64,
                    ),
                );
            }
        }
    }
    let mut g = format!(
        "<defs><marker id=\"arr\" viewBox=\"0 0 10 10\" refX=\"9\" refY=\"5\" markerWidth=\"8\" markerHeight=\"8\" orient=\"auto-start-reverse\"><path d=\"M0,0 L10,5 L0,10 z\" fill=\"{MUTED}\"/></marker></defs>"
    );
    let top = pos.values().map(|p| p.1).fold(f64::INFINITY, f64::min);
    let low = pos.values().map(|p| p.1).fold(f64::NEG_INFINITY, f64::max) + bh as f64; // lowest box edge (LR) / rightmost (TB)
    let right = pos.values().map(|p| p.0).fold(f64::NEG_INFINITY, f64::max) + bw as f64;
    let (bwf, bhf) = (bw as f64, bh as f64);
    for (a, b, l) in &es {
        let ((xa, ya), (xb, yb)) = (pos[a.as_str()], pos[b.as_str()]);
        let span = layer[b.as_str()] - layer[a.as_str()];
        let (p0, p1, p2, p3);
        if lr {
            if span == 1 && xb > xa {
                // neighbouring columns: straight-ish S curve
                p0 = (xa + bwf, ya + bhf / 2.0);
                p3 = (xb - 2.0, yb + bhf / 2.0);
                let mx = (p0.0 + p3.0) / 2.0;
                p1 = (mx, p0.1);
                p2 = (mx, p3.1);
            } else {
                // skips columns or goes back: detour below the boxes, or above them when a box sits under the
                // start/end in its column
                let below = ids.iter().filter(|n2| *n2 != a && *n2 != b).any(|n2| {
                    (layer[n2.as_str()] == layer[a.as_str()] && pos[n2.as_str()].1 > ya)
                        || (layer[n2.as_str()] == layer[b.as_str()] && pos[n2.as_str()].1 > yb)
                });
                let dip;
                if below {
                    p0 = (xa + bwf / 2.0, ya);
                    p3 = (xb + bwf / 2.0, yb - 2.0);
                    dip = top - 36.0 - 14.0 * span.abs() as f64;
                } else {
                    p0 = (xa + bwf / 2.0, ya + bhf);
                    p3 = (xb + bwf / 2.0, yb + bhf + 2.0);
                    dip = low + 36.0 + 14.0 * span.abs() as f64;
                }
                p1 = (p0.0, dip);
                p2 = (p3.0, dip);
            }
        } else if span == 1 && yb > ya {
            p0 = (xa + bwf / 2.0, ya + bhf);
            p3 = (xb + bwf / 2.0, yb - 2.0);
            let my = (p0.1 + p3.1) / 2.0;
            p1 = (p0.0, my);
            p2 = (p3.0, my);
        } else {
            // detour to the right of the boxes
            p0 = (xa + bwf, ya + bhf / 2.0);
            p3 = (xb + bwf + 2.0, yb + bhf / 2.0);
            let out = right + 36.0 + 14.0 * span.abs() as f64;
            p1 = (out, p0.1);
            p2 = (out, p3.1);
        }
        let d = format!(
            "M{},{} C{},{} {},{} {},{}",
            f1(p0.0),
            f1(p0.1),
            f1(p1.0),
            f1(p1.1),
            f1(p2.0),
            f1(p2.1),
            f1(p3.0),
            f1(p3.1)
        );
        g.push_str(&format!("<path d=\"{d}\" fill=\"none\" stroke=\"{MUTED}\" stroke-width=\"1.8\" marker-end=\"url(#arr)\"/>"));
        if !l.is_empty() {
            // label at the curve's midpoint
            let mx = (p0.0 + 3.0 * p1.0 + 3.0 * p2.0 + p3.0) / 8.0;
            let my = (p0.1 + 3.0 * p1.1 + 3.0 * p2.1 + p3.1) / 8.0;
            let twl = tw(l, 11.0).f() + 10.0;
            g.push_str(&format!(
                "<rect x=\"{}\" y=\"{}\" width=\"{}\" height=\"18\" rx=\"4\" fill=\"#fff\" stroke=\"{GRID}\"/><text x=\"{}\" y=\"{}\" font-size=\"11\" fill=\"{MUTED}\" text-anchor=\"middle\">{}</text>",
                f1(mx - twl / 2.0),
                f1(my - 10.0),
                f1(twl),
                f1(mx),
                f1(my + 3.0),
                e(l)
            ));
        }
    }
    for n in &ids {
        let (x, y) = pos[n.as_str()];
        let c = PALETTE[(layer[n.as_str()] as usize) % PALETTE.len()];
        let lines = wrap(&lab[n.as_str()], (bw - 18) as f64, 14.0);
        g.push_str(&format!(
            "<rect x=\"{}\" y=\"{}\" width=\"{bw}\" height=\"{bh}\" rx=\"10\" fill=\"#fff\" stroke=\"{c}\" stroke-width=\"2\"/><rect x=\"{}\" y=\"{}\" width=\"6\" height=\"{bh}\" rx=\"3\" fill=\"{c}\"/>",
            f1(x),
            f1(y),
            f1(x),
            f1(y)
        ));
        let y0 = y + bhf / 2.0 - ((lines.len() - 1) * 9) as f64 + 5.0;
        for (k, ln) in lines.iter().enumerate() {
            g.push_str(&format!(
                "<text x=\"{}\" y=\"{}\" font-size=\"14\" fill=\"{INK}\" text-anchor=\"middle\">{}</text>",
                f1(x + bwf / 2.0 + 3.0),
                f1(y0 + (k * 18) as f64),
                e(ln)
            ));
        }
    }
    Ok(finish(spec, &g, w, h))
}

/// SVG text for a chart spec (`{type, title, ...}`); the error message is one the agent can act on.
pub fn render(spec: &Value) -> Result<String> {
    dict(spec, "get")?;
    let raw_type = get(spec, "type");
    let kind = if truthy(raw_type) {
        py_str(raw_type).to_lowercase().trim().to_string()
    } else {
        String::new()
    };
    let kind = match kind.as_str() {
        "hbar" | "column" | "ranking" => "bar",
        "donut" => "pie",
        "flowchart" | "diagram" => "flow",
        "timeline" => "gantt",
        "area" | "trend" => "line",
        k => k,
    }
    .to_string();
    let mut owned;
    let spec = if kind == "bar"
        && matches!(py_str(raw_type).to_lowercase().as_str(), "hbar" | "ranking")
    {
        owned = spec.clone();
        owned["horizontal"] = json!(true);
        &owned
    } else {
        spec
    };
    match kind.as_str() {
        "line" => line(spec),
        "bar" => bar(spec),
        "pie" => pie(spec),
        "gantt" => gantt(spec),
        "flow" => flow(spec),
        _ => err(format!(
            "type 应为 {} 之一 (type must be one of {})",
            TYPES.join(", "),
            TYPES.join(", ")
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gantt_one_day_items() {
        let svg = render(&json!({"type": "gantt", "title": "t", "tasks": [{"name": "Deadline", "start": "2026-10-03", "end": "2026-10-03"},
                                                                       {"name": "Work", "start": "2026-10-01", "end": "2026-10-05"}]}))
        .unwrap();
        assert!(svg.contains("<svg") && svg.contains("Deadline"));
        assert!(svg.contains("2026-10-01 → 2026-10-05（共 4 天 4 days）"));
        assert!(svg.contains("10/3–10/4 · 1天"));
        assert!(svg.ends_with("</svg>"));
    }

    #[test]
    fn helpers_match_python() {
        assert_eq!(fmt(1234567.0, ""), "1.23M");
        assert_eq!(fmt(15000.0, "元"), "15,000元");
        assert_eq!(fmt(150.0, ""), "150");
        assert_eq!(fmt(1500.5, ""), "1,500.5");
        assert_eq!(fmt(0.5, "%"), "0.5%");
        assert_eq!(fmt(0.0, ""), "0");
        assert_eq!(
            nice_ticks(0.0, 100.0),
            vec![0.0, 20.0, 40.0, 60.0, 80.0, 100.0]
        );
        assert_eq!(nice_ticks(3.0, 3.0), vec![1.0, 2.0, 3.0, 4.0, 5.0]);
        assert_eq!(
            nice_ticks(0.07, 0.31),
            vec![0.05, 0.1, 0.15, 0.2, 0.25, 0.3]
        );
        assert_eq!(num(&json!("$1,200")).unwrap(), Some(1200.0));
        assert_eq!(num(&json!("HK$5")).unwrap(), Some(5.0));
        assert!(num(&json!("S$1,200")).is_err()); // like Python: the S prefix is not stripped
        assert_eq!(num(&json!("12%")).unwrap(), Some(12.0));
        assert_eq!(num(&json!("")).unwrap(), None);
        assert_eq!(
            num(&json!("abc")).unwrap_err().0,
            "不是数字 not a number: 'abc'"
        );
        assert_eq!(
            num(&json!(true)).unwrap_err().0,
            "不是数字 not a number: True"
        );
        assert_eq!(
            wrap("订单审核通过后发货", 60.0, 14.0),
            vec!["订单审核", "通过后发", "货"]
        );
        assert_eq!(
            wrap("hello wide world", 50.0, 14.0),
            vec!["hello", "wide", "world"]
        );
        assert_eq!(tw("", 13.0).s(), "0");
        assert_eq!(tw("ab", 10.0).s(), "11.6");
    }

    #[test]
    fn line_bar_pie_flow_render() {
        let l = render(&json!({"type": "line", "title": "T", "subtitle": "S", "source": "src", "labels": ["1", "2", "3"], "values": [1, 2, 1.5], "unit": "%", "y_label": "y"})).unwrap();
        assert!(l.starts_with("<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"960\" height=\"540\" viewBox=\"0 0 960 540\""));
        assert!(l.contains("最高 High 2% (2)") && l.contains("最低 Low 1% (1)"));
        assert!(l.contains("来源 Source: src"));
        assert!(l.contains("y=\"281.0\"") && l.contains("rotate(-90 18 281.0)"));
        let b = render(&json!({"type": "ranking", "labels": ["a", "b"], "series": [{"name": "s", "values": [3, -1]}], "sort": "desc"})).unwrap();
        assert!(b.contains("fill=\"#dc2626\"") && b.contains("fill=\"#16a34a\""));
        assert!(
            b.contains("<text x=\"45.54\" y=\"178.5\" font-size=\"13\""),
            "{b}"
        );
        let m = render(&json!({"type": "bar", "labels": ["a"], "series": [{"name": "x", "values": [1]}, {"name": "y", "values": [2]}]})).unwrap();
        assert!(
            m.contains("<rect x=\"78\" y=\"52\" width=\"14\" height=\"10\"")
                && m.contains("<rect x=\"125.53999999999999\" y=\"52\""),
            "{m}"
        );
        let p = render(
            &json!({"type": "pie", "labels": ["a", "b"], "values": [1, 3], "center_text": "4"}),
        )
        .unwrap();
        assert!(
            p.contains("A175,175 0 1 1 330.0,109.0 L330,284.0 Z")
                && p.contains("r=\"96.25000000000001\"")
                && p.contains("y=\"290.0\""),
            "{p}"
        );
        assert!(p.contains("75%") && p.contains("3  ·  75.0%"));
        let f =
            render(&json!({"type": "flowchart", "steps": ["开始", "审核", "结束"], "edges": []}))
                .unwrap();
        assert!(f.contains("marker-end=\"url(#arr)\"") && f.contains("审核"));
        let f2 = render(&json!({"type": "flow", "nodes": [{"id": "a", "label": "A"}, "b"], "edges": [{"from": "a", "to": "b", "label": "go"}, {"from": "b", "to": "a"}]})).unwrap();
        assert!(f2.contains(">go<"));
        assert_eq!(
            render(&json!({"type": "flow", "nodes": ["a"], "edges": [{"from": "a", "to": "zz"}]}))
                .unwrap_err()
                .0,
            "连线指向不存在的节点 edge refers to an unknown node: a → zz"
        );
        assert_eq!(render(&json!({"type": "nope"})).unwrap_err().0, "type 应为 line, bar, pie, gantt, flow 之一 (type must be one of line, bar, pie, gantt, flow)");
        assert_eq!(
            render(&json!({"type": "line", "labels": ["a", "b"], "values": [1]}))
                .unwrap_err()
                .0,
            "series「」有 1 个数值，但 labels 有 2 个 (values must match labels)"
        );
        assert_eq!(
            render(&json!({"type": "pie", "labels": ["a"], "values": [-1]}))
                .unwrap_err()
                .0,
            "饼图的数值必须是正数 (pie values must be positive)"
        );
        assert_eq!(
            render(&json!({"type": "gantt", "tasks": [{"name": "x", "start": "3 Oct"}]}))
                .unwrap_err()
                .0,
            "日期格式应为 YYYY-MM-DD (date must be YYYY-MM-DD): '3 Oct'"
        );
    }
}
