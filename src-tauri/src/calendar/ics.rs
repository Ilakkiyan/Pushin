//! Minimal iCalendar (`.ics`) reader for **read-only calendar subscriptions** — point Pushin at a
//! feed URL (a shared calendar, a team schedule, holidays) and its events flow into the calendar so the
//! scheduler plans around them. SQLite stays the source of truth; these events are marked `provider =
//! "ics"` and are never edited or pushed back.
//!
//! Scope (v1): non-recurring `VEVENT`s with `SUMMARY` / `UID` / `DTSTART` / `DTEND`, handling folded
//! lines, escaped text, all-day (`VALUE=DATE`) and UTC (`...Z`) times. **`RRULE` recurrence is not yet
//! expanded** — a recurring event yields only its first occurrence. Times are normalized to naive-local
//! `YYYY-MM-DDTHH:MM:SS` (gotcha: Pushin stores wall-clock local, never a zone).

use chrono::{Duration, Local, NaiveDate, NaiveDateTime, TimeZone, Utc};

use crate::model::DT_FMT;

/// One event parsed from an `.ics` feed. Times are naive-local strings (`DT_FMT`).
#[derive(Debug, Clone, PartialEq)]
pub struct IcsEvent {
    pub uid: String,
    pub title: String,
    pub start: String,
    pub end: String,
}

/// Parse an `.ics` document into its (non-recurring) events. Malformed events are skipped, never fatal.
pub fn parse_ics(text: &str) -> Vec<IcsEvent> {
    let unfolded = unfold(text);
    let mut out = Vec::new();
    let mut cur: Option<Partial> = None;
    for line in unfolded.lines() {
        match line.trim_end() {
            "BEGIN:VEVENT" => cur = Some(Partial::default()),
            "END:VEVENT" => {
                if let Some(p) = cur.take() {
                    if let Some(ev) = p.finish() {
                        out.push(ev);
                    }
                }
            }
            other => {
                if let Some(p) = cur.as_mut() {
                    if let Some((name, params, value)) = split_prop(other) {
                        match name.as_str() {
                            "UID" => p.uid = value,
                            "SUMMARY" => p.title = unescape(&value),
                            "DTSTART" => p.start = parse_dt(&params, &value),
                            "DTEND" => p.end = parse_dt(&params, &value),
                            _ => {}
                        }
                    }
                }
            }
        }
    }
    out
}

#[derive(Default)]
struct Partial {
    uid: String,
    title: String,
    start: Option<(NaiveDateTime, bool)>, // (when, all_day)
    end: Option<(NaiveDateTime, bool)>,
}

impl Partial {
    fn finish(self) -> Option<IcsEvent> {
        let (start, all_day) = self.start?;
        // Missing DTEND: all-day → +1 day; timed → +1 hour (a sane default, same as the app's).
        let end = self
            .end
            .map(|(e, _)| e)
            .unwrap_or_else(|| start + if all_day { Duration::days(1) } else { Duration::hours(1) });
        let title = if self.title.trim().is_empty() { "(untitled)".to_string() } else { self.title };
        Some(IcsEvent {
            uid: if self.uid.trim().is_empty() { format!("{}-{}", title, start.format(DT_FMT)) } else { self.uid },
            title,
            start: start.format(DT_FMT).to_string(),
            end: end.format(DT_FMT).to_string(),
        })
    }
}

/// RFC 5545 line unfolding: a line beginning with a space or tab continues the previous one.
fn unfold(text: &str) -> String {
    let mut out = String::new();
    for raw in text.split('\n') {
        let line = raw.strip_suffix('\r').unwrap_or(raw);
        if line.starts_with(' ') || line.starts_with('\t') {
            out.push_str(&line[1..]);
        } else {
            if !out.is_empty() {
                out.push('\n');
            }
            out.push_str(line);
        }
    }
    out
}

/// Split a content line into `(NAME, params, value)`. `NAME;PARAM=x;PARAM2=y:VALUE`.
fn split_prop(line: &str) -> Option<(String, String, String)> {
    let colon = line.find(':')?;
    let (lhs, rhs) = line.split_at(colon);
    let value = rhs[1..].to_string();
    let (name, params) = match lhs.find(';') {
        Some(semi) => (lhs[..semi].to_string(), lhs[semi + 1..].to_string()),
        None => (lhs.to_string(), String::new()),
    };
    Some((name.to_uppercase(), params.to_uppercase(), value))
}

