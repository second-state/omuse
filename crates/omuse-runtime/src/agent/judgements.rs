//! The judgements the run still needs from the model on a PAGE, each one a [`Runtime::decide`] call over a fixed list
//! of choices and a fixed code action on the choice (never a message to the model): a deep link that lands on an error
//! page (code opens the site's home page) and a shop's product page the agent comes back to without adding to the cart
//! (code clicks the page's own Add-to-Cart button).
//!
//! And the structured data and pure parts of facts and steps decided from JSON (docs/json-tools.md): the official
//! medical, consular and embassy lines and the official visa pages and visa-free table (`emergency_contacts` and
//! `visa_rules`, agent/facts.rs), and the file formats the user asked for (the file calls' `deliver_as`): what the run
//! did not make is converted from what it made and sent.
//!
//! This file holds the pure parts (what the transcript shows, the calls code makes); the decisions and the calls run
//! in `runtime.rs` (the end of a run) and `exec.rs` (a page just opened).

use std::sync::LazyLock;

use omuse_common::guard::urlparse;
use omuse_common::util::{get_str, get_truthy, take_chars};
use regex::Regex;
use serde_json::{json, Value};

use super::util::{content_of, sget, stored_call, tool_calls};
use crate::llm::ToolCall;

/// `(call id, tool name, args)` of every call in the transcript, in order.
fn calls(transcript: &[Value]) -> Vec<(String, String, Value)> {
    transcript
        .iter()
        .flat_map(tool_calls)
        .map(|tc| {
            let (n, a) = stored_call(tc);
            (get_str(tc, "id"), n, a)
        })
        .collect()
}

/// The result of call `id`, when it ran without an error or a refusal.
fn ok_result<'a>(transcript: &'a [Value], id: &str) -> Option<&'a Value> {
    transcript.iter().find(|m| {
        get_str(m, "role") == "tool" && get_str(m, "tool_call_id") == id && {
            let c = content_of(m);
            !c.starts_with("ERROR") && !c.starts_with("DENIED")
        }
    })
}

// ---------------------------------------------------------------- a held pick

// ---------------------------------------------------------------- a held offer (the previous turn's option)

// ---------------------------------------------------------------- local medical help in a health emergency

/// An official medical information line of a destination: who runs it, its phone as the official page lists it, its
/// English hours and the page. Fixed data, checked against the official pages (Oct 2026).
#[derive(Debug, PartialEq)]
pub struct MedicalLine {
    pub place: &'static str,
    /// lowercase words that name the place in a request
    pub words: &'static [&'static str],
    pub service: &'static str,
    pub phone: &'static str,
    pub hours: &'static str,
    pub url: &'static str,
}

pub const MEDICAL_LINES: &[MedicalLine] = &[
    MedicalLine {
        place: "Tokyo",
        words: &["tokyo", "东京", "東京"],
        service: "Tokyo Metropolitan Health and Medical Information Center \"Himawari\" (finds a clinic or hospital with \
English support)",
        phone: "03-5285-8181",
        hours: "English daily 9:00–20:00",
        url: "https://www.hokeniryo.metro.tokyo.lg.jp/iryo/iryo_hoken/medical_info_eng/himawari",
    },
    MedicalLine {
        place: "Japan",
        words: &["japan", "osaka", "kyoto", "sapporo", "fukuoka", "okinawa", "nagoya", "日本", "大阪", "京都"],
        service: "Japan Visitor Hotline (JNTO: medical institutions with English, emergencies)",
        phone: "050-3816-2787",
        hours: "24 hours",
        url: "https://www.jnto.go.jp/emergency/eng/mi_guide.html",
    },
];

// ---------------------------------------------------------------- the consulate in a lost-passport emergency

/// A government's 24-hour consular emergency line for its citizens abroad, as its official page lists it (fixed data,
/// Oct 2026): the line the answer to a lost or stolen passport always carries, in its international form.
#[derive(Debug, PartialEq)]
pub struct ConsularLine {
    /// the citizenship, as the answer names it ("Australian")
    pub citizens: &'static str,
    /// lowercase nationalities of the profile it serves
    pub nationality: &'static [&'static str],
    pub service: &'static str,
    /// international form ("+61 2 6261 3305")
    pub phone: &'static str,
    pub url: &'static str,
}

