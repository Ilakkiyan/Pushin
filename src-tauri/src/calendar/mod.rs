//! Calendar provider abstraction.
//!
//! Everything in Pushin reads/writes through a `CalendarProvider`. Today only
//! `LocalProvider` (SQLite-backed) is wired in. `GoogleProvider` is a stub that
//! will mirror blocks/events to Google Calendar — the `events`/`blocks` tables
//! already carry `provider`/`external_id`/`etag` columns so enabling it is additive.

pub mod google;
pub mod local;

use crate::model::{Block, Event};
use anyhow::Result;
use rusqlite::Connection;

// `push_block`/`update_block`/`delete_block` are seam methods used once GoogleProvider
// is fleshed out; allowed-dead-code keeps the intentional API without noise.
#[allow(dead_code)]
pub trait CalendarProvider {
    fn name(&self) -> &'static str;

    /// Pull events within [range_start, range_end] (ISO) into Pushin.
    fn pull_events(&self, conn: &Connection, range_start: &str, range_end: &str) -> Result<Vec<Event>>;

    /// Mirror a newly created block out to the provider.
    fn push_block(&self, conn: &Connection, block: &Block) -> Result<()>;

    /// Reflect a moved/changed block.
    fn update_block(&self, conn: &Connection, block: &Block) -> Result<()>;

    /// Remove a block that no longer exists locally.
    fn delete_block(&self, conn: &Connection, external_id: &str) -> Result<()>;
}
