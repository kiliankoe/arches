//! Integration tests over the whole router, against a database seeded by running the real
//! ingest over the fixture backup into a temp dir.

use std::fs;
use std::path::{Path, PathBuf};

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use http_body_util::BodyExt;
use serde_json::Value;
use tower::ServiceExt;

use super::*;
use crate::{db, ingest};

const DAY: &str = "2025-06-10";
const VISIT_ID: &str = "B1000000-0000-4000-8000-000000000101";
const TRAM_ID: &str = "B2000000-0000-4000-8000-000000000102";
const WALK_ID: &str = "B5000000-0000-4000-8000-000000000105";
const HBF_ID: &str = "A1000000-0000-4000-8000-000000000001";
const FLIGHT_ID: &str = "B7000000-0000-4000-8000-000000000107";
/// The `creator` of the GPX history fixtures, which is the `source` their rows carry.
const GPX_SOURCE: &str = "quantified-map-gpx";
/// A viewport around the fixture's corner of Dresden.
const DRESDEN: &str = "bbox=13.70,51.03,13.76,51.06";

struct Fixture {
    _temp: tempfile::TempDir,
    app: Router,
}

impl Fixture {
    fn new() -> Self {
        Self::build(false)
    }

    /// Also ingests the GPX history fixtures, the second source. Its 2025 file lands on Arc's
    /// first day and is skipped, so the two never overlap.
    fn with_gpx() -> Self {
        Self::build(true)
    }

    fn build(with_gpx: bool) -> Self {
        let temp = tempfile::tempdir().unwrap();
        let fixtures = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
        let arc_dir = temp.path().join("arc");
        copy_tree(&fixtures.join("backup"), &arc_dir);
        let gpx_dir = temp.path().join("gpx");
        if with_gpx {
            copy_tree(&fixtures.join("gpx"), &gpx_dir);
        }
        let config = Config::from_pairs([
            ("ARCHES_ARC_DIR", arc_dir.to_str().unwrap()),
            (
                "ARCHES_GPX_DIR",
                if with_gpx {
                    gpx_dir.to_str().unwrap()
                } else {
                    ""
                },
            ),
            (
                "ARCHES_DATA_DIR",
                temp.path().join("data").to_str().unwrap(),
            ),
            ("ARCHES_MAP_STYLE", "https://example.com/style.json"),
        ])
        .unwrap();

        let mut conn = db::open(&config.db_path()).unwrap();
        let summary = ingest::run(&mut conn, &config).unwrap();
        assert!(summary.error.is_none(), "{:?}", summary.error);

        Self {
            _temp: temp,
            app: router(AppState::new(config, conn)),
        }
    }

    async fn response(&self, request: Request<Body>) -> (StatusCode, Vec<u8>, header::HeaderMap) {
        let response = self.app.clone().oneshot(request).await.unwrap();
        let status = response.status();
        let headers = response.headers().clone();
        let body = response.into_body().collect().await.unwrap().to_bytes();
        (status, body.to_vec(), headers)
    }

    async fn get(&self, uri: &str) -> (StatusCode, Value) {
        let (status, body, _) = self
            .response(Request::builder().uri(uri).body(Body::empty()).unwrap())
            .await;
        let json = serde_json::from_slice(&body).unwrap_or_else(|_| {
            panic!(
                "{uri} did not return JSON: {}",
                String::from_utf8_lossy(&body)
            )
        });
        (status, json)
    }

    async fn ok(&self, uri: &str) -> Value {
        let (status, json) = self.get(uri).await;
        assert_eq!(status, StatusCode::OK, "{uri}: {json}");
        json
    }

    async fn text(&self, uri: &str) -> (StatusCode, String, header::HeaderMap) {
        let (status, body, headers) = self
            .response(Request::builder().uri(uri).body(Body::empty()).unwrap())
            .await;
        (status, String::from_utf8(body).unwrap(), headers)
    }
}

fn copy_tree(from: &Path, to: &Path) {
    fs::create_dir_all(to).unwrap();
    for entry in fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_tree(&entry.path(), &target);
        } else {
            fs::copy(entry.path(), &target).unwrap();
        }
    }
}

#[tokio::test]
async fn status_reports_the_run_and_the_counts_with_rfc_3339_dates() {
    let fixture = Fixture::new();

    let status = fixture.ok("/api/status").await;

    assert_eq!(status["version"], env!("CARGO_PKG_VERSION"));
    // The 10th has five items, the 12th the flight and its neighbours.
    assert_eq!(status["counts"]["items"], 9);
    assert_eq!(status["counts"]["places"], 2);
    assert_eq!(status["counts"]["samples"], 23);
    assert!(status["counts"]["heatmapCells"].as_u64().unwrap() > 15);
    assert_eq!(status["ingestRunning"], false);
    assert_eq!(status["firstSummarizedDate"], DAY);
    assert_eq!(status["lastBackupDate"], "2025-06-10T10:00:00Z");
    assert!(status["newestBucketMtime"].as_str().unwrap().ends_with('Z'));
    let run = &status["lastRun"];
    assert_eq!(run["filesIngested"], 4);
    assert!(
        run["startedAt"].as_str().unwrap().ends_with('Z'),
        "{}",
        run["startedAt"]
    );
}

