import { defineConfig } from "vite";

export default defineConfig({
  base: "./",
  esbuild: { jsx: "automatic", jsxImportSource: "preact" },
  worker: { format: "es" },
  build: { target: "es2022" },
});
