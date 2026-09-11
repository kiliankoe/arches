import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { BrowserRouter } from "react-router";
import App from "./App.tsx";
import { applyPalette } from "./lib/activity.ts";

const root = document.getElementById("root");
if (!root) throw new Error("missing #root element");

// The activity palette is defined in TypeScript and mirrored into CSS custom properties here, so
// the timeline rules, the calendar bars and the map lines cannot disagree about what "tram" is.
applyPalette(document.documentElement);

createRoot(root).render(
  <StrictMode>
    <BrowserRouter>
      <App />
    </BrowserRouter>
  </StrictMode>,
);
