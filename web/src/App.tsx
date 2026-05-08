import { Navigate, Route, Routes } from "react-router-dom";
import Layout from "./components/Layout";
import PhasePlaceholder from "./components/PhasePlaceholder";
import DocsList from "./pages/DocsList";
import DocsApp from "./pages/DocsApp";
import DocsEntry from "./pages/DocsEntry";

const PLACEHOLDERS: Record<
  string,
  { phase: number; feature: string; description: string }
> = {
  store: {
    phase: 3,
    feature: "Store",
    description:
      "Catalogue des apps Flutter installables. Lecture seule sur les manifest YAML des apps. Aucun runtime — purement déclaratif.",
  },
  git: {
    phase: 4,
    feature: "Git",
    description:
      "Bare repos par app, smart HTTP, navigation des commits. Pousse / clone se font via HTTPS authentifié.",
  },
  flows: {
    phase: 5,
    feature: "Flows",
    description:
      "Moteur d'orchestration TOML. Read-only en Phase 5 (visualisation), exécution en Phase 6. La cible long terme est un daemon multi-stack hr-flowd qui rend les flows utilisables aussi côté NextJS.",
  },
  dataverse: {
    phase: 7,
    feature: "Dataverse",
    description:
      "Postgres avec schéma dynamique, GraphQL gateway, expressions dvexpr. Atelier expose la console schéma + l'éditeur de tables + un mode SQL brut.",
  },
  apps: {
    phase: 9,
    feature: "Apps",
    description:
      "Lifecycle complet des apps : start / stop / build / deploy / logs. C'est la phase finale du cutover : Atelier reprend la supervision actuellement assurée par hr-orchestrator sur Medion.",
  },
  studio: {
    phase: 9,
    feature: "Studio",
    description:
      "Code-server intégré : édition des sources, consultation logs, exécution de tâches. La page actuelle studio.mynetwk.biz reste fonctionnelle pendant tout le cutover.",
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
        <Route path="/store" element={<PlaceholderPage slug="store" />} />
        <Route path="/git" element={<PlaceholderPage slug="git" />} />
        <Route path="/flows" element={<PlaceholderPage slug="flows" />} />
        <Route
          path="/dataverse"
          element={<PlaceholderPage slug="dataverse" />}
        />
        <Route path="/apps" element={<PlaceholderPage slug="apps" />} />
        <Route path="/studio" element={<PlaceholderPage slug="studio" />} />
        <Route path="*" element={<Navigate to="/docs" replace />} />
      </Routes>
    </Layout>
  );
}
