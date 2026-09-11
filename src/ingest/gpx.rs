//! Ingesting a directory of daily GPX files into the same tables as Arc's backup.
//!
//! One `<trk>` becomes a trip item with a sample per fix, one `<wpt>` becomes a visit item with
//! a single sample at the waypoint, and a named waypoint also becomes a place. Ids are hashes
//! of the file and the element rather than record ids, because GPX has none; see `gpx::ids`.
//!
//! Everything downstream then works unchanged: the rows are Arc-shaped, so derivation, the API,
//! the heatmap and the UI never learn that a second source exists beyond the `source` column.
//!
//! The GPX directory is read-only, exactly like Arc's iCloud folder: files are opened to read
//! and copied into the mirror, and nothing is ever written back.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::time::SystemTime;

use anyhow::{Context, Result};
use jiff::civil::Date;
use jiff::tz::Offset;
use jiff::{Timestamp, ToSpan};
use rusqlite::{Connection, OptionalExtension, params};

use super::{RunState, mirror, system_time_millis, upsert};
use crate::arc::enums::{ActivityType, MovingState, RecordingState};
use crate::arc::types::{
    LocomotionSample, Place, TimelineItem, TimelineItemBase, TimelineItemTrip, TimelineItemVisit,
};
use crate::config::Config;
use crate::derive;
use crate::geo::haversine_m;
use crate::gpx::{self, Document, Element, Timezones, Track, Waypoint, deterministic_id};

/// GPX files are their own lineage in `ingest_files`, not a device backup.
const DEVICE: &str = "gpx";
/// What a file without a `creator` attribute is recorded as.
const FALLBACK_SOURCE: &str = "gpx";
/// The shape of the mapping below, bumped if a re-ingest would produce different rows.
const SOURCE_VERSION: &str = "1";
/// Quantified Map's word for motorised transport it could not name. Arc has no such case, so it
/// lands on the nearest one unconfirmed, which the UI shows as needing review.
const TRANSPORT: &str = "transport";
/// A waypoint the exporter had no place for.
const UNNAMED_VISIT: &str = "Unnamed Visit";
/// A visit ends when the next element in the stream starts, but the very last visit of the
/// whole directory has nothing after it. An hour is a plausible stay and keeps the item from
/// swallowing the rest of history.
const LAST_VISIT_HOURS: i64 = 1;
/// The recording stopping is not the same as a visit going on. Where the export has a gap, the
/// next element can be months after the waypoint before it: 458 of the 2087 visits at the end
/// of a file in the quantified-map archive run past a day, the longest by 418 of them. Past a
/// day the visit is capped rather than stretched, the way Arc's own timeline gaps are read.
const MAX_VISIT_HOURS: i64 = 24;

/// One file in the GPX directory.
struct GpxFile {
    path: PathBuf,
    name: String,
    mtime: SystemTime,
    len: u64,
}

impl GpxFile {
    /// The key in `ingest_files`, alongside Arc's `samples/2025-W24.json.gz`.
    fn relative(&self) -> String {
        format!("gpx/{}", self.name)
    }

    /// The UTC day the file holds, from its name, falling back to its first timestamp for a
    /// directory that names its files some other way.
    fn date(&self) -> Option<Date> {
        let stem = self.name.strip_suffix(".gpx").unwrap_or(&self.name);
        if let Ok(date) = stem.parse::<Date>() {
            return Some(date);
        }
        gpx::next_element_time(&self.path, None)
            .ok()
            .flatten()
            .map(|time| Offset::UTC.to_datetime(time).date())
    }
}

/// What carries across the files of one pass.
struct Run {
    source: String,
    /// The first day Arc covers; a GPX file on or after it is skipped. `None` when there is no
    /// Arc data to defend.
    arc_floor: Option<Date>,
    /// Built on first use: loading the timezone boundaries costs more than a pass that finds
    /// nothing changed should pay.
    zones: Option<Timezones>,
    /// Place id to name, for the one pass over places at the end.
    places: BTreeMap<String, String>,
    /// The item before the next one to be written, for `previous_item_id` / `next_item_id`.
    previous: Option<ItemLink>,
    /// The newest file mtime the pass has ingested, which is what places are stamped with.
    last_saved: Option<Timestamp>,
}

impl Run {
    fn zones(&mut self) -> &mut Timezones {
        self.zones.get_or_insert_with(Timezones::new)
    }
}

#[derive(Clone)]
struct ItemLink {
    id: String,
    previous_id: Option<String>,
}

pub fn ingest(
    conn: &mut Connection,
    config: &Config,
    dir: &Path,
    state: &mut RunState,
) -> Result<()> {
    let files = list_files(dir)?;
    state.counters.files_seen += files.len() as i64;
    let Some(first) = files.first() else {
        tracing::warn!(dir = %dir.display(), "no .gpx files in the GPX dir");
        return Ok(());
    };

    // One directory is one export, so the first file's creator names the whole source.
    let source = gpx::creator_of(&first.path)?.unwrap_or_else(|| FALLBACK_SOURCE.to_string());
    let arc_floor = arc_floor(conn, &source)?;
    tracing::info!(
        dir = %dir.display(),
        files = files.len(),
        %source,
        arc_floor = ?arc_floor.map(|date| date.to_string()),
        "ingesting GPX history"
    );

    let mut run = Run {
        source,
        arc_floor,
        zones: None,
        places: BTreeMap::new(),
        previous: None,
        last_saved: None,
    };
    for index in 0..files.len() {
        if let Err(error) = ingest_file(conn, config, &files, index, &mut run, state) {
            let file = &files[index];
            tracing::error!(
                path = %file.relative(),
                error = %format!("{error:#}"),
                "GPX file failed"
            );
            state.errors.push(format!("{}: {error:#}", file.relative()));
            // Better an unknown link than one that skips whatever the failed file held.
            run.previous = None;
        }
    }

    write_places(conn, &mut run, state)
}

