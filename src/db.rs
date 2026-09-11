//! SQLite schema and connection setup.
//!
//! Timestamps are unix milliseconds (`jiff::Timestamp::as_millisecond`) and booleans are 0/1,
//! so every column is a plain INTEGER that sorts and compares without parsing.

use std::path::Path;
use std::sync::LazyLock;

use anyhow::{Context, Result};
use rusqlite::Connection;
use rusqlite_migration::{M, Migrations};

/// The schema version arches writes. Recorded on every ingest run so an old row stays readable.
pub const SCHEMA_VERSION: i64 = 1;

/// Append-only: never edit a shipped migration, add a new one.
static MIGRATIONS: LazyLock<Migrations> = LazyLock::new(|| {
    Migrations::new(vec![M::up(
        r#"
CREATE TABLE places (
    id                         TEXT PRIMARY KEY,
    name                       TEXT NOT NULL,
    latitude                   REAL NOT NULL,
    longitude                  REAL NOT NULL,
    radius_mean                REAL,
    radius_sd                  REAL,
    street_address             TEXT,
    locality                   TEXT,
    country_code               TEXT,
    seconds_from_gmt           INTEGER,
    is_stale                   INTEGER,
    visit_count                INTEGER,
    visit_days                 INTEGER,
    last_visit_date            INTEGER,
    last_saved                 INTEGER NOT NULL,
    source                     TEXT,
    category                   TEXT,
    user_category              TEXT,
    mapbox_place_id            TEXT,
    mapbox_category            TEXT,
    mapbox_maki_icon           TEXT,
    google_place_id            TEXT,
    google_primary_type        TEXT,
    foursquare_place_id        TEXT,
    foursquare_category_id     INTEGER,
    foursquare_category_v2_id  TEXT
);

CREATE TABLE items (
    id                          TEXT PRIMARY KEY,
    is_visit                    INTEGER NOT NULL,
    start_date                  INTEGER NOT NULL,
    end_date                    INTEGER NOT NULL,
    last_saved                  INTEGER NOT NULL,
    source                      TEXT,
    source_version              TEXT,
    disabled                    INTEGER,
    -- Arc never drops history; a removed item is flagged here and stays in the export.
    deleted                     INTEGER,
    previous_item_id            TEXT,
    next_item_id                TEXT,
    locked                      INTEGER,
    step_count                  REAL,
    floors_ascended             REAL,
    floors_descended            REAL,
    average_altitude            REAL,
    active_energy_burned        REAL,
    average_heart_rate          REAL,
    max_heart_rate              REAL,
    visit_latitude              REAL,
    visit_longitude             REAL,
    visit_radius_mean           REAL,
    visit_radius_sd             REAL,
    visit_place_id              TEXT,
    visit_confirmed_place       INTEGER,
    visit_uncertain_place       INTEGER,
    visit_custom_title          TEXT,
    visit_street_address        TEXT,
    visit_locality              TEXT,
    visit_country_code          TEXT,
    trip_distance               REAL,
    trip_speed                  REAL,
    trip_classified_activity_type INTEGER,
    trip_confirmed_activity_type  INTEGER,
    trip_uncertain_activity_type  INTEGER,
    -- Resolved once on ingest: a user confirmation wins over the classifier. NULL for visits.
    activity_type               INTEGER
);
CREATE INDEX idx_items_start_date ON items(start_date);
CREATE INDEX idx_items_end_date ON items(end_date);
CREATE INDEX idx_items_visit_place_id ON items(visit_place_id);

CREATE TABLE samples (
    id                         TEXT PRIMARY KEY,
    date                       INTEGER NOT NULL,
    last_saved                 INTEGER NOT NULL,
    timeline_item_id           TEXT,
    seconds_from_gmt           INTEGER,
    moving_state               INTEGER,
    recording_state            INTEGER,
    disabled                   INTEGER,
    latitude                   REAL,
    longitude                  REAL,
    altitude                   REAL,
    horizontal_accuracy        REAL,
    vertical_accuracy          REAL,
    speed                      REAL,
    course                     REAL,
    step_hz                    REAL,
    heart_rate                 REAL,
    classified_activity_type   INTEGER,
    confirmed_activity_type    INTEGER
);
CREATE INDEX idx_samples_item_date ON samples(timeline_item_id, date);
CREATE INDEX idx_samples_date ON samples(date);

-- One row per bucket file seen, keyed by its path relative to the device dir. (mtime, len)
-- is what lets the next run skip a file that Arc has not rewritten.
CREATE TABLE ingest_files (
    path          TEXT PRIMARY KEY,
    device_id     TEXT NOT NULL,
    mtime         INTEGER NOT NULL,
    len           INTEGER NOT NULL,
    ingested_at   INTEGER NOT NULL,
    record_count  INTEGER NOT NULL
);

CREATE TABLE ingest_runs (
    id                INTEGER PRIMARY KEY AUTOINCREMENT,
    started_at        INTEGER NOT NULL,
    finished_at       INTEGER,
    device_id         TEXT,
    schema_version    INTEGER,
    last_backup_date  INTEGER,
    files_seen        INTEGER NOT NULL DEFAULT 0,
    files_ingested    INTEGER NOT NULL DEFAULT 0,
    places_upserted   INTEGER NOT NULL DEFAULT 0,
    items_upserted    INTEGER NOT NULL DEFAULT 0,
    samples_upserted  INTEGER NOT NULL DEFAULT 0,
    error             TEXT
);
"#,
    )])
});

pub fn open(path: &Path) -> Result<Connection> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating data dir {}", parent.display()))?;
    }
    prepare(Connection::open(path).with_context(|| format!("opening {}", path.display()))?)
}

#[cfg(test)]
pub fn open_in_memory() -> Result<Connection> {
    prepare(Connection::open_in_memory()?)
}

fn prepare(mut conn: Connection) -> Result<Connection> {
    conn.pragma_update(None, "journal_mode", "WAL")?;
    // Durable enough under WAL: a crash can lose the last transaction, which ingest re-does.
    conn.pragma_update(None, "synchronous", "NORMAL")?;
    // Buckets arrive in whatever order Arc wrote them, so samples routinely reference items
    // that are not ingested yet. Referential integrity is Arc's job, not ours.
    conn.pragma_update(None, "foreign_keys", "OFF")?;
    MIGRATIONS
        .to_latest(&mut conn)
        .context("applying migrations")?;
    Ok(conn)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn migrations_are_valid() {
        MIGRATIONS.validate().unwrap();
    }

    #[test]
    fn open_creates_the_parent_dir() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("nested").join("arches.db");
        let conn = open(&path).unwrap();

        assert!(path.exists());
        let mode: String = conn
            .pragma_query_value(None, "journal_mode", |row| row.get(0))
            .unwrap();
        assert_eq!(mode, "wal");
        let foreign_keys: i64 = conn
            .pragma_query_value(None, "foreign_keys", |row| row.get(0))
            .unwrap();
        assert_eq!(foreign_keys, 0);
    }

    #[test]
    fn in_memory_has_the_schema() {
        let conn = open_in_memory().unwrap();
        for table in ["places", "items", "samples", "ingest_files", "ingest_runs"] {
            let count: i64 = conn
                .query_row(&format!("SELECT count(*) FROM {table}"), [], |row| {
                    row.get(0)
                })
                .unwrap();
            assert_eq!(count, 0);
        }
    }
}
