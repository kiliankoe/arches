/** The data-loading hooks the views share: fetch on key change, cancel on the way out. */

import { useCallback, useEffect, useRef, useState } from "react";
import { api, type DayGeoJson } from "./api";

export type Resource<T> = {
  data: T | null;
  error: unknown;
  loading: boolean;
  reload: () => void;
};

export function useResource<T>(key: string, load: () => Promise<T>): Resource<T> {
  const [data, setData] = useState<T | null>(null);
  const [error, setError] = useState<unknown>(null);
  const [loading, setLoading] = useState(true);
  const [attempt, setAttempt] = useState(0);
  // `load` is a fresh closure every render; the key decides when it is worth calling again.
  const latest = useRef(load);
  latest.current = load;

  // biome-ignore lint/correctness/useExhaustiveDependencies: key and attempt are the triggers, not values the effect reads.
  useEffect(() => {
    let cancelled = false;
    setLoading(true);
    setError(null);
    latest.current().then(
      (loaded) => {
        if (cancelled) return;
        setData(loaded);
        setLoading(false);
      },
      (failure: unknown) => {
        if (cancelled) return;
        setData(null);
        setError(failure);
        setLoading(false);
      },
    );
    return () => {
      cancelled = true;
    };
  }, [key, attempt]);

  const reload = useCallback(() => setAttempt((count) => count + 1), []);
  return { data, error, loading, reload };
}

/**
 * The tracks of a range, for the views that draw one as context behind their own content. The
 * request is aborted when the range changes, so the answer to a week the user has already
 * stepped past never lands on the map. A failure leaves the map empty rather than surfacing:
 * the range's own content is elsewhere, and nothing here is the point of the view.
 */
export function useRangeGeoJson(
  from: string | null,
  to: string | null,
  simplify: number,
): DayGeoJson | null {
  const [geojson, setGeojson] = useState<DayGeoJson | null>(null);

  useEffect(() => {
    setGeojson(null);
    if (!from || !to) return;
    const controller = new AbortController();
    api.rangeGeoJson({ from, to, simplify }, controller.signal).then(
      (loaded) => setGeojson(loaded),
      () => {},
    );
    return () => controller.abort();
  }, [from, to, simplify]);

  return geojson;
}
