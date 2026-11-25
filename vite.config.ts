import { defineConfig } from "vite";
// @ts-expect-error no @types/node in this project
import process from "node:process";

const host = process.env.TAURI_DEV_HOST;

// Tauri runs the dev server on a fixed port and shows Rust errors itself.
export default defineConfig(() => ({
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
    host: host || false,
    hmr: host ? { protocol: "ws", host, port: 1421 } : undefined,
    watch: { ignored: ["**/src-tauri/**"] },
  },
  // hls.js is one lazy chunk of about 580 kB, loaded only where the webview cannot play HLS.
  build: { chunkSizeWarningLimit: 600 },
}));
