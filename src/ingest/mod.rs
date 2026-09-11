//! One ingest pass over Arc's backup: mirror changed bucket files into the data dir, upsert
//! their records, and record what happened in `ingest_files` and `ingest_runs`.
//!
//! Arc's folder is only ever read: see README.md.

mod mirror;
mod upsert;

use std::collections::BTreeSet;
use std::sync::{Mutex, PoisonError};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use jiff::Timestamp;
use jiff::civil::Date;
use rusqlite::{Connection, OptionalExtension, params};
use serde::Serialize;

use crate::arc::backup::{Backup, BucketFile, BucketKind};
use crate::arc::backup::{read_items, read_places, read_samples};
use crate::config::Config;
use crate::db::SCHEMA_VERSION;
use crate::derive;

/// Ingest is not re-entrant: two passes would race on the same bucket files and run counters.
/// Phase 4's timer takes the same lock as the manual trigger.
static RUN_LOCK: Mutex<()> = Mutex::new(());

/// The `ingest_runs` row for one pass, plus how long it took. Serialized by the CLI today and
/// by `/api/status` in phase 4.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RunSummary {
    pub id: i64,
    pub started_at: i64,
    pub finished_at: i64,
    pub device_id: Option<String>,
    pub schema_version: i64,
    pub last_backup_date: Option<i64>,
    pub files_seen: i64,
    pub files_ingested: i64,
    pub places_upserted: i64,
    pub items_upserted: i64,
    pub samples_upserted: i64,
    pub days_recomputed: i64,
    pub error: Option<String>,
    pub elapsed_ms: i64,
}

#[derive(Default)]
struct Counters {
    files_seen: i64,
    files_ingested: i64,
    places: i64,
    items: i64,
    samples: i64,
    days: i64,
}

struct RunState {
    id: i64,
    started_at: Timestamp,
    clock: Instant,
    device_id: Option<String>,
    last_backup_date: Option<Timestamp>,
    counters: Counters,
    errors: Vec<String>,
    /// Every item in a changed month bucket, plus every item a changed sample bucket points at.
    touched_items: BTreeSet<String>,
    /// The local days changed sample buckets landed on.
    touched_dates: BTreeSet<String>,
}

pub fn run(conn: &mut Connection, config: &Config) -> Result<RunSummary> {
    // A poisoned lock only means an earlier run panicked; the next pass re-reads everything
    // it needs from the database anyway.
    let _guard = RUN_LOCK.lock().unwrap_or_else(PoisonError::into_inner);

    let started_at = Timestamp::now();
    let id = conn
        .query_row(
            "INSERT INTO ingest_runs (started_at, schema_version) VALUES (?, ?) RETURNING id",
            params![started_at.as_millisecond(), SCHEMA_VERSION],
            |row| row.get(0),
        )
        .context("recording ingest run start")?;
    let mut state = RunState {
        id,
        started_at,
        clock: Instant::now(),
        device_id: None,
        last_backup_date: None,
        counters: Counters::default(),
        errors: Vec::new(),
        touched_items: BTreeSet::new(),
        touched_dates: BTreeSet::new(),
    };

    // A fatal error still closes the run row, so `arches status` shows why a pass stopped.
    let outcome = ingest_all(conn, config, &mut state);
    if let Err(error) = &outcome {
        state.errors.push(format!("{error:#}"));
    }
    let summary = state.finish(conn)?;
    outcome?;

    tracing::info!(
        files_seen = summary.files_seen,
        files_ingested = summary.files_ingested,
        places = summary.places_upserted,
        items = summary.items_upserted,
        samples = summary.samples_upserted,
        days = summary.days_recomputed,
        elapsed_ms = summary.elapsed_ms,
        "ingest finished"
    );
    Ok(summary)
}

fn ingest_all(conn: &mut Connection, config: &Config, state: &mut RunState) -> Result<()> {
    let backup = Backup::discover(&config.arc_dir)?;
    let metadata = backup.metadata()?;
    state.device_id = Some(backup.device_id.clone());
    state.last_backup_date = metadata.last_backup_date;
    tracing::info!(
        device_id = %backup.device_id,
        schema_version = %metadata.schema_version,
        last_backup_date = ?metadata.last_backup_date,
        "ingesting Arc backup"
    );

    // Places before items before samples, so a foreign key is usually already there to join to
    // even though the schema does not enforce one.
    let mut files = backup.place_files()?;
    files.extend(backup.item_files()?);
    files.extend(backup.sample_files()?);
    state.counters.files_seen = files.len() as i64;

    for file in &files {
        match ingest_file(conn, config, &backup.device_id, file, state) {
            Ok(()) => {}
            // One unreadable or half-written bucket must not cost the run the other 200.
            Err(error) => {
                tracing::error!(path = %file.path.display(), error = %format!("{error:#}"), "bucket file failed");
                state
                    .errors
                    .push(format!("{}: {error:#}", relative_path(file)));
            }
        }
    }

    // Derivation runs once for the whole pass rather than per file: a trip's item bucket and
    // its sample buckets are separate files, and only the union of them is a complete picture.
    derive_touched(conn, state)
}