#[rustfmt::skip]
pub const CONSULAR_LINES: &[ConsularLine] = &[
    ConsularLine { citizens: "Australian", nationality: &["australia", "australian"],
        service: "Australian Government 24-hour Consular Emergency Centre", phone: "+61 2 6261 3305",
        url: "https://www.smartraveller.gov.au/" },
    ConsularLine { citizens: "British", nationality: &["united kingdom", "uk", "british", "great britain"],
        service: "Foreign, Commonwealth & Development Office (24 hours)", phone: "+44 20 7008 5000",
        url: "https://www.gov.uk/guidance/contact-the-fcdo" },
    ConsularLine { citizens: "US", nationality: &["united states", "usa", "us", "american"],
        service: "U.S. Department of State, Overseas Citizens Services (24 hours)", phone: "+1 202 501 4444",
        url: "https://travel.state.gov/content/travel/en/contact-us/Emergencies-Abroad.html" },
    ConsularLine { citizens: "Canadian", nationality: &["canada", "canadian"],
        service: "Global Affairs Canada Emergency Watch and Response Centre (24 hours)", phone: "+1 613 996 8885",
        url: "https://travel.gc.ca/assistance/emergency-assistance" },
    ConsularLine { citizens: "New Zealand", nationality: &["new zealand", "new zealander"],
        service: "MFAT 24-hour Consular Emergency Line", phone: "+64 99 20 20 20",
        url: "https://www.safetravel.govt.nz/" },
    ConsularLine { citizens: "Singapore", nationality: &["singapore", "singaporean"],
        service: "MFA Duty Office (24 hours)", phone: "+65 6379 8800",
        url: "https://www.mfa.gov.sg/travelling-overseas/general-travel-advice-for-singaporeans/" },
];

/// An embassy of a nationality in a destination, its phone as its official page lists it (international form and
/// the form dialled locally) and that page — fixed data, checked against the embassy's site (Oct 2026).
#[derive(Debug, PartialEq)]
pub struct EmbassyLine {
    /// `ConsularLine::citizens` of the nationality it serves
    pub citizens: &'static str,
    /// lowercase words that name the destination in a request
    pub words: &'static [&'static str],
    pub name: &'static str,
    pub phone: &'static str,
    pub local: &'static str,
    pub url: &'static str,
}

#[rustfmt::skip]
pub const EMBASSY_LINES: &[EmbassyLine] = &[
    EmbassyLine { citizens: "Australian",
        words: &["japan", "tokyo", "osaka", "kyoto", "sapporo", "fukuoka", "okinawa", "nagoya", "日本", "东京", "東京", "大阪", "京都"],
        name: "Australian Embassy Tokyo", phone: "+81 3 5232 4111", local: "03-5232-4111",
        url: "https://japan.embassy.gov.au/tkyo/consular.html" },
];

/// The consular help for a lost or stolen passport: the nationality's 24-hour line and, when the request names a
/// destination the table knows, its embassy there.
#[derive(Debug, PartialEq)]
pub struct ConsularHelp {
    pub line: &'static ConsularLine,
    pub embassy: Option<&'static EmbassyLine>,
}

/// Whether `low` (a lowercased place, e.g. a tool argument's city and country) names one of `words`.
pub(crate) fn names_place(low: &str, words: &[&str]) -> bool {
    words.iter().any(|w| {
        if w.is_ascii() {
            Regex::new(&format!(r"\b{}\b", regex::escape(w))).is_ok_and(|r| r.is_match(low))
        } else {
            low.contains(w)
        }
    })
}

/// The consular help for a nationality and a place (`emergency_contacts`' arguments, lowercased); `None` for a
/// nationality the table does not know (nothing is added — never a guess).
pub fn consular_help_for(nationality: &str, place: &str) -> Option<ConsularHelp> {
    let nat = nationality.trim().to_lowercase();
    let line = CONSULAR_LINES
        .iter()
        .find(|l| l.nationality.contains(&nat.as_str()))?;
    let low = place.to_lowercase();
    let embassy = EMBASSY_LINES
        .iter()
        .find(|e| e.citizens == line.citizens && names_place(&low, e.words));
    Some(ConsularHelp { line, embassy })
}

// ---------------------------------------------------------------- the file formats asked for

/// A file the run made, with what it can be converted from.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Made {
    /// the workspace path
    pub path: String,
    /// its extension, lowercased
    pub ext: String,
    /// the Markdown it was made from, when the call gave it as text
    pub markdown: String,
    /// the workspace text file it was made from (`.md` / `.txt` / `.html` / `.csv`), or the file itself when it is one
    pub source: String,
    /// an Excel workbook's sheets as the call gave them
    pub sheets: Value,
}

fn ext_of(path: &str) -> String {
    path.rsplit_once('.')
        .map(|(_, e)| e.to_lowercase())
        .filter(|e| !e.contains('/'))
        .unwrap_or_default()
}

fn stem_of(path: &str) -> &str {
    match path.rsplit_once('.') {
        Some((s, e)) if !e.contains('/') => s,
        _ => path,
    }
}

