//! Places: search, one place, its visits, and what is near a coordinate.

use std::collections::HashMap;

use anyhow::Result;
use axum::Json;
use axum::extract::{Path, Query, State};
use rusqlite::{Connection, params};

use super::AppState;
use super::convert::{parse_date, parse_f64, parse_limit};
use super::error::{ApiError, ApiResult};
use super::model::{self, Item, PLACE_COLUMNS, Place};
use crate::geo::haversine_m;

const DEFAULT_RADIUS_M: f64 = 250.0;
const MAX_RADIUS_M: f64 = 5_000.0;
/// A degree of latitude, near enough for a bounding box that only has to be too generous.
const METRES_PER_DEGREE: f64 = 111_320.0;

pub async fn list(
    State(state): State<AppState>,
    Query(params): Query<HashMap<String, String>>,
) -> ApiResult<Json<Vec<Place>>> {
    let query = params.get("q").cloned();
    let country = params.get("country").cloned();
    let limit = parse_limit(params.get("limit").map(String::as_str), 50, 500)?;

    let places = state
        .db
        .read(move |conn| search(conn, query.as_deref(), country.as_deref(), limit))
        .await?;
    Ok(Json(places))
}

pub async fn get(State(state): State<AppState>, Path(id): Path<String>) -> ApiResult<Json<Place>> {
    let place = state.db.read(move |conn| model::place(conn, &id)).await?;
    place
        .map(Json)
        .ok_or_else(|| ApiError::not_found("no such place"))
}

pub async fn visits(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(params): Query<HashMap<String, String>>,
) -> ApiResult<Json<Vec<Item>>> {
    let from = params.get("from").map(|raw| parse_date(raw)).transpose()?;
    let to = params.get("to").map(|raw| parse_date(raw)).transpose()?;
    let limit = parse_limit(params.get("limit").map(String::as_str), 100, 500)?;

    let visits = state
        .db
        .read(move |conn| {
            let Some(place) = model::place(conn, &id)? else {
                return Ok(None);
            };
            let items = model::visits_at(
                conn,
                &id,
                from.map(|date| date.to_string()).as_deref(),
                to.map(|date| date.to_string()).as_deref(),
                limit,
            )?;
            Ok(Some(
                items
                    .iter()
                    .map(|item| item.to_json(Some(place.clone()), None))
                    .collect::<Vec<_>>(),
            ))
        })
        .await?;

    visits
        .map(Json)
        .ok_or_else(|| ApiError::not_found("no such place"))
}

pub async fn near(
    State(state): State<AppState>,
    Query(params): Query<HashMap<String, String>>,
) -> ApiResult<Json<Vec<Place>>> {
    let latitude = parse_f64(
        "lat",
        params
            .get("lat")
            .ok_or_else(|| ApiError::bad_request("lat is required"))?,
    )?;
    let longitude = parse_f64(
        "lon",
        params
            .get("lon")
            .ok_or_else(|| ApiError::bad_request("lon is required"))?,
    )?;
    if !(-90.0..=90.0).contains(&latitude) || !(-180.0..=180.0).contains(&longitude) {
        return Err(ApiError::bad_request("lat or lon is out of range"));
    }
    let radius_m = match params.get("radius") {
        Some(raw) => parse_f64("radius", raw)?,
        None => DEFAULT_RADIUS_M,
    };
    if radius_m <= 0.0 || radius_m > MAX_RADIUS_M {
        return Err(ApiError::bad_request(format!(
            "radius must be between 0 and {MAX_RADIUS_M} metres"
        )));
    }

    let places = state
        .db
        .read(move |conn| within(conn, latitude, longitude, radius_m))
        .await?;
    Ok(Json(places))
}

/// `q` matches the three fields a person would recognise a place by.
fn search(
    conn: &Connection,
    query: Option<&str>,
    country: Option<&str>,
    limit: i64,
) -> Result<Vec<Place>> {
    let pattern = query.map(|query| format!("%{}%", query.replace('%', "\\%")));
    let places = conn
        .prepare(&format!(
            "SELECT {PLACE_COLUMNS} FROM places
             WHERE (?1 IS NULL OR name LIKE ?1 ESCAPE '\\' COLLATE NOCASE
                    OR locality LIKE ?1 ESCAPE '\\' COLLATE NOCASE
                    OR street_address LIKE ?1 ESCAPE '\\' COLLATE NOCASE)
               AND (?2 IS NULL OR country_code = ?2 COLLATE NOCASE)
             ORDER BY coalesce(visit_count, 0) DESC, name
             LIMIT ?3"
        ))?
        .query_map(params![pattern, country, limit], Place::from_row)?
        .collect::<Result<_, _>>()?;
    Ok(places)
}

/// Nearest first. The bounding box is a cheap prefilter that the index can use; haversine then
/// decides, so the corners of the box do not sneak in.
fn within(conn: &Connection, latitude: f64, longitude: f64, radius_m: f64) -> Result<Vec<Place>> {
    let latitude_span = radius_m / METRES_PER_DEGREE;
    // Longitude degrees shrink towards the poles; the clamp keeps the box sane near them.
    let longitude_span =
        radius_m / (METRES_PER_DEGREE * latitude.to_radians().cos().abs().max(0.01));

    let mut places: Vec<Place> = conn
        .prepare(&format!(
            "SELECT {PLACE_COLUMNS} FROM places
             WHERE latitude BETWEEN ?1 AND ?2 AND longitude BETWEEN ?3 AND ?4"
        ))?
        .query_map(
            params![
                latitude - latitude_span,
                latitude + latitude_span,
                longitude - longitude_span,
                longitude + longitude_span,
            ],
            Place::from_row,
        )?
        .collect::<Result<_, _>>()?;

    places.retain_mut(|place| {
        let distance = haversine_m(latitude, longitude, place.latitude, place.longitude);
        place.distance_m = Some(distance);
        distance <= radius_m
    });
    places.sort_by(|a, b| {
        a.distance_m
            .unwrap_or_default()
            .total_cmp(&b.distance_m.unwrap_or_default())
    });
    Ok(places)
}
