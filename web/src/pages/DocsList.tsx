import { useEffect, useState } from "react";
import { Link } from "react-router-dom";
import { FileText } from "lucide-react";
import { AppCard, listApps } from "../api";

export default function DocsList() {
  const [apps, setApps] = useState<AppCard[] | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    listApps()
      .then(setApps)
      .catch((e) => setError(String(e)));
  }, []);

  if (error)
    return <p className="text-red-400">Erreur: {error}</p>;
  if (!apps) return <p className="text-gray-500">Chargement…</p>;

  return (
    <div>
      <p className="text-sm text-gray-500 mb-6">
        {apps.length} apps documentées. Données rsync depuis Medion toutes les 5 min.
      </p>
      <ul className="grid grid-cols-1 sm:grid-cols-2 lg:grid-cols-3 gap-3">
        {apps.map((a) => (
          <li key={a.app_id}>
            <Link
              to={`/docs/${a.app_id}`}
              className="block p-4 bg-gray-900 border border-gray-800 rounded-md hover:border-amber-400/50 transition-colors h-full"
            >
              <div className="flex items-center gap-2 mb-2">
                <span className="text-lg">{a.logo || <FileText className="w-4 h-4 inline text-gray-500" />}</span>
                <span className="font-semibold text-gray-100">
                  {a.name || a.app_id}
                </span>
              </div>
              {a.description && (
                <p className="text-xs text-gray-400 mb-2 line-clamp-3">
                  {a.description}
                </p>
              )}
              {a.stack && (
                <p className="text-[11px] text-gray-500 italic mb-2">
                  {a.stack}
                </p>
              )}
              <div className="flex flex-wrap gap-1.5 text-[10px] text-gray-500">
                <span className="badge">{a.stats.screens} screens</span>
                <span className="badge">{a.stats.features} features</span>
                <span className="badge">{a.stats.components} comps</span>
                {a.stats.with_diagram > 0 && (
                  <span className="badge !text-amber-400 !border-amber-900">
                    {a.stats.with_diagram} diagrams
                  </span>
                )}
              </div>
            </Link>
          </li>
        ))}
      </ul>
    </div>
  );
}