/// The files the run made (`files_write`, `make_pdf`, `make_docx`, `make_xlsx` without an error), the path read from
/// each result.
pub fn made_files(transcript: &[Value]) -> Vec<Made> {
    static MADE: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"created locally: (\S+?) \(").unwrap());
    static WRITTEN: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"written: (\S+?) \(").unwrap());
    let mut out: Vec<Made> = Vec::new();
    for (id, n, a) in calls(transcript) {
        if !matches!(
            n.as_str(),
            "files_write" | "make_pdf" | "make_docx" | "make_xlsx"
        ) {
            continue;
        }
        let Some(m) = ok_result(transcript, &id) else {
            continue;
        };
        let c = content_of(m);
        let re: &Regex = if n == "files_write" { &WRITTEN } else { &MADE };
        let path = re
            .captures(&c)
            .map(|x| x[1].to_string())
            .unwrap_or_else(|| {
                let o = sget(&a, "output");
                if o.is_empty() {
                    sget(&a, "path")
                } else {
                    o
                }
            });
        if path.is_empty() {
            continue;
        }
        let ext = ext_of(&path);
        let mut made = Made {
            path: path.clone(),
            ext: ext.clone(),
            ..Default::default()
        };
        if n == "files_write" {
            if !get_truthy(&a, "append") {
                made.markdown = sget(&a, "content");
            }
            if matches!(ext.as_str(), "md" | "txt" | "html" | "csv") {
                made.source = path.clone();
            }
        } else {
            made.markdown = sget(&a, "markdown");
            made.source = sget(&a, "source");
            made.sheets = a.get("sheets").cloned().unwrap_or(Value::Null);
        }
        out.retain(|x| x.path != made.path); // the last write of a path wins
        out.push(made);
    }
    out
}

/// The rows of a Markdown table in `text` (header first), or `None` when it has none.
pub fn markdown_table(text: &str) -> Option<Vec<Vec<String>>> {
    static SEP: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"^\|?\s*:?-{3,}:?\s*(\|\s*:?-{3,}:?\s*)*\|?\s*$").unwrap());
    let lines: Vec<&str> = text.lines().map(str::trim).collect();
    let i = lines
        .iter()
        .position(|l| SEP.is_match(l) && l.contains('-'))?;
    if i == 0 || !lines[i - 1].contains('|') {
        return None;
    }
    let cells = |l: &str| -> Vec<String> {
        l.trim_matches('|')
            .split('|')
            .map(|c| c.trim().to_string())
            .collect()
    };
    let mut rows = vec![cells(lines[i - 1])];
    rows.extend(
        lines[i + 1..]
            .iter()
            .take_while(|l| l.contains('|'))
            .map(|l| cells(l)),
    );
    Some(rows)
}

/// The rows of a CSV text (plain commas, quoted cells kept whole).
fn csv_rows(text: &str) -> Vec<Vec<String>> {
    text.lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| {
            let (mut cells, mut cur, mut quoted) = (Vec::new(), String::new(), false);
            for ch in l.chars() {
                match ch {
                    '"' => quoted = !quoted,
                    ',' if !quoted => cells.push(std::mem::take(&mut cur).trim().to_string()),
                    _ => cur.push(ch),
                }
            }
            cells.push(cur.trim().to_string());
            cells
        })
        .collect()
}

fn rows_markdown(rows: &[Vec<String>]) -> String {
    let Some(head) = rows.first() else {
        return String::new();
    };
    let line = |r: &Vec<String>| format!("| {} |", r.join(" | "));
    let mut out = vec![
        line(head),
        format!("|{}", " --- |".repeat(head.len().max(1))),
    ];
    out.extend(rows[1..].iter().map(line));
    out.join("\n")
}

/// An Excel call's `sheets` as Markdown: each sheet's name as a heading and its table.
fn sheets_markdown(sheets: &Value) -> String {
    sheets
        .as_array()
        .into_iter()
        .flatten()
        .map(|s| {
            let mut rows: Vec<Vec<String>> = vec![s
                .get("columns")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .map(super::util::pystr_or_empty)
                .collect()];
            rows.extend(
                s.get("rows")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .map(|r| {
                        r.as_array()
                            .into_iter()
                            .flatten()
                            .map(super::util::pystr_or_empty)
                            .collect()
                    }),
            );
            let name = sget(s, "name");
            let table = rows_markdown(&rows);
            if name.is_empty() {
                table
            } else {
                format!("## {name}\n\n{table}")
            }
        })
        .collect::<Vec<_>>()
        .join("\n\n")
}

