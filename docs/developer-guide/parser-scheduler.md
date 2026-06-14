# Parser and Scheduler

Pushin's planning pipeline deliberately separates extraction from scheduling.

## Parser

The model returns structured events, tasks, edits, removals, habits, and clarifications. Rust then:

- resolves relative days and dates
- backfills obvious task fields
- filters noisy clarifications
- deduplicates duplicate model output
- stores the final plan

## Scheduler

The scheduler considers:

- fixed events
- locked task blocks
- working hours
- routines and sleep
- task deadlines
- dependencies
- labels with scheduling preferences

Label time windows are soft preferences. If preferred time does not fit, the scheduler falls back to available time.
