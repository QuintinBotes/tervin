import { defineConfig } from "vitest/config";
import react from "@vitejs/plugin-react";

// The UI lives in ui/ and builds to ui-dist/, which tauri.conf.json points at.
export default defineConfig({
  root: "ui",
  plugins: [react()],
  clearScreen: false,
  server: {
    port: 5173,
    strictPort: true,
    // A dropped HMR socket inside the webview should reconnect, not reload.
    watch: { ignored: ["**/target/**", "**/ui-dist/**"] },
  },
  test: {
    // `node` by default, because most tests here are pure logic and do not need a DOM.
    // A component test opts in with a `@vitest-environment jsdom` docblock — vitest 3
    // removed `environmentMatchGlobs`, and declaring it in the file is clearer anyway.
    environment: "node",
    globals: false,
    restoreMocks: true,
  },
  build: {
    outDir: "../ui-dist",
    emptyOutDir: true,
    // Tauri ships a current WebView; targeting it avoids needless transpilation.
    target: "es2022",
    sourcemap: true,
  },
});