/// The Markdown a made file can be converted from: its text, its source file's text (`read` gives a workspace file's
/// text), or its workbook's tables.
fn markdown_of(m: &Made, read: &dyn Fn(&str) -> Option<String>) -> Option<String> {
    if !m.markdown.trim().is_empty() && m.ext != "csv" {
        return Some(m.markdown.clone());
    }
    if !m.source.is_empty() {
        if let Some(text) = read(&m.source) {
            return Some(if ext_of(&m.source) == "csv" {
                rows_markdown(&csv_rows(&text))
            } else {
                text
            });
        }
    }
    if m.sheets.is_array() {
        let md = sheets_markdown(&m.sheets);
        if !md.trim().is_empty() {
            return Some(md);
        }
    }
    None
}

/// The calls that make every format in `wanted` the run did not make — from the first made file it can be converted
/// from — each followed by `send_file` of the new file; ids `fmt_<n>`. `read` gives a workspace file's text.
pub fn conversion_calls(
    made: &[Made],
    wanted: &[String],
    read: &dyn Fn(&str) -> Option<String>,
) -> Vec<ToolCall> {
    let mut out: Vec<ToolCall> = Vec::new();
    let mut push = |name: &str, args: Value| {
        out.push(ToolCall {
            id: format!("fmt_{}", out.len() + 1),
            name: name.into(),
            args,
        });
    };
    for f in wanted {
        if f == "none" || made.iter().any(|m| &m.ext == f) {
            continue;
        }
        let Some((base, md)) = made
            .iter()
            .find_map(|m| markdown_of(m, read).map(|md| (m, md)))
        else {
            continue;
        };
        let output = format!("{}.{f}", stem_of(&base.path));
        let made_one = match f.as_str() {
            "pdf" => {
                let args = if matches!(ext_of(&base.source).as_str(), "md" | "txt" | "html") {
                    json!({"source": base.source, "output": output})
                } else {
                    json!({"markdown": md, "output": output})
                };
                push("make_pdf", args);
                true
            }
            "docx" => {
                let args = if ext_of(&base.source) == "md" {
                    json!({"source": base.source, "output": output})
                } else {
                    json!({"markdown": md, "output": output})
                };
                push("make_docx", args);
                true
            }
            "md" => {
                push("files_write", json!({"path": output, "content": md}));
                true
            }
            "xlsx" => match markdown_table(&md) {
                Some(rows) if rows.len() > 1 => {
                    let sheet = json!({"name": "Sheet1", "columns": rows[0], "rows": rows[1..]});
                    push("make_xlsx", json!({"output": output, "sheets": [sheet]}));
                    true
                }
                _ => false,
            },
            _ => false,
        };
        if made_one {
            push("send_file", json!({"path": output}));
        }
    }
    out
}

// ---------------------------------------------------------------- visa-free days

/// A destination's official visa page.
pub struct VisaPage {
    pub code: &'static str,
    pub country: &'static str,
    /// lowercase words that name the destination in a request
    pub words: &'static [&'static str],
    pub url: &'static str,
    pub host: &'static str,
}

