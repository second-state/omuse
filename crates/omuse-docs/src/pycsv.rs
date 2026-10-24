//! A port of the parts of Python's `csv` module the document code relies on: the reader state machine
//! (so blank lines, embedded newlines and quotes behave exactly as in Python), the `QUOTE_MINIMAL` writer and
//! `csv.Sniffer` (delimiter / quote detection used by `dataq.load`). Private to the crate.

use std::sync::LazyLock;

use fancy_regex::Regex as FRegex;

/// The dialect fields Python's reader/writer look at (`escapechar` is never used by the callers).
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Dialect {
    pub delimiter: char,
    pub quotechar: char,
    pub doublequote: bool,
    pub skipinitialspace: bool,
}

impl Dialect {
    /// `csv.excel`.
    pub fn excel() -> Self {
        Dialect {
            delimiter: ',',
            quotechar: '"',
            doublequote: true,
            skipinitialspace: false,
        }
    }

    /// `csv.excel_tab`.
    pub fn excel_tab() -> Self {
        Dialect {
            delimiter: '\t',
            ..Dialect::excel()
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum State {
    StartRecord,
    StartField,
    InField,
    InQuotedField,
    QuoteInQuotedField,
    EatCrnl,
}

/// `list(csv.reader(io.StringIO(text), dialect))`: lines are split on `\n` only (StringIO's default);
/// `\r` is handled by the state machine like CPython's `_csv`.
pub(crate) fn read_all(text: &str, d: &Dialect) -> Vec<Vec<String>> {
    let mut records = Vec::new();
    let mut lines = text.split_inclusive('\n').peekable();
    loop {
        let mut fields: Vec<String> = Vec::new();
        let mut field = String::new();
        let mut state = State::StartRecord;
        let mut got_record = false;
        for line in lines.by_ref() {
            for c in line.chars() {
                process(Some(c), d, &mut state, &mut field, &mut fields);
            }
            process(None, d, &mut state, &mut field, &mut fields);
            if state == State::StartRecord {
                got_record = true;
                break;
            }
        }
        if !got_record {
            // end of input: an unterminated field / quoted field still counts (non-strict)
            if !field.is_empty() || state == State::InQuotedField {
                fields.push(std::mem::take(&mut field));
            }
            if fields.is_empty() {
                break;
            }
            records.push(fields);
            break;
        }
        records.push(fields);
        if lines.peek().is_none() {
            break;
        }
    }
    records
}

fn process(
    c: Option<char>,
    d: &Dialect,
    state: &mut State,
    field: &mut String,
    fields: &mut Vec<String>,
) {
    let save = |field: &mut String, fields: &mut Vec<String>| fields.push(std::mem::take(field));
    let is_nl = |c: Option<char>| matches!(c, Some('\n') | Some('\r'));
    if *state == State::StartRecord {
        if c.is_none() {
            return; // empty line: an empty record
        }
        if is_nl(c) {
            *state = State::EatCrnl;
            return;
        }
        *state = State::StartField;
    }
    match *state {
        State::StartField => match c {
            None => {
                save(field, fields);
                *state = State::StartRecord;
            }
            Some(ch) if ch == '\n' || ch == '\r' => {
                save(field, fields);
                *state = State::EatCrnl;
            }
            Some(ch) if ch == d.quotechar => *state = State::InQuotedField,
            Some(' ') if d.skipinitialspace => {}
            Some(ch) if ch == d.delimiter => save(field, fields),
            Some(ch) => {
                field.push(ch);
                *state = State::InField;
            }
        },
        State::InField => match c {
            None => {
                save(field, fields);
                *state = State::StartRecord;
            }
            Some(ch) if ch == '\n' || ch == '\r' => {
                save(field, fields);
                *state = State::EatCrnl;
            }
            Some(ch) if ch == d.delimiter => {
                save(field, fields);
                *state = State::StartField;
            }
            Some(ch) => field.push(ch),
        },
        State::InQuotedField => match c {
            None => {}
            Some(ch) if ch == d.quotechar => {
                *state = if d.doublequote {
                    State::QuoteInQuotedField
                } else {
                    State::InField
                };
            }
            Some(ch) => field.push(ch),
        },
        State::QuoteInQuotedField => match c {
            Some(ch) if ch == d.quotechar => {
                field.push(ch);
                *state = State::InQuotedField;
            }
            Some(ch) if ch == d.delimiter => {
                save(field, fields);
                *state = State::StartField;
            }
            None => {
                save(field, fields);
                *state = State::StartRecord;
            }
            Some(ch) if ch == '\n' || ch == '\r' => {
                save(field, fields);
                *state = State::EatCrnl;
            }
            Some(ch) => {
                field.push(ch);
                *state = State::InField;
            }
        },
        State::EatCrnl => match c {
            Some('\n') | Some('\r') => {}
            None => *state = State::StartRecord,
            Some(ch) => {
                // CPython raises here ("new-line character seen in unquoted field"); the callers never
                // hit it because lines are split on \n, so a stray \r simply starts a new field.
                field.push(ch);
                *state = State::InField;
            }
        },
        State::StartRecord => {}
    }
}

/// `csv.writer(f).writerow(fields)` with the excel dialect (`QUOTE_MINIMAL`, `\r\n` line ends).
pub(crate) fn write_row(out: &mut String, fields: &[String]) {
    let mut first = true;
    for f in fields {
        if !first {
            out.push(',');
        }
        first = false;
        let needs = f.contains(',')
            || f.contains('"')
            || f.contains('\r')
            || f.contains('\n')
            || (f.is_empty() && fields.len() == 1);
        if needs {
            out.push('"');
            out.push_str(&f.replace('"', "\"\""));
            out.push('"');
        } else {
            out.push_str(f);
        }
    }
    out.push_str("\r\n");
}

static QUOTE_PATTERNS: LazyLock<Vec<(FRegex, bool, bool)>> = LazyLock::new(|| {
    // (pattern, has delim group, has space group)
    vec![
        (
            FRegex::new(
                r#"(?sm)(?P<delim>[^\w\n"'])(?P<space> ?)(?P<quote>["']).*?\k<quote>\k<delim>"#,
            )
            .unwrap(),
            true,
            true,
        ),
        (
            FRegex::new(
                r#"(?sm)(?:^|\n)(?P<quote>["']).*?\k<quote>(?P<delim>[^\w\n"'])(?P<space> ?)"#,
            )
            .unwrap(),
            true,
            true,
        ),
        (
            FRegex::new(
                r#"(?sm)(?P<delim>[^\w\n"'])(?P<space> ?)(?P<quote>["']).*?\k<quote>(?:$|\n)"#,
            )
            .unwrap(),
            true,
            true,
        ),
        (
            FRegex::new(r#"(?sm)(?:^|\n)(?P<quote>["']).*?\k<quote>(?:$|\n)"#).unwrap(),
            false,
            false,
        ),
    ]
});

/// First key with the highest count (Python `max(d, key=d.get)` over an insertion-ordered dict).
fn arg_max(items: &[(char, usize)]) -> Option<char> {
    let mut best: Option<(char, usize)> = None;
    for &(k, n) in items {
        if best.map(|(_, b)| n > b).unwrap_or(true) {
            best = Some((k, n));
        }
    }
    best.map(|(k, _)| k)
}

fn bump(items: &mut Vec<(char, usize)>, k: char) {
    if let Some(e) = items.iter_mut().find(|e| e.0 == k) {
        e.1 += 1;
    } else {
        items.push((k, 1));
    }
}

/// (quote char, delimiter, had a space after the delimiter) of one quote-pattern match.
type QuoteMatch = (Option<char>, Option<char>, bool);

/// `csv.Sniffer._guess_quote_and_delimiter` → (quotechar, doublequote, delimiter, skipinitialspace).
fn guess_quote_and_delimiter(
    data: &str,
    delimiters: &str,
) -> (Option<char>, bool, Option<char>, bool) {
    let mut found: Option<(Vec<QuoteMatch>, bool)> = None;
    for (rx, has_delim, has_space) in QUOTE_PATTERNS.iter() {
        let mut matches = Vec::new();
        for caps in rx.captures_iter(data).flatten() {
            let quote = caps.name("quote").and_then(|m| m.as_str().chars().next());
            let delim = if *has_delim {
                caps.name("delim").and_then(|m| m.as_str().chars().next())
            } else {
                None
            };
            let space = *has_space
                && caps
                    .name("space")
                    .map(|m| !m.as_str().is_empty())
                    .unwrap_or(false);
            matches.push((quote, delim, space));
        }
        if !matches.is_empty() {
            found = Some((matches, *has_delim));
            break;
        }
    }
    let Some((matches, has_delim)) = found else {
        return (None, false, None, false);
    };
    let mut quotes: Vec<(char, usize)> = Vec::new();
    let mut delims: Vec<(char, usize)> = Vec::new();
    let mut spaces = 0usize;
    for (q, d, sp) in &matches {
        if let Some(q) = q {
            bump(&mut quotes, *q);
        }
        if !has_delim {
            continue;
        }
        if let Some(d) = d {
            if delimiters.contains(*d) {
                bump(&mut delims, *d);
            }
        }
        if *sp {
            spaces += 1;
        }
    }
    let quotechar = arg_max(&quotes);
    let (delim, skipinitialspace) = match arg_max(&delims) {
        Some(d) => {
            let n = delims.iter().find(|e| e.0 == d).map(|e| e.1).unwrap_or(0);
            (if d == '\n' { None } else { Some(d) }, n == spaces)
        }
        None => (None, false),
    };
    // an extra quote between delimiters means a double-quoted format
    let dq = {
        let de = delim
            .map(|d| regex::escape(&d.to_string()))
            .unwrap_or_default();
        let q = quotechar
            .map(|q| regex::escape(&q.to_string()))
            .unwrap_or_default();
        let pat = format!(r"(?m)(({de})|^)\W*{q}[^{de}\n]*{q}[^{de}\n]*{q}\W*(({de})|$)");
        FRegex::new(&pat)
            .ok()
            .and_then(|r| r.is_match(data).ok())
            .unwrap_or(false)
    };
    (quotechar, dq, delim, skipinitialspace)
}

/// `csv.Sniffer._guess_delimiter` → (delimiter, skipinitialspace).
fn guess_delimiter(data: &str, delimiters: &str) -> (Option<char>, bool) {
    let lines: Vec<&str> = data.split('\n').filter(|l| !l.is_empty()).collect();
    let ascii: Vec<char> = (0u8..127).map(|c| c as char).collect();
    let chunk = lines.len().min(10);
    let mut iteration = 0usize;
    // char → [(freq, count)] in insertion order
    let mut char_freq: Vec<(char, Vec<(usize, usize)>)> = Vec::new();
    let mut modes: Vec<(char, (usize, usize))> = Vec::new();
    let mut delims: Vec<(char, (usize, usize))> = Vec::new();
    let (mut start, mut end) = (0usize, chunk);
    let count = |line: &str, c: char| line.chars().filter(|x| *x == c).count();
    let skip_space = |d: char| {
        let l0 = lines.first().copied().unwrap_or("");
        count(l0, d) == l0.matches(&format!("{d} ")).count()
    };
    while start < lines.len() {
        iteration += 1;
        for line in &lines[start..end.min(lines.len())] {
            for &ch in &ascii {
                let freq = count(line, ch);
                let entry = match char_freq.iter_mut().find(|e| e.0 == ch) {
                    Some(e) => e,
                    None => {
                        char_freq.push((ch, Vec::new()));
                        char_freq.last_mut().unwrap()
                    }
                };
                match entry.1.iter_mut().find(|e| e.0 == freq) {
                    Some(e) => e.1 += 1,
                    None => entry.1.push((freq, 1)),
                }
            }
        }
        for (ch, items) in &char_freq {
            let mut items = items.clone();
            if items.len() == 1 && items[0].0 == 0 {
                continue;
            }
            let mode = if items.len() > 1 {
                let mut best = items[0];
                for it in &items {
                    if it.1 > best.1 {
                        best = *it;
                    }
                }
                if let Some(p) = items.iter().position(|x| *x == best) {
                    items.remove(p);
                }
                let rest: usize = items.iter().map(|x| x.1).sum();
                (best.0, best.1.wrapping_sub(rest))
            } else {
                items[0]
            };
            match modes.iter_mut().find(|e| e.0 == *ch) {
                Some(e) => e.1 = mode,
                None => modes.push((*ch, mode)),
            }
        }
        let total = (chunk * iteration).min(lines.len()) as f64;
        let mut consistency = 1.0f64;
        let threshold = 0.9;
        while delims.is_empty() && consistency >= threshold {
            for (k, v) in &modes {
                // (v[1] may have wrapped below zero in Python: a negative count never passes the test)
                if v.0 > 0
                    && (v.1 as isize) > 0
                    && (v.1 as f64 / total) >= consistency
                    && delimiters.contains(*k)
                {
                    match delims.iter_mut().find(|e| e.0 == *k) {
                        Some(e) => e.1 = *v,
                        None => delims.push((*k, *v)),
                    }
                }
            }
            consistency -= 0.01;
        }
        if delims.len() == 1 {
            let d = delims[0].0;
            return (Some(d), skip_space(d));
        }
        start = end;
        end += chunk;
    }
    if delims.is_empty() {
        return (None, false);
    }
    if delims.len() > 1 {
        for d in [',', '\t', ';', ' ', ':'] {
            if delims.iter().any(|e| e.0 == d) {
                return (Some(d), skip_space(d));
            }
        }
    }
    // pick the character that dominates: sort by (freq, count, char) and take the last
    let mut items: Vec<((usize, usize), char)> = delims.iter().map(|(k, v)| (*v, *k)).collect();
    items.sort();
    let d = items.last().map(|x| x.1).unwrap();
    (Some(d), skip_space(d))
}

/// `csv.Sniffer().sniff(sample, delimiters)`; `None` when Python raised `csv.Error`.
pub(crate) fn sniff(sample: &str, delimiters: &str) -> Option<Dialect> {
    let (quotechar, doublequote, mut delimiter, mut skipinitialspace) =
        guess_quote_and_delimiter(sample, delimiters);
    if delimiter.is_none() {
        let (d, s) = guess_delimiter(sample, delimiters);
        delimiter = d;
        skipinitialspace = s;
    }
    let delimiter = delimiter?;
    Some(Dialect {
        delimiter,
        quotechar: quotechar.unwrap_or('"'),
        doublequote,
        skipinitialspace,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reader_like_python() {
        let d = Dialect::excel();
        assert_eq!(
            read_all("a,b\n\"x, y\",2\n", &d),
            vec![vec!["a", "b"], vec!["x, y", "2"]]
        );
        assert_eq!(
            read_all("a,b\n\nc,d", &d),
            vec![vec!["a", "b"], vec![], vec!["c", "d"]]
        );
        assert_eq!(
            read_all("\"multi\nline\",1\r\n", &d),
            vec![vec!["multi\nline", "1"]]
        );
        assert_eq!(
            read_all("a,\"he said \"\"hi\"\"\"\n", &d),
            vec![vec!["a", "he said \"hi\""]]
        );
        assert_eq!(read_all("", &d), Vec::<Vec<String>>::new());
        assert_eq!(read_all("x", &d), vec![vec!["x"]]);
    }

    #[test]
    fn writer_like_python() {
        let mut out = String::new();
        write_row(
            &mut out,
            &["a".into(), "x, y".into(), "q\"q".into(), "".into()],
        );
        assert_eq!(out, "a,\"x, y\",\"q\"\"q\",\r\n");
        let mut out = String::new();
        write_row(&mut out, &["".into()]);
        assert_eq!(out, "\"\"\r\n");
    }

    #[test]
    fn sniffer_finds_delimiters() {
        let d = sniff(
            "姓名,年龄,金额(新元)\nA,25,\"1,200\"\nB,35,S$800\n",
            ",\t;|",
        )
        .unwrap();
        assert_eq!(d.delimiter, ',');
        assert_eq!(d.quotechar, '"');
        let d = sniff("a;b;c\n1;2;3\n4;5;6\n", ",\t;|").unwrap();
        assert_eq!(d.delimiter, ';');
        let d = sniff("a\tb\n1\t2\n", ",\t;|").unwrap();
        assert_eq!(d.delimiter, '\t');
        assert!(sniff("just one column\nno delimiters here\n", ",\t;|").is_none());
    }
}
