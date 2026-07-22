import path from "node:path";
import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

export default defineConfig({
  root: "src",
  plugins: [react()],
  base: "/",
  build: {
    outDir: "../dist-web",
    emptyOutDir: true,
  },
  server: {
    port: 4173,
    strictPort: true,
    host: "0.0.0.0",
    proxy: {
      "/api": {
        target: "http://localhost:3000",
        changeOrigin: true,
      },
    },
  },
  resolve: {
    alias: {
      "@": path.resolve(__dirname, "./src"),
    },
  },
  define: {
    "import.meta.env.VITE_MODE": JSON.stringify("web"),
  },
  clearScreen: false,
  envPrefix: ["VITE_"],
});
