//! `/api/heatmap`: everywhere the user has been in a date range, binned for the current view.
//!
//! `heatmap_cells` holds one row per local day per 6 m cell. A request names a bounding box and
//! a map zoom; the cells are coarsened to that zoom by right-shifting their indices, which is
//! why there is one fine table rather than a pyramid.

use std::collections::{HashMap, HashSet};

use anyhow::Result;
use axum::Json;
use axum::extract::{Query, State};
use rusqlite::{Connection, params};
use serde_json::{Value, json};

use super::AppState;
use super::convert::{parse_date, parse_f64};
use super::error::{ApiError, ApiResult};
use crate::geo::{
    CELL_ZOOM, MAX_MERCATOR_LAT, ROLLUP_SHIFTS, cell_centre, cell_metres, cell_of, cell_position,
};

/// More points than this and MapLibre spends longer uploading the source than drawing it. Going
/// over means coarsening, never truncating: half a heatmap is a lie about where someone was.
/// A full viewport at street zoom is around 30 000 once the trips are rasterized into cells.
const MAX_POINTS: usize = 40_000;

/// How many levels coarser than a screen pixel a cell is drawn at, so a point covers about
/// four pixels. MapLibre's heatmap layer blurs each point over a few cells anyway, and a view
/// at one point per pixel is a hundred thousand points for no visible gain.
const PIXEL_SHIFT: i64 = 2;

/// A tile is 256 px, so the pixel grid at map zoom `z` is the tile grid at `z + 8`.
const TILE_PIXELS_SHIFT: i64 = 8;

/// Four cells across the world is as coarse as coarsening ever needs to get.
const MAX_SHIFT: u32 = CELL_ZOOM - 4;

#[derive(Clone, Copy)]
enum Weight {
    /// Distinct days the cell was visited on. The default: a night at home is thousands of
    /// samples in one cell and would drown out every route the person actually took.
    Days,
    Samples,
}

struct Request {
    /// Inclusive fine-cell bounds, already clamped onto the projection.
    min_x: i64,
    min_y: i64,
    max_x: i64,
    max_y: i64,
    centre_lat: f64,
    from: String,
    to: String,
    weight: Weight,
    shift: u32,
}

pub async fn get(
    State(state): State<AppState>,
    Query(params): Query<HashMap<String, String>>,
) -> ApiResult<Json<Value>> {
    let request = Request::parse(&params)?;
    let collection = state.db.read(move |conn| render(conn, &request)).await?;
    Ok(Json(collection))
}

impl Request {
    fn parse(params: &HashMap<String, String>) -> ApiResult<Self> {
        let raw = params
            .get("bbox")
            .ok_or_else(|| ApiError::bad_request("bbox is required"))?;
        let numbers: Vec<f64> = raw
            .split(',')
            .map(|part| parse_f64("bbox", part.trim()))
            .collect::<ApiResult<_>>()?;
        let [min_lon, min_lat, max_lon, max_lat] = numbers[..] else {
            return Err(ApiError::bad_request(
                "bbox must be minLon,minLat,maxLon,maxLat",
            ));
        };
        if min_lon > max_lon || min_lat > max_lat {
            return Err(ApiError::bad_request("bbox is inside out"));
        }
        // A world-wrapped viewport reports longitudes past the antimeridian and latitudes past
        // the mercator cut; both simply mean "to the edge".
        let (min_lon, max_lon) = (min_lon.clamp(-180.0, 180.0), max_lon.clamp(-180.0, 180.0));
        let min_lat = min_lat.clamp(-MAX_MERCATOR_LAT, MAX_MERCATOR_LAT);
        let max_lat = max_lat.clamp(-MAX_MERCATOR_LAT, MAX_MERCATOR_LAT);

        let raw = params
            .get("zoom")
            .ok_or_else(|| ApiError::bad_request("zoom is required"))?;
        let zoom = parse_f64("zoom", raw)?;
        if !(0.0..=24.0).contains(&zoom) {
            return Err(ApiError::bad_request("zoom must be between 0 and 24"));
        }

        let weight = match params.get("weight").map(String::as_str) {
            None | Some("days") => Weight::Days,
            Some("samples") => Weight::Samples,
            Some(other) => {
                return Err(ApiError::bad_request(format!(
                    "weight {other:?} is not days or samples"
                )));
            }
        };

        let from = match params.get("from") {
            Some(raw) => parse_date(raw)?.to_string(),
            // Sorts before every date arches can store, so the range is simply open.
            None => String::new(),
        };
        let to = match params.get("to") {
            Some(raw) => parse_date(raw)?.to_string(),
            None => "9999-12-31".to_string(),
        };
        if !from.is_empty() && from > to {
            return Err(ApiError::bad_request("from must not be after to"));
        }

        // Mercator's y grows southwards, so the top-left corner is (minLon, maxLat).
        let (min_x, min_y) = cell_of(max_lat, min_lon);
        let (max_x, max_y) = cell_of(min_lat, max_lon);
        Ok(Self {
            min_x,
            min_y,
            max_x,
            max_y,
            centre_lat: (min_lat + max_lat) / 2.0,
            from,
            to,
            weight,
            shift: shift_for(zoom),
        })
    }
}

