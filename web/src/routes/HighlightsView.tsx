/** The notable events of a range of days: first times, flights and the longest trips. */

import { useMemo } from "react";
import { Link, useSearchParams } from "react-router";
import { api, errorMessage, type Highlight } from "../api";
import Rail from "../components/Rail";
import TopBar from "../components/TopBar";
import { useResource } from "../hooks";
import { activityColor } from "../lib/activity";
import { unionBbox } from "../lib/bbox";
import { parseDate, today } from "../lib/dates";
import { formatCount, formatDistance, formatMonthTitle, formatWeekdayShort } from "../lib/format";
import {
  byMonth,
  HIGHLIGHT_PRESETS,
  highlightSearch,
  parseHighlightRange,
} from "../lib/highlights";
import { type DateRange, matchingPreset, presetRange } from "../lib/range";
import { useScene } from "../scene";

export default function HighlightsView() {
  const [search, setSearch] = useSearchParams();
  const now = today();
  // Keyed on the rendered query: a fresh `URLSearchParams` per render would refetch every time.
  const query = search.toString();
  // biome-ignore lint/correctness/useExhaustiveDependencies: the rendered query is the trigger.
  const range = useMemo(() => parseHighlightRange(search, now), [query, now]);
  const { from, to } = range as { from: string; to: string };

  const events = useResource(`highlights:${from}:${to}`, () => api.highlights({ from, to }));
  // The map has no layer of its own here; the days of the range are only what frames it.
  const days = useResource(`highlight-days:${from}:${to}`, () => api.days(from, to));

  const scene = useMemo(
    () => ({
      geojson: null,
      pins: [],
      fit: unionBbox((days.data ?? []).map((day) => day.bbox)),
    }),
    [days.data],
  );
  useScene(scene);

  const update = (next: DateRange) =>
    setSearch(highlightSearch(next).replace(/^\?/, ""), { replace: true });
  const selected = matchingPreset(range, now);
  const months = byMonth(events.data ?? []);

  return (
    <>
      <TopBar title="Highlights" previous={null} next={null}>
        <Link to={`/day/${now}`}>Today</Link>
        <Link to="/heat">Heatmap</Link>
      </TopBar>
      <Rail>
        <div className="heat-rail">
          <div className="heat-presets">
            {HIGHLIGHT_PRESETS.map(({ key, label }) => (
              <button
                key={key}
                type="button"
                className={key === selected ? "chip chip-on" : "chip"}
                aria-pressed={key === selected}
                onClick={() => update(presetRange(key, now))}
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
                value={from}
                max={to}
                onChange={(event) =>
                  event.target.value && update({ ...range, from: event.target.value })
                }
              />
            </label>
            <label>
              <span className="quiet">To</span>
              <input
                type="date"
                className="date-input"
                value={to}
                min={from}
                onChange={(event) =>
                  event.target.value && update({ ...range, to: event.target.value })
                }
              />
            </label>
          </div>

          <p className="figures">
            {events.error ? (
              <span className="figure-quiet notice-error">{errorMessage(events.error)}</span>
            ) : (
              <span className="figure-quiet">
                {events.loading ? "Loading…" : formatCount(events.data?.length ?? 0, "event")}
              </span>
            )}
          </p>
        </div>

        {months.map(({ month, events: inMonth }) => (
          <section key={month}>
            <h2 className="highlight-month">{formatMonthTitle(month)}</h2>
            <ol className="timeline">
              {inMonth.map((event) => (
                <HighlightRow key={`${event.kind}:${event.date}:${event.title}`} event={event} />
              ))}
            </ol>
          </section>
        ))}
        {!events.loading && !events.error && months.length === 0 ? (
          <p className="notice">Nothing worth reporting in this range.</p>
        ) : null}
      </Rail>
    </>
  );
}

function HighlightRow({ event }: { event: Highlight }) {
  return (
    <li className="item">
      <span
        className={`item-rule${event.confirmed ? "" : " item-rule-open"}`}
        style={{ "--rule": ruleColor(event) } as never}
      />
      <span className="item-time">
        {formatWeekdayShort(event.date)} {parseDate(event.date).day}
      </span>
      <span className="item-main">
        <Link className="item-title" to={`/day/${event.date}`}>
          {event.title}
        </Link>
      </span>
      <span className="item-figure">{formatDistance(event.distanceM)}</span>
    </li>
  );
}

/** The same palette the timeline uses: a first time is ink, a trip is its activity. */
function ruleColor(event: Highlight): string {
  if (event.kind === "flight") return activityColor("airplane");
  if (event.kind === "longest") return activityColor(event.activityType);
  return activityColor("stationary");
}
