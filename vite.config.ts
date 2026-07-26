import { fileURLToPath, URL } from "node:url";
import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
import tailwindcss from "@tailwindcss/vite";

// @ts-expect-error process is a nodejs global
const host = process.env.TAURI_DEV_HOST;

// https://vite.dev/config/
export default defineConfig(async () => ({
  plugins: [react(), tailwindcss()],
  resolve: {
    alias: {
      "@": fileURLToPath(new URL("./src", import.meta.url)),
    },
  },

  // Vite options tailored for Tauri development and only applied in `tauri dev` or `tauri build`
  //
  // 1. prevent Vite from obscuring rust errors
  clearScreen: false,
  // 2. tauri expects a fixed port, fail if that port is not available
  server: {
    port: 1420,
    strictPort: true,
    host: host || false,
    hmr: host
      ? {
          protocol: "ws",
          host,
          port: 1421,
        }
      : undefined,
    watch: {
      // 3. tell Vite to ignore watching `src-tauri`
      // 4. and `.hangar/worktrees` — subsession worktrees/copies Hangar
      //    itself creates under a managed project's own directory. When
      //    Hangar is used on its own repo (dogfooding), those worktrees land
      //    inside this very project tree; without this, creating/updating a
      //    subsession looks like a source change and triggers a full page
      //    reload of the whole app. `.fleet/` is the pre-rename location —
      //    worktrees created before the rename still live there.
      ignored: ["**/src-tauri/**", "**/.hangar/**", "**/.fleet/**"],
    },
  },
}));
