-- A habit's learned preferred time-of-day, in minutes since midnight. NULL = no preference yet, so
-- the scheduler best-fits the habit into any free gap. Set when the user drags a habit on the
-- calendar (which also re-places its future occurrences there) — the app "learning" when you like to
-- do each habit. Syncs like the rest of the row (the 0015 change-capture reads columns dynamically).
ALTER TABLE habits ADD COLUMN preferred_minute INTEGER;
