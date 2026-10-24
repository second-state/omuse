//! Excel (.xlsx) files made and read locally — nothing leaves the box (`app/common/xlsx.py`).
//!
//! `make()`: sheets of rows → a tidy workbook (bold header, frozen top row, filter, sensible column widths,
//! numbers stored as numbers). Formulas are only kept when the caller asks for them, and never functions that reach
//! the network or other programs (text copied from web pages must not become a live formula).
//! `read_text()`: a workbook → plain text (one Markdown-ish table per sheet) so the agent can read .xlsx files.

use std::collections::HashSet;
use std::io::Write;
use std::sync::LazyLock;

use calamine::{Data, Reader};
use regex::Regex;
use serde_json::{json, Value};
use unicode_width::UnicodeWidthChar;

use crate::pycsv;
use crate::pyfmt::{self, py_str, take_chars};

/// Most rows a sheet may have.
pub const MAX_ROWS: usize = 20000;
/// Most columns a sheet may have.
pub const MAX_COLS: usize = 200;

static NUM: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[+-]?(\d{1,3}(,\d{3})+|\d+)(\.\d+)?$").unwrap());
static PCT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[+-]?\d+(\.\d+)?%$").unwrap());
static BAD_FORMULA: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)(WEBSERVICE|HYPERLINK|IMPORT\w*|FILTERXML|CALL|REGISTER|EXEC|DDE|RTD|INDIRECT)\s*\(|\||!\w+\(").unwrap()
});
static SHEET_BAD: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[\[\]:*?/\\]").unwrap());
static MD_SEP: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^:?-{2,}:?$").unwrap());
static MD_BOLD: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\*\*(.+?)\*\*").unwrap());

/// `XlsxError` of the Python module.
#[derive(Debug, Clone, thiserror::Error)]
#[error("{0}")]
pub struct XlsxError(pub String);

/// Display width of the longest line of `str(v)` (CJK wide glyphs count 2).
fn width(v: &Value) -> usize {
    let s = if v.is_null() {
        String::new()
    } else {
        py_str(v)
    };
    let line = s
        .split('\n')
        .max_by_key(|l| l.chars().count())
        .unwrap_or("");
    // Python's max() keeps the first longest line; max_by_key keeps the last — pick the first explicitly
    let line = s
        .split('\n')
        .find(|l| l.chars().count() == line.chars().count())
        .unwrap_or("");
    line.chars()
        .map(|ch| if ch.width() == Some(2) { 2 } else { 1 })
        .sum()
}

/// What goes into a cell: the value and an optional format (`"0.0%"`, or `"text"` for text that must never be
/// evaluated).
#[derive(Debug, Clone, PartialEq)]
pub enum CellFormat {
    None,
    Percent,
    Text,
}

/// Numbers stay numbers; text that looks like a formula stays text unless formulas are allowed and safe.
pub fn cell_value(v: &Value, formulas: bool) -> (Value, CellFormat) {
    match v {
        Value::Null | Value::Bool(_) | Value::Number(_) => return (v.clone(), CellFormat::None),
        _ => {}
    }
    let s = py_str(v);
    let t = s.trim();
    if NUM.is_match(t) {
        let n: f64 = t.replace(',', "").parse().unwrap_or(0.0);
        if n.fract() == 0.0 && !t.contains('.') && n.abs() < 9.2e18 {
            return (json!(n as i64), CellFormat::None);
        }
        return (pyfmt::float_value(n), CellFormat::None);
    }
    if PCT.is_match(t) {
        let n: f64 = t[..t.len() - 1].parse().unwrap_or(0.0);
        return (pyfmt::float_value(n / 100.0), CellFormat::Percent);
    }
    if matches!(
        t.chars().next(),
        Some('=') | Some('+') | Some('-') | Some('@')
    ) {
        if formulas && t.starts_with('=') && !BAD_FORMULA.is_match(t) {
            return (Value::String(t.to_string()), CellFormat::None);
        }
        return (Value::String(s), CellFormat::Text); // stored as text, never evaluated
    }
    (Value::String(s), CellFormat::None)
}

