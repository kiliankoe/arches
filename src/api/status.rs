//! `/api/status` and `POST /api/ingest`.

use std::sync::atomic::Ordering;

use anyhow::Result;
use axum::Json;
use axum::extract::State;
use rusqlite::Connection;
use serde::Serialize;
use serde_json::Value;

use super::AppState;
use super::convert::rfc3339;
use super::error::{ApiError, ApiResult};
use crate::status::{self, Counts};

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    version: &'static str,
    /// The last ingest run, with its dates as RFC 3339. Null before the first one.
    last_run: Option<Value>,
    /// What Arc last finished backing up, from the device's `metadata.json`.
    last_backup_date: Option<String>,
    /// The newest bucket mtime ingest has seen, which is how fresh the data can possibly be.
    newest_bucket_mtime: Option<String>,
    counts: Counts,
    first_summarized_date: Option<String>,
    last_summarized_date: Option<String>,
    ingest_running: bool,
}

pub async fn get(State(state): State<AppState>) -> ApiResult<Json<Status>> {
    let running = state.ingest_running.load(Ordering::Relaxed);
    let status = state.db.read(move |conn| build(conn, running)).await?;
    Ok(Json(status))
}

/// One pass now, for a consumer that has just been told the backup changed. The ingest lock
/// serializes it against the timer, so the worst a burst of calls costs is a queue.
pub async fn ingest(State(state): State<AppState>) -> ApiResult<Json<Value>> {
    let summary = state
        .run_ingest()
        .await
        .map_err(|error| ApiError::Internal(error.context("ingest failed")))?;
    Ok(Json(with_rfc3339_dates(&summary)))
}

fn build(conn: &Connection, ingest_running: bool) -> Result<Status> {
    let status = status::status(conn)?;
    let newest_bucket_mtime: Option<i64> =
        conn.query_row("SELECT max(mtime) FROM ingest_files", [], |row| row.get(0))?;

    Ok(Status {
        version: env!("CARGO_PKG_VERSION"),
        last_backup_date: status
            .last_run
            .as_ref()
            .and_then(|run| run.last_backup_date)
            .and_then(rfc3339),
        last_run: status.last_run.as_ref().map(with_rfc3339_dates),
        newest_bucket_mtime: newest_bucket_mtime.and_then(rfc3339),
        counts: status.counts,
        first_summarized_date: status.first_summarized_date,
        last_summarized_date: status.last_summarized_date,
        ingest_running,
    })
}

/// A run as the CLI serializes it, with its millisecond fields rewritten as RFC 3339: the API
/// never puts raw millis on the wire.
fn with_rfc3339_dates<T: Serialize>(run: &T) -> Value {
    let mut value = serde_json::to_value(run).unwrap_or(Value::Null);
    if let Some(object) = value.as_object_mut() {
        for key in ["startedAt", "finishedAt", "lastBackupDate"] {
            let millis = object.get(key).and_then(Value::as_i64);
            let rendered = millis.and_then(rfc3339).map(Value::String);
            object.insert(key.to_string(), rendered.unwrap_or(Value::Null));
        }
    }
    value
}
