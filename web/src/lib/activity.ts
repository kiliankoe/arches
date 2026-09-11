/**
 * The one place activity colours are decided.
 *
 * Nine hues, evenly spread and all dark enough to read as a 4 px line over OpenFreeMap's pale
 * basemap: pastels vanish over parks and neon fights the roads. The map, the timeline, the
 * calendar bars and the week strips all read from here, so a colour means the same thing
 * everywhere. Confirmation state is deliberately not a colour: an unconfirmed item is dashed,
 * which leaves the palette to mean activity and nothing else.
 */

/** The activity types worth their own hue. Everything else falls back to `other`. */
export const ACTIVITY_KEYS = [
  "walking",
  "running",
  "cycling",
  "car",
  "bus",
  "train",
  "tram",
  "airplane",
  "other",
] as const;

export type ActivityKey = (typeof ACTIVITY_KEYS)[number];

export const ACTIVITY_COLORS: Record<ActivityKey, string> = {
  walking: "#3c7a1e",
  running: "#a92e72",
  cycling: "#0e7b72",
  car: "#be3a2e",
  bus: "#b0690c",
  train: "#2b4ca8",
  tram: "#7040a6",
  airplane: "#1b6c93",
  other: "#7a7420",
};

/** Visits are not an activity; they are ink, the same colour as the text. */
export const VISIT_COLOR = "#1b2230";

/**
 * Arc's enum names that do not deserve a hue of their own. `stationary` is what a visit is made
 * of, and the rest are either noise or a type never recorded here.
 */
const INK_TYPES = new Set(["stationary", "unknown", "bogus"]);

export function activityKey(type: string | null | undefined): ActivityKey {
  if (!type) return "other";
  return (ACTIVITY_KEYS as readonly string[]).includes(type) ? (type as ActivityKey) : "other";
}

export function activityColor(type: string | null | undefined): string {
  if (type && INK_TYPES.has(type)) return VISIT_COLOR;
  return ACTIVITY_COLORS[activityKey(type)];
}

/** Sentence case, and Arc's camelCase names split apart: `publicTransport` reads as two words. */
export function activityLabel(type: string | null | undefined): string {
  if (!type) return "Trip";
  const words = type.replace(/([a-z])([A-Z])/g, "$1 $2").toLowerCase();
  return words.charAt(0).toUpperCase() + words.slice(1);
}

/**
 * A MapLibre `match` expression over the feature's `activityType`, so the palette lives in one
 * module rather than being restated in the style.
 */
export function activityMatchExpression(): unknown[] {
  const cases = ACTIVITY_KEYS.filter((key) => key !== "other").flatMap((key) => [
    key,
    ACTIVITY_COLORS[key],
  ]);
  return ["match", ["get", "activityType"], ...cases, ACTIVITY_COLORS.other];
}

/** The palette as CSS custom properties, so stylesheets and TS cannot drift apart. */
export function paletteCssVariables(): Record<string, string> {
  const variables: Record<string, string> = { "--visit": VISIT_COLOR };
  for (const key of ACTIVITY_KEYS) variables[`--activity-${key}`] = ACTIVITY_COLORS[key];
  return variables;
}

export function applyPalette(element: HTMLElement): void {
  for (const [name, value] of Object.entries(paletteCssVariables())) {
    element.style.setProperty(name, value);
  }
}