/// The coarsening that puts a cell at roughly `2^PIXEL_SHIFT` screen pixels.
fn shift_for(zoom: f64) -> u32 {
    let shift = CELL_ZOOM as i64 - zoom.round() as i64 - TILE_PIXELS_SHIFT + PIXEL_SHIFT;
    shift.clamp(0, MAX_SHIFT as i64) as u32
}

fn render(conn: &Connection, request: &Request) -> Result<Value> {
    render_capped(conn, request, MAX_POINTS)
}

fn render_capped(conn: &Connection, request: &Request, cap: usize) -> Result<Value> {
    let started = std::time::Instant::now();
    let mut shift = request.shift;
    let mut aggregate = aggregate(conn, request, shift)?;
    while aggregate.cells.len() > cap && shift < MAX_SHIFT {
        shift += 1;
        tracing::debug!(
            points = aggregate.cells.len(),
            shift,
            "heatmap exceeded the point cap, coarsening"
        );
        aggregate = self::aggregate(conn, request, shift)?;
    }

    let mut max_weight = 0;
    let mut extent: Option<(i64, i64, i64, i64)> = None;
    let features: Vec<Value> = aggregate
        .cells
        .iter()
        .map(|&(x, y, weight)| {
            max_weight = max_weight.max(weight);
            extent = Some(match extent {
                None => (x, y, x, y),
                Some((min_x, min_y, max_x, max_y)) => {
                    (min_x.min(x), min_y.min(y), max_x.max(x), max_y.max(y))
                }
            });
            let (latitude, longitude) = cell_centre(x, y, shift);
            json!({
                "type": "Feature",
                "geometry": { "type": "Point", "coordinates": [longitude, latitude] },
                "properties": { "weight": weight },
            })
        })
        .collect();

    tracing::debug!(
        points = aggregate.cells.len(),
        shift,
        elapsed_ms = started.elapsed().as_millis() as u64,
        "heatmap rendered"
    );
    Ok(json!({
        "type": "FeatureCollection",
        "features": features,
        // A foreign member: the spec allows it, and the client needs the scale to render with.
        "meta": {
            "cellMetres": cell_metres(shift, request.centre_lat),
            "maxWeight": max_weight,
            "points": aggregate.cells.len(),
            "days": aggregate.days,
            "bbox": extent.map(|extent| bbox_of(extent, shift)),
        },
    }))
}

/// The grid a shift is answered from: the coarsest rollup at or below it, or 0 for the fine
/// table, which is finer than every rollup level.
fn base_shift(shift: u32) -> u32 {
    ROLLUP_SHIFTS
        .iter()
        .copied()
        .filter(|&level| level <= shift)
        .max()
        .unwrap_or(0)
}

/// The table a base shift lives in, and the extra predicate a rollup needs. The level goes in
/// as a literal because it comes from [`ROLLUP_SHIFTS`], never from the request.
fn source(base: u32) -> (&'static str, String) {
    match base {
        0 => ("heatmap_cells", String::new()),
        level => ("heatmap_rollup", format!("shift = {level} AND ")),
    }
}

/// The coarse cells in a request's window with their weights, and how many distinct days put
/// anything into the window at all.
struct Aggregate {
    cells: Vec<(i64, i64, i64)>,
    days: i64,
}

/// One pass over the rows, aggregated here rather than in SQL. `GROUP BY` with a
/// `count(DISTINCT date)` costs SQLite a temp b-tree per group, and a street-level viewport
/// with the trips rasterized is a few hundred thousand rows; a hash map over the same rows is
/// several times faster, and the distinct-day total falls out of the same pass instead of a
/// second scan.
fn aggregate(conn: &Connection, request: &Request, shift: u32) -> Result<Aggregate> {
    let base = base_shift(shift);
    let (table, level) = source(base);
    // `+date`: the unary plus stops SQLite from seeing a range on the primary key's leading
    // column and walking the whole table by date instead of seeking the spatial index. With an
    // open range that choice was eight times slower.
    let mut statement = conn.prepare_cached(&format!(
        "SELECT date, x >> ?, y >> ?, samples
         FROM {table}
         WHERE {level}x >= ? AND x <= ? AND y >= ? AND y <= ? AND +date >= ? AND +date <= ?"
    ))?;
    let mut rows = statement.query(params![
        shift - base,
        shift - base,
        request.min_x >> base,
        request.max_x >> base,
        request.min_y >> base,
        request.max_y >> base,
        request.from,
        request.to
    ])?;

    // Dates are interned to small ids so a cell's set of days is a set of integers, not strings.
    let mut dates: HashMap<String, u32> = HashMap::new();
    let mut cells: HashMap<(i64, i64), Cell> = HashMap::new();
    while let Some(row) = rows.next()? {
        let date = row.get_ref(0)?.as_str()?;
        let next_id = dates.len() as u32;
        let date_id = match dates.get(date) {
            Some(&id) => id,
            None => {
                dates.insert(date.to_string(), next_id);
                next_id
            }
        };
        let key: (i64, i64) = (row.get(1)?, row.get(2)?);
        let samples: i64 = row.get(3)?;
        let cell = cells.entry(key).or_default();
        match request.weight {
            Weight::Days => {
                cell.days.insert(date_id);
            }
            Weight::Samples => cell.samples += samples,
        }
    }

    let mut cells: Vec<(i64, i64, i64)> = cells
        .into_iter()
        .map(|((x, y), cell)| {
            let weight = match request.weight {
                Weight::Days => cell.days.len() as i64,
                Weight::Samples => cell.samples,
            };
            (x, y, weight)
        })
        .collect();
    // A stable order keeps responses byte-identical for the same request, which the tests and
    // any cache in front of this rely on.
    cells.sort_unstable();
    Ok(Aggregate {
        cells,
        days: dates.len() as i64,
    })
}

