//! The response shapes shared by several endpoints, and the queries behind them.
//!
//! Handlers stay thin: everything that knows SQL or column order lives here.

use std::collections::HashMap;

use anyhow::Result;
use jiff::civil::Date;
use rusqlite::{Connection, OptionalExtension, Row, params};
use serde::Serialize;

use super::convert::rfc3339;
use crate::arc::enums::{ActivityType, MovingState};
use crate::confirmation::{self, Confirmation};
use crate::derive;

/// Columns 0..=20 of an item row. The five [`confirmation::COLUMNS`] follow at 21.
const ITEM_COLUMNS: &str = "id, is_visit, start_date, end_date, local_start_date, local_end_date,
     start_offset_seconds, end_offset_seconds, activity_type, trip_distance, step_count,
     floors_ascended, floors_descended, average_altitude, active_energy_burned,
     average_heart_rate, max_heart_rate, visit_latitude, visit_longitude, visit_custom_title,
     visit_place_id";

/// Arc keeps removed items as tombstones and the user can switch an item off; neither is part
/// of the timeline that happened, here or in the derivation.
pub const LIVE_ITEM: &str = "coalesce(deleted, 0) = 0 AND coalesce(disabled, 0) = 0";

pub struct ItemRow {
    pub id: String,
    pub is_visit: bool,
    pub start_date: i64,
    pub end_date: i64,
    local_start_date: Option<String>,
    local_end_date: Option<String>,
    start_offset_seconds: Option<i32>,
    end_offset_seconds: Option<i32>,
    activity_type: Option<i32>,
    pub distance_m: Option<f64>,
    health: Health,
    visit_latitude: Option<f64>,
    visit_longitude: Option<f64>,
    pub visit_custom_title: Option<String>,
    pub visit_place_id: Option<String>,
    confirmation: Confirmation,
}

