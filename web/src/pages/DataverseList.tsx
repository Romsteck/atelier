import { useEffect, useState } from "react";
import { Link } from "react-router-dom";
import { Database, Lock } from "lucide-react";
import { App, listApps } from "../api";

export default function DataverseList() {
  const [apps, setApps] = useState<App[] | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    listApps()
      .then((all) =>
        setApps(all.filter((a) => a.db_backend === "postgres-dataverse")),
      )
      .catch((e) => setError(String(e)));
  }, []);

  if (error) return <p className="text-red-400">Erreur: {error}</p>;
  if (!apps) return <p className="text-gray-500">Chargement…</p>;

  return (
    <div>
      <p className="text-sm text-gray-500 mb-6">
        {apps.length} apps avec un dataverse Postgres. Atelier se connecte en
        LAN à Medion (10.0.0.254:5432) avec les credentials synchronisés
        (sync-state, 2 min). <span className="text-amber-400">Read-only</span> —
        les écritures et schema-ops continuent côté{" "}
        <a href="https://proxy.mynetwk.biz" className="hover:underline">
          homeroute
        </a>
        .
      </p>
      {apps.length === 0 && (
        <p className="text-gray-500">Aucune app avec dataverse activé.</p>
      )}
      <ul className="grid grid-cols-1 md:grid-cols-2 gap-3">
        {apps.map((a) => (
          <li key={a.slug}>
            <Link
              to={`/dataverse/${a.slug}`}
              className="block p-4 bg-gray-900 border border-gray-800 rounded-md hover:border-amber-400/50 transition-colors"
            >
              <div className="flex items-center gap-2 mb-1">
                <Database className="w-4 h-4 text-amber-400 shrink-0" />
                <span className="font-semibold text-gray-100">{a.name}</span>
              </div>
              <div className="text-xs text-gray-400 font-mono mb-2">
                postgres://app_{a.slug}@10.0.0.254/app_{a.slug}
              </div>
              <div className="flex flex-wrap gap-1.5 text-[11px]">
                <span className="badge inline-flex items-center gap-1">
                  <Lock className="w-3 h-3" /> bearer-required
                </span>
                <span className="badge">{a.stack}</span>
              </div>
            </Link>
          </li>
        ))}
      </ul>
    </div>
  );
}
