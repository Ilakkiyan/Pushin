# Focus & time-tracking

Pushin can track the time you actually spend on a task — and quietly use that to schedule you more realistically.

## Starting a focus session

In the task list, hover a task and press the **▶ play** button. A live `m:ss` timer replaces it; press **■ stop** when you're done. Only one focus session runs at a time — starting a new one stops the previous.

The timer survives navigation: if you switch views and come back, the running session is still there.

## Adaptive scheduling

Once you've completed a handful of focus-tracked tasks, Pushin compares how long they **actually** took to what they were **estimated** at, and applies a gentle correction factor to future task durations when it schedules them.

- It's a **soft** input: the factor stays at 1.0 (no change) until there's enough history, and it's clamped to a sensible range. Your stored estimates are never overwritten — only the scheduling pass is adjusted.
- It only kicks in after real usage, so a fresh install schedules exactly as you'd expect.

## Today's focus, at a glance

The [Daily Briefing](./briefing) shows how many minutes of focused work are already blocked on your calendar today.