impl ItemRow {
    fn from_row(row: &Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            id: row.get(0)?,
            is_visit: row.get(1)?,
            start_date: row.get(2)?,
            end_date: row.get(3)?,
            local_start_date: row.get(4)?,
            local_end_date: row.get(5)?,
            start_offset_seconds: row.get(6)?,
            end_offset_seconds: row.get(7)?,
            activity_type: row.get(8)?,
            distance_m: row.get(9)?,
            health: Health {
                step_count: row.get(10)?,
                floors_ascended: row.get(11)?,
                floors_descended: row.get(12)?,
                average_altitude: row.get(13)?,
                active_energy_burned: row.get(14)?,
                average_heart_rate: row.get(15)?,
                max_heart_rate: row.get(16)?,
            },
            visit_latitude: row.get(17)?,
            visit_longitude: row.get(18)?,
            visit_custom_title: row.get(19)?,
            visit_place_id: row.get(20)?,
            confirmation: Confirmation::from_row(row, 21)?,
        })
    }

    pub fn activity_type(&self) -> Option<String> {
        self.activity_type
            .map(|raw| ActivityType::from_raw(raw).as_str().into_owned())
    }

    pub fn confirmation(&self) -> Confirmation {
        self.confirmation
    }

    /// The coordinates to draw a visit at: the place is the better fix, the visit's own is the
    /// fallback for one that never got a place.
    pub fn visit_coordinates(&self, place: Option<&Place>) -> Option<(f64, f64)> {
        place.map(|place| (place.latitude, place.longitude)).or(
            match (self.visit_latitude, self.visit_longitude) {
                (Some(latitude), Some(longitude)) => Some((latitude, longitude)),
                _ => None,
            },
        )
    }

    /// `clipped_to` is the local day to measure the item inside, for the day timeline; item
    /// endpoints pass `None` and report the full duration only.
    pub fn to_json(&self, place: Option<Place>, clipped_to: Option<Date>) -> Item {
        Item {
            id: self.id.clone(),
            kind: if self.is_visit { "visit" } else { "trip" },
            start_date: rfc3339(self.start_date),
            end_date: rfc3339(self.end_date),
            local_start_date: self.local_start_date.clone(),
            local_end_date: self.local_end_date.clone(),
            start_offset_seconds: self.start_offset_seconds,
            end_offset_seconds: self.end_offset_seconds,
            duration_seconds: (self.end_date - self.start_date).max(0) / 1000,
            clipped_seconds: clipped_to.map(|date| {
                derive::clipped_ms(
                    self.start_date,
                    self.end_date,
                    self.start_offset_seconds.unwrap_or(0),
                    self.end_offset_seconds.unwrap_or(0),
                    date,
                ) / 1000
            }),
            activity_type: self.activity_type(),
            distance_m: self.distance_m,
            confirmed: self.confirmation.confirmed,
            uncertain: self.confirmation.uncertain,
            health: self.health,
            latitude: self.visit_latitude,
            longitude: self.visit_longitude,
            custom_title: self.visit_custom_title.clone(),
            place,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Item {
    pub id: String,
    /// `"visit"` or `"trip"`.
    pub kind: &'static str,
    pub start_date: Option<String>,
    pub end_date: Option<String>,
    pub local_start_date: Option<String>,
    pub local_end_date: Option<String>,
    pub start_offset_seconds: Option<i32>,
    pub end_offset_seconds: Option<i32>,
    pub duration_seconds: i64,
    /// Seconds of the item inside the requested day, null outside a day timeline.
    pub clipped_seconds: Option<i64>,
    pub activity_type: Option<String>,
    pub distance_m: Option<f64>,
    pub confirmed: bool,
    pub uncertain: bool,
    pub health: Health,
    /// The visit's own coordinates and title; null on a trip.
    pub latitude: Option<f64>,
    pub longitude: Option<f64>,
    pub custom_title: Option<String>,
    pub place: Option<Place>,
}

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Health {
    pub step_count: Option<f64>,
    pub floors_ascended: Option<f64>,
    pub floors_descended: Option<f64>,
    pub average_altitude: Option<f64>,
    pub active_energy_burned: Option<f64>,
    pub average_heart_rate: Option<f64>,
    pub max_heart_rate: Option<f64>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Place {
    pub id: String,
    pub name: String,
    pub street_address: Option<String>,
    pub locality: Option<String>,
    pub country_code: Option<String>,
    pub latitude: f64,
    pub longitude: f64,
    pub radius_mean: Option<f64>,
    pub visit_count: Option<i64>,
    pub visit_days: Option<i64>,
    pub last_visit_date: Option<String>,
    pub category: Option<String>,
    pub is_stale: Option<bool>,
    /// Only set by `/api/near`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub distance_m: Option<f64>,
}

pub const PLACE_COLUMNS: &str = "id, name, street_address, locality, country_code, latitude,
     longitude, radius_mean, visit_count, visit_days, last_visit_date, category, is_stale";

impl Place {
    pub fn from_row(row: &Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            id: row.get(0)?,
            name: row.get(1)?,
            street_address: row.get(2)?,
            locality: row.get(3)?,
            country_code: row.get(4)?,
            latitude: row.get(5)?,
            longitude: row.get(6)?,
            radius_mean: row.get(7)?,
            visit_count: row.get(8)?,
            visit_days: row.get(9)?,
            last_visit_date: row.get::<_, Option<i64>>(10)?.and_then(rfc3339),
            category: row.get(11)?,
            is_stale: row.get(12)?,
            distance_m: None,
        })
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Sample {
    pub date: Option<String>,
    pub latitude: Option<f64>,
    pub longitude: Option<f64>,
    pub altitude: Option<f64>,
    pub horizontal_accuracy: Option<f64>,
    pub speed: Option<f64>,
    pub course: Option<f64>,
    pub classified_activity_type: Option<String>,
    pub confirmed_activity_type: Option<String>,
    pub moving_state: Option<String>,
}

fn item_query(filter: &str) -> String {
    format!(
        "SELECT {ITEM_COLUMNS}, {} FROM items WHERE {LIVE_ITEM} AND {filter}",
        confirmation::COLUMNS
    )
}

pub fn item(conn: &Connection, id: &str) -> Result<Option<ItemRow>> {
    Ok(conn
        .query_row(&item_query("id = ?"), params![id], ItemRow::from_row)
        .optional()?)
}

/// The items covering a local day, in start order. Same selection as the derivation, so the
/// timeline and the day summary always agree on what the day contained.
pub fn items_on(conn: &Connection, date: &str) -> Result<Vec<ItemRow>> {
    let query = item_query(
        "local_start_date IS NOT NULL AND local_end_date IS NOT NULL
         AND local_start_date <= ?1 AND local_end_date >= ?1",
    );
    let items = conn
        .prepare(&format!("{query} ORDER BY start_date, id"))?
        .query_map(params![date], ItemRow::from_row)?
        .collect::<Result<_, _>>()?;
    Ok(items)
}

/// Visits at a place, newest first.
pub fn visits_at(
    conn: &Connection,
    place_id: &str,
    from: Option<&str>,
    to: Option<&str>,
    limit: i64,
) -> Result<Vec<ItemRow>> {
    let query = item_query(
        "visit_place_id = ?1
         AND (?2 IS NULL OR local_start_date >= ?2)
         AND (?3 IS NULL OR local_start_date <= ?3)",
    );
    let items = conn
        .prepare(&format!("{query} ORDER BY start_date DESC LIMIT ?4"))?
        .query_map(params![place_id, from, to, limit], ItemRow::from_row)?
        .collect::<Result<_, _>>()?;
    Ok(items)
}

/// The item covering an instant. A visit wins over a trip that shares the boundary, and a
/// later start wins over an earlier one, so the answer is the most specific item there is.
pub fn item_at(conn: &Connection, millis: i64) -> Result<Option<ItemRow>> {
    Ok(conn
        .query_row(
            &format!(
                "{} ORDER BY is_visit DESC, start_date DESC LIMIT 1",
                item_query("start_date <= ?1 AND end_date >= ?1")
            ),
            params![millis],
            ItemRow::from_row,
        )
        .optional()?)
}

pub fn place(conn: &Connection, id: &str) -> Result<Option<Place>> {
    Ok(conn
        .query_row(
            &format!("SELECT {PLACE_COLUMNS} FROM places WHERE id = ?"),
            params![id],
            Place::from_row,
        )
        .optional()?)
}

/// The places a batch of items visited, so a timeline needs one query rather than one per item.
pub fn places_of(conn: &Connection, items: &[ItemRow]) -> Result<HashMap<String, Place>> {
    let mut statement =
        conn.prepare(&format!("SELECT {PLACE_COLUMNS} FROM places WHERE id = ?"))?;
    let mut places = HashMap::new();
    for id in items
        .iter()
        .filter_map(|item| item.visit_place_id.as_deref())
    {
        if places.contains_key(id) {
            continue;
        }
        if let Some(place) = statement
            .query_row(params![id], Place::from_row)
            .optional()?
        {
            places.insert(id.to_string(), place);
        }
    }
    Ok(places)
}

/// Every usable fix on a local day, grouped by the item it belongs to and in time order.
///
/// "Usable" is the same rule the derivation measures distance by: a fix worse than
/// [`derive::MAX_ACCURACY_M`] is noise that would draw a line through a street the person
/// never walked down.
pub fn day_samples(conn: &Connection, date: &str) -> Result<HashMap<String, Vec<SampleRow>>> {
    let mut by_item: HashMap<String, Vec<SampleRow>> = HashMap::new();
    let mut statement = conn.prepare(
        "SELECT date, latitude, longitude, altitude, horizontal_accuracy, speed, course,
                classified_activity_type, confirmed_activity_type, moving_state,
                timeline_item_id
         FROM samples
         WHERE local_date = ? AND coalesce(disabled, 0) = 0
           AND latitude IS NOT NULL AND longitude IS NOT NULL
           AND coalesce(horizontal_accuracy, 0) <= ?
         ORDER BY timeline_item_id, date",
    )?;
    let rows = statement.query_map(params![date, derive::MAX_ACCURACY_M], |row| {
        Ok((row.get::<_, Option<String>>(10)?, SampleRow::from_row(row)?))
    })?;
    for row in rows {
        let (item_id, sample) = row?;
        let Some(item_id) = item_id else { continue };
        by_item.entry(item_id).or_default().push(sample);
    }
    Ok(by_item)
}

/// An item's samples in time order. Disabled samples are excluded everywhere.
pub fn samples_of(conn: &Connection, item_id: &str) -> Result<Vec<SampleRow>> {
    let samples = conn
        .prepare(
            "SELECT date, latitude, longitude, altitude, horizontal_accuracy, speed, course,
                    classified_activity_type, confirmed_activity_type, moving_state
             FROM samples
             WHERE timeline_item_id = ? AND coalesce(disabled, 0) = 0
             ORDER BY date",
        )?
        .query_map(params![item_id], SampleRow::from_row)?
        .collect::<Result<_, _>>()?;
    Ok(samples)
}

pub struct SampleRow {
    pub date: i64,
    pub latitude: Option<f64>,
    pub longitude: Option<f64>,
    pub altitude: Option<f64>,
    pub horizontal_accuracy: Option<f64>,
    speed: Option<f64>,
    course: Option<f64>,
    classified_activity_type: Option<i32>,
    confirmed_activity_type: Option<i32>,
    moving_state: Option<i32>,
}

impl SampleRow {
    pub fn from_row(row: &Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            date: row.get(0)?,
            latitude: row.get(1)?,
            longitude: row.get(2)?,
            altitude: row.get(3)?,
            horizontal_accuracy: row.get(4)?,
            speed: row.get(5)?,
            course: row.get(6)?,
            classified_activity_type: row.get(7)?,
            confirmed_activity_type: row.get(8)?,
            moving_state: row.get(9)?,
        })
    }

    pub fn coordinates(&self) -> Option<(f64, f64)> {
        match (self.latitude, self.longitude) {
            (Some(latitude), Some(longitude)) => Some((latitude, longitude)),
            _ => None,
        }
    }

    pub fn to_json(&self) -> Sample {
        let activity =
            |raw: Option<i32>| raw.map(|raw| ActivityType::from_raw(raw).as_str().into_owned());
        Sample {
            date: rfc3339(self.date),
            latitude: self.latitude,
            longitude: self.longitude,
            altitude: self.altitude,
            horizontal_accuracy: self.horizontal_accuracy,
            speed: self.speed,
            course: self.course,
            classified_activity_type: activity(self.classified_activity_type),
            confirmed_activity_type: activity(self.confirmed_activity_type),
            moving_state: self
                .moving_state
                .map(|raw| MovingState::from_raw(raw).as_str().into_owned()),
        }
    }
}
