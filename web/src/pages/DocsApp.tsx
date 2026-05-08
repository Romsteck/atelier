import { useEffect, useState } from "react";
import { Link, useParams } from "react-router-dom";
import { ChevronLeft } from "lucide-react";
import { Overview, OverviewEntry, getOverview } from "../api";

function EntryRow({ appId, e }: { appId: string; e: OverviewEntry }) {
  return (
    <li className="py-2 border-b border-gray-800 last:border-0">
      <div className="flex items-center gap-2 flex-wrap">
        <Link
          to={`/docs/${appId}/${e.doc_type}/${e.name}`}
          className="font-medium text-gray-100 hover:text-amber-400"
        >
          {e.title || e.name}
        </Link>
        {e.has_diagram && (
          <span className="badge !text-amber-400 !border-amber-900">diagram</span>
        )}
        {e.scope && <span className="badge">{e.scope}</span>}
      </div>
      {e.summary && <p className="text-xs text-gray-500 mt-1">{e.summary}</p>}
    </li>
  );
}

function Section({
  title,
  entries,
  appId,
}: {
  title: string;
  entries: OverviewEntry[];
  appId: string;
}) {
  if (entries.length === 0) return null;
  return (
    <section className="mt-6">
      <h3 className="text-sm font-semibold text-gray-400 uppercase tracking-wider mb-2">
        {title} ({entries.length})
      </h3>
      <ul>
        {entries.map((e) => (
          <EntryRow key={e.name} appId={appId} e={e} />
        ))}
      </ul>
    </section>
  );
}

export default function DocsApp() {
  const { appId } = useParams();
  const [overview, setOverview] = useState<Overview | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    if (!appId) return;
    getOverview(appId)
      .then(setOverview)
      .catch((e) => setError(String(e)));
  }, [appId]);

  if (error) return <p className="text-red-400">Erreur: {error}</p>;
  if (!overview) return <p className="text-gray-500">Chargement…</p>;

  return (
    <div className="max-w-4xl">
      <Link
        to="/docs"
        className="inline-flex items-center gap-1 text-sm text-gray-400 hover:text-amber-400 mb-4"
      >
        <ChevronLeft className="w-4 h-4" />
        Toutes les apps
      </Link>
      <h2 className="text-2xl font-semibold mb-1">
        {overview.meta.name || appId}
      </h2>
      {overview.meta.description && (
        <p className="text-gray-400 mb-4">{overview.meta.description}</p>
      )}
      {overview.body && (
        <pre className="bg-gray-900 border border-gray-800 rounded-md p-4 text-sm whitespace-pre-wrap font-mono text-gray-300 overflow-x-auto">
          {overview.body}
        </pre>
      )}

      <Section
        title="Screens"
        entries={overview.index.screens}
        appId={appId!}
      />
      <Section
        title="Features"
        entries={overview.index.features}
        appId={appId!}
      />
      <Section
        title="Components"
        entries={overview.index.components}
        appId={appId!}
      />
    </div>
  );
}
