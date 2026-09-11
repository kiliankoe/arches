/**
 * The one MapLibre instance. Everything else describes what should be on it.
 *
 * This module is loaded lazily (see `App.tsx`), which is what keeps maplibre-gl and its stylesheet
 * out of the initial bundle: the month grid renders before the map library has arrived.
 */

import {
  type GeoJSONSource,
  type LngLatBoundsLike,
  type MapLayerMouseEvent,
  Map as MapLibreMap,
  type MapMouseEvent,
  NavigationControl,
  setWorkerUrl,
} from "maplibre-gl";
import "maplibre-gl/dist/maplibre-gl.css";
// MapLibre builds its worker URL from a template string at runtime, which Vite cannot follow, so
// the file never lands in the bundle and the request falls through to the SPA shell. Importing
// it through Vite's worker pipeline emits a self-contained chunk, and the explicit URL makes
// MapLibre load that copy. Plain `?url` would copy the file alone, without the sibling
// `maplibre-gl-shared.mjs` it imports, and vector tiles would silently never render.
import workerUrl from "maplibre-gl/dist/maplibre-gl-worker.mjs?worker&url";
import { useEffect, useRef } from "react";
import type { Bbox, DayGeoJson, HeatGeoJson } from "../api";
import { activityMatchExpression, VISIT_COLOR } from "../lib/activity";
import { HEAT_RAMP, heatRadiusExpression, heatWeightExpression } from "../lib/heatLayer";

/** What the map is currently showing, for a view that fetches per viewport. */
export type Viewport = { bbox: Bbox; zoom: number };

export type Pin = {
  id: string;
  longitude: number;
  latitude: number;
  label?: string;
};

type Props = {
  styleUrl: string;
  geojson: DayGeoJson | null;
  /** The heatmap's points, or null when no view is asking for one. */
  heat: HeatGeoJson | null;
  /** Called after every pan and zoom, and once when a view starts listening. */
  onViewport?: ((view: Viewport) => void) | null;
  pins: Pin[];
  /** The bounds to frame. Refits only when the numbers actually change. */
  fit: Bbox | null;
  selectedId: string | null;
  onSelect: (id: string | null) => void;
};

setWorkerUrl(workerUrl);

const EMPTY: DayGeoJson = { type: "FeatureCollection", features: [] };
const TRACK_SOURCE = "arches-day";
const PIN_SOURCE = "arches-pins";
const HEAT_SOURCE = "arches-heat";
const CLICKABLE = ["trips-solid", "trips-dashed", "visits-solid", "visits-hollow"];

/** A day is rarely bigger than a city; this is the view before any data arrives. */
const INITIAL_VIEW = { center: [10, 50] as [number, number], zoom: 3 };

export default function MapView({
  styleUrl,
  geojson,
  heat,
  onViewport,
  pins,
  fit,
  selectedId,
  onSelect,
}: Props) {
  const container = useRef<HTMLDivElement>(null);
  const map = useRef<MapLibreMap | null>(null);
  const ready = useRef(false);
  const lastFit = useRef<string | null>(null);
  // The click handler closes over `onSelect`, which changes every render; a ref keeps the
  // listener registration out of the dependency list.
  const select = useRef(onSelect);
  select.current = onSelect;
  const viewport = useRef(onViewport);
  viewport.current = onViewport;
  // Data can arrive before the style finishes loading, so the load handler reads the latest
  // scene through a ref rather than the values it closed over on mount.
  const scene = useRef({ geojson, heat, pins, selectedId });
  scene.current = { geojson, heat, pins, selectedId };

  useEffect(() => {
    if (!container.current) return;
    const instance = new MapLibreMap({
      container: container.current,
      style: styleUrl,
      ...INITIAL_VIEW,
      // OpenFreeMap's licence rides on the attribution control, which stays on by default.
      attributionControl: { compact: true },
    });
    instance.addControl(new NavigationControl({ showCompass: false }), "bottom-right");
    map.current = instance;

    instance.on("load", () => {
      addLayers(instance);
      ready.current = true;
      const { geojson, heat, pins, selectedId } = scene.current;
      refresh(instance, geojson, heat, pins, selectedId);
      report(instance, viewport.current);
    });
    // Both, because a zoom that does not move the centre still changes which cells are asked for.
    for (const event of ["moveend", "zoomend"] as const) {
      instance.on(event, () => ready.current && report(instance, viewport.current));
    }
    for (const layer of CLICKABLE) {
      instance.on("click", layer, (event: MapLayerMouseEvent) => {
        const id = event.features?.[0]?.properties?.itemId;
        if (typeof id === "string") select.current(id);
      });
      instance.on("mouseenter", layer, () => {
        instance.getCanvas().style.cursor = "pointer";
      });
      instance.on("mouseleave", layer, () => {
        instance.getCanvas().style.cursor = "";
      });
    }
    // Clicking the basemap clears the selection, the same as clicking off a list.
    instance.on("click", (event: MapMouseEvent) => {
      if (!ready.current) return;
      const hits = instance.queryRenderedFeatures(event.point, {
        layers: CLICKABLE.filter((layer) => instance.getLayer(layer)),
      });
      if (hits.length === 0) select.current(null);
    });

    return () => {
      ready.current = false;
      map.current = null;
      instance.remove();
    };
  }, [styleUrl]);

  useEffect(() => {
    const instance = map.current;
    if (!instance || !ready.current) return;
    refresh(instance, geojson, heat, pins, selectedId);
  }, [geojson, heat, pins, selectedId]);

  // A view that has just mounted needs the viewport it arrived at, which no pan will announce.
  useEffect(() => {
    const instance = map.current;
    if (!instance || !ready.current || !onViewport) return;
    report(instance, onViewport);
  }, [onViewport]);

  useEffect(() => {
    const instance = map.current;
    if (!instance || !fit) return;
    const key = fit.join(",");
    if (lastFit.current === key) return;
    lastFit.current = key;
    instance.fitBounds(boundsOf(fit), {
      padding: framePadding(),
      maxZoom: 16,
      duration: prefersReducedMotion() ? 0 : 800,
    });
  }, [fit]);

  return <div className="map" ref={container} />;
}

