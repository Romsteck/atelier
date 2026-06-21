import { defineConfig } from 'vite';
import react from '@vitejs/plugin-react';

// 2e build Vite : l'app STUDIO, séparée de la homepage. Servie sous `/studio/<slug>`
// par l'API Axum (nest_service, cf. crates/atelier-api/src/lib.rs). `base: '/studio/'`
// → les assets sont référencés en `/studio/assets/...`. Sort dans `dist/studio/`
// (sous-dossier de la dist homepage → un seul rsync au deploy).
//
// ⚠️ À builder APRÈS la homepage : `vite build` (homepage) vide `dist/` (donc
// `dist/studio/` aussi). L'entrée est `studio.html` → la sortie est `studio.html`
// (Vite préserve le nom du fichier d'entrée) ; c'est ce fichier que l'API sert en
// fallback SPA du Studio.
export default defineConfig({
  plugins: [react()],
  base: '/studio/',
  build: {
    outDir: 'dist/studio',
    emptyOutDir: true,
    rollupOptions: { input: 'studio.html' },
  },
});
