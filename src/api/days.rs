//! Day summaries and the day timeline.

use std::collections::HashMap;

use anyhow::Result;
use axum::Json;
use axum::extract::{Path, Query, State};
use axum::response::{IntoResponse, Response};
use jiff::ToSpan;
use jiff::civil::Date;
use rusqlite::{Connection, OptionalExtension, params};
use serde::Serialize;
use serde_json::Value;

use super::convert::{parse_date, rfc3339, today};
use super::error::{ApiError, ApiResult};
use super::model::{self, Item};
use super::{AppState, geojson, gpx};

/// A month of context is what a calendar view needs; anything much longer is a mistake or a
/// scrape, and a summary row per day adds up.
const DEFAULT_RANGE_DAYS: i64 = 30;
const MAX_RANGE_DAYS: i64 = 400;

const SUMMARY_COLUMNS: &str = "date, utc_offset_seconds, item_count, visit_count, trip_count,
     sample_count, distance_m, moving_seconds, distance_by_type, duration_by_type, place_ids,
     country_codes, localities, min_lat, min_lon, max_lat, max_lon, first_sample_at,
     last_sample_at, unconfirmed_items, confirmed";

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DaySummary {
    pub date: String,
    pub utc_offset_seconds: Option<i32>,
    pub item_count: i64,
    pub visit_count: i64,
    pub trip_count: i64,
    pub sample_count: i64,
    pub distance_m: f64,
    pub moving_seconds: i64,
    pub distance_by_type: Value,
    pub duration_by_type: Value,
    pub place_ids: Value,
    pub country_codes: Value,
    pub localities: Value,
    /// `[minLon, minLat, maxLon, maxLat]`, the GeoJSON order, or null for a day with no fix.
    pub bbox: Option<[f64; 4]>,
    pub first_sample_at: Option<String>,
    pub last_sample_at: Option<String>,
    pub unconfirmed_items: i64,
    /// Whether every item on the day has been reviewed in Arc: see the README.
    pub confirmed: bool,
}

impl DaySummary {
    fn from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Self> {
        // The derivation stores these as JSON text; a consumer should not have to parse twice.
        let json = |index: usize| -> rusqlite::Result<Value> {
            Ok(serde_json::from_str(&row.get::<_, String>(index)?).unwrap_or(Value::Null))
        };
        let bbox = match (
            row.get::<_, Option<f64>>(13)?,
            row.get::<_, Option<f64>>(14)?,
            row.get::<_, Option<f64>>(15)?,
            row.get::<_, Option<f64>>(16)?,
        ) {
            (Some(min_lat), Some(min_lon), Some(max_lat), Some(max_lon)) => {
                Some([min_lon, min_lat, max_lon, max_lat])
            }
            _ => None,
        };
        Ok(Self {
            date: row.get(0)?,
            utc_offset_seconds: row.get(1)?,
            item_count: row.get(2)?,
            visit_count: row.get(3)?,
            trip_count: row.get(4)?,
            sample_count: row.get(5)?,
            distance_m: row.get(6)?,
            moving_seconds: row.get(7)?,
            distance_by_type: json(8)?,
            duration_by_type: json(9)?,
            place_ids: json(10)?,
            country_codes: json(11)?,
            localities: json(12)?,
            bbox,
            first_sample_at: row.get::<_, Option<i64>>(17)?.and_then(rfc3339),
            last_sample_at: row.get::<_, Option<i64>>(18)?.and_then(rfc3339),
            unconfirmed_items: row.get(19)?,
            confirmed: row.get(20)?,
        })
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Day {
    summary: DaySummary,
    items: Vec<Item>,
}

pub async fn list(
    State(state): State<AppState>,
    Query(params): Query<HashMap<String, String>>,
) -> ApiResult<Json<Vec<DaySummary>>> {
    let to = match params.get("to") {
        Some(raw) => parse_date(raw)?,
        None => today(),
    };
    let from = match params.get("from") {
        Some(raw) => parse_date(raw)?,
        None => to.checked_sub(DEFAULT_RANGE_DAYS.days()).unwrap_or(to),
    };
    if from > to {
        return Err(ApiError::bad_request("from must not be after to"));
    }
    let days = from.until(to).map_or(0, |span| span.get_days() as i64) + 1;
    if days > MAX_RANGE_DAYS {
        return Err(ApiError::bad_request(format!(
            "range of {days} days is longer than the {MAX_RANGE_DAYS} day maximum"
        )));
    }

    let summaries = state
        .db
        .read(move |conn| summaries_in(conn, &from.to_string(), &to.to_string()))
        .await?;
    Ok(Json(summaries))
}

pub async fn get(State(state): State<AppState>, Path(date): Path<String>) -> ApiResult<Response> {
    // axum 0.8 captures whole path segments only, so the GPX rendering of a day arrives here
    // rather than on a route of its own.
    if let Some(date) = date.strip_suffix(".gpx") {
        return gpx::day(&state, parse_date(date)?).await;
    }
    let date = parse_date(&date)?;

    let day = state
        .db
        .read(move |conn| {
            let key = date.to_string();
            let Some(summary) = summary(conn, &key)? else {
                return Ok(None);
            };
            let items = model::items_on(conn, &key)?;
            let places = model::places_of(conn, &items)?;
            let items = items
                .iter()
                .map(|item| {
                    let place = item
                        .visit_place_id
                        .as_deref()
                        .and_then(|id| places.get(id).cloned());
                    item.to_json(place, Some(date))
                })
                .collect();
            Ok(Some(Day { summary, items }))
        })
        .await?;

    match day {
        Some(day) => Ok(Json(day).into_response()),
        None => Err(ApiError::not_found(format!("no summary for {date}"))),
    }
}

pub async fn geojson(
    State(state): State<AppState>,
    Path(date): Path<String>,
    Query(params): Query<HashMap<String, String>>,
) -> ApiResult<Json<Value>> {
    geojson::day(
        &state,
        parse_date(&date)?,
        super::simplify_tolerance(&params)?,
    )
    .await
}

fn summary(conn: &Connection, date: &str) -> Result<Option<DaySummary>> {
    Ok(conn
        .query_row(
            &format!("SELECT {SUMMARY_COLUMNS} FROM day_summaries WHERE date = ?"),
            params![date],
            DaySummary::from_row,
        )
        .optional()?)
}

/// Only days that have a row: a gap in the recording is simply absent, never a zeroed day.
fn summaries_in(conn: &Connection, from: &str, to: &str) -> Result<Vec<DaySummary>> {
    let summaries = conn
        .prepare(&format!(
            "SELECT {SUMMARY_COLUMNS} FROM day_summaries
             WHERE date >= ? AND date <= ? ORDER BY date"
        ))?
        .query_map(params![from, to], DaySummary::from_row)?
        .collect::<Result<_, _>>()?;
    Ok(summaries)
}

/// The date a day summary exists for, for the GPX and GeoJSON renderings to 404 on.
pub fn summary_exists(conn: &Connection, date: &Date) -> Result<bool> {
    Ok(conn.query_row(
        "SELECT count(*) FROM day_summaries WHERE date = ?",
        params![date.to_string()],
        |row| row.get::<_, i64>(0),
    )? > 0)
}
