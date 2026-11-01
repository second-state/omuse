//! The train-times machine behind the workflow tool `train_times` (docs/json-tools.md): the departures of a train the
//! user asks about, read by code from the operator's own timetable page — a known site whose fixed parts code reads.
//! Today: the Narita Express from Narita Airport Terminal 1 towards Tokyo, from JR East's station timetable
//! (travel_info_real 2026-11-01, twice: the model ran ten web searches for the times and never read the timetable, so
//! the answer gave no departure at all).
//!
//! The model passes the line, the date and the time the user reaches the station as JSON; code picks the weekday or the
//! Saturdays-and-holidays table (Japan's national holidays in a fixed table), opens the page (`browser_navigate`, prefix
//! `trn_`), reads each departure by its HTML elements (`browser_read_elements`: the timetable's `div.timetable_time`
//! with its train mark, its minute, its `data-misc` marks and its row's hour cell — docs/json-tools.md, "Reading
//! pages") and returns the next departures with `must_say`.

use chrono::{Datelike, NaiveDate, NaiveTime, Weekday};
use omuse_common::util::get_str;
use serde_json::{json, Map, Value};

use super::driver::Step;
use super::tools::fn_;
use super::util::{sget, stored_call, tool_calls};
use super::workflow::{self, Checked};

/// The workflow tool.
pub const TRAIN_TIMES: &str = "train_times";
/// The machine's tools.
pub const TOOLS: [&str; 1] = [TRAIN_TIMES];
/// The id prefix of the machine's code calls.
pub const PREFIX: &str = "trn_";
/// How many departures the result lists.
const SHOWN: usize = 4;

/// One line the machine knows: its station timetable on the operator's site.
struct Line {
    id: &'static str,
    name: &'static str,
    station: &'static str,
    towards: &'static str,
    operator: &'static str,
    /// the train mark of the line's departures (the `.train` element: "NEX")
    mark: &'static str,
    /// each departure on the timetable page (a CSS selector), with its row (`tr`) whose first cell is the hour
    departure_sel: &'static str,
    /// the timetable page for a month (`%y%m`) and a table (weekday / Saturdays and holidays)
    page: fn(&str, bool) -> String,
}

fn nex_page(yymm: &str, holiday: bool) -> String {
    format!(
        "https://timetables.jreast.co.jp/en/{yymm}/timetable/tt1130/{}.html",
        if holiday { "1130021" } else { "1130020" }
    )
}

const LINES: [Line; 1] = [Line {
    id: "narita_express",
    name: "Narita Express (N'EX)",
    station: "Narita Airport Terminal 1 Station",
    towards: "Tokyo Station",
    operator: "JR East",
    mark: "NEX",
    departure_sel: "table.result_03 tr[id^='time_'] div.timetable_time",
    page: nex_page,
}];

/// Japan's national holidays (and JR's year-end days on the holiday timetable), 2026–2027.
const HOLIDAYS: &[&str] = &[
    "2026-01-01",
    "2026-01-02",
    "2026-01-03",
    "2026-01-12",
    "2026-02-11",
    "2026-02-23",
    "2026-03-20",
    "2026-04-29",
    "2026-05-03",
    "2026-05-04",
    "2026-05-05",
    "2026-05-06",
    "2026-07-20",
    "2026-08-11",
    "2026-09-21",
    "2026-09-22",
    "2026-09-23",
    "2026-10-12",
    "2026-11-03",
    "2026-11-23",
    "2026-12-30",
    "2026-12-31",
    "2027-01-01",
    "2027-01-02",
    "2027-01-03",
    "2027-01-11",
    "2027-02-11",
    "2027-02-23",
    "2027-03-21",
    "2027-03-22",
    "2027-04-29",
    "2027-05-03",
    "2027-05-04",
    "2027-05-05",
    "2027-07-19",
    "2027-08-11",
    "2027-09-20",
    "2027-09-23",
    "2027-10-11",
    "2027-11-03",
    "2027-11-23",
];

/// Whether `d` runs on the Saturdays-and-holidays table.
pub fn holiday_table(d: NaiveDate) -> bool {
    matches!(d.weekday(), Weekday::Sat | Weekday::Sun)
        || HOLIDAYS.contains(&d.format("%Y-%m-%d").to_string().as_str())
}

