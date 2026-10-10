import react from "@vitejs/plugin-react";
import { defineConfig } from "vite";

// The screen preview harness: a dev server of its own, run with this folder as its root (see the
// scripts in package.json). It is never part of `vite build`, whose input is ../index.html. The
// two modules testApi.tsx takes from the test tools are swapped for ones that run in a browser;
// the paths are from this root.
export default defineConfig({
  plugins: [react()],
  clearScreen: false,
  resolve: {
    alias: [
      { find: /^vitest$/, replacement: "/shims/vitest.ts" },
      { find: /^@testing-library\/react$/, replacement: "/shims/act.ts" },
    ],
  },
  server: { port: 1430, strictPort: true },
});
