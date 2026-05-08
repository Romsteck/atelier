import { Navigate, Route, Routes } from "react-router-dom";
import Layout from "./components/Layout";
import PhasePlaceholder from "./components/PhasePlaceholder";
import DocsList from "./pages/DocsList";
import DocsApp from "./pages/DocsApp";
import DocsEntry from "./pages/DocsEntry";
import StoreList from "./pages/StoreList";
import StoreAppPage from "./pages/StoreApp";
import GitList from "./pages/GitList";
import GitRepoPage from "./pages/GitRepo";
import AppsList from "./pages/AppsList";
import AppDetail from "./pages/AppDetail";
import StudioPage from "./pages/Studio";
import DataverseList from "./pages/DataverseList";
import DataverseSchemaPage from "./pages/DataverseSchema";

const PLACEHOLDERS: Record<
  string,
  { phase: number; feature: string; description: string }
> = {
  flows: {
    phase: 5,
    feature: "Flows",
    description:
      "Moteur d'orchestration TOML. Read-only en Phase 5 (visualisation), exécution en Phase 6. La cible long terme est un daemon multi-stack hr-flowd qui rend les flows utilisables aussi côté NextJS.",
  },
};

function PlaceholderPage({ slug }: { slug: keyof typeof PLACEHOLDERS }) {
  const p = PLACEHOLDERS[slug];
  return <PhasePlaceholder {...p} />;
}

export default function App() {
  return (
    <Layout>
      <Routes>
        <Route path="/" element={<Navigate to="/docs" replace />} />
        <Route path="/docs" element={<DocsList />} />
        <Route path="/docs/:appId" element={<DocsApp />} />
        <Route path="/docs/:appId/:docType/:name" element={<DocsEntry />} />
        <Route path="/store" element={<StoreList />} />
        <Route path="/store/:slug" element={<StoreAppPage />} />
        <Route path="/git" element={<GitList />} />
        <Route path="/git/:slug" element={<GitRepoPage />} />
        <Route path="/flows" element={<PlaceholderPage slug="flows" />} />
        <Route path="/dataverse" element={<DataverseList />} />
        <Route path="/dataverse/:slug" element={<DataverseSchemaPage />} />
        <Route path="/apps" element={<AppsList />} />
        <Route path="/apps/:slug" element={<AppDetail />} />
        <Route path="/studio" element={<StudioPage />} />
        <Route path="*" element={<Navigate to="/docs" replace />} />
      </Routes>
    </Layout>
  );
}
