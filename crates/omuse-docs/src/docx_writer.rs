//! Markdown → Word (.docx) without third-party packages (`app/common/docx_writer.py`; the 2026-10-02 test pasted a
//! user's contract summary into an online converter because OMuse had no way to make a Word file).
//!
//! Supported: # / ## / ### headings, paragraphs, **bold**, *italic*, `code`, bullet (- * •) and numbered (1.) lists,
//! Markdown tables, > quotes, --- rules and ![alt](workspace image) pictures (PNG/JPEG). Chinese text uses a CJK font.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use fancy_regex::Regex as FRegex;
use regex::Regex;
use serde_json::{json, Value};

use crate::pyfmt::html_escape;

/// Latin font of the document.
pub const FONT_LATIN: &str = "Calibri";
/// CJK font of the document.
pub const FONT_CJK: &str = "Microsoft YaHei";

/// `DocxError` of the Python module.
#[derive(Debug, Clone, thiserror::Error)]
#[error("{0}")]
pub struct DocxError(pub String);

static LINK_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\[([^\]]+)\]\((https?://[^)\s]+)\)").unwrap());
static RUN_SPLIT: LazyLock<FRegex> =
    LazyLock::new(|| FRegex::new(r"(\*\*[^*]+\*\*|`[^`]+`|(?<!\*)\*[^*\s][^*]*\*(?!\*))").unwrap());
static TABLE_SEP: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^\s*\|?\s*:?-{2,}").unwrap());
static HEADING_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^(#{1,6})\s+(.*)").unwrap());
static IMG_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^!\[([^\]]*)\]\(([^)\s]+)\)").unwrap());
static BULLET_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[-*•]\s+").unwrap());
static NUMBERED_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^\d+[.)、]\s*").unwrap());
static RULE_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^(-{3,}|\*{3,}|_{3,})$").unwrap());

/// `xml.sax.saxutils.escape`.
fn esc(s: &str) -> String {
    html_escape(s, false)
}

/// Inline Markdown → runs (bold, italic, code); links keep their text and show the URL in brackets.
fn runs(text: &str) -> String {
    let text = LINK_RE.replace_all(text, "$1 ($2)");
    // Python re.split with a capturing group: text between matches and the matches themselves
    let mut parts: Vec<&str> = Vec::new();
    let mut last = 0;
    for m in RUN_SPLIT.find_iter(&text).flatten() {
        parts.push(&text[last..m.start()]);
        parts.push(m.as_str());
        last = m.end();
    }
    parts.push(&text[last..]);
    let mut out = String::new();
    for part in parts {
        if part.is_empty() {
            continue;
        }
        let n = part.chars().count();
        let (part, props) = if part.starts_with("**") && part.ends_with("**") && n > 4 {
            (&part[2..part.len() - 2], "<w:b/>")
        } else if part.starts_with('`') && part.ends_with('`') && n > 2 {
            (&part[1..part.len() - 1], "<w:rFonts w:ascii=\"Consolas\" w:hAnsi=\"Consolas\"/><w:shd w:val=\"clear\" w:fill=\"F2F2F2\"/>")
        } else if part.starts_with('*') && part.ends_with('*') && n > 2 {
            (&part[1..part.len() - 1], "<w:i/>")
        } else {
            (part, "")
        };
        out.push_str(&format!(
            "<w:r><w:rPr>{props}</w:rPr><w:t xml:space=\"preserve\">{}</w:t></w:r>",
            esc(part)
        ));
    }
    out
}

fn p(text: &str, style: &str, num: Option<(u32, u32)>, align: &str) -> String {
    let mut ppr = String::new();
    if !style.is_empty() {
        ppr.push_str(&format!("<w:pStyle w:val=\"{style}\"/>"));
    }
    if let Some((id, lvl)) = num {
        ppr.push_str(&format!(
            "<w:numPr><w:ilvl w:val=\"{lvl}\"/><w:numId w:val=\"{id}\"/></w:numPr>"
        ));
    }
    if !align.is_empty() {
        ppr.push_str(&format!("<w:jc w:val=\"{align}\"/>"));
    }
    format!("<w:p><w:pPr>{ppr}</w:pPr>{}</w:p>", runs(text))
}