/// The official visa pages of the common Asian destinations (the immigration department or the foreign ministry).
pub const VISA_PAGES: &[VisaPage] = &[
    VisaPage {
        code: "SG",
        country: "Singapore",
        words: &["singapore", "新加坡"],
        url: "https://www.ica.gov.sg/enter-transit-depart/entering-singapore/visa_requirements",
        host: "ica.gov.sg",
    },
    VisaPage {
        code: "TH",
        country: "Thailand",
        words: &[
            "thailand",
            "bangkok",
            "phuket",
            "chiang mai",
            "pattaya",
            "泰国",
            "泰國",
        ],
        url: "https://www.thaievisa.go.th/",
        host: "thaievisa.go.th",
    },
    VisaPage {
        code: "MY",
        country: "Malaysia",
        words: &[
            "malaysia",
            "kuala lumpur",
            "penang",
            "langkawi",
            "马来西亚",
            "馬來西亞",
        ],
        url: "https://www.imi.gov.my/index.php/en/main-services/visa/visa-requirement-by-country/",
        host: "imi.gov.my",
    },
    VisaPage {
        code: "ID",
        country: "Indonesia",
        words: &["indonesia", "bali", "jakarta", "印尼", "印度尼西亚"],
        url: "https://evisa.imigrasi.go.id/",
        host: "imigrasi.go.id",
    },
    VisaPage {
        code: "VN",
        country: "Vietnam",
        words: &[
            "vietnam",
            "viet nam",
            "hanoi",
            "ho chi minh",
            "saigon",
            "da nang",
            "越南",
        ],
        url: "https://evisa.gov.vn/",
        host: "evisa.gov.vn",
    },
    VisaPage {
        code: "KR",
        country: "South Korea",
        words: &["korea", "seoul", "busan", "jeju", "韩国", "韓國"],
        url: "https://www.k-eta.go.kr/",
        host: "k-eta.go.kr",
    },
    VisaPage {
        code: "JP",
        country: "Japan",
        words: &["japan", "tokyo", "osaka", "kyoto", "sapporo", "日本"],
        url: "https://www.mofa.go.jp/j_info/visit/visa/short/novisa.html",
        host: "mofa.go.jp",
    },
    VisaPage {
        code: "TW",
        country: "Taiwan",
        words: &["taiwan", "taipei", "kaohsiung", "台湾", "台灣"],
        url: "https://www.boca.gov.tw/sp-foof-countrylp-03-2.html",
        host: "boca.gov.tw",
    },
    VisaPage {
        code: "CN",
        country: "China",
        words: &[
            "china",
            "beijing",
            "shanghai",
            "shenzhen",
            "guangzhou",
            "chengdu",
            "中国",
            "中國",
        ],
        url: "https://en.nia.gov.cn/",
        host: "nia.gov.cn",
    },
    VisaPage {
        code: "PH",
        country: "Philippines",
        words: &[
            "philippines",
            "manila",
            "cebu",
            "boracay",
            "菲律宾",
            "菲律賓",
        ],
        url: "https://dfa.gov.ph/list-of-countries-for-30-day-visa-free-entry",
        host: "dfa.gov.ph",
    },
    VisaPage {
        code: "KH",
        country: "Cambodia",
        words: &["cambodia", "phnom penh", "siem reap", "柬埔寨"],
        url: "https://www.evisa.gov.kh/",
        host: "evisa.gov.kh",
    },
];

/// Visa-free stays from the destinations' official tables, for when no page the run read states the days: (destination
/// code, the nationalities by the words a profile uses, days, as of, the official source). Only entries checked against
/// the source go here.
pub const VISA_FREE: &[(&str, &[&str], u32, &str, &str)] = &[(
    "TH",
    // Thailand's 2026 revision: the 30-day visa exemption group (MFA summary, 22 May 2026; in force 15 September 2026)
    &[
        "australian", "australia", "american", "united states", "usa", "british", "united kingdom", "uk", "canadian",
        "canada", "japanese", "japan", "singaporean", "singapore", "malaysian", "malaysia", "filipino", "philippines",
        "german", "germany", "french", "france",
    ],
    30,
    "15 September 2026",
    "https://image.mfa.go.th/mfa/0/wdW3FTtVMc/2026-05-22/%E0%B8%95%E0%B8%B2%E0%B8%A3%E0%B8%B2%E0%B8%87%E0%B8%97%E0%B8%9A%E0%B8%97%E0%B8%A7%E0%B8%99%E0%B8%A1%E0%B8%B2%E0%B8%95%E0%B8%A3%E0%B8%81%E0%B8%B2%E0%B8%A3_ver._eng.pdf",
)];

/// The table's visa-free stay for `nationality` in the destination `code`: (days, as of, source).
pub fn visa_free_from_table(
    code: &str,
    nationality: &str,
) -> Option<(u32, &'static str, &'static str)> {
    let nat = nationality.trim().to_lowercase();
    if nat.is_empty() {
        return None;
    }
    VISA_FREE
        .iter()
        .find(|(c, nats, ..)| *c == code && nats.contains(&nat.as_str()))
        .map(|(_, _, d, asof, src)| (*d, *asof, *src))
}

/// The line the answer gets for a destination: its days with the official page, or the page alone.
pub fn visa_line(page: &VisaPage, days: Option<u32>, nationality: &str, zh: bool) -> String {
    let who = nationality.trim();
    match (days, zh) {
        (Some(d), false) if !who.is_empty() => format!(
            "Visa-free stay in {} with your {who} passport: up to {d} days (official source: {}).",
            page.country, page.url
        ),
        (Some(d), false) => format!(
            "Visa-free stay in {}: up to {d} days (official source: {}).",
            page.country, page.url
        ),
        (Some(d), true) => format!(
            "{} 免签停留：最多 {d} 天（官方来源：{}）。",
            page.country, page.url
        ),
        (None, false) => format!(
            "How many days the visa-free stay in {} allows for your passport is on the official page: {}",
            page.country, page.url
        ),
        (None, true) => format!(
            "{} 的免签停留天数请以官方页面为准：{}",
            page.country, page.url
        ),
    }
}

// ---------------------------------------------------------------- a page just opened