/// `csv.reader` over the text (Python semantics: blank lines give empty rows).
pub fn rows_from_csv(text: &str) -> Vec<Vec<String>> {
    pycsv::read_all(text, &pycsv::Dialect::excel())
}

/// The first Markdown table in the text (`**bold**` stripped from cells).
pub fn rows_from_markdown(text: &str) -> Vec<Vec<String>> {
    let mut rows: Vec<Vec<String>> = Vec::new();
    for line in pyfmt::splitlines(text) {
        let line = line.trim();
        if !(line.starts_with('|') && line.ends_with('|')) {
            if !rows.is_empty() {
                break;
            }
            continue;
        }
        let cells: Vec<&str> = line
            .trim_matches('|')
            .split('|')
            .map(|c| c.trim())
            .collect();
        if cells
            .iter()
            .filter(|c| !c.is_empty())
            .all(|c| MD_SEP.is_match(c))
        {
            continue;
        }
        rows.push(
            cells
                .iter()
                .map(|c| MD_BOLD.replace_all(c, "$1").into_owned())
                .collect(),
        );
    }
    rows
}

/// Excel column letters for a 1-based index.
fn col_letter(mut j: usize) -> String {
    let mut s = Vec::new();
    while j > 0 {
        let r = (j - 1) % 26;
        s.push((b'A' + r as u8) as char);
        j = (j - 1) / 26;
    }
    s.iter().rev().collect()
}

fn xml_text(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            c if (c as u32) < 0x20 && c != '\t' && c != '\n' && c != '\r' => {}
            c => out.push(c),
        }
    }
    out
}

/// Python `str(v)` for the sheet XML (numbers as text).
fn num_text(v: &Value) -> String {
    py_str(v)
}

struct Sheet {
    name: String,
    xml: String,
    filter: Option<String>,
}

const XML_HEAD: &str = "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n";

