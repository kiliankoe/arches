//! Single items and their samples.

use std::collections::HashMap;

use axum::Json;
use axum::extract::{Path, Query, State};

use super::error::{ApiError, ApiResult};
use super::model::{self, Item, Sample};
use super::{AppState, simplify_tolerance};
use crate::geo::simplify_indices;

pub async fn get(State(state): State<AppState>, Path(id): Path<String>) -> ApiResult<Json<Item>> {
    let item = state
        .db
        .read(move |conn| {
            let Some(item) = model::item(conn, &id)? else {
                return Ok(None);
            };
            let place = match &item.visit_place_id {
                Some(place_id) => model::place(conn, place_id)?,
                None => None,
            };
            Ok(Some(item.to_json(place, None)))
        })
        .await?;

    item.map(Json)
        .ok_or_else(|| ApiError::not_found("no such item"))
}

pub async fn samples(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(params): Query<HashMap<String, String>>,
) -> ApiResult<Json<Vec<Sample>>> {
    let simplify_m = simplify_tolerance(&params)?;
    let samples = state
        .db
        .read(move |conn| {
            if model::item(conn, &id)?.is_none() {
                return Ok(None);
            }
            let rows = model::samples_of(conn, &id)?;
            // Simplification runs over the coordinate chain, and a sample without a fix is not
            // part of it; it is kept either way, since it still says what was going on.
            let Some(tolerance) = simplify_m else {
                return Ok(Some(rows.iter().map(|row| row.to_json()).collect()));
            };
            let positioned: Vec<usize> = (0..rows.len())
                .filter(|&index| rows[index].coordinates().is_some())
                .collect();
            let points: Vec<(f64, f64)> = positioned
                .iter()
                .filter_map(|&index| rows[index].coordinates())
                .collect();
            let kept: std::collections::HashSet<usize> = simplify_indices(&points, tolerance)
                .into_iter()
                .map(|index| positioned[index])
                .collect();
            let samples: Vec<Sample> = rows
                .iter()
                .enumerate()
                .filter(|(index, row)| row.coordinates().is_none() || kept.contains(index))
                .map(|(_, row)| row.to_json())
                .collect();
            Ok(Some(samples))
        })
        .await?;

    samples
        .map(Json)
        .ok_or_else(|| ApiError::not_found("no such item"))
}
