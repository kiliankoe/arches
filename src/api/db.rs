//! Database access for request handlers.
//!
//! `rusqlite::Connection` is neither `Sync` nor async, and an ingest pass holds a write
//! transaction for as long as a bucket file takes to upsert. So reads never touch the ingest
//! connection: they take their own from a small pool of read-only ones, which WAL lets run
//! concurrently with the writer, and every rusqlite call happens on a blocking thread.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};

use anyhow::{Context, Result};
use rusqlite::Connection;

/// Connections kept around between requests. More than this many concurrent readers is fine,
/// the extras just open and close their own.
const MAX_IDLE: usize = 4;

pub struct Db {
    path: PathBuf,
    idle: Mutex<Vec<Connection>>,
}

impl Db {
    pub fn new(path: &Path) -> Arc<Self> {
        Arc::new(Self {
            path: path.to_path_buf(),
            idle: Mutex::new(Vec::new()),
        })
    }

    /// Run `work` against a reader on a blocking thread.
    pub async fn read<T, F>(self: &Arc<Self>, work: F) -> Result<T>
    where
        F: FnOnce(&Connection) -> Result<T> + Send + 'static,
        T: Send + 'static,
    {
        let db = Arc::clone(self);
        tokio::task::spawn_blocking(move || {
            let conn = db.checkout()?;
            let result = work(&conn);
            db.check_in(conn);
            result
        })
        .await
        .context("database task panicked")?
    }

    fn checkout(&self) -> Result<Connection> {
        if let Some(conn) = self.lock_idle().pop() {
            return Ok(conn);
        }
        let conn = Connection::open(&self.path)
            .with_context(|| format!("opening {}", self.path.display()))?;
        // Belt and braces: a handler has no business writing, and the schema belongs to ingest.
        conn.pragma_update(None, "query_only", "ON")?;
        // A checkpointing writer can hold the database briefly; waiting beats failing.
        conn.busy_timeout(std::time::Duration::from_secs(5))?;
        Ok(conn)
    }

    fn check_in(&self, conn: Connection) {
        let mut idle = self.lock_idle();
        if idle.len() < MAX_IDLE {
            idle.push(conn);
        }
    }

    fn lock_idle(&self) -> std::sync::MutexGuard<'_, Vec<Connection>> {
        // A poisoned pool only means a handler panicked; the connections in it are still fine.
        self.idle.lock().unwrap_or_else(PoisonError::into_inner)
    }
}