fn table(rows: &[Vec<String>]) -> String {
    let ncol = rows.iter().map(|r| r.len()).max().unwrap_or(0);
    let w = 9000 / ncol.max(1);
    let grid: String = (0..ncol)
        .map(|_| format!("<w:gridCol w:w=\"{w}\"/>"))
        .collect();
    let borders: String = ["top", "left", "bottom", "right", "insideH", "insideV"]
        .iter()
        .map(|b| format!("<w:{b} w:val=\"single\" w:sz=\"4\" w:color=\"BFBFBF\"/>"))
        .collect();
    let mut xml = format!(
        "<w:tbl><w:tblPr><w:tblStyle w:val=\"TableGrid\"/><w:tblW w:w=\"0\" w:type=\"auto\"/><w:tblBorders>{borders}</w:tblBorders></w:tblPr><w:tblGrid>{grid}</w:tblGrid>"
    );
    for (i, r) in rows.iter().enumerate() {
        let mut cells = String::new();
        for j in 0..ncol {
            let txt = r.get(j).map(|s| s.as_str()).unwrap_or("");
            let shade = if i == 0 {
                "<w:shd w:val=\"clear\" w:fill=\"EAF1FB\"/>"
            } else {
                ""
            };
            let body = if i == 0 && !txt.is_empty() && !txt.starts_with("**") {
                runs(&format!("**{txt}**"))
            } else {
                runs(txt)
            };
            cells.push_str(&format!(
                "<w:tc><w:tcPr><w:tcW w:w=\"{w}\" w:type=\"dxa\"/>{shade}</w:tcPr><w:p>{body}</w:p></w:tc>"
            ));
        }
        xml.push_str(&format!("<w:tr>{cells}</w:tr>"));
    }
    xml.push_str("</w:tbl><w:p/>");
    xml
}

/// Pixel size of a PNG or JPEG (800×600 when unknown).
fn png_size(data: &[u8]) -> (u32, u32) {
    if data.len() >= 24 && data[..8] == *b"\x89PNG\r\n\x1a\n" {
        let w = u32::from_be_bytes([data[16], data[17], data[18], data[19]]);
        let h = u32::from_be_bytes([data[20], data[21], data[22], data[23]]);
        return (w, h);
    }
    if data.len() >= 2 && data[..2] == [0xff, 0xd8] {
        // JPEG: walk the segments to the SOF marker
        let mut i = 2usize;
        while i + 9 < data.len() {
            if data[i] != 0xFF {
                i += 1;
                continue;
            }
            let m = data[i + 1];
            if matches!(m, 0xC0..=0xC2) {
                let h = u16::from_be_bytes([data[i + 5], data[i + 6]]) as u32;
                let w = u16::from_be_bytes([data[i + 7], data[i + 8]]) as u32;
                return (w, h);
            }
            i += 2 + u16::from_be_bytes([data[i + 2], data[i + 3]]) as usize;
        }
    }
    (800, 600)
}

fn image(rid: &str, idx: usize, w: u32, h: u32) -> String {
    let max_w = 6.0 * 914400.0; // 6 inches
    let cx = (w as f64 * 9525.0).min(max_w);
    let cy = (cx * h as f64 / (w.max(1)) as f64).trunc() as i64;
    let cx = cx as i64;
    format!(
        "<w:p><w:pPr><w:jc w:val=\"center\"/></w:pPr><w:r><w:drawing><wp:inline><wp:extent cx=\"{cx}\" cy=\"{cy}\"/><wp:docPr id=\"{idx}\" name=\"Picture {idx}\"/><a:graphic><a:graphicData uri=\"http://schemas.openxmlformats.org/drawingml/2006/picture\"><pic:pic><pic:nvPicPr><pic:cNvPr id=\"{idx}\" name=\"img{idx}\"/><pic:cNvPicPr/></pic:nvPicPr><pic:blipFill><a:blip r:embed=\"{rid}\"/><a:stretch><a:fillRect/></a:stretch></pic:blipFill><pic:spPr><a:xfrm><a:off x=\"0\" y=\"0\"/><a:ext cx=\"{cx}\" cy=\"{cy}\"/></a:xfrm><a:prstGeom prst=\"rect\"><a:avLst/></a:prstGeom></pic:spPr></pic:pic></a:graphicData></a:graphic></wp:inline></w:drawing></w:r></w:p>"
    )
}

