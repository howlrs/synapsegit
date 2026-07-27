mod decision_intents;
mod outcomes;
mod proposal_intents;
mod reviews;

use std::fmt;
use std::path::Path;
use std::time::Duration;

use rusqlite::{Connection, TransactionBehavior};

use crate::error::{JournalError, Result};
use crate::schema::{SCHEMA_VERSION, create_schema_v2, migrate_schema_v1_to_v2};

/// SQLite-backed review journal. This type is storage, never authority.
pub struct SqliteReviewJournal {
    pub(crate) connection: Connection,
}

impl fmt::Debug for SqliteReviewJournal {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SqliteReviewJournal")
            .finish_non_exhaustive()
    }
}

impl SqliteReviewJournal {
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        Self::initialize(Connection::open(path)?)
    }

    pub fn open_in_memory() -> Result<Self> {
        Self::initialize(Connection::open_in_memory()?)
    }

    fn initialize(mut connection: Connection) -> Result<Self> {
        connection.busy_timeout(Duration::from_secs(5))?;
        connection.execute_batch("PRAGMA foreign_keys = ON;")?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let version =
            transaction.query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))?;
        match version {
            0 => create_schema_v2(&transaction)?,
            1 => migrate_schema_v1_to_v2(&transaction)?,
            SCHEMA_VERSION => {}
            _ => {
                return Err(JournalError::CorruptData(format!(
                    "unsupported schema version {version}"
                )));
            }
        }
        transaction.commit()?;
        Ok(Self { connection })
    }
}
