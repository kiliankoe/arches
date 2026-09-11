//! Record upserts. Nothing here ever deletes: Arc keeps removed items around with `deleted`
//! set, and a bucket file that no longer mentions a record is not evidence that it is gone.

use std::sync::LazyLock;

use anyhow::Result;
use jiff::Timestamp;
use rusqlite::{Transaction, params};

use crate::arc::types::{LocomotionSample, Place, TimelineItem};
use crate::derive::local_date;

const PLACE_COLUMNS: &[&str] = &[
    "id",
    "name",
    "latitude",
    "longitude",
    "radius_mean",
    "radius_sd",
    "street_address",
    "locality",
    "country_code",
    "seconds_from_gmt",
    "is_stale",
    "visit_count",
    "visit_days",
    "last_visit_date",
    "last_saved",
    "source",
    "category",
    "user_category",
    "mapbox_place_id",
    "mapbox_category",
    "mapbox_maki_icon",
    "google_place_id",
    "google_primary_type",
    "foursquare_place_id",
    "foursquare_category_id",
    "foursquare_category_v2_id",
];

const ITEM_COLUMNS: &[&str] = &[
    "id",
    "is_visit",
    "start_date",
    "end_date",
    "last_saved",
    "source",
    "source_version",
    "disabled",
    "deleted",
    "previous_item_id",
    "next_item_id",
    "locked",
    "step_count",
    "floors_ascended",
    "floors_descended",
    "average_altitude",
    "active_energy_burned",
    "average_heart_rate",
    "max_heart_rate",
    "visit_latitude",
    "visit_longitude",
    "visit_radius_mean",
    "visit_radius_sd",
    "visit_place_id",
    "visit_confirmed_place",
    "visit_uncertain_place",
    "visit_custom_title",
    "visit_street_address",
    "visit_locality",
    "visit_country_code",
    "trip_distance",
    "trip_speed",
    "trip_classified_activity_type",
    "trip_confirmed_activity_type",
    "trip_uncertain_activity_type",
    "activity_type",
];

const SAMPLE_COLUMNS: &[&str] = &[
    "id",
    "date",
    "last_saved",
    "source",
    "timeline_item_id",
    "seconds_from_gmt",
    "moving_state",
    "recording_state",
    "disabled",
    "latitude",
    "longitude",
    "altitude",
    "horizontal_accuracy",
    "vertical_accuracy",
    "speed",
    "course",
    "step_hz",
    "heart_rate",
    "classified_activity_type",
    "confirmed_activity_type",
    "local_date",
];

static PLACE_SQL: LazyLock<String> = LazyLock::new(|| upsert_sql("places", PLACE_COLUMNS));
static ITEM_SQL: LazyLock<String> = LazyLock::new(|| upsert_sql("items", ITEM_COLUMNS));
static SAMPLE_SQL: LazyLock<String> = LazyLock::new(|| upsert_sql("samples", SAMPLE_COLUMNS));

/// Upsert guarded on `last_saved`: Arc rewrites whole buckets retroactively, and an older
/// rendering of a record must never overwrite a newer one. A conflict that fails the guard
/// reports zero changed rows, which is exactly what the run counters want to see.
fn upsert_sql(table: &str, columns: &[&str]) -> String {
    let names = columns.join(", ");
    let placeholders = vec!["?"; columns.len()].join(", ");
    let assignments = columns
        .iter()
        .filter(|column| **column != "id")
        .map(|column| format!("{column} = excluded.{column}"))
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        "INSERT INTO {table} ({names}) VALUES ({placeholders}) \
         ON CONFLICT(id) DO UPDATE SET {assignments} \
         WHERE excluded.last_saved > {table}.last_saved"
    )
}

fn millis(timestamp: Timestamp) -> i64 {
    timestamp.as_millisecond()
}

fn millis_opt(timestamp: Option<Timestamp>) -> Option<i64> {
    timestamp.map(millis)
}

pub fn upsert_places(tx: &Transaction<'_>, places: &[Place]) -> Result<usize> {
    let mut statement = tx.prepare_cached(&PLACE_SQL)?;
    let mut changed = 0;
    for place in places {
        changed += statement.execute(params![
            place.id,
            place.name,
            place.latitude,
            place.longitude,
            place.radius_mean,
            place.radius_sd,
            place.street_address,
            place.locality,
            place.country_code,
            place.seconds_from_gmt,
            place.is_stale,
            place.visit_count,
            place.visit_days,
            millis_opt(place.last_visit_date),
            millis(place.last_saved),
            place.source,
            place.category,
            place.user_category,
            place.mapbox_place_id,
            place.mapbox_category,
            place.mapbox_maki_icon,
            place.google_place_id,
            place.google_primary_type,
            place.foursquare_place_id,
            place.foursquare_category_id,
            place.foursquare_category_v2_id,
        ])?;
    }
    Ok(changed)
}

