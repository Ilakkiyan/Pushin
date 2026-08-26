-- Missed-task rollover bookkeeping.
--
-- When a task's planned time passes without it being done, the day-rollover sweep
-- (`schedule_service::sweep_missed`) drops the stale blocks and lets the scheduler re-place the
-- task in the next available slot. These two columns are the audit trail for that:
--   * `missed_count`  — how many days in a row it has been kicked forward. The "you keep not doing
--                       this" signal the UI shows as a "rolled over N×" chip.
--   * `last_missed_on` — the LOCAL DATE (YYYY-MM-DD) of the most recent sweep, which makes the
--                       sweep idempotent: reschedule runs many times a day, and the count must
--                       only move once per day no matter how often it runs.
ALTER TABLE tasks ADD COLUMN missed_count   INTEGER NOT NULL DEFAULT 0;
ALTER TABLE tasks ADD COLUMN last_missed_on TEXT;
