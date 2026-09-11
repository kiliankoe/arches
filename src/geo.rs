//! Geometry helpers. Distances are great-circle; Arc's traces are short enough that the
//! ellipsoid correction is far below GPS noise.

use std::f64::consts::PI;

/// IUGG mean earth radius, the usual choice for haversine.
const EARTH_RADIUS_M: f64 = 6_371_008.8;

/// The grid heatmap cells are counted on: web-mercator tile coordinates at zoom 22, which is the
/// pixel grid of zoom 14 tiles and about 6 m per cell at 51 N. Coarser levels are a right shift
/// of the indices, so one fine table serves every zoom.
pub const CELL_ZOOM: u32 = 22;

/// The coarser grids `heatmap_rollup` keeps a copy of the counts on, as shifts from
/// [`CELL_ZOOM`]. Every level from one above the fine grid up to where a level holds barely a
/// thousand rows, so a query always reads exactly the grid it wants: a request served from a
/// level below its own scans several times the rows for the same answer. Together they cost
/// about as many rows again as the fine table. Changing this list needs an `arches derive`.
pub const ROLLUP_SHIFTS: [u32; 14] = [1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14];

/// Where the square web-mercator map is cut off: the projection sends the poles to infinity.
pub const MAX_MERCATOR_LAT: f64 = 85.051_128_779_806_6;

/// WGS84 equatorial circumference, the one web mercator is scaled to.
const EQUATOR_M: f64 = 2.0 * PI * 6_378_137.0;

/// The number of cells across the world at `CELL_ZOOM - shift`.
fn grid(shift: u32) -> f64 {
    (1u64 << (CELL_ZOOM - shift)) as f64
}

/// The [`CELL_ZOOM`] cell a coordinate falls in. Latitudes beyond the mercator cut and
/// longitudes beyond the antimeridian land in the edge cell rather than off the grid.
pub fn cell_of(latitude: f64, longitude: f64) -> (i64, i64) {
    let n = grid(0);
    let latitude = latitude
        .clamp(-MAX_MERCATOR_LAT, MAX_MERCATOR_LAT)
        .to_radians();
    let x = (longitude.clamp(-180.0, 180.0) + 180.0) / 360.0 * n;
    let y = (1.0 - (latitude.tan() + 1.0 / latitude.cos()).ln() / PI) / 2.0 * n;
    let last = n as i64 - 1;
    (
        (x.floor() as i64).clamp(0, last),
        (y.floor() as i64).clamp(0, last),
    )
}

/// The `(latitude, longitude)` of a point on the grid `shift` levels coarser than [`CELL_ZOOM`],
/// measured in cells. Whole numbers are cell corners, so `x + 0.5` is a cell centre.
pub fn cell_position(x: f64, y: f64, shift: u32) -> (f64, f64) {
    let n = grid(shift);
    let longitude = x / n * 360.0 - 180.0;
    let latitude = (PI * (1.0 - 2.0 * y / n)).sinh().atan().to_degrees();
    (latitude, longitude)
}

/// The centre of a coarse cell, the point the heatmap plots.
pub fn cell_centre(x: i64, y: i64, shift: u32) -> (f64, f64) {
    cell_position(x as f64 + 0.5, y as f64 + 0.5, shift)
}

/// How wide a coarse cell is on the ground at a latitude. Mercator stretches east-west away
/// from the equator, so a cell is only square in projected space.
pub fn cell_metres(shift: u32, latitude: f64) -> f64 {
    EQUATOR_M * latitude.to_radians().cos() / grid(shift)
}

/// Great-circle distance in metres between two WGS84 coordinates.
pub fn haversine_m(lat1: f64, lon1: f64, lat2: f64, lon2: f64) -> f64 {
    let (phi1, phi2) = (lat1.to_radians(), lat2.to_radians());
    let delta_phi = phi2 - phi1;
    let delta_lambda = (lon2 - lon1).to_radians();

    let a = (delta_phi / 2.0).sin().powi(2)
        + phi1.cos() * phi2.cos() * (delta_lambda / 2.0).sin().powi(2);
    2.0 * EARTH_RADIUS_M * a.sqrt().clamp(-1.0, 1.0).asin()
}

