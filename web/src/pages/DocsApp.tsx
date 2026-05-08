import { useEffect, useState } from "react";
import { Link, useParams } from "react-router-dom";
import { Overview, OverviewEntry, getOverview } from "../api";

function EntryRow({
  appId,
  e,
}: {
  appId: string;
  e: OverviewEntry;
}) {
  return (
    <li>
      <Link to={`/docs/${appId}/${e.doc_type}/${e.name}`}>
        <strong>{e.title || e.name}</strong>
      </Link>
      {e.has_diagram && <span className="badge">diagram</span>}
      {e.scope && <span className="scope">{e.scope}</span>}
      {e.summary && <p className="muted">{e.summary}</p>}
    </li>
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

  if (error) return <p className="error">{error}</p>;
  if (!overview) return <p>Chargement…</p>;

  return (
    <div>
      <p>
        <Link to="/">← Toutes les apps</Link>
      </p>
      <h1>{overview.meta.name || appId}</h1>
      {overview.meta.description && <p>{overview.meta.description}</p>}
      <pre className="markdown">{overview.body}</pre>

      {overview.index.screens.length > 0 && (
        <section>
          <h2>Screens ({overview.index.screens.length})</h2>
          <ul className="entries">
            {overview.index.screens.map((e) => (
              <EntryRow key={e.name} appId={appId!} e={e} />
            ))}
          </ul>
        </section>
      )}

      {overview.index.features.length > 0 && (
        <section>
          <h2>Features ({overview.index.features.length})</h2>
          <ul className="entries">
            {overview.index.features.map((e) => (
              <EntryRow key={e.name} appId={appId!} e={e} />
            ))}
          </ul>
        </section>
      )}

      {overview.index.components.length > 0 && (
        <section>
          <h2>Components ({overview.index.components.length})</h2>
          <ul className="entries">
            {overview.index.components.map((e) => (
              <EntryRow key={e.name} appId={appId!} e={e} />
            ))}
          </ul>
        </section>
      )}
    </div>
  );
}