/// `*.gpx` in name order, which for daily files is chronological order.
fn list_files(dir: &Path) -> Result<Vec<GpxFile>> {
    let entries =
        std::fs::read_dir(dir).with_context(|| format!("reading GPX dir {}", dir.display()))?;
    let mut files = Vec::new();
    for entry in entries {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') || !name.ends_with(".gpx") {
            continue;
        }
        let metadata = entry.metadata()?;
        if !metadata.is_file() {
            continue;
        }
        files.push(GpxFile {
            path: entry.path(),
            name,
            mtime: metadata.modified()?,
            len: metadata.len(),
        });
    }
    files.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(files)
}

/// The first day Arc has anything on. GPX history predates the Arc recording it is stitched in
/// front of, and where the two meet Arc is the better data: it has accuracies, confirmed places
/// and real ids. The day is measured in UTC, which errs on the side of skipping a GPX file
/// whose evening already belongs to Arc.
fn arc_floor(conn: &Connection, source: &str) -> Result<Option<Date>> {
    let earliest: Option<i64> = conn.query_row(
        "SELECT min(start_date) FROM items WHERE coalesce(source, '') <> ?",
        params![source],
        |row| row.get(0),
    )?;
    Ok(earliest
        .and_then(|millis| Timestamp::from_millisecond(millis).ok())
        .map(|time| Offset::UTC.to_datetime(time).date()))
}

fn ingest_file(
    conn: &mut Connection,
    config: &Config,
    files: &[GpxFile],
    index: usize,
    run: &mut Run,
    state: &mut RunState,
) -> Result<()> {
    let file = &files[index];
    let relative = file.relative();

    if let (Some(floor), Some(date)) = (run.arc_floor, file.date())
        && date >= floor
    {
        tracing::warn!(
            path = %relative,
            %date,
            arc_floor = %floor,
            "skipping GPX file: Arc already covers that day"
        );
        run.previous = None;
        return Ok(());
    }

    let mtime = system_time_millis(file.mtime);
    let len = file.len as i64;
    if unchanged(conn, &relative, mtime, len)? {
        tracing::debug!(path = %relative, "unchanged GPX file, skipping");
        // The next file's first item looks its neighbour up in the database instead.
        run.previous = None;
        return Ok(());
    }

    let destination = mirror::mirror_path(&config.data_dir, DEVICE, &file.name);
    mirror::copy_into_mirror(&file.path, &destination)?;
    let document = gpx::read(&destination)?;

    let last_saved = Timestamp::from_millisecond(mtime).unwrap_or_else(|_| Timestamp::now());
    let previous = match &run.previous {
        Some(link) => Some(link.clone()),
        None => previous_from_db(conn, &run.source, &document)?,
    };
    let built = build(
        &document,
        file,
        &files[index + 1..],
        previous,
        last_saved,
        run,
    );

    let tx = conn.transaction()?;
    state.counters.items += upsert::upsert_items(&tx, &built.items)? as i64;
    state.counters.samples += upsert::upsert_samples(&tx, &built.samples)? as i64;
    // The item before this file's first one lives in an earlier file, and only now knows what
    // follows it.
    if let Some((id, next_id)) = &built.link_back {
        tx.execute(
            "UPDATE items SET next_item_id = ? WHERE id = ?",
            params![next_id, id],
        )?;
    }
    // Same transaction as the records, so a crash re-ingests the file instead of losing it.
    tx.execute(
        "INSERT INTO ingest_files (path, device_id, mtime, len, ingested_at, record_count)
         VALUES (?, ?, ?, ?, ?, ?)
         ON CONFLICT(path) DO UPDATE SET
             device_id = excluded.device_id, mtime = excluded.mtime, len = excluded.len,
             ingested_at = excluded.ingested_at, record_count = excluded.record_count",
        params![
            relative,
            DEVICE,
            mtime,
            len,
            Timestamp::now().as_millisecond(),
            built.items.len() as i64,
        ],
    )?;
    tx.commit()?;

    state
        .touched_items
        .extend(built.items.iter().map(|item| item.base.id.clone()));
    let mut previous_date: Option<String> = None;
    for sample in &built.samples {
        let Some(offset) = sample.seconds_from_gmt else {
            continue;
        };
        let date = derive::local_date(sample.date, offset);
        // A day's fixes arrive together, so remembering one date saves the set a million inserts.
        if previous_date.as_deref() != Some(date.as_str()) {
            state.touched_dates.insert(date.clone());
            previous_date = Some(date);
        }
    }
    for (id, name) in built.places {
        run.places.insert(id, name);
    }
    run.previous = built.last.or_else(|| run.previous.take());
    run.last_saved = Some(
        run.last_saved
            .map_or(last_saved, |seen| seen.max(last_saved)),
    );
    state.counters.files_ingested += 1;
    tracing::debug!(
        path = %relative,
        items = built.items.len(),
        samples = built.samples.len(),
        "ingested GPX file"
    );
    Ok(())
}