/// The catalog entry of the workflow tool.
pub fn tool_schemas() -> Vec<Value> {
    vec![fn_(
        TRAIN_TIMES,
        "The departure times of a train from the operator's own timetable (today: the Narita Express from Narita Airport \
Terminal 1 Station to Tokyo Station, JR East). Code picks the weekday or the Saturdays-and-holidays table for the date, \
reads the timetable page and returns the next departures after the time given as JSON. Call it whenever the user asks \
when this train leaves; write the reply from the result, with every fact in its must_say.",
        json!({
            "line": {"type": "string", "enum": ["narita_express"]},
            "date": {"type": "string", "description": "YYYY-MM-DD, the day of the journey (resolve relative dates from today in the live state)"},
            "after": {"type": "string", "description": "HH:MM, the earliest time the user can be at the station (e.g. the landing time plus the time for immigration and baggage); empty = the whole day"}
        }),
        &["line", "date"],
    )]
}

/// The arguments checked against the line table and the calendar.
pub fn check(tool: &str, a: &Value, today: NaiveDate) -> Checked {
    if tool != TRAIN_TIMES {
        return Checked::Invalid(vec![format!("unknown tool {tool}")]);
    }
    let mut out = Map::new();
    let mut problems = Vec::new();
    let line = sget(a, "line");
    match LINES.iter().find(|l| l.id == line) {
        Some(l) => {
            out.insert("line".into(), json!(l.id));
        }
        None => problems.push(format!("line: one of {}", LINES.map(|l| l.id).join(", "))),
    }
    match NaiveDate::parse_from_str(sget(a, "date").trim(), "%Y-%m-%d") {
        Ok(d) if d < today => problems.push(format!("date: {d} is in the past (today is {today})")),
        Ok(d) => {
            out.insert("date".into(), json!(d.format("%Y-%m-%d").to_string()));
        }
        Err(_) => problems.push("date: YYYY-MM-DD".into()),
    }
    // a time that does not parse is dropped (the whole day is listed)
    if let Ok(t) = NaiveTime::parse_from_str(sget(a, "after").trim(), "%H:%M") {
        out.insert("after".into(), json!(t.format("%H:%M").to_string()));
    }
    if problems.is_empty() {
        Checked::Valid(Value::Object(out))
    } else {
        Checked::Invalid(problems)
    }
}

/// A departure on the timetable page.
#[derive(Debug, Clone, PartialEq)]
pub struct Departure {
    pub time: NaiveTime,
    /// false for a train marked ◆ ("does not operate every day")
    pub every_day: bool,
}

/// The name of the element read that holds the departures.
const DEPARTURES: &str = "departures";

/// The arguments of `browser_read_elements` for a line's departures: per departure its hour (the first cell of its
/// row), its train mark, its minute and its marks (`data-misc`: "◆" = not every day).
fn read_args(line: &Line) -> Value {
    json!({"read": [{"name": DEPARTURES, "sel": line.departure_sel, "max": 200,
        "fields": {"hour": {"closest": "tr", "sel": "td"}, "train": ".train", "minute": ".minute",
                   "misc": {"attr": "data-misc"}}}]})
}

/// The departures of `mark` trains among the elements read (each `{hour, train, minute, misc}`), in the page's
/// order. Trains listed twice (a train that splits) count once.
pub fn departures(
    read: &[std::collections::BTreeMap<String, String>],
    mark: &str,
) -> Vec<Departure> {
    let mut out: Vec<Departure> = Vec::new();
    for d in read {
        let get = |k: &str| d.get(k).map(|v| v.trim()).unwrap_or_default();
        if get("train") != mark {
            continue;
        }
        let time = get("hour")
            .parse::<u32>()
            .ok()
            .zip(get("minute").parse::<u32>().ok())
            .and_then(|(h, m)| NaiveTime::from_hms_opt(h, m, 0));
        let Some(time) = time else { continue };
        let every_day = !get("misc").contains('◆');
        match out.iter_mut().find(|x| x.time == time) {
            Some(x) => x.every_day |= every_day,
            None => out.push(Departure { time, every_day }),
        }
    }
    out
}

/// The machine's state as it needs it.
pub struct Machine {
    pub today: NaiveDate,
    /// the machine's code calls in the transcript before this run's tool call (the ids go on)
    pub first: usize,
}

