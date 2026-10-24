//! Fill PDF forms (AcroForm) locally — permission slips, applications, school forms (`app/runtime/pdfforms.py`,
//! pypdf → `lopdf`).
//!
//! The agent lists the fields, asks the user for what it doesn't know, fills a *copy*, and shows it to the user before
//! anything is sent. Nothing leaves the machine. Signature fields are never filled (the user signs).
//!
//! The filled copy carries `/NeedAppearances true` so viewers render the new values (pypdf wrote no appearance
//! streams either, with `auto_regenerate=False`).

use std::collections::{HashMap, HashSet};
use std::path::Path;

use lopdf::{Dictionary, Document, Object, ObjectId};
use omuse_common::pyjson;
use omuse_common::util::{py_str, truthy};
use serde_json::{json, Map, Value};

/// Checkbox values that mean "on".
pub const YES: [&str; 14] = [
    "yes", "y", "true", "1", "on", "x", "✓", "✔", "checked", "是", "对", "勾选", "同意", "✓",
];
/// Checkbox values that mean "off".
pub const NO: [&str; 10] = [
    "no",
    "n",
    "false",
    "0",
    "off",
    "",
    "unchecked",
    "否",
    "不",
    "不同意",
];

/// `FormError`: a PDF that cannot be filled, with the bilingual reason the agent shows.
#[derive(Debug, Clone, thiserror::Error)]
#[error("{0}")]
pub struct FormError(pub String);

/// Decode a PDF text string (UTF-16BE with BOM, else PDFDocEncoding ≈ Latin-1).
fn pdf_text(bytes: &[u8]) -> String {
    if bytes.len() >= 2 && bytes[0] == 0xFE && bytes[1] == 0xFF {
        let units: Vec<u16> = bytes[2..]
            .chunks(2)
            .map(|c| u16::from_be_bytes([c[0], *c.get(1).unwrap_or(&0)]))
            .collect();
        return String::from_utf16_lossy(&units);
    }
    if bytes.len() >= 3 && bytes[0] == 0xEF && bytes[1] == 0xBB && bytes[2] == 0xBF {
        return String::from_utf8_lossy(&bytes[3..]).into_owned();
    }
    match std::str::from_utf8(bytes) {
        Ok(s) => s.to_string(),
        Err(_) => bytes.iter().map(|b| *b as char).collect(),
    }
}

/// Encode a text string the way pypdf's `TextStringObject` is written: plain bytes when ASCII, else UTF-16BE + BOM.
fn pdf_string(s: &str) -> Object {
    if s.is_ascii() {
        Object::string_literal(s.as_bytes().to_vec())
    } else {
        let mut out = vec![0xFE, 0xFF];
        for u in s.encode_utf16() {
            out.extend_from_slice(&u.to_be_bytes());
        }
        Object::string_literal(out)
    }
}

