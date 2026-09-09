import react from "@vitejs/plugin-react";
import { defineConfig } from "vite";

// https://vite.dev/config/
export default defineConfig({
  plugins: [react()],
  server: {
    proxy: {
      // Override to point the dev server at a backend on another port or host.
      "/api": process.env.ARCHES_API_URL ?? "http://127.0.0.1:8471",
    },
  },
});
