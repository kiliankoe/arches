//! Day summaries: one row per local day with everything the calendar, week and day views need.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use anyhow::Result;
use jiff::Timestamp;
use jiff::civil::Date;
use rusqlite::{Connection, params};

use super::{clipped_ms, dates_in_range};
use crate::arc::enums::ActivityType;
use crate::confirmation::{self, Confirmation};
use crate::geo::haversine_m;

/// Arc sleeps between samples; a longer gap than this is not a straight line someone walked,
/// so the pair contributes no distance.
const MAX_SAMPLE_GAP_MS: i64 = 10 * 60 * 1000;
/// Above this, a fix is noise that would add kilometres a person never moved. The API drops
/// the same fixes from the traces it draws.
pub const MAX_ACCURACY_M: f64 = 200.0;
/// Visits have no activity type of their own, but the day still spent that time somewhere.
const STATIONARY: &str = "stationary";

/// A place's country code and locality, the two fields a day summary borrows from it.
type PlaceInfo = (Option<String>, Option<String>);

struct Item {
    id: String,
    is_visit: bool,
    start_date: i64,
    end_date: i64,
    start_offset: i32,
    end_offset: i32,
    activity_type: Option<i32>,
    local_start_date: String,
    local_end_date: String,
    visit_place_id: Option<String>,
    visit_country_code: Option<String>,
    visit_locality: Option<String>,
    confirmation: Confirmation,
}

impl Item {
    fn type_name(&self) -> String {
        if self.is_visit {
            return STATIONARY.to_string();
        }
        ActivityType::from_raw(self.activity_type.unwrap_or(ActivityType::Unknown.raw()))
            .as_str()
            .into_owned()
    }

    fn clipped_ms(&self, date: Date) -> i64 {
        clipped_ms(
            self.start_date,
            self.end_date,
            self.start_offset,
            self.end_offset,
            date,
        )
    }
}

struct Sample {
    item_id: Option<String>,
    date: i64,
    seconds_from_gmt: Option<i32>,
    latitude: Option<f64>,
    longitude: Option<f64>,
    horizontal_accuracy: Option<f64>,
}

/// Recompute the `day_summaries` row for each of `dates` (`YYYY-MM-DD`). A day with neither an
/// item nor a sample left loses its row; gap days never get one.
pub fn recompute_days(conn: &mut Connection, dates: &[String]) -> Result<usize> {
    let dates: Vec<Date> = dates.iter().filter_map(|date| date.parse().ok()).collect();
    if dates.is_empty() {
        return Ok(0);
    }
    let (Some(min), Some(max)) = (dates.iter().min(), dates.iter().max()) else {
        return Ok(0);
    };

    let wanted: BTreeSet<Date> = dates.iter().copied().collect();
    let items = load_items(conn, &min.to_string(), &max.to_string())?;
    let by_date = bucket_by_date(&items, &wanted);
    let places = load_places(conn, &items)?;

    let computed_at = Timestamp::now().as_millisecond();
    let tx = conn.transaction()?;
    let mut written = 0;
    {
        let mut samples = tx.prepare(
            "SELECT timeline_item_id, date, seconds_from_gmt, latitude, longitude,
                    horizontal_accuracy
             FROM samples
             WHERE local_date = ? AND coalesce(disabled, 0) = 0
             ORDER BY timeline_item_id, date",
        )?;
        let mut insert = tx.prepare(
            "INSERT OR REPLACE INTO day_summaries (
                 date, utc_offset_seconds, item_count, visit_count, trip_count, sample_count,
                 distance_m, moving_seconds, distance_by_type, duration_by_type, place_ids,
                 country_codes, localities, min_lat, min_lon, max_lat, max_lon,
                 first_sample_at, last_sample_at, unconfirmed_items, confirmed, computed_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )?;
        let mut delete = tx.prepare("DELETE FROM day_summaries WHERE date = ?")?;

        for date in &wanted {
            let key = date.to_string();
            let day_items: Vec<&Item> = by_date
                .get(date)
                .map(|indexes| indexes.iter().map(|&i| &items[i]).collect())
                .unwrap_or_default();
            let day_samples: Vec<Sample> = samples
                .query_map(params![key], |row| {
                    Ok(Sample {
                        item_id: row.get(0)?,
                        date: row.get(1)?,
                        seconds_from_gmt: row.get(2)?,
                        latitude: row.get(3)?,
                        longitude: row.get(4)?,
                        horizontal_accuracy: row.get(5)?,
                    })
                })?
                .collect::<Result<_, _>>()?;

            if !is_a_day(&key, &day_items, &day_samples) {
                delete.execute(params![key])?;
                continue;
            }
            let summary = summarize(*date, &day_items, &day_samples, &places);
            let (min_lat, min_lon, max_lat, max_lon) = match summary.bbox {
                Some((min_lat, min_lon, max_lat, max_lon)) => {
                    (Some(min_lat), Some(min_lon), Some(max_lat), Some(max_lon))
                }
                None => (None, None, None, None),
            };
            insert.execute(params![
                key,
                summary.utc_offset_seconds,
                summary.item_count,
                summary.visit_count,
                summary.trip_count,
                summary.sample_count,
                summary.distance_m,
                summary.moving_seconds,
                summary.distance_by_type,
                summary.duration_by_type,
                summary.place_ids,
                summary.country_codes,
                summary.localities,
                min_lat,
                min_lon,
                max_lat,
                max_lon,
                summary.first_sample_at,
                summary.last_sample_at,
                summary.unconfirmed_items,
                summary.confirmed,
                computed_at,
            ])?;
            written += 1;
        }
    }
    tx.commit()?;
    Ok(written)
}