/// `str(obj)` as pypdf prints a field attribute: names keep their slash, strings are decoded.
fn obj_str(doc: &Document, o: &Object) -> String {
    match o {
        Object::Null => String::new(),
        Object::Boolean(b) => if *b { "True" } else { "False" }.to_string(),
        Object::Integer(i) => i.to_string(),
        Object::Real(f) => pyjson::format_f64(*f as f64),
        Object::Name(n) => format!("/{}", String::from_utf8_lossy(n)),
        Object::String(s, _) => pdf_text(s),
        Object::Array(a) => {
            format!(
                "[{}]",
                a.iter()
                    .map(|x| py_repr_obj(doc, x))
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        }
        Object::Reference(id) => doc
            .get_object(*id)
            .map(|x| obj_str(doc, x))
            .unwrap_or_default(),
        Object::Dictionary(_) | Object::Stream(_) => String::new(),
    }
}

fn py_repr_obj(doc: &Document, o: &Object) -> String {
    match o {
        Object::String(_, _) | Object::Name(_) => pyjson::py_repr(&Value::String(obj_str(doc, o))),
        Object::Reference(id) => doc
            .get_object(*id)
            .map(|x| py_repr_obj(doc, x))
            .unwrap_or_default(),
        other => obj_str(doc, other),
    }
}

fn deref<'a>(doc: &'a Document, o: &'a Object) -> &'a Object {
    match o {
        Object::Reference(id) => doc.get_object(*id).map(|x| deref(doc, x)).unwrap_or(o),
        other => other,
    }
}

fn get<'a>(doc: &'a Document, d: &'a Dictionary, key: &[u8]) -> Option<&'a Object> {
    d.get(key)
        .ok()
        .map(|o| deref(doc, o))
        .filter(|o| !o.is_null())
}

fn dict_of<'a>(doc: &'a Document, o: &'a Object) -> Option<&'a Dictionary> {
    match deref(doc, o) {
        Object::Dictionary(d) => Some(d),
        Object::Stream(s) => Some(&s.dict),
        _ => None,
    }
}

fn int_of(o: Option<&Object>) -> i64 {
    match o {
        Some(Object::Integer(i)) => *i,
        Some(Object::Real(f)) => *f as i64,
        _ => 0,
    }
}

/// Inheritable attribute: the node's own, else the nearest ancestor's.
fn inherited<'a>(doc: &'a Document, node: &'a Dictionary, key: &[u8]) -> Option<&'a Object> {
    let mut cur = Some(node);
    let mut guard = 0;
    while let Some(d) = cur {
        if let Some(v) = get(doc, d, key) {
            return Some(v);
        }
        cur = d.get(b"Parent").ok().and_then(|p| dict_of(doc, p));
        guard += 1;
        if guard > 64 {
            break;
        }
    }
    None
}

/// Fully qualified name: the `/T` of the node and its ancestors, joined by ".".
fn qualified_name(doc: &Document, node: &Dictionary) -> String {
    let mut parts = Vec::new();
    let mut cur = Some(node);
    let mut guard = 0;
    while let Some(d) = cur {
        if let Some(t) = get(doc, d, b"T") {
            parts.push(obj_str(doc, t));
        }
        cur = d.get(b"Parent").ok().and_then(|p| dict_of(doc, p));
        guard += 1;
        if guard > 64 {
            break;
        }
    }
    parts.reverse();
    parts.join(".")
}

fn ap_states(doc: &Document, widget: &Dictionary) -> Vec<String> {
    let Some(ap) = get(doc, widget, b"AP").and_then(|o| dict_of(doc, o)) else {
        return vec![];
    };
    let Some(n) = get(doc, ap, b"N").and_then(|o| dict_of(doc, o)) else {
        return vec![];
    };
    n.iter()
        .map(|(k, _)| format!("/{}", String::from_utf8_lossy(k)))
        .collect()
}

/// A field node of the AcroForm tree as pypdf's `get_fields()` reports it.
struct FieldInfo {
    name: String,
    ft: String,
    ff: i64,
    value: Option<Object>,
    tu: Option<String>,
    opt: Vec<Object>,
    /// `/_States_` (checkbox / radio appearance states, choice options).
    states: Vec<String>,
}

fn collect_fields(
    doc: &Document,
    node: &Dictionary,
    out: &mut Vec<FieldInfo>,
    seen: &mut HashSet<ObjectId>,
    depth: usize,
) {
    if depth > 32 {
        return;
    }
    if get(doc, node, b"T").is_some() || get(doc, node, b"TM").is_some() {
        let ft = inherited(doc, node, b"FT")
            .map(|o| obj_str(doc, o))
            .unwrap_or_default();
        let ff = int_of(inherited(doc, node, b"Ff"));
        let mut states: Vec<String> = Vec::new();
        let opt: Vec<Object> = inherited(doc, node, b"Opt")
            .and_then(|o| match deref(doc, o) {
                Object::Array(a) => Some(a.clone()),
                _ => None,
            })
            .unwrap_or_default();
        if ft == "/Ch" {
            states = opt.iter().map(|o| obj_str(doc, o)).collect();
        } else if ft == "/Btn" && get(doc, node, b"AP").is_some() {
            states = ap_states(doc, node);
            if !states.iter().any(|s| s == "/Off") {
                states.push("/Off".into());
            }
        } else if ft == "/Btn" && (ff & (1 << 15)) != 0 && get(doc, node, b"Kids").is_some() {
            let mut all: HashSet<String> = HashSet::new();
            if let Some(Object::Array(kids)) = get(doc, node, b"Kids") {
                for k in kids {
                    if let Some(kd) = dict_of(doc, k) {
                        all.extend(ap_states(doc, kd));
                    }
                }
            }
            states = all.into_iter().collect();
            states.sort();
        }
        out.push(FieldInfo {
            name: qualified_name(doc, node),
            ft,
            ff,
            value: inherited(doc, node, b"V").cloned(),
            tu: get(doc, node, b"TU").map(|o| obj_str(doc, o)),
            opt,
            states,
        });
    }
    if let Some(Object::Array(kids)) = get(doc, node, b"Kids") {
        for k in kids {
            if let Object::Reference(id) = k {
                if !seen.insert(*id) {
                    continue;
                }
            }
            if let Some(kd) = dict_of(doc, k) {
                collect_fields(doc, kd, out, seen, depth + 1);
            }
        }
    }
}

fn all_fields(doc: &Document) -> Vec<FieldInfo> {
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    let Ok(catalog) = doc.catalog() else {
        return out;
    };
    let Some(acro) = get(doc, catalog, b"AcroForm").and_then(|o| dict_of(doc, o)) else {
        return out;
    };
    if let Some(Object::Array(fields)) = get(doc, acro, b"Fields") {
        for f in fields {
            if let Object::Reference(id) = f {
                if !seen.insert(*id) {
                    continue;
                }
            }
            if let Some(d) = dict_of(doc, f) {
                collect_fields(doc, d, &mut out, &mut seen, 0);
            }
        }
    }
    out
}

fn kind(f: &FieldInfo) -> &'static str {
    match f.ft.as_str() {
        "/Tx" => "text",
        "/Btn" => {
            if f.ff & (1 << 16) != 0 {
                "pushbutton"
            } else if f.ff & (1 << 15) != 0 {
                "radio"
            } else {
                "checkbox"
            }
        }
        "/Ch" => {
            if f.ff & (1 << 17) != 0 {
                "dropdown"
            } else {
                "list"
            }
        }
        "/Sig" => "signature",
        _ => "unknown",
    }
}

