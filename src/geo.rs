//! Geometry helpers. Distances are great-circle; Arc's traces are short enough that the
//! ellipsoid correction is far below GPS noise.

/// IUGG mean earth radius, the usual choice for haversine.
const EARTH_RADIUS_M: f64 = 6_371_008.8;

/// Great-circle distance in metres between two WGS84 coordinates.
pub fn haversine_m(lat1: f64, lon1: f64, lat2: f64, lon2: f64) -> f64 {
    let (phi1, phi2) = (lat1.to_radians(), lat2.to_radians());
    let delta_phi = phi2 - phi1;
    let delta_lambda = (lon2 - lon1).to_radians();

    let a = (delta_phi / 2.0).sin().powi(2)
        + phi1.cos() * phi2.cos() * (delta_lambda / 2.0).sin().powi(2);
    2.0 * EARTH_RADIUS_M * a.sqrt().clamp(-1.0, 1.0).asin()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Dresden Hauptbahnhof to Postplatz, about 1.07 km as the crow flies.
    #[test]
    fn matches_a_known_distance() {
        let metres = haversine_m(51.0403, 13.7320, 51.0499, 13.7333);
        assert!(
            (metres - 1070.0).abs() < 20.0,
            "expected about 1070 m, got {metres}"
        );
    }

    #[test]
    fn is_zero_for_the_same_point_and_symmetric() {
        assert_eq!(haversine_m(51.0403, 13.732, 51.0403, 13.732), 0.0);
        let there = haversine_m(51.0403, 13.732, 51.0499, 13.7333);
        let back = haversine_m(51.0499, 13.7333, 51.0403, 13.732);
        assert!((there - back).abs() < 1e-9);
    }

    #[test]
    fn handles_a_long_distance() {
        // Dresden to Berlin Hbf, about 165 km.
        let metres = haversine_m(51.0403, 13.732, 52.5251, 13.3694);
        assert!((metres - 165_800.0).abs() < 2_000.0, "{metres}");
    }
}
