//! A day as a GeoJSON FeatureCollection: a LineString per trip, a Point per visit.

use anyhow::Result;
use axum::Json;
use jiff::civil::Date;
use rusqlite::Connection;
use serde_json::{Value, json};

use super::convert::rfc3339;
use super::error::{ApiError, ApiResult};
use super::{AppState, days, model};
use crate::geo::simplify_indices;

pub async fn day(state: &AppState, date: Date, simplify_m: Option<f64>) -> ApiResult<Json<Value>> {
    let collection = state
        .db
        .read(move |conn| {
            if !days::summary_exists(conn, &date)? {
                return Ok(None);
            }
            Ok(Some(render(conn, &date.to_string(), simplify_m)?))
        })
        .await?;

    match collection {
        Some(collection) => Ok(Json(collection)),
        None => Err(ApiError::not_found(format!("no summary for {date}"))),
    }
}

fn render(conn: &Connection, date: &str, simplify_m: Option<f64>) -> Result<Value> {
    let items = model::items_on(conn, date)?;
    let places = model::places_of(conn, &items)?;
    let mut samples = model::day_samples(conn, date)?;

    let mut features = Vec::new();
    for item in &items {
        let confirmation = item.confirmation();
        let place = item.visit_place_id.as_deref().and_then(|id| places.get(id));

        if item.is_visit {
            let Some((latitude, longitude)) = item.visit_coordinates(place) else {
                continue;
            };
            features.push(json!({
                "type": "Feature",
                "geometry": { "type": "Point", "coordinates": [longitude, latitude] },
                "properties": {
                    "itemId": item.id,
                    "placeId": item.visit_place_id,
                    "name": place.map(|place| place.name.clone()),
                    "startDate": rfc3339(item.start_date),
                    "endDate": rfc3339(item.end_date),
                    "confirmed": confirmation.confirmed,
                },
            }));
            continue;
        }

        let Some(samples) = samples.remove(&item.id) else {
            continue;
        };
        let points: Vec<(f64, f64)> = samples.iter().filter_map(|s| s.coordinates()).collect();
        // A single fix is not a line; GeoJSON wants at least two positions.
        if points.len() < 2 {
            continue;
        }
        let coordinates: Vec<Value> = match simplify_m {
            Some(tolerance) => simplify_indices(&points, tolerance)
                .into_iter()
                .map(|index| json!([points[index].1, points[index].0]))
                .collect(),
            None => points
                .iter()
                .map(|(latitude, longitude)| json!([longitude, latitude]))
                .collect(),
        };
        features.push(json!({
            "type": "Feature",
            "geometry": { "type": "LineString", "coordinates": coordinates },
            "properties": {
                "itemId": item.id,
                "activityType": item.activity_type(),
                "startDate": rfc3339(item.start_date),
                "endDate": rfc3339(item.end_date),
                "distanceM": item.distance_m,
                "confirmed": confirmation.confirmed,
                "uncertain": confirmation.uncertain,
            },
        }));
    }

    Ok(json!({ "type": "FeatureCollection", "features": features }))
}
