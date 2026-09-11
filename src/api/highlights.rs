//! `/api/highlights`: the notable events of a date range.
//!
//! The rules live in [`crate::highlights`]; this is the SQL and the query string around them.
//! A first time is first over all of history, so the day summaries are read from the beginning
//! up to `to` rather than only inside the range. That is about a thousand rows a year and two
//! small JSON arrays per row, which is cheaper than keeping a table of firsts in step with a
//! backup that gets rewritten retroactively.

use std::collections::HashMap;

use anyhow::Result;
use axum::Json;
use axum::extract::{Query, State};
use rusqlite::{Connection, OptionalExtension, params};

use super::AppState;
use super::convert::parse_date;
use super::error::{ApiError, ApiResult};
use super::model::LIVE_ITEM;
use crate::arc::enums::ActivityType;
use crate::confirmation::{self, Confirmation};
use crate::highlights::{
    self, Direction, Endpoint, FlightRow, Highlight, Input, Kind, Neighbour, TripRow,
};

/// The same ceiling `/api/days` has: a year and a bit is a feed, more is a scrape.
const MAX_RANGE_DAYS: i64 = 400;

pub async fn get(
    State(state): State<AppState>,
    Query(params): Query<HashMap<String, String>>,
) -> ApiResult<Json<Vec<Highlight>>> {
    let request = Request::parse(&params)?;
    let highlights = state.db.read(move |conn| collect(conn, &request)).await?;
    Ok(Json(highlights))
}

struct Request {
    from: String,
    to: String,
    kinds: Vec<Kind>,
    confirmed_only: bool,
}

impl Request {
    fn parse(params: &HashMap<String, String>) -> ApiResult<Self> {
        let required = |name: &str| {
            params
                .get(name)
                .ok_or_else(|| ApiError::bad_request(format!("{name} is required")))
        };
        let from = parse_date(required("from")?)?;
        let to = parse_date(required("to")?)?;
        if from > to {
            return Err(ApiError::bad_request("from must not be after to"));
        }
        let days = from.until(to).map_or(0, |span| span.get_days() as i64) + 1;
        if days > MAX_RANGE_DAYS {
            return Err(ApiError::bad_request(format!(
                "range of {days} days is longer than the {MAX_RANGE_DAYS} day maximum"
            )));
        }

        let kinds = match params.get("kinds") {
            None => Kind::ALL.to_vec(),
            Some(raw) => {
                let kinds: Vec<Kind> = raw
                    .split(',')
                    .map(|name| {
                        Kind::parse(name.trim())
                            .ok_or_else(|| ApiError::bad_request(format!("{name:?} is not a kind")))
                    })
                    .collect::<ApiResult<_>>()?;
                kinds
            }
        };

        let confirmed_only = match params.get("confirmed").map(String::as_str) {
            None | Some("false") => false,
            Some("true") => true,
            Some(other) => {
                return Err(ApiError::bad_request(format!(
                    "confirmed {other:?} is not true or false"
                )));
            }
        };

        Ok(Self {
            from: from.to_string(),
            to: to.to_string(),
            kinds,
            confirmed_only,
        })
    }

    fn wants(&self, kind: Kind) -> bool {
        self.kinds.contains(&kind)
    }
}

/// Only the queries the requested kinds actually need are run.
fn collect(conn: &Connection, request: &Request) -> Result<Vec<Highlight>> {
    let first_times = request.wants(Kind::Country) || request.wants(Kind::Locality);
    let days = if first_times {
        day_rows(conn, &request.to)?
    } else {
        Vec::new()
    };
    let flights = if request.wants(Kind::Flight) {
        flight_rows(conn, &request.from, &request.to)?
    } else {
        Vec::new()
    };
    let trips = if request.wants(Kind::Longest) {
        longest_candidates(conn, &request.from, &request.to)?
    } else {
        Vec::new()
    };

    Ok(highlights::compute(&Input {
        from: &request.from,
        to: &request.to,
        days: &days,
        flights: &flights,
        trips: &trips,
        kinds: &request.kinds,
        confirmed_only: request.confirmed_only,
    }))
}

