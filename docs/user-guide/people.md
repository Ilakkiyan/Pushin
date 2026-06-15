# People

Pushin keeps a private, on-device record of the people you work with — a lightweight CRM that the rest of the app feeds into.

## How people appear

- **Automatically from bookings.** When someone books time with you (see [Booking](./booking)), Pushin creates or updates a person record from their name and email — deduplicated by email, so re-bookings don't pile up.
- **Manually.** Open **People** in the sidebar and click **+** to add someone.

## The People pane

Select a person to edit their **name**, **email**, and **notes**, attach [labels](./labels), and see their **meeting history** (every booking with that email, most recent first). Notes are where you keep what you want to remember — "prefers mornings", "met at the conference", anything.

## People in recall

People are part of the [shared context](../developer-guide/architecture) Pushin searches. Asking your vault a question (via the [command palette](./planning)) can surface a person and their notes alongside your tasks, events, and pages — so "what did I tell Ava about the timeline?" can find it.

## Privacy

Like everything in Pushin, people records live only in your local database. Nothing is uploaded.
