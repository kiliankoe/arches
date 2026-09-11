/** A month as a calendar of days, each one a distance bar broken down by how it was covered. */

import { useMemo } from "react";
import { Link, useParams } from "react-router";
import { api, type DaySummary, errorMessage } from "../api";
import Rail from "../components/Rail";
import TopBar from "../components/TopBar";
import { useResource } from "../hooks";
import { activityColor } from "../lib/activity";
import { unionBbox } from "../lib/bbox";
import { addMonths, isMonth, monthEnd, monthGrid, monthOf, monthStart, today } from "../lib/dates";
import { formatDistance, formatMonthTitle } from "../lib/format";
import { useScene } from "../scene";

const WEEKDAYS = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];

export default function MonthView() {
  const { month = "" } = useParams();
  const valid = isMonth(month);

  const days = useResource(`month:${month}`, () =>
    valid ? api.days(monthStart(month), monthEnd(month)) : Promise.resolve([]),
  );

  const byDate = useMemo(() => {
    const map = new Map<string, DaySummary>();
    for (const day of days.data ?? []) map.set(day.date, day);
    return map;
  }, [days.data]);

  const scene = useMemo(
    () => ({
      geojson: null,
      pins: [],
      fit: unionBbox((days.data ?? []).map((day) => day.bbox)),
    }),
    [days.data],
  );
  useScene(scene);

  if (!valid) {
    return (
      <Rail wide>
        <p className="notice">
          "{month}" is not a month. Try <Link to={`/month/${monthOf(today())}`}>this month</Link>.
        </p>
      </Rail>
    );
  }

  const grid = monthGrid(month);
  const total = (days.data ?? []).reduce((sum, day) => sum + day.distanceM, 0);

  return (
    <>
      <TopBar
        title={formatMonthTitle(month)}
        previous={{
          to: `/month/${addMonths(month, -1)}`,
          label: "Previous month",
        }}
        next={{ to: `/month/${addMonths(month, 1)}`, label: "Next month" }}
      >
        <Link to={`/month/${monthOf(today())}`}>This month</Link>
        <Link to={`/day/${today()}`}>Today</Link>
      </TopBar>
      <Rail wide>
        {days.error ? (
          <p className="notice notice-error">{errorMessage(days.error)}</p>
        ) : (
          <>
            <p className="figures">
              <span className="figure">{formatDistance(total) || "0 m"}</span>
              <span className="figure figure-quiet">
                {byDate.size} of {grid.flat().filter(Boolean).length} days recorded
              </span>
            </p>
            <table className="calendar">
              <thead>
                <tr>
                  {WEEKDAYS.map((name) => (
                    <th key={name} scope="col">
                      {name}
                    </th>
                  ))}
                </tr>
              </thead>
              <tbody>
                {grid.map((week) => (
                  <tr key={week.find(Boolean) ?? week.join()}>
                    {week.map((date, index) => (
                      <td key={date ?? `pad-${WEEKDAYS[index]}`}>
                        {date ? <Cell date={date} summary={byDate.get(date)} /> : null}
                      </td>
                    ))}
                  </tr>
                ))}
              </tbody>
            </table>
          </>
        )}
      </Rail>
    </>
  );
}

function Cell({ date, summary }: { date: string; summary: DaySummary | undefined }) {
  const number = Number(date.slice(8, 10));
  if (!summary) {
    return (
      <div className="cell cell-empty">
        <span className="cell-day">{number}</span>
      </div>
    );
  }
  return (
    <Link
      className={`cell${summary.confirmed ? "" : " cell-unconfirmed"}`}
      to={`/day/${date}`}
      title={`${date}, ${formatDistance(summary.distanceM)}`}
    >
      <span className="cell-day">{number}</span>
      <DistanceBar distanceByType={summary.distanceByType} total={summary.distanceM} />
      <span className="cell-distance">{formatDistance(summary.distanceM)}</span>
    </Link>
  );
}

/** One thin bar per day, cut into segments in the same colours the map draws the day in. */
function DistanceBar({
  distanceByType,
  total,
}: {
  distanceByType: Record<string, number>;
  total: number;
}) {
  const segments = Object.entries(distanceByType)
    .filter(([, metres]) => metres > 0)
    .sort((a, b) => b[1] - a[1]);
  if (segments.length === 0 || total <= 0) return <span className="bar bar-none" />;
  return (
    <span className="bar">
      {segments.map(([type, metres]) => (
        <span
          key={type}
          className="bar-segment"
          style={{
            width: `${(metres / total) * 100}%`,
            background: activityColor(type),
          }}
        />
      ))}
    </span>
  );
}
