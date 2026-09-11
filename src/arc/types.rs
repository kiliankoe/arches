//! Serde types for the LocoKit2 bucketed export format.
//!
//! Two rules shape every struct here. Arc omits null fields entirely rather than emitting
//! `null`, so anything not guaranteed by the spec is `Option`. And nothing uses
//! `deny_unknown_fields`, because a newer schema version must still ingest.

use jiff::Timestamp;
use serde::{Deserialize, Serialize};

use super::enums::{ActivityType, MovingState, RecordingState};

/// `metadata.json` at the root of a device backup dir.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Metadata {
    pub schema_version: String,
    pub export_id: Option<String>,
    pub export_mode: Option<String>,
    /// `"full"` or `"incremental"` upstream, kept as a string so a third value cannot break ingest.
    pub export_type: Option<String>,
    pub session_start_date: Option<Timestamp>,
    /// Null while a backup session is in flight or was never finalized.
    pub session_finish_date: Option<Timestamp>,
    pub last_backup_date: Option<Timestamp>,
    pub backup_progress_date: Option<Timestamp>,
    pub items_completed: Option<bool>,
    pub places_completed: Option<bool>,
    pub samples_completed: Option<bool>,
    pub stats: Option<Stats>,
    pub extensions: Option<serde_json::Value>,
    pub app_metadata: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Stats {
    pub place_count: Option<i64>,
    pub item_count: Option<i64>,
    pub sample_count: Option<i64>,
}

/// One record from `places/{0-9A-F}.json`.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Place {
    pub id: String,
    pub name: String,
    pub latitude: f64,
    pub longitude: f64,
    pub radius_mean: Option<f64>,
    #[serde(rename = "radiusSD")]
    pub radius_sd: Option<f64>,
    pub street_address: Option<String>,
    pub locality: Option<String>,
    pub country_code: Option<String>,
    /// Null for pre-2019 data that predates timezone capture.
    #[serde(rename = "secondsFromGMT")]
    pub seconds_from_gmt: Option<i32>,
    pub is_stale: Option<bool>,
    pub visit_count: Option<i64>,
    pub visit_days: Option<i64>,
    pub last_visit_date: Option<Timestamp>,
    pub last_saved: Timestamp,
    pub source: Option<String>,
    pub rtree_id: Option<i64>,
    pub mapbox_place_id: Option<String>,
    pub mapbox_category: Option<String>,
    pub mapbox_maki_icon: Option<String>,
    pub google_place_id: Option<String>,
    pub google_primary_type: Option<String>,
    pub foursquare_place_id: Option<String>,
    pub foursquare_category_id: Option<i64>,
    pub foursquare_category_v2_id: Option<String>,
    pub user_category: Option<String>,
    /// Derived on export from the provider categories; upstream ignores it on import.
    pub category: Option<String>,
}

/// One wrapper record from `items/YYYY-MM.json[.gz]`. Exactly one of `visit` and `trip` is
/// present, discriminated by `base.is_visit`.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct TimelineItem {
    pub base: TimelineItemBase,
    pub visit: Option<TimelineItemVisit>,
    pub trip: Option<TimelineItemTrip>,
}

impl TimelineItem {
    pub fn id(&self) -> &str {
        &self.base.id
    }

    pub fn is_visit(&self) -> bool {
        self.base.is_visit
    }

