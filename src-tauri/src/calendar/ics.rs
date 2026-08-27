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
    // Depth of any sub-component nested INSIDE the current VEVENT (`VALARM`, and in principle
    // anything else a producer nests). Properties there belong to the sub-component, not the event:
    // a `VALARM` carries its own `SUMMARY` (the reminder's subject), and reading it flat used to
    // silently overwrite the event's title with the alarm's — every Google/Outlook feed with
    // reminders came in mis-titled. Skip properties while depth > 0.
    let mut nested = 0usize;
    for raw in unfolded.lines() {
        let line = raw.trim_end();
        if let Some(comp) = line.strip_prefix("BEGIN:") {
            if comp.eq_ignore_ascii_case("VEVENT") && cur.is_none() {
                cur = Some(Partial::default());
            } else if cur.is_some() {
                nested += 1;
            }
            continue;
        }
        if let Some(comp) = line.strip_prefix("END:") {
            if nested > 0 {
                nested -= 1;
            } else if comp.eq_ignore_ascii_case("VEVENT") {
                if let Some(p) = cur.take() {
                    if let Some(ev) = p.finish() {
                        out.push(ev);
                    }
                }
            }
            continue;
        }
        if nested > 0 {
            continue;
        }
        if let Some(p) = cur.as_mut() {
            if let Some((name, params, value)) = split_prop(line) {
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
        //
        // A DTEND that is not strictly after DTSTART gets the same treatment. Feeds in the wild
        // ship both zero-length events (`DTEND == DTSTART`) and outright reversed ones; either
        // produces an interval that every overlap test in the scheduler (`a.start < b.end &&
        // b.start < a.end`) silently reports as "never overlaps", so the event drew as a hairline
        // and the scheduler happily planned straight through it.
        let end = match self.end.map(|(e, _)| e) {
            Some(e) if e > start => e,
            _ => start + if all_day { Duration::days(1) } else { Duration::hours(1) },
        };
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

    /// Build an `.ics` document from its content lines, CRLF-terminated the way real feeds are.
    /// Composing from lines keeps the fixtures readable and escape-proof — hand-written inline
    /// literals hid a folded-line bug in the *test data* rather than in the parser.
    fn ics(lines: &[&str]) -> String {
        lines.iter().map(|l| format!("{l}\r\n")).collect()
    }

    #[test]
    fn a_valarm_summary_does_not_clobber_the_event_title() {
        // Regression: VALARM properties were read flat into the enclosing event, so any feed whose
        // events carry reminders (i.e. most of them) imported titled after the alarm.
        let ev = one(&ics(&[
            "BEGIN:VEVENT",
            "UID:v1",
            "SUMMARY:Quarterly review",
            "DTSTART:20260715T140000",
            "DTEND:20260715T150000",
            "BEGIN:VALARM",
            "ACTION:DISPLAY",
            "SUMMARY:Reminder",
            "DESCRIPTION:Ping",
            "TRIGGER:-PT15M",
            "END:VALARM",
            "END:VEVENT",
        ]));
        assert_eq!(ev.title, "Quarterly review");
        assert_eq!(ev.start, "2026-07-15T14:00:00");
        assert_eq!(ev.end, "2026-07-15T15:00:00");
    }

    #[test]
    fn a_valarm_cannot_overwrite_the_events_times_either() {
        // A VALARM carrying its own DTSTART/DTEND must not move the event — that would silently
        // drag a meeting to the reminder's time.
        let ev = one(&ics(&[
            "BEGIN:VEVENT",
            "UID:v2",
            "SUMMARY:Real meeting",
            "DTSTART:20260715T140000",
            "DTEND:20260715T150000",
            "BEGIN:VALARM",
            "DTSTART:20260101T000000",
            "DTEND:20260101T003000",
            "UID:alarm-uid",
            "END:VALARM",
            "END:VEVENT",
        ]));
        assert_eq!(ev.start, "2026-07-15T14:00:00");
        assert_eq!(ev.end, "2026-07-15T15:00:00");
        assert_eq!(ev.uid, "v2");
    }

    #[test]
    fn nested_components_do_not_swallow_the_end_of_the_event() {
        // Two VALARMs back to back — the depth counter has to unwind both before END:VEVENT counts,
        // or the second event is parsed as part of the first and vanishes.
        let evs = parse_ics(&ics(&[
            "BEGIN:VEVENT",
            "UID:a",
            "SUMMARY:First",
            "DTSTART:20260715T090000",
            "DTEND:20260715T100000",
            "BEGIN:VALARM",
            "SUMMARY:R1",
            "END:VALARM",
            "BEGIN:VALARM",
            "SUMMARY:R2",
            "END:VALARM",
            "END:VEVENT",
            "BEGIN:VEVENT",
            "UID:b",
            "SUMMARY:Second",
            "DTSTART:20260716T090000",
            "DTEND:20260716T100000",
            "END:VEVENT",
        ]));
        assert_eq!(evs.len(), 2, "both events survive the nesting");
        assert_eq!(evs[0].title, "First");
        assert_eq!(evs[1].title, "Second");
    }

    #[test]
    fn a_reversed_dtend_falls_back_to_a_sane_duration() {
        // A backwards interval reads as "never overlaps anything" to every scheduler check
        // (`a.start < b.end && b.start < a.end`), so the event drew as a hairline AND the
        // auto-scheduler planned straight through it. Treat it as a missing DTEND instead.
        let ev = one(&ics(&[
            "BEGIN:VEVENT",
            "UID:r",
            "SUMMARY:Backwards",
            "DTSTART:20260715T140000",
            "DTEND:20260715T130000",
            "END:VEVENT",
        ]));
        assert_eq!(ev.start, "2026-07-15T14:00:00");
        assert_eq!(ev.end, "2026-07-15T15:00:00", "reversed end falls back to the +1h default");
    }

    #[test]
    fn a_zero_length_event_gets_a_duration() {
        let ev = one(&ics(&[
            "BEGIN:VEVENT",
            "UID:z",
            "SUMMARY:Instant",
            "DTSTART:20260715T140000",
            "DTEND:20260715T140000",
            "END:VEVENT",
        ]));
        assert_eq!(ev.end, "2026-07-15T15:00:00");

        let allday = one(&ics(&[
            "BEGIN:VEVENT",
            "UID:z2",
            "SUMMARY:Day",
            "DTSTART;VALUE=DATE:20260715",
            "DTEND;VALUE=DATE:20260715",
            "END:VEVENT",
        ]));
        assert_eq!(allday.end, "2026-07-16T00:00:00", "a same-day all-day DTEND still spans the day");
    }

    #[test]
    fn an_event_with_no_uid_gets_a_stable_synthetic_one() {
        // The synthetic uid is what de-dups a feed across refreshes, so it must be deterministic.
        let doc = ics(&["BEGIN:VEVENT", "SUMMARY:No id", "DTSTART:20260715T140000", "DTEND:20260715T150000", "END:VEVENT"]);
        let a = one(&doc);
        let b = one(&doc);
        assert_eq!(a.uid, b.uid, "same feed, same uid — otherwise every refresh duplicates the event");
        assert!(a.uid.contains("No id"), "uid derives from title + start: {}", a.uid);
    }

    #[test]
    fn a_utc_timestamp_lands_on_the_local_wall_clock() {
        // Pushin stores naive-local, so a `...Z` value must be converted, not copied.
        let ev = one(&ics(&[
            "BEGIN:VEVENT",
            "UID:u",
            "SUMMARY:UTC",
            "DTSTART:20260715T120000Z",
            "DTEND:20260715T130000Z",
            "END:VEVENT",
        ]));
        let want = Utc
            .from_utc_datetime(&NaiveDateTime::parse_from_str("20260715T120000", "%Y%m%dT%H%M%S").unwrap())
            .with_timezone(&Local)
            .naive_local();
        assert_eq!(ev.start, want.format(DT_FMT).to_string());
        // ...and the hour-long span survives the conversion (both ends shift together).
        let s = NaiveDateTime::parse_from_str(&ev.start, DT_FMT).unwrap();
        let e = NaiveDateTime::parse_from_str(&ev.end, DT_FMT).unwrap();
        assert_eq!((e - s).num_minutes(), 60);
    }

    #[test]
    fn property_names_and_parameters_are_matched_case_insensitively() {
        // Producers vary the case of both.
        let ev = one(&ics(&["BEGIN:VEVENT", "uid:c1", "summary:Lowercase", "dtstart;value=date:20260715", "END:VEVENT"]));
        assert_eq!(ev.title, "Lowercase");
        assert_eq!(ev.start, "2026-07-15T00:00:00");
        assert_eq!(ev.end, "2026-07-16T00:00:00", "all-day recognised through a lowercase VALUE=DATE");
    }

    #[test]
    fn a_tzid_parameter_is_taken_as_wall_clock_local() {
        // Documented v1 limitation (no tz database) — pinned so changing it is a deliberate act.
        let ev = one(&ics(&[
            "BEGIN:VEVENT",
            "UID:t",
            "SUMMARY:Zoned",
            "DTSTART;TZID=America/New_York:20260715T140000",
            "DTEND;TZID=America/New_York:20260715T150000",
            "END:VEVENT",
        ]));
        assert_eq!(ev.start, "2026-07-15T14:00:00");
        assert_eq!(ev.end, "2026-07-15T15:00:00");
    }

    #[test]
    fn garbage_input_yields_no_events_and_never_panics() {
        let junk: Vec<String> = vec![
            String::new(),
            "\n\n\n".to_string(),
            ics(&["BEGIN:VEVENT"]),                                             // never closed
            ics(&["END:VEVENT"]),                                               // closed without opening
            ics(&["BEGIN:VEVENT", "DTSTART:notadate", "END:VEVENT"]),           // unparseable date
            ics(&["BEGIN:VEVENT", "DTSTART:", "END:VEVENT"]),                   // empty value
            ics(&["BEGIN:VEVENT", "SUMMARY", "END:VEVENT"]),                    // no colon at all
            ics(&["BEGIN:VEVENT", "DTSTART;VALUE=DATE:20261332", "END:VEVENT"]), // month 13, day 32
            ics(&["BEGIN:VEVENT", "DTSTART:20260715T250000", "END:VEVENT"]),    // hour 25
            " leading fold with no previous line".to_string(),
            "\u{0}\u{1}binary junk\u{feff}".to_string(),
        ];
        for j in &junk {
            assert!(parse_ics(j).is_empty(), "expected no events from {j:?}");
        }
    }

    #[test]
    fn one_broken_event_does_not_take_the_rest_of_the_feed_with_it() {
        // The whole point of "malformed events are skipped, never fatal": a single bad entry in a
        // 500-event corporate feed must not blank the calendar.
        let evs = parse_ics(&ics(&[
            "BEGIN:VCALENDAR",
            "BEGIN:VEVENT",
            "UID:ok1",
            "SUMMARY:Good one",
            "DTSTART:20260715T090000",
            "END:VEVENT",
            "BEGIN:VEVENT",
            "UID:bad",
            "SUMMARY:No start",
            "END:VEVENT",
            "BEGIN:VEVENT",
            "UID:ok2",
            "SUMMARY:Good two",
            "DTSTART:20260716T090000",
            "END:VEVENT",
            "END:VCALENDAR",
        ]));
        assert_eq!(evs.len(), 2);
        assert_eq!(evs[0].uid, "ok1");
        assert_eq!(evs[1].uid, "ok2");
    }

    #[test]
    fn non_ascii_titles_survive_folding_and_escaping() {
        // Byte-oriented unfolding over multi-byte text is a classic panic source. This SUMMARY is
        // folded mid-phrase and carries combining marks plus astral-plane-adjacent symbols. Note
        // the DOUBLE space on the continuation: RFC 5545 unfolding eats the single fold
        // whitespace, so a space that belongs to the value has to be written twice.
        let ev = one(&ics(&[
            "BEGIN:VEVENT",
            "UID:e",
            "SUMMARY:Cafe\u{301} \u{2615} with Jo\u{308}rg",
            "  and 15\u{20ac}",
            "DTSTART:20260715T140000",
            "END:VEVENT",
        ]));
        assert_eq!(ev.title, "Cafe\u{301} \u{2615} with Jo\u{308}rg and 15\u{20ac}");
    }

    #[test]
    fn escaped_text_is_unescaped() {
        let ev = one(&ics(&[
            "BEGIN:VEVENT",
            "UID:n",
            r"SUMMARY:Line one\nLine two\, and\; more",
            "DTSTART:20260715T140000",
            "END:VEVENT",
        ]));
        assert_eq!(ev.title, "Line one\nLine two, and; more");
    }

    #[test]
    fn a_trailing_backslash_does_not_eat_the_string() {
        let ev = one(&ics(&["BEGIN:VEVENT", "UID:b", r"SUMMARY:Ends with a slash\", "DTSTART:20260715T140000", "END:VEVENT"]));
        assert_eq!(ev.title, r"Ends with a slash\");
    }

    #[test]
    fn properties_outside_a_vevent_are_ignored() {
        // Calendar-level SUMMARY/UID must not leak into the first event.
        let evs = parse_ics(&ics(&[
            "BEGIN:VCALENDAR",
            "SUMMARY:The whole calendar",
            "UID:cal",
            "BEGIN:VEVENT",
            "UID:e1",
            "SUMMARY:Real event",
            "DTSTART:20260715T140000",
            "END:VEVENT",
            "END:VCALENDAR",
        ]));
        assert_eq!(evs.len(), 1);
        assert_eq!(evs[0].title, "Real event");
        assert_eq!(evs[0].uid, "e1");
    }

    #[test]
    fn a_value_containing_a_colon_is_kept_whole() {
        // split_prop cuts at the FIRST colon; everything after it is the value.
        let ev = one(&ics(&["BEGIN:VEVENT", "UID:x", "SUMMARY:Standup: sprint 42", "DTSTART:20260715T140000", "END:VEVENT"]));
        assert_eq!(ev.title, "Standup: sprint 42");
    }

    #[test]
    fn empty_summary_becomes_untitled() {
        let ev = one("BEGIN:VEVENT\nUID:u\nDTSTART:20260715T140000\nDTEND:20260715T150000\nEND:VEVENT\n");
        assert_eq!(ev.title, "(untitled)");
    }
}
