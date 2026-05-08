import { useEffect, useState } from "react";
import { Link, useParams } from "react-router-dom";
import { ChevronLeft } from "lucide-react";
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

  if (error) return <p className="text-red-400">Erreur: {error}</p>;
  if (!entry) return <p className="text-gray-500">Chargement…</p>;

  return (
    <div className="max-w-4xl">
      <Link
        to={`/docs/${appId}`}
        className="inline-flex items-center gap-1 text-sm text-gray-400 hover:text-amber-400 mb-4"
      >
        <ChevronLeft className="w-4 h-4" />
        {appId}
      </Link>
      <div className="flex items-center gap-2 mb-3">
        <span className="badge">{entry.type}</span>
        <h2 className="text-xl font-semibold">{entry.name}</h2>
      </div>
      <pre className="bg-gray-900 border border-gray-800 rounded-md p-4 text-sm whitespace-pre-wrap font-mono text-gray-300 overflow-x-auto">
        {entry.body}
      </pre>
      {entry.diagram && (
        <details open className="mt-4">
          <summary className="text-sm text-gray-500 cursor-pointer hover:text-gray-300">
            Diagramme mermaid
          </summary>
          <pre className="mt-2 bg-gray-950 border border-gray-800 rounded-md p-3 text-xs whitespace-pre-wrap font-mono text-amber-200/90 overflow-x-auto">
            {entry.diagram}
          </pre>
        </details>
      )}
    </div>
  );
}