function prefersReducedMotion(): boolean {
  return window.matchMedia("(prefers-reduced-motion: reduce)").matches;
}

/** Keep the framed data out from under the rail, which floats over the map rather than beside it. */
function framePadding() {
  // The rail is measured rather than assumed: the day rail is 360 px but the week and month
  // panels are twice that, and a fit padded for the narrow one lands half its tracks behind
  // the wide one. Narrow layouts turn the rail into a bottom sheet instead.
  const rail = document.querySelector<HTMLElement>(".rail, .rail-wide");
  const narrow = window.matchMedia("(max-width: 720px)").matches;
  if (narrow) {
    const sheet = rail?.offsetHeight ?? window.innerHeight * 0.45;
    return { top: 72, right: 24, bottom: sheet + 24, left: 24 };
  }
  return { top: 72, right: 32, bottom: 32, left: (rail?.offsetWidth ?? 360) + 32 };
}

function boundsOf([minLon, minLat, maxLon, maxLat]: Bbox): LngLatBoundsLike {
  // A day spent in one building has a zero-area box, which fitBounds cannot frame.
  const pad = 0.0004;
  return [
    [minLon - pad, minLat - pad],
    [maxLon + pad, maxLat + pad],
  ];
}

function report(instance: MapLibreMap, onViewport: Props["onViewport"]) {
  if (!onViewport) return;
  const bounds = instance.getBounds();
  onViewport({
    bbox: [bounds.getWest(), bounds.getSouth(), bounds.getEast(), bounds.getNorth()],
    zoom: instance.getZoom(),
  });
}

function refresh(
  instance: MapLibreMap,
  geojson: DayGeoJson | null,
  heat: HeatGeoJson | null,
  pins: Pin[],
  selectedId: string | null,
) {
  const tracks = instance.getSource(TRACK_SOURCE) as GeoJSONSource | undefined;
  tracks?.setData(geojson ?? EMPTY);

  const heatSource = instance.getSource(HEAT_SOURCE) as GeoJSONSource | undefined;
  heatSource?.setData(heat ?? EMPTY);
  if (heat && instance.getLayer("heat")) {
    // The scale rides on the response: a range whose hottest cell is 900 days and one whose
    // hottest is 3 have to read the same, or panning would keep repainting the same ground.
    setIfPresent(instance, "heat", "heatmap-weight", heatWeightExpression(heat.meta.maxWeight));
    setIfPresent(
      instance,
      "heat",
      "heatmap-radius",
      heatRadiusExpression(heat.meta.cellMetres, instance.getZoom(), instance.getCenter().lat),
    );
  }

  const pinSource = instance.getSource(PIN_SOURCE) as GeoJSONSource | undefined;
  if (pinSource) {
    pinSource.setData({
      type: "FeatureCollection",
      features: pins.map((pin) => ({
        type: "Feature" as const,
        geometry: {
          type: "Point" as const,
          coordinates: [pin.longitude, pin.latitude],
        },
        properties: { itemId: pin.id, name: pin.label ?? "" },
      })),
    });
  }

  const selected = ["==", ["get", "itemId"], selectedId ?? " "];
  setIfPresent(instance, "trips-solid", "line-width", ["case", selected, 7, 4]);
  setIfPresent(instance, "trips-dashed", "line-width", ["case", selected, 6, 3.5]);
  setIfPresent(instance, "trips-casing", "line-width", ["case", selected, 12, 8]);
  setIfPresent(instance, "visits-solid", "circle-radius", ["case", selected, 9, 5.5]);
  setIfPresent(instance, "visits-hollow", "circle-radius", ["case", selected, 9, 5.5]);
}

