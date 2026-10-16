import react from "@vitejs/plugin-react";
import { defineConfig } from "vite";

export default defineConfig({
  plugins: [react()],
  server: {
    port: 5173,
    // dev: proxy API calls to the rust backend
    proxy: {
      "/api": process.env.MANDARA_API_TARGET || "http://127.0.0.1:8080",
    },
  },
  build: {
    outDir: "dist",
  },
});