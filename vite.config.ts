import { defineConfig } from "vite";
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
  build: {
    outDir: "../ui-dist",
    emptyOutDir: true,
    // Tauri ships a current WebView; targeting it avoids needless transpilation.
    target: "es2022",
    sourcemap: true,
  },
});
