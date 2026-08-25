-- Read-only iCalendar (.ics) subscriptions: point Pushin at a feed URL and its events flow into the
-- calendar (marked provider='ics', never edited or pushed back). Events carry ics_sub_id so a feed's
-- events can be refreshed/removed as a unit.
CREATE TABLE IF NOT EXISTS ics_subscriptions (
    id          INTEGER PRIMARY KEY,
    name        TEXT NOT NULL,
    url         TEXT NOT NULL,
    color       TEXT NOT NULL DEFAULT '#64748b',
    last_synced TEXT,
    created_at  TEXT NOT NULL
);

ALTER TABLE events ADD COLUMN ics_sub_id INTEGER REFERENCES ics_subscriptions(id) ON DELETE CASCADE;

CREATE INDEX IF NOT EXISTS idx_events_ics_sub ON events(ics_sub_id);
