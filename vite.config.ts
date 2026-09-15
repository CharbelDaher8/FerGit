/// <reference types="vitest/config" />
import { defineConfig } from "vite";
import { svelte } from "@sveltejs/vite-plugin-svelte";

// Tauri sets this when developing against a physical device; unset on desktop.
const host = process.env.TAURI_DEV_HOST;

export default defineConfig({
  root: "ui",
  plugins: [svelte()],
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
    host: host || false,
    watch: { ignored: ["**/src-tauri/**", "**/target/**", "**/crates/**"] },
  },
  envPrefix: ["VITE_", "TAURI_ENV_*"],
  build: {
    outDir: "../dist",
    emptyOutDir: true,
    target: "es2022",
  },
  test: {
    // Relative to `root`. Only the pure modules are unit-tested; they need no DOM.
    include: ["src/**/*.test.ts"],
    environment: "node",
  },
});
