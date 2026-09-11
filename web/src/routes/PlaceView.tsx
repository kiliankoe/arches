/** One place: where it is, how often it was there, and every visit that led back to it. */

import { useMemo } from "react";
import { Link, useParams } from "react-router";
import { api, errorMessage, type Item } from "../api";
import Rail from "../components/Rail";
import TopBar from "../components/TopBar";
import { useResource } from "../hooks";
import { today } from "../lib/dates";
import { formatCount, formatDayShort, formatDuration, formatTimeRange } from "../lib/format";
import { useScene } from "../scene";

export default function PlaceView() {
  const { id = "" } = useParams();
  const place = useResource(`place:${id}`, () => api.place(id));
  const visits = useResource<Item[]>(`visits:${id}`, () => api.placeVisits(id));

  const scene = useMemo(() => {
    if (!place.data) return { geojson: null, pins: [], fit: null };
    const { longitude, latitude } = place.data;
    return {
      geojson: null,
      pins: [{ id: place.data.id, longitude, latitude, label: place.data.name }],
      // A place is a point, so the box is a small square around it rather than an extent.
      fit: [longitude - 0.0025, latitude - 0.0015, longitude + 0.0025, latitude + 0.0015] as [
        number,
        number,
        number,
        number,
      ],
    };
  }, [place.data]);
  useScene(scene);

  const address = place.data
    ? [place.data.streetAddress, place.data.locality, place.data.countryCode?.toUpperCase()]
        .filter(Boolean)
        .join(", ")
    : "";

  return (
    <>
      <TopBar title={place.data?.name ?? "Place"} previous={null} next={null}>
        <Link to={`/day/${today()}`}>Today</Link>
      </TopBar>
      <Rail>
        {place.error ? (
          <p className="notice notice-error">{errorMessage(place.error)}</p>
        ) : place.data ? (
          <>
            <div className="day-header">
              {address ? <p className="quiet">{address}</p> : null}
              <p className="figures">
                <span className="figure">{formatCount(place.data.visitCount ?? 0, "visit")}</span>
                <span className="figure figure-quiet">
                  {formatCount(place.data.visitDays ?? 0, "day")}
                </span>
              </p>
            </div>
            {visits.data && visits.data.length > 0 ? (
              <ol className="timeline">
                {visits.data.map((visit) => (
                  <VisitRow key={visit.id} visit={visit} />
                ))}
              </ol>
            ) : (
              <p className="notice">{visits.loading ? "Loading…" : "No visits recorded."}</p>
            )}
          </>
        ) : (
          <p className="notice">{place.loading ? "Loading…" : "No such place."}</p>
        )}
      </Rail>
    </>
  );
}

function VisitRow({ visit }: { visit: Item }) {
  const date = visit.localStartDate;
  return (
    <li className="item item-place">
      <span className={`item-rule${visit.confirmed ? "" : " item-rule-open"}`} />
      <span className="item-main">
        {date ? (
          <Link className="item-title" to={`/day/${date}`}>
            {formatDayShort(date)}
          </Link>
        ) : (
          <span className="item-title">Undated</span>
        )}
        <span className="item-detail">
          {formatTimeRange(
            visit.startDate,
            visit.endDate,
            visit.startOffsetSeconds,
            visit.endOffsetSeconds,
          )}
        </span>
      </span>
      <span className="item-figure">{formatDuration(visit.durationSeconds)}</span>
    </li>
  );
}