struct Summary {
    utc_offset_seconds: Option<i32>,
    item_count: i64,
    visit_count: i64,
    trip_count: i64,
    sample_count: i64,
    distance_m: f64,
    moving_seconds: i64,
    distance_by_type: String,
    duration_by_type: String,
    place_ids: String,
    country_codes: String,
    localities: String,
    bbox: Option<(f64, f64, f64, f64)>,
    first_sample_at: Option<i64>,
    last_sample_at: Option<i64>,
    unconfirmed_items: i64,
    /// True when every item counted on the day is confirmed, and so trivially true for a day
    /// that has only samples on it: there is nothing left for the user to review.
    confirmed: bool,
}

/// Whether a day happened at all. A sample or an item boundary is evidence that it did; an item
/// merely spanning the day is not, because a months-long recording gap shows up in Arc as one
/// bogus item stretched across it, and those days have no data in them to summarize.
fn is_a_day(date: &str, items: &[&Item], samples: &[Sample]) -> bool {
    !samples.is_empty()
        || items
            .iter()
            .any(|item| item.local_start_date == date || item.local_end_date == date)
}

fn summarize(
    date: Date,
    items: &[&Item],
    samples: &[Sample],
    places: &HashMap<String, PlaceInfo>,
) -> Summary {
    let by_id: HashMap<&str, &&Item> = items.iter().map(|item| (item.id.as_str(), item)).collect();

    let mut duration_by_type: BTreeMap<String, i64> = BTreeMap::new();
    let mut moving_seconds = 0;
    let (mut visit_count, mut trip_count) = (0, 0);
    let mut unconfirmed_items = 0;
    let mut place_ids: Vec<String> = Vec::new();
    let mut country_codes: Vec<String> = Vec::new();
    let mut localities: Vec<String> = Vec::new();

    for item in items {
        let seconds = item.clipped_ms(date) / 1000;
        *duration_by_type.entry(item.type_name()).or_default() += seconds;
        if !item.confirmation.confirmed {
            unconfirmed_items += 1;
        }
        if item.is_visit {
            visit_count += 1;
            let place = item
                .visit_place_id
                .as_deref()
                .and_then(|id| places.get(id).cloned());
            if let Some(id) = &item.visit_place_id {
                push_distinct(&mut place_ids, id);
            }
            // The place is the better source; a visit that never got one still knows roughly
            // where it was.
            let (country, locality) = place.unwrap_or((None, None));
            if let Some(code) = country.as_deref().or(item.visit_country_code.as_deref()) {
                push_distinct(&mut country_codes, code);
            }
            if let Some(name) = locality.as_deref().or(item.visit_locality.as_deref()) {
                push_distinct(&mut localities, name);
            }
        } else {
            trip_count += 1;
            moving_seconds += seconds;
        }
    }

    let mut distance_by_type: BTreeMap<String, f64> = BTreeMap::new();
    let mut distance_m = 0.0;
    let mut bbox: Option<(f64, f64, f64, f64)> = None;
    let mut offsets: HashMap<i32, usize> = HashMap::new();
    let mut previous: Option<(&str, i64, f64, f64)> = None;

    for sample in samples {
        if let Some(offset) = sample.seconds_from_gmt {
            *offsets.entry(offset).or_default() += 1;
        }
        let (Some(latitude), Some(longitude)) = (sample.latitude, sample.longitude) else {
            continue;
        };
        bbox = Some(match bbox {
            None => (latitude, longitude, latitude, longitude),
            Some((min_lat, min_lon, max_lat, max_lon)) => (
                min_lat.min(latitude),
                min_lon.min(longitude),
                max_lat.max(latitude),
                max_lon.max(longitude),
            ),
        });

        // An inaccurate fix is dropped from the chain entirely rather than bridged over, so the
        // next accurate pair is measured against the last accurate one.
        if sample.horizontal_accuracy.unwrap_or(0.0) > MAX_ACCURACY_M {
            continue;
        }
        let Some(item_id) = sample.item_id.as_deref() else {
            previous = None;
            continue;
        };
        if let Some((previous_id, previous_date, previous_lat, previous_lon)) = previous
            && previous_id == item_id
            && sample.date - previous_date <= MAX_SAMPLE_GAP_MS
            && let Some(item) = by_id.get(item_id)
            && !item.is_visit
        {
            let metres = haversine_m(previous_lat, previous_lon, latitude, longitude);
            distance_m += metres;
            *distance_by_type.entry(item.type_name()).or_default() += metres;
        }
        previous = Some((item_id, sample.date, latitude, longitude));
    }

    Summary {
        utc_offset_seconds: offsets
            .into_iter()
            // Ties go to the larger offset only so the result does not depend on hash order.
            .max_by_key(|&(offset, count)| (count, offset))
            .map(|(offset, _)| offset)
            .or_else(|| items.first().map(|item| item.start_offset)),
        item_count: items.len() as i64,
        visit_count,
        trip_count,
        sample_count: samples.len() as i64,
        distance_m,
        moving_seconds,
        distance_by_type: serde_json::to_string(&distance_by_type).unwrap_or_default(),
        duration_by_type: serde_json::to_string(&duration_by_type).unwrap_or_default(),
        place_ids: serde_json::to_string(&place_ids).unwrap_or_default(),
        country_codes: serde_json::to_string(&country_codes).unwrap_or_default(),
        localities: serde_json::to_string(&localities).unwrap_or_default(),
        bbox,
        // Not the first and last of the slice: it is ordered by item, not by time.
        first_sample_at: samples.iter().map(|sample| sample.date).min(),
        last_sample_at: samples.iter().map(|sample| sample.date).max(),
        unconfirmed_items,
        confirmed: unconfirmed_items == 0,
    }
}

