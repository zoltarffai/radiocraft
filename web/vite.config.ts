import { svelte, vitePreprocess } from "@sveltejs/vite-plugin-svelte";
import { defineConfig } from "vite";

export default defineConfig({
  plugins: [svelte()],
  build: {
    outDir: "dist",
    emptyOutDir: true,
    target: "es2022",
  },
  server: {
    port: 5173,
    proxy: {
      // `npm run dev` talks to a server on the default port.
      "/ws": { target: "http://127.0.0.1:8080", ws: true },
      "/api": { target: "http://127.0.0.1:8080" },
      "/health": { target: "http://127.0.0.1:8080" },
    },
  },
});