/// The GPX history is a second source in the same tables, and the API says so on every item it
/// hands out and in the status counts.
#[tokio::test]
async fn a_gpx_day_reads_like_any_other_day() {
    let fixture = Fixture::with_gpx();

    let day = fixture.ok("/api/days/2016-02-18").await;

    assert_eq!(day["summary"]["itemCount"], 5);
    assert_eq!(day["summary"]["utcOffsetSeconds"], 3600);
    // The transport trip and the unnamed visit are what is left to review.
    assert_eq!(day["summary"]["unconfirmedItems"], 2);
    assert_eq!(day["summary"]["confirmed"], false);
    assert_eq!(day["summary"]["durationByType"]["walking"], 600);

    let items = day["items"].as_array().unwrap();
    assert!(
        items.iter().all(|item| item["source"] == GPX_SOURCE),
        "{items:?}"
    );
    let by_type = |activity: &str| {
        items
            .iter()
            .find(|item| item["activityType"] == activity)
            .unwrap_or_else(|| panic!("no {activity} item in {items:?}"))
            .clone()
    };
    // `transport` means motorised and nothing more, so the car it lands on is not a claim.
    let transport = by_type("car");
    assert_eq!(transport["confirmed"], false);
    assert_eq!(transport["uncertain"], true);
    let walk = by_type("walking");
    assert_eq!(walk["confirmed"], true);
    assert_eq!(walk["uncertain"], false);
    assert!(walk["distanceM"].as_f64().unwrap() > 1_000.0);

    let visit = items
        .iter()
        .find(|item| item["kind"] == "visit" && item["place"]["name"] == "Zwinger")
        .unwrap();
    assert_eq!(visit["confirmed"], true);
    assert_eq!(visit["place"]["visitCount"], 2);
    assert!(visit["place"]["countryCode"].is_null());

    // Arc's own day is untouched by any of it.
    let arc_day = fixture.ok(&format!("/api/days/{DAY}")).await;
    assert_eq!(arc_day["items"][0]["source"], "LocoKit2");
}

#[tokio::test]
async fn status_counts_what_each_source_contributed() {
    let fixture = Fixture::with_gpx();

    let status = fixture.ok("/api/status").await;

    let by_source = status["bySource"].as_array().unwrap();
    assert_eq!(by_source.len(), 2, "{by_source:?}");
    assert_eq!(by_source[0]["source"], "LocoKit2");
    assert_eq!(by_source[0]["items"], 9);
    assert_eq!(by_source[0]["samples"], 23);
    assert_eq!(by_source[1]["source"], GPX_SOURCE);
    assert_eq!(by_source[1]["items"], 8);
    assert_eq!(by_source[1]["samples"], 14);
    assert_eq!(by_source[1]["firstItemStart"], "2016-02-18T08:00:00Z");
}

#[tokio::test]
async fn config_serves_the_map_style() {
    let fixture = Fixture::new();
    let config = fixture.ok("/api/config").await;
    assert_eq!(config["mapStyle"], "https://example.com/style.json");
}

#[tokio::test]
async fn ingest_can_be_triggered_and_finds_nothing_changed() {
    let fixture = Fixture::new();

    let (status, body, _) = fixture
        .response(
            Request::builder()
                .method("POST")
                .uri("/api/ingest")
                .body(Body::empty())
                .unwrap(),
        )
        .await;

    assert_eq!(status, StatusCode::OK);
    let summary: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(summary["filesSeen"], 4);
    assert_eq!(summary["filesIngested"], 0);
    assert!(summary["startedAt"].as_str().unwrap().ends_with('Z'));
}

