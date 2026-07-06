import { useEffect } from 'react';
import { Routes, Route, Navigate, useParams } from 'react-router-dom';
import { ThemeProvider } from './context/ThemeContext';
import { AuthProvider } from './context/AuthContext';
import { TaskProvider } from './context/TaskContext';
import { AppsProvider } from './context/AppsContext';
import { NotificationsProvider } from './context/NotificationsContext';
import { IssuesProvider } from './context/IssuesContext';
import Layout from './components/Layout';
import ErrorBoundary from './components/ErrorBoundary';
import Tasks from './pages/Tasks';
import TaskDetail from './pages/TaskDetail';
import Git from './pages/Git';
import Apps from './pages/Apps';
import DbExplorer from './pages/DbExplorer';
import SchemaPage from './pages/SchemaPage';
import Surveillance from './pages/Surveillance';
import Backup from './pages/Backup';
import Settings from './pages/Settings';
import Issues from './pages/Issues';

// Le Studio est désormais une app Vite SÉPARÉE servie sous `/studio/<slug>` (cf.
// crates/atelier-api/src/lib.rs). La homepage (cette SPA) ne contient plus le
// Studio : la galerie d'apps est la landing `/`, et ouvrir une app se fait via
// `openStudio` (nouvel onglet focalisé). Plus aucun lien client-side vers `/studio`.

// Legacy : `/docs/:appId` → onglet Docs du Studio de l'app. Navigation document
// (hors SPA) : on ne peut pas PUT-puis-attendre proprement, donc on cible l'onglet
// par `?tab=docs` (lu en fallback par StudioShell).
function DocsRedirect() {
  const { appId } = useParams();
  useEffect(() => {
    window.location.replace(appId ? `/studio/${appId}?tab=docs` : '/');
  }, [appId]);
  return null;
}

function App() {
  return (
    <ThemeProvider>
    <AuthProvider>
      <TaskProvider>
        <AppsProvider>
          <NotificationsProvider>
          <IssuesProvider>
          <ErrorBoundary>
          <Layout>
          <Routes>
            {/* Landing = galerie d'apps */}
            <Route path="/" element={<Apps />} />

            {/* Panneau de contrôle */}
            <Route path="/database" element={<DbExplorer />} />
            <Route path="/schema" element={<SchemaPage />} />
            <Route path="/git" element={<Git />} />
            <Route path="/surveillance" element={<Surveillance />} />
            <Route path="/issues" element={<Issues />} />
            <Route path="/backup" element={<Backup />} />
            <Route path="/settings" element={<Settings />} />

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
          </ErrorBoundary>
          </IssuesProvider>
          </NotificationsProvider>
        </AppsProvider>
      </TaskProvider>
    </AuthProvider>
    </ThemeProvider>
  );
}

export default App;