/// Write a workbook. `sheets` are `{name, columns, rows}` objects (rows may be lists or dicts);
/// returns `{"sheets": [names], "rows": total}`.
pub fn make(path: &str, sheets: &[Value], formulas: bool) -> Result<Value, XlsxError> {
    if sheets.is_empty() {
        return Err(XlsxError("没有数据 no sheets/rows given".into()));
    }
    let mut used: HashSet<String> = HashSet::new();
    let mut total = 0usize;
    let mut out_sheets: Vec<Sheet> = Vec::new();
    for (i, sh) in sheets.iter().enumerate() {
        let default = format!("Sheet{}", i + 1);
        let raw = match sh.get("name") {
            Some(v) if omuse_common::util::truthy(v) => py_str(v),
            _ => default.clone(),
        };
        let mut name = take_chars(&SHEET_BAD.replace_all(&raw, "_"), 31).to_string();
        if name.is_empty() {
            name = default.clone();
        }
        let mut guard = 0;
        while used.contains(&name) {
            name = take_chars(&format!("{}_{}", take_chars(&name, 28), i + 1), 31).to_string();
            guard += 1;
            if guard > 1000 {
                // the Python loop would spin forever here; make the name unique instead
                name = take_chars(&format!("{}_{}", take_chars(&name, 24), used.len() + 1), 31)
                    .to_string();
            }
        }
        used.insert(name.clone());
        let mut columns: Vec<Value> = match sh.get("columns") {
            Some(v) if omuse_common::util::truthy(v) => pyfmt::py_iter(v).map_err(XlsxError)?,
            _ => Vec::new(),
        };
        let raw_rows: Vec<Value> = match sh.get("rows") {
            Some(v) if omuse_common::util::truthy(v) => pyfmt::py_iter(v).map_err(XlsxError)?,
            _ => Vec::new(),
        };
        let mut rows: Vec<Vec<Value>> = raw_rows
            .iter()
            .map(|r| {
                if let Value::Array(a) = r {
                    a.clone()
                } else {
                    vec![r.clone()]
                }
            })
            .collect();
        if !rows.is_empty() && raw_rows[0].is_object() {
            if columns.is_empty() {
                if let Value::Object(o) = &raw_rows[0] {
                    columns = o.keys().map(|k| Value::String(k.clone())).collect();
                }
            }
            rows = raw_rows
                .iter()
                .map(|d| match d {
                    Value::Object(o) => Ok(columns
                        .iter()
                        .map(|c| match c {
                            Value::String(k) => o.get(k).cloned().unwrap_or(Value::Null),
                            _ => Value::Null,
                        })
                        .collect()),
                    other => Err(XlsxError(format!(
                        "'{}' object has no attribute 'get'",
                        pyfmt::py_type(other)
                    ))),
                })
                .collect::<Result<_, _>>()?;
        }
        if rows.len() > MAX_ROWS {
            return Err(XlsxError(format!("行数太多 too many rows (> {MAX_ROWS})")));
        }
        let mut widths: Vec<(usize, usize)> = Vec::new(); // (col, width) in first-seen order
        let mut bump = |j: usize, w: usize| match widths.iter_mut().find(|e| e.0 == j) {
            Some(e) => e.1 = e.1.max(w),
            None => widths.push((j, w)),
        };
        let mut data = String::new();
        let mut r0 = 1usize;
        let ncols_hdr = columns.len().min(MAX_COLS);
        let mut max_col = 0usize;
        if !columns.is_empty() {
            data.push_str("<row r=\"1\">");
            for (j, c) in columns.iter().take(MAX_COLS).enumerate() {
                let j = j + 1;
                data.push_str(&format!(
                    "<c r=\"{}1\" s=\"1\" t=\"inlineStr\"><is><t xml:space=\"preserve\">{}</t></is></c>",
                    col_letter(j),
                    xml_text(&py_str(c))
                ));
                bump(j, width(c));
                max_col = max_col.max(j);
            }
            data.push_str("</row>");
            r0 = 2;
        }
        for (ri, row) in rows.iter().enumerate() {
            let ri = ri + r0;
            data.push_str(&format!("<row r=\"{ri}\">"));
            for (j, v) in row.iter().take(MAX_COLS).enumerate() {
                let j = j + 1;
                max_col = max_col.max(j);
                let (val, fmt) = cell_value(v, formulas);
                let r = format!("{}{}", col_letter(j), ri);
                let wrap =
                    matches!(&val, Value::String(s) if s.contains('\n') || s.chars().count() > 60);
                let style = match (&fmt, wrap) {
                    (CellFormat::Percent, _) => " s=\"2\"",
                    (_, true) => " s=\"3\"",
                    _ => "",
                };
                match (&val, &fmt) {
                    (Value::Null, _) => {}
                    (Value::Bool(b), _) => data.push_str(&format!("<c r=\"{r}\" t=\"b\"><v>{}</v></c>", *b as u8)),
                    (Value::Number(_), _) => data.push_str(&format!("<c r=\"{r}\"{style}><v>{}</v></c>", num_text(&val))),
                    (Value::String(s), CellFormat::None) if s.starts_with('=') => {
                        data.push_str(&format!("<c r=\"{r}\"><f>{}</f></c>", xml_text(&s[1..])));
                    }
                    (Value::String(s), _) => data.push_str(&format!(
                        "<c r=\"{r}\"{style} t=\"inlineStr\"><is><t xml:space=\"preserve\">{}</t></is></c>",
                        xml_text(s)
                    )),
                    (other, _) => data.push_str(&format!(
                        "<c r=\"{r}\"{style} t=\"inlineStr\"><is><t xml:space=\"preserve\">{}</t></is></c>",
                        xml_text(&py_str(other))
                    )),
                }
                bump(j, width(&val).min(60));
            }
            data.push_str("</row>");
        }
        let mut cols_xml = String::new();
        for (j, w) in &widths {
            let w = (*w + 2).clamp(8, 62);
            cols_xml.push_str(&format!(
                "<col min=\"{j}\" max=\"{j}\" width=\"{w}\" customWidth=\"1\"/>"
            ));
        }
        let last_row = (r0 - 1 + rows.len()).max(1);
        let dim = format!("A1:{}{}", col_letter(max_col.max(1)), last_row);
        let mut xml = format!("{XML_HEAD}<worksheet xmlns=\"http://schemas.openxmlformats.org/spreadsheetml/2006/main\" xmlns:r=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships\"><dimension ref=\"{dim}\"/>");
        let filter = if !columns.is_empty() && !rows.is_empty() {
            Some(format!("A1:{}{}", col_letter(ncols_hdr), rows.len() + 1))
        } else {
            None
        };
        if !columns.is_empty() {
            xml.push_str("<sheetViews><sheetView workbookViewId=\"0\"><pane ySplit=\"1\" topLeftCell=\"A2\" activePane=\"bottomLeft\" state=\"frozen\"/><selection pane=\"bottomLeft\" activeCell=\"A2\" sqref=\"A2\"/></sheetView></sheetViews>");
        } else {
            xml.push_str("<sheetViews><sheetView workbookViewId=\"0\"/></sheetViews>");
        }
        xml.push_str("<sheetFormatPr defaultRowHeight=\"15\"/>");
        if !cols_xml.is_empty() {
            xml.push_str(&format!("<cols>{cols_xml}</cols>"));
        }
        if data.is_empty() {
            xml.push_str("<sheetData/>");
        } else {
            xml.push_str(&format!("<sheetData>{data}</sheetData>"));
        }
        if let Some(f) = &filter {
            xml.push_str(&format!("<autoFilter ref=\"{f}\"/>"));
        }
        xml.push_str("<pageMargins left=\"0.75\" right=\"0.75\" top=\"1\" bottom=\"1\" header=\"0.5\" footer=\"0.5\"/></worksheet>");
        total += rows.len();
        out_sheets.push(Sheet { name, xml, filter });
    }
    write_package(path, &out_sheets).map_err(|e| XlsxError(e.to_string()))?;
    Ok(
        json!({"sheets": out_sheets.iter().map(|s| s.name.clone()).collect::<Vec<_>>(), "rows": total}),
    )
}