#[tokio::test]
async fn days_returns_only_the_days_that_have_rows() {
    let fixture = Fixture::new();

    let days = fixture.ok("/api/days?from=2025-06-01&to=2025-06-30").await;

    let days = days.as_array().unwrap();
    assert_eq!(days.len(), 2, "{days:?}");
    assert_eq!(days[0]["date"], DAY);
    assert_eq!(days[1]["date"], "2025-06-12");
    let first = &days[0];
    assert_eq!(first["utcOffsetSeconds"], 7200);
    assert_eq!(first["itemCount"], 5);
    assert_eq!(first["visitCount"], 2);
    assert_eq!(first["tripCount"], 3);
    assert!(first["distanceM"].as_f64().unwrap() > 1_000.0);
    assert!(first["durationByType"]["tram"].as_i64().unwrap() > 0);
    assert_eq!(first["placeIds"].as_array().unwrap().len(), 2);
    assert_eq!(first["countryCodes"][0], "de");
    assert_eq!(first["localities"][0], "Dresden");
    assert_eq!(first["bbox"].as_array().unwrap().len(), 4);
    // [minLon, minLat, maxLon, maxLat], so longitude comes first.
    assert!(first["bbox"][0].as_f64().unwrap() < 20.0);
    assert!(first["firstSampleAt"].as_str().unwrap().ends_with('Z'));
}

/// Two of the day's five items were never reviewed in Arc, so nothing about it is final yet.
#[tokio::test]
async fn confirmation_flags_travel_from_items_to_days() {
    let fixture = Fixture::new();

    let day = fixture.ok(&format!("/api/days/{DAY}")).await;

    assert_eq!(day["summary"]["unconfirmedItems"], 2);
    assert_eq!(day["summary"]["confirmed"], false);
    let items = day["items"].as_array().unwrap();
    let by_id = |id: &str| items.iter().find(|item| item["id"] == id).unwrap().clone();
    assert_eq!(by_id(VISIT_ID)["confirmed"], true);
    assert_eq!(by_id(TRAM_ID)["confirmed"], true);
    assert_eq!(by_id(WALK_ID)["confirmed"], false);
    assert_eq!(by_id(WALK_ID)["uncertain"], true);

    // The second day is the flight and its neighbours; only the arrival is still unreviewed.
    let second = fixture.ok("/api/days/2025-06-12").await;
    assert_eq!(second["summary"]["itemCount"], 4);
    assert_eq!(second["summary"]["confirmed"], false);
    assert_eq!(second["summary"]["unconfirmedItems"], 1);
}

#[tokio::test]
async fn a_day_lists_its_items_in_order_with_places_and_clipping() {
    let fixture = Fixture::new();

    let day = fixture.ok(&format!("/api/days/{DAY}")).await;

    let items = day["items"].as_array().unwrap();
    assert_eq!(items.len(), 5);
    assert_eq!(items[0]["id"], VISIT_ID);
    assert_eq!(items[0]["kind"], "visit");
    assert_eq!(items[0]["startDate"], "2025-06-10T08:00:00Z");
    assert_eq!(items[0]["localStartDate"], DAY);
    assert_eq!(items[0]["startOffsetSeconds"], 7200);
    assert_eq!(items[0]["durationSeconds"], 1200);
    // The whole visit falls inside the day, so clipping changes nothing.
    assert_eq!(items[0]["clippedSeconds"], 1200);
    assert_eq!(items[0]["place"]["name"], "Dresden Hauptbahnhof");
    assert_eq!(items[0]["place"]["locality"], "Dresden");
    assert_eq!(items[0]["place"]["id"], HBF_ID);
    assert_eq!(items[0]["health"]["stepCount"], 320.0);
    assert_eq!(items[1]["kind"], "trip");
    assert_eq!(items[1]["activityType"], "tram");
    assert_eq!(items[1]["distanceM"], 1420.5);
    assert!(items[1]["place"].is_null());
}

#[tokio::test]
async fn days_rejects_bad_parameters_and_unknown_dates() {
    let fixture = Fixture::new();

    for uri in [
        "/api/days?from=nonsense",
        "/api/days?from=2025-06-10&to=2025-06-01",
        "/api/days?from=2020-01-01&to=2025-01-01",
    ] {
        let (status, json) = fixture.get(uri).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{uri}");
        assert!(json["error"].is_string(), "{uri}: {json}");
    }

    let (status, json) = fixture.get("/api/days/2024-01-01").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert!(json["error"].as_str().unwrap().contains("2024-01-01"));
    assert_eq!(
        fixture.get("/api/days/nope").await.0,
        StatusCode::BAD_REQUEST
    );

    // The default range ends today, so the fixture's 2025 days are well outside it.
    let today = fixture.ok("/api/days").await;
    assert!(today.as_array().unwrap().is_empty(), "{today}");
}

