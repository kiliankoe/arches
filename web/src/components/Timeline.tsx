/** The day's items in order, with the header of totals above them. */

import { useEffect, useRef } from "react";
import { Link } from "react-router";
import type { Day, Item } from "../api";
import { activityColor, activityLabel } from "../lib/activity";
import { formatDistance, formatDuration, formatTimeRange, formatUnconfirmed } from "../lib/format";

type Props = {
  day: Day;
  selectedId: string | null;
  onSelect: (id: string | null) => void;
};

export default function Timeline({ day, selectedId, onSelect }: Props) {
  const { summary, items } = day;
  const unconfirmed = formatUnconfirmed(summary.unconfirmedItems, summary.itemCount);

  return (
    <>
      <div className="day-header">
        <p className="figures">
          <span className="figure">{formatDistance(summary.distanceM) || "0 m"}</span>
          <span className="figure figure-quiet">
            {formatDuration(summary.movingSeconds)} moving
          </span>
        </p>
        {unconfirmed ? <p className="quiet">{unconfirmed}</p> : null}
      </div>
      {items.length === 0 ? (
        <p className="notice">Nothing recorded on this day.</p>
      ) : (
        <ol className="timeline">
          {items.map((item) => (
            <TimelineRow
              key={item.id}
              item={item}
              selected={item.id === selectedId}
              onSelect={onSelect}
            />
          ))}
        </ol>
      )}
    </>
  );
}

function TimelineRow({
  item,
  selected,
  onSelect,
}: {
  item: Item;
  selected: boolean;
  onSelect: (id: string | null) => void;
}) {
  const row = useRef<HTMLLIElement>(null);
  const wasSelected = useRef(selected);

  // A click on the map has to bring its row into view; a click on the row must not scroll itself.
  useEffect(() => {
    if (selected && !wasSelected.current) {
      row.current?.scrollIntoView({ block: "nearest" });
    }
    wasSelected.current = selected;
  }, [selected]);

  const visit = item.kind === "visit";
  const title = visit
    ? (item.place?.name ?? item.customTitle ?? "Unnamed place")
    : activityLabel(item.activityType);
  const figure = visit
    ? formatDuration(item.clippedSeconds ?? item.durationSeconds)
    : formatDistance(item.distanceM) || formatDuration(item.clippedSeconds ?? item.durationSeconds);
  const detail = visit ? [item.place?.locality, item.place?.countryCode?.toUpperCase()] : [];

  return (
    // Pointing at a row highlights it, and so does tabbing into it: focus bubbles up here from
    // the title link or button, which is the row's only focusable control.
    <li
      ref={row}
      className={`item${selected ? " item-selected" : ""}`}
      onMouseEnter={() => onSelect(item.id)}
      onFocus={() => onSelect(item.id)}
    >
      <span
        className={`item-rule${item.confirmed ? "" : " item-rule-open"}`}
        style={
          {
            "--rule": activityColor(visit ? "stationary" : item.activityType),
          } as never
        }
      />
      <span className="item-time">
        {formatTimeRange(
          item.startDate,
          item.endDate,
          item.startOffsetSeconds,
          item.endOffsetSeconds,
        )}
      </span>
      <span className="item-main">
        {visit && item.place ? (
          <Link className="item-title" to={`/place/${item.place.id}`}>
            {title}
          </Link>
        ) : (
          <button type="button" className="item-title" onFocus={() => onSelect(item.id)}>
            {title}
          </button>
        )}
        {detail.filter(Boolean).length > 0 ? (
          <span className="item-detail">{detail.filter(Boolean).join(", ")}</span>
        ) : null}
      </span>
      <span className="item-figure">{figure}</span>
    </li>
  );
}
