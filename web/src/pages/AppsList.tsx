import { useEffect, useState } from "react";
import { Link } from "react-router-dom";
import { Boxes, Lock, Globe } from "lucide-react";
import { App, listApps } from "../api";

const STATE_COLOR: Record<string, string> = {
  running: "bg-emerald-500",
  stopped: "bg-gray-500",
  starting: "bg-amber-400",
  failed: "bg-red-500",
  building: "bg-blue-400",
};

export default function AppsList() {
  const [apps, setApps] = useState<App[] | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    listApps()
      .then(setApps)
      .catch((e) => setError(String(e)));
  }, []);

  if (error) return <p className="text-red-400">Erreur: {error}</p>;
  if (!apps) return <p className="text-gray-500">Chargement…</p>;

  return (
    <div>
      <p className="text-sm text-gray-500 mb-6">
        {apps.length} apps · état issu de la registry Medion (sync 2 min) ·{" "}
        <span className="text-amber-400">read-only</span> jusqu'au cutover Phase 9
        (build / deploy / start / stop restent côté{" "}
        <a href="https://proxy.mynetwk.biz" className="hover:underline">
          homeroute
        </a>
        ).
      </p>
      <ul className="grid grid-cols-1 md:grid-cols-2 gap-3">
        {apps.map((a) => {
          const dot = STATE_COLOR[a.state] ?? "bg-gray-600";
          const Vis = a.visibility === "private" ? Lock : Globe;
          return (
            <li key={a.slug}>
              <Link
                to={`/apps/${a.slug}`}
                className="block p-4 bg-gray-900 border border-gray-800 rounded-md hover:border-amber-400/50 transition-colors"
              >
                <div className="flex items-center gap-2 mb-1">
                  <Boxes className="w-4 h-4 text-amber-400 shrink-0" />
                  <span className="font-semibold text-gray-100">{a.name}</span>
                  <span
                    className={`w-2 h-2 rounded-full ${dot}`}
                    title={a.state}
                  />
                  <span className="text-[11px] text-gray-500">{a.state}</span>
                </div>
                <div className="text-xs text-gray-400 font-mono mb-2">
                  {a.slug} · :{a.port}
                </div>
                <div className="flex flex-wrap gap-1.5 text-[11px]">
                  <span className="badge">{a.stack}</span>
                  <span className="badge inline-flex items-center gap-1">
                    <Vis className="w-3 h-3" />
                    {a.visibility}
                  </span>
                  {a.db_backend && a.db_backend !== "legacy-sqlite" && (
                    <span className="badge !text-blue-300">{a.db_backend}</span>
                  )}
                  {a.sources_on && (
                    <span className="badge">src: {a.sources_on}</span>
                  )}
                </div>
                <div className="text-[11px] text-amber-400/80 mt-2 truncate">
                  {a.domain}
                </div>
              </Link>
            </li>
          );
        })}
      </ul>
    </div>
  );
}
