//! A Python-`datetime`-like aware timestamp for the calendar connectors.
//!
//! The Python code mixed `ZoneInfo` zones and fixed offsets freely: arithmetic is on the wall clock
//! (`dt + timedelta(days=1)` keeps the local time across a DST change), `isoformat()` prints the local
//! time with its offset, comparisons use the instant. [`PyDt`] reproduces that so the strings sent to
//! Google and shown to the user match the Python ones.

use std::cmp::Ordering;
use std::sync::LazyLock;

use chrono::{
    DateTime, Duration, FixedOffset, Local, LocalResult, NaiveDate, NaiveDateTime, NaiveTime,
    Offset, TimeZone, Utc,
};
use chrono_tz::Tz;
use regex::Regex;

/// A `tzinfo`: a fixed offset (`timezone(...)`, parsed `+08:00`) or an IANA zone (`ZoneInfo`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PyTz {
    Fixed(FixedOffset),
    Zone(Tz),
}

impl PyTz {
    /// `timezone.utc`.
    pub const UTC: PyTz = PyTz::Fixed(FixedOffset::east_opt(0).unwrap());

    /// `ZoneInfo(name)`; `None` when the name is unknown.
    pub fn zone(name: &str) -> Option<PyTz> {
        name.parse::<Tz>().ok().map(PyTz::Zone)
    }

    /// `ZoneInfo(name)` with a fallback zone when the name is unknown.
    pub fn zone_or(name: &str, fallback: PyTz) -> PyTz {
        PyTz::zone(name).unwrap_or(fallback)
    }

    /// Zone name (`str(tzinfo)`): the IANA name, or `UTC` / `UTC+08:00` for fixed offsets.
    pub fn name(&self) -> String {
        match self {
            PyTz::Zone(tz) => tz.name().to_string(),
            PyTz::Fixed(o) if o.local_minus_utc() == 0 => "UTC".into(),
            PyTz::Fixed(o) => format!("UTC{}", fmt_offset(*o)),
        }
    }

    fn offset_at_local(&self, local: &NaiveDateTime) -> FixedOffset {
        match self {
            PyTz::Fixed(o) => *o,
            PyTz::Zone(tz) => match tz.offset_from_local_datetime(local) {
                LocalResult::Single(o) | LocalResult::Ambiguous(o, _) => o.fix(),
                LocalResult::None => tz.offset_from_utc_datetime(local).fix(),
            },
        }
    }

    fn offset_at_utc(&self, utc: &NaiveDateTime) -> FixedOffset {
        match self {
            PyTz::Fixed(o) => *o,
            PyTz::Zone(tz) => tz.offset_from_utc_datetime(utc).fix(),
        }
    }
}

fn fmt_offset(o: FixedOffset) -> String {
    let secs = o.local_minus_utc();
    let sign = if secs < 0 { '-' } else { '+' };
    let a = secs.abs();
    if a % 60 == 0 {
        format!("{}{:02}:{:02}", sign, a / 3600, a % 3600 / 60)
    } else {
        format!("{}{:02}:{:02}:{:02}", sign, a / 3600, a % 3600 / 60, a % 60)
    }
}

/// An aware `datetime`: local wall-clock time plus its `tzinfo`.
#[derive(Clone, Copy, Debug)]
pub struct PyDt {
    local: NaiveDateTime,
    tz: PyTz,
}

impl PyDt {
    /// `datetime(..., tzinfo=tz)` from a wall-clock time.
    pub fn new(local: NaiveDateTime, tz: PyTz) -> Self {
        Self { local, tz }
    }

    /// `datetime.fromtimestamp(utc, tz)`.
    pub fn from_utc(utc: NaiveDateTime, tz: PyTz) -> Self {
        let off = tz.offset_at_utc(&utc);
        Self {
            local: utc + off,
            tz,
        }
    }

    /// `datetime.now(tz)`.
    pub fn now(tz: PyTz) -> Self {
        Self::from_utc(Utc::now().naive_utc(), tz)
    }

