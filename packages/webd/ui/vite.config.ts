import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
import tailwindcss from "@tailwindcss/vite";

// Builds to ui/dist, which build.rs embeds into webd. In dev, proxy the API
// and WebSocket to a running webd (set OHC_DEV_TARGET, default the CA-1).
const target = process.env.OHC_DEV_TARGET || "http://192.168.1.178";
// In production webd proxies /iod itself. In dev there is no webd in front, so
// point /iod straight at a reachable iod — a tunnel, or the box.
const iod = process.env.OHC_DEV_IOD || "http://localhost:7070";

export default defineConfig({
  plugins: [react(), tailwindcss()],
  build: { outDir: "dist", emptyOutDir: true },
  server: {
    proxy: {
      "/api": { target, changeOrigin: true },
      "/ws": { target: target.replace(/^http/, "ws"), ws: true },
      // Mirrors what webd does in production: strip the mount point and
      // forward, upgrades included.
      "/iod": {
        target: iod,
        ws: true,
        changeOrigin: true,
        rewrite: (p) => p.replace(/^\/iod/, ""),
      },
    },
  },
});
