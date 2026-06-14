# Planning with AI

Use the chat panel to describe events, tasks, edits, and routines in plain language.

Examples:

- "Lunch with mom Friday 12-2 and a party from 6-10."
- "Study for the exam, about 4 hours, due Thursday."
- "Move the dentist appointment to tomorrow at 3pm."
- "Make the meeting today 2 hours instead of 1."

Pushin sends the request to the local model, stores the structured result in SQLite, then re-runs the scheduler.

## Memory and Labels

Pushin can recall relevant vault notes while planning. If it notices a durable fact or a likely label, it offers a confirmation chip before saving anything.

## Boundaries

The model extracts intent; it does not do calendar math. Dates, durations, dependencies, conflicts, and task placement are resolved by the Rust backend.
