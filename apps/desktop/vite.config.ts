import { defineConfig } from "vite";
import { existsSync } from "node:fs";
import { join } from "node:path";

const cubismFramework = join(process.cwd(), ".vendor/live2d-cubism-sdk/Framework/src");

export default defineConfig({
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
    watch: { ignored: ["**/src-tauri/target/**", "**/src-tauri/gen/**"] },
  },
  envPrefix: ["VITE_", "TAURI_"],
  build: { target: "es2022", minify: "esbuild", sourcemap: true },
  ...(existsSync(cubismFramework)
    ? { resolve: { alias: { "@cubism-framework": cubismFramework } } }
    : {}),
});
