use std::sync::Arc;

use axum::extract::State;
use axum::routing::get;
use axum::{Json, Router};
use serde_json::{Value, json};

/// The MapLibre style URL is the only thing the frontend needs from the server before it can
/// render, so it is the whole of state for now; ingest state joins it in phase 2.
#[derive(Clone)]
pub struct AppState {
    map_style: Arc<str>,
}

impl AppState {
    pub fn new(map_style: &str) -> Self {
        Self {
            map_style: Arc::from(map_style),
        }
    }
}

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/api/status", get(status))
        .route("/api/config", get(config))
        .with_state(state)
}

async fn status() -> Json<Value> {
    Json(json!({ "version": env!("CARGO_PKG_VERSION"), "ok": true }))
}

async fn config(State(state): State<AppState>) -> Json<Value> {
    Json(json!({ "mapStyle": state.map_style.as_ref() }))
}

#[cfg(test)]
mod tests {
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    use super::*;

    #[tokio::test]
    async fn status_reports_version_and_ok() {
        let app = router(AppState::new("https://example.com/style.json"));

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/status")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = response.into_body().collect().await.unwrap().to_bytes();
        let json: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["version"], env!("CARGO_PKG_VERSION"));
        assert_eq!(json["ok"], true);
    }
}
