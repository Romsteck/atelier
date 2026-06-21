import { useEffect } from 'react';
import { Routes, Route, useParams } from 'react-router-dom';
import { ThemeProvider } from './context/ThemeContext';
import { AuthProvider } from './context/AuthContext';
import StudioShell from './pages/StudioShell';

// Slug absent (`/studio/` nu) → rien à éditer : on quitte vers la homepage. WHY
// `window.location` et pas <Navigate to="/"> : avec `basename="/studio"`, un
// Navigate resterait DANS le préfixe (`/studio/`) ; la homepage est une AUTRE app
// Vite, on en sort donc par une vraie navigation document.
function RedirectHome() {
  useEffect(() => { window.location.replace('/'); }, []);
  return null;
}

// Garantit un slug non vide avant de monter le Shell.
function StudioRoute() {
  const { slug } = useParams();
  if (!slug) return <RedirectHome />;
  return <StudioShell slug={slug} />;
}

export default function StudioApp() {
  return (
    <ThemeProvider>
      <AuthProvider>
        <Routes>
          <Route path="/:slug" element={<StudioRoute />} />
          <Route path="*" element={<RedirectHome />} />
        </Routes>
      </AuthProvider>
    </ThemeProvider>
  );
}