fn push_distinct(values: &mut Vec<String>, value: &str) {
    if !values.iter().any(|existing| existing == value) {
        values.push(value.to_string());
    }
}

/// Deleted items are Arc's removal tombstones and disabled ones are what the user switched off;
/// neither is part of the day that happened.
fn load_items(conn: &Connection, min: &str, max: &str) -> Result<Vec<Item>> {
    let columns = confirmation::COLUMNS;
    let mut statement = conn.prepare(&format!(
        "SELECT id, is_visit, start_date, end_date, start_offset_seconds, end_offset_seconds,
                activity_type, local_start_date, local_end_date, visit_place_id,
                visit_country_code, visit_locality, {columns}
         FROM items
         WHERE coalesce(deleted, 0) = 0 AND coalesce(disabled, 0) = 0
           AND local_start_date IS NOT NULL AND local_end_date IS NOT NULL
           AND local_start_date <= ? AND local_end_date >= ?
         ORDER BY start_date, id"
    ))?;
    let items = statement
        .query_map(params![max, min], |row| {
            Ok(Item {
                id: row.get(0)?,
                is_visit: row.get(1)?,
                start_date: row.get(2)?,
                end_date: row.get(3)?,
                start_offset: row.get::<_, Option<i32>>(4)?.unwrap_or(0),
                end_offset: row.get::<_, Option<i32>>(5)?.unwrap_or(0),
                activity_type: row.get(6)?,
                local_start_date: row.get(7)?,
                local_end_date: row.get(8)?,
                visit_place_id: row.get(9)?,
                visit_country_code: row.get(10)?,
                visit_locality: row.get(11)?,
                confirmation: Confirmation::from_row(row, 12)?,
            })
        })?
        .collect::<Result<_, _>>()?;
    Ok(items)
}

fn bucket_by_date(items: &[Item], wanted: &BTreeSet<Date>) -> HashMap<Date, Vec<usize>> {
    let mut by_date: HashMap<Date, Vec<usize>> = HashMap::new();
    for (index, item) in items.iter().enumerate() {
        let (Ok(from), Ok(to)) = (
            item.local_start_date.parse::<Date>(),
            item.local_end_date.parse::<Date>(),
        ) else {
            continue;
        };
        for date in dates_in_range(from, to) {
            if wanted.contains(&date) {
                by_date.entry(date).or_default().push(index);
            }
        }
    }
    by_date
}

fn load_places(conn: &Connection, items: &[Item]) -> Result<HashMap<String, PlaceInfo>> {
    let mut statement = conn.prepare("SELECT country_code, locality FROM places WHERE id = ?")?;
    let mut places = HashMap::new();
    for id in items
        .iter()
        .filter_map(|item| item.visit_place_id.as_deref())
    {
        if places.contains_key(id) {
            continue;
        }
        let found = statement
            .query_row(params![id], |row| Ok((row.get(0)?, row.get(1)?)))
            .ok();
        if let Some(found) = found {
            places.insert(id.to_string(), found);
        }
    }
    Ok(places)
}

#[cfg(test)]
mod tests {
    use super::super::derive_items;
    use super::super::items::tests::{insert_item, insert_place, insert_sample, millis};
    use super::*;
    use crate::db;

