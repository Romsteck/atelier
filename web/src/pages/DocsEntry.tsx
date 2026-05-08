import { useEffect, useState } from "react";
import { Link, useParams } from "react-router-dom";
import { DocEntry, getEntry } from "../api";

export default function DocsEntry() {
  const { appId, docType, name } = useParams();
  const [entry, setEntry] = useState<DocEntry | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    if (!appId || !docType || !name) return;
    getEntry(appId, docType, name)
      .then(setEntry)
      .catch((e) => setError(String(e)));
  }, [appId, docType, name]);

  if (error) return <p className="error">{error}</p>;
  if (!entry) return <p>Chargement…</p>;

  return (
    <div>
      <p>
        <Link to={`/docs/${appId}`}>← {appId}</Link>
      </p>
      <h1>
        <span className="badge">{entry.type}</span> {entry.name}
      </h1>
      <pre className="markdown">{entry.body}</pre>
      {entry.diagram && (
        <details open>
          <summary>Diagramme mermaid</summary>
          <pre className="mermaid">{entry.diagram}</pre>
        </details>
      )}
    </div>
  );
}
