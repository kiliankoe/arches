/**
 * What the map should be showing, and which item is highlighted.
 *
 * The map outlives every route, so the scene is lifted to the app and the views push into it.
 * Selection is shared the other way round too: the rail highlights what the map was clicked on.
 */

import { createContext, useContext, useEffect } from "react";
import type { Bbox, DayGeoJson } from "./api";

import type { Pin } from "./components/MapView";

export type Scene = {
  geojson: DayGeoJson | null;
  pins: Pin[];
  fit: Bbox | null;
};

export const EMPTY_SCENE: Scene = { geojson: null, pins: [], fit: null };

type SceneContextValue = {
  setScene: (scene: Scene) => void;
  selectedId: string | null;
  setSelectedId: (id: string | null) => void;
};

export const SceneContext = createContext<SceneContextValue>({
  setScene: () => {},
  selectedId: null,
  setSelectedId: () => {},
});

export function useSelection() {
  const { selectedId, setSelectedId } = useContext(SceneContext);
  return { selectedId, setSelectedId };
}

/** Pass a memoized scene: an object rebuilt on every render would push on every render. */
export function useScene(scene: Scene): void {
  const { setScene } = useContext(SceneContext);
  useEffect(() => {
    setScene(scene);
  }, [scene, setScene]);
}
