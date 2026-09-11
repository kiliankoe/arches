/** Everywhere you have been, over a range of days. The map is the whole view. */

import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { Link, useSearchParams } from "react-router";
import { api, type Bbox, errorMessage, type HeatGeoJson } from "../api";
import type { Viewport } from "../components/MapView";
import Rail from "../components/Rail";
import TopBar from "../components/TopBar";
import { isoWeekOf, monthOf, today } from "../lib/dates";
import { formatCount } from "../lib/format";
import { type HeatWeight, heatSearch, parseHeatParams } from "../lib/heat";
import { heatLegendGradient } from "../lib/heatLayer";
import { type DateRange, matchingPreset, PRESETS, presetRange } from "../lib/range";
import { useScene } from "../scene";

/** Long enough to sit out a flick of the wheel, short enough that a pan feels answered. */
const DEBOUNCE_MS = 250;

export default function HeatView() {
  const [search, setSearch] = useSearchParams();
  // Keyed on the string, not the object: a fresh `URLSearchParams` per render would restart the
  // fetch effect on every render.
  const query = search.toString();
  // biome-ignore lint/correctness/useExhaustiveDependencies: the rendered query is the trigger.
  const { range, weight } = useMemo(() => parseHeatParams(search), [query]);
  const now = today();

  const [view, setView] = useState<Viewport | null>(null);
  const [heat, setHeat] = useState<HeatGeoJson | null>(null);
  const [error, setError] = useState<unknown>(null);
  const timer = useRef<number | null>(null);

  const onViewport = useCallback((next: Viewport) => {
    if (timer.current !== null) window.clearTimeout(timer.current);
    timer.current = window.setTimeout(() => setView(next), DEBOUNCE_MS);
  }, []);
  useEffect(
    () => () => {
      if (timer.current !== null) window.clearTimeout(timer.current);
    },
    [],
  );

  useEffect(() => {
    if (!view) return;
    const controller = new AbortController();
    api.heatmap({ ...view, ...range, weight }, controller.signal).then(
      (loaded) => {
        setHeat(loaded);
        setError(null);
      },
      (failure: unknown) => {
        // A request cancelled by the next pan is not a failure anyone needs to read about.
        if (controller.signal.aborted) return;
        setError(failure);
      },
    );
    return () => controller.abort();
  }, [view, range, weight]);

  // The first answer for a range frames it; after that the map is the user's to move, and
  // refitting on every response would fight the pan that asked for it.
  const rangeKey = `${range.from ?? ""}:${range.to ?? ""}`;
  const [fit, setFit] = useState<{ key: string; bbox: Bbox } | null>(null);
  useEffect(() => {
    const bbox = heat?.meta.bbox;
    if (bbox) setFit((framed) => (framed?.key === rangeKey ? framed : { key: rangeKey, bbox }));
  }, [heat, rangeKey]);

  const scene = useMemo(
    () => ({ geojson: null, heat, pins: [], fit: fit?.bbox ?? null, onViewport }),
    [heat, fit, onViewport],
  );
  useScene(scene);

  const update = (next: DateRange, nextWeight: HeatWeight) =>
    setSearch(heatSearch(next, nextWeight).replace(/^\?/, ""), { replace: true });
  const selected = matchingPreset(range, now);
  const meta = heat?.meta;

  return (
    <>
      <TopBar title="Heatmap" previous={null} next={null}>
        <Link to={`/day/${now}`}>Today</Link>
        <Link to={`/week/${isoWeekOf(now)}`}>Week</Link>
        <Link to={`/month/${monthOf(now)}`}>Month</Link>
        <Link to="/highlights">Highlights</Link>
      </TopBar>
      <Rail>
        <div className="heat-rail">
          <div className="heat-presets">
            {PRESETS.map(({ key, label }) => (
              <button
                key={key}
                type="button"
                className={key === selected ? "chip chip-on" : "chip"}
                aria-pressed={key === selected}
                onClick={() => update(presetRange(key, now), weight)}
              >
                {label}
              </button>
            ))}
          </div>

          <div className="heat-dates">
            <label>
              <span className="quiet">From</span>
              <input
                type="date"
                className="date-input"
                value={range.from ?? ""}
                max={range.to ?? undefined}
                onChange={(event) => update({ ...range, from: event.target.value || null }, weight)}
              />
            </label>
            <label>
              <span className="quiet">To</span>
              <input
                type="date"
                className="date-input"
                value={range.to ?? ""}
                min={range.from ?? undefined}
                onChange={(event) => update({ ...range, to: event.target.value || null }, weight)}
              />
            </label>
          </div>

          <p className="figures">
            {error ? (
              <span className="figure-quiet notice-error">{errorMessage(error)}</span>
            ) : meta ? (
              <span className="figure-quiet">
                {formatCount(meta.days, "day")}, {formatCount(meta.points, "cell")}
              </span>
            ) : (
              <span className="figure-quiet">Loading…</span>
            )}
          </p>

          <div className="heat-presets">
            {(
              [
                ["days", "Count days"],
                ["samples", "Count samples"],
              ] as [HeatWeight, string][]
            ).map(([key, label]) => (
              <button
                key={key}
                type="button"
                className={key === weight ? "chip chip-on" : "chip"}
                aria-pressed={key === weight}
                onClick={() => update(range, key)}
              >
                {label}
              </button>
            ))}
          </div>

          <div className="heat-legend">
            <div className="heat-swatch" style={{ background: heatLegendGradient() }} />
            <div className="heat-legend-ends quiet">
              <span>{weight === "days" ? "once" : "few"}</span>
              <span>
                {meta ? formatCount(meta.maxWeight, weight === "days" ? "day" : "sample") : ""}
              </span>
            </div>
          </div>
        </div>
      </Rail>
    </>
  );
}