fn derive_touched(conn: &mut Connection, state: &mut RunState) -> Result<()> {
    if state.touched_items.is_empty() && state.touched_dates.is_empty() {
        return Ok(());
    }
    let clock = Instant::now();

    let ids: Vec<String> = state.touched_items.iter().cloned().collect();
    derive::derive_items(conn, &ids).context("deriving item offsets")?;

    let mut dates = std::mem::take(&mut state.touched_dates);
    dates.extend(padded_item_dates(conn, &ids)?);
    let dates: Vec<String> = dates.into_iter().collect();
    let days = derive::recompute_days(conn, &dates).context("recomputing day summaries")?;

    state.counters.days = days as i64;
    tracing::info!(
        items = ids.len(),
        days_considered = dates.len(),
        days_recomputed = days,
        elapsed_ms = clock.elapsed().as_millis() as i64,
        "derivation finished"
    );
    Ok(())
}

/// Every local day the touched items cover, padded by one day on each side: a changed offset
/// can move an item across a midnight and leave the day it used to be on stale.
fn padded_item_dates(conn: &Connection, ids: &[String]) -> Result<BTreeSet<String>> {
    let mut statement = conn.prepare(
        "SELECT local_start_date, local_end_date FROM items
         WHERE id = ? AND local_start_date IS NOT NULL AND local_end_date IS NOT NULL",
    )?;
    let mut dates = BTreeSet::new();
    for id in ids {
        let span = statement
            .query_row(params![id], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })
            .optional()?;
        let Some((from, to)) = span else { continue };
        let (Ok(from), Ok(to)) = (from.parse::<Date>(), to.parse::<Date>()) else {
            continue;
        };
        let from = from.yesterday().unwrap_or(from);
        let to = to.tomorrow().unwrap_or(to);
        dates.extend(
            derive::dates_in_range(from, to)
                .into_iter()
                .map(|date| date.to_string()),
        );
    }
    Ok(dates)
}