fn options(doc: &Document, f: &FieldInfo) -> Vec<String> {
    f.opt
        .iter()
        .map(|o| match deref(doc, o) {
            Object::Array(pair) if !pair.is_empty() => obj_str(doc, &pair[0]), // [export value, display text]
            other => obj_str(doc, other),
        })
        .collect()
}

/// Page number (1-based) of each field's first widget, by qualified name.
fn pages(doc: &Document) -> HashMap<String, u32> {
    let mut where_ = HashMap::new();
    for (i, pid) in doc.get_pages() {
        let Ok(annots) = doc.get_page_annotations(pid) else {
            continue;
        };
        for a in annots {
            let name = qualified_name(doc, a);
            if !name.is_empty() {
                where_.entry(name).or_insert(i);
            }
        }
    }
    where_
}

fn load(path: &Path) -> Result<Document, FormError> {
    let doc = Document::load(path)
        .map_err(|e| FormError(format!("不是有效的 PDF (not a readable PDF): {}", e)))?;
    if doc.is_encrypted() {
        return Err(FormError("PDF 有密码保护 (encrypted PDF)".into()));
    }
    Ok(doc)
}

fn fields_of(doc: &Document) -> Result<Vec<Value>, FormError> {
    let got = all_fields(doc);
    if got.is_empty() {
        return Err(FormError(
            "这个 PDF 没有可填写的表单字段（可能是扫描件或普通 PDF）。This PDF has no fillable form fields \
(it may be a scan); fill it by writing a new document instead."
                .into(),
        ));
    }
    let pages = pages(doc);
    let mut out = Vec::new();
    for f in &got {
        let k = kind(f);
        if k == "pushbutton" {
            continue;
        }
        let value = f
            .value
            .as_ref()
            .map(|v| obj_str(doc, v))
            .unwrap_or_default();
        let mut item = Map::new();
        item.insert("name".into(), json!(f.name));
        item.insert("type".into(), json!(k));
        item.insert("value".into(), json!(value));
        item.insert(
            "page".into(),
            pages.get(&f.name).map(|p| json!(p)).unwrap_or(Value::Null),
        );
        if let Some(tu) = &f.tu {
            if !tu.is_empty() {
                item.insert("label".into(), json!(tu));
            }
        }
        if k == "checkbox" || k == "radio" {
            let opts: Vec<String> = f
                .states
                .iter()
                .filter(|s| *s != "/Off")
                .map(|s| s.trim_start_matches('/').to_string())
                .collect();
            item.insert("options".into(), json!(opts));
            item.insert("value".into(), json!(value.trim_start_matches('/')));
        } else if k == "dropdown" || k == "list" {
            item.insert("options".into(), json!(options(doc, f)));
        }
        if f.ff & 2 != 0 {
            item.insert("required".into(), json!(true));
        }
        if f.ff & 1 != 0 {
            item.insert("read_only".into(), json!(true));
        }
        out.push(Value::Object(item));
    }
    Ok(out)
}

