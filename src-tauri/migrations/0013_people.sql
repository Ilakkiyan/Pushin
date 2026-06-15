-- People — the relationship layer (private CRM). First-class entities auto-created from booking
-- invitees (and later event attendees / [[mentions]]), so the booking flow feeds the rest of the app.
-- Indexed for cross-entity recall as EntityKind::Person (already in the model).
CREATE TABLE IF NOT EXISTS people (
  id         INTEGER PRIMARY KEY AUTOINCREMENT,
  name       TEXT NOT NULL,
  email      TEXT,
  notes      TEXT NOT NULL DEFAULT '',
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL
);
-- Dedupe by email (when present) so re-bookings don't create duplicate people.
CREATE UNIQUE INDEX IF NOT EXISTS idx_people_email ON people(email) WHERE email IS NOT NULL AND email <> '';
