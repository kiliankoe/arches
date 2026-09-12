/**
 * How the heatmap is painted: one hue family, and a scale that survives its own long tail.
 *
 * Weights are wildly skewed. Counting days over three years, the cell holding a desk is 900 and
 * the cell holding a street walked once is 1, and counting samples is worse. Normalising
 * linearly would leave everything but home invisible, so the weight goes through a logarithm
 * first: that street comes out at a tenth of full heat instead of a thousandth, which is the
 * difference between a map of routes and a map of one bright dot. A square root was the other
 * candidate and is not aggressive enough at this range.
 */

/**
 * Magma: transparent, dark purple, magenta, red, orange, yellow. The heatmap view renders on a
 * dark basemap, where a ramp that climbs in luminance as well as hue is what reads; the
 * familiar Strava look is this ramp for the same reason.
 */
const STOPS: [number, string][] = [
  [0, "rgba(59, 15, 112, 0)"],
  [0.1, "rgba(59, 15, 112, 0.6)"],
  [0.3, "#8c2981"],
  [0.5, "#de4968"],
  [0.7, "#fe9f6d"],
  [1, "#fcfdbf"],
];

/** The MapLibre `heatmap-color` ramp, interpolated over the layer's own density. */
export const HEAT_RAMP: unknown[] = [
  "interpolate",
  ["linear"],
  ["heatmap-density"],
  ...STOPS.flat(),
];

/**
 * `heatmap-weight` as a 0..1 fraction of the hottest cell in the response, log scaled. The
 * response's own maximum is the scale, so the picture does not change meaning when a cooler
 * corner of the map is panned into view on its own.
 */
export function heatWeightExpression(maxWeight: number): unknown[] {
  const ceiling = Math.log(1 + Math.max(1, maxWeight));
  return ["min", 1, ["/", ["ln", ["+", 1, ["get", "weight"]]], ceiling]];
}

/**
 * Ground resolution of web mercator at the equator and zoom 0, metres per pixel. MapLibre's
 * world is one 512 px tile at zoom 0, not the 256 px of older web maps; the 256 px figure
 * halves every size below and the grid shows through the blur.
 */
const EQUATOR_METRES_PER_PIXEL = 78271.51696402048;

/**
 * Kernel radius as a multiple of the spacing between cells. MapLibre's kernel is a Gaussian
 * with sigma at a third of the radius, so three puts sigma at one cell spacing, which is where
 * the grid stops showing through: at half that every cell is its own bead.
 */
const RADIUS_PER_CELL = 3;

/**
 * `heatmap-intensity`. With sigma a whole cell wide each kernel overlaps its neighbours', and a
 * filled area sums to about two and a half times its weight. At half intensity a wide, busy
 * area still reaches the top of the ramp while a single street walked daily stays short of it.
 */
export const HEAT_INTENSITY = 0.5;

/**
 * `heatmap-radius` for one response. The server bins to a cell size it reports in metres, and
 * the radius has to track how many screen pixels that is: fixed in pixels, it blends at one zoom
 * and shows a grid of blobs at the next. Anchored at the zoom the data was fetched for, it then
 * doubles per zoom level in either direction, exactly as the cells spread apart or crowd
 * together on screen, until the next fetch.
 */
export function heatRadiusExpression(
  cellMetres: number,
  zoom: number,
  latitude: number,
): unknown[] {
  const metresPerPixel =
    (EQUATOR_METRES_PER_PIXEL * Math.cos((latitude * Math.PI) / 180)) / 2 ** zoom;
  const radius = (cellMetres / metresPerPixel) * RADIUS_PER_CELL;
  // Base-2 exponential interpolation between the ends of the zoom range is r * 2^(z - zoom).
  return [
    "interpolate",
    ["exponential", 2],
    ["zoom"],
    0,
    radius / 2 ** zoom,
    24,
    radius * 2 ** (24 - zoom),
  ];
}

/** The same ramp as a CSS gradient, so the legend swatch cannot drift from the map. */
export function heatLegendGradient(): string {
  const stops = STOPS.map(([at, color]) => `${color} ${Math.round(at * 100)}%`);
  return `linear-gradient(to right, ${stops.join(", ")})`;
}