    pub fn local(&self) -> NaiveDateTime {
        self.local
    }

    pub fn tz(&self) -> PyTz {
        self.tz
    }

    pub fn date(&self) -> NaiveDate {
        self.local.date()
    }

    /// `utcoffset()`.
    pub fn offset(&self) -> FixedOffset {
        self.tz.offset_at_local(&self.local)
    }

    /// The instant as naive UTC.
    pub fn utc(&self) -> NaiveDateTime {
        self.local - self.offset()
    }

    /// `astimezone(tz)`.
    pub fn astimezone(&self, tz: PyTz) -> PyDt {
        PyDt::from_utc(self.utc(), tz)
    }

    /// `dt + timedelta` — wall-clock arithmetic, like Python.
    pub fn add(&self, d: Duration) -> PyDt {
        PyDt {
            local: self.local + d,
            tz: self.tz,
        }
    }

    /// `self - other` as a duration between the two instants.
    pub fn diff(&self, other: &PyDt) -> Duration {
        self.utc() - other.utc()
    }

    /// `isoformat()`: `2026-10-05T00:00:00+08:00` (microseconds only when non-zero).
    pub fn isoformat(&self) -> String {
        let base = if self.local.and_utc().timestamp_subsec_micros() == 0 {
            self.local.format("%Y-%m-%dT%H:%M:%S").to_string()
        } else {
            self.local.format("%Y-%m-%dT%H:%M:%S%.6f").to_string()
        };
        format!("{}{}", base, fmt_offset(self.offset()))
    }

    /// `strftime(fmt)` on the wall-clock time.
    pub fn strftime(&self, fmt: &str) -> String {
        self.local.format(fmt).to_string()
    }

    /// The instant as a chrono UTC time.
    pub fn to_utc(&self) -> DateTime<Utc> {
        self.utc().and_utc()
    }

    /// `datetime.fromisoformat(s)`: `YYYY-MM-DD`, `YYYY-MM-DD[T ]HH:MM[:SS[.ffffff]]`, optional `Z` /
    /// `±HH:MM` / `±HHMM` / `±HH` offset. A value without an offset gets `default_tz` (Python: naive →
    /// `replace(tzinfo=...)`). `None` when Python would raise `ValueError`.
    pub fn fromisoformat(s: &str, default_tz: PyTz) -> Option<PyDt> {
        static RE: LazyLock<Regex> = LazyLock::new(|| {
            Regex::new(
                r"^(\d{4})-(\d{2})-(\d{2})(?:[Tt ](\d{2})(?::(\d{2})(?::(\d{2})(?:[.,](\d{1,6}))?)?)?(Z|z|[+-]\d{2}(?::?\d{2})?(?::?\d{2})?)?)?$",
            )
            .unwrap()
        });
        let c = RE.captures(s)?;
        let num = |i: usize| c.get(i).map(|m| m.as_str().parse::<u32>().unwrap_or(99));
        let date = NaiveDate::from_ymd_opt(num(1)? as i32, num(2)?, num(3)?)?;
        let (h, mi, se) = (
            num(4).unwrap_or(0),
            num(5).unwrap_or(0),
            num(6).unwrap_or(0),
        );
        let micro = c
            .get(7)
            .map(|m| format!("{:0<6}", m.as_str()).parse::<u32>().unwrap_or(0))
            .unwrap_or(0);
        let time = NaiveTime::from_hms_micro_opt(h, mi, se, micro)?;
        let tz = match c.get(8) {
            None => default_tz,
            Some(m) if m.as_str().eq_ignore_ascii_case("z") => PyTz::UTC,
            Some(m) => PyTz::Fixed(parse_offset(m.as_str())?),
        };
        Some(PyDt::new(NaiveDateTime::new(date, time), tz))
    }

