/** The one data-loading hook the views share: fetch on key change, cancel on the way out. */

import { useCallback, useEffect, useRef, useState } from "react";

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