fn write_package(path: &str, sheets: &[Sheet]) -> std::io::Result<()> {
    let file = std::fs::File::create(path)?;
    let mut z = zip::ZipWriter::new(file);
    let opts = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);
    let mut put = |name: &str, body: &str| -> std::io::Result<()> {
        z.start_file(name, opts).map_err(std::io::Error::other)?;
        z.write_all(body.as_bytes())
    };
    let mut ctypes = format!("{XML_HEAD}<Types xmlns=\"http://schemas.openxmlformats.org/package/2006/content-types\"><Default Extension=\"rels\" ContentType=\"application/vnd.openxmlformats-package.relationships+xml\"/><Default Extension=\"xml\" ContentType=\"application/xml\"/><Override PartName=\"/xl/workbook.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml\"/><Override PartName=\"/xl/styles.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.spreadsheetml.styles+xml\"/><Override PartName=\"/docProps/core.xml\" ContentType=\"application/vnd.openxmlformats-package.core-properties+xml\"/><Override PartName=\"/docProps/app.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.extended-properties+xml\"/>");
    for k in 1..=sheets.len() {
        ctypes.push_str(&format!("<Override PartName=\"/xl/worksheets/sheet{k}.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml\"/>"));
    }
    ctypes.push_str("</Types>");
    put("[Content_Types].xml", &ctypes)?;
    put("_rels/.rels", &format!("{XML_HEAD}<Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\"><Relationship Id=\"rId1\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument\" Target=\"xl/workbook.xml\"/><Relationship Id=\"rId2\" Type=\"http://schemas.openxmlformats.org/package/2006/relationships/metadata/core-properties\" Target=\"docProps/core.xml\"/><Relationship Id=\"rId3\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/extended-properties\" Target=\"docProps/app.xml\"/></Relationships>"))?;
    put("docProps/app.xml", &format!("{XML_HEAD}<Properties xmlns=\"http://schemas.openxmlformats.org/officeDocument/2006/extended-properties\" xmlns:vt=\"http://schemas.openxmlformats.org/officeDocument/2006/docPropsVTypes\"><Application>OMuse</Application></Properties>"))?;
    put("docProps/core.xml", &format!("{XML_HEAD}<cp:coreProperties xmlns:cp=\"http://schemas.openxmlformats.org/package/2006/metadata/core-properties\" xmlns:dc=\"http://purl.org/dc/elements/1.1/\" xmlns:dcterms=\"http://purl.org/dc/terms/\" xmlns:dcmitype=\"http://purl.org/dc/dcmitype/\" xmlns:xsi=\"http://www.w3.org/2001/XMLSchema-instance\"><dc:creator>OMuse</dc:creator></cp:coreProperties>"))?;
    let mut wb = format!("{XML_HEAD}<workbook xmlns=\"http://schemas.openxmlformats.org/spreadsheetml/2006/main\" xmlns:r=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships\"><workbookPr/><bookViews><workbookView/></bookViews><sheets>");
    for (k, s) in sheets.iter().enumerate() {
        wb.push_str(&format!(
            "<sheet name=\"{}\" sheetId=\"{}\" r:id=\"rId{}\"/>",
            xml_text(&s.name),
            k + 1,
            k + 1
        ));
    }
    wb.push_str("</sheets>");
    let defined: Vec<String> = sheets
        .iter()
        .enumerate()
        .filter_map(|(k, s)| {
            s.filter.as_ref().map(|f| {
                let (a, b) = f.split_once(':').unwrap_or((f, f));
                let abs = |c: &str| {
                    let letters: String = c.chars().take_while(|c| c.is_ascii_alphabetic()).collect();
                    format!("${}${}", letters, &c[letters.len()..])
                };
                format!(
                    "<definedName name=\"_xlnm._FilterDatabase\" localSheetId=\"{k}\" hidden=\"1\">'{}'!{}:{}</definedName>",
                    xml_text(&s.name.replace('\'', "''")),
                    abs(a),
                    abs(b)
                )
            })
        })
        .collect();
    if !defined.is_empty() {
        wb.push_str(&format!(
            "<definedNames>{}</definedNames>",
            defined.join("")
        ));
    }
    wb.push_str("</workbook>");
    put("xl/workbook.xml", &wb)?;
    let mut rels = format!("{XML_HEAD}<Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\">");
    for k in 1..=sheets.len() {
        rels.push_str(&format!("<Relationship Id=\"rId{k}\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet\" Target=\"worksheets/sheet{k}.xml\"/>"));
    }
    rels.push_str(&format!("<Relationship Id=\"rId{}\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/styles\" Target=\"styles.xml\"/></Relationships>", sheets.len() + 1));
    put("xl/_rels/workbook.xml.rels", &rels)?;
    put("xl/styles.xml", &format!("{XML_HEAD}<styleSheet xmlns=\"http://schemas.openxmlformats.org/spreadsheetml/2006/main\"><numFmts count=\"1\"><numFmt numFmtId=\"164\" formatCode=\"0.0%\"/></numFmts><fonts count=\"2\"><font><sz val=\"11\"/><name val=\"Calibri\"/><family val=\"2\"/></font><font><b/><sz val=\"11\"/><color rgb=\"00FFFFFF\"/><name val=\"Calibri\"/><family val=\"2\"/></font></fonts><fills count=\"3\"><fill><patternFill patternType=\"none\"/></fill><fill><patternFill patternType=\"gray125\"/></fill><fill><patternFill patternType=\"solid\"><fgColor rgb=\"001F6F5C\"/><bgColor indexed=\"64\"/></patternFill></fill></fills><borders count=\"1\"><border><left/><right/><top/><bottom/><diagonal/></border></borders><cellStyleXfs count=\"1\"><xf numFmtId=\"0\" fontId=\"0\" fillId=\"0\" borderId=\"0\"/></cellStyleXfs><cellXfs count=\"4\"><xf numFmtId=\"0\" fontId=\"0\" fillId=\"0\" borderId=\"0\" xfId=\"0\"/><xf numFmtId=\"0\" fontId=\"1\" fillId=\"2\" borderId=\"0\" xfId=\"0\" applyFont=\"1\" applyFill=\"1\" applyAlignment=\"1\"><alignment vertical=\"center\" wrapText=\"1\"/></xf><xf numFmtId=\"164\" fontId=\"0\" fillId=\"0\" borderId=\"0\" xfId=\"0\" applyNumberFormat=\"1\"/><xf numFmtId=\"0\" fontId=\"0\" fillId=\"0\" borderId=\"0\" xfId=\"0\" applyAlignment=\"1\"><alignment vertical=\"top\" wrapText=\"1\"/></xf></cellXfs><cellStyles count=\"1\"><cellStyle name=\"Normal\" xfId=\"0\" builtinId=\"0\"/></cellStyles></styleSheet>"))?;
    for (k, s) in sheets.iter().enumerate() {
        put(&format!("xl/worksheets/sheet{}.xml", k + 1), &s.xml)?;
    }
    z.finish().map_err(std::io::Error::other)?;
    Ok(())
}

