-- Per-habit duration, used when slotting the habit onto the calendar.
ALTER TABLE habits ADD COLUMN duration_minutes INTEGER NOT NULL DEFAULT 30;
