//! Reading a directory of daily GPX files, shaped the way Arc's own daily exports are.
//!
//! A file holds `<trk>` and `<wpt>` elements interleaved in chronological order: a trip is a
//! track with one `<trkseg>`, a lowercase `<type>` and a `<name>`, a visit is a waypoint with
//! the place name and the start time of the visit. Files are cut on UTC midnight, so an element
//! can continue in the next file; see README.md for what that means for ingest.
//!
//! Everything here opens files read-only. The GPX directory is read-only for arches exactly
//! like Arc's iCloud folder.

mod ids;
mod parse;
mod tz;

use jiff::Timestamp;

pub use ids::deterministic_id;
pub use parse::{creator_of, next_element_time, read};
pub use tz::Timezones;

/// One `<trkpt>`: a fix on a trip.
#[derive(Debug, Clone, PartialEq)]
pub struct Point {
    pub latitude: f64,
    pub longitude: f64,
    /// `<ele>`, missing on fixes from sources that never recorded altitude.
    pub altitude: Option<f64>,
    pub time: Option<Timestamp>,
}

/// One `<trk>`: a trip, always with a single `<trkseg>` in these files.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Track {
    pub name: Option<String>,
    /// `<type>`, the lowercase activity name. Mapped through `ActivityType` on ingest.
    pub kind: Option<String>,
    pub points: Vec<Point>,
}

/// One `<wpt>`: a visit, timestamped with its start, which may predate the file's own day.
#[derive(Debug, Clone, PartialEq)]
pub struct Waypoint {
    pub latitude: f64,
    pub longitude: f64,
    pub time: Option<Timestamp>,
    pub name: Option<String>,
}

/// Tracks and waypoints in document order, which is chronological order.
#[derive(Debug, Clone, PartialEq)]
pub enum Element {
    Track(Track),
    Waypoint(Waypoint),
}

impl Element {
    /// When the element starts: a waypoint's own time, a track's first fix.
    pub fn time(&self) -> Option<Timestamp> {
        match self {
            Element::Track(track) => track.points.iter().find_map(|point| point.time),
            Element::Waypoint(waypoint) => waypoint.time,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Document {
    /// The `creator` attribute of `<gpx>`, which is what ingest records as the row `source`.
    pub creator: Option<String>,
    pub elements: Vec<Element>,
}
