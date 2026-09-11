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

/// Two months of geometry is already more than a map shows at once; a wider window is a scrape.
const MAX_RANGE_DAYS: i64 = 62;

pub async fn day(state: &AppState, date: Date, simplify_m: Option<f64>) -> ApiResult<Json<Value>> {
    let features = state
        .db
        .read(move |conn| {
            if !days::summary_exists(conn, &date)? {
                return Ok(None);
            }
            Ok(Some(render(conn, &date.to_string(), simplify_m)?))
        })
        .await?;

    match features {
        Some(features) => Ok(Json(collection(features))),
        None => Err(ApiError::not_found(format!("no summary for {date}"))),
    }
}

/// Every day of an inclusive range in one collection, for a view that frames a week or a month
/// and wants the tracks behind it. A day with no summary contributes nothing, so a gap in the
/// recording is simply absent rather than an error.
pub async fn range(
    state: &AppState,
    from: Date,
    to: Date,
    simplify_m: Option<f64>,
) -> ApiResult<Json<Value>> {
    if from > to {
        return Err(ApiError::bad_request("from must not be after to"));
    }
    let span = from.until(to).map_or(0, |span| span.get_days() as i64) + 1;
    if span > MAX_RANGE_DAYS {
        return Err(ApiError::bad_request(format!(
            "range of {span} days is longer than the {MAX_RANGE_DAYS} day maximum"
        )));
    }

    let features = state
        .db
        .read(move |conn| {
            let dates = days::summary_dates_in(conn, &from.to_string(), &to.to_string())?;
            let mut features = Vec::new();
            for date in &dates {
                features.extend(render(conn, date, simplify_m)?);
            }
            Ok(features)
        })
        .await?;
    Ok(Json(collection(features)))
}

fn collection(features: Vec<Value>) -> Value {
    json!({ "type": "FeatureCollection", "features": features })
}

/// Every feature carries its `date`, which is what lets a client that asked for a range tell one
/// day's geometry from the next.
fn render(conn: &Connection, date: &str, simplify_m: Option<f64>) -> Result<Vec<Value>> {
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
                    "date": date,
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
                "date": date,
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

    Ok(features)
}
