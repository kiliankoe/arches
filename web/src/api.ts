/**
 * The typed client for arches' own API.
 *
 * Types mirror the serde models in `src/api/`: camelCase keys, local `YYYY-MM-DD` dates, RFC 3339
 * timestamps, activity types as their enum names. A missing value is `null` rather than absent,
 * except where the Rust side skips it.
 */

export type Counts = {
  places: number;
  items: number;
  samples: number;
  files: number;
  daySummaries: number;
};

export type IngestRun = {
  id: number;
  startedAt: string | null;
  finishedAt: string | null;
  deviceId: string | null;
  schemaVersion: number;
  lastBackupDate: string | null;
  filesSeen: number;
  filesIngested: number;
  placesUpserted: number;
  itemsUpserted: number;
  samplesUpserted: number;
  daysRecomputed: number;
  error: string | null;
  elapsedMs: number;
};

export type Status = {
  version: string;
  lastRun: IngestRun | null;
  lastBackupDate: string | null;
  newestBucketMtime: string | null;
  counts: Counts;
  firstSummarizedDate: string | null;
  lastSummarizedDate: string | null;
  ingestRunning: boolean;
};

export type Config = { mapStyle: string };

export type Health = {
  stepCount: number | null;
  floorsAscended: number | null;
  floorsDescended: number | null;
  averageAltitude: number | null;
  activeEnergyBurned: number | null;
  averageHeartRate: number | null;
  maxHeartRate: number | null;
};

export type Place = {
  id: string;
  name: string;
  streetAddress: string | null;
  locality: string | null;
  countryCode: string | null;
  latitude: number;
  longitude: number;
  radiusMean: number | null;
  visitCount: number | null;
  visitDays: number | null;
  lastVisitDate: string | null;
  category: string | null;
  isStale: boolean | null;
  /** Only present on `/api/near`. */
  distanceM?: number;
};

export type Item = {
  id: string;
  kind: "visit" | "trip";
  startDate: string | null;
  endDate: string | null;
  localStartDate: string | null;
  localEndDate: string | null;
  startOffsetSeconds: number | null;
  endOffsetSeconds: number | null;
  durationSeconds: number;
  /** Seconds inside the requested day; null outside a day timeline. */
  clippedSeconds: number | null;
  activityType: string | null;
  distanceM: number | null;
  confirmed: boolean;
  uncertain: boolean;
  health: Health;
  latitude: number | null;
  longitude: number | null;
  customTitle: string | null;
  place: Place | null;
};

/** `[minLon, minLat, maxLon, maxLat]`, the GeoJSON order. */
export type Bbox = [number, number, number, number];

export type DaySummary = {
  date: string;
  utcOffsetSeconds: number | null;
  itemCount: number;
  visitCount: number;
  tripCount: number;
  sampleCount: number;
  distanceM: number;
  movingSeconds: number;
  distanceByType: Record<string, number>;
  durationByType: Record<string, number>;
  placeIds: string[];
  countryCodes: string[];
  localities: string[];
  bbox: Bbox | null;
  firstSampleAt: string | null;
  lastSampleAt: string | null;
  unconfirmedItems: number;
  confirmed: boolean;
};

export type Day = { summary: DaySummary; items: Item[] };

export type DayFeature = {
  type: "Feature";
  geometry:
    | { type: "LineString"; coordinates: [number, number][] }
    | { type: "Point"; coordinates: [number, number] };
  properties: {
    itemId: string;
    activityType?: string | null;
    placeId?: string | null;
    name?: string | null;
    startDate: string | null;
    endDate: string | null;
    distanceM?: number | null;
    confirmed: boolean;
    uncertain?: boolean;
  };
};

export type DayGeoJson = { type: "FeatureCollection"; features: DayFeature[] };

/** A 404 is routine here: a day with no recording simply has no row. */
export class ApiError extends Error {
  readonly status: number;

  constructor(status: number, message: string) {
    super(message);
    this.name = "ApiError";
    this.status = status;
  }

  get notFound(): boolean {
    return this.status === 404;
  }
}

async function request<T>(path: string, init?: RequestInit): Promise<T> {
  let response: Response;
  try {
    response = await fetch(`/api${path}`, init);
  } catch {
    // A failed fetch means the server, not the request: say so rather than leaking "Failed to fetch".
    throw new ApiError(0, "Server not reachable at /api. Is arches running?");
  }
  if (!response.ok) {
    const detail = await response
      .json()
      .then((body: { error?: string }) => body.error)
      .catch(() => null);
    throw new ApiError(response.status, detail ?? `${response.status} ${response.statusText}`);
  }
  return (await response.json()) as T;
}

function query(params: Record<string, string | number | undefined>): string {
  const search = new URLSearchParams();
  for (const [key, value] of Object.entries(params)) {
    if (value !== undefined) search.set(key, String(value));
  }
  const rendered = search.toString();
  return rendered ? `?${rendered}` : "";
}

export const api = {
  status: () => request<Status>("/status"),
  config: () => request<Config>("/config"),
  ingest: () => request<IngestRun>("/ingest", { method: "POST" }),
  days: (from: string, to: string) => request<DaySummary[]>(`/days${query({ from, to })}`),
  day: (date: string) => request<Day>(`/days/${date}`),
  dayGeoJson: (date: string, simplify?: number) =>
    request<DayGeoJson>(`/days/${date}/geojson${query({ simplify })}`),
  place: (id: string) => request<Place>(`/places/${encodeURIComponent(id)}`),
  placeVisits: (id: string, limit?: number) =>
    request<Item[]>(`/places/${encodeURIComponent(id)}/visits${query({ limit })}`),
};

export function errorMessage(error: unknown): string {
  if (error instanceof ApiError) return error.message;
  return error instanceof Error ? error.message : String(error);
}