pub const PAGE_KIND_CHOICES: [&str; 3] = ["error_page", "normal_page", "login_page"];
pub const SHOP_PAGE_CHOICES: [&str; 4] = ["product_page", "listing_page", "cart_page", "other"];

/// Whether `asked` is a deep link (a path or a query beyond the home page) that landed somewhere else (`landed`).
pub fn deep_link_redirect(asked: &str, landed: &str) -> bool {
    let (Ok(a), Ok(b)) = (urlparse(asked), urlparse(landed)) else {
        return false;
    };
    !a.netloc.is_empty()
        && !b.netloc.is_empty()
        && (!a.path.trim_matches('/').is_empty() || !a.query.is_empty())
        && asked.trim_end_matches('/') != landed.trim_end_matches('/')
}

/// `scheme://host/` of a URL.
pub fn site_root(url: &str) -> Option<String> {
    let p = urlparse(url).ok()?;
    (!p.netloc.is_empty() && p.scheme.starts_with("http"))
        .then(|| format!("{}://{}/", p.scheme, p.netloc))
}

/// What a decision about a page reads: its title, its URL and the first lines of its snapshot.
pub fn page_context(title: &str, url: &str, snapshot: &str) -> String {
    let first: Vec<&str> = snapshot
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .take(15)
        .collect();
    format!(
        "Title: {}\nURL: {}\nFirst lines of the page:\n{}",
        take_chars(title, 200),
        take_chars(url, 300),
        take_chars(&first.join("\n"), 2000)
    )
}

/// Whether the run loaded the online-shopping skill.
pub fn shopping_run(transcript: &[Value]) -> bool {
    calls(transcript)
        .iter()
        .any(|(_, n, a)| n == "load_skill" && sget(a, "name") == "online-shopping")
}

/// The ref of the page's own Add-to-Cart button (`[e12] button "Add to Cart"`), read from the snapshot's elements.
pub fn add_to_cart_ref(snapshot: &str) -> Option<String> {
    static BUTTON: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r#"\[(e\d+)\] button "([^"]*)""#).unwrap());
    static CART: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(
            r"(?i)^\s*add to (cart|bag|basket|trolley)\b|加入购物车|加入購物車|カートに入れる",
        )
        .unwrap()
    });
    BUTTON
        .captures_iter(snapshot)
        .find(|c| CART.is_match(&c[2]))
        .map(|c| c[1].to_string())
}

fn same_page(a: &str, b: &str) -> bool {
    let norm = |u: &str| {
        u.split('#')
            .next()
            .unwrap_or("")
            .trim_end_matches('/')
            .to_lowercase()
    };
    !a.is_empty() && norm(a) == norm(b)
}

/// Whether the agent comes back to `url` — a page it opened before and left — without having clicked `cart_ref` (the
/// page's Add-to-Cart button) since it first opened it.
pub fn back_on_uncarted_page(transcript: &[Value], url: &str, cart_ref: &str) -> bool {
    let cs = calls(transcript);
    let Some(first) = cs.iter().position(|(id, n, a)| {
        n == "browser_navigate"
            && same_page(&sget(a, "url"), url)
            && ok_result(transcript, id).is_some()
    }) else {
        return false;
    };
    let left = cs[first + 1..].iter().any(|(id, n, a)| {
        n == "browser_navigate"
            && !same_page(&sget(a, "url"), url)
            && ok_result(transcript, id).is_some()
    });
    let carted = cs[first + 1..]
        .iter()
        .any(|(_, n, a)| n == "browser_click" && sget(a, "ref") == cart_ref);
    left && !carted
}

/// A page just opened that one judgement looks at.
#[derive(Debug, Clone, PartialEq)]
pub enum PageCheck {
    /// a deep link redirected to `landed`: an error page sends code to the site's home page
    DeepLink { landed: String, context: String },
    /// a shop page the agent came back to without adding to the cart: a product page gets its `cart_ref` clicked
    Cart {
        url: String,
        context: String,
        cart_ref: String,
    },
}

/// The judgement a just-opened page needs (`asked` the link opened, `landed` where it ended, its title and snapshot),
/// from code alone: a redirected deep link, or — in a shopping run — a page with an Add-to-Cart button the agent comes
/// back to without having pressed it.
pub fn page_check_for(
    transcript: &[Value],
    asked: &str,
    landed: &str,
    title: &str,
    snapshot: &str,
) -> Option<PageCheck> {
    if deep_link_redirect(asked, landed) && site_root(landed).is_some() {
        return Some(PageCheck::DeepLink {
            landed: landed.to_string(),
            context: page_context(title, landed, snapshot),
        });
    }
    if !shopping_run(transcript) {
        return None;
    }
    let cart_ref = add_to_cart_ref(snapshot)?;
    back_on_uncarted_page(transcript, asked, &cart_ref).then(|| PageCheck::Cart {
        url: asked.to_string(),
        context: page_context(title, landed, snapshot),
        cart_ref,
    })
}

