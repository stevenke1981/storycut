import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

export default defineConfig({
  plugins: [react()],
  // A machine-level PostCSS config under D:\ injects Tailwind into every Vite
  // app. This desktop ships plain CSS and must not depend on that global file.
  css: { postcss: { plugins: [] } },
  clearScreen: false,
  server: { strictPort: true, watch: { ignored: ["**/src-tauri/**", "**/node_modules/**", "**/target/**"] } },
});