#[tokio::test]
async fn geojson_has_a_line_per_trip_and_a_point_per_visit() {
    let fixture = Fixture::new();

    let collection = fixture.ok(&format!("/api/days/{DAY}/geojson")).await;

    assert_eq!(collection["type"], "FeatureCollection");
    let features = collection["features"].as_array().unwrap();
    // Two visits, the tram and the walk. The third trip has no fix on this day.
    assert_eq!(features.len(), 4, "{features:?}");
    let kinds: Vec<&str> = features
        .iter()
        .map(|feature| feature["geometry"]["type"].as_str().unwrap())
        .collect();
    assert_eq!(kinds, ["Point", "LineString", "Point", "LineString"]);

    let point = &features[0];
    assert_eq!(point["properties"]["date"], DAY);
    assert_eq!(point["properties"]["itemId"], VISIT_ID);
    assert_eq!(point["properties"]["placeId"], HBF_ID);
    assert_eq!(point["properties"]["name"], "Dresden Hauptbahnhof");
    assert_eq!(point["properties"]["confirmed"], true);
    assert_eq!(point["geometry"]["coordinates"][0], 13.732);

    let line = &features[1];
    assert_eq!(line["properties"]["itemId"], TRAM_ID);
    assert_eq!(line["properties"]["activityType"], "tram");
    assert_eq!(
        line["geometry"]["coordinates"].as_array().unwrap().len(),
        10
    );
    assert_eq!(features[3]["properties"]["confirmed"], false);

    assert_eq!(
        fixture.get("/api/days/2024-01-01/geojson").await.0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        fixture
            .get(&format!("/api/days/{DAY}/geojson?simplify=nope"))
            .await
            .0,
        StatusCode::BAD_REQUEST
    );
}

/// The range rendering is the union of the days in it, and `/api/days/geojson` is a route of its
/// own rather than a date that happens to parse.
#[tokio::test]
async fn range_geojson_unions_the_days_and_labels_every_feature() {
    let fixture = Fixture::new();

    let range = fixture
        .ok("/api/days/geojson?from=2025-06-01&to=2025-06-30")
        .await;

    assert_eq!(range["type"], "FeatureCollection");
    let features = range["features"].as_array().unwrap();
    let first = fixture.ok(&format!("/api/days/{DAY}/geojson")).await;
    let second = fixture.ok("/api/days/2025-06-12/geojson").await;
    let expected =
        first["features"].as_array().unwrap().len() + second["features"].as_array().unwrap().len();
    assert_eq!(features.len(), expected, "{features:#?}");
    let dates: Vec<&str> = features
        .iter()
        .map(|feature| feature["properties"]["date"].as_str().unwrap())
        .collect();
    assert_eq!(dates.iter().filter(|date| **date == DAY).count(), 4);
    assert!(dates.contains(&"2025-06-12"));
    assert_eq!(features[0]["properties"]["itemId"], VISIT_ID);

    // A range the recording never reached is an empty collection, not a 404.
    let empty = fixture
        .ok("/api/days/geojson?from=2025-07-01&to=2025-07-31")
        .await;
    assert!(empty["features"].as_array().unwrap().is_empty(), "{empty}");

    let simplified = fixture
        .ok("/api/days/geojson?from=2025-06-01&to=2025-06-30&simplify=100")
        .await;
    let total = |collection: &Value| -> usize {
        collection["features"]
            .as_array()
            .unwrap()
            .iter()
            .map(|feature| match &feature["geometry"]["coordinates"] {
                Value::Array(coordinates) => coordinates.len(),
                _ => 0,
            })
            .sum()
    };
    assert!(total(&simplified) < total(&range));
}

#[tokio::test]
async fn range_geojson_rejects_a_missing_or_broken_range() {
    let fixture = Fixture::new();

    for uri in [
        "/api/days/geojson",
        "/api/days/geojson?from=2025-06-01",
        "/api/days/geojson?to=2025-06-30",
        "/api/days/geojson?from=someday&to=2025-06-30",
        "/api/days/geojson?from=2025-06-30&to=2025-06-01",
        "/api/days/geojson?from=2025-01-01&to=2025-06-30",
        "/api/days/geojson?from=2025-06-01&to=2025-06-30&simplify=nope",
    ] {
        let (status, body) = fixture.get(uri).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{uri}: {body}");
        assert!(body["error"].is_string(), "{uri}: {body}");
    }

    // Were the literal captured as a date it would 400 on the parse, not on the missing range.
    let (_, body) = fixture.get("/api/days/geojson").await;
    assert_eq!(body["error"], "from is required");
}

#[tokio::test]
async fn simplify_drops_points_but_never_the_ends() {
    let fixture = Fixture::new();

    let full = fixture.ok(&format!("/api/days/{DAY}/geojson")).await;
    let simplified = fixture
        .ok(&format!("/api/days/{DAY}/geojson?simplify=100"))
        .await;

    let line = |collection: &Value| collection["features"][1]["geometry"]["coordinates"].clone();
    let full = line(&full);
    let simplified = line(&simplified);
    let (full, simplified) = (full.as_array().unwrap(), simplified.as_array().unwrap());

    assert!(
        simplified.len() < full.len(),
        "{} points, expected fewer than {}",
        simplified.len(),
        full.len()
    );
    assert_eq!(simplified.first(), full.first());
    assert_eq!(simplified.last(), full.last());
}