/// The file formats the run's file calls say the user asked to receive (`deliver_as` of `files_write`, `make_pdf`,
/// `make_docx`, `make_xlsx`), in order, once each.
pub fn wanted_formats(transcript: &[Value]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for (_, n, a) in calls(transcript) {
        if !matches!(
            n.as_str(),
            "files_write" | "make_pdf" | "make_docx" | "make_xlsx"
        ) {
            continue;
        }
        for f in super::guards::deliver_as(&a) {
            if !out.contains(&f) {
                out.push(f);
            }
        }
    }
    out
}

impl super::Runtime {
    /// The call code makes for a page just opened, after its decision (once per page and kind): the site's home page
    /// for an error page a deep link landed on, the page's own Add-to-Cart click for a product page.
    pub(crate) async fn page_check_call(
        &self,
        task_id: &str,
        call_id: &str,
        pc: PageCheck,
    ) -> Option<ToolCall> {
        let key = match &pc {
            PageCheck::DeepLink { landed, .. } => format!("{task_id}|deep|{}", site_root(landed)?),
            PageCheck::Cart { url, .. } => format!("{task_id}|cart|{url}"),
        };
        if !self.inner.lock().page_checked.insert(key) {
            return None;
        }
        match pc {
            PageCheck::DeepLink { landed, context } => {
                let kind = self
                    .decide(
                        task_id,
                        "A link to a page deep inside a site was opened and the browser was sent elsewhere. What kind of page is this?",
                        &context,
                        &PAGE_KIND_CHOICES,
                        "normal_page",
                    )
                    .await;
                if kind != "error_page" {
                    return None;
                }
                let root = site_root(&landed)?;
                self.event(
                    task_id,
                    "deep_link_error",
                    json!({"landed": landed, "home": root}),
                )
                .await;
                Some(ToolCall {
                    id: format!("{call_id}_home"),
                    name: "browser_navigate".into(),
                    args: json!({"url": root}),
                })
            }
            PageCheck::Cart {
                url,
                context,
                cart_ref,
            } => {
                let kind = self
                    .decide(
                        task_id,
                        "What kind of shop page is this?",
                        &context,
                        &SHOP_PAGE_CHOICES,
                        "other",
                    )
                    .await;
                if kind != "product_page" {
                    return None;
                }
                self.event(task_id, "add_to_cart", json!({"url": url, "ref": cart_ref}))
                    .await;
                Some(ToolCall {
                    id: format!("{call_id}_cart"),
                    name: "browser_click".into(),
                    args: json!({"ref": cart_ref}),
                })
            }
        }
    }

