//! The HTTP API.
//!
//! Everything lives under `/api` and speaks JSON, except the GeoJSON and GPX renderings of a
//! day. Errors are `{ "error": "..." }` with 400 for a bad parameter, 404 for an id or date
//! that is not there and 500 otherwise. Dates are local `YYYY-MM-DD`, timestamps are RFC 3339,
//! activity types are their enum names, and keys are camelCase.
//!
//! CORS allows any origin: the API is tailnet-only with no auth, and pensieve's browser
//! frontend fetches map data from it directly.

mod assets;
mod at;
mod convert;
mod days;
mod db;
mod error;
mod geojson;
mod gpx;
mod heatmap;
mod highlights;
mod items;
mod model;
mod places;
mod status;

#[cfg(test)]
mod tests;

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::{Context, Result};
use axum::http::{Method, header};
use axum::routing::{get, post};
use axum::{Json, Router};
use rusqlite::Connection;
use serde_json::{Value, json};
use tower_http::compression::CompressionLayer;
use tower_http::cors::{Any, CorsLayer};

use crate::config::Config;
use crate::ingest::RunSummary;
use convert::parse_f64;
use db::Db;
use error::{ApiError, ApiResult};

#[derive(Clone)]
pub struct AppState {
    map_style: Arc<str>,
    map_style_dark: Arc<str>,
    db: Arc<Db>,
    /// The one connection ingest writes through. Readers never touch it, so a pass in flight
    /// cannot block a request: WAL lets them run side by side.
    ingest_conn: Arc<std::sync::Mutex<Connection>>,
    ingest_config: Arc<Config>,
    ingest_running: Arc<AtomicBool>,
}

impl AppState {
    pub fn new(config: Config, ingest_conn: Connection) -> Self {
        Self {
            map_style: Arc::from(config.map_style.as_str()),
            map_style_dark: Arc::from(config.map_style_dark.as_str()),
            db: Db::new(&config.db_path()),
            ingest_conn: Arc::new(std::sync::Mutex::new(ingest_conn)),
            ingest_config: Arc::new(config),
            ingest_running: Arc::new(AtomicBool::new(false)),
        }
    }

    /// One ingest pass on the dedicated connection. Serialized by ingest's own lock, so the
    /// timer and `POST /api/ingest` queue behind each other rather than racing.
    pub async fn run_ingest(&self) -> Result<RunSummary> {
        let conn = Arc::clone(&self.ingest_conn);
        let config = Arc::clone(&self.ingest_config);
        let running = Arc::clone(&self.ingest_running);

        tokio::task::spawn_blocking(move || {
            running.store(true, Ordering::Relaxed);
            let mut conn = conn
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let summary = crate::ingest::run(&mut conn, &config);
            running.store(false, Ordering::Relaxed);
            summary
        })
        .await
        .context("ingest task panicked")?
    }

    /// The periodic pass: once at startup, then every `ingest_interval`. Arc rewrites whole
    /// buckets retroactively, so polling is the only way to notice.
    pub fn spawn_periodic_ingest(&self) {
        let state = self.clone();
        let interval = state.ingest_config.ingest_interval;
        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(interval);
            // A slow pass must not make the next ones fire back to back.
            ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            loop {
                ticker.tick().await;
                // An unreachable iCloud folder or a half-written bucket is a normal day; the
                // server logs it and tries again on the next tick.
                match state.run_ingest().await {
                    Ok(summary) => tracing::info!(
                        files_ingested = summary.files_ingested,
                        days_recomputed = summary.days_recomputed,
                        elapsed_ms = summary.elapsed_ms,
                        "periodic ingest finished"
                    ),
                    Err(error) => {
                        tracing::error!(error = %format!("{error:#}"), "periodic ingest failed")
                    }
                }
            }
        });
    }
}

pub fn router(state: AppState) -> Router {
    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods([Method::GET, Method::POST])
        .allow_headers([header::CONTENT_TYPE]);

    Router::new()
        .route("/api/status", get(status::get))
        .route("/api/config", get(config))
        .route("/api/ingest", post(status::ingest))
        .route("/api/days", get(days::list))
        // Before the parameter route it shares a shape with: matchit prefers the static segment,
        // and registering it first says so rather than relying on the reader knowing that.
        .route("/api/days/geojson", get(days::geojson_range))
        // Also serves `{date}.gpx`: axum captures whole segments only.
        .route("/api/days/{date}", get(days::get))
        .route("/api/days/{date}/geojson", get(days::geojson))
        .route("/api/items/{id}", get(items::get))
        .route("/api/items/{id}/samples", get(items::samples))
        .route("/api/places", get(places::list))
        .route("/api/places/{id}", get(places::get))
        .route("/api/places/{id}/visits", get(places::visits))
        .route("/api/near", get(places::near))
        .route("/api/at", get(at::at))
        .route("/api/heatmap", get(heatmap::get))
        .route("/api/highlights", get(highlights::get))
        .fallback(assets::serve)
        .layer(cors)
        .layer(CompressionLayer::new())
        .with_state(state)
}

async fn config(axum::extract::State(state): axum::extract::State<AppState>) -> Json<Value> {
    Json(json!({
        "mapStyle": state.map_style.as_ref(),
        "mapStyleDark": state.map_style_dark.as_ref(),
    }))
}

/// The shared `simplify=<metres>` parameter: absent means the full trace.
fn simplify_tolerance(params: &HashMap<String, String>) -> ApiResult<Option<f64>> {
    let Some(raw) = params.get("simplify") else {
        return Ok(None);
    };
    let tolerance = parse_f64("simplify", raw)?;
    if tolerance < 0.0 {
        return Err(ApiError::bad_request("simplify must not be negative"));
    }
    Ok((tolerance > 0.0).then_some(tolerance))
}
