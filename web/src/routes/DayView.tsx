/** A day: its track on the map, its timeline in the rail. */

import { useCallback, useEffect, useMemo } from "react";
import { Link, useNavigate, useParams } from "react-router";
import { ApiError, api, type Day, errorMessage } from "../api";
import Rail from "../components/Rail";
import StatusLine from "../components/StatusLine";
import Timeline from "../components/Timeline";
import TopBar from "../components/TopBar";
import { useResource } from "../hooks";
import { isDate, isoWeekOf, monthOf, today } from "../lib/dates";
import { dayWindow, neighbours } from "../lib/dayIndex";
import { formatDayShort, formatDayTitle } from "../lib/format";
import { useScene, useSelection } from "../scene";

/** Five metres of tolerance drops the GPS jitter without straightening a single corner. */
const SIMPLIFY_M = 5;

export default function DayView() {
  const { date = "" } = useParams();
  const navigate = useNavigate();
  const { selectedId, setSelectedId } = useSelection();

  const valid = isDate(date);
  const day = useResource<Day | null>(`day:${date}`, () =>
    valid ? api.day(date).catch(missingIsNull) : Promise.resolve(null),
  );
  const geojson = useResource(`geojson:${date}`, () =>
    valid ? api.dayGeoJson(date, SIMPLIFY_M).catch(missingIsNull) : Promise.resolve(null),
  );
  const around = useResource(`around:${date}`, () =>
    valid ? dayWindow(date).then((window) => neighbours(window, date)) : Promise.resolve(null),
  );

  const scene = useMemo(
    () => ({
      geojson: geojson.data,
      pins: [],
      fit: day.data?.summary.bbox ?? null,
    }),
    [geojson.data, day.data],
  );
  useScene(scene);

  // A new day deserves a clean slate; a stale highlight would point at an item that is gone.
  // biome-ignore lint/correctness/useExhaustiveDependencies: the date is the reset trigger.
  useEffect(() => setSelectedId(null), [date, setSelectedId]);

  const previous = around.data?.previous ?? null;
  const next = around.data?.next ?? null;
  const step = useCallback(
    (target: string | null) => target && navigate(`/day/${target}`),
    [navigate],
  );

  useEffect(() => {
    function onKey(event: KeyboardEvent) {
      if (event.metaKey || event.ctrlKey || event.altKey) return;
      const target = event.target as HTMLElement | null;
      if (target && /^(INPUT|SELECT|TEXTAREA)$/.test(target.tagName)) return;
      if (event.key === "ArrowLeft") step(previous);
      if (event.key === "ArrowRight") step(next);
    }
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [previous, next, step]);

  if (!valid) {
    return (
      <Rail>
        <p className="notice">
          {date ? `"${date}" is not a date.` : "No date."} Try{" "}
          <Link to={`/day/${today()}`}>today</Link>.
        </p>
      </Rail>
    );
  }

  const failure = day.error ?? geojson.error;

  return (
    <>
      <TopBar
        title={formatDayTitle(date)}
        previous={
          previous
            ? {
                to: `/day/${previous}`,
                label: `Go to ${formatDayShort(previous)}`,
              }
            : null
        }
        next={next ? { to: `/day/${next}`, label: `Go to ${formatDayShort(next)}` } : null}
      >
        <input
          type="date"
          className="date-input"
          value={date}
          aria-label="Jump to a date"
          onChange={(event) => event.target.value && navigate(`/day/${event.target.value}`)}
        />
        <Link to={`/week/${isoWeekOf(date)}`}>Week</Link>
        <Link to={`/month/${monthOf(date)}`}>Month</Link>
        <Link to="/heat">Heatmap</Link>
      </TopBar>
      <Rail footer={<StatusLine onIngested={day.reload} />}>
        {failure ? (
          <p className="notice notice-error">{errorMessage(failure)}</p>
        ) : day.data ? (
          <Timeline day={day.data} selectedId={selectedId} onSelect={setSelectedId} />
        ) : day.loading ? (
          <p className="notice">Loading…</p>
        ) : (
          <p className="notice">No data for this day.</p>
        )}
      </Rail>
    </>
  );
}

/** A day with no recording is a 404 and an ordinary answer, not an error to shout about. */
function missingIsNull(error: unknown): null {
  if (error instanceof ApiError && error.notFound) return null;
  throw error;
}
