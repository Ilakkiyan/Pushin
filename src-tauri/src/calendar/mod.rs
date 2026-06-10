//! Calendar sync. Two-way Google Calendar sync lives in `google` (free functions). SQLite is
//! Pushin's source of truth, so there's no local-provider indirection.

pub mod google;
