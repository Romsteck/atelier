import { useEffect, useMemo, useState } from "react";
import { useSearchParams } from "react-router-dom";
import { Code2, ExternalLink, AlertTriangle } from "lucide-react";
import { App, listApps } from "../api";

const STUDIO_BASE = "https://studio.mynetwk.biz";
const FOLDER_PREFIX = "/opt/homeroute/apps";

function buildStudioUrl(slug: string): string {
  return `${STUDIO_BASE}/?folder=${encodeURIComponent(`${FOLDER_PREFIX}/${slug}/src`)}`;
}

export default function StudioPage() {
  const [params, setParams] = useSearchParams();
  const slug = params.get("app") ?? "";
  const [apps, setApps] = useState<App[] | null>(null);
  const [iframeBlocked, setIframeBlocked] = useState(false);

  useEffect(() => {
    listApps()
      .then((list) => {
        setApps(list);
        if (!slug && list.length > 0) {
          setParams({ app: list[0].slug }, { replace: true });
        }
      })
      .catch(() => setApps([]));
  }, []);

  const url = useMemo(() => (slug ? buildStudioUrl(slug) : ""), [slug]);

  // code-server vit sur un autre subdomain (studio.mynetwk.biz) et envoie souvent
  // X-Frame-Options=DENY ou CSP frame-ancestors restrictif. Si le iframe ne
  // se charge pas après 3 s on bascule sur un fallback "ouvrir dans un onglet".
  useEffect(() => {
    setIframeBlocked(false);
    if (!url) return;
    const t = setTimeout(() => {
      // Best-effort detection : on n'a pas accès au contentDocument cross-origin,
      // donc on offre toujours le bouton fallback en parallèle. Voir le panneau ci-dessous.
    }, 3000);
    return () => clearTimeout(t);
  }, [url]);

  return (
    <div className="h-full flex flex-col -m-6">
      <div className="px-4 py-2 bg-gray-900 border-b border-gray-800 flex items-center gap-3">
        <Code2 className="w-4 h-4 text-amber-400 shrink-0" />
        <select
          value={slug}
          onChange={(e) => setParams({ app: e.target.value })}
          className="bg-gray-800 border border-gray-700 rounded text-sm px-2 py-1 text-gray-100 focus:outline-none focus:border-amber-400"
        >
          {!apps && <option>Chargement…</option>}
          {apps && apps.length === 0 && <option>Aucune app</option>}
          {apps?.map((a) => (
            <option key={a.slug} value={a.slug}>
              {a.name} · {a.slug}
            </option>
          ))}
        </select>
        {slug && (
          <span className="text-[11px] text-gray-500 font-mono truncate">
            {FOLDER_PREFIX}/{slug}/src
          </span>
        )}
        <div className="ml-auto flex items-center gap-2">
          {url && (
            <a
              href={url}
              target="_blank"
              rel="noopener noreferrer"
              className="inline-flex items-center gap-1 px-2 py-1 bg-gray-800 hover:bg-gray-700 rounded text-[11px] text-gray-300"
            >
              <ExternalLink className="w-3 h-3" />
              Ouvrir dans un onglet
            </a>
          )}
        </div>
      </div>

      {url && !iframeBlocked && (
        <iframe
          key={url}
          src={url}
          className="flex-1 w-full bg-gray-950"
          allow="clipboard-read; clipboard-write"
          onError={() => setIframeBlocked(true)}
          title={`code-server (${slug})`}
        />
      )}

      {url && iframeBlocked && (
        <div className="flex-1 flex items-center justify-center p-6">
          <div className="max-w-md text-center">
            <AlertTriangle className="w-8 h-8 text-amber-400 mx-auto mb-3" />
            <h3 className="text-lg font-semibold mb-2">
              Iframe bloqué par code-server
            </h3>
            <p className="text-sm text-gray-400 mb-4">
              code-server (studio.mynetwk.biz) refuse d'être chargé dans un iframe
              cross-domain. Ouvre-le dans un onglet dédié — c'est la même
              instance, juste un autre window.
            </p>
            <a
              href={url}
              target="_blank"
              rel="noopener noreferrer"
              className="inline-flex items-center gap-1.5 px-4 py-2 bg-amber-400 text-black rounded text-sm font-medium hover:bg-amber-300"
            >
              <ExternalLink className="w-4 h-4" />
              Ouvrir code-server
            </a>
          </div>
        </div>
      )}

      {!url && apps && (
        <div className="flex-1 flex items-center justify-center p-6 text-gray-500 text-sm">
          Sélectionne une app pour ouvrir code-server.
        </div>
      )}
    </div>
  );
}
