import { defineConfig } from 'vite';
import react from '@vitejs/plugin-react';

// Build de la HOMEPAGE (panneau de contrôle), base `/`, sort dans `dist/`.
// L'entrée par défaut est `index.html` UNIQUEMENT — `studio.html` est buildé
// séparément (vite.config.studio.js) → la homepage n'embarque pas le poids du
// Studio (agent, mermaid, xterm…).
export default defineConfig({
  plugins: [react()],
  build: { outDir: 'dist' },
  server: {
    port: 5173,
    host: true,
    allowedHosts: true,
    proxy: {
      // WS live (`/api/ws`) inclus via ws:true. L'API Atelier écoute sur 4100.
      '/api': { target: 'http://localhost:4100', changeOrigin: true, ws: true },
      '/apps': { target: 'http://localhost:4100', changeOrigin: true },
    },
  },
});
