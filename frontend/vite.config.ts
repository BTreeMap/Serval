import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
import tailwindcss from "@tailwindcss/vite";

// The production bundle is embedded into the Rust binary from `dist/`. During
// development, `/api` is proxied to the Control Plane so the SPA and the live
// backend share an origin without CORS friction.
export default defineConfig({
    plugins: [react(), tailwindcss()],
    build: {
        outDir: "dist",
        emptyOutDir: true,
    },
    server: {
        port: 5173,
        proxy: {
            "/api": {
                target: "http://localhost:8080",
                changeOrigin: true,
            },
        },
    },
});