/// Ramer-Douglas-Peucker over a chain of `(latitude, longitude)` points, returning the indices
/// to keep in order. The first and last point are always kept.
///
/// Distances are measured on a local equirectangular projection centred on the chain, so the
/// tolerance is in metres and a degree of longitude is not treated as a degree of latitude.
/// A single trace never spans enough of the globe for the projection's error to matter.
pub fn simplify_indices(points: &[(f64, f64)], tolerance_m: f64) -> Vec<usize> {
    if points.len() < 3 || tolerance_m <= 0.0 || tolerance_m.is_nan() {
        return (0..points.len()).collect();
    }

    let centre_lat = points.iter().map(|(lat, _)| lat).sum::<f64>() / points.len() as f64;
    let centre_lon = points.iter().map(|(_, lon)| lon).sum::<f64>() / points.len() as f64;
    let scale_x = EARTH_RADIUS_M * centre_lat.to_radians().cos();
    let projected: Vec<(f64, f64)> = points
        .iter()
        .map(|(lat, lon)| {
            (
                (lon - centre_lon).to_radians() * scale_x,
                (lat - centre_lat).to_radians() * EARTH_RADIUS_M,
            )
        })
        .collect();

    let mut keep = vec![false; points.len()];
    keep[0] = true;
    keep[points.len() - 1] = true;
    // Iterative rather than recursive: a day's trace can be tens of thousands of fixes long and
    // a nearly straight one recurses once per point.
    let mut stack = vec![(0, points.len() - 1)];
    while let Some((first, last)) = stack.pop() {
        let mut worst = (0usize, 0.0);
        for (index, point) in projected.iter().enumerate().take(last).skip(first + 1) {
            let distance = perpendicular_m(*point, projected[first], projected[last]);
            if distance > worst.1 {
                worst = (index, distance);
            }
        }
        if worst.1 > tolerance_m {
            keep[worst.0] = true;
            stack.push((first, worst.0));
            stack.push((worst.0, last));
        }
    }

    keep.iter()
        .enumerate()
        .filter_map(|(index, keep)| keep.then_some(index))
        .collect()
}