    /// The activity type to show for this item: a user confirmation wins over the classifier.
    /// Visits have none.
    pub fn activity_type(&self) -> Option<ActivityType> {
        let trip = self.trip.as_ref()?;
        trip.confirmed_activity_type
            .or(trip.classified_activity_type)
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TimelineItemBase {
    pub id: String,
    pub is_visit: bool,
    pub start_date: Timestamp,
    pub end_date: Timestamp,
    pub last_saved: Timestamp,
    pub source: Option<String>,
    pub source_version: Option<String>,
    pub disabled: Option<bool>,
    /// Deleted items stay in the export; upstream never drops history.
    pub deleted: Option<bool>,
    pub previous_item_id: Option<String>,
    pub next_item_id: Option<String>,
    pub samples_changed: Option<bool>,
    pub locked: Option<bool>,
    pub step_count: Option<f64>,
    pub floors_ascended: Option<f64>,
    pub floors_descended: Option<f64>,
    pub average_altitude: Option<f64>,
    pub active_energy_burned: Option<f64>,
    pub average_heart_rate: Option<f64>,
    pub max_heart_rate: Option<f64>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TimelineItemVisit {
    pub item_id: String,
    /// Both coordinates are null together, or both are valid.
    pub latitude: Option<f64>,
    pub longitude: Option<f64>,
    pub radius_mean: Option<f64>,
    #[serde(rename = "radiusSD")]
    pub radius_sd: Option<f64>,
    pub place_id: Option<String>,
    pub confirmed_place: Option<bool>,
    pub uncertain_place: Option<bool>,
    pub custom_title: Option<String>,
    pub street_address: Option<String>,
    /// Undocumented in FORMAT.md but present in real 2.4.0 exports.
    pub locality: Option<String>,
    /// Undocumented in FORMAT.md but present in real 2.4.0 exports.
    pub country_code: Option<String>,
    pub last_saved: Timestamp,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TimelineItemTrip {
    pub item_id: String,
    pub distance: Option<f64>,
    pub speed: Option<f64>,
    pub classified_activity_type: Option<ActivityType>,
    pub confirmed_activity_type: Option<ActivityType>,
    pub uncertain_activity_type: Option<bool>,
    pub last_saved: Timestamp,
}

/// One record from `samples/YYYY-Www.json[.gz]`.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LocomotionSample {
    pub id: String,
    pub date: Timestamp,
    pub last_saved: Timestamp,
    pub source: Option<String>,
    pub source_version: Option<String>,
    /// Null for pre-2019 samples that predate timezone capture.
    #[serde(rename = "secondsFromGMT")]
    pub seconds_from_gmt: Option<i32>,
    pub moving_state: Option<MovingState>,
    pub recording_state: Option<RecordingState>,
    pub disabled: Option<bool>,
    pub rtree_id: Option<i64>,
    pub timeline_item_id: Option<String>,
    pub latitude: Option<f64>,
    pub longitude: Option<f64>,
    pub altitude: Option<f64>,
    pub horizontal_accuracy: Option<f64>,
    pub vertical_accuracy: Option<f64>,
    pub speed: Option<f64>,
    pub course: Option<f64>,
    pub step_hz: Option<f64>,
    pub xy_acceleration: Option<f64>,
    pub z_acceleration: Option<f64>,
    pub heart_rate: Option<f64>,
    pub classified_activity_type: Option<ActivityType>,
    pub confirmed_activity_type: Option<ActivityType>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn visit_item(json: &str) -> TimelineItem {
        serde_json::from_str(json).unwrap()
    }

    #[test]
    fn parses_a_visit_and_reports_no_activity_type() {
        let item = visit_item(
            r#"{
                "base": {
                    "id": "B1000000-0000-4000-8000-000000000101",
                    "isVisit": true,
                    "startDate": "2025-06-10T08:00:00Z",
                    "endDate": "2025-06-10T08:20:00Z",
                    "lastSaved": "2025-06-10T09:30:00Z",
                    "somethingArcAddedLater": 7
                },
                "visit": {
                    "itemId": "B1000000-0000-4000-8000-000000000101",
                    "latitude": 51.0403,
                    "longitude": 13.732,
                    "placeId": null,
                    "lastSaved": "2025-06-10T09:30:00Z"
                }
            }"#,
        );

        assert!(item.is_visit());
        assert_eq!(item.id(), "B1000000-0000-4000-8000-000000000101");
        assert_eq!(item.activity_type(), None);
        assert_eq!(
            item.base.start_date,
            "2025-06-10T08:00:00Z".parse::<Timestamp>().unwrap()
        );
        assert!(item.trip.is_none());
    }

    #[test]
    fn confirmed_activity_type_wins_over_classified() {
        let trip = |confirmed: &str| {
            visit_item(&format!(
                r#"{{
                    "base": {{
                        "id": "B2000000-0000-4000-8000-000000000102",
                        "isVisit": false,
                        "startDate": "2025-06-10T08:20:00Z",
                        "endDate": "2025-06-10T08:28:00Z",
                        "lastSaved": "2025-06-10T09:30:00Z"
                    }},
                    "trip": {{
                        "itemId": "B2000000-0000-4000-8000-000000000102",
                        "classifiedActivityType": 5,
                        "confirmedActivityType": {confirmed},
                        "lastSaved": "2025-06-10T09:30:00Z"
                    }}
                }}"#
            ))
        };

        assert_eq!(trip("24").activity_type(), Some(ActivityType::Tram));
        assert_eq!(trip("null").activity_type(), Some(ActivityType::Car));
        assert_eq!(trip("99").activity_type(), Some(ActivityType::Other(99)));
    }

    #[test]
    fn metadata_tolerates_missing_optionals_and_extra_fields() {
        let metadata: Metadata =
            serde_json::from_str(r#"{ "schemaVersion": "2.2.0", "somethingNew": { "a": 1 } }"#)
                .unwrap();

        assert_eq!(metadata.schema_version, "2.2.0");
        assert!(metadata.stats.is_none());
        assert!(metadata.session_finish_date.is_none());
        assert!(metadata.app_metadata.is_none());
    }
}
