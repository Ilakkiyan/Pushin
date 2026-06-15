//! Meeting Companion — the deterministic pre-meeting brief (ROADMAP Phase 4.3). Given a meeting
//! (event), it gathers who's attending (booked invitees → people), each attendee's relationship
//! history, and any notes linked to the meeting. Pure: no LLM, no I/O — the caller passes in the
//! bookings/people/pages it loaded, so this is fully testable. (LLM action-item extraction is a
//! separate, confirm-gated step built on top of this reliable core.)

use crate::model::{AttendeeBrief, Booking, Event, MeetingBrief, Page, Person};
use std::collections::HashSet;

/// Total meetings + most-recent meeting time across a person's bookings. ISO timestamps sort
/// chronologically, so `max` is the latest.
pub fn history_summary(bookings: &[&Booking]) -> (i64, Option<String>) {
    let last = bookings.iter().map(|b| b.start.clone()).max();
    (bookings.len() as i64, last)
}

/// Build the brief for `event` from the full booking/people/page sets. Attendees are the distinct
/// invitees booked into THIS event (first-seen order); each is matched to a person record by email,
/// falling back to a transient person from the booking when no record exists.
pub fn assemble(event: &Event, bookings: &[Booking], people: &[Person], linked_pages: Vec<Page>) -> MeetingBrief {
    let mut seen: HashSet<String> = HashSet::new();
    let mut attendees = Vec::new();

    for b in bookings.iter().filter(|b| b.event_id == Some(event.id)) {
        let email = b.invitee_email.trim().to_lowercase();
        if email.is_empty() || !seen.insert(email.clone()) {
            continue;
        }
        let person = people
            .iter()
            .find(|p| p.email.as_deref().map(str::to_lowercase) == Some(email.clone()))
            .cloned()
            .unwrap_or_else(|| Person {
                id: -1,
                name: b.invitee_name.clone(),
                email: Some(b.invitee_email.clone()),
                notes: String::new(),
                created_at: String::new(),
                updated_at: String::new(),
            });
        let theirs: Vec<&Booking> = bookings.iter().filter(|x| x.invitee_email.trim().to_lowercase() == email).collect();
        let (total_meetings, last_met) = history_summary(&theirs);
        attendees.push(AttendeeBrief { person, total_meetings, last_met });
    }

    MeetingBrief { event: event.clone(), attendees, linked_pages }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event(id: i64, title: &str) -> Event {
        Event {
            id,
            title: title.into(),
            start: "2026-06-15T14:00:00".into(),
            end: "2026-06-15T14:30:00".into(),
            kind: "fixed".into(),
            source: "manual".into(),
            created_at: String::new(),
            provider: None,
            external_id: None,
            account_id: None,
            etag: None,
        }
    }

    fn booking(event_id: Option<i64>, email: &str, name: &str, start: &str) -> Booking {
        Booking {
            id: 0,
            event_type_id: 1,
            event_id,
            invitee_name: name.into(),
            invitee_email: email.into(),
            start: start.into(),
            end: start.into(),
            status: "confirmed".into(),
            created_at: String::new(),
        }
    }

    fn person(id: i64, name: &str, email: &str, notes: &str) -> Person {
        Person { id, name: name.into(), email: Some(email.into()), notes: notes.into(), created_at: String::new(), updated_at: String::new() }
    }

    #[test]
    fn assembles_attendees_with_history_and_dedupes() {
        let ev = event(7, "Intro call");
        let bookings = vec![
            booking(Some(7), "ava@x.io", "Ava", "2026-06-15T14:00:00"), // this meeting
            booking(Some(3), "ava@x.io", "Ava", "2026-05-01T09:00:00"), // an earlier meeting (other event)
            booking(Some(7), "ava@x.io", "Ava", "2026-06-15T14:00:00"), // dup invitee on this event → one attendee
            booking(Some(7), "  ", "Anon", "2026-06-15T14:00:00"),      // blank email → skipped
            booking(Some(9), "bob@x.io", "Bob", "2026-04-01T10:00:00"), // not this event → not an attendee
        ];
        let people = vec![person(1, "Ava Stone", "ava@x.io", "prefers mornings")];

        let brief = assemble(&ev, &bookings, &people, vec![]);
        assert_eq!(brief.attendees.len(), 1, "one distinct attendee on this event");
        let a = &brief.attendees[0];
        assert_eq!(a.person.name, "Ava Stone", "matched to the person record, not the booking name");
        assert_eq!(a.person.notes, "prefers mornings");
        assert_eq!(a.total_meetings, 3, "all of Ava's bookings count toward history");
        assert_eq!(a.last_met.as_deref(), Some("2026-06-15T14:00:00"), "latest meeting time");
    }

    #[test]
    fn falls_back_to_booking_when_no_person_record() {
        let ev = event(7, "Call");
        let bookings = vec![booking(Some(7), "new@x.io", "New Person", "2026-06-15T14:00:00")];
        let brief = assemble(&ev, &bookings, &[], vec![]);
        assert_eq!(brief.attendees.len(), 1);
        assert_eq!(brief.attendees[0].person.id, -1, "transient person");
        assert_eq!(brief.attendees[0].person.name, "New Person");
    }
}