/** MapLibre types paint properties per layer type; these four pairings are checked by hand. */
function setIfPresent(instance: MapLibreMap, layer: string, property: string, value: unknown) {
  if (!instance.getLayer(layer)) return;
  const set = instance.setPaintProperty as unknown as (
    layer: string,
    property: string,
    value: unknown,
  ) => void;
  set.call(instance, layer, property, value);
}

/**
 * Four track layers rather than two: `line-dasharray` is not data-driven in MapLibre, so the
 * confirmed and unconfirmed halves are split by filter. Visits cannot take a dashed outline at
 * all, so an unconfirmed visit is a hollow ring instead of a filled dot; the reading is the same,
 * that the item has not been settled yet.
 */
function addLayers(instance: MapLibreMap) {
  instance.addSource(HEAT_SOURCE, { type: "geojson", data: EMPTY });
  // First, so a day's track drawn on top of it stays readable.
  instance.addLayer({
    id: "heat",
    type: "heatmap",
    source: HEAT_SOURCE,
    paint: {
      "heatmap-weight": heatWeightExpression(1) as never,
      "heatmap-color": HEAT_RAMP as never,
      // Replaced per response from the reported cell size; see `heatRadiusExpression`. Not
      // animated: the transition from the old radius to the new one reads as a flash on every
      // fetch.
      "heatmap-radius": 18,
      "heatmap-radius-transition": { duration: 0 },
      // Below one the shader truncates faint kernels; see `heatLayer.ts`.
      "heatmap-intensity": 1,
      "heatmap-opacity": 0.85,
    },
  });

  instance.addSource(TRACK_SOURCE, { type: "geojson", data: EMPTY });
  instance.addSource(PIN_SOURCE, {
    type: "geojson",
    data: { type: "FeatureCollection", features: [] },
  });

  instance.addLayer({
    id: "trips-casing",
    type: "line",
    source: TRACK_SOURCE,
    filter: ["==", ["geometry-type"], "LineString"],
    layout: { "line-cap": "round", "line-join": "round" },
    paint: { "line-color": "#ffffff", "line-width": 8, "line-opacity": 0.75 },
  });

  const color = activityMatchExpression();
  instance.addLayer({
    id: "trips-solid",
    type: "line",
    source: TRACK_SOURCE,
    filter: ["all", ["==", ["geometry-type"], "LineString"], ["==", ["get", "confirmed"], true]],
    layout: { "line-cap": "round", "line-join": "round" },
    paint: { "line-color": color as never, "line-width": 4 },
  });
  instance.addLayer({
    id: "trips-dashed",
    type: "line",
    source: TRACK_SOURCE,
    filter: ["all", ["==", ["geometry-type"], "LineString"], ["!=", ["get", "confirmed"], true]],
    layout: { "line-cap": "butt", "line-join": "round" },
    paint: {
      "line-color": color as never,
      "line-width": 3.5,
      "line-dasharray": [2, 1.6],
    },
  });

  instance.addLayer({
    id: "visits-solid",
    type: "circle",
    source: TRACK_SOURCE,
    filter: ["all", ["==", ["geometry-type"], "Point"], ["==", ["get", "confirmed"], true]],
    paint: {
      "circle-radius": 5.5,
      "circle-color": VISIT_COLOR,
      "circle-stroke-color": "#ffffff",
      "circle-stroke-width": 2,
    },
  });
  instance.addLayer({
    id: "visits-hollow",
    type: "circle",
    source: TRACK_SOURCE,
    filter: ["all", ["==", ["geometry-type"], "Point"], ["!=", ["get", "confirmed"], true]],
    paint: {
      "circle-radius": 5.5,
      "circle-color": "#ffffff",
      "circle-stroke-color": VISIT_COLOR,
      "circle-stroke-width": 2,
    },
  });

  instance.addLayer({
    id: "place-pin",
    type: "circle",
    source: PIN_SOURCE,
    paint: {
      "circle-radius": 8,
      "circle-color": VISIT_COLOR,
      "circle-stroke-color": "#ffffff",
      "circle-stroke-width": 3,
    },
  });
}