/// The workspace image an `![alt](src)` refers to, when it is a PNG/JPEG inside `base_dir`.
fn resolve_image(base_dir: &str, src: &str) -> Option<(Vec<u8>, &'static str)> {
    let p: PathBuf = Path::new(base_dir).join(src);
    let p = std::fs::canonicalize(&p).ok()?;
    let base = std::fs::canonicalize(base_dir).ok()?;
    let mut prefix = base.into_os_string();
    prefix.push(std::path::MAIN_SEPARATOR_STR);
    if !p.to_string_lossy().starts_with(&*prefix.to_string_lossy()) || !p.is_file() {
        return None;
    }
    let ext = p
        .extension()
        .map(|e| e.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    let ext = match ext.as_str() {
        "png" => "png",
        "jpg" | "jpeg" => "jpeg",
        _ => return None,
    };
    let data = std::fs::read(&p).ok()?;
    Some((data, ext))
}

fn style(
    sid: &str,
    name: &str,
    size: u32,
    bold: bool,
    color: &str,
    spacing: u32,
    extra: &str,
) -> String {
    let b = if bold { "<w:b/>" } else { "" };
    let c = if color.is_empty() {
        String::new()
    } else {
        format!("<w:color w:val=\"{color}\"/>")
    };
    format!(
        "<w:style w:type=\"paragraph\" w:styleId=\"{sid}\"><w:name w:val=\"{name}\"/><w:basedOn w:val=\"Normal\"/><w:pPr><w:spacing w:before=\"{spacing}\" w:after=\"80\"/>{extra}</w:pPr><w:rPr>{b}{c}<w:sz w:val=\"{size}\"/></w:rPr></w:style>"
    )
}

fn absnum(aid: u32, fmt: &str, txt: &str) -> String {
    let lv: String = (0..3)
        .map(|k| {
            let t = if fmt != "decimal" { txt.to_string() } else { format!("%{}.", k + 1) };
            format!(
                "<w:lvl w:ilvl=\"{k}\"><w:start w:val=\"1\"/><w:numFmt w:val=\"{fmt}\"/><w:lvlText w:val=\"{t}\"/><w:lvlJc w:val=\"left\"/><w:pPr><w:ind w:left=\"{}\" w:hanging=\"300\"/></w:pPr></w:lvl>",
                420 + 420 * k
            )
        })
        .collect();
    format!("<w:abstractNum w:abstractNumId=\"{aid}\">{lv}</w:abstractNum>")
}

/// Convert Markdown to a .docx at `out_path`; returns `{"path", "size", "paragraphs", "images"}`.
pub fn markdown_to_docx(
    md: &str,
    out_path: &str,
    title: &str,
    base_dir: &str,
) -> Result<Value, DocxError> {
    let text = md.replace("\r\n", "\n");
    let lines: Vec<&str> = text.split('\n').collect();
    let mut body: Vec<String> = Vec::new();
    let mut images: Vec<(Vec<u8>, &'static str)> = Vec::new();
    let mut i = 0;
    if !title.is_empty()
        && !lines
            .first()
            .map(|l| l.trim_start().starts_with("# "))
            .unwrap_or(false)
    {
        body.push(p(title, "Title", None, ""));
    }
    while i < lines.len() {
        let ln = lines[i].trim_end();
        let s = ln.trim();
        if s.is_empty() {
            i += 1;
            continue;
        }
        if s.starts_with('|') && i + 1 < lines.len() && TABLE_SEP.is_match(lines[i + 1]) {
            let mut rows: Vec<Vec<String>> = Vec::new();
            while i < lines.len() && lines[i].trim().starts_with('|') {
                if !TABLE_SEP.is_match(lines[i]) {
                    rows.push(
                        lines[i]
                            .trim()
                            .trim_matches('|')
                            .split('|')
                            .map(|c| c.trim().to_string())
                            .collect(),
                    );
                }
                i += 1;
            }
            body.push(table(&rows));
            continue;
        }
        let heading = HEADING_RE.captures(s);
        let img = IMG_RE.captures(s);
        if let Some(m) = heading {
            let lvl = m[1].len();
            let st = if lvl == 1 && body.is_empty() {
                "Title".to_string()
            } else {
                format!("Heading{}", lvl.min(3))
            };
            body.push(p(m[2].trim(), &st, None, ""));
        } else if let (Some(im), false) = (img, base_dir.is_empty()) {
            let alt = &im[1];
            match resolve_image(base_dir, &im[2]) {
                Some((data, ext)) => {
                    let (w, h) = png_size(&data);
                    images.push((data, ext));
                    body.push(image(
                        &format!("rIdImg{}", images.len()),
                        images.len(),
                        w,
                        h,
                    ));
                    if !alt.is_empty() {
                        body.push(p(alt, "Caption", None, "center"));
                    }
                }
                None => body.push(p(
                    &format!("[{}]", if alt.is_empty() { "image" } else { alt }),
                    "",
                    None,
                    "",
                )),
            }
        } else if BULLET_RE.is_match(s) {
            let indent = (ln.chars().count() - ln.trim_start().chars().count()) / 2;
            body.push(p(
                &BULLET_RE.replace(s, ""),
                "ListParagraph",
                Some((1, indent.min(2) as u32)),
                "",
            ));
        } else if NUMBERED_RE.is_match(s) {
            body.push(p(
                &NUMBERED_RE.replace(s, ""),
                "ListParagraph",
                Some((2, 0)),
                "",
            ));
        } else if s.starts_with('>') {
            body.push(p(
                s.trim_start_matches(['>', ' ']).trim(),
                "Quote",
                None,
                "",
            ));
        } else if RULE_RE.is_match(s) {
            body.push("<w:p><w:pPr><w:pBdr><w:bottom w:val=\"single\" w:sz=\"6\" w:space=\"1\" w:color=\"BFBFBF\"/></w:pBdr></w:pPr></w:p>".to_string());
        } else {
            body.push(p(s, "", None, ""));
        }
        i += 1;
    }
    if body.is_empty() {
        return Err(DocxError("内容为空 (empty document)".into()));
    }
    let ns = "xmlns:w=\"http://schemas.openxmlformats.org/wordprocessingml/2006/main\" xmlns:r=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships\" xmlns:wp=\"http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing\" xmlns:a=\"http://schemas.openxmlformats.org/drawingml/2006/main\" xmlns:pic=\"http://schemas.openxmlformats.org/drawingml/2006/picture\"";
    let document = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?><w:document {ns}><w:body>{}<w:sectPr><w:pgSz w:w=\"11906\" w:h=\"16838\"/><w:pgMar w:top=\"1200\" w:right=\"1200\" w:bottom=\"1200\" w:left=\"1200\" w:header=\"708\" w:footer=\"708\" w:gutter=\"0\"/></w:sectPr></w:body></w:document>",
        body.join("")
    );
    let fonts = format!("<w:rFonts w:ascii=\"{FONT_LATIN}\" w:hAnsi=\"{FONT_LATIN}\" w:eastAsia=\"{FONT_CJK}\" w:cs=\"{FONT_LATIN}\"/>");
    let styles = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?><w:styles xmlns:w=\"http://schemas.openxmlformats.org/wordprocessingml/2006/main\"><w:docDefaults><w:rPrDefault><w:rPr>{fonts}<w:sz w:val=\"22\"/><w:lang w:val=\"en-US\" w:eastAsia=\"zh-CN\"/></w:rPr></w:rPrDefault><w:pPrDefault><w:pPr><w:spacing w:after=\"100\" w:line=\"300\" w:lineRule=\"auto\"/></w:pPr></w:pPrDefault></w:docDefaults><w:style w:type=\"paragraph\" w:default=\"1\" w:styleId=\"Normal\"><w:name w:val=\"Normal\"/></w:style>{}{}{}{}{}{}{}<w:style w:type=\"table\" w:styleId=\"TableGrid\"><w:name w:val=\"Table Grid\"/></w:style></w:styles>",
        style("Title", "Title", 36, true, "1F3864", 0, ""),
        style("Heading1", "heading 1", 30, true, "1F3864", 240, ""),
        style("Heading2", "heading 2", 26, true, "2E5597", 200, ""),
        style("Heading3", "heading 3", 23, true, "2E5597", 160, ""),
        style("ListParagraph", "List Paragraph", 22, false, "", 0, ""),
        style("Quote", "Quote", 22, false, "595959", 120, "<w:ind w:left=\"567\"/>"),
        style("Caption", "caption", 18, false, "7F7F7F", 0, ""),
    );
    let numbering = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?><w:numbering xmlns:w=\"http://schemas.openxmlformats.org/wordprocessingml/2006/main\">{}{}<w:num w:numId=\"1\"><w:abstractNumId w:val=\"0\"/></w:num><w:num w:numId=\"2\"><w:abstractNumId w:val=\"1\"/></w:num></w:numbering>",
        absnum(0, "bullet", "•"),
        absnum(1, "decimal", "")
    );
    let img_rels: String = images
        .iter()
        .enumerate()
        .map(|(k, (_d, ext))| {
            format!(
                "<Relationship Id=\"rIdImg{}\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/image\" Target=\"media/image{}.{ext}\"/>",
                k + 1,
                k + 1
            )
        })
        .collect();
    let rels = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?><Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\"><Relationship Id=\"rIdStyles\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/styles\" Target=\"styles.xml\"/><Relationship Id=\"rIdNum\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/numbering\" Target=\"numbering.xml\"/>{img_rels}</Relationships>"
    );
    let ctypes = "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?><Types xmlns=\"http://schemas.openxmlformats.org/package/2006/content-types\"><Default Extension=\"rels\" ContentType=\"application/vnd.openxmlformats-package.relationships+xml\"/><Default Extension=\"xml\" ContentType=\"application/xml\"/><Default Extension=\"png\" ContentType=\"image/png\"/><Default Extension=\"jpeg\" ContentType=\"image/jpeg\"/><Override PartName=\"/word/document.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml\"/><Override PartName=\"/word/styles.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.wordprocessingml.styles+xml\"/><Override PartName=\"/word/numbering.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.wordprocessingml.numbering+xml\"/></Types>";
    let root_rels = "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?><Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\"><Relationship Id=\"rId1\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument\" Target=\"word/document.xml\"/></Relationships>";
    let dir = Path::new(out_path)
        .parent()
        .filter(|d| !d.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    std::fs::create_dir_all(dir).map_err(|e| DocxError(e.to_string()))?;
    let io = |e: std::io::Error| DocxError(e.to_string());
    let zerr = |e: zip::result::ZipError| DocxError(e.to_string());
    let file = std::fs::File::create(out_path).map_err(io)?;
    let mut z = zip::ZipWriter::new(file);
    let opts = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);
    for (name, data) in [
        ("[Content_Types].xml", ctypes.as_bytes()),
        ("_rels/.rels", root_rels.as_bytes()),
        ("word/document.xml", document.as_bytes()),
        ("word/styles.xml", styles.as_bytes()),
        ("word/numbering.xml", numbering.as_bytes()),
        ("word/_rels/document.xml.rels", rels.as_bytes()),
    ] {
        z.start_file(name, opts).map_err(zerr)?;
        z.write_all(data).map_err(io)?;
    }
    for (k, (data, ext)) in images.iter().enumerate() {
        z.start_file(format!("word/media/image{}.{ext}", k + 1), opts)
            .map_err(zerr)?;
        z.write_all(data).map_err(io)?;
    }
    z.finish().map_err(zerr)?;
    let size = std::fs::metadata(out_path).map_err(io)?.len();
    Ok(json!({"path": out_path, "size": size, "paragraphs": body.len(), "images": images.len()}))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runs_and_paragraphs() {
        assert_eq!(
            runs("a **b** `c` *d* [e](https://x.y)"),
            "<w:r><w:rPr></w:rPr><w:t xml:space=\"preserve\">a </w:t></w:r><w:r><w:rPr><w:b/></w:rPr><w:t xml:space=\"preserve\">b</w:t></w:r><w:r><w:rPr></w:rPr><w:t xml:space=\"preserve\"> </w:t></w:r><w:r><w:rPr><w:rFonts w:ascii=\"Consolas\" w:hAnsi=\"Consolas\"/><w:shd w:val=\"clear\" w:fill=\"F2F2F2\"/></w:rPr><w:t xml:space=\"preserve\">c</w:t></w:r><w:r><w:rPr></w:rPr><w:t xml:space=\"preserve\"> </w:t></w:r><w:r><w:rPr><w:i/></w:rPr><w:t xml:space=\"preserve\">d</w:t></w:r><w:r><w:rPr></w:rPr><w:t xml:space=\"preserve\"> e (https://x.y)</w:t></w:r>"
        );
        assert_eq!(p("x & y", "Quote", None, ""), "<w:p><w:pPr><w:pStyle w:val=\"Quote\"/></w:pPr><w:r><w:rPr></w:rPr><w:t xml:space=\"preserve\">x &amp; y</w:t></w:r></w:p>");
        assert_eq!(p("i", "ListParagraph", Some((1, 2)), "center"), "<w:p><w:pPr><w:pStyle w:val=\"ListParagraph\"/><w:numPr><w:ilvl w:val=\"2\"/><w:numId w:val=\"1\"/></w:numPr><w:jc w:val=\"center\"/></w:pPr><w:r><w:rPr></w:rPr><w:t xml:space=\"preserve\">i</w:t></w:r></w:p>");
    }

    #[test]
    fn image_sizes() {
        let mut png = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR".to_vec();
        png.extend_from_slice(&640u32.to_be_bytes());
        png.extend_from_slice(&480u32.to_be_bytes());
        assert_eq!(png_size(&png), (640, 480));
        assert_eq!(png_size(b"junk"), (800, 600));
        assert!(
            image("rIdImg1", 1, 640, 480).contains("<wp:extent cx=\"5486400\" cy=\"4114800\"/>")
        );
        assert!(image("rIdImg1", 1, 100, 50).contains("<wp:extent cx=\"952500\" cy=\"476250\"/>"));
    }

    #[test]
    fn whole_document() {
        let dir = std::env::temp_dir().join(format!("omuse-docs-docx-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut png = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR".to_vec();
        png.extend_from_slice(&10u32.to_be_bytes());
        png.extend_from_slice(&20u32.to_be_bytes());
        std::fs::write(dir.join("pic.png"), &png).unwrap();
        let out = dir.join("sub").join("r.docx");
        let md = "# 合同摘要\n\nIntro **bold**.\n\n| a | b |\n|---|---|\n| 1 | 2 |\n\n- one\n  - two\n1. first\n> quote\n---\n![cap](pic.png)\n![missing](../etc/passwd.png)\n";
        let info = markdown_to_docx(md, out.to_str().unwrap(), "T", dir.to_str().unwrap()).unwrap();
        assert_eq!(info["paragraphs"], 11);
        assert_eq!(info["images"], 1);
        assert!(info["size"].as_u64().unwrap() > 1000);
        let f = std::fs::File::open(&out).unwrap();
        let mut z = zip::ZipArchive::new(f).unwrap();
        let mut doc = String::new();
        std::io::Read::read_to_string(&mut z.by_name("word/document.xml").unwrap(), &mut doc)
            .unwrap();
        assert!(doc.contains("<w:pStyle w:val=\"Title\"/></w:pPr><w:r><w:rPr></w:rPr><w:t xml:space=\"preserve\">合同摘要</w:t>"));
        assert!(doc.contains("<w:ilvl w:val=\"1\"/><w:numId w:val=\"1\"/>"));
        assert!(doc.contains("<w:numId w:val=\"2\"/>"));
        assert!(doc.contains("[missing]"));
        assert!(doc.contains("r:embed=\"rIdImg1\""));
        assert!(doc.contains("<w:pStyle w:val=\"Caption\"/><w:jc w:val=\"center\"/>"));
        assert!(z.by_name("word/media/image1.png").is_ok());
        assert!(markdown_to_docx("\n\n", out.to_str().unwrap(), "", "").is_err());
    }
}
