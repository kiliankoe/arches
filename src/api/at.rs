//! What was happening at an instant. Backs pensieve's "where was I when I wrote this".

use std::collections::HashMap;

use axum::Json;
use axum::extract::{Query, State};
use serde_json::{Value, json};

use super::AppState;
use super::convert::parse_timestamp;
use super::error::{ApiError, ApiResult};
use super::model;

pub async fn at(
    State(state): State<AppState>,
    Query(params): Query<HashMap<String, String>>,
) -> ApiResult<Json<Value>> {
    let raw = params
        .get("ts")
        .ok_or_else(|| ApiError::bad_request("ts is required"))?;
    let timestamp = parse_timestamp(raw)?;

    let item = state
        .db
        .read(move |conn| {
            let Some(item) = model::item_at(conn, timestamp.as_millisecond())? else {
                return Ok(None);
            };
            let place = match &item.visit_place_id {
                Some(place_id) => model::place(conn, place_id)?,
                None => None,
            };
            Ok(Some(item.to_json(place, None)))
        })
        .await?;

    // Not a 404: a gap in the recording is a perfectly good answer to "where was I".
    Ok(Json(json!({ "item": item })))
}