/// Every summarized day up to `to`, in date order.
fn day_rows(conn: &Connection, to: &str) -> Result<Vec<highlights::DayRow>> {
    let rows = conn
        .prepare(
            "SELECT date, country_codes, localities, confirmed FROM day_summaries
             WHERE date <= ? ORDER BY date",
        )?
        .query_map(params![to], |row| {
            // The derivation writes these as JSON arrays; anything unreadable is an empty day
            // rather than a failed request.
            let list = |index: usize| -> rusqlite::Result<Vec<String>> {
                Ok(serde_json::from_str(&row.get::<_, String>(index)?).unwrap_or_default())
            };
            Ok(highlights::DayRow {
                date: row.get(0)?,
                country_codes: list(1)?,
                localities: list(2)?,
                confirmed: row.get(3)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

const TRIP_COLUMNS: &str = "id, local_start_date, activity_type, trip_distance,
     start_date, end_date";

fn trip_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<TripRow> {
    let (start, end) = (row.get::<_, i64>(4)?, row.get::<_, i64>(5)?);
    Ok(TripRow {
        item_id: row.get(0)?,
        date: row.get(1)?,
        activity_type: ActivityType::from_raw(row.get(2)?),
        distance_m: row.get(3)?,
        duration_seconds: (end - start).max(0) / 1000,
        confirmed: Confirmation::from_row(row, 6)?.confirmed,
    })
}

/// Flights whose local start date is in the range, with the places either side resolved.
fn flight_rows(conn: &Connection, from: &str, to: &str) -> Result<Vec<FlightRow>> {
    let trips: Vec<(TripRow, Option<String>, Option<String>)> = conn
        .prepare(&format!(
            "SELECT {TRIP_COLUMNS}, {}, previous_item_id, next_item_id FROM items
             WHERE {LIVE_ITEM} AND activity_type = ?1
               AND local_start_date >= ?2 AND local_start_date <= ?3
             ORDER BY start_date, id",
            confirmation::COLUMNS
        ))?
        .query_map(params![ActivityType::Airplane.raw(), from, to], |row| {
            Ok((trip_row(row)?, row.get(11)?, row.get(12)?))
        })?
        .collect::<Result<_, _>>()?;

    let mut statement = conn.prepare(&format!(
        "SELECT i.is_visit, i.previous_item_id, i.next_item_id,
                coalesce(p.locality, i.visit_locality),
                coalesce(p.country_code, i.visit_country_code),
                coalesce(p.name, i.visit_custom_title)
         FROM items i LEFT JOIN places p ON p.id = i.visit_place_id
         WHERE i.id = ? AND {LIVE_ITEM}"
    ))?;
    let mut neighbour = |id: &str| -> Result<Option<Neighbour>> {
        Ok(statement
            .query_row(params![id], |row| {
                Ok(Neighbour {
                    is_visit: row.get(0)?,
                    previous_item_id: row.get(1)?,
                    next_item_id: row.get(2)?,
                    endpoint: Endpoint {
                        locality: row.get(3)?,
                        country_code: row.get(4)?,
                        place_name: row.get(5)?,
                    },
                })
            })
            .optional()?)
    };

    trips
        .into_iter()
        .map(|(trip, previous, next)| {
            Ok(FlightRow {
                from: highlights::nearest_visit(
                    previous.as_deref(),
                    Direction::Previous,
                    &mut neighbour,
                )?,
                to: highlights::nearest_visit(next.as_deref(), Direction::Next, &mut neighbour)?,
                trip,
            })
        })
        .collect()
}

/// Every trip in the range that could be the longest of its kind. Which one wins is the pure
/// module's decision; SQL only narrows it to the four types and the one kilometre floor.
fn longest_candidates(conn: &Connection, from: &str, to: &str) -> Result<Vec<TripRow>> {
    let types: Vec<i32> = [
        ActivityType::Walking,
        ActivityType::Running,
        ActivityType::Cycling,
        ActivityType::Hiking,
    ]
    .map(ActivityType::raw)
    .to_vec();
    let list = types
        .iter()
        .map(i32::to_string)
        .collect::<Vec<_>>()
        .join(", ");
    let trips = conn
        .prepare(&format!(
            "SELECT {TRIP_COLUMNS}, {} FROM items
             WHERE {LIVE_ITEM} AND activity_type IN ({list})
               AND local_start_date >= ?1 AND local_start_date <= ?2
               AND trip_distance >= ?3
             ORDER BY start_date, id",
            confirmation::COLUMNS
        ))?
        .query_map(params![from, to, highlights::MIN_LONGEST_M], trip_row)?
        .collect::<Result<_, _>>()?;
    Ok(trips)
}
