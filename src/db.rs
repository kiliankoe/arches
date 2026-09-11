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
pub const SCHEMA_VERSION: i64 = 2;

/// Append-only: never edit a shipped migration, add a new one.
static MIGRATIONS: LazyLock<Migrations> = LazyLock::new(|| {
    Migrations::new(vec![
        M::up(
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
        ),
        M::up(
            r#"
-- Derived local-day columns. "Day" always means the local day the record happened in, taken
-- from the offset Arc recorded with it, never the UTC day: see README.md.
ALTER TABLE samples ADD COLUMN local_date TEXT;
-- Leading local_date makes this the day lookup index; the rest lets the day summary walk a
-- day's samples grouped per item and in time order without a sort.
CREATE INDEX idx_samples_local_date ON samples(local_date, timeline_item_id, date);

ALTER TABLE items ADD COLUMN start_offset_seconds INTEGER;
ALTER TABLE items ADD COLUMN end_offset_seconds INTEGER;
ALTER TABLE items ADD COLUMN local_start_date TEXT;
ALTER TABLE items ADD COLUMN local_end_date TEXT;
CREATE INDEX idx_items_local_start_date ON items(local_start_date);
CREATE INDEX idx_items_local_end_date ON items(local_end_date);

-- One row per local day that has anything in it. Gap days simply have no row; recording
-- stopped for months in 2025 and nothing may assume continuity.
CREATE TABLE day_summaries (
    date                TEXT PRIMARY KEY,
    -- The offset most of the day's samples used, i.e. where the day was actually spent.
    utc_offset_seconds  INTEGER,
    item_count          INTEGER NOT NULL,
    visit_count         INTEGER NOT NULL,
    trip_count          INTEGER NOT NULL,
    sample_count        INTEGER NOT NULL,
    distance_m          REAL NOT NULL,
    moving_seconds      INTEGER NOT NULL,
    -- JSON objects keyed by activity type name, metres and seconds. Visits count as
    -- "stationary" in duration_by_type and contribute no distance.
    distance_by_type    TEXT NOT NULL,
    duration_by_type    TEXT NOT NULL,
    -- JSON arrays, distinct, place ids in visit order.
    place_ids           TEXT NOT NULL,
    country_codes       TEXT NOT NULL,
    localities          TEXT NOT NULL,
    min_lat             REAL,
    min_lon             REAL,
    max_lat             REAL,
    max_lon             REAL,
    first_sample_at     INTEGER,
    last_sample_at      INTEGER,
    computed_at         INTEGER NOT NULL
);

ALTER TABLE ingest_runs ADD COLUMN days_recomputed INTEGER NOT NULL DEFAULT 0;

-- Backfill what plain SQL can do, so an existing database is queryable by local day right
-- after the migration. Item offsets and day summaries need `arches derive`.
UPDATE samples
   SET local_date = strftime('%Y-%m-%d', (date + seconds_from_gmt * 1000) / 1000, 'unixepoch')
 WHERE seconds_from_gmt IS NOT NULL;
"#,
        ),
    ])
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

    /// An existing database has to come out of the migration queryable by local day without
    /// waiting for `arches derive`.
    #[test]
    fn migration_two_backfills_sample_local_dates() {
        let mut conn = Connection::open_in_memory().unwrap();
        MIGRATIONS.to_version(&mut conn, 1).unwrap();
        conn.execute(
            "INSERT INTO samples (id, date, last_saved, seconds_from_gmt) VALUES
                 ('a', 1749592800000, 0, 7200),   -- 2025-06-10T22:40:00Z, 00:40 local
                 ('b', 1749592800000, 0, NULL)",
            [],
        )
        .unwrap();

        MIGRATIONS.to_latest(&mut conn).unwrap();

        let dates: Vec<Option<String>> = conn
            .prepare("SELECT local_date FROM samples ORDER BY id")
            .unwrap()
            .query_map([], |row| row.get(0))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        assert_eq!(dates, [Some("2025-06-11".to_string()), None]);
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
        for table in [
            "places",
            "items",
            "samples",
            "ingest_files",
            "ingest_runs",
            "day_summaries",
        ] {
            let count: i64 = conn
                .query_row(&format!("SELECT count(*) FROM {table}"), [], |row| {
                    row.get(0)
                })
                .unwrap();
            assert_eq!(count, 0);
        }
    }
}
