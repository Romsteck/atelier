import { useEffect } from 'react';
import { Routes, Route, Navigate, useParams } from 'react-router-dom';
import { ThemeProvider } from './context/ThemeContext';
import { AuthProvider } from './context/AuthContext';
import { TaskProvider } from './context/TaskContext';
import { AppsProvider } from './context/AppsContext';
import Layout from './components/Layout';
import Tasks from './pages/Tasks';
import TaskDetail from './pages/TaskDetail';
import Git from './pages/Git';
import Apps from './pages/Apps';
import DbExplorer from './pages/DbExplorer';
import SchemaPage from './pages/SchemaPage';
import Surveillance from './pages/Surveillance';
import Backup from './pages/Backup';

// Le Studio est désormais une app Vite SÉPARÉE servie sous `/studio/<slug>` (cf.
// crates/atelier-api/src/lib.rs). La homepage (cette SPA) ne contient plus le
// Studio : la galerie d'apps est la landing `/`, et ouvrir une app se fait via
// `openStudio` (nouvel onglet focalisé). Plus aucun lien client-side vers `/studio`.

// Redirection « dure » (document) vers une URL hors de cette SPA (ex. l'app Studio).
function HardRedirect({ to }) {
  useEffect(() => { window.location.replace(to); }, [to]);
  return null;
}

// Legacy : `/docs/:appId` → onglet Docs du Studio de l'app (même onglet).
function DocsRedirect() {
  const { appId } = useParams();
  return <HardRedirect to={appId ? `/studio/${appId}?tab=docs` : '/'} />;
}

function App() {
  return (
    <ThemeProvider>
    <AuthProvider>
      <TaskProvider>
        <AppsProvider>
          <Layout>
          <Routes>
            {/* Landing = galerie d'apps */}
            <Route path="/" element={<Apps />} />

            {/* Panneau de contrôle */}
            <Route path="/database" element={<DbExplorer />} />
            <Route path="/schema" element={<SchemaPage />} />
            <Route path="/git" element={<Git />} />
            <Route path="/surveillance" element={<Surveillance />} />
            <Route path="/backup" element={<Backup />} />

            {/* Tasks panel */}
            <Route path="/tasks" element={<Tasks />} />
            <Route path="/tasks/:id" element={<TaskDetail />} />

            {/* Compat redirects */}
            <Route path="/studio" element={<Navigate to="/" replace />} />
            <Route path="/apps" element={<Navigate to="/" replace />} />
            <Route path="/dataverse" element={<Navigate to="/database" replace />} />
            <Route path="/dataverse/:slug" element={<Navigate to="/database" replace />} />
            <Route path="/docs" element={<Navigate to="/" replace />} />
            <Route path="/docs/:appId" element={<DocsRedirect />} />

            <Route path="*" element={<Navigate to="/" replace />} />
          </Routes>
          </Layout>
        </AppsProvider>
      </TaskProvider>
    </AuthProvider>
    </ThemeProvider>
  );
}

export default App;
