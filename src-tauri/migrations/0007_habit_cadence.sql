-- Richer habit recurrence: weekly (specific weekdays) and interval ("every other day").
-- `days` is a CSV of weekday numbers (1=Mon..7=Sun) used when cadence='weekly'.
-- `interval_days` is the step used when cadence='interval' (2 = every other day; 1 otherwise).
ALTER TABLE habits ADD COLUMN days TEXT NOT NULL DEFAULT '';
ALTER TABLE habits ADD COLUMN interval_days INTEGER NOT NULL DEFAULT 1;
