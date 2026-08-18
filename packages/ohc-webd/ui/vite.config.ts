import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

// Builds to ui/dist, which build.rs embeds into ohc-webd. In dev, proxy the API
// and WebSocket to a running ohc-webd (set OHC_DEV_TARGET, default the CA-1).
const target = process.env.OHC_DEV_TARGET || "http://192.168.1.178";

export default defineConfig({
  plugins: [react()],
  build: { outDir: "dist", emptyOutDir: true },
  server: {
    proxy: {
      "/api": { target, changeOrigin: true },
      "/ws": { target: target.replace(/^http/, "ws"), ws: true },
    },
  },
});