    /// The conversions the formats the user asked for need (`conversion_calls`): the formats come from the file calls'
    /// `deliver_as` (the model's JSON, from the user's words), the files from the run's results — no decision on words.
    pub(crate) async fn format_calls(&self, task_id: &str, transcript: &[Value]) -> Vec<ToolCall> {
        let made = made_files(transcript);
        let wanted = wanted_formats(transcript);
        if made.is_empty() || wanted.is_empty() {
            return Vec::new();
        }
        let read = |p: &str| -> Option<String> {
            let path = self.workspace.join(p);
            std::fs::read_to_string(path).ok()
        };
        let out = conversion_calls(&made, &wanted, &read);
        if !out.is_empty() {
            self.event(
                task_id,
                "format_conversion",
                json!({"wanted": wanted, "calls": out.iter().map(|c| json!({"name": c.name, "args": super::guards::preview_args(&c.args)})).collect::<Vec<_>>()}),
            )
            .await;
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn step(id: &str, name: &str, args: Value, result: &str) -> Vec<Value> {
        vec![
            json!({"role": "assistant", "content": "", "tool_calls": [{"id": id, "type": "function",
                   "function": {"name": name, "arguments": args.to_string()}}]}),
            json!({"role": "tool", "tool_call_id": id, "content": result}),
        ]
    }

    #[test]
    fn a_missing_format_is_converted_from_what_the_run_made() {
        let mut t = step(
            "c1",
            "files_write",
            json!({"path": "reports/trip.md", "content": "# Trip\n\n| Day | City |\n| --- | --- |\n| 1 | Tokyo |\n| 2 | Kyoto |"}),
            "已写入 written: reports/trip.md (80 bytes; 9 words).",
        );
        let made = made_files(&t);
        assert_eq!(made.len(), 1);
        assert_eq!(
            (made[0].ext.as_str(), made[0].source.as_str()),
            ("md", "reports/trip.md")
        );
        let none = |_: &str| None;
        let want = |w: &[&str]| w.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        let c = conversion_calls(&made, &want(&["pdf", "docx", "xlsx", "md"]), &none);
        let names: Vec<(&str, String)> = c
            .iter()
            .map(|x| {
                (
                    x.name.as_str(),
                    sget(&x.args, "output") + &sget(&x.args, "path"),
                )
            })
            .collect();
        assert_eq!(
            names,
            [
                ("make_pdf", "reports/trip.pdf".to_string()),
                ("send_file", "reports/trip.pdf".to_string()),
                ("make_docx", "reports/trip.docx".to_string()),
                ("send_file", "reports/trip.docx".to_string()),
                ("make_xlsx", "reports/trip.xlsx".to_string()),
                ("send_file", "reports/trip.xlsx".to_string()),
            ]
        );
        assert_eq!(sget(&c[0].args, "source"), "reports/trip.md");
        assert_eq!(c[4].args["sheets"][0]["columns"], json!(["Day", "City"]));
        assert_eq!(c[4].args["sheets"][0]["rows"][1], json!(["2", "Kyoto"]));
        assert_eq!(c[0].id, "fmt_1");
        // nothing asked, or all made: no call
        assert!(conversion_calls(&made, &want(&["none"]), &none).is_empty());
        assert!(conversion_calls(&made, &want(&["md"]), &none).is_empty());
        // an Excel workbook made from sheets → a PDF of its table
        t = step(
            "c1",
            "make_xlsx",
            json!({"output": "data/prices.xlsx", "sheets": [{"name": "Prices", "columns": ["Item", "SGD"], "rows": [["Tea", 3]]}]}),
            "Excel 已在本机生成 created locally: data/prices.xlsx (6 KB)",
        );
        let c = conversion_calls(&made_files(&t), &want(&["pdf"]), &none);
        assert_eq!(c[0].name, "make_pdf");
        assert_eq!(sget(&c[0].args, "output"), "data/prices.pdf");
        let md = sget(&c[0].args, "markdown");
        assert!(
            md.contains("## Prices") && md.contains("| Tea | 3 |"),
            "{md}"
        );
        // a CSV source is read from the workspace
        t = step(
            "c1",
            "make_xlsx",
            json!({"output": "data/p.xlsx", "source": "data/p.csv"}),
            "created locally: data/p.xlsx (6 KB)",
        );
        let read =
            |p: &str| (p == "data/p.csv").then(|| "Item,SGD\n\"Tea, green\",3\n".to_string());
        let c = conversion_calls(&made_files(&t), &want(&["docx"]), &read);
        assert!(sget(&c[0].args, "markdown").contains("| Tea, green | 3 |"));
        // a failed file is not a source
        let bad = step(
            "c1",
            "make_pdf",
            json!({"markdown": "x"}),
            "ERROR: PDF export failed",
        );
        assert!(made_files(&bad).is_empty());
    }

    #[test]
    fn a_deep_link_redirect_and_the_cart_button_are_read_by_code() {
        assert!(deep_link_redirect(
            "https://www.airline.test/manage/booking?pnr=ABC123",
            "https://www.airline.test/error?code=500"
        ));
        assert!(!deep_link_redirect(
            "https://www.airline.test/",
            "https://www.airline.test/home"
        ));
        assert!(!deep_link_redirect("https://a.test/x", "https://a.test/x/"));
        assert_eq!(
            site_root("https://www.airline.test/manage/booking?pnr=1").as_deref(),
            Some("https://www.airline.test/")
        );
        let snap = "[e3] link \"Add to cart offers\"\n  [e7] button \"Add to Wishlist\"\n  [e9] button \"Add to Cart\"";
        assert_eq!(add_to_cart_ref(snap).as_deref(), Some("e9"));
        assert_eq!(add_to_cart_ref("[e1] button \"Buy now\""), None);
        let p = "https://shop.test/p/case-1";
        let mut t = step(
            "n1",
            "browser_navigate",
            json!({"url": p}),
            "{\"url\": \"x\"}",
        );
        assert!(!back_on_uncarted_page(&t, p, "e9")); // never left
        t.extend(step(
            "n2",
            "browser_navigate",
            json!({"url": "https://shop.test/search?q=case"}),
            "{}",
        ));
        assert!(back_on_uncarted_page(&t, p, "e9"));
        t.extend(step("c1", "browser_click", json!({"ref": "e9"}), "clicked"));
        assert!(!back_on_uncarted_page(&t, p, "e9"));
    }
}