/// The next step: read the timetable page, then the departures as the result.
pub fn next(_tool: &str, args: &Value, t: &[Value], m: &Machine) -> Step {
    let Some(line) = LINES.iter().find(|l| l.id == sget(args, "line")) else {
        return Step::Done(json!({"status": "error", "reason": "unknown_line", "must_say": []}));
    };
    let date = NaiveDate::parse_from_str(&sget(args, "date"), "%Y-%m-%d").unwrap_or(m.today);
    let holiday = holiday_table(date);
    let url = (line.page)(&m.today.format("%y%m").to_string(), holiday);
    let first_result = |tool: &str| {
        t.iter().enumerate().find_map(|(i, msg)| {
            tool_calls(msg).iter().find_map(|tc| {
                let (name, a) = stored_call(tc);
                let id = get_str(tc, "id");
                (name.ends_with(tool)
                    && id.starts_with(PREFIX)
                    && (tool != "browser_navigate" || sget(&a, "url") == url))
                    .then(|| {
                        t[i..].iter().find(|r| {
                            get_str(r, "role") == "tool" && get_str(r, "tool_call_id") == id
                        })
                    })
                    .flatten()
            })
        })
    };
    let n = m.first
        + t.iter()
            .flat_map(tool_calls)
            .filter(|tc| get_str(tc, "id").starts_with(PREFIX))
            .count()
        + 1;
    if first_result("browser_navigate").is_none() {
        return Step::Calls(vec![workflow::code_call(
            PREFIX,
            n,
            "browser_navigate",
            json!({"url": url}),
        )]);
    }
    let Some(read) = first_result("browser_read_elements") else {
        return Step::Calls(vec![workflow::code_call(
            PREFIX,
            n,
            "browser_read_elements",
            read_args(line),
        )]);
    };
    let table = if holiday {
        "Saturdays and holidays"
    } else {
        "weekdays"
    };
    let after = NaiveTime::parse_from_str(&sget(args, "after"), "%H:%M").ok();
    let all = departures(&super::confirm::element_fields(read, DEPARTURES), line.mark);
    let base = json!({"line": line.name, "operator": line.operator, "from": line.station, "towards": line.towards,
                      "date": date.format("%Y-%m-%d").to_string(), "timetable": table, "source": url});
    if all.is_empty() {
        let mut v = base;
        v["status"] = json!("not_read");
        v["must_say"] = json!([format!(
            "the {} timetable could not be read just now: {} has the {} times for {}",
            line.operator, url, table, line.station
        )]);
        return Step::Done(v);
    }
    let next: Vec<&Departure> = all
        .iter()
        .filter(|d| after.is_none_or(|a| d.time >= a))
        .take(SHOWN)
        .collect();
    // every train marked ◆ (the holiday table): one note instead of one per time
    let all_marked = next.iter().all(|d| !d.every_day);
    let times: Vec<String> = next
        .iter()
        .map(|d| {
            format!(
                "{}{}",
                d.time.format("%H:%M"),
                if d.every_day || all_marked {
                    ""
                } else {
                    " (not every day)"
                }
            )
        })
        .collect();
    let marked_note = if all_marked && !next.is_empty() {
        "; the timetable marks these trains as not running every day — check the page on the day"
    } else {
        ""
    };
    let mut v = base;
    v["status"] = json!("shown");
    v["departures"] = json!(next
        .iter()
        .map(|d| json!({"time": d.time.format("%H:%M").to_string(), "every_day": d.every_day}))
        .collect::<Vec<_>>());
    v["must_say"] = if times.is_empty() {
        json!([format!(
            "no {} leaves {} after {} on {} ({} timetable: {url})",
            line.name,
            line.station,
            after
                .map(|a| a.format("%H:%M").to_string())
                .unwrap_or_default(),
            date.format("%Y-%m-%d"),
            table
        )])
    } else {
        json!([format!(
            "{} from {} towards {} on {} ({} timetable{}): {}{marked_note} — {}'s timetable {url}",
            line.name,
            line.station,
            line.towards,
            date.format("%Y-%m-%d"),
            table,
            after
                .map(|a| format!(", from {}", a.format("%H:%M")))
                .unwrap_or_default(),
            times.join(", "),
            line.operator
        )])
    };
    Step::Done(v)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One departure as `browser_read_elements` reads it on JR East's page (`div.timetable_time` with `fields`).
    fn dep(hour: u32, minute: u32, misc: &str) -> Value {
        json!({"hour": hour.to_string(), "train": "NEX", "minute": minute.to_string(), "misc": misc})
    }

    /// The departures of the Saturdays-and-holidays page (2026-11): each train twice (it splits), all marked ◆.
    fn holiday_read() -> Value {
        let mut v = Vec::new();
        for (h, m) in [(7, 37), (8, 12), (8, 49), (9, 16), (9, 46), (10, 15)] {
            v.push(dep(h, m, "◆"));
            v.push(dep(h, m, "◆"));
        }
        json!(v)
    }

    fn fields(v: &Value) -> Vec<std::collections::BTreeMap<String, String>> {
        serde_json::from_value(v.clone()).unwrap()
    }

    fn d(s: &str) -> NaiveDate {
        NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap()
    }

    #[test]
    fn the_departure_elements_become_departures() {
        let weekday = json!([dep(7, 37, ""), dep(8, 12, ""), dep(8, 49, ""), dep(18, 45, "◆"),
            {"hour": "18", "train": "", "minute": "50", "misc": ""}]);
        let all = departures(&fields(&weekday), "NEX");
        assert_eq!(all.len(), 4);
        assert_eq!(all[0].time.format("%H:%M").to_string(), "07:37");
        assert!(all[0].every_day && !all[3].every_day);
        // the holiday page: doubles count once, all marked
        let all = departures(&fields(&holiday_read()), "NEX");
        let times: Vec<String> = all
            .iter()
            .map(|d| d.time.format("%H:%M").to_string())
            .collect();
        assert_eq!(
            times,
            ["07:37", "08:12", "08:49", "09:16", "09:46", "10:15"]
        );
        assert!(all.iter().all(|d| !d.every_day));
    }

    #[test]
    fn the_table_follows_the_day() {
        // Tuesday 3 November 2026 is Culture Day; 4 November a Wednesday
        assert!(holiday_table(d("2026-11-03")) && holiday_table(d("2026-11-07")));
        assert!(!holiday_table(d("2026-11-04")));
    }

    #[test]
    fn the_arguments_are_checked() {
        let today = d("2026-11-02");
        assert_eq!(
            check(
                TRAIN_TIMES,
                &json!({"line": "narita_express", "date": "2026-11-03", "after": "8:00"}),
                today
            ),
            Checked::Valid(
                json!({"line": "narita_express", "date": "2026-11-03", "after": "08:00"})
            )
        );
        assert!(matches!(
            check(TRAIN_TIMES, &json!({"line": "bus", "date": "2026-11-01"}), today),
            Checked::Invalid(p) if p.len() == 2
        ));
    }

    #[test]
    fn the_machine_reads_the_holiday_table_and_lists_the_next_departures() {
        // travel_info_real 2026-11-01: landing at Narita T1 at 7 on Culture Day, the reply gave no departure time
        let args = json!({"line": "narita_express", "date": "2026-11-03", "after": "08:00"});
        let mut t = vec![
            json!({"role": "assistant", "content": "", "tool_calls": [{"id": "c1", "type": "function",
                   "function": {"name": TRAIN_TIMES, "arguments": args.to_string()}}]}),
            json!({"role": "tool", "tool_call_id": "c1", "content": json!({"status": "running", "args": args}).to_string()}),
        ];
        let m = Machine {
            today: d("2026-11-02"),
            first: 0,
        };
        let Step::Calls(c) = next(TRAIN_TIMES, &args, &t, &m) else {
            panic!("the page first")
        };
        assert_eq!(
            (c[0].id.as_str(), c[0].name.as_str()),
            ("trn_1", "browser_navigate")
        );
        assert_eq!(
            c[0].args["url"],
            "https://timetables.jreast.co.jp/en/2611/timetable/tt1130/1130021.html"
        );
        t.push(json!({"role": "assistant", "content": "", "tool_calls": [{"id": "trn_1", "type": "function",
               "function": {"name": "browser_navigate", "arguments": c[0].args.to_string()}}]}));
        t.push(json!({"role": "tool", "tool_call_id": "trn_1", "content": "{\"url\": \"https://timetables.jreast.co.jp/\"}"}));
        let Step::Calls(c) = next(TRAIN_TIMES, &args, &t, &m) else {
            panic!("then its elements")
        };
        assert_eq!(
            (c[0].id.as_str(), c[0].name.as_str()),
            ("trn_2", "browser_read_elements")
        );
        assert_eq!(
            c[0].args["read"][0]["sel"],
            "table.result_03 tr[id^='time_'] div.timetable_time"
        );
        let env = json!({"trust": "untrusted", "source": "web page", "elements": {"departures": holiday_read()}});
        t.push(json!({"role": "assistant", "content": "", "tool_calls": [{"id": "trn_2", "type": "function",
               "function": {"name": "browser_read_elements", "arguments": c[0].args.to_string()}}]}));
        t.push(json!({"role": "tool", "tool_call_id": "trn_2",
               "content": format!("<untrusted_content source=\"web page\">\n{env}\n</untrusted_content>")}));
        let Step::Done(v) = next(TRAIN_TIMES, &args, &t, &m) else {
            panic!("done")
        };
        assert_eq!(v["status"], "shown");
        assert_eq!(v["timetable"], "Saturdays and holidays");
        let must = workflow::must_say(&v).join("\n");
        assert!(
            must.contains("08:12, 08:49, 09:16, 09:46")
                && must.contains("on 2026-11-03")
                && must.contains("1130021.html"),
            "{must}"
        );
    }
}