#[tokio::test]
async fn gpx_renders_waypoints_tracks_and_escapes_markup() {
    let fixture = Fixture::new();

    let (status, gpx, headers) = fixture.text(&format!("/api/days/{DAY}.gpx")).await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(headers[header::CONTENT_TYPE], "application/gpx+xml");
    assert!(gpx.starts_with("<?xml"), "{gpx}");
    assert!(gpx.contains(r#"<gpx version="1.1""#));
    assert!(gpx.contains(r#"<wpt lat="51.0403" lon="13.732">"#), "{gpx}");
    assert!(gpx.contains("<time>2025-06-10T08:00:00Z</time>"));
    assert!(gpx.contains("<name>Dresden Hauptbahnhof</name>"));
    assert!(gpx.contains("<type>tram</type>"));
    assert!(gpx.contains("<ele>113</ele>"), "{gpx}");
    assert!(gpx.contains("<trkpt lat=\"51.0403\" lon=\"13.732\">"));
    // The visit's custom title has an ampersand in it and must arrive escaped.
    assert!(gpx.contains("<name>coffee &amp; cake</name>"), "{gpx}");
    assert!(!gpx.contains("coffee & cake"));

    assert_eq!(
        fixture.get("/api/days/2024-01-01.gpx").await.0,
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn an_item_and_its_samples_are_addressable() {
    let fixture = Fixture::new();

    let item = fixture.ok(&format!("/api/items/{TRAM_ID}")).await;
    assert_eq!(item["kind"], "trip");
    assert_eq!(item["activityType"], "tram");
    assert_eq!(item["confirmed"], true);
    // Nothing to clip against outside a day.
    assert!(item["clippedSeconds"].is_null());
    assert_eq!(
        fixture.ok(&format!("/api/items/{VISIT_ID}")).await["place"]["id"],
        HBF_ID
    );

    let samples = fixture.ok(&format!("/api/items/{TRAM_ID}/samples")).await;
    let samples = samples.as_array().unwrap();
    assert_eq!(samples.len(), 10);
    assert_eq!(samples[0]["date"], "2025-06-10T08:20:00Z");
    assert_eq!(samples[0]["latitude"], 51.0403);
    assert_eq!(samples[0]["altitude"], 113.0);
    assert_eq!(samples[0]["movingState"], "moving");
    assert_eq!(samples[0]["classifiedActivityType"], "tram");

    let simplified = fixture
        .ok(&format!("/api/items/{TRAM_ID}/samples?simplify=100"))
        .await;
    let simplified = simplified.as_array().unwrap();
    assert!(simplified.len() < samples.len());
    assert_eq!(simplified.first(), samples.first());
    assert_eq!(simplified.last(), samples.last());

    assert_eq!(
        fixture.get("/api/items/nope").await.0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        fixture.get("/api/items/nope/samples").await.0,
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn places_search_filter_and_list_visits() {
    let fixture = Fixture::new();

    let all = fixture.ok("/api/places").await;
    let all = all.as_array().unwrap();
    assert_eq!(all.len(), 2);
    // Most visited first.
    assert_eq!(all[0]["id"], HBF_ID);
    assert_eq!(all[0]["visitCount"], 12);
    assert_eq!(all[0]["visitDays"], 9);
    assert_eq!(all[0]["radiusMean"], 62.5);
    assert_eq!(all[0]["category"], "transit_station");
    assert_eq!(all[0]["isStale"], false);
    assert_eq!(all[0]["lastVisitDate"], "2025-06-10T08:20:00Z");

    // Case-insensitive, and across name, locality and street address.
    assert_eq!(fixture.ok("/api/places?q=hauptbahn").await[0]["id"], HBF_ID);
    assert_eq!(
        fixture.ok("/api/places?q=wiener%20platz").await[0]["id"],
        HBF_ID
    );
    assert_eq!(
        fixture
            .ok("/api/places?q=dresden")
            .await
            .as_array()
            .unwrap()
            .len(),
        2
    );
    assert!(
        fixture
            .ok("/api/places?q=berlin")
            .await
            .as_array()
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        fixture
            .ok("/api/places?country=DE")
            .await
            .as_array()
            .unwrap()
            .len(),
        2
    );
    assert_eq!(
        fixture
            .ok("/api/places?limit=1")
            .await
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        fixture.get("/api/places?limit=0").await.0,
        StatusCode::BAD_REQUEST
    );

    let place = fixture.ok(&format!("/api/places/{HBF_ID}")).await;
    assert_eq!(place["name"], "Dresden Hauptbahnhof");
    assert_eq!(
        fixture.get("/api/places/nope").await.0,
        StatusCode::NOT_FOUND
    );

    let visits = fixture.ok(&format!("/api/places/{HBF_ID}/visits")).await;
    let visits = visits.as_array().unwrap();
    assert_eq!(visits.len(), 1);
    assert_eq!(visits[0]["id"], VISIT_ID);
    assert_eq!(visits[0]["place"]["id"], HBF_ID);
    assert!(
        fixture
            .ok(&format!("/api/places/{HBF_ID}/visits?from=2025-07-01"))
            .await
            .as_array()
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        fixture.get("/api/places/nope/visits").await.0,
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn near_orders_by_distance_and_honours_the_radius() {
    let fixture = Fixture::new();

    // Hauptbahnhof itself: Postplatz is about 1.07 km away.
    let near = fixture
        .ok("/api/near?lat=51.0403&lon=13.7320&radius=2000")
        .await;
    let near = near.as_array().unwrap();
    assert_eq!(near.len(), 2);
    assert_eq!(near[0]["id"], HBF_ID);
    assert!(near[0]["distanceM"].as_f64().unwrap() < 1.0);
    assert!(near[1]["distanceM"].as_f64().unwrap() > near[0]["distanceM"].as_f64().unwrap());

    // The default radius of 250 m leaves Postplatz out.
    assert_eq!(
        fixture
            .ok("/api/near?lat=51.0403&lon=13.7320")
            .await
            .as_array()
            .unwrap()
            .len(),
        1
    );

    for uri in [
        "/api/near",
        "/api/near?lat=51.0403",
        "/api/near?lat=51.0403&lon=13.732&radius=99999",
        "/api/near?lat=okay&lon=13.732",
        "/api/near?lat=991.0&lon=13.732",
    ] {
        assert_eq!(fixture.get(uri).await.0, StatusCode::BAD_REQUEST, "{uri}");
    }
}

#[tokio::test]
async fn at_finds_the_covering_item_and_prefers_the_visit() {
    let fixture = Fixture::new();

    let inside = fixture.ok("/api/at?ts=2025-06-10T08:22:00Z").await;
    assert_eq!(inside["item"]["id"], TRAM_ID);
    assert_eq!(inside["item"]["activityType"], "tram");

    // 08:20 ends the visit and starts the tram ride; the visit is the more specific answer.
    let boundary = fixture.ok("/api/at?ts=2025-06-10T08:20:00Z").await;
    assert_eq!(boundary["item"]["id"], VISIT_ID);
    assert_eq!(boundary["item"]["place"]["id"], HBF_ID);

    // A gap in the recording is an answer, not a 404.
    let nothing = fixture.ok("/api/at?ts=2020-01-01T00:00:00Z").await;
    assert!(nothing["item"].is_null());

    assert_eq!(fixture.get("/api/at").await.0, StatusCode::BAD_REQUEST);
    assert_eq!(
        fixture.get("/api/at?ts=yesterday").await.0,
        StatusCode::BAD_REQUEST
    );
}

/// pensieve's frontend fetches map data straight from the browser, so every GET has to be
/// readable cross-origin.
#[tokio::test]
async fn cors_allows_any_origin() {
    let fixture = Fixture::new();

    let (status, _, headers) = fixture
        .response(
            Request::builder()
                .uri("/api/config")
                .header(header::ORIGIN, "https://pensieve.example")
                .body(Body::empty())
                .unwrap(),
        )
        .await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(headers[header::ACCESS_CONTROL_ALLOW_ORIGIN], "*");
}

/// `web/dist` is empty in a checkout that never ran the frontend build, and the fallback has
/// to say so rather than 404 at whoever opened the page.
#[tokio::test]
async fn the_ui_falls_back_to_a_note_when_it_was_not_built() {
    let fixture = Fixture::new();

    let (status, body, _) = fixture.text("/some/client/route").await;

    assert_eq!(status, StatusCode::OK);
    assert!(
        body.contains("<!doctype html") || body.contains("was not built"),
        "{body}"
    );
}

/// The fixture is two days a couple of hundred metres apart in Dresden, which is enough to
/// pin down both what the grid does and what the two weightings mean. Zoomed in, every fix is
/// its own cell (plus the cells the trips are rasterized through) and only the one coordinate
/// both days share carries a weight of two; zoomed out to 6 km cells, the whole thing
/// collapses into a single point.
#[tokio::test]
async fn heatmap_aggregates_by_zoom_and_by_weight() {
    let fixture = Fixture::new();

    let close = fixture.ok(&format!("/api/heatmap?{DRESDEN}&zoom=17")).await;
    assert_eq!(close["type"], "FeatureCollection");
    // Every fix is a cell, and the walk's segments are rasterized into the cells between them.
    let points = close["meta"]["points"].as_u64().unwrap();
    assert!(points > 14, "{points}");
    assert_eq!(close["features"].as_array().unwrap().len() as u64, points);
    // Both days recorded at Postplatz, everywhere else was passed through once.
    assert_eq!(close["meta"]["maxWeight"], 2);
    assert_eq!(close["meta"]["days"], 2);
    assert!(close["meta"]["cellMetres"].as_f64().unwrap() < 7.0);

    let samples = fixture
        .ok(&format!("/api/heatmap?{DRESDEN}&zoom=17&weight=samples"))
        .await;
    assert_eq!(samples["meta"]["points"].as_u64().unwrap(), points);
    // Two fixes on the 10th and one on the 12th, where counting days said 2.
    assert_eq!(samples["meta"]["maxWeight"], 3);

    let far = fixture.ok(&format!("/api/heatmap?{DRESDEN}&zoom=6")).await;
    assert_eq!(far["meta"]["points"], 1);
    assert_eq!(far["meta"]["maxWeight"], 2);
    assert!(far["meta"]["cellMetres"].as_f64().unwrap() > 5_000.0);

    let far_samples = fixture
        .ok(&format!("/api/heatmap?{DRESDEN}&zoom=6&weight=samples"))
        .await;
    // Every fix on both days minus the one with no coordinates, plus the cells the trips were
    // rasterized through: more than the fixes, and the same total the zoomed-in view added up to.
    let far_weight = far_samples["features"][0]["properties"]["weight"]
        .as_u64()
        .unwrap();
    assert!(far_weight > 16, "{far_weight}");
    let close_total: u64 = samples["features"]
        .as_array()
        .unwrap()
        .iter()
        .map(|feature| feature["properties"]["weight"].as_u64().unwrap())
        .sum();
    assert_eq!(far_weight, close_total);

    // The point sits at the centre of its cell, and the meta bbox spans that cell's edges.
    let [longitude, latitude] = [
        far["features"][0]["geometry"]["coordinates"][0]
            .as_f64()
            .unwrap(),
        far["features"][0]["geometry"]["coordinates"][1]
            .as_f64()
            .unwrap(),
    ];
    let bbox = far["meta"]["bbox"].as_array().unwrap();
    assert!(bbox[0].as_f64().unwrap() < longitude && longitude < bbox[2].as_f64().unwrap());
    assert!(bbox[1].as_f64().unwrap() < latitude && latitude < bbox[3].as_f64().unwrap());
}

#[tokio::test]
async fn heatmap_respects_the_date_range_and_the_viewport() {
    let fixture = Fixture::new();

    let second = fixture
        .ok(&format!(
            "/api/heatmap?{DRESDEN}&zoom=6&from=2025-06-12&to=2025-06-12&weight=samples"
        ))
        .await;
    assert_eq!(second["meta"]["days"], 1);
    assert_eq!(second["features"][0]["properties"]["weight"], 1);

    let none = fixture
        .ok(&format!(
            "/api/heatmap?{DRESDEN}&zoom=6&from=2025-06-13&to=2025-06-30"
        ))
        .await;
    assert_eq!(none["meta"]["points"], 0);
    assert_eq!(none["meta"]["days"], 0);
    assert_eq!(none["meta"]["bbox"], Value::Null);

    // Berlin, 165 km away: in range on the dates, out of it on the map.
    let elsewhere = fixture
        .ok("/api/heatmap?bbox=13.3,52.4,13.5,52.6&zoom=12")
        .await;
    assert_eq!(elsewhere["meta"]["points"], 0);
}

#[tokio::test]
async fn heatmap_rejects_a_missing_or_broken_viewport() {
    let fixture = Fixture::new();

    for uri in [
        "/api/heatmap?zoom=12",
        &format!("/api/heatmap?{DRESDEN}"),
        "/api/heatmap?bbox=13.70,51.03,13.76&zoom=12",
        "/api/heatmap?bbox=13.70,51.03,east,51.06&zoom=12",
        // Inside out: the maximum corner is south-west of the minimum one.
        "/api/heatmap?bbox=13.76,51.06,13.70,51.03&zoom=12",
        &format!("/api/heatmap?{DRESDEN}&zoom=99"),
        &format!("/api/heatmap?{DRESDEN}&zoom=near"),
        &format!("/api/heatmap?{DRESDEN}&zoom=12&weight=hours"),
        &format!("/api/heatmap?{DRESDEN}&zoom=12&from=last-week"),
        &format!("/api/heatmap?{DRESDEN}&zoom=12&from=2025-06-12&to=2025-06-10"),
    ] {
        let (status, body) = fixture.get(uri).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{uri}: {body}");
        assert!(body["error"].is_string(), "{uri}: {body}");
    }
}

/// The fixture is a day in Dresden and, two days later, a flight to Prague: the first time in
/// both countries and both cities, one flight and one walk long enough to be worth reporting.
#[tokio::test]
async fn highlights_report_first_times_flights_and_the_longest_trip() {
    let fixture = Fixture::new();

    let events = fixture
        .ok("/api/highlights?from=2025-06-01&to=2025-06-30")
        .await;

    let events = events.as_array().unwrap();
    let titles: Vec<&str> = events
        .iter()
        .map(|event| event["title"].as_str().unwrap())
        .collect();
    assert_eq!(
        titles,
        [
            "First time in Germany",
            "First time in Dresden",
            "First time in Czechia",
            "First time in Praha",
            "Flight from Dresden to Praha",
            "Longest walk: 1.4 km",
        ],
        "{events:#?}"
    );

    let germany = &events[0];
    assert_eq!(germany["kind"], "country");
    assert_eq!(germany["date"], DAY);
    // Uppercase as ISO 3166-1 writes it, whichever way round Arc happened to store it.
    assert_eq!(germany["countryCode"], "DE");
    // Two of the day's items are still unreviewed, so the day is not final and nor is this.
    assert_eq!(germany["confirmed"], false);
    // Only Germany was recorded on the 10th, so the locality's country is not in doubt.
    assert_eq!(events[1]["locality"], "Dresden");
    assert_eq!(events[1]["countryCode"], "DE");
    // The 12th touched two countries, so there is no telling which one Praha is in.
    assert!(events[3]["countryCode"].is_null());

    let flight = &events[4];
    assert_eq!(flight["date"], "2025-06-12");
    assert_eq!(flight["itemId"], FLIGHT_ID);
    assert_eq!(flight["distanceM"], 120_000.0);
    assert_eq!(flight["durationSeconds"], 5100);
    assert_eq!(flight["from"]["locality"], "Dresden");
    assert_eq!(flight["from"]["placeName"], "Dresden Airport");
    // The arrival is a visit past the walk out of the terminal, not the flight's own neighbour.
    assert_eq!(flight["to"]["locality"], "Praha");
    assert_eq!(flight["to"]["countryCode"], "CZ");
    assert_eq!(flight["confirmed"], true);

    let walk = &events[5];
    assert_eq!(walk["activityType"], "walking");
    assert_eq!(walk["distanceM"], 1400.0);

    // A range before any of it happened is an empty feed, not a 404.
    let empty = fixture
        .ok("/api/highlights?from=2025-01-01&to=2025-01-31")
        .await;
    assert!(empty.as_array().unwrap().is_empty(), "{empty}");
}

/// A first time is first over all of history: the same range asked for later reports nothing.
#[tokio::test]
async fn highlights_filter_by_kind_and_by_confirmation() {
    let fixture = Fixture::new();

    let later = fixture
        .ok("/api/highlights?from=2025-06-11&to=2025-06-30&kinds=country")
        .await;
    let later = later.as_array().unwrap();
    assert_eq!(later.len(), 1);
    assert_eq!(later[0]["countryCode"], "CZ");

    let two = fixture
        .ok("/api/highlights?from=2025-06-01&to=2025-06-30&kinds=flight,longest")
        .await;
    assert_eq!(two.as_array().unwrap().len(), 2);

    // The flight and the walk are confirmed items; both days are still unreviewed.
    let reviewed = fixture
        .ok("/api/highlights?from=2025-06-01&to=2025-06-30&confirmed=true")
        .await;
    let reviewed = reviewed.as_array().unwrap();
    assert_eq!(reviewed.len(), 2, "{reviewed:#?}");
    assert!(reviewed.iter().all(|event| event["confirmed"] == true));
}

#[tokio::test]
async fn highlights_reject_a_missing_or_broken_range() {
    let fixture = Fixture::new();

    for uri in [
        "/api/highlights",
        "/api/highlights?from=2025-06-01",
        "/api/highlights?to=2025-06-30",
        "/api/highlights?from=yesterday&to=2025-06-30",
        "/api/highlights?from=2025-06-30&to=2025-06-01",
        "/api/highlights?from=2020-01-01&to=2025-01-01",
        "/api/highlights?from=2025-06-01&to=2025-06-30&kinds=weather",
        "/api/highlights?from=2025-06-01&to=2025-06-30&kinds=",
        "/api/highlights?from=2025-06-01&to=2025-06-30&confirmed=maybe",
    ] {
        let (status, body) = fixture.get(uri).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{uri}: {body}");
        assert!(body["error"].is_string(), "{uri}: {body}");
    }
}
