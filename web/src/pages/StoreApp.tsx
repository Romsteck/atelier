import { useEffect, useState } from "react";
import { Link, useParams } from "react-router-dom";
import { ChevronLeft, Download, Package } from "lucide-react";
import { StoreApp, formatBytes, getStoreApp } from "../api";

export default function StoreAppPage() {
  const { slug } = useParams();
  const [app, setApp] = useState<StoreApp | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    if (!slug) return;
    getStoreApp(slug)
      .then(setApp)
      .catch((e) => setError(String(e)));
  }, [slug]);

  if (error) return <p className="text-red-400">Erreur: {error}</p>;
  if (!app) return <p className="text-gray-500">Chargement…</p>;

  const releases = [...app.releases].reverse();

  return (
    <div className="max-w-4xl">
      <Link
        to="/store"
        className="inline-flex items-center gap-1 text-sm text-gray-400 hover:text-amber-400 mb-4"
      >
        <ChevronLeft className="w-4 h-4" />
        Store
      </Link>
      <div className="flex gap-4 mb-6">
        {app.icon ? (
          <img
            src={app.icon}
            alt=""
            className="w-20 h-20 rounded-lg bg-gray-800 shrink-0"
          />
        ) : (
          <div className="w-20 h-20 rounded-lg bg-gray-800 flex items-center justify-center shrink-0">
            <Package className="w-8 h-8 text-gray-600" />
          </div>
        )}
        <div className="min-w-0">
          <h2 className="text-2xl font-semibold">{app.name}</h2>
          {app.category && (
            <div className="text-xs text-gray-500 uppercase tracking-wider">
              {app.category}
            </div>
          )}
          {app.description && (
            <p className="text-gray-400 mt-2">{app.description}</p>
          )}
          <div className="flex flex-wrap gap-1.5 mt-2 text-[11px]">
            <span className="badge">{app.slug}</span>
            {app.android_package && (
              <span className="badge font-mono">{app.android_package}</span>
            )}
            {app.publisher_app_id && (
              <span className="badge">publisher: {app.publisher_app_id}</span>
            )}
          </div>
        </div>
      </div>

      <h3 className="text-sm font-semibold text-gray-400 uppercase tracking-wider mb-2">
        Releases ({releases.length})
      </h3>
      <ul className="divide-y divide-gray-800 border border-gray-800 rounded-md bg-gray-900">
        {releases.map((r) => (
          <li
            key={r.version}
            className="px-4 py-3 flex items-center gap-3"
          >
            <div className="flex-1 min-w-0">
              <div className="flex items-center gap-2">
                <span className="font-mono text-sm font-medium">
                  v{r.version}
                </span>
                <span className="text-[11px] text-gray-500">
                  {new Date(r.created_at).toLocaleDateString()}
                </span>
                <span className="text-[11px] text-gray-500">
                  · {formatBytes(r.size_bytes)}
                </span>
              </div>
              {r.changelog && (
                <p className="text-xs text-gray-400 mt-0.5 line-clamp-2">
                  {r.changelog}
                </p>
              )}
              <p
                className="text-[10px] font-mono text-gray-600 truncate mt-0.5"
                title={r.sha256}
              >
                sha256: {r.sha256.slice(0, 16)}…
              </p>
            </div>
            <a
              href={`/api/store/releases/${app.slug}/${r.version}/download`}
              className="shrink-0 inline-flex items-center gap-1.5 px-3 py-1.5 bg-gray-800 hover:bg-gray-700 border border-gray-700 rounded text-xs text-amber-400"
            >
              <Download className="w-3.5 h-3.5" />
              APK
            </a>
          </li>
        ))}
      </ul>
    </div>
  );
}
