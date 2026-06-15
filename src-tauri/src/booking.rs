//! Booking-page logic (Calendly-style). The first pass is local-only; the public,
//! shareable page needs a small hosted relay (later). Availability is computed by
//! REUSING `scheduler::free_slots` — no duplicate calendar math.

use crate::db;
use crate::model::{EventType, Settings};
use crate::scheduler::{self, fmt_dt, parse_dt, Interval};
use anyhow::{anyhow, bail, Result};
use chrono::Local;
use rusqlite::Connection;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BookingSlot {
    pub start: String,
    pub end: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PublicEventType {
    pub name: String,
    pub duration_minutes: i64,
    pub buffer_minutes: i64,
    pub color: String,
    pub slug: String,
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

pub fn public_event_type(et: &EventType) -> PublicEventType {
    PublicEventType {
        name: et.name.clone(),
        duration_minutes: et.duration_minutes,
        buffer_minutes: et.buffer_minutes,
        color: et.color.clone(),
        slug: et.slug.clone(),
    }
}

pub fn validate_invitee(name: &str, email: &str) -> Result<(String, String)> {
    let name = name.trim();
    let email = email.trim().to_lowercase();
    if name.is_empty() {
        bail!("name is required");
    }
    let Some((local, domain)) = email.split_once('@') else {
        bail!("valid email is required");
    };
    if local.is_empty() || !domain.contains('.') || domain.ends_with('.') {
        bail!("valid email is required");
    }
    Ok((name.to_string(), email))
}

pub fn confirm_booking(
    conn: &mut Connection,
    settings: &Settings,
    event_type: &EventType,
    name: &str,
    email: &str,
    start: &str,
    end: &str,
) -> Result<i64> {
    if !event_type.enabled {
        bail!("event type is disabled");
    }
    let (name, email) = validate_invitee(name, email)?;
    let start_dt = parse_dt(start).ok_or_else(|| anyhow!("invalid start time"))?;
    let end_dt = parse_dt(end).ok_or_else(|| anyhow!("invalid end time"))?;
    if end_dt <= start_dt {
        bail!("invalid booking time");
    }
    let expected = event_type.duration_minutes.max(15);
    if (end_dt - start_dt).num_minutes() != expected {
        bail!("booking duration no longer matches this event type");
    }

    let slots = available_slots(conn, settings, event_type, settings.horizon_days.clamp(1, 60))?;
    let wanted = BookingSlot { start: fmt_dt(start_dt), end: fmt_dt(end_dt) };
    if !slots.iter().any(|slot| slot.start == wanted.start && slot.end == wanted.end) {
        bail!("that time is no longer available");
    }

    let title = format!("{}: {}", event_type.name, name);
    let booking_id = db::insert_booking(conn, event_type.id, &title, &name, &email, &wanted.start, &wanted.end)?;
    // Relationship layer: a booking creates (or links to) a person, deduped by email. Best-effort —
    // never fail a confirmed booking over a person-record hiccup.
    let _ = db::upsert_person_by_email(conn, &name, Some(&email));
    Ok(booking_id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scheduler::parse_dt;

    fn et(dur: i64, buf: i64) -> EventType {
        EventType {
            id: 1,
            name: "Call".into(),
            duration_minutes: dur,
            buffer_minutes: buf,
            color: "#000".into(),
            slug: "call-1".into(),
            share_token: "token".into(),
            enabled: true,
        }
    }

    fn inserted_et(conn: &Connection, dur: i64, buf: i64) -> EventType {
        let id = db::insert_event_type(conn, "Call", dur, buf, "#000").unwrap();
        db::get_event_type(conn, id).unwrap()
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

    #[test]
    fn confirm_booking_rejects_invalid_email() {
        let mut conn = db::test_conn();
        let settings = Settings::default();
        let event_type = inserted_et(&conn, 30, 0);
        let slot = available_slots(&conn, &settings, &event_type, 2).unwrap().remove(0);
        let err = confirm_booking(&mut conn, &settings, &event_type, "Ava", "not-email", &slot.start, &slot.end).unwrap_err();
        assert!(err.to_string().contains("email"));
    }

    #[test]
    fn confirm_booking_creates_booking_and_event() {
        let mut conn = db::test_conn();
        let settings = Settings::default();
        let event_type = inserted_et(&conn, 30, 0);
        let slot = available_slots(&conn, &settings, &event_type, 2).unwrap().remove(0);
        confirm_booking(&mut conn, &settings, &event_type, "Ava", "ava@example.com", &slot.start, &slot.end).unwrap();
        assert_eq!(db::list_bookings(&conn).unwrap().len(), 1);
        assert!(db::list_events(&conn).unwrap().iter().any(|e| e.title.contains("Ava")));
    }

    #[test]
    fn confirm_booking_creates_a_person_from_the_invitee() {
        let mut conn = db::test_conn();
        let settings = Settings::default();
        let event_type = inserted_et(&conn, 30, 0);
        let slot = available_slots(&conn, &settings, &event_type, 2).unwrap().remove(0);
        confirm_booking(&mut conn, &settings, &event_type, "Ava", "ava@example.com", &slot.start, &slot.end).unwrap();
        let people = db::list_people(&conn).unwrap();
        assert_eq!(people.len(), 1, "the invitee becomes a person");
        assert_eq!(people[0].name, "Ava");
        assert_eq!(people[0].email.as_deref(), Some("ava@example.com"));
    }

    #[test]
    fn confirm_booking_rejects_stale_slot() {
        let mut conn = db::test_conn();
        let settings = Settings::default();
        let event_type = inserted_et(&conn, 30, 0);
        let slot = available_slots(&conn, &settings, &event_type, 2).unwrap().remove(0);
        confirm_booking(&mut conn, &settings, &event_type, "Ava", "ava@example.com", &slot.start, &slot.end).unwrap();
        let err = confirm_booking(&mut conn, &settings, &event_type, "Bea", "bea@example.com", &slot.start, &slot.end).unwrap_err();
        assert!(err.to_string().contains("available"));
    }
}
