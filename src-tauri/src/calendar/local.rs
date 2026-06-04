//! Local provider: SQLite is the source of truth, so push/update/delete are no-ops
//! and pulling events just reads the local table (filtered to the range).

use super::CalendarProvider;
use crate::db;
use crate::model::{Block, Event};
use anyhow::Result;
use rusqlite::Connection;

pub struct LocalProvider;

impl CalendarProvider for LocalProvider {
    fn name(&self) -> &'static str {
        "local"
    }

    fn pull_events(&self, conn: &Connection, range_start: &str, range_end: &str) -> Result<Vec<Event>> {
        let all = db::list_events(conn)?;
        Ok(all
            .into_iter()
            .filter(|e| e.end.as_str() >= range_start && e.start.as_str() <= range_end)
            .collect())
    }

    fn push_block(&self, _conn: &Connection, _block: &Block) -> Result<()> {
        Ok(())
    }

    fn update_block(&self, _conn: &Connection, _block: &Block) -> Result<()> {
        Ok(())
    }

    fn delete_block(&self, _conn: &Connection, _external_id: &str) -> Result<()> {
        Ok(())
    }
}