/// `str(v)` of a cell as openpyxl (`data_only=True`) would give it.
fn data_str(d: &Data) -> Option<String> {
    Some(match d {
        Data::Empty => return None,
        Data::Int(i) => i.to_string(),
        // calamine gives every number as a float; openpyxl returns an int for whole numbers written without a
        // decimal point (the usual case), so whole values print as ints
        Data::Float(f) if f.fract() == 0.0 && f.abs() < 1e15 => (*f as i64).to_string(),
        Data::Float(f) => pyfmt::float_str(*f),
        Data::String(s) => s.clone(),
        Data::Bool(b) => if *b { "True" } else { "False" }.to_string(),
        Data::DateTime(dt) => match dt.as_datetime() {
            Some(t) => {
                let base = t.format("%Y-%m-%d %H:%M:%S").to_string();
                let micros = t.and_utc().timestamp_subsec_micros();
                if micros > 0 {
                    format!("{base}.{micros:06}")
                } else {
                    base
                }
            }
            None => pyfmt::float_str(dt.as_f64()),
        },
        Data::DateTimeIso(s) | Data::DurationIso(s) => s.clone(),
        Data::Error(e) => e.to_string(),
    })
}

/// A workbook as text: one Markdown-ish table per sheet, cut at about `limit` characters.
pub fn read_text(path: &str, limit: usize) -> Result<String, XlsxError> {
    let mut wb = calamine::open_workbook_auto(path).map_err(|e| XlsxError(e.to_string()))?;
    let names: Vec<String> = wb.sheet_names().to_vec();
    let mut out: Vec<String> = Vec::new();
    let mut size = 0usize;
    for name in names {
        let range = wb
            .worksheet_range(&name)
            .map_err(|e| XlsxError(e.to_string()))?;
        out.push(format!("## Sheet: {name}"));
        size += out.last().unwrap().chars().count();
        let mut n = 0;
        for row in range.rows() {
            if row.iter().all(|c| matches!(c, Data::Empty)) {
                continue;
            }
            let cells: Vec<String> = row
                .iter()
                .map(|c| {
                    data_str(c)
                        .map(|s| s.replace('\n', " "))
                        .unwrap_or_default()
                })
                .collect();
            out.push(format!("| {} |", cells.join(" | ")));
            size += out.last().unwrap().chars().count();
            n += 1;
            if size > limit {
                out.push("…[已截断 truncated]".to_string());
                return Ok(out.join("\n"));
            }
        }
        if n == 0 {
            out.push("(空 empty)".to_string());
            size += out.last().unwrap().chars().count();
        }
    }
    Ok(out.join("\n"))
}

