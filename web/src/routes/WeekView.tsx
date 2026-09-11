/** Seven days side by side, each a 24 hour strip of what happened when. */

import { useMemo } from "react";
import { Link, useParams } from "react-router";
import { ApiError, api, type Day, errorMessage } from "../api";
import Rail from "../components/Rail";
import TopBar from "../components/TopBar";
import { useRangeGeoJson, useResource } from "../hooks";
import { activityColor, activityLabel } from "../lib/activity";
import { unionBbox } from "../lib/bbox";
import { addWeeks, isoWeekDates, isoWeekOf, isWeek, today } from "../lib/dates";
import {
  formatDistance,
  formatDuration,
  formatTime,
  formatWeekdayShort,
  formatWeekTitle,
  minutesFromMidnight,
} from "../lib/format";
import { useScene } from "../scene";

/** The map is context at the scale of a week, so the tracks are smoothed to stay legible. */
const SIMPLIFY_M = 15;

const HOUR_LABELS = [0, 6, 12, 18];
const GRIDLINES = [6, 12, 18];
const MINUTES_IN_DAY = 1440;

export default function WeekView() {
  const { week = "" } = useParams();
  const valid = isWeek(week);
  const dates = useMemo(() => (valid ? isoWeekDates(week) : []), [valid, week]);

  const days = useResource<(Day | null)[]>(`week:${week}`, () =>
    Promise.all(dates.map((date) => api.day(date).catch(missingIsNull))),
  );

  const geojson = useRangeGeoJson(dates[0] ?? null, dates[6] ?? null, SIMPLIFY_M);

  const scene = useMemo(
    () => ({
      geojson,
      pins: [],
      fit: unionBbox((days.data ?? []).map((day) => day?.summary.bbox)),
    }),
    [geojson, days.data],
  );
  useScene(scene);

  if (!valid) {
    return (
      <Rail wide>
        <p className="notice">
          "{week}" is not an ISO week. Try <Link to={`/week/${isoWeekOf(today())}`}>this week</Link>
          .
        </p>
      </Rail>
    );
  }

  return (
    <>
      <TopBar
        title={formatWeekTitle(week)}
        previous={{ to: `/week/${addWeeks(week, -1)}`, label: "Previous week" }}
        next={{ to: `/week/${addWeeks(week, 1)}`, label: "Next week" }}
      >
        <Link to={`/week/${isoWeekOf(today())}`}>This week</Link>
        <Link to={`/day/${today()}`}>Today</Link>
        <Link to="/heat">Heatmap</Link>
        <Link to="/highlights">Highlights</Link>
      </TopBar>
      <Rail wide>
        {days.error ? (
          <p className="notice notice-error">{errorMessage(days.error)}</p>
        ) : (
          <div className="week">
            <div className="week-hours" aria-hidden="true">
              {HOUR_LABELS.map((hour) => (
                <span key={hour} className="week-hour" style={{ top: `${(hour / 24) * 100}%` }}>
                  {String(hour).padStart(2, "0")}
                </span>
              ))}
            </div>
            {dates.map((date, index) => (
              <DayColumn
                key={date}
                date={date}
                day={days.data?.[index] ?? null}
                column={index + 2}
              />
            ))}
          </div>
        )}
      </Rail>
    </>
  );
}

/** Head and strip are placed on the parent grid directly, so all seven strips share a baseline. */
function DayColumn({ date, day, column }: { date: string; day: Day | null; column: number }) {
  return (
    <>
      <Link className="week-day-head" style={{ gridColumn: column }} to={`/day/${date}`}>
        <span className="week-day-name">{formatWeekdayShort(date)}</span>
        <span className="week-day-number">{Number(date.slice(8, 10))}</span>
        <span className="week-day-distance">
          {day ? formatDistance(day.summary.distanceM) : ""}
        </span>
      </Link>
      <div className="week-strip" style={{ gridColumn: column }}>
        {GRIDLINES.map((hour) => (
          <span key={hour} className="week-gridline" style={{ top: `${(hour / 24) * 100}%` }} />
        ))}
        {(day?.items ?? []).map((item) => (
          <Block key={item.id} date={date} item={item} offset={day?.summary.utcOffsetSeconds} />
        ))}
      </div>
    </>
  );
}

/**
 * An item's block, clipped to the day it is drawn under. An item that started yesterday starts at
 * the top of this column; one that runs past midnight ends at the bottom.
 */
function Block({
  date,
  item,
  offset,
}: {
  date: string;
  item: Day["items"][number];
  offset: number | null | undefined;
}) {
  const start = minutesFromMidnight(item.startDate, item.startOffsetSeconds ?? offset, date);
  const end = minutesFromMidnight(item.endDate, item.endOffsetSeconds ?? offset, date);
  if (start === null || end === null) return null;
  const top = Math.max(0, Math.min(MINUTES_IN_DAY, start));
  const bottom = Math.max(0, Math.min(MINUTES_IN_DAY, end));
  // A two-minute visit still has to be visible, so every block gets a floor.
  const height = Math.max(bottom - top, 3);
  const visit = item.kind === "visit";
  const label = visit
    ? (item.place?.name ?? item.customTitle ?? "Visit")
    : activityLabel(item.activityType);

  return (
    <span
      className={`block${visit ? " block-visit" : ""}${item.confirmed ? "" : " block-open"}`}
      style={
        {
          top: `${(top / MINUTES_IN_DAY) * 100}%`,
          height: `${(height / MINUTES_IN_DAY) * 100}%`,
          "--block": activityColor(visit ? "stationary" : item.activityType),
        } as never
      }
      title={`${formatTime(item.startDate, item.startOffsetSeconds ?? offset)} ${label}, ${formatDuration(item.clippedSeconds ?? item.durationSeconds)}`}
    />
  );
}

function missingIsNull(error: unknown): null {
  if (error instanceof ApiError && error.notFound) return null;
  throw error;
}