#[derive(Default)]
struct Cell {
    days: HashSet<u32>,
    samples: i64,
}

/// The matched cells' outer edges as `[minLon, minLat, maxLon, maxLat]`, for the client to
/// frame the range with on first load.
fn bbox_of((min_x, min_y, max_x, max_y): (i64, i64, i64, i64), shift: u32) -> [f64; 4] {
    let (max_lat, min_lon) = cell_position(min_x as f64, min_y as f64, shift);
    let (min_lat, max_lon) = cell_position(max_x as f64 + 1.0, max_y as f64 + 1.0, shift);
    [min_lon, min_lat, max_lon, max_lat]
}

#[cfg(test)]
mod tests {
    use super::*;

    /// At the default coarsening a cell is about four screen pixels, which is what keeps a
    /// full viewport in the tens of thousands of points at most.
    #[test]
    fn the_shift_tracks_the_zoom_and_stops_at_both_ends() {
        assert_eq!(shift_for(14.0), CELL_ZOOM - 14 - 8 + PIXEL_SHIFT as u32);
        assert_eq!(shift_for(12.4), shift_for(12.0));
        assert_eq!(shift_for(12.6), shift_for(13.0));
        // One step of zoom is one step of coarsening, so cells stay the same size on screen.
        assert_eq!(shift_for(6.0) - shift_for(7.0), 1);
        // Zoomed all the way out a cell is a level 5 tile; zoomed in it is the grid itself.
        assert_eq!(shift_for(0.0), 16);
        assert_eq!(shift_for(16.0), 0);
        assert_eq!(shift_for(24.0), 0);
    }

    /// Too many points is a rendering problem, not a data problem, so the answer is bigger
    /// cells rather than fewer of them: every sample counted still ends up in the response.
    #[test]
    fn the_point_cap_coarsens_rather_than_dropping_cells() {
        let mut conn = crate::db::open_in_memory().unwrap();
        // 12 100 adjacent cells, comfortably over a cap of 10 000 and exactly a quarter of
        // that once the indices are shifted by one.
        let side = 110;
        let base = 1i64 << (CELL_ZOOM - 1);
        let tx = conn.transaction().unwrap();
        for x in 0..side {
            for y in 0..side {
                tx.execute(
                    "INSERT INTO heatmap_cells (date, x, y, samples)
                     VALUES ('2025-06-10', ?, ?, 1)",
                    params![base + x, base + y],
                )
                .unwrap();
            }
        }
        // The coarsened request reads the level 1 rollup, so seed that the way derive would.
        for x in 0..((side + 1) / 2) {
            for y in 0..((side + 1) / 2) {
                tx.execute(
                    "INSERT INTO heatmap_rollup (date, shift, x, y, samples)
                     VALUES ('2025-06-10', 1, ?, ?, 4)",
                    params![(base >> 1) + x, (base >> 1) + y],
                )
                .unwrap();
            }
        }
        tx.commit().unwrap();

        let request = Request {
            min_x: 0,
            min_y: 0,
            max_x: i64::MAX,
            max_y: i64::MAX,
            centre_lat: 0.0,
            from: String::new(),
            to: "9999-12-31".to_string(),
            weight: Weight::Samples,
            shift: 0,
        };
        assert_eq!(aggregate(&conn, &request, 0).unwrap().cells.len(), 12_100);

        let rendered = render_capped(&conn, &request, 10_000).unwrap();

        assert_eq!(rendered["meta"]["points"], 55 * 55);
        assert_eq!(rendered["meta"]["days"], 1);
        // Four fine cells to a coarse one, and not one sample lost on the way.
        assert_eq!(rendered["meta"]["maxWeight"], 4);
        let total: i64 = rendered["features"]
            .as_array()
            .unwrap()
            .iter()
            .map(|feature| feature["properties"]["weight"].as_i64().unwrap())
            .sum();
        assert_eq!(total, side * side);
        assert!(
            (rendered["meta"]["cellMetres"].as_f64().unwrap() - 2.0 * cell_metres(0, 0.0)).abs()
                < 1e-6
        );
    }
}
