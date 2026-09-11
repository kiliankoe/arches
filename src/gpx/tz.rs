//! UTC offsets for GPX fixes.
//!
//! GPX timestamps are all UTC, but every day in this database is a local day, so each fix needs
//! the offset that was in force where and when it was recorded. The coordinate gives an IANA
//! zone (tzf-rs, bundled boundary data, no network) and the zone gives the offset at that
//! instant (jiff, system tzdb), which is what makes a 2016 summer day come out at +2 and the
//! winter either side of it at +1.

use std::collections::HashMap;

use jiff::Timestamp;
use jiff::tz::TimeZone;
use tzf_rs::DefaultFinder;

/// Cache key resolution: two decimals of a degree, about 1 km. A day's fixes are almost all in
/// one zone, and a zone boundary is never decided at that scale anyway.
const CELLS_PER_DEGREE: f64 = 100.0;

pub struct Timezones {
    finder: DefaultFinder,
    by_cell: HashMap<(i32, i32), Option<TimeZone>>,
    /// A missing tzdb would otherwise log once per fix.
    warned: bool,
}

impl Timezones {
    pub fn new() -> Self {
        Self {
            finder: DefaultFinder::new(),
            by_cell: HashMap::new(),
            warned: false,
        }
    }

    /// The offset in seconds east of UTC at a coordinate and an instant. Falls back to UTC for
    /// a point no zone covers, which is the open sea, and says so once.
    pub fn offset_seconds(&mut self, latitude: f64, longitude: f64, at: Timestamp) -> i32 {
        let cell = (
            (latitude * CELLS_PER_DEGREE).round() as i32,
            (longitude * CELLS_PER_DEGREE).round() as i32,
        );
        let finder = &self.finder;
        let zone = self.by_cell.entry(cell).or_insert_with(|| {
            let name = finder.get_tz_name(longitude, latitude);
            if name.is_empty() {
                return None;
            }
            TimeZone::get(name).ok()
        });

        match zone {
            Some(zone) => zone.to_offset(at).seconds(),
            None => {
                if !self.warned {
                    self.warned = true;
                    tracing::warn!(
                        latitude,
                        longitude,
                        "no timezone for a fix, assuming UTC (is the system tzdb readable?)"
                    );
                }
                0
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(text: &str) -> Timestamp {
        text.parse().unwrap()
    }

    #[test]
    fn dresden_follows_central_european_summer_time() {
        let mut zones = Timezones::new();
        let (latitude, longitude) = (51.0499, 13.7333);

        assert_eq!(
            zones.offset_seconds(latitude, longitude, at("2016-02-18T12:00:00Z")),
            3600
        );
        assert_eq!(
            zones.offset_seconds(latitude, longitude, at("2016-07-18T12:00:00Z")),
            7200
        );
        // Same cell, so the second call came out of the cache.
        assert_eq!(zones.by_cell.len(), 1);
    }

    #[test]
    fn other_zones_and_the_open_sea() {
        let mut zones = Timezones::new();

        // Lisboa is an hour behind Dresden, and Reykjavik never leaves UTC.
        assert_eq!(
            zones.offset_seconds(38.7223, -9.1393, at("2016-07-18T12:00:00Z")),
            3600
        );
        assert_eq!(
            zones.offset_seconds(64.1466, -21.9426, at("2016-07-18T12:00:00Z")),
            0
        );
        // The open sea is covered too, by the nautical `Etc/GMT` zones, which is why the UTC
        // fallback above is only ever reached when the system tzdb cannot be read at all.
        assert_eq!(
            zones.offset_seconds(-40.0, -30.0, at("2016-07-18T12:00:00Z")),
            -7200
        );
    }
}