pub fn upsert_items(tx: &Transaction<'_>, items: &[TimelineItem]) -> Result<usize> {
    let mut statement = tx.prepare_cached(&ITEM_SQL)?;
    let mut changed = 0;
    for item in items {
        let base = &item.base;
        let visit = item.visit.as_ref();
        let trip = item.trip.as_ref();
        changed += statement.execute(params![
            base.id,
            base.is_visit,
            millis(base.start_date),
            millis(base.end_date),
            millis(base.last_saved),
            base.source,
            base.source_version,
            base.disabled,
            base.deleted,
            base.previous_item_id,
            base.next_item_id,
            base.locked,
            base.step_count,
            base.floors_ascended,
            base.floors_descended,
            base.average_altitude,
            base.active_energy_burned,
            base.average_heart_rate,
            base.max_heart_rate,
            visit.and_then(|visit| visit.latitude),
            visit.and_then(|visit| visit.longitude),
            visit.and_then(|visit| visit.radius_mean),
            visit.and_then(|visit| visit.radius_sd),
            visit.and_then(|visit| visit.place_id.as_deref()),
            visit.and_then(|visit| visit.confirmed_place),
            visit.and_then(|visit| visit.uncertain_place),
            visit.and_then(|visit| visit.custom_title.as_deref()),
            visit.and_then(|visit| visit.street_address.as_deref()),
            visit.and_then(|visit| visit.locality.as_deref()),
            visit.and_then(|visit| visit.country_code.as_deref()),
            trip.and_then(|trip| trip.distance),
            trip.and_then(|trip| trip.speed),
            trip.and_then(|trip| trip.classified_activity_type.map(|kind| kind.raw())),
            trip.and_then(|trip| trip.confirmed_activity_type.map(|kind| kind.raw())),
            trip.and_then(|trip| trip.uncertain_activity_type),
            item.activity_type().map(|kind| kind.raw()),
        ])?;
    }
    Ok(changed)
}

pub fn upsert_samples(tx: &Transaction<'_>, samples: &[LocomotionSample]) -> Result<usize> {
    let mut statement = tx.prepare_cached(&SAMPLE_SQL)?;
    let mut changed = 0;
    for sample in samples {
        changed += statement.execute(params![
            sample.id,
            millis(sample.date),
            millis(sample.last_saved),
            sample.source,
            sample.timeline_item_id,
            sample.seconds_from_gmt,
            sample.moving_state.map(|state| state.raw()),
            sample.recording_state.map(|state| state.raw()),
            sample.disabled,
            sample.latitude,
            sample.longitude,
            sample.altitude,
            sample.horizontal_accuracy,
            sample.vertical_accuracy,
            sample.speed,
            sample.course,
            sample.step_hz,
            sample.heart_rate,
            sample.classified_activity_type.map(|kind| kind.raw()),
            sample.confirmed_activity_type.map(|kind| kind.raw()),
            // Derived here rather than in a later pass so it can never drift from the row.
            sample
                .seconds_from_gmt
                .map(|offset| local_date(sample.date, offset)),
        ])?;
    }
    Ok(changed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;

    fn place(last_saved: &str, name: &str) -> Place {
        serde_json::from_str(&format!(
            r#"{{
                "id": "P0000000-0000-4000-8000-000000000001",
                "name": "{name}",
                "latitude": 51.04,
                "longitude": 13.73,
                "lastSaved": "{last_saved}"
            }}"#
        ))
        .unwrap()
    }

    #[test]
    fn upsert_applies_only_newer_records() {
        let mut conn = db::open_in_memory().unwrap();
        let name = |conn: &rusqlite::Connection| -> String {
            conn.query_row("SELECT name FROM places", [], |row| row.get(0))
                .unwrap()
        };

        let tx = conn.transaction().unwrap();
        assert_eq!(
            upsert_places(&tx, &[place("2025-06-10T10:00:00Z", "first")]).unwrap(),
            1
        );
        tx.commit().unwrap();
        assert_eq!(name(&conn), "first");

        let tx = conn.transaction().unwrap();
        assert_eq!(
            upsert_places(&tx, &[place("2025-06-09T10:00:00Z", "older")]).unwrap(),
            0
        );
        assert_eq!(
            upsert_places(&tx, &[place("2025-06-11T10:00:00Z", "newer")]).unwrap(),
            1
        );
        tx.commit().unwrap();
        assert_eq!(name(&conn), "newer");
    }

    #[test]
    fn item_activity_type_resolves_confirmed_over_classified() {
        let mut conn = db::open_in_memory().unwrap();
        let items: Vec<TimelineItem> = serde_json::from_str(
            r#"[{
                "base": {
                    "id": "B2000000-0000-4000-8000-000000000102",
                    "isVisit": false,
                    "startDate": "2025-06-10T08:20:00Z",
                    "endDate": "2025-06-10T08:28:00Z",
                    "lastSaved": "2025-06-10T09:30:00Z"
                },
                "trip": {
                    "itemId": "B2000000-0000-4000-8000-000000000102",
                    "classifiedActivityType": 5,
                    "confirmedActivityType": 24,
                    "lastSaved": "2025-06-10T09:30:00Z"
                }
            }]"#,
        )
        .unwrap();

        let tx = conn.transaction().unwrap();
        upsert_items(&tx, &items).unwrap();
        tx.commit().unwrap();

        let (activity, start): (i64, i64) = conn
            .query_row("SELECT activity_type, start_date FROM items", [], |row| {
                Ok((row.get(0)?, row.get(1)?))
            })
            .unwrap();
        assert_eq!(activity, 24);
        assert_eq!(
            start,
            "2025-06-10T08:20:00Z"
                .parse::<Timestamp>()
                .unwrap()
                .as_millisecond()
        );
    }
}
