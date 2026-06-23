import React from 'react';
import ReactDOM from 'react-dom/client';
import { BrowserRouter } from 'react-router-dom';
import StudioApp from './StudioApp';
import './index.css';

// App Studio = 2e build Vite (base `/studio/`), servie par la même API sous
// `/studio/<slug>`. `basename="/studio"` → les routes du Studio sont relatives à
// ce préfixe (route `/:slug`).
ReactDOM.createRoot(document.getElementById('root')).render(
  <React.StrictMode>
    <BrowserRouter basename="/studio">
      <StudioApp />
    </BrowserRouter>
  </React.StrictMode>
);

// On enregistre le service worker ICI aussi (script à la racine → scope `/`).
// Indispensable pour les notifications agent : les chats vivent dans l'onglet
// Studio, qui doit être contrôlé par le SW (registration.showNotification +
// Badging). Idempotent : no-op si la homepage l'a déjà enregistré.
if ('serviceWorker' in navigator) {
  window.addEventListener('load', () => {
    navigator.serviceWorker.register('/sw.js').catch(() => {});
  });
}
