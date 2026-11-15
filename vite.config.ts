import { defineConfig } from "vite";
import vue from "@vitejs/plugin-vue";

export default defineConfig({
  plugins: [vue()],
  clearScreen: false,
  server: { port: 1420, strictPort: true, watch: { ignored: ["**/src-tauri/**", "**/obs-plugin/**", "**/shm-proto/**"] } },
  build: { target: "chrome110", outDir: "dist" },
});
