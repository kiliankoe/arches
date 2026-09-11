import { lazy, Suspense, useMemo, useState } from "react";
import { Navigate, Route, Routes, useLocation } from "react-router";
import { api } from "./api";
import MapBoundary from "./components/MapBoundary";
import { useResource } from "./hooks";
import { today } from "./lib/dates";
import DayView from "./routes/DayView";
import HeatView from "./routes/HeatView";
import MonthView from "./routes/MonthView";
import PlaceView from "./routes/PlaceView";
import WeekView from "./routes/WeekView";
import { EMPTY_SCENE, type Scene, SceneContext } from "./scene";
import "./styles/base.css";
import "./styles/chrome.css";
import "./styles/views.css";

// Loaded on demand so maplibre-gl and its stylesheet stay out of the first paint; the calendar
// grid is readable before the map library has finished arriving.
const MapView = lazy(() => import("./components/MapView"));

export default function App() {
  const [scene, setScene] = useState<Scene>(EMPTY_SCENE);
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const config = useResource("config", () => api.config());
  // The heatmap ramp is built for a dark basemap; every other view keeps the light one.
  const dark = useLocation().pathname.startsWith("/heat");

  const context = useMemo(() => ({ setScene, selectedId, setSelectedId }), [selectedId]);

  return (
    <SceneContext.Provider value={context}>
      <div className="shell">
        {config.data ? (
          <MapBoundary>
            <Suspense fallback={<div className="map map-placeholder" />}>
              <MapView
                styleUrl={dark ? config.data.mapStyleDark : config.data.mapStyle}
                geojson={scene.geojson}
                heat={scene.heat ?? null}
                onViewport={scene.onViewport ?? null}
                pins={scene.pins}
                fit={scene.fit}
                selectedId={selectedId}
                onSelect={setSelectedId}
              />
            </Suspense>
          </MapBoundary>
        ) : (
          <div className="map map-placeholder" />
        )}
        <Routes>
          <Route path="/" element={<Navigate to={`/day/${today()}`} replace />} />
          <Route path="/day/:date" element={<DayView />} />
          <Route path="/month/:month" element={<MonthView />} />
          <Route path="/week/:week" element={<WeekView />} />
          <Route path="/place/:id" element={<PlaceView />} />
          <Route path="/heat" element={<HeatView />} />
          <Route path="*" element={<Navigate to={`/day/${today()}`} replace />} />
        </Routes>
      </div>
    </SceneContext.Provider>
  );
}