/// Parse a DTSTART/DTEND value into (naive-local datetime, all_day). Handles `VALUE=DATE` (all-day),
/// UTC (`...Z` → converted to local wall time), and floating/`TZID` local times (taken as-is).
fn parse_dt(params: &str, value: &str) -> Option<(NaiveDateTime, bool)> {
    let v = value.trim();
    if params.contains("VALUE=DATE") || (v.len() == 8 && !v.contains('T')) {
        let d = NaiveDate::parse_from_str(v, "%Y%m%d").ok()?;
        return Some((d.and_hms_opt(0, 0, 0)?, true));
    }
    if let Some(utc) = v.strip_suffix('Z') {
        let naive = NaiveDateTime::parse_from_str(utc, "%Y%m%dT%H%M%S").ok()?;
        // The written time is UTC — convert to the machine's local wall clock (what Pushin stores).
        let local = Utc.from_utc_datetime(&naive).with_timezone(&Local).naive_local();
        return Some((local, false));
    }
    // Floating time or an explicit TZID we don't resolve — take the wall-clock as local.
    let naive = NaiveDateTime::parse_from_str(v, "%Y%m%dT%H%M%S").ok()?;
    Some((naive, false))
}

/// Unescape iCalendar TEXT: `\n`, `\,`, `\;`, `\\`.
fn unescape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.next() {
                Some('n') | Some('N') => out.push('\n'),
                Some(other) => out.push(other),
                None => out.push('\\'),
            }
        } else {
            out.push(c);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn one(ics: &str) -> IcsEvent {
        let evs = parse_ics(ics);
        assert_eq!(evs.len(), 1, "expected exactly one event");
        evs.into_iter().next().unwrap()
    }

    #[test]
    fn parses_a_basic_timed_event() {
        let ev = one("BEGIN:VEVENT\nUID:abc123\nSUMMARY:Team sync\nDTSTART:20260715T140000\nDTEND:20260715T150000\nEND:VEVENT\n");
        assert_eq!(ev.uid, "abc123");
        assert_eq!(ev.title, "Team sync");
        assert_eq!(ev.start, "2026-07-15T14:00:00");
        assert_eq!(ev.end, "2026-07-15T15:00:00");
    }

    #[test]
    fn all_day_event_spans_the_day() {
        let ev = one("BEGIN:VEVENT\nUID:h1\nSUMMARY:Holiday\nDTSTART;VALUE=DATE:20260715\nEND:VEVENT\n");
        assert_eq!(ev.start, "2026-07-15T00:00:00");
        assert_eq!(ev.end, "2026-07-16T00:00:00", "no DTEND → all-day rolls to next midnight");
    }

    #[test]
    fn missing_dtend_defaults_to_one_hour() {
        let ev = one("BEGIN:VEVENT\nUID:x\nSUMMARY:Quick\nDTSTART:20260715T090000\nEND:VEVENT\n");
        assert_eq!(ev.end, "2026-07-15T10:00:00");
    }

    #[test]
    fn folded_lines_and_escapes_are_handled() {
        // A folded SUMMARY (continuation line starts with a space) + escaped comma/newline.
        let ev = one("BEGIN:VEVENT\nUID:f1\nSUMMARY:Lunch with Sam\\, then a \n walk\nDTSTART:20260715T120000\nDTEND:20260715T130000\nEND:VEVENT\n");
        assert_eq!(ev.title, "Lunch with Sam, then a walk");
    }

    #[test]
    fn crlf_and_multiple_events() {
        let ics = "BEGIN:VCALENDAR\r\nBEGIN:VEVENT\r\nUID:1\r\nSUMMARY:A\r\nDTSTART:20260715T090000\r\nDTEND:20260715T100000\r\nEND:VEVENT\r\nBEGIN:VEVENT\r\nUID:2\r\nSUMMARY:B\r\nDTSTART:20260716T090000\r\nDTEND:20260716T100000\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";
        let evs = parse_ics(ics);
        assert_eq!(evs.len(), 2);
        assert_eq!(evs[0].title, "A");
        assert_eq!(evs[1].uid, "2");
    }

    #[test]
    fn a_broken_event_without_dtstart_is_skipped() {
        let evs = parse_ics("BEGIN:VEVENT\nUID:no-start\nSUMMARY:Nope\nEND:VEVENT\n");
        assert!(evs.is_empty(), "an event with no DTSTART is dropped, not fatal");
    }

    #[test]
    fn empty_summary_becomes_untitled() {
        let ev = one("BEGIN:VEVENT\nUID:u\nDTSTART:20260715T140000\nDTEND:20260715T150000\nEND:VEVENT\n");
        assert_eq!(ev.title, "(untitled)");
    }
}
