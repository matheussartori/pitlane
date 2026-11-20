import { defineConfig } from "vite";
import solid from "vite-plugin-solid";

// Tauri expects a fixed port and must see Rust errors in the terminal.
export default defineConfig({
  plugins: [solid()],
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
    // design/ lives at the repo root and is the single source of the tokens.
    fs: { allow: ["..", "../.."] },
    watch: { ignored: ["**/src-tauri/**"] },
  },
  build: { target: "safari16", outDir: "dist" },
});
