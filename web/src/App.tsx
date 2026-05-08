import { Link, Route, Routes } from "react-router-dom";
import DocsList from "./pages/DocsList";
import DocsApp from "./pages/DocsApp";
import DocsEntry from "./pages/DocsEntry";

export default function App() {
  return (
    <div className="layout">
      <header className="topbar">
        <Link to="/" className="brand">
          Atelier
        </Link>
        <span className="tag">Phase 2 — Docs read-only</span>
      </header>
      <main className="content">
        <Routes>
          <Route path="/" element={<DocsList />} />
          <Route path="/docs/:appId" element={<DocsApp />} />
          <Route path="/docs/:appId/:docType/:name" element={<DocsEntry />} />
        </Routes>
      </main>
    </div>
  );
}
