/**
 * A map that cannot start must not take the timeline down with it. WebGL can be missing or
 * blocked, and the lists and figures are still worth reading without it.
 */

import { Component, type ErrorInfo, type ReactNode } from "react";

type State = { failed: boolean };

export default class MapBoundary extends Component<{ children: ReactNode }, State> {
  state: State = { failed: false };

  static getDerivedStateFromError(): State {
    return { failed: true };
  }

  componentDidCatch(error: Error, info: ErrorInfo) {
    console.error("map failed to start", error, info.componentStack);
  }

  render() {
    if (this.state.failed) {
      return (
        <div className="map map-placeholder">
          <p className="map-failed">Map could not start. WebGL may be unavailable.</p>
        </div>
      );
    }
    return this.props.children;
  }
}