    struct Row {
        utc_offset_seconds: Option<i32>,
        item_count: i64,
        visit_count: i64,
        trip_count: i64,
        sample_count: i64,
        distance_m: f64,
        moving_seconds: i64,
        distance_by_type: BTreeMap<String, f64>,
        duration_by_type: BTreeMap<String, i64>,
        place_ids: Vec<String>,
        country_codes: Vec<String>,
        localities: Vec<String>,
        bbox: Option<(f64, f64, f64, f64)>,
        first_sample_at: Option<i64>,
        last_sample_at: Option<i64>,
        unconfirmed_items: i64,
        confirmed: bool,
    }

    fn row(conn: &Connection, date: &str) -> Option<Row> {
        conn.query_row(
            "SELECT utc_offset_seconds, item_count, visit_count, trip_count, sample_count,
                    distance_m, moving_seconds, distance_by_type, duration_by_type, place_ids,
                    country_codes, localities, min_lat, min_lon, max_lat, max_lon,
                    first_sample_at, last_sample_at, unconfirmed_items, confirmed
             FROM day_summaries WHERE date = ?",
            params![date],
            |row| {
                let json = |index: usize| -> rusqlite::Result<String> { row.get(index) };
                let bbox = match (
                    row.get::<_, Option<f64>>(12)?,
                    row.get::<_, Option<f64>>(13)?,
                    row.get::<_, Option<f64>>(14)?,
                    row.get::<_, Option<f64>>(15)?,
                ) {
                    (Some(a), Some(b), Some(c), Some(d)) => Some((a, b, c, d)),
                    _ => None,
                };
                Ok(Row {
                    utc_offset_seconds: row.get(0)?,
                    item_count: row.get(1)?,
                    visit_count: row.get(2)?,
                    trip_count: row.get(3)?,
                    sample_count: row.get(4)?,
                    distance_m: row.get(5)?,
                    moving_seconds: row.get(6)?,
                    distance_by_type: serde_json::from_str(&json(7)?).unwrap(),
                    duration_by_type: serde_json::from_str(&json(8)?).unwrap(),
                    place_ids: serde_json::from_str(&json(9)?).unwrap(),
                    country_codes: serde_json::from_str(&json(10)?).unwrap(),
                    localities: serde_json::from_str(&json(11)?).unwrap(),
                    bbox,
                    first_sample_at: row.get(16)?,
                    last_sample_at: row.get(17)?,
                    unconfirmed_items: row.get(18)?,
                    confirmed: row.get(19)?,
                })
            },
        )
        .ok()
    }

    fn set_trip(conn: &Connection, id: &str, activity_type: i32) {
        conn.execute(
            "UPDATE items SET activity_type = ? WHERE id = ?",
            params![activity_type, id],
        )
        .unwrap();
    }

    /// A sample with a fix on it: `at` is (latitude, longitude, horizontal accuracy).
    fn positioned_sample(
        conn: &Connection,
        id: &str,
        item_id: &str,
        date: &str,
        offset: i32,
        at: (f64, f64, f64),
    ) {
        let (latitude, longitude, accuracy) = at;
        insert_sample(conn, id, item_id, date, Some(offset));
        conn.execute(
            "UPDATE samples SET latitude = ?, longitude = ?, horizontal_accuracy = ?
             WHERE id = ?",
            params![latitude, longitude, accuracy, id],
        )
        .unwrap();
    }

    fn derive(conn: &mut Connection, ids: &[&str], dates: &[&str]) {
        let ids: Vec<String> = ids.iter().map(|id| id.to_string()).collect();
        derive_items(conn, &ids).unwrap();
        let dates: Vec<String> = dates.iter().map(|date| date.to_string()).collect();
        recompute_days(conn, &dates).unwrap();
    }

