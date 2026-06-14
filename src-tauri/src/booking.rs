//! Booking-page logic (Calendly-style). The first pass is local-only; the public,
//! shareable page needs a small hosted relay (later). Availability is computed by
//! REUSING `scheduler::free_slots` — no duplicate calendar math.

use crate::db;
use crate::model::{EventType, Settings};
use crate::scheduler::{self, fmt_dt, parse_dt, Interval};
use anyhow::Result;
use chrono::Local;
use rusqlite::Connection;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BookingSlot {
    pub start: String,
    pub end: String,
}

/// Bookable slots for an event type over the next `horizon_days`, derived from the
/// same free-time the scheduler uses. Busy = all events + all existing blocks.
pub fn available_slots(
    conn: &Connection,
    base_settings: &Settings,
    event_type: &EventType,
    horizon_days: i64,
) -> Result<Vec<BookingSlot>> {
    let mut busy: Vec<Interval> = Vec::new();
    for e in db::list_events(conn)? {
        if let (Some(s), Some(en)) = (parse_dt(&e.start), parse_dt(&e.end)) {
            busy.push(Interval { start: s, end: en });
        }
    }
    for b in db::list_blocks(conn)? {
        if let (Some(s), Some(en)) = (parse_dt(&b.start), parse_dt(&b.end)) {
            busy.push(Interval { start: s, end: en });
        }
    }

    // Use the user's working hours but our own horizon + buffer for booking.
    let settings = Settings {
        horizon_days,
        buffer_minutes: event_type.buffer_minutes,
        ..base_settings.clone()
    };

    let now = Local::now().naive_local();
    let free = scheduler::free_slots(now, &settings, &busy);

    let dur = chrono::Duration::minutes(event_type.duration_minutes.max(15));
    let mut slots = Vec::new();
    for iv in free {
        let mut cur = iv.start;
        while cur + dur <= iv.end {
            slots.push(BookingSlot { start: fmt_dt(cur), end: fmt_dt(cur + dur) });
            cur += dur;
        }
    }
    Ok(slots)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scheduler::parse_dt;

    fn et(dur: i64, buf: i64) -> EventType {
        EventType { id: 1, name: "Call".into(), duration_minutes: dur, buffer_minutes: buf, color: "#000".into() }
    }

    #[test]
    fn slots_have_the_event_duration_and_dont_overlap_each_other() {
        let conn = db::test_conn();
        let slots = available_slots(&conn, &Settings::default(), &et(30, 0), 7).unwrap();
        assert!(!slots.is_empty(), "an empty calendar yields bookable slots");
        for s in &slots {
            let (start, end) = (parse_dt(&s.start).unwrap(), parse_dt(&s.end).unwrap());
            assert_eq!((end - start).num_minutes(), 30, "each slot is exactly the event duration");
        }
        // Sorted and non-overlapping (each starts no earlier than the previous ends).
        for w in slots.windows(2) {
            assert!(parse_dt(&w[1].start).unwrap() >= parse_dt(&w[0].end).unwrap());
        }
    }

    #[test]
    fn slots_avoid_busy_events() {
        let conn = db::test_conn();
        // Block out a wide window tomorrow during typical working hours.
        let tomorrow = (Local::now().naive_local().date() + chrono::Duration::days(1)).format("%Y-%m-%d");
        let busy_start = format!("{tomorrow}T09:00:00");
        let busy_end = format!("{tomorrow}T17:00:00");
        db::insert_event(&conn, "All-day workshop", &busy_start, &busy_end, "fixed").unwrap();

        let slots = available_slots(&conn, &Settings::default(), &et(30, 0), 3).unwrap();
        let (bs, be) = (parse_dt(&busy_start).unwrap(), parse_dt(&busy_end).unwrap());
        for s in &slots {
            let (start, end) = (parse_dt(&s.start).unwrap(), parse_dt(&s.end).unwrap());
            assert!(end <= bs || start >= be, "slot {start}–{end} must not overlap the busy window");
        }
    }

    #[test]
    fn longer_horizon_offers_at_least_as_many_slots() {
        let conn = db::test_conn();
        let few = available_slots(&conn, &Settings::default(), &et(60, 0), 2).unwrap();
        let many = available_slots(&conn, &Settings::default(), &et(60, 0), 7).unwrap();
        assert!(many.len() >= few.len());
    }
}