    /// The process-local zone as Python's `astimezone()` of a naive value would use it.
    pub fn local_tz_for(local: &NaiveDateTime) -> PyTz {
        match Local.offset_from_local_datetime(local) {
            LocalResult::Single(o) | LocalResult::Ambiguous(o, _) => PyTz::Fixed(o.fix()),
            LocalResult::None => PyTz::UTC,
        }
    }
}

fn parse_offset(s: &str) -> Option<FixedOffset> {
    let sign = if s.starts_with('-') { -1 } else { 1 };
    let digits: String = s[1..].chars().filter(|c| c.is_ascii_digit()).collect();
    let (hh, mm, ss) = match digits.len() {
        2 => (digits[..2].parse::<i32>().ok()?, 0, 0),
        4 => (
            digits[..2].parse::<i32>().ok()?,
            digits[2..4].parse::<i32>().ok()?,
            0,
        ),
        6 => (
            digits[..2].parse::<i32>().ok()?,
            digits[2..4].parse::<i32>().ok()?,
            digits[4..6].parse::<i32>().ok()?,
        ),
        _ => return None,
    };
    if hh > 23 || mm > 59 || ss > 59 {
        return None;
    }
    FixedOffset::east_opt(sign * (hh * 3600 + mm * 60 + ss))
}

impl PartialEq for PyDt {
    fn eq(&self, other: &Self) -> bool {
        self.utc() == other.utc()
    }
}
impl Eq for PyDt {}
impl PartialOrd for PyDt {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}
impl Ord for PyDt {
    fn cmp(&self, other: &Self) -> Ordering {
        self.utc().cmp(&other.utc())
    }
}

/// Python `max(a, b)`: `a` unless `b > a`.
pub fn py_max(a: PyDt, b: PyDt) -> PyDt {
    if b > a {
        b
    } else {
        a
    }
}

/// Python `min(a, b)`: `a` unless `b < a`.
pub fn py_min(a: PyDt, b: PyDt) -> PyDt {
    if b < a {
        b
    } else {
        a
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn iso_roundtrip() {
        let sg = PyTz::zone("Asia/Singapore").unwrap();
        let d = PyDt::fromisoformat("2026-10-03T19:00", sg).unwrap();
        assert_eq!(d.isoformat(), "2026-10-03T19:00:00+08:00");
        assert_eq!(
            d.astimezone(PyTz::UTC).isoformat(),
            "2026-10-03T11:00:00+00:00"
        );
        let z = PyDt::fromisoformat("2026-10-05T01:30:00+00:00", sg).unwrap();
        assert_eq!(
            z.astimezone(sg).strftime("%Y-%m-%d %a %H:%M"),
            "2026-10-05 Mon 09:30"
        );
        assert!(PyDt::fromisoformat("next friday", sg).is_none());
        assert!(PyDt::fromisoformat("2026-13-01", sg).is_none());
        assert_eq!(
            PyDt::fromisoformat("2026-10-03", sg).unwrap().isoformat(),
            "2026-10-03T00:00:00+08:00"
        );
        assert_eq!(
            PyDt::fromisoformat("2026-10-03T19:00:00.5+0530", sg)
                .unwrap()
                .isoformat(),
            "2026-10-03T19:00:00.500000+05:30"
        );
    }

    #[test]
    fn wall_clock_arithmetic_and_order() {
        let ny = PyTz::zone("America/New_York").unwrap();
        let d = PyDt::fromisoformat("2026-10-31T09:00", ny).unwrap();
        let e = d.add(Duration::days(7)); // crosses the DST end on 2026-11-01
        assert_eq!(e.isoformat(), "2026-11-07T09:00:00-05:00");
        assert_eq!(e.diff(&d), Duration::days(7) + Duration::hours(1));
        let a = PyDt::fromisoformat("2026-10-05T10:00+08:00", PyTz::UTC).unwrap();
        let b = PyDt::fromisoformat("2026-10-05T02:00Z", PyTz::UTC).unwrap();
        assert!(a == b && py_max(a, b).tz() == a.tz());
    }
}
