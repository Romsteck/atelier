import { Routes, Route, Navigate, useParams } from 'react-router-dom';
import { AuthProvider } from './context/AuthContext';
import { TaskProvider } from './context/TaskContext';
import { StudioProvider } from './context/StudioContext';
import Layout from './components/Layout';
import Tasks from './pages/Tasks';
import TaskDetail from './pages/TaskDetail';
import Store from './pages/Store';
import Git from './pages/Git';
import DbExplorer from './pages/DbExplorer';
import SchemaPage from './pages/SchemaPage';
import FlowsStats from './pages/FlowsStats';

// Atelier sert le groupe "Applications" du dashboard homeroute, en read-only
// pour la migration parallèle (Phase 2-9 du plan d'extraction).
// Studio + Database + Schema + Store + Git + FlowStats — pas de network/system.

function DocsRedirect() {
  const { appId } = useParams();
  const target = appId
    ? `/studio?app=${encodeURIComponent(appId)}&tab=docs`
    : '/studio?tab=docs';
  return <Navigate to={target} replace />;
}

function App() {
  return (
    <AuthProvider>
      <TaskProvider>
        <StudioProvider>
          <Layout>
          <Routes>
            {/* Default → Studio */}
            <Route path="/" element={<Navigate to="/studio" replace />} />

            {/* Applications group (mirror homeroute Sidebar) */}
            <Route path="/studio" element={null} />
            <Route path="/database" element={<DbExplorer />} />
            <Route path="/schema" element={<SchemaPage />} />
            <Route path="/store" element={<Store />} />
            <Route path="/git" element={<Git />} />
            <Route path="/flows-stats" element={<FlowsStats />} />

            {/* Tasks panel */}
            <Route path="/tasks" element={<Tasks />} />
            <Route path="/tasks/:id" element={<TaskDetail />} />

            {/* Compat redirects */}
            <Route path="/apps" element={<Navigate to="/studio" replace />} />
            <Route path="/apps/:slug" element={<Navigate to="/studio" replace />} />
            <Route path="/dataverse" element={<Navigate to="/database" replace />} />
            <Route path="/dataverse/:slug" element={<Navigate to="/database" replace />} />
            <Route path="/docs" element={<Navigate to="/studio?tab=docs" replace />} />
            <Route path="/docs/:appId" element={<DocsRedirect />} />

            <Route path="*" element={<Navigate to="/studio" replace />} />
          </Routes>
          </Layout>
        </StudioProvider>
      </TaskProvider>
    </AuthProvider>
  );
}

export default App;
