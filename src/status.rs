//! What the database has to say about itself: the last ingest run and what it left behind.

use std::collections::HashMap;

use anyhow::Result;
use rusqlite::{Connection, OptionalExtension};
use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    /// `None` before the first run. `finished_at` unset means a run is in flight or crashed.
    pub last_run: Option<RunRow>,
    pub counts: Counts,
    pub first_item_start: Option<i64>,
    pub last_item_start: Option<i64>,
    /// Local `YYYY-MM-DD`. The range is not continuous: gap days have no summary at all.
    pub first_summarized_date: Option<String>,
    pub last_summarized_date: Option<String>,
    pub last_ingested_at: Option<i64>,
    /// How much of the database each source contributed, most items first. Arc's own recording
    /// is `LocoKit2`; imported GPX history carries the creator string of its files.
    pub by_source: Vec<SourceCounts>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceCounts {
    /// `None` for rows old enough to predate the column.
    pub source: Option<String>,
    pub items: i64,
    pub samples: i64,
    pub first_item_start: Option<i64>,
    pub last_item_start: Option<i64>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RunRow {
    pub id: i64,
    pub started_at: i64,
    pub finished_at: Option<i64>,
    pub device_id: Option<String>,
    pub schema_version: Option<i64>,
    pub last_backup_date: Option<i64>,
    pub files_seen: i64,
    pub files_ingested: i64,
    pub places_upserted: i64,
    pub items_upserted: i64,
    pub samples_upserted: i64,
    pub days_recomputed: i64,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Counts {
    pub places: i64,
    pub items: i64,
    pub samples: i64,
    pub files: i64,
    pub day_summaries: i64,
    pub heatmap_cells: i64,
}

pub fn status(conn: &Connection) -> Result<Status> {
    let last_run = conn
        .query_row(
            "SELECT id, started_at, finished_at, device_id, schema_version, last_backup_date,
                    files_seen, files_ingested, places_upserted, items_upserted,
                    samples_upserted, days_recomputed, error
             FROM ingest_runs ORDER BY id DESC LIMIT 1",
            [],
            |row| {
                Ok(RunRow {
                    id: row.get(0)?,
                    started_at: row.get(1)?,
                    finished_at: row.get(2)?,
                    device_id: row.get(3)?,
                    schema_version: row.get(4)?,
                    last_backup_date: row.get(5)?,
                    files_seen: row.get(6)?,
                    files_ingested: row.get(7)?,
                    places_upserted: row.get(8)?,
                    items_upserted: row.get(9)?,
                    samples_upserted: row.get(10)?,
                    days_recomputed: row.get(11)?,
                    error: row.get(12)?,
                })
            },
        )
        .optional()?;

    let count = |table: &str| -> Result<i64> {
        Ok(
            conn.query_row(&format!("SELECT count(*) FROM {table}"), [], |row| {
                row.get(0)
            })?,
        )
    };
    let counts = Counts {
        places: count("places")?,
        items: count("items")?,
        samples: count("samples")?,
        files: count("ingest_files")?,
        day_summaries: count("day_summaries")?,
        heatmap_cells: count("heatmap_cells")?,
    };

    let (first_item_start, last_item_start) = conn.query_row(
        "SELECT min(start_date), max(start_date) FROM items",
        [],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    let (first_summarized_date, last_summarized_date) = conn.query_row(
        "SELECT min(date), max(date) FROM day_summaries",
        [],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    let last_ingested_at =
        conn.query_row("SELECT max(ingested_at) FROM ingest_files", [], |row| {
            row.get(0)
        })?;

    Ok(Status {
        last_run,
        counts,
        first_item_start,
        last_item_start,
        first_summarized_date,
        last_summarized_date,
        last_ingested_at,
        by_source: by_source(conn)?,
    })
}

/// Items and samples per source. The two are counted separately rather than joined: a sample
/// carries its own source, and joining millions of them to their items to find out would cost
/// far more than a status call is worth.
fn by_source(conn: &Connection) -> Result<Vec<SourceCounts>> {
    let mut samples: HashMap<Option<String>, i64> = conn
        .prepare("SELECT source, count(*) FROM samples GROUP BY source")?
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
        .collect::<Result<_, _>>()?;

    let mut by_source: Vec<SourceCounts> = conn
        .prepare(
            "SELECT source, count(*), min(start_date), max(start_date)
             FROM items GROUP BY source",
        )?
        .query_map([], |row| {
            Ok(SourceCounts {
                source: row.get(0)?,
                items: row.get(1)?,
                samples: 0,
                first_item_start: row.get(2)?,
                last_item_start: row.get(3)?,
            })
        })?
        .collect::<Result<_, _>>()?;

    for counts in &mut by_source {
        counts.samples = samples.remove(&counts.source).unwrap_or(0);
    }
    // A source with samples but no items at all still has to show up.
    by_source.extend(samples.into_iter().map(|(source, samples)| SourceCounts {
        source,
        items: 0,
        samples,
        first_item_start: None,
        last_item_start: None,
    }));
    by_source.sort_by(|a, b| b.items.cmp(&a.items).then_with(|| a.source.cmp(&b.source)));
    Ok(by_source)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;

    #[test]
    fn empty_database_reports_no_run() {
        let conn = db::open_in_memory().unwrap();
        let status = status(&conn).unwrap();

        assert!(status.last_run.is_none());
        assert_eq!(status.counts.items, 0);
        assert!(status.first_item_start.is_none());
        assert_eq!(status.counts.day_summaries, 0);
        assert!(status.first_summarized_date.is_none());
        assert!(status.last_ingested_at.is_none());
    }

    #[test]
    fn counts_rows_per_source() {
        let conn = db::open_in_memory().unwrap();
        conn.execute(
            "INSERT INTO items (id, is_visit, start_date, end_date, last_saved, source) VALUES
                 ('a', 0, 10, 20, 0, 'LocoKit2'),
                 ('b', 0, 30, 40, 0, 'LocoKit2'),
                 ('c', 1, 5, 8, 0, 'quantified-map-gpx')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO samples (id, date, last_saved, source) VALUES
                 ('s1', 10, 0, 'LocoKit2'),
                 ('s2', 5, 0, 'quantified-map-gpx'),
                 ('s3', 6, 0, 'quantified-map-gpx'),
                 ('s4', 7, 0, NULL)",
            [],
        )
        .unwrap();

        let by_source = status(&conn).unwrap().by_source;

        assert_eq!(by_source.len(), 3);
        assert_eq!(by_source[0].source.as_deref(), Some("LocoKit2"));
        assert_eq!((by_source[0].items, by_source[0].samples), (2, 1));
        assert_eq!(by_source[0].first_item_start, Some(10));
        assert_eq!(by_source[0].last_item_start, Some(30));
        assert_eq!(by_source[1].source.as_deref(), Some("quantified-map-gpx"));
        assert_eq!((by_source[1].items, by_source[1].samples), (1, 2));
        // A sample from before the column existed still has to be counted somewhere.
        assert_eq!(by_source[2].source, None);
        assert_eq!((by_source[2].items, by_source[2].samples), (0, 1));
    }

    #[test]
    fn reports_the_newest_run() {
        let conn = db::open_in_memory().unwrap();
        conn.execute(
            "INSERT INTO ingest_runs (started_at, finished_at, error) VALUES (1, 2, 'boom')",
            [],
        )
        .unwrap();
        conn.execute("INSERT INTO ingest_runs (started_at) VALUES (10)", [])
            .unwrap();

        let last_run = status(&conn).unwrap().last_run.unwrap();
        assert_eq!(last_run.started_at, 10);
        assert!(last_run.finished_at.is_none());
        assert!(last_run.error.is_none());
    }
}
