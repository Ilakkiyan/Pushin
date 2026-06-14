-- Public booking links and cancellation support.
ALTER TABLE event_types ADD COLUMN slug TEXT NOT NULL DEFAULT '';
ALTER TABLE event_types ADD COLUMN share_token TEXT NOT NULL DEFAULT '';
ALTER TABLE event_types ADD COLUMN enabled INTEGER NOT NULL DEFAULT 1;

ALTER TABLE bookings ADD COLUMN event_id INTEGER REFERENCES events(id) ON DELETE SET NULL;
CREATE INDEX IF NOT EXISTS idx_event_types_share ON event_types(share_token, slug);
CREATE INDEX IF NOT EXISTS idx_bookings_event ON bookings(event_id);
