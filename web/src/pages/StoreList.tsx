import { useEffect, useState } from "react";
import { Link } from "react-router-dom";
import { Package, Download } from "lucide-react";
import { StoreAppSummary, formatBytes, listStoreApps } from "../api";

export default function StoreList() {
  const [apps, setApps] = useState<StoreAppSummary[] | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    listStoreApps()
      .then(setApps)
      .catch((e) => setError(String(e)));
  }, []);

  if (error) return <p className="text-red-400">Erreur: {error}</p>;
  if (!apps) return <p className="text-gray-500">Chargement…</p>;

  return (
    <div>
      <div className="flex items-baseline justify-between mb-6">
        <p className="text-sm text-gray-500">
          {apps.length} apps · catalogue rsync depuis Medion (5 min).
        </p>
        <a
          href="/api/store/client/apk"
          className="inline-flex items-center gap-1.5 text-xs text-amber-400 hover:underline"
        >
          <Download className="w-3.5 h-3.5" />
          Client APK
        </a>
      </div>
      {apps.length === 0 && (
        <p className="text-gray-500">Aucune app publiée pour le moment.</p>
      )}
      <ul className="grid grid-cols-1 sm:grid-cols-2 lg:grid-cols-3 gap-3">
        {apps.map((a) => (
          <li key={a.slug}>
            <Link
              to={`/store/${a.slug}`}
              className="flex gap-3 p-4 bg-gray-900 border border-gray-800 rounded-md hover:border-amber-400/50 transition-colors h-full"
            >
              {a.icon ? (
                <img
                  src={a.icon}
                  alt=""
                  className="w-12 h-12 rounded-md shrink-0 bg-gray-800"
                  onError={(e) =>
                    ((e.target as HTMLImageElement).style.visibility = "hidden")
                  }
                />
              ) : (
                <div className="w-12 h-12 rounded-md shrink-0 bg-gray-800 flex items-center justify-center">
                  <Package className="w-5 h-5 text-gray-600" />
                </div>
              )}
              <div className="flex-1 min-w-0">
                <div className="font-semibold text-gray-100 truncate">
                  {a.name}
                </div>
                {a.category && (
                  <div className="text-[10px] text-gray-600 uppercase tracking-wider">
                    {a.category}
                  </div>
                )}
                {a.description && (
                  <p className="text-xs text-gray-400 mt-1 line-clamp-2">
                    {a.description}
                  </p>
                )}
                <div className="flex items-center gap-1.5 mt-1.5 text-[11px] text-gray-500">
                  {a.latest_version && (
                    <span className="badge !text-amber-400 !border-amber-900">
                      v{a.latest_version}
                    </span>
                  )}
                  <span className="badge">{a.release_count} releases</span>
                  {a.latest_size_bytes != null && (
                    <span className="badge">
                      {formatBytes(a.latest_size_bytes)}
                    </span>
                  )}
                </div>
              </div>
            </Link>
          </li>
        ))}
      </ul>
    </div>
  );
}
