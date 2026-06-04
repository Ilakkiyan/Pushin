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
