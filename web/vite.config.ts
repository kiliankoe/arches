import react from "@vitejs/plugin-react";
import { defineConfig } from "vite";

// https://vite.dev/config/
export default defineConfig({
  plugins: [react()],
  // The dev-time dependency optimizer does not carry MapLibre's worker module along.
  optimizeDeps: { exclude: ["maplibre-gl"] },
  server: {
    proxy: {
      // Override to point the dev server at a backend on another port or host.
      "/api": process.env.ARCHES_API_URL ?? "http://127.0.0.1:8471",
    },
  },
});