/// The fillable fields of a PDF: `{"name", "type", "value", "page", "label"?, "options"?, "required"?, "read_only"?}`.
pub fn fields(path: &Path) -> Result<Vec<Value>, FormError> {
    let doc = load(path)?;
    fields_of(&doc)
}

// ---------------------------------------------------------------- difflib.get_close_matches

fn longest_match(
    a: &[char],
    b2j: &HashMap<char, Vec<usize>>,
    alo: usize,
    ahi: usize,
    blo: usize,
    bhi: usize,
) -> (usize, usize, usize) {
    let (mut besti, mut bestj, mut bestsize) = (alo, blo, 0usize);
    let mut j2len: HashMap<usize, usize> = HashMap::new();
    for (i, ch) in a.iter().enumerate().take(ahi).skip(alo) {
        let mut newj2len: HashMap<usize, usize> = HashMap::new();
        if let Some(js) = b2j.get(ch) {
            for &j in js {
                if j < blo {
                    continue;
                }
                if j >= bhi {
                    break;
                }
                let k = j2len.get(&(j.wrapping_sub(1))).copied().unwrap_or(0) + 1;
                newj2len.insert(j, k);
                if k > bestsize {
                    besti = i + 1 - k;
                    bestj = j + 1 - k;
                    bestsize = k;
                }
            }
        }
        j2len = newj2len;
    }
    (besti, bestj, bestsize)
}

/// `difflib.SequenceMatcher(None, a, b).ratio()` (no junk heuristics: our names are short).
fn ratio(a: &[char], b: &[char]) -> f64 {
    if a.is_empty() && b.is_empty() {
        return 1.0;
    }
    let mut b2j: HashMap<char, Vec<usize>> = HashMap::new();
    for (j, ch) in b.iter().enumerate() {
        b2j.entry(*ch).or_default().push(j);
    }
    let mut matches = 0usize;
    let mut queue = vec![(0usize, a.len(), 0usize, b.len())];
    while let Some((alo, ahi, blo, bhi)) = queue.pop() {
        let (i, j, k) = longest_match(a, &b2j, alo, ahi, blo, bhi);
        if k > 0 {
            matches += k;
            if alo < i && blo < j {
                queue.push((alo, i, blo, j));
            }
            if i + k < ahi && j + k < bhi {
                queue.push((i + k, ahi, j + k, bhi));
            }
        }
    }
    2.0 * matches as f64 / (a.len() + b.len()) as f64
}

/// `difflib.get_close_matches(word, possibilities, n, cutoff)`.
pub fn close_matches(word: &str, possibilities: &[String], n: usize, cutoff: f64) -> Vec<String> {
    let w: Vec<char> = word.chars().collect();
    let mut scored: Vec<(f64, &String)> = possibilities
        .iter()
        .map(|x| (ratio(&x.chars().collect::<Vec<_>>(), &w), x))
        .filter(|(r, _)| *r >= cutoff)
        .collect();
    scored.sort_by(|a, b| {
        b.0.partial_cmp(&a.0)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| b.1.cmp(a.1))
    });
    scored.into_iter().take(n).map(|(_, x)| x.clone()).collect()
}

// ---------------------------------------------------------------- filling

fn py_list(items: &[String]) -> String {
    pyjson::py_repr(&Value::Array(
        items.iter().map(|s| Value::String(s.clone())).collect(),
    ))
}

