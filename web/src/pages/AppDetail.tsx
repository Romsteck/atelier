import { useEffect, useState } from "react";
import { Link, useParams } from "react-router-dom";
import {
  Boxes,
  ChevronLeft,
  Code2,
  ExternalLink,
  GitBranch,
  Database,
  BookOpen,
} from "lucide-react";
import { App, getApp } from "../api";

function Field({ label, value }: { label: string; value: React.ReactNode }) {
  return (
    <div className="flex gap-3 py-1.5 border-b border-gray-800 last:border-0">
      <span className="text-xs text-gray-500 uppercase tracking-wider w-32 shrink-0">
        {label}
      </span>
      <span className="text-sm text-gray-200 break-all">{value}</span>
    </div>
  );
}

export default function AppDetail() {
  const { slug } = useParams();
  const [app, setApp] = useState<App | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    if (!slug) return;
    getApp(slug)
      .then(setApp)
      .catch((e) => setError(String(e)));
  }, [slug]);

  if (error) return <p className="text-red-400">Erreur: {error}</p>;
  if (!app) return <p className="text-gray-500">Chargement…</p>;

  const envEntries = Object.entries(app.env_vars);

  return (
    <div className="max-w-4xl">
      <Link
        to="/apps"
        className="inline-flex items-center gap-1 text-sm text-gray-400 hover:text-amber-400 mb-4"
      >
        <ChevronLeft className="w-4 h-4" />
        Toutes les apps
      </Link>

      <div className="flex items-center gap-2 mb-2">
        <Boxes className="w-5 h-5 text-amber-400" />
        <h2 className="text-2xl font-semibold">{app.name}</h2>
      </div>

      <div className="flex flex-wrap gap-2 mb-6">
        <Link
          to={`/studio?app=${app.slug}`}
          className="inline-flex items-center gap-1.5 px-3 py-1.5 bg-amber-400/10 border border-amber-400/30 rounded text-xs text-amber-400 hover:bg-amber-400/20"
        >
          <Code2 className="w-3.5 h-3.5" /> Studio
        </Link>
        <Link
          to={`/docs/${app.slug}`}
          className="inline-flex items-center gap-1.5 px-3 py-1.5 bg-gray-800 hover:bg-gray-700 border border-gray-700 rounded text-xs text-gray-300"
        >
          <BookOpen className="w-3.5 h-3.5" /> Docs
        </Link>
        <Link
          to={`/git/${app.slug}`}
          className="inline-flex items-center gap-1.5 px-3 py-1.5 bg-gray-800 hover:bg-gray-700 border border-gray-700 rounded text-xs text-gray-300"
        >
          <GitBranch className="w-3.5 h-3.5" /> Git
        </Link>
        {app.db_backend && app.db_backend !== "legacy-sqlite" && (
          <Link
            to={`/dataverse/${app.slug}`}
            className="inline-flex items-center gap-1.5 px-3 py-1.5 bg-gray-800 hover:bg-gray-700 border border-gray-700 rounded text-xs text-gray-300"
          >
            <Database className="w-3.5 h-3.5" /> Dataverse
          </Link>
        )}
        <a
          href={`https://${app.domain}`}
          target="_blank"
          rel="noopener noreferrer"
          className="inline-flex items-center gap-1.5 px-3 py-1.5 bg-gray-800 hover:bg-gray-700 border border-gray-700 rounded text-xs text-gray-300"
        >
          <ExternalLink className="w-3.5 h-3.5" /> Ouvrir l'app
        </a>
        <a
          href={`https://proxy.mynetwk.biz/studio?app=${app.slug}`}
          target="_blank"
          rel="noopener noreferrer"
          className="inline-flex items-center gap-1.5 px-3 py-1.5 bg-gray-800 hover:bg-gray-700 border border-gray-700 rounded text-xs text-amber-400/80"
          title="Build / deploy / start / stop restent sur homeroute jusqu'au cutover Phase 9"
        >
          <ExternalLink className="w-3.5 h-3.5" /> Lifecycle (homeroute)
        </a>
      </div>

      <section className="bg-gray-900 border border-gray-800 rounded-md p-4 mb-6">
        <h3 className="text-sm font-semibold text-gray-400 uppercase tracking-wider mb-2">
          Métadonnées
        </h3>
        <Field label="Slug" value={<code className="font-mono">{app.slug}</code>} />
        <Field label="Stack" value={app.stack} />
        <Field label="State" value={app.state} />
        <Field label="Visibility" value={app.visibility} />
        <Field label="Domain" value={<code className="font-mono">{app.domain}</code>} />
        <Field label="Port" value={<code className="font-mono">{app.port}</code>} />
        <Field
          label="DB backend"
          value={app.db_backend || "—"}
        />
        <Field label="Sources on" value={app.sources_on || "—"} />
        <Field label="Health path" value={<code className="font-mono">{app.health_path}</code>} />
        <Field label="Created" value={new Date(app.created_at).toLocaleString()} />
        <Field label="Updated" value={new Date(app.updated_at).toLocaleString()} />
      </section>

      <section className="bg-gray-900 border border-gray-800 rounded-md p-4 mb-6">
        <h3 className="text-sm font-semibold text-gray-400 uppercase tracking-wider mb-2">
          Build / Run
        </h3>
        <Field
          label="Build"
          value={<pre className="font-mono text-xs whitespace-pre-wrap">{app.build_command}</pre>}
        />
        <Field
          label="Run"
          value={<pre className="font-mono text-xs whitespace-pre-wrap">{app.run_command}</pre>}
        />
        <Field
          label="Artefact"
          value={<pre className="font-mono text-xs whitespace-pre-wrap">{app.build_artefact}</pre>}
        />
      </section>

      {envEntries.length > 0 && (
        <section className="bg-gray-900 border border-gray-800 rounded-md p-4">
          <h3 className="text-sm font-semibold text-gray-400 uppercase tracking-wider mb-2">
            Environment ({envEntries.length})
          </h3>
          <ul className="font-mono text-xs">
            {envEntries.map(([k, v]) => (
              <li key={k} className="py-1 border-b border-gray-800 last:border-0">
                <span className="text-amber-400">{k}</span>
                <span className="text-gray-500">=</span>
                <span className="text-gray-300">{v}</span>
              </li>
            ))}
          </ul>
        </section>
      )}
    </div>
  );
}
