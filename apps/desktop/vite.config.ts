import { defineConfig } from "vite";
import { existsSync } from "node:fs";
import { join } from "node:path";

const cubismFramework = join(process.cwd(), ".vendor/live2d-cubism-sdk/Framework/src");
const cubismRuntime = existsSync(cubismFramework)
  ? join(process.cwd(), "../../scripts/live2d/cubism-runtime.ts")
  : join(process.cwd(), "src/runtime-live2d/cubism-runtime-unavailable.ts");

export default defineConfig({
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
    watch: { ignored: ["**/src-tauri/target/**", "**/src-tauri/gen/**"] },
  },
  envPrefix: ["VITE_", "TAURI_"],
  build: {
    target: "es2022",
    minify: "esbuild",
    sourcemap: true,
    rollupOptions: {
      input: {
        index: join(process.cwd(), "index.html"),
        settings: join(process.cwd(), "settings.html"),
      },
    },
  },
  resolve: {
    alias: {
      "@cubism-runtime": cubismRuntime,
      ...(existsSync(cubismFramework) ? { "@cubism-framework": cubismFramework } : {}),
    },
  },
});