    /// A late tram ride, 23:40 to 00:20 CEST, belongs to both days with 20 minutes in each.
    #[test]
    fn a_trip_over_local_midnight_lands_on_both_days() {
        let mut conn = db::open_in_memory().unwrap();
        insert_item(
            &conn,
            "tram",
            false,
            "2025-06-10T21:40:00Z",
            "2025-06-10T22:20:00Z",
        );
        set_trip(&conn, "tram", ActivityType::Tram.raw());
        // Hauptbahnhof before midnight, Postplatz after: about 1.07 km, one sample each side.
        positioned_sample(
            &conn,
            "s1",
            "tram",
            "2025-06-10T21:40:00Z",
            7200,
            (51.0403, 13.7320, 10.0),
        );
        positioned_sample(
            &conn,
            "s2",
            "tram",
            "2025-06-10T21:45:00Z",
            7200,
            (51.0499, 13.7333, 10.0),
        );
        positioned_sample(
            &conn,
            "s3",
            "tram",
            "2025-06-10T22:10:00Z",
            7200,
            (51.0403, 13.7320, 10.0),
        );
        positioned_sample(
            &conn,
            "s4",
            "tram",
            "2025-06-10T22:15:00Z",
            7200,
            (51.0499, 13.7333, 10.0),
        );

        derive(&mut conn, &["tram"], &["2025-06-10", "2025-06-11"]);

        let first = row(&conn, "2025-06-10").unwrap();
        let second = row(&conn, "2025-06-11").unwrap();
        assert_eq!(first.moving_seconds, 20 * 60);
        assert_eq!(second.moving_seconds, 20 * 60);
        assert_eq!(first.duration_by_type["tram"], 20 * 60);
        assert_eq!(second.duration_by_type["tram"], 20 * 60);
        assert_eq!((first.item_count, first.trip_count), (1, 1));
        assert_eq!((second.item_count, second.trip_count), (1, 1));

        // Each day's samples carry their own day's distance; the pair across midnight is in
        // neither, because a sample only ever belongs to one local day.
        let expected = haversine_m(51.0403, 13.7320, 51.0499, 13.7333);
        assert!(
            (first.distance_m - expected).abs() < 1.0,
            "{first:?}",
            first = first.distance_m
        );
        assert!((second.distance_m - expected).abs() < 1.0);
        assert!((first.distance_by_type["tram"] - expected).abs() < 1.0);
        assert_eq!(first.sample_count, 2);
        assert_eq!(second.sample_count, 2);
        assert_eq!(first.last_sample_at, Some(millis("2025-06-10T21:45:00Z")));
        assert_eq!(second.first_sample_at, Some(millis("2025-06-10T22:10:00Z")));
    }

    /// The night Europe/Berlin falls back: 26 October 2025 is 25 hours long and nothing about
    /// clipping may produce a negative or a 25-hour item.
    #[test]
    fn a_dst_transition_night_stays_sane() {
        let mut conn = db::open_in_memory().unwrap();
        // 01:30 CEST (+7200) to 02:30 CET (+3600), an hour of wall clock across the switch.
        insert_item(
            &conn,
            "night",
            false,
            "2025-10-25T23:30:00Z",
            "2025-10-26T01:30:00Z",
        );
        set_trip(&conn, "night", ActivityType::Walking.raw());
        insert_sample(&conn, "n1", "night", "2025-10-25T23:30:00Z", Some(7200));
        insert_sample(&conn, "n2", "night", "2025-10-26T01:30:00Z", Some(3600));
        // A daytime visit on the 25th so that day exists too.
        insert_item(
            &conn,
            "day25",
            true,
            "2025-10-25T08:00:00Z",
            "2025-10-25T09:00:00Z",
        );
        insert_sample(&conn, "d1", "day25", "2025-10-25T08:00:00Z", Some(7200));
        // A daytime visit on the 26th, after the switch, so the modal offset has a majority.
        insert_item(
            &conn,
            "day26",
            true,
            "2025-10-26T08:00:00Z",
            "2025-10-26T09:00:00Z",
        );
        insert_sample(&conn, "d2", "day26", "2025-10-26T08:00:00Z", Some(3600));
        insert_sample(&conn, "d3", "day26", "2025-10-26T08:30:00Z", Some(3600));

        derive(
            &mut conn,
            &["night", "day25", "day26"],
            &["2025-10-25", "2025-10-26"],
        );

        let before = row(&conn, "2025-10-25").unwrap();
        let after = row(&conn, "2025-10-26").unwrap();
        assert_eq!(before.utc_offset_seconds, Some(7200));
        assert_eq!(after.utc_offset_seconds, Some(3600));
        // The item runs 01:30 CEST to 02:30 CET, two hours of elapsed time, all on the 26th.
        assert_eq!(before.moving_seconds, 0);
        assert_eq!(after.moving_seconds, 2 * 3600);
        for summary in [&before, &after] {
            for seconds in summary.duration_by_type.values() {
                assert!(*seconds >= 0, "negative duration");
                assert!(*seconds <= 25 * 3600, "longer than the longest local day");
            }
        }
    }