/// Load a workbook for `dataq`: the grid of one sheet (`None` = first) as JSON values; errors carry the texts
/// `dataq.load` reports.
pub(crate) fn read_grid(
    path: &str,
    sheet: Option<&str>,
    max_rows: usize,
) -> Result<Vec<Vec<Value>>, String> {
    let mut wb = calamine::open_workbook_auto(path).map_err(|e| {
        format!(
            "cannot open workbook (damaged or not a real .xlsx?): {}",
            take_chars(&e.to_string(), 120)
        )
    })?;
    let names: Vec<String> = wb.sheet_names().to_vec();
    let name = match sheet {
        Some(s) => {
            if !names.iter().any(|n| n == s) {
                return Err(format!(
                    "no sheet {}; sheets: {}",
                    pyfmt::py_repr(&Value::String(s.to_string())),
                    pyfmt::py_repr(&Value::Array(
                        names.iter().map(|n| Value::String(n.clone())).collect()
                    ))
                ));
            }
            s.to_string()
        }
        None => names
            .first()
            .cloned()
            .ok_or_else(|| "the table is empty".to_string())?,
    };
    let range = wb.worksheet_range(&name).map_err(|e| {
        format!(
            "cannot open workbook (damaged or not a real .xlsx?): {}",
            take_chars(&e.to_string(), 120)
        )
    })?;
    let mut grid = Vec::new();
    for row in range.rows() {
        let any = row
            .iter()
            .any(|c| !matches!(c, Data::Empty) && !matches!(c, Data::String(s) if s.is_empty()));
        if any {
            grid.push(
                row.iter()
                    .map(|c| match c {
                        Data::Empty => Value::Null,
                        Data::Int(i) => json!(i),
                        Data::Float(f) => pyfmt::float_value(*f),
                        Data::String(s) => Value::String(s.clone()),
                        Data::Bool(b) => Value::Bool(*b),
                        Data::DateTime(dt) => match dt.as_datetime() {
                            Some(t) => Value::String(
                                t.format("%Y-%m-%d %H:%M:%S")
                                    .to_string()
                                    .replace(" 00:00:00", ""),
                            ),
                            None => pyfmt::float_value(dt.as_f64()),
                        },
                        Data::DateTimeIso(s) | Data::DurationIso(s) => Value::String(s.clone()),
                        Data::Error(e) => Value::String(e.to_string()),
                    })
                    .collect(),
            );
        }
        if grid.len() > max_rows {
            break;
        }
    }
    Ok(grid)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> String {
        let dir = std::env::temp_dir().join(format!("omuse-docs-xlsx-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir.join(name).to_string_lossy().into_owned()
    }

    #[test]
    fn roundtrip_numbers_and_safe_formulas() {
        let p = tmp("t.xlsx");
        let info = make(
            &p,
            &[
                json!({"name": "航班 Flights", "columns": ["航空公司", "价格 SGD", "占比", "备注"],
                "rows": [["Scoot", "600", "12.5%", "=HYPERLINK(\"http://evil\",\"x\")"],
                         ["ANA", "1,085", "30%", "=SUM(B2:B3)"],
                         ["ZIPAIR", 805.5, null, "-"]]}),
            ],
            true,
        )
        .unwrap();
        assert_eq!(info, json!({"sheets": ["航班 Flights"], "rows": 3}));
        let mut wb = calamine::open_workbook_auto(&p).unwrap();
        let r = wb.worksheet_range("航班 Flights").unwrap();
        assert_eq!(r.get_value((1, 1)), Some(&Data::Float(600.0)));
        assert_eq!(r.get_value((2, 1)), Some(&Data::Float(1085.0)));
        assert_eq!(r.get_value((3, 1)), Some(&Data::Float(805.5)));
        assert_eq!(r.get_value((1, 2)), Some(&Data::Float(0.125)));
        assert_eq!(
            r.get_value((1, 3)),
            Some(&Data::String("=HYPERLINK(\"http://evil\",\"x\")".into()))
        );
        let mut xml = String::new();
        {
            let f = std::fs::File::open(&p).unwrap();
            let mut z = zip::ZipArchive::new(f).unwrap();
            std::io::Read::read_to_string(
                &mut z.by_name("xl/worksheets/sheet1.xml").unwrap(),
                &mut xml,
            )
            .unwrap();
        }
        assert!(xml.contains("<c r=\"D3\"><f>SUM(B2:B3)</f></c>"));
        assert!(xml.contains("<c r=\"C2\" s=\"2\"><v>0.125</v></c>"));
        assert!(xml.contains("<pane ySplit=\"1\" topLeftCell=\"A2\""));
        assert!(xml.contains("<autoFilter ref=\"A1:D4\"/>"));
        assert!(xml.contains("<c r=\"A1\" s=\"1\" t=\"inlineStr\">"));
        let txt = read_text(&p, 12000).unwrap();
        assert!(txt.contains("Scoot") && txt.contains("航班 Flights"));
        assert_eq!(
            txt.lines().nth(1).unwrap(),
            "| 航空公司 | 价格 SGD | 占比 | 备注 |"
        );
        assert_eq!(
            txt.lines().nth(2).unwrap(),
            "| Scoot | 600 | 0.125 | =HYPERLINK(\"http://evil\",\"x\") |"
        );
        assert_eq!(txt.lines().nth(4).unwrap(), "| ZIPAIR | 805.5 |  | - |");
    }

    #[test]
    fn formulas_off_by_default() {
        let p = tmp("t2.xlsx");
        make(
            &p,
            &[json!({"columns": ["a"], "rows": [["=1+1"], ["+cmd|' /C calc'!A0"]]})],
            false,
        )
        .unwrap();
        let mut wb = calamine::open_workbook_auto(&p).unwrap();
        let r = wb.worksheet_range("Sheet1").unwrap();
        assert_eq!(r.get_value((1, 0)), Some(&Data::String("=1+1".into())));
        assert_eq!(
            r.get_value((2, 0)),
            Some(&Data::String("+cmd|' /C calc'!A0".into()))
        );
        assert_eq!(
            cell_value(&json!("=1+1"), false),
            (json!("=1+1"), CellFormat::Text)
        );
        assert_eq!(
            cell_value(&json!("=1+1"), true),
            (json!("=1+1"), CellFormat::None)
        );
        assert_eq!(
            cell_value(&json!("=WEBSERVICE(\"x\")"), true),
            (json!("=WEBSERVICE(\"x\")"), CellFormat::Text)
        );
        assert_eq!(
            cell_value(&json!(" 1,085 "), true),
            (json!(1085), CellFormat::None)
        );
        assert_eq!(
            cell_value(&json!("1.50"), true),
            (json!(1.5), CellFormat::None)
        );
        assert_eq!(
            cell_value(&json!("-"), true),
            (json!("-"), CellFormat::Text)
        );
    }

    #[test]
    fn from_markdown_and_csv() {
        let rows = rows_from_markdown(
            "intro\n\n| 名称 | 价格 |\n|---|---:|\n| **A** | 1 |\n| B | 2 |\n\nafter",
        );
        assert_eq!(
            rows,
            vec![vec!["名称", "价格"], vec!["A", "1"], vec!["B", "2"]]
        );
        assert_eq!(
            rows_from_csv("a,b\n\"x, y\",2\n"),
            vec![vec!["a", "b"], vec!["x, y", "2"]]
        );
    }

    #[test]
    fn dict_rows_and_sheet_names() {
        let p = tmp("t3.xlsx");
        let info = make(
            &p,
            &[
                json!({"name": "a/b:c", "rows": [{"x": 1, "y": "two"}, {"x": 3}]}),
                json!({"name": "a/b:c", "rows": [[1]]}),
                json!({}),
            ],
            false,
        )
        .unwrap();
        assert_eq!(
            info,
            json!({"sheets": ["a_b_c", "a_b_c_2", "Sheet3"], "rows": 3})
        );
        let txt = read_text(&p, 12000).unwrap();
        assert_eq!(txt, "## Sheet: a_b_c\n| x | y |\n| 1 | two |\n| 3 |  |\n## Sheet: a_b_c_2\n| 1 |\n## Sheet: Sheet3\n(空 empty)");
        assert!(make(&p, &[], false).is_err());
    }
}