/// Distance from `point` to the segment `from`..`to`, all in projected metres.
fn perpendicular_m(point: (f64, f64), from: (f64, f64), to: (f64, f64)) -> f64 {
    let (dx, dy) = (to.0 - from.0, to.1 - from.1);
    let length_squared = dx * dx + dy * dy;
    if length_squared == 0.0 {
        return ((point.0 - from.0).powi(2) + (point.1 - from.1).powi(2)).sqrt();
    }
    // Project onto the segment, clamped to its ends so a point beyond them measures to the end.
    let t = (((point.0 - from.0) * dx + (point.1 - from.1) * dy) / length_squared).clamp(0.0, 1.0);
    let (nearest_x, nearest_y) = (from.0 + t * dx, from.1 + t * dy);
    ((point.0 - nearest_x).powi(2) + (point.1 - nearest_y).powi(2)).sqrt()
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

    /// Hauptbahnhof to Postplatz with a detour: the straight-line points in between are noise
    /// at a 100 m tolerance, the detour is not.
    #[test]
    fn simplify_drops_collinear_points_and_keeps_the_corners() {
        let straight = [
            (51.0403, 13.7320),
            (51.0427, 13.7323),
            (51.0451, 13.7327),
            (51.0475, 13.7330),
            (51.0499, 13.7333),
        ];
        assert_eq!(simplify_indices(&straight, 100.0), [0, 4]);

        let mut with_detour = straight.to_vec();
        // A kilometre east, well outside the tolerance.
        with_detour[2] = (51.0451, 13.7470);
        assert_eq!(simplify_indices(&with_detour, 600.0), [0, 2, 4]);
        assert!(simplify_indices(&with_detour, 100.0).contains(&2));
    }

    #[test]
    fn simplify_keeps_the_endpoints_and_short_or_untolerated_chains() {
        let points = [(51.0403, 13.7320), (51.0451, 13.7327), (51.0499, 13.7333)];
        let kept = simplify_indices(&points, 1_000.0);
        assert_eq!(kept.first(), Some(&0));
        assert_eq!(kept.last(), Some(&2));

        // Nothing to simplify, and a tolerance of zero means "leave it alone".
        assert_eq!(simplify_indices(&points[..2], 1_000.0), [0, 1]);
        assert_eq!(simplify_indices(&points, 0.0), [0, 1, 2]);
        assert!(simplify_indices(&[], 10.0).is_empty());
    }

    /// Longitude degrees are shorter than latitude degrees at 51 N, and the projection has to
    /// know it: 0.01 degrees east is about 700 m, not 1.1 km.
    #[test]
    fn simplify_measures_longitude_in_metres() {
        let points = [(51.0, 13.0), (51.0, 13.01), (51.0, 13.02)];
        let bulge = [(51.0, 13.0), (51.0045, 13.01), (51.0, 13.02)];
        assert_eq!(simplify_indices(&points, 10.0), [0, 2]);
        // 0.0045 degrees of latitude is 500 m, so it survives a 400 m tolerance.
        assert_eq!(simplify_indices(&bulge, 400.0), [0, 1, 2]);
        assert_eq!(simplify_indices(&bulge, 600.0), [0, 2]);
    }

    /// Null island is the middle of the square map, which pins both axes at once.
    #[test]
    fn the_grid_is_centred_on_lon_zero_lat_zero() {
        let middle = 1i64 << (CELL_ZOOM - 1);
        assert_eq!(cell_of(0.0, 0.0), (middle, middle));

        let (latitude, longitude) = cell_position(middle as f64, middle as f64, 0);
        assert!(latitude.abs() < 1e-9 && longitude.abs() < 1e-9);
    }

    #[test]
    fn cells_round_trip_within_their_own_width() {
        for (latitude, longitude) in [
            (51.0499, 13.7333),
            (-33.8688, 151.2093),
            (0.0, -179.9),
            (64.1466, -21.9426),
        ] {
            let (x, y) = cell_of(latitude, longitude);
            let (back_lat, back_lon) = cell_centre(x, y, 0);
            let width = cell_metres(0, latitude);
            assert!(
                haversine_m(latitude, longitude, back_lat, back_lon) < width,
                "{latitude},{longitude} round tripped to {back_lat},{back_lon}"
            );
        }
    }

    /// The x index doubles per level, so shifting an index right is the same as binning at the
    /// coarser level, and the coarse cell still contains the point it came from.
    #[test]
    fn shifting_coarsens_without_moving_the_point() {
        let (x, y) = cell_of(51.0499, 13.7333);
        assert_eq!((x, y), (2_257_156, 1_403_234));

        for shift in [4, 8, 14] {
            let (coarse_x, coarse_y) = (x >> shift, y >> shift);
            let (top_lat, left_lon) = cell_position(coarse_x as f64, coarse_y as f64, shift);
            let (bottom_lat, right_lon) =
                cell_position(coarse_x as f64 + 1.0, coarse_y as f64 + 1.0, shift);
            assert!((left_lon..right_lon).contains(&13.7333), "shift {shift}");
            assert!((bottom_lat..top_lat).contains(&51.0499), "shift {shift}");
        }
    }

    /// Near the poles and past the antimeridian, a coordinate has to land in an edge cell
    /// rather than one step off the grid.
    #[test]
    fn coordinates_outside_the_projection_clamp_onto_it() {
        let last = (1i64 << CELL_ZOOM) - 1;
        assert_eq!(cell_of(89.9, 180.0), (last, 0));
        assert_eq!(cell_of(-89.9, -180.0), (0, last));
        assert_eq!(cell_of(MAX_MERCATOR_LAT, 0.0).1, 0);
    }

    #[test]
    fn cell_width_matches_the_mercator_scale() {
        // 40 075 km round the equator, halved by the cosine at 60 N.
        assert!((cell_metres(CELL_ZOOM, 0.0) - 40_075_016.7).abs() < 1.0);
        assert!((cell_metres(0, 51.0) - 6.013).abs() < 0.01);
        assert!((cell_metres(0, 60.0) / cell_metres(0, 0.0) - 0.5).abs() < 1e-6);
    }
}