fn unchanged(conn: &Connection, relative: &str, mtime: i64, len: i64) -> Result<bool> {
    let seen: Option<(String, i64, i64)> = conn
        .query_row(
            "SELECT device_id, mtime, len FROM ingest_files WHERE path = ?",
            params![relative],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()?;
    Ok(seen.is_some_and(|(device, seen_mtime, seen_len)| {
        device == DEVICE && seen_mtime == mtime && seen_len == len
    }))
}

/// The item just before this file's first one, for a run that has not written it: the file
/// before this one was unchanged, or failed. Chronological order is document order here.
fn previous_from_db(
    conn: &Connection,
    source: &str,
    document: &Document,
) -> Result<Option<ItemLink>> {
    let Some(start) = document.elements.iter().find_map(Element::time) else {
        return Ok(None);
    };
    Ok(conn
        .query_row(
            "SELECT id, previous_item_id FROM items
             WHERE source = ? AND start_date < ? ORDER BY start_date DESC LIMIT 1",
            params![source, start.as_millisecond()],
            |row| {
                Ok(ItemLink {
                    id: row.get(0)?,
                    previous_id: row.get(1)?,
                })
            },
        )
        .optional()?)
}

/// One file's rows, ready to upsert.
struct Built {
    items: Vec<TimelineItem>,
    samples: Vec<LocomotionSample>,
    places: Vec<(String, String)>,
    /// The last item of the file, which the next file chains onto.
    last: Option<ItemLink>,
    /// `(item id, next id)` for the item in an earlier file that now knows its successor.
    link_back: Option<(String, String)>,
}

fn build(
    document: &Document,
    file: &GpxFile,
    rest: &[GpxFile],
    previous: Option<ItemLink>,
    last_saved: Timestamp,
    run: &mut Run,
) -> Built {
    let mut built = Built {
        items: Vec::new(),
        samples: Vec::new(),
        places: Vec::new(),
        last: None,
        link_back: None,
    };
    let source = run.source.clone();

    for (index, element) in document.elements.iter().enumerate() {
        let item = match element {
            Element::Track(track) => {
                build_trip(track, file, index, &source, last_saved, run, &mut built)
            }
            Element::Waypoint(waypoint) => build_visit(
                waypoint,
                &document.elements[index + 1..],
                rest,
                &source,
                last_saved,
                run,
                &mut built,
            ),
        };
        let Some(mut item) = item else { continue };

        // A visit spanning UTC midnight is the same item in both files, so the copy at the head
        // of the next file inherits the link the first copy already had.
        match built.last.clone().or_else(|| previous.clone()) {
            Some(link) if link.id == item.base.id => {
                item.base.previous_item_id = link.previous_id;
            }
            Some(link) => {
                item.base.previous_item_id = Some(link.id.clone());
                match built.items.last_mut() {
                    Some(earlier) => earlier.base.next_item_id = Some(item.base.id.clone()),
                    None => built.link_back = Some((link.id, item.base.id.clone())),
                }
            }
            None => {}
        }
        built.last = Some(ItemLink {
            id: item.base.id.clone(),
            previous_id: item.base.previous_item_id.clone(),
        });
        built.items.push(item);
    }

    // The row for a midnight-spanning visit is written by both of its files, and the upsert
    // keeps whichever has the newer `last_saved`: when the two files share an mtime to the
    // millisecond the second write is dropped, taking the forward link it learned with it.
    if let Some(first) = built.items.first()
        && previous.is_some_and(|link| link.id == first.base.id)
        && let Some(next) = &first.base.next_item_id
    {
        built.link_back = Some((first.base.id.clone(), next.clone()));
    }
    built
}

fn build_trip(
    track: &Track,
    file: &GpxFile,
    index: usize,
    source: &str,
    last_saved: Timestamp,
    run: &mut Run,
    built: &mut Built,
) -> Option<TimelineItem> {
    let points: Vec<_> = track
        .points
        .iter()
        .filter_map(|point| point.time.map(|time| (point, time)))
        .collect();
    // A single fix is a sliver at a segment boundary or a midnight split, not a trip.
    if points.len() < 2 {
        return None;
    }

    let id = deterministic_id(source, "trip", &format!("{}#{index}", file.name));
    let start = points.first()?.1;
    let end = points.last()?.1;
    let mut distance = 0.0;
    for pair in points.windows(2) {
        let (from, to) = (pair[0].0, pair[1].0);
        distance += haversine_m(from.latitude, from.longitude, to.latitude, to.longitude);
    }
    let seconds = (end.as_millisecond() - start.as_millisecond()) as f64 / 1000.0;

    for (point_index, (point, time)) in points.iter().enumerate() {
        let offset = run
            .zones()
            .offset_seconds(point.latitude, point.longitude, *time);
        built.samples.push(sample(
            deterministic_id(
                source,
                "sample",
                &format!("{}#{index}#{point_index}", file.name),
            ),
            &id,
            *time,
            point.latitude,
            point.longitude,
            point.altitude,
            offset,
            MovingState::Moving,
            source,
            last_saved,
        ));
    }

    let (classified, confirmed, uncertain) = activity_type(track.kind.as_deref());
    Some(TimelineItem {
        base: base(id.clone(), false, start, end, source, last_saved),
        visit: None,
        trip: Some(TimelineItemTrip {
            item_id: id,
            distance: Some(distance),
            speed: (seconds > 0.0).then(|| distance / seconds),
            classified_activity_type: Some(classified),
            confirmed_activity_type: confirmed,
            uncertain_activity_type: Some(uncertain),
            last_saved,
        }),
    })
}

fn build_visit(
    waypoint: &Waypoint,
    rest_of_file: &[Element],
    rest: &[GpxFile],
    source: &str,
    last_saved: Timestamp,
    run: &mut Run,
    built: &mut Built,
) -> Option<TimelineItem> {
    let start = waypoint.time?;
    let end = visit_end(waypoint, start, rest_of_file, rest);
    let name = waypoint.name.as_deref().unwrap_or(UNNAMED_VISIT);
    // Keyed on name and start rather than the file, so the copy of a midnight-spanning visit in
    // the next file is the same item.
    let key = format!("{name}@{start}");
    let id = deterministic_id(source, "visit", &key);

    let place_id = (name != UNNAMED_VISIT).then(|| {
        let place_id = deterministic_id(source, "place", name);
        built.places.push((place_id.clone(), name.to_string()));
        place_id
    });

    let offset = run
        .zones()
        .offset_seconds(waypoint.latitude, waypoint.longitude, start);
    // One sample so the derivation has an offset to give the item, and so the visit shows up on
    // the map at all.
    built.samples.push(sample(
        deterministic_id(source, "visit-sample", &key),
        &id,
        start,
        waypoint.latitude,
        waypoint.longitude,
        None,
        offset,
        MovingState::Stationary,
        source,
        last_saved,
    ));

    Some(TimelineItem {
        base: base(id.clone(), true, start, end, source, last_saved),
        visit: Some(TimelineItemVisit {
            item_id: id,
            latitude: Some(waypoint.latitude),
            longitude: Some(waypoint.longitude),
            radius_mean: None,
            radius_sd: None,
            confirmed_place: Some(place_id.is_some()),
            uncertain_place: Some(place_id.is_none()),
            place_id,
            custom_title: None,
            street_address: None,
            locality: None,
            country_code: None,
            last_saved,
        }),
        trip: None,
    })
}

/// A visit runs until the next element starts, wherever that is: later in the same file, or at
/// the head of one of the files after it.
fn visit_end(
    waypoint: &Waypoint,
    start: Timestamp,
    rest_of_file: &[Element],
    rest: &[GpxFile],
) -> Timestamp {
    let next = rest_of_file
        .iter()
        .find_map(Element::time)
        .or_else(|| next_file_time(waypoint, start, rest));
    match next {
        // A midnight copy whose successor has already been passed: the visit is this instant.
        Some(next) if next < start => start,
        Some(next) => next.min(start + MAX_VISIT_HOURS.hours()),
        None => start + LAST_VISIT_HOURS.hours(),
    }
}

fn next_file_time(waypoint: &Waypoint, start: Timestamp, rest: &[GpxFile]) -> Option<Timestamp> {
    let name = waypoint.name.as_deref().unwrap_or(UNNAMED_VISIT);
    for file in rest {
        match gpx::next_element_time(&file.path, Some((name, start))) {
            Ok(Some(time)) => return Some(time),
            // An empty file says nothing about when the visit ended; look further ahead.
            Ok(None) => continue,
            Err(error) => {
                tracing::warn!(
                    path = %file.name,
                    error = %format!("{error:#}"),
                    "could not read the file after a visit"
                );
                return None;
            }
        }
    }
    None
}

/// The `<type>` of a track as Arc's three activity columns.
///
/// A type Arc knows is taken as confirmed: it is what the timeline recorded at the time, not a
/// classifier's guess made now. `transport` and `unknown` carry no such claim, so they land on
/// their nearest type unconfirmed and uncertain, which reads as unreviewed everywhere.
fn activity_type(kind: Option<&str>) -> (ActivityType, Option<ActivityType>, bool) {
    match kind.map(str::trim).unwrap_or_default() {
        TRANSPORT => (ActivityType::Car, None, true),
        raw => match ActivityType::from_str(raw) {
            Ok(known) if known != ActivityType::Unknown => (known, Some(known), false),
            _ => (ActivityType::Unknown, None, true),
        },
    }
}

fn base(
    id: String,
    is_visit: bool,
    start_date: Timestamp,
    end_date: Timestamp,
    source: &str,
    last_saved: Timestamp,
) -> TimelineItemBase {
    TimelineItemBase {
        id,
        is_visit,
        start_date,
        end_date,
        last_saved,
        source: Some(source.to_string()),
        source_version: Some(SOURCE_VERSION.to_string()),
        disabled: None,
        deleted: None,
        previous_item_id: None,
        next_item_id: None,
        samples_changed: None,
        locked: None,
        step_count: None,
        floors_ascended: None,
        floors_descended: None,
        average_altitude: None,
        active_energy_burned: None,
        average_heart_rate: None,
        max_heart_rate: None,
    }
}

/// GPX carries no accuracies, speeds or courses, so those columns stay NULL rather than being
/// invented from the geometry.
#[allow(clippy::too_many_arguments, reason = "one flat row, not a hidden type")]
fn sample(
    id: String,
    item_id: &str,
    date: Timestamp,
    latitude: f64,
    longitude: f64,
    altitude: Option<f64>,
    seconds_from_gmt: i32,
    moving_state: MovingState,
    source: &str,
    last_saved: Timestamp,
) -> LocomotionSample {
    LocomotionSample {
        id,
        date,
        last_saved,
        source: Some(source.to_string()),
        source_version: Some(SOURCE_VERSION.to_string()),
        seconds_from_gmt: Some(seconds_from_gmt),
        moving_state: Some(moving_state),
        recording_state: Some(RecordingState::Recording),
        disabled: Some(false),
        rtree_id: None,
        timeline_item_id: Some(item_id.to_string()),
        latitude: Some(latitude),
        longitude: Some(longitude),
        altitude,
        horizontal_accuracy: None,
        vertical_accuracy: None,
        speed: None,
        course: None,
        step_hz: None,
        xy_acceleration: None,
        z_acceleration: None,
        heart_rate: None,
        classified_activity_type: None,
        confirmed_activity_type: None,
    }
}

/// One row per place the pass touched, counted from every visit the database has rather than
/// only this pass's: a rerun over a single changed file must not leave the other counts stale.
fn write_places(conn: &mut Connection, run: &mut Run, state: &mut RunState) -> Result<()> {
    let names = std::mem::take(&mut run.places);
    if names.is_empty() {
        return Ok(());
    }
    let last_saved = run.last_saved.unwrap_or_else(Timestamp::now);
    let mut places = Vec::with_capacity(names.len());

    for (id, name) in &names {
        let Some(stats) = place_stats(conn, id)? else {
            continue;
        };
        // One offset for the whole place, the way Arc stores it: an approximation that is only
        // ever wrong for a place someone visits from either side of a zone change.
        let seconds_from_gmt = stats.last_visit_date.map(|last| {
            run.zones()
                .offset_seconds(stats.latitude, stats.longitude, last)
        });
        places.push(Place {
            id: id.clone(),
            name: name.clone(),
            latitude: stats.latitude,
            longitude: stats.longitude,
            // GPX has no radius, no address and no country: the export carries a name and a
            // position and nothing else. See README.md for what that costs the highlights.
            radius_mean: None,
            radius_sd: None,
            street_address: None,
            locality: None,
            country_code: None,
            seconds_from_gmt,
            is_stale: None,
            visit_count: Some(stats.visit_count),
            visit_days: Some(stats.visit_days),
            last_visit_date: stats.last_visit_date,
            last_saved,
            source: Some(run.source.clone()),
            rtree_id: None,
            mapbox_place_id: None,
            mapbox_category: None,
            mapbox_maki_icon: None,
            google_place_id: None,
            google_primary_type: None,
            foursquare_place_id: None,
            foursquare_category_id: None,
            foursquare_category_v2_id: None,
            user_category: None,
            category: None,
        });
    }

    let tx = conn.transaction()?;
    state.counters.places += upsert::upsert_places(&tx, &places)? as i64;
    tx.commit()?;
    Ok(())
}

struct PlaceStats {
    latitude: f64,
    longitude: f64,
    visit_count: i64,
    visit_days: i64,
    last_visit_date: Option<Timestamp>,
}

fn place_stats(conn: &Connection, place_id: &str) -> Result<Option<PlaceStats>> {
    let stats = conn.query_row(
        "SELECT count(*), avg(visit_latitude), avg(visit_longitude), max(start_date)
         FROM items WHERE visit_place_id = ?",
        params![place_id],
        |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, Option<f64>>(1)?,
                row.get::<_, Option<f64>>(2)?,
                row.get::<_, Option<i64>>(3)?,
            ))
        },
    )?;
    let (visit_count, Some(latitude), Some(longitude), last_visit) = stats else {
        return Ok(None);
    };
    // Each visit has exactly one sample, and a sample knows its local day.
    let visit_days: i64 = conn.query_row(
        "SELECT count(DISTINCT s.local_date) FROM items i
         JOIN samples s ON s.timeline_item_id = i.id
         WHERE i.visit_place_id = ? AND s.local_date IS NOT NULL",
        params![place_id],
        |row| row.get(0),
    )?;

    Ok(Some(PlaceStats {
        latitude,
        longitude,
        visit_count,
        visit_days,
        last_visit_date: last_visit.and_then(|millis| Timestamp::from_millisecond(millis).ok()),
    }))
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::collections::BTreeMap;
    use std::fs;
    use std::io::Write;
    use std::sync::OnceLock;
    use std::time::{Duration, UNIX_EPOCH};

    use super::super::tests::copy_tree;
    use super::*;
    use crate::db;
    use crate::ingest::{RunSummary, run};

    const SOURCE: &str = "quantified-map-gpx";
    const KULTURPALAST: &str = "Kulturpalast@2016-02-18T22:30:00Z";
    const ZWINGER: &str = "Zwinger@2016-02-18T08:10:00Z";
    const UNNAMED: &str = "Unnamed Visit@2016-02-18T09:10:00Z";
    const GARTEN: &str = "Großer Garten@2016-02-19T08:00:00Z";
    const LAST_VISIT: &str = "Neustädter Markt@2025-06-10T08:05:00Z";

    fn fixtures() -> PathBuf {
        PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/gpx"))
    }

    fn visit_id(key: &str) -> String {
        deterministic_id(SOURCE, "visit", key)
    }

    /// Fixture mtimes are pinned and increasing, so `last_saved` is deterministic and the later
    /// copy of a midnight-spanning visit is the one that wins.
    fn set_mtime(path: &Path, index: u64) {
        let file = fs::File::options().write(true).open(path).unwrap();
        file.set_modified(UNIX_EPOCH + Duration::from_secs(1_700_000_000 + index))
            .unwrap();
    }

    struct Fixture {
        _temp: tempfile::TempDir,
        gpx_dir: PathBuf,
        config: Config,
        conn: Connection,
    }

    impl Fixture {
        /// `with_arc` also ingests the Arc backup fixture, whose first day is 2025-06-10 and so
        /// claims the last GPX file.
        fn new(with_arc: bool) -> Self {
            let temp = tempfile::tempdir().unwrap();
            let gpx_dir = temp.path().join("gpx");
            fs::create_dir_all(&gpx_dir).unwrap();
            let mut names: Vec<String> = fs::read_dir(fixtures())
                .unwrap()
                .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
                .collect();
            names.sort();
            for (index, name) in names.iter().enumerate() {
                let target = gpx_dir.join(name);
                fs::copy(fixtures().join(name), &target).unwrap();
                set_mtime(&target, index as u64);
            }

            let arc_dir = temp.path().join("arc");
            if with_arc {
                copy_tree(
                    &PathBuf::from(concat!(
                        env!("CARGO_MANIFEST_DIR"),
                        "/tests/fixtures/backup"
                    )),
                    &arc_dir,
                );
            }
            let config = Config::from_pairs([
                ("ARCHES_ARC_DIR", arc_dir.to_str().unwrap()),
                ("ARCHES_GPX_DIR", gpx_dir.to_str().unwrap()),
                (
                    "ARCHES_DATA_DIR",
                    temp.path().join("data").to_str().unwrap(),
                ),
            ])
            .unwrap();
            let conn = db::open(&config.db_path()).unwrap();
            Self {
                _temp: temp,
                gpx_dir,
                config,
                conn,
            }
        }

        fn run(&mut self) -> RunSummary {
            run(&mut self.conn, &self.config).unwrap()
        }

        fn count(&self, table: &str) -> i64 {
            self.conn
                .query_row(&format!("SELECT count(*) FROM {table}"), [], |row| {
                    row.get(0)
                })
                .unwrap()
        }

        fn item(&self, id: &str) -> Option<(String, String, Option<String>, Option<String>)> {
            self.conn
                .query_row(
                    "SELECT start_date, end_date, previous_item_id, next_item_id
                     FROM items WHERE id = ?",
                    params![id],
                    |row| {
                        Ok((
                            stamp(row.get(0)?),
                            stamp(row.get(1)?),
                            row.get(2)?,
                            row.get(3)?,
                        ))
                    },
                )
                .optional()
                .unwrap()
        }
    }

    fn stamp(millis: i64) -> String {
        Timestamp::from_millisecond(millis).unwrap().to_string()
    }

    /// A GPX-only run has no Arc backup at all, which is an error against the Arc source and
    /// nothing more: the GPX files still go in.
    fn gpx_only_error(summary: &RunSummary) {
        let error = summary.error.as_deref().unwrap_or_default();
        assert!(error.contains("arc:"), "{error}");
        assert!(!error.contains("gpx/"), "{error}");
    }

    #[test]
    fn a_first_run_ingests_every_file() {
        let mut fixture = Fixture::new(false);

        let summary = fixture.run();

        gpx_only_error(&summary);
        assert_eq!(summary.files_seen, 3);
        assert_eq!(summary.files_ingested, 3);
        // The midnight visit and its sample are written twice, by both files that hold them.
        assert_eq!(summary.items_upserted, 11);
        assert_eq!(summary.samples_upserted, 18);
        assert_eq!(summary.places_upserted, 4);
        assert_eq!(summary.days_recomputed, 4);

        assert_eq!(fixture.count("items"), 10);
        assert_eq!(fixture.count("samples"), 17);
        assert_eq!(fixture.count("places"), 4);
        assert_eq!(fixture.count("ingest_files"), 3);

        for name in ["2016-02-18.gpx", "2016-02-19.gpx", "2025-06-10.gpx"] {
            let mirrored = fixture.config.data_dir.join("raw/gpx").join(name);
            assert_eq!(
                fs::read(fixture.gpx_dir.join(name)).unwrap(),
                fs::read(&mirrored).unwrap(),
                "mirror differs for {name}"
            );
        }
    }

    #[test]
    fn a_second_run_ingests_nothing() {
        let mut fixture = Fixture::new(false);
        fixture.run();
        let ids: Vec<String> = fixture
            .conn
            .prepare("SELECT id FROM items ORDER BY id")
            .unwrap()
            .query_map([], |row| row.get(0))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();

        let summary = fixture.run();

        assert_eq!(summary.files_seen, 3);
        assert_eq!(summary.files_ingested, 0);
        assert_eq!(summary.items_upserted, 0);
        assert_eq!(summary.samples_upserted, 0);
        assert_eq!(summary.places_upserted, 0);
        assert_eq!(summary.days_recomputed, 0);
        assert_eq!(fixture.count("items"), 10);

        // The ids are hashes of the file and the element, so a rerun lands on the same rows.
        let again: Vec<String> = fixture
            .conn
            .prepare("SELECT id FROM items ORDER BY id")
            .unwrap()
            .query_map([], |row| row.get(0))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        assert_eq!(ids, again);
    }

    #[test]
    fn a_visit_ends_when_the_next_element_starts() {
        let mut fixture = Fixture::new(false);
        fixture.run();

        // The Zwinger visit ends when the transport track leaves.
        let (start, end, _, _) = fixture.item(&visit_id(ZWINGER)).unwrap();
        assert_eq!(start, "2016-02-18T08:10:00Z");
        assert_eq!(end, "2016-02-18T09:00:00Z");

        // Nothing follows the last visit of the whole directory, so it gets an hour.
        let (start, end, _, _) = fixture.item(&visit_id(LAST_VISIT)).unwrap();
        assert_eq!(start, "2025-06-10T08:05:00Z");
        assert_eq!(end, "2025-06-10T09:05:00Z");

        // The next element after the Großer Garten visit is nine years later: the recording
        // stopped rather than the visit going on, so it is capped at a day.
        let (start, end, _, _) = fixture.item(&visit_id(GARTEN)).unwrap();
        assert_eq!(start, "2016-02-19T08:00:00Z");
        assert_eq!(end, "2016-02-20T08:00:00Z");
    }

    #[test]
    fn a_visit_spanning_utc_midnight_is_one_item() {
        let mut fixture = Fixture::new(false);
        fixture.run();

        let id = visit_id(KULTURPALAST);
        let (start, end, previous, next) = fixture.item(&id).unwrap();
        assert_eq!(start, "2016-02-18T22:30:00Z");
        // Both files hold the visit; the end is the element after the second copy.
        assert_eq!(end, "2016-02-19T07:00:00Z");
        assert_eq!(previous.as_deref(), Some(visit_id(UNNAMED).as_str()));
        assert!(next.is_some());

        let copies: i64 = fixture
            .conn
            .query_row(
                "SELECT count(*) FROM items WHERE start_date = ?",
                params![
                    "2016-02-18T22:30:00Z"
                        .parse::<Timestamp>()
                        .unwrap()
                        .as_millisecond()
                ],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(copies, 1);

        // Its one sample is the same row too, and it lands on the local day it started.
        let (samples, local_date): (i64, String) = fixture
            .conn
            .query_row(
                "SELECT count(*), min(local_date) FROM samples WHERE timeline_item_id = ?",
                params![id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(samples, 1);
        assert_eq!(local_date, "2016-02-18");

        let (local_start, local_end): (String, String) = fixture
            .conn
            .query_row(
                "SELECT local_start_date, local_end_date FROM items WHERE id = ?",
                params![id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(
            (local_start.as_str(), local_end.as_str()),
            ("2016-02-18", "2016-02-19")
        );
    }

    #[test]
    fn items_are_chained_in_stream_order_across_files() {
        let mut fixture = Fixture::new(false);
        fixture.run();

        let chain = |id: &str| fixture.item(id).map(|item| (item.2, item.3));
        let kulturpalast = visit_id(KULTURPALAST);
        let walk = deterministic_id(SOURCE, "trip", "2016-02-19.gpx#1");

        assert_eq!(
            chain(&kulturpalast),
            Some((Some(visit_id(UNNAMED)), Some(walk.clone())))
        );
        assert_eq!(
            chain(&walk),
            Some((
                Some(kulturpalast),
                Some(visit_id("Zwinger@2016-02-19T07:10:00Z"))
            ))
        );
    }

    /// Both files of a midnight-spanning visit can carry the same mtime, which makes the upsert
    /// drop the second write of that row. The link it learned has to survive anyway.
    #[test]
    fn a_midnight_visit_points_forward_even_when_its_row_is_not_rewritten() {
        let mut fixture = Fixture::new(false);
        for name in ["2016-02-18.gpx", "2016-02-19.gpx"] {
            set_mtime(&fixture.gpx_dir.join(name), 0);
        }

        let summary = fixture.run();

        assert_eq!(summary.files_ingested, 3);
        let (_, _, previous, next) = fixture.item(&visit_id(KULTURPALAST)).unwrap();
        assert_eq!(previous.as_deref(), Some(visit_id(UNNAMED).as_str()));
        assert_eq!(
            next.as_deref(),
            Some(deterministic_id(SOURCE, "trip", "2016-02-19.gpx#1").as_str())
        );
    }

    /// Only one file changes, so the run never sees the item the changed file chains onto and
    /// has to find it in the database instead.
    #[test]
    fn a_partial_rerun_keeps_the_chain() {
        let mut fixture = Fixture::new(false);
        fixture.run();
        set_mtime(&fixture.gpx_dir.join("2016-02-19.gpx"), 99);

        let summary = fixture.run();

        assert_eq!(summary.files_ingested, 1);
        let (_, _, previous, next) = fixture.item(&visit_id(KULTURPALAST)).unwrap();
        assert_eq!(previous.as_deref(), Some(visit_id(UNNAMED).as_str()));
        assert_eq!(
            next.as_deref(),
            Some(deterministic_id(SOURCE, "trip", "2016-02-19.gpx#1").as_str())
        );
        assert_eq!(fixture.count("items"), 10);
    }

    #[test]
    fn a_named_waypoint_becomes_a_place_counted_from_every_visit() {
        let mut fixture = Fixture::new(false);
        fixture.run();

        let place_id = deterministic_id(SOURCE, "place", "Zwinger");
        let (name, latitude, longitude, visits, days, last, offset): (
            String,
            f64,
            f64,
            i64,
            i64,
            i64,
            i32,
        ) = fixture
            .conn
            .query_row(
                "SELECT name, latitude, longitude, visit_count, visit_days, last_visit_date,
                        seconds_from_gmt
                 FROM places WHERE id = ?",
                params![place_id],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                        row.get(5)?,
                        row.get(6)?,
                    ))
                },
            )
            .unwrap();

        assert_eq!(name, "Zwinger");
        // The mean of the two waypoints that carried the name.
        assert!((latitude - 51.0534).abs() < 1e-9, "{latitude}");
        assert!((longitude - 13.7345).abs() < 1e-9, "{longitude}");
        assert_eq!((visits, days), (2, 2));
        assert_eq!(stamp(last), "2016-02-19T07:10:00Z");
        assert_eq!(offset, 3600);

        // An unnamed visit has no place and is the one visit left to review.
        let (place, confirmed, uncertain): (Option<String>, i64, i64) = fixture
            .conn
            .query_row(
                "SELECT visit_place_id, visit_confirmed_place, visit_uncertain_place
                 FROM items WHERE id = ?",
                params![visit_id(UNNAMED)],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(place, None);
        assert_eq!((confirmed, uncertain), (0, 1));
        assert_eq!(fixture.count("places"), 4);
    }

    #[test]
    fn samples_carry_the_offset_of_where_they_were_recorded() {
        let mut fixture = Fixture::new(false);
        fixture.run();

        let offsets: Vec<(String, i32, String)> = fixture
            .conn
            .prepare(
                "SELECT min(date), seconds_from_gmt, local_date FROM samples
                 GROUP BY local_date ORDER BY local_date",
            )
            .unwrap()
            .query_map([], |row| Ok((stamp(row.get(0)?), row.get(1)?, row.get(2)?)))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();

        // February in Dresden is CET, June is CEST.
        assert_eq!(
            offsets,
            [
                (
                    "2016-02-18T08:00:00Z".to_string(),
                    3600,
                    "2016-02-18".to_string()
                ),
                (
                    "2016-02-19T07:00:00Z".to_string(),
                    3600,
                    "2016-02-19".to_string()
                ),
                (
                    "2025-06-10T08:00:00Z".to_string(),
                    7200,
                    "2025-06-10".to_string()
                ),
            ]
        );

        // Altitude comes across where the export has it, and accuracies stay NULL: GPX has none.
        let (altitude, accuracy): (Option<f64>, Option<f64>) = fixture
            .conn
            .query_row(
                "SELECT altitude, horizontal_accuracy FROM samples
                 WHERE date = ? ORDER BY id LIMIT 1",
                params![
                    "2016-02-19T07:05:00Z"
                        .parse::<Timestamp>()
                        .unwrap()
                        .as_millisecond()
                ],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(altitude, Some(115.2));
        assert_eq!(accuracy, None);
    }

    #[test]
    fn a_track_type_arc_knows_is_confirmed_and_transport_is_not() {
        let mut fixture = Fixture::new(false);
        fixture.run();

        let trip = |key: &str| -> (Option<i64>, Option<i64>, i64, i64, f64) {
            fixture
                .conn
                .query_row(
                    "SELECT trip_classified_activity_type, trip_confirmed_activity_type,
                            trip_uncertain_activity_type, activity_type, trip_distance
                     FROM items WHERE id = ?",
                    params![deterministic_id(SOURCE, "trip", key)],
                    |row| {
                        Ok((
                            row.get(0)?,
                            row.get(1)?,
                            row.get(2)?,
                            row.get(3)?,
                            row.get(4)?,
                        ))
                    },
                )
                .unwrap()
        };

        // Walking is what the timeline recorded, not a guess made now.
        let (classified, confirmed, uncertain, resolved, distance) = trip("2016-02-18.gpx#0");
        assert_eq!(classified, Some(ActivityType::Walking.raw() as i64));
        assert_eq!(confirmed, Some(ActivityType::Walking.raw() as i64));
        assert_eq!(
            (uncertain, resolved),
            (0, ActivityType::Walking.raw() as i64)
        );
        assert!((distance - 1069.0).abs() < 25.0, "{distance}");

        // `transport` is Quantified Map's word for "motorised, and that is all I know".
        let (classified, confirmed, uncertain, resolved, _) = trip("2016-02-18.gpx#2");
        assert_eq!(classified, Some(ActivityType::Car.raw() as i64));
        assert_eq!(confirmed, None);
        assert_eq!((uncertain, resolved), (1, ActivityType::Car.raw() as i64));

        let speed: f64 = fixture
            .conn
            .query_row(
                "SELECT trip_speed FROM items WHERE id = ?",
                params![deterministic_id(SOURCE, "trip", "2016-02-18.gpx#0")],
                |row| row.get(0),
            )
            .unwrap();
        assert!((speed - 1.78).abs() < 0.1, "{speed}");
    }

    #[test]
    fn days_are_summarized_from_the_imported_history() {
        let mut fixture = Fixture::new(false);
        fixture.run();

        let day = |date: &str| -> (BTreeMap<String, i64>, i64, bool, i64) {
            fixture
                .conn
                .query_row(
                    "SELECT duration_by_type, unconfirmed_items, confirmed, utc_offset_seconds
                     FROM day_summaries WHERE date = ?",
                    params![date],
                    |row| {
                        let durations: String = row.get(0)?;
                        Ok((
                            serde_json::from_str(&durations).unwrap(),
                            row.get(1)?,
                            row.get(2)?,
                            row.get(3)?,
                        ))
                    },
                )
                .unwrap()
        };

        let (durations, unconfirmed, confirmed, offset) = day("2016-02-18");
        assert_eq!(durations.get("walking"), Some(&600));
        assert_eq!(durations.get("car"), Some(&600));
        assert_eq!(durations.get("stationary"), Some(&52_800));
        // The transport trip and the unnamed visit are what is left to review.
        assert_eq!(unconfirmed, 2);
        assert!(!confirmed);
        assert_eq!(offset, 3600);

        let (durations, unconfirmed, confirmed, _) = day("2016-02-19");
        assert_eq!(durations.get("walking"), Some(&600));
        assert_eq!(durations.get("stationary"), Some(&85_800));
        assert_eq!(unconfirmed, 0);
        assert!(confirmed);

        // The day the capped visit ran into still gets its share.
        let (durations, _, _, _) = day("2016-02-20");
        assert_eq!(durations.get("stationary"), Some(&32_400));
    }

    #[test]
    fn arc_wins_where_the_two_sources_meet() {
        let mut fixture = Fixture::new(true);
        capture_warnings();

        let summary = fixture.run();

        assert!(summary.error.is_none(), "{:?}", summary.error);
        // Four Arc buckets and three GPX files, of which the one Arc covers is skipped.
        assert_eq!(summary.files_seen, 7);
        assert_eq!(summary.files_ingested, 6);
        let warnings = captured_warnings();
        assert!(
            warnings.contains("Arc already covers") && warnings.contains("gpx/2025-06-10.gpx"),
            "no warning naming the skipped file: {warnings}"
        );

        let recorded: i64 = fixture
            .conn
            .query_row(
                "SELECT count(*) FROM ingest_files WHERE path = 'gpx/2025-06-10.gpx'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(recorded, 0, "a skipped file must not be recorded");
        assert!(fixture.item(&visit_id(LAST_VISIT)).is_none());

        // Both sources, side by side, each with its own items.
        let sources: Vec<(String, i64)> = fixture
            .conn
            .prepare("SELECT source, count(*) FROM items GROUP BY source ORDER BY source")
            .unwrap()
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        assert_eq!(
            sources,
            [("LocoKit2".to_string(), 9), (SOURCE.to_string(), 8)]
        );
    }

    thread_local! {
        static WARNINGS: RefCell<Vec<u8>> = const { RefCell::new(Vec::new()) };
    }

    /// Starts collecting this thread's warnings, and empties whatever was there before.
    ///
    /// The subscriber is global and installed once, rather than scoped to the test: a warning
    /// whose callsite another test reached with no subscriber installed is cached as
    /// uninteresting for the whole binary, and only a global default rebuilds that cache. The
    /// buffer is per thread, so the tests running beside this one cannot write into it.
    fn capture_warnings() {
        static INSTALLED: OnceLock<()> = OnceLock::new();
        INSTALLED.get_or_init(|| {
            let subscriber = tracing_subscriber::fmt()
                .with_writer(ThreadWarnings)
                .with_max_level(tracing::Level::WARN)
                .with_ansi(false)
                .finish();
            let _ = tracing::subscriber::set_global_default(subscriber);
        });
        WARNINGS.with(|warnings| warnings.borrow_mut().clear());
    }

    fn captured_warnings() -> String {
        WARNINGS.with(|warnings| String::from_utf8_lossy(&warnings.borrow()).into_owned())
    }

    struct ThreadWarnings;

    impl Write for ThreadWarnings {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            WARNINGS.with(|warnings| warnings.borrow_mut().extend_from_slice(buf));
            Ok(buf.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for ThreadWarnings {
        type Writer = Self;

        fn make_writer(&'a self) -> Self::Writer {
            ThreadWarnings
        }
    }
}
