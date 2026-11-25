import { defineConfig } from "vite";
import { svelte } from "@sveltejs/vite-plugin-svelte";

// Puerto fijo: tauri.conf.json apunta a devUrl.
export default defineConfig({
  plugins: [svelte()],
  clearScreen: false,
  envPrefix: ["VITE_", "TAURI_ENV_*"],
  server: { port: 1420, strictPort: true, host: "127.0.0.1" },
  build: {
    target: process.env.TAURI_ENV_PLATFORM === "windows" ? "chrome105" : "safari15",
    minify: !process.env.TAURI_ENV_DEBUG,
    sourcemap: !!process.env.TAURI_ENV_DEBUG,
  },
});
