import React from 'react';
import ReactDOM from 'react-dom/client';
import { BrowserRouter } from 'react-router-dom';
import StudioApp from './StudioApp';
import './index.css';

// App Studio = 2e build Vite (base `/studio/`), servie par la même API sous
// `/studio/<slug>`. `basename="/studio"` → les routes du Studio sont relatives à
// ce préfixe (route `/:slug`). Pas d'enregistrement de service worker ici : celui
// de la homepage (scope `/`) couvre déjà `/studio/*`.
ReactDOM.createRoot(document.getElementById('root')).render(
  <React.StrictMode>
    <BrowserRouter basename="/studio">
      <StudioApp />
    </BrowserRouter>
  </React.StrictMode>
);