fn ingest_file(
    conn: &mut Connection,
    config: &Config,
    device_id: &str,
    file: &BucketFile,
    state: &mut RunState,
) -> Result<()> {
    let relative = relative_path(file);
    let mtime = system_time_millis(file.mtime);
    let len = file.len as i64;

    let seen: Option<(String, i64, i64)> = conn
        .query_row(
            "SELECT device_id, mtime, len FROM ingest_files WHERE path = ?",
            params![relative],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()?;
    // A different device means a different backup lineage, so nothing stored about the old one
    // says anything about this file.
    if let Some((seen_device, seen_mtime, seen_len)) = seen
        && seen_device == device_id
        && seen_mtime == mtime
        && seen_len == len
    {
        tracing::debug!(path = %relative, "unchanged bucket file, skipping");
        return Ok(());
    }

    let destination = mirror::mirror_path(&config.data_dir, device_id, &relative);
    mirror::copy_into_mirror(&file.path, &destination)?;

    let tx = conn.transaction()?;
    let record_count = match file.kind {
        BucketKind::Places => {
            let places = read_places(&destination)?;
            state.counters.places += upsert::upsert_places(&tx, &places)? as i64;
            places.len()
        }
        BucketKind::Items => {
            let items = read_items(&destination)?;
            state.counters.items += upsert::upsert_items(&tx, &items)? as i64;
            // Every item in the bucket, not just the ones whose `lastSaved` won: a rewritten
            // bucket can change a neighbour's offset without changing the item itself.
            state
                .touched_items
                .extend(items.iter().map(|item| item.base.id.clone()));
            items.len()
        }
        BucketKind::Samples => {
            let samples = read_samples(&destination)?;
            state.counters.samples += upsert::upsert_samples(&tx, &samples)? as i64;
            for sample in &samples {
                if let Some(item_id) = &sample.timeline_item_id {
                    state.touched_items.insert(item_id.clone());
                }
                if let Some(offset) = sample.seconds_from_gmt {
                    state
                        .touched_dates
                        .insert(derive::local_date(sample.date, offset));
                }
            }
            samples.len()
        }
    };

    // Same transaction as the records: a crash between the two would either re-ingest the file
    // or, worse, skip it forever.
    tx.execute(
        "INSERT INTO ingest_files (path, device_id, mtime, len, ingested_at, record_count)
         VALUES (?, ?, ?, ?, ?, ?)
         ON CONFLICT(path) DO UPDATE SET
             device_id = excluded.device_id, mtime = excluded.mtime, len = excluded.len,
             ingested_at = excluded.ingested_at, record_count = excluded.record_count",
        params![
            relative,
            device_id,
            mtime,
            len,
            Timestamp::now().as_millisecond(),
            record_count as i64,
        ],
    )?;
    tx.commit()?;

    state.counters.files_ingested += 1;
    tracing::debug!(path = %relative, record_count, "ingested bucket file");
    Ok(())
}

impl RunState {
    fn finish(self, conn: &Connection) -> Result<RunSummary> {
        let finished_at = Timestamp::now();
        let error = (!self.errors.is_empty()).then(|| self.errors.join("\n"));
        let summary = RunSummary {
            id: self.id,
            started_at: self.started_at.as_millisecond(),
            finished_at: finished_at.as_millisecond(),
            device_id: self.device_id,
            schema_version: SCHEMA_VERSION,
            last_backup_date: self.last_backup_date.map(Timestamp::as_millisecond),
            files_seen: self.counters.files_seen,
            files_ingested: self.counters.files_ingested,
            places_upserted: self.counters.places,
            items_upserted: self.counters.items,
            samples_upserted: self.counters.samples,
            days_recomputed: self.counters.days,
            error,
            elapsed_ms: self.clock.elapsed().as_millis() as i64,
        };

        conn.execute(
            "UPDATE ingest_runs SET
                 finished_at = ?, device_id = ?, last_backup_date = ?, files_seen = ?,
                 files_ingested = ?, places_upserted = ?, items_upserted = ?,
                 samples_upserted = ?, days_recomputed = ?, error = ?
             WHERE id = ?",
            params![
                summary.finished_at,
                summary.device_id,
                summary.last_backup_date,
                summary.files_seen,
                summary.files_ingested,
                summary.places_upserted,
                summary.items_upserted,
                summary.samples_upserted,
                summary.days_recomputed,
                summary.error,
                summary.id,
            ],
        )
        .context("recording ingest run result")?;
        Ok(summary)
    }
}

/// The key in `ingest_files`: path relative to the device dir, e.g. `samples/2026-W36.json.gz`.
fn relative_path(file: &BucketFile) -> String {
    let name = file.path.file_name().unwrap_or_default().to_string_lossy();
    format!("{}/{name}", file.kind.dir_name())
}

fn system_time_millis(time: SystemTime) -> i64 {
    match time.duration_since(UNIX_EPOCH) {
        Ok(since) => since.as_millis() as i64,
        // Pre-epoch mtimes are nonsense from a broken clock, but must still compare stably.
        Err(error) => -(error.duration().as_millis() as i64),
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::time::Duration;

    use super::*;
    use crate::db;

    const DEVICE: &str = "11111111-2222-4333-8444-555555555555";

    fn fixture_arc_dir() -> PathBuf {
        PathBuf::from(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/backup"
        ))
    }

    /// The fixtures are checked in read-only in spirit: every test works on a copy so it can
    /// rewrite buckets and bump mtimes without touching the tree in git.
    fn copy_tree(from: &Path, to: &Path) {
        fs::create_dir_all(to).unwrap();
        for entry in fs::read_dir(from).unwrap() {
            let entry = entry.unwrap();
            let target = to.join(entry.file_name());
            if entry.file_type().unwrap().is_dir() {
                copy_tree(&entry.path(), &target);
            } else {
                fs::copy(entry.path(), &target).unwrap();
            }
        }
    }

    struct Fixture {
        _temp: tempfile::TempDir,
        arc_dir: PathBuf,
        config: Config,
        conn: Connection,
    }

    impl Fixture {
        fn new() -> Self {
            let temp = tempfile::tempdir().unwrap();
            let arc_dir = temp.path().join("arc");
            let data_dir = temp.path().join("data");
            copy_tree(&fixture_arc_dir(), &arc_dir);
            let config = Config::from_pairs([
                ("ARCHES_ARC_DIR", arc_dir.to_str().unwrap()),
                ("ARCHES_DATA_DIR", data_dir.to_str().unwrap()),
            ])
            .unwrap();
            let conn = db::open(&config.db_path()).unwrap();
            Self {
                _temp: temp,
                arc_dir,
                config,
                conn,
            }
        }

        fn run(&mut self) -> RunSummary {
            run(&mut self.conn, &self.config).unwrap()
        }

        fn device_dir(&self) -> PathBuf {
            self.arc_dir.join("Backup").join(DEVICE)
        }

        fn mirror(&self, relative: &str) -> PathBuf {
            mirror::mirror_path(&self.config.data_dir, DEVICE, relative)
        }

        fn count(&self, table: &str) -> i64 {
            self.conn
                .query_row(&format!("SELECT count(*) FROM {table}"), [], |row| {
                    row.get(0)
                })
                .unwrap()
        }
    }

    /// Rewrites a bucket file and moves its mtime forward, the way Arc rewriting a bucket looks
    /// to us.
    fn rewrite(path: &Path, contents: &str) {
        fs::write(path, contents).unwrap();
        let file = fs::File::options().write(true).open(path).unwrap();
        let mtime = std::time::SystemTime::now() + Duration::from_secs(60);
        file.set_modified(mtime).unwrap();
    }

    #[test]
    fn first_run_ingests_every_bucket_file() {
        let mut fixture = Fixture::new();

        let summary = fixture.run();

        assert_eq!(summary.files_seen, 4);
        assert_eq!(summary.files_ingested, 4);
        assert_eq!(summary.places_upserted, 2);
        assert_eq!(summary.items_upserted, 9);
        assert_eq!(summary.samples_upserted, 23);
        assert_eq!(summary.device_id.as_deref(), Some(DEVICE));
        assert!(summary.error.is_none(), "{:?}", summary.error);

        assert_eq!(fixture.count("places"), 2);
        assert_eq!(fixture.count("items"), 9);
        assert_eq!(fixture.count("samples"), 23);
        assert_eq!(fixture.count("ingest_files"), 4);

        for relative in [
            "places/A.json",
            "items/2025-06.json",
            "samples/2025-W24.json.gz",
            "samples/2025-W25.json",
        ] {
            let source = fixture.device_dir().join(relative);
            assert_eq!(
                fs::read(&source).unwrap(),
                fs::read(fixture.mirror(relative)).unwrap(),
                "mirror differs for {relative}"
            );
            let record_count: i64 = fixture
                .conn
                .query_row(
                    "SELECT record_count FROM ingest_files WHERE path = ?",
                    params![relative],
                    |row| row.get(0),
                )
                .unwrap();
            assert!(record_count > 0, "{relative}");
        }
    }

    #[test]
    fn second_run_skips_unchanged_files() {
        let mut fixture = Fixture::new();
        fixture.run();

        let summary = fixture.run();

        assert_eq!(summary.files_seen, 4);
        assert_eq!(summary.files_ingested, 0);
        assert_eq!(summary.places_upserted, 0);
        assert_eq!(summary.items_upserted, 0);
        assert_eq!(summary.samples_upserted, 0);
        // Nothing changed, so nothing needs deriving again.
        assert_eq!(summary.days_recomputed, 0);
    }

    #[test]
    fn first_run_derives_local_days() {
        let mut fixture = Fixture::new();

        let summary = fixture.run();

        // The fixture's items and samples fall on 2025-06-10 and 2025-06-12 local.
        assert_eq!(summary.days_recomputed, 2);
        let days: Vec<(String, i64, i64, f64)> = fixture
            .conn
            .prepare(
                "SELECT date, item_count, sample_count, distance_m
                 FROM day_summaries ORDER BY date",
            )
            .unwrap()
            .query_map([], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
            })
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        assert_eq!(days.len(), 2);
        assert_eq!(days[0].0, "2025-06-10");
        assert_eq!(days[0].1, 5);
        assert_eq!(days[0].2, 15);
        // The tram ride from Hauptbahnhof to Postplatz and the walk on to Theaterplatz,
        // about 1.15 km plus 530 m along their fifteen fixes.
        assert!((days[0].3 - 1680.0).abs() < 25.0, "{:?}", days[0]);
        assert_eq!(days[1].0, "2025-06-12");

        // 2025-06-11 has neither an item nor a sample and must have no row at all.
        assert!(days.iter().all(|day| day.0 != "2025-06-11"));

        let (offset, local_start, local_end): (i64, String, String) = fixture
            .conn
            .query_row(
                "SELECT start_offset_seconds, local_start_date, local_end_date
                 FROM items ORDER BY start_date LIMIT 1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(offset, 7200);
        assert_eq!(
            (local_start.as_str(), local_end.as_str()),
            ("2025-06-10", "2025-06-10")
        );

        let unsummarized: i64 = fixture
            .conn
            .query_row(
                "SELECT count(*) FROM samples WHERE seconds_from_gmt IS NOT NULL
                 AND local_date IS NULL",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(unsummarized, 0);
    }

    #[test]
    fn only_a_newer_last_saved_updates_a_record() {
        let mut fixture = Fixture::new();
        fixture.run();
        let items_path = fixture.device_dir().join("items/2025-06.json");
        let id: String = fixture
            .conn
            .query_row(
                "SELECT id FROM items ORDER BY start_date LIMIT 1",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let title = |fixture: &Fixture| -> Option<String> {
            fixture
                .conn
                .query_row(
                    "SELECT visit_custom_title FROM items WHERE id = ?",
                    params![id],
                    |row| row.get(0),
                )
                .unwrap()
        };
        let edited = |last_saved: &str, custom_title: &str| {
            let mut items: serde_json::Value =
                serde_json::from_str(&fs::read_to_string(&items_path).unwrap()).unwrap();
            items[0]["base"]["lastSaved"] = last_saved.into();
            items[0]["visit"]["lastSaved"] = last_saved.into();
            items[0]["visit"]["customTitle"] = custom_title.into();
            serde_json::to_string(&items).unwrap()
        };

        rewrite(&items_path, &edited("2025-06-11T10:00:00Z", "newer"));
        let summary = fixture.run();
        assert_eq!(summary.files_ingested, 1);
        assert_eq!(summary.items_upserted, 1);
        assert_eq!(title(&fixture).as_deref(), Some("newer"));

        rewrite(&items_path, &edited("2025-01-01T00:00:00Z", "older"));
        let summary = fixture.run();
        assert_eq!(summary.files_ingested, 1);
        assert_eq!(summary.items_upserted, 0);
        assert_eq!(title(&fixture).as_deref(), Some("newer"));
    }

    #[test]
    fn a_corrupt_bucket_is_reported_and_the_others_still_ingest() {
        let mut fixture = Fixture::new();
        rewrite(
            &fixture.device_dir().join("samples/2025-W25.json"),
            "{ not json",
        );

        let summary = fixture.run();

        assert_eq!(summary.files_seen, 4);
        assert_eq!(summary.files_ingested, 3);
        assert_eq!(summary.samples_upserted, 15);
        let error = summary.error.unwrap();
        assert!(error.contains("samples/2025-W25.json"), "{error}");

        let recorded: i64 = fixture
            .conn
            .query_row(
                "SELECT count(*) FROM ingest_files WHERE path = 'samples/2025-W25.json'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(recorded, 0, "a file that failed to parse must be retried");

        // The run row carries the failure, so `arches status` shows it after the process exits.
        let stored: String = fixture
            .conn
            .query_row(
                "SELECT error FROM ingest_runs WHERE id = ?",
                params![summary.id],
                |row| row.get(0),
            )
            .unwrap();
        assert!(stored.contains("samples/2025-W25.json"), "{stored}");
    }

    #[test]
    fn a_different_device_forces_a_full_reingest() {
        let mut fixture = Fixture::new();
        fixture.run();

        let replacement = "aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee";
        let backup = fixture.arc_dir.join("Backup");
        fs::rename(backup.join(DEVICE), backup.join(replacement)).unwrap();

        let summary = fixture.run();

        assert_eq!(summary.device_id.as_deref(), Some(replacement));
        assert_eq!(summary.files_ingested, 4);
        assert!(
            mirror::mirror_path(&fixture.config.data_dir, replacement, "places/A.json").exists()
        );
    }

    #[test]
    fn ingest_never_needs_to_write_to_the_arc_dir() {
        let mut fixture = Fixture::new();
        let arc_dir = fixture.arc_dir.clone();
        set_writable(&arc_dir, false);

        let summary = run(&mut fixture.conn, &fixture.config);

        set_writable(&arc_dir, true);
        let summary = summary.unwrap();
        assert_eq!(summary.files_ingested, 4);
        assert!(summary.error.is_none(), "{:?}", summary.error);
    }

    fn set_writable(path: &Path, writable: bool) {
        let mode = if writable { "u+w" } else { "a-w" };
        let status = std::process::Command::new("chmod")
            .args(["-R", mode])
            .arg(path)
            .status()
            .unwrap();
        assert!(status.success());
    }
}