/// Write a filled copy to `output`. Returns what was set, what was skipped and why:
/// `{"output", "filled", "problems", "required_still_empty", "untouched"}`.
pub fn fill(path: &Path, values: &Value, output: &Path) -> Result<Value, FormError> {
    let listed = fields(path)?;
    let info: Map<String, Value> = listed
        .iter()
        .map(|f| (py_str(&f["name"]), f.clone()))
        .collect();
    let names: Vec<String> = info.keys().cloned().collect();
    let mut doc = load(path)?;
    let mut ok: Map<String, Value> = Map::new();
    let mut problems: Vec<String> = Vec::new();
    let mut text_vals: HashMap<String, String> = HashMap::new();
    let mut btn_vals: HashMap<String, String> = HashMap::new();
    let empty = Map::new();
    for (name, val) in values.as_object().unwrap_or(&empty) {
        let Some(f) = info.get(name) else {
            let near = close_matches(name, &names, 3, 0.5);
            problems.push(format!(
                "unknown field '{}'{}",
                name,
                if near.is_empty() {
                    String::new()
                } else {
                    format!(" (did you mean {}?)", near.join(", "))
                }
            ));
            continue;
        };
        let ftype = py_str(&f["type"]);
        if ftype == "signature" {
            problems.push(format!(
                "'{}' is a signature field — the user must sign it themselves",
                name
            ));
            continue;
        }
        if truthy(f.get("read_only").unwrap_or(&Value::Null)) {
            problems.push(format!("'{}' is read-only", name));
            continue;
        }
        let v = if val.is_null() {
            String::new()
        } else {
            py_str(val).trim().to_string()
        };
        let opts: Vec<String> = f
            .get("options")
            .and_then(Value::as_array)
            .map(|a| a.iter().map(py_str).collect())
            .unwrap_or_default();
        match ftype.as_str() {
            "checkbox" => {
                let on = opts.first().cloned().unwrap_or_else(|| "Yes".to_string());
                let low = v.to_lowercase();
                let state = if let Value::Bool(b) = val {
                    if *b {
                        on.clone()
                    } else {
                        "Off".to_string()
                    }
                } else if YES.contains(&low.as_str()) || v.trim_start_matches('/') == on {
                    on.clone()
                } else if NO.contains(&low.as_str()) || v.trim_start_matches('/') == "Off" {
                    "Off".to_string()
                } else {
                    problems.push(format!("'{}' is a checkbox: use true/false", name));
                    continue;
                };
                ok.insert(name.clone(), json!(state != "Off"));
                btn_vals.insert(name.clone(), state);
            }
            "radio" => {
                let want = v.trim_start_matches('/').to_lowercase();
                match opts.iter().find(|o| o.to_lowercase() == want) {
                    Some(m) => {
                        btn_vals.insert(name.clone(), m.clone());
                        ok.insert(name.clone(), json!(m));
                    }
                    None => problems.push(format!("'{}' must be one of {}", name, py_list(&opts))),
                }
            }
            "dropdown" | "list" => {
                let mut v = v;
                if !opts.is_empty() && !opts.contains(&v) {
                    match opts.iter().find(|o| o.to_lowercase() == v.to_lowercase()) {
                        Some(m) => v = m.clone(),
                        None => {
                            problems.push(format!("'{}' must be one of {}", name, py_list(&opts)));
                            continue;
                        }
                    }
                }
                ok.insert(name.clone(), json!(v));
                text_vals.insert(name.clone(), v);
            }
            _ => {
                ok.insert(name.clone(), json!(v));
                text_vals.insert(name.clone(), v);
            }
        }
    }
    if ok.is_empty() {
        return Err(FormError(format!(
            "没有填写任何字段 (nothing was filled): {}",
            problems.join("; ")
        )));
    }

    // Plan the edits on a shared borrow: (widget id, field node id to carry /V, states of the widget).
    struct Edit {
        widget: ObjectId,
        target: ObjectId,
        text: Option<String>,
        button: Option<(String, Vec<String>)>, // state, the widget's appearance states
        is_choice: bool,
    }
    let mut edits: Vec<Edit> = Vec::new();
    {
        let d: &Document = &doc;
        let page_ids: Vec<ObjectId> = d.get_pages().into_values().collect();
        for pid in page_ids {
            let Ok(page) = d.get_dictionary(pid) else {
                continue;
            };
            let annots: Vec<Object> = match page.get(b"Annots") {
                Ok(Object::Reference(id)) => d
                    .get_object(*id)
                    .and_then(Object::as_array)
                    .cloned()
                    .unwrap_or_default(),
                Ok(Object::Array(a)) => a.clone(),
                _ => vec![],
            };
            for a in annots {
                let Object::Reference(wid) = a else { continue }; // inline annotation dicts are not form widgets in practice
                let Ok(w) = d.get_dictionary(wid) else {
                    continue;
                };
                let parent_id = match w.get(b"Parent") {
                    Ok(Object::Reference(p)) => Some(*p),
                    _ => None,
                };
                let parent = parent_id.and_then(|p| d.get_dictionary(p).ok());
                let has_t = get(d, w, b"T").is_some();
                // text / choice values (pypdf update_page_form_field_values): the field node is the widget when it is
                // merged with its field, else its parent
                if !text_vals.is_empty() {
                    let (node, node_id) = if get(d, w, b"FT").is_some() && has_t {
                        (Some(w), wid)
                    } else {
                        (parent, parent_id.unwrap_or(wid))
                    };
                    if let Some(node) = node {
                        let qn = qualified_name(d, node);
                        let t = get(d, node, b"T").map(|o| obj_str(d, o));
                        let hit = text_vals
                            .iter()
                            .find(|(k, _)| **k == qn || t.as_deref() == Some(k.as_str()));
                        if let Some((_, val)) = hit {
                            let is_choice =
                                inherited(d, node, b"FT").map(|o| obj_str(d, o)).as_deref()
                                    == Some("/Ch");
                            edits.push(Edit {
                                widget: wid,
                                target: node_id,
                                text: Some(val.clone()),
                                button: None,
                                is_choice,
                            });
                        }
                    }
                }
                // buttons
                let fname = match get(d, w, b"T") {
                    Some(t) => obj_str(d, t),
                    None => parent
                        .and_then(|p| get(d, p, b"T"))
                        .map(|t| obj_str(d, t))
                        .unwrap_or_default(),
                };
                if let Some(state) = btn_vals.get(&fname) {
                    let target = match parent_id {
                        Some(p) if !has_t => p,
                        _ => wid,
                    };
                    edits.push(Edit {
                        widget: wid,
                        target,
                        text: None,
                        button: Some((state.clone(), ap_states(d, w))),
                        is_choice: false,
                    });
                }
            }
        }
    }
    for e in edits {
        if let Some(val) = &e.text {
            if let Ok(node) = doc.get_dictionary_mut(e.target) {
                if e.is_choice && node.has(b"I") {
                    node.remove(b"I");
                }
                node.set("V", pdf_string(val));
            }
        }
        if let Some((state, aps)) = &e.button {
            if let Ok(node) = doc.get_dictionary_mut(e.target) {
                node.set("V", Object::Name(state.as_bytes().to_vec()));
            }
            if let Ok(w) = doc.get_dictionary_mut(e.widget) {
                let as_ = if aps.iter().any(|a| a == &format!("/{}", state)) {
                    state.as_str()
                } else {
                    "Off"
                };
                w.set("AS", Object::Name(as_.as_bytes().to_vec()));
            }
        }
    }
    // NeedAppearances: viewers draw the values themselves
    let acro_ref = doc.catalog().ok().and_then(|c| match c.get(b"AcroForm") {
        Ok(Object::Reference(id)) => Some(*id),
        _ => None,
    });
    match acro_ref {
        Some(id) => {
            if let Ok(a) = doc.get_dictionary_mut(id) {
                a.set("NeedAppearances", Object::Boolean(true));
            }
        }
        None => {
            if let Ok(c) = doc.catalog_mut() {
                if let Ok(Object::Dictionary(a)) = c.get_mut(b"AcroForm") {
                    a.set("NeedAppearances", Object::Boolean(true));
                }
            }
        }
    }
    if let Some(dir) = output.parent() {
        if !dir.as_os_str().is_empty() {
            std::fs::create_dir_all(dir)
                .map_err(|e| FormError(format!("cannot create {}: {}", dir.display(), e)))?;
        }
    }
    doc.save(output)
        .map_err(|e| FormError(format!("cannot write {}: {}", output.display(), e)))?;

    // read back: what the filled copy actually contains
    let after: HashMap<String, String> = fields(output)?
        .iter()
        .map(|f| (py_str(&f["name"]), py_str(&f["value"])))
        .collect();
    let after_val = |n: &str| after.get(n).cloned().unwrap_or_default();
    let mismatched: Vec<String> = ok
        .iter()
        .filter(|(n, v)| {
            v.is_string() && after_val(n).trim_start_matches('/') != v.as_str().unwrap_or("")
        })
        .map(|(n, _)| n.clone())
        .collect();
    if !mismatched.is_empty() {
        problems.push(format!("could not verify: {}", mismatched.join(", ")));
    }
    let empty_required: Vec<&String> = info
        .iter()
        .filter(|(n, f)| {
            truthy(f.get("required").unwrap_or(&Value::Null))
                && after_val(n).is_empty()
                && !ok.contains_key(*n)
        })
        .map(|(n, _)| n)
        .collect();
    let untouched: Vec<&String> = info
        .keys()
        .filter(|n| !ok.contains_key(*n) && after_val(n).is_empty())
        .take(40)
        .collect();
    Ok(
        json!({"output": output.to_string_lossy(), "filled": ok, "problems": problems,
              "required_still_empty": empty_required, "untouched": untouched}),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn fixture() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/pages/permission_slip.pdf")
    }

    fn by_name(list: &[Value]) -> Map<String, Value> {
        list.iter()
            .map(|f| (py_str(&f["name"]), f.clone()))
            .collect()
    }

    #[test]
    fn lists_fields_like_pypdf() {
        let names = by_name(&fields(&fixture()).unwrap());
        assert_eq!(names["lunch"]["type"], "radio");
        assert_eq!(names["consent"]["type"], "checkbox");
        assert_eq!(names["tshirt"]["options"], json!(["S", "M", "L", "XL"]));
        assert_eq!(names["lunch"]["options"], json!(["regular", "vegetarian"]));
        assert_eq!(
            names["consent"],
            json!({"name": "consent", "type": "checkbox", "value": "Off", "page": 1,
                                             "label": "Permission granted", "options": ["Yes"], "required": true})
        );
        assert_eq!(
            names["student_name"],
            json!({"name": "student_name", "type": "text", "value": "", "page": 1, "label": "Student name"})
        );
        assert_eq!(names["tshirt"]["type"], "dropdown");
        assert_eq!(names["tshirt"]["value"], "M");
        assert_eq!(names["lunch"]["value"], "");
        assert!(names["lunch"]["required"] == true);
        assert_eq!(names.len(), 7);
        // the text in the JSON the agent sees (e2e: '"lunch"', '"radio"', 'vegetarian')
        let txt = pyjson::dumps(&Value::Array(fields(&fixture()).unwrap()));
        assert!(
            txt.contains("\"lunch\"") && txt.contains("\"radio\"") && txt.contains("vegetarian")
        );
    }

    #[test]
    fn fills_a_copy_and_reports_problems() {
        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("f.pdf");
        let r = fill(
            &fixture(),
            &json!({"student_name": "Able", "consent": "yes", "lunch": "Regular", "tshirt": "XXL", "nope": 1}),
            &out,
        )
        .unwrap();
        let got: HashMap<String, String> = fields(&out)
            .unwrap()
            .iter()
            .map(|f| (py_str(&f["name"]), py_str(&f["value"])))
            .collect();
        assert_eq!(got["student_name"], "Able");
        assert_eq!(got["consent"], "Yes");
        assert_eq!(got["lunch"], "regular");
        let problems: Vec<String> = r["problems"]
            .as_array()
            .unwrap()
            .iter()
            .map(py_str)
            .collect();
        assert!(
            problems.iter().any(|p| p.contains("tshirt"))
                && problems.iter().any(|p| p.contains("nope"))
        );
        assert!(problems
            .iter()
            .any(|p| p == "'tshirt' must be one of ['S', 'M', 'L', 'XL']"));
        assert!(problems
            .iter()
            .any(|p| p.starts_with("unknown field 'nope'")));
        assert!(!problems.iter().any(|p| p.starts_with("could not verify")));
        assert_eq!(
            r["filled"],
            json!({"student_name": "Able", "consent": true, "lunch": "regular"})
        );
        assert_eq!(r["output"], out.to_string_lossy().as_ref());
        let e = fill(&fixture(), &json!({"nope": 1}), &out).unwrap_err();
        assert!(e
            .0
            .starts_with("没有填写任何字段 (nothing was filled): unknown field 'nope'"));
    }

    #[test]
    fn e2e_0212_expectations() {
        // tests/e2e_0212.py: Able Lu, consent yes, vegetarian, L; "signature" is unknown; emergency_phone is pointed out
        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("forms").join("permission_slip-filled.pdf");
        let r = fill(
            &fixture(),
            &json!({"student_name": "Able Lu", "class": "P4", "parent_name": "Lucas 吕", "consent": true, "lunch": "vegetarian",
                    "tshirt": "L", "signature": "Lucas"}),
            &out,
        )
        .unwrap();
        let preview = pyjson::dumps(&r);
        assert!(
            preview.contains("permission_slip-filled.pdf")
                && preview.contains("unknown field 'signature'")
        );
        assert!(preview.contains("emergency_phone"));
        assert_eq!(r["untouched"], json!(["emergency_phone"]));
        assert_eq!(r["required_still_empty"], json!([]));
        let vals: HashMap<String, String> = fields(&out)
            .unwrap()
            .iter()
            .map(|f| (py_str(&f["name"]), py_str(&f["value"])))
            .collect();
        assert_eq!(vals["student_name"], "Able Lu");
        assert_eq!(vals["parent_name"], "Lucas 吕"); // non-ASCII round-trips (UTF-16BE)
        assert_eq!(vals["consent"], "Yes");
        assert_eq!(vals["lunch"], "vegetarian");
        assert_eq!(vals["tshirt"], "L");
        let orig: HashMap<String, String> = fields(&fixture())
            .unwrap()
            .iter()
            .map(|f| (py_str(&f["name"]), py_str(&f["value"])))
            .collect();
        assert_eq!(orig["student_name"], "");
        assert_eq!(orig["consent"], "Off");
        // NeedAppearances is set on the copy
        let doc = Document::load(&out).unwrap();
        let acro = get(&doc, doc.catalog().unwrap(), b"AcroForm")
            .and_then(|o| dict_of(&doc, o))
            .unwrap();
        assert!(acro.get(b"NeedAppearances").unwrap().as_bool().unwrap());
        // unchecking and bad checkbox values
        let r = fill(
            &fixture(),
            &json!({"consent": "maybe", "lunch": "/Vegetarian", "student_name": null}),
            &dir.path().join("g.pdf"),
        )
        .unwrap();
        assert_eq!(
            r["filled"],
            json!({"lunch": "vegetarian", "student_name": ""})
        );
        assert_eq!(
            r["problems"],
            json!(["'consent' is a checkbox: use true/false"])
        );
    }

    #[test]
    fn close_matches_like_difflib() {
        let names: Vec<String> = [
            "student_name",
            "class",
            "parent_name",
            "emergency_phone",
            "consent",
            "lunch",
            "tshirt",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        assert_eq!(
            close_matches("student", &names, 3, 0.5),
            vec!["student_name".to_string(), "consent".to_string()]
        );
        assert_eq!(
            close_matches("parent", &names, 3, 0.5),
            vec!["parent_name".to_string()]
        );
        assert!(close_matches("nope", &names, 3, 0.5).is_empty());
        assert_eq!(
            close_matches("signature", &names, 3, 0.5),
            Vec::<String>::new()
        );
        assert!(
            (ratio(
                &"abcd".chars().collect::<Vec<_>>(),
                &"bcde".chars().collect::<Vec<_>>()
            ) - 0.75)
                .abs()
                < 1e-9
        );
    }

    #[test]
    fn unreadable_and_formless_pdfs() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("x.pdf");
        std::fs::write(&p, b"not a pdf").unwrap();
        assert!(fields(&p)
            .unwrap_err()
            .0
            .starts_with("不是有效的 PDF (not a readable PDF)"));
        let mut doc = Document::with_version("1.5");
        let pages_id = doc.new_object_id();
        let page_id = doc.add_object(lopdf::dictionary! {"Type" => "Page", "Parent" => pages_id});
        doc.objects.insert(pages_id, Object::Dictionary(lopdf::dictionary! {"Type" => "Pages", "Kids" => vec![page_id.into()], "Count" => 1}));
        let catalog = doc.add_object(lopdf::dictionary! {"Type" => "Catalog", "Pages" => pages_id});
        doc.trailer.set("Root", catalog);
        let q = dir.path().join("plain.pdf");
        doc.save(&q).unwrap();
        assert!(fields(&q)
            .unwrap_err()
            .0
            .contains("no fillable form fields"));
    }
}