    /// A flight leaving CEST and landing in UTC: two offsets, and the local dates follow them.
    #[test]
    fn a_flight_summarizes_under_both_offsets() {
        let mut conn = db::open_in_memory().unwrap();
        insert_item(
            &conn,
            "flight",
            false,
            "2025-06-10T21:00:00Z",
            "2025-06-10T23:30:00Z",
        );
        set_trip(&conn, "flight", ActivityType::Airplane.raw());
        insert_sample(&conn, "f1", "flight", "2025-06-10T21:00:00Z", Some(7200));
        insert_sample(&conn, "f2", "flight", "2025-06-10T23:30:00Z", Some(0));

        derive(&mut conn, &["flight"], &["2025-06-10", "2025-06-11"]);

        let (start, end): (String, String) = conn
            .query_row(
                "SELECT local_start_date, local_end_date FROM items WHERE id = 'flight'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!((start.as_str(), end.as_str()), ("2025-06-10", "2025-06-10"));

        let day = row(&conn, "2025-06-10").unwrap();
        // Takeoff 23:00 CEST, landing 23:30 UTC: the day window ends at 00:00 UTC, so the
        // whole two and a half hours sit in the 10th.
        assert_eq!(day.duration_by_type["airplane"], 150 * 60);
        assert!(row(&conn, "2025-06-11").is_none());
    }

    #[test]
    fn distance_skips_long_gaps_and_inaccurate_fixes() {
        let mut conn = db::open_in_memory().unwrap();
        insert_item(
            &conn,
            "walk",
            false,
            "2025-06-10T08:00:00Z",
            "2025-06-10T09:00:00Z",
        );
        set_trip(&conn, "walk", ActivityType::Walking.raw());
        let leg = haversine_m(51.0403, 13.7320, 51.0499, 13.7333);
        // One good leg.
        positioned_sample(
            &conn,
            "a",
            "walk",
            "2025-06-10T08:00:00Z",
            7200,
            (51.0403, 13.7320, 10.0),
        );
        positioned_sample(
            &conn,
            "b",
            "walk",
            "2025-06-10T08:01:00Z",
            7200,
            (51.0499, 13.7333, 10.0),
        );
        // Arc slept for 20 minutes: no straight line across the gap.
        positioned_sample(
            &conn,
            "c",
            "walk",
            "2025-06-10T08:21:00Z",
            7200,
            (51.0403, 13.7320, 10.0),
        );
        // A garbage fix in the middle is dropped, and the leg is measured around it.
        positioned_sample(
            &conn,
            "d",
            "walk",
            "2025-06-10T08:22:00Z",
            7200,
            (52.5251, 13.3694, 900.0),
        );
        positioned_sample(
            &conn,
            "e",
            "walk",
            "2025-06-10T08:23:00Z",
            7200,
            (51.0499, 13.7333, 10.0),
        );

        derive(&mut conn, &["walk"], &["2025-06-10"]);

        let day = row(&conn, "2025-06-10").unwrap();
        assert!(
            (day.distance_m - 2.0 * leg).abs() < 1.0,
            "{}",
            day.distance_m
        );
        assert!((day.distance_by_type["walking"] - 2.0 * leg).abs() < 1.0);
        // The inaccurate fix still counts as a sample and still stretches the bounding box.
        assert_eq!(day.sample_count, 5);
        let (min_lat, _, max_lat, _) = day.bbox.unwrap();
        assert!((min_lat - 51.0403).abs() < 1e-9);
        assert!((max_lat - 52.5251).abs() < 1e-9);
    }

    #[test]
    fn distance_never_crosses_an_item_boundary_and_never_counts_a_visit() {
        let mut conn = db::open_in_memory().unwrap();
        insert_item(
            &conn,
            "a_trip",
            false,
            "2025-06-10T08:00:00Z",
            "2025-06-10T08:10:00Z",
        );
        set_trip(&conn, "a_trip", ActivityType::Cycling.raw());
        insert_item(
            &conn,
            "b_visit",
            true,
            "2025-06-10T08:10:00Z",
            "2025-06-10T08:20:00Z",
        );
        positioned_sample(
            &conn,
            "1",
            "a_trip",
            "2025-06-10T08:00:00Z",
            7200,
            (51.0403, 13.7320, 10.0),
        );
        positioned_sample(
            &conn,
            "2",
            "a_trip",
            "2025-06-10T08:05:00Z",
            7200,
            (51.0499, 13.7333, 10.0),
        );
        // Same coordinates, but inside a visit and after an item change: no distance at all.
        positioned_sample(
            &conn,
            "3",
            "b_visit",
            "2025-06-10T08:11:00Z",
            7200,
            (51.0403, 13.7320, 10.0),
        );
        positioned_sample(
            &conn,
            "4",
            "b_visit",
            "2025-06-10T08:15:00Z",
            7200,
            (51.0499, 13.7333, 10.0),
        );

        derive(&mut conn, &["a_trip", "b_visit"], &["2025-06-10"]);

        let day = row(&conn, "2025-06-10").unwrap();
        let leg = haversine_m(51.0403, 13.7320, 51.0499, 13.7333);
        assert!((day.distance_m - leg).abs() < 1.0, "{}", day.distance_m);
        assert_eq!(day.distance_by_type.keys().collect::<Vec<_>>(), ["cycling"]);
        assert_eq!(day.duration_by_type["stationary"], 10 * 60);
        assert_eq!((day.visit_count, day.trip_count), (1, 1));
    }

    #[test]
    fn visits_contribute_places_countries_and_localities_in_order() {
        let mut conn = db::open_in_memory().unwrap();
        insert_place(&conn, "hbf", Some(7200));
        insert_place(&conn, "postplatz", Some(7200));
        conn.execute(
            "UPDATE places SET country_code = 'de', locality = 'Dresden'",
            [],
        )
        .unwrap();
        insert_item(
            &conn,
            "v1",
            true,
            "2025-06-10T08:00:00Z",
            "2025-06-10T08:20:00Z",
        );
        insert_item(
            &conn,
            "v2",
            true,
            "2025-06-10T09:00:00Z",
            "2025-06-10T09:20:00Z",
        );
        insert_item(
            &conn,
            "v3",
            true,
            "2025-06-10T10:00:00Z",
            "2025-06-10T10:20:00Z",
        );
        conn.execute(
            "UPDATE items SET visit_place_id = 'postplatz' WHERE id = 'v1'",
            [],
        )
        .unwrap();
        conn.execute(
            "UPDATE items SET visit_place_id = 'hbf' WHERE id = 'v2'",
            [],
        )
        .unwrap();
        // A visit whose place was never ingested falls back to its own columns.
        conn.execute(
            "UPDATE items SET visit_place_id = 'missing', visit_country_code = 'cz',
                    visit_locality = 'Decin' WHERE id = 'v3'",
            [],
        )
        .unwrap();

        derive(&mut conn, &["v1", "v2", "v3"], &["2025-06-10"]);

        let day = row(&conn, "2025-06-10").unwrap();
        assert_eq!(day.place_ids, ["postplatz", "hbf", "missing"]);
        assert_eq!(day.country_codes, ["de", "cz"]);
        assert_eq!(day.localities, ["Dresden", "Decin"]);
        assert_eq!(day.visit_count, 3);
    }

    #[test]
    fn deleted_and_disabled_items_are_excluded_and_an_empty_day_loses_its_row() {
        let mut conn = db::open_in_memory().unwrap();
        insert_item(
            &conn,
            "gone",
            true,
            "2025-06-10T08:00:00Z",
            "2025-06-10T09:00:00Z",
        );
        insert_item(
            &conn,
            "off",
            false,
            "2025-06-10T09:00:00Z",
            "2025-06-10T10:00:00Z",
        );
        insert_item(
            &conn,
            "real",
            true,
            "2025-06-10T10:00:00Z",
            "2025-06-10T11:00:00Z",
        );
        insert_sample(&conn, "s", "real", "2025-06-10T10:30:00Z", Some(7200));

        derive(&mut conn, &["gone", "off", "real"], &["2025-06-10"]);
        assert_eq!(row(&conn, "2025-06-10").unwrap().item_count, 3);

        conn.execute("UPDATE items SET deleted = 1 WHERE id = 'gone'", [])
            .unwrap();
        conn.execute("UPDATE items SET disabled = 1 WHERE id = 'off'", [])
            .unwrap();
        recompute_days(&mut conn, &["2025-06-10".into()]).unwrap();
        assert_eq!(row(&conn, "2025-06-10").unwrap().item_count, 1);

        // With the last live item gone and its sample deleted too, the day is not a day.
        conn.execute("UPDATE items SET deleted = 1 WHERE id = 'real'", [])
            .unwrap();
        conn.execute("DELETE FROM samples", []).unwrap();
        recompute_days(&mut conn, &["2025-06-10".into()]).unwrap();
        assert!(row(&conn, "2025-06-10").is_none());
    }

    #[test]
    fn a_gap_day_never_gets_a_row() {
        let mut conn = db::open_in_memory().unwrap();
        insert_item(
            &conn,
            "v",
            true,
            "2025-06-10T08:00:00Z",
            "2025-06-10T09:00:00Z",
        );
        insert_sample(&conn, "s", "v", "2025-06-10T08:30:00Z", Some(7200));

        derive(
            &mut conn,
            &["v"],
            &["2025-06-09", "2025-06-10", "2025-06-11"],
        );

        assert!(row(&conn, "2025-06-09").is_none());
        assert!(row(&conn, "2025-06-10").is_some());
        assert!(row(&conn, "2025-06-11").is_none());
        let count: i64 = conn
            .query_row("SELECT count(*) FROM day_summaries", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 1);
    }

    /// A multi-day visit covers the days in the middle too, as long as they recorded something.
    #[test]
    fn a_long_visit_covers_every_day_it_spans() {
        let mut conn = db::open_in_memory().unwrap();
        insert_item(
            &conn,
            "long",
            true,
            "2025-06-09T22:00:00Z",
            "2025-06-12T10:00:00Z",
        );
        insert_sample(&conn, "s", "long", "2025-06-09T23:00:00Z", Some(7200));
        insert_sample(&conn, "s10", "long", "2025-06-10T12:00:00Z", Some(7200));
        insert_sample(&conn, "s11", "long", "2025-06-11T12:00:00Z", Some(7200));

        derive(
            &mut conn,
            &["long"],
            &["2025-06-10", "2025-06-11", "2025-06-12"],
        );

        for date in ["2025-06-10", "2025-06-11", "2025-06-12"] {
            let day = row(&conn, date).unwrap();
            assert_eq!(day.item_count, 1, "{date}");
        }
        // A full day in the middle is 24 hours of stationary time.
        assert_eq!(
            row(&conn, "2025-06-11").unwrap().duration_by_type["stationary"],
            24 * 3600
        );
        assert_eq!(
            row(&conn, "2025-06-12").unwrap().duration_by_type["stationary"],
            12 * 3600
        );
    }

    /// Arc renders a months-long recording gap as one stretched item with a couple of samples.
    /// The days inside it recorded nothing and must not turn into summaries.
    #[test]
    fn an_item_stretched_over_a_recording_gap_leaves_the_gap_empty() {
        let mut conn = db::open_in_memory().unwrap();
        insert_item(
            &conn,
            "gap",
            false,
            "2025-06-09T22:00:00Z",
            "2025-06-13T10:00:00Z",
        );
        set_trip(&conn, "gap", ActivityType::Tram.raw());
        insert_sample(&conn, "s", "gap", "2025-06-09T23:00:00Z", Some(7200));
        insert_sample(&conn, "e", "gap", "2025-06-13T09:00:00Z", Some(7200));

        derive(
            &mut conn,
            &["gap"],
            &["2025-06-10", "2025-06-11", "2025-06-12", "2025-06-13"],
        );

        // Only the days the item starts and ends on have anything to say.
        assert_eq!(row(&conn, "2025-06-10").unwrap().item_count, 1);
        assert_eq!(row(&conn, "2025-06-13").unwrap().item_count, 1);
        assert!(row(&conn, "2025-06-11").is_none());
        assert!(row(&conn, "2025-06-12").is_none());
    }

    #[test]
    fn first_and_last_sample_are_by_time_not_by_item() {
        let mut conn = db::open_in_memory().unwrap();
        // Two items whose ids sort the opposite way round from their samples' times.
        insert_item(
            &conn,
            "z_early",
            false,
            "2025-06-10T06:00:00Z",
            "2025-06-10T07:00:00Z",
        );
        insert_item(
            &conn,
            "a_late",
            false,
            "2025-06-10T18:00:00Z",
            "2025-06-10T19:00:00Z",
        );
        insert_sample(&conn, "1", "z_early", "2025-06-10T06:30:00Z", Some(7200));
        insert_sample(&conn, "2", "a_late", "2025-06-10T18:30:00Z", Some(7200));

        derive(&mut conn, &["z_early", "a_late"], &["2025-06-10"]);

        let day = row(&conn, "2025-06-10").unwrap();
        assert_eq!(day.first_sample_at, Some(millis("2025-06-10T06:30:00Z")));
        assert_eq!(day.last_sample_at, Some(millis("2025-06-10T18:30:00Z")));
    }

    /// A consumer reads `confirmed` to know whether the day's places and activity types are
    /// the user's own or still Arc's guesses.
    #[test]
    fn a_day_is_confirmed_only_once_every_item_on_it_is() {
        let mut conn = db::open_in_memory().unwrap();
        insert_item(
            &conn,
            "visit",
            true,
            "2025-06-10T06:00:00Z",
            "2025-06-10T07:00:00Z",
        );
        insert_item(
            &conn,
            "trip",
            false,
            "2025-06-10T07:00:00Z",
            "2025-06-10T08:00:00Z",
        );
        insert_sample(&conn, "s1", "visit", "2025-06-10T06:30:00Z", Some(7200));
        insert_sample(&conn, "s2", "trip", "2025-06-10T07:30:00Z", Some(7200));
        conn.execute(
            "UPDATE items SET visit_confirmed_place = 1 WHERE id = 'visit'",
            [],
        )
        .unwrap();

        derive(&mut conn, &["visit", "trip"], &["2025-06-10"]);

        // The trip has no confirmed activity type yet, so the day is not reviewed.
        let day = row(&conn, "2025-06-10").unwrap();
        assert_eq!(day.unconfirmed_items, 1);
        assert!(!day.confirmed);

        conn.execute(
            "UPDATE items SET trip_confirmed_activity_type = 24 WHERE id = 'trip'",
            [],
        )
        .unwrap();
        derive(&mut conn, &["visit", "trip"], &["2025-06-10"]);

        let day = row(&conn, "2025-06-10").unwrap();
        assert_eq!(day.unconfirmed_items, 0);
        assert!(day.confirmed);
    }

    /// A day of nothing but samples has nothing to review.
    #[test]
    fn a_day_without_items_is_confirmed() {
        let mut conn = db::open_in_memory().unwrap();
        insert_sample(&conn, "s1", "gone", "2025-06-10T06:30:00Z", Some(7200));

        derive(&mut conn, &[], &["2025-06-10"]);

        let day = row(&conn, "2025-06-10").unwrap();
        assert_eq!(day.item_count, 0);
        assert_eq!(day.unconfirmed_items, 0);
        assert!(day.confirmed);
    }

    #[test]
    fn no_dates_is_a_no_op() {
        let mut conn = db::open_in_memory().unwrap();
        assert_eq!(recompute_days(&mut conn, &[]).unwrap(), 0);
        assert_eq!(
            recompute_days(&mut conn, &["not a date".into()]).unwrap(),
            0
        );
    }
}
