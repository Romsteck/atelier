import { useEffect, useMemo, useState } from "react";
import { Link, useParams } from "react-router-dom";
import { ChevronLeft, Database, Key, Lock as LockIcon } from "lucide-react";
import { DvListResult, DvSchema, getDvSchema, listDvRows } from "../api";

const TOKEN_STORAGE_KEY = (slug: string) => `atelier:dv-token:${slug}`;

export default function DataverseSchemaPage() {
  const { slug } = useParams();
  const [token, setToken] = useState<string>(() =>
    slug ? localStorage.getItem(TOKEN_STORAGE_KEY(slug)) ?? "" : "",
  );
  const [schema, setSchema] = useState<DvSchema | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [activeTable, setActiveTable] = useState<string | null>(null);
  const [rows, setRows] = useState<DvListResult | null>(null);
  const [rowError, setRowError] = useState<string | null>(null);

  useEffect(() => {
    if (!slug || !token) return;
    setError(null);
    setSchema(null);
    getDvSchema(slug, token)
      .then((s) => {
        setSchema(s);
        if (s.tables.length > 0) setActiveTable(s.tables[0].name);
      })
      .catch((e) => setError(String(e)));
  }, [slug, token]);

  useEffect(() => {
    if (!slug || !token || !activeTable) return;
    setRowError(null);
    setRows(null);
    listDvRows(slug, activeTable, token, { top: 50, count: true })
      .then(setRows)
      .catch((e) => setRowError(String(e)));
  }, [slug, token, activeTable]);

  const persistToken = (v: string) => {
    setToken(v);
    if (slug) {
      if (v) localStorage.setItem(TOKEN_STORAGE_KEY(slug), v);
      else localStorage.removeItem(TOKEN_STORAGE_KEY(slug));
    }
  };

  const userColumns = useMemo(() => {
    if (!schema || !activeTable) return [];
    const t = schema.tables.find((x) => x.name === activeTable);
    return t?.columns.filter((c) => !c.is_system) ?? [];
  }, [schema, activeTable]);

  const allColumns = useMemo(() => {
    if (!schema || !activeTable) return [];
    return schema.tables.find((x) => x.name === activeTable)?.columns ?? [];
  }, [schema, activeTable]);

  return (
    <div className="max-w-6xl">
      <Link
        to="/dataverse"
        className="inline-flex items-center gap-1 text-sm text-gray-400 hover:text-amber-400 mb-4"
      >
        <ChevronLeft className="w-4 h-4" />
        Toutes les bases
      </Link>
      <div className="flex items-center gap-2 mb-2">
        <Database className="w-5 h-5 text-amber-400" />
        <h2 className="text-2xl font-semibold font-mono">app_{slug}</h2>
      </div>

      <section className="bg-gray-900 border border-gray-800 rounded-md p-4 mb-4">
        <label className="text-xs text-gray-400 uppercase tracking-wider block mb-1">
          <LockIcon className="w-3 h-3 inline mr-1" />
          Bearer token (gateway_token, conservé en localStorage)
        </label>
        <input
          type="password"
          value={token}
          onChange={(e) => persistToken(e.target.value)}
          placeholder="Coller le gateway_token de l'app"
          className="w-full bg-gray-950 border border-gray-700 rounded px-3 py-1.5 text-sm font-mono text-gray-100 focus:outline-none focus:border-amber-400"
        />
        <p className="text-[11px] text-gray-600 mt-1">
          Source du token : <code>/opt/homeroute/data/dataverse-secrets.json</code>{" "}
          (sur Medion ou rsync vers /var/lib/atelier/state).
        </p>
      </section>

      {!token && (
        <p className="text-sm text-gray-500">
          Saisis le bearer token pour explorer la base.
        </p>
      )}
      {error && <p className="text-sm text-red-400">Erreur: {error}</p>}

      {schema && (
        <div className="grid grid-cols-1 lg:grid-cols-[220px_1fr] gap-4">
          <aside className="bg-gray-900 border border-gray-800 rounded-md p-2">
            <h3 className="text-[10px] text-gray-500 uppercase tracking-wider px-2 py-1">
              Tables ({schema.tables.length})
            </h3>
            <ul className="text-sm font-mono">
              {schema.tables.map((t) => (
                <li key={t.name}>
                  <button
                    onClick={() => setActiveTable(t.name)}
                    className={`w-full text-left px-2 py-1 rounded ${
                      activeTable === t.name
                        ? "bg-amber-400/10 text-amber-400"
                        : "text-gray-300 hover:bg-gray-800"
                    }`}
                  >
                    {t.name}
                    <span className="text-[10px] text-gray-600 ml-1">
                      ({t.columns.filter((c) => !c.is_system).length})
                    </span>
                  </button>
                </li>
              ))}
            </ul>
          </aside>

          <main className="min-w-0">
            {activeTable && (
              <>
                <h3 className="text-base font-semibold mb-2 font-mono">
                  {activeTable}
                </h3>
                <details className="mb-3 bg-gray-900 border border-gray-800 rounded">
                  <summary className="cursor-pointer text-xs text-gray-400 px-3 py-2">
                    Colonnes ({allColumns.length}) ·{" "}
                    {userColumns.length} user / {allColumns.length - userColumns.length} système
                  </summary>
                  <ul className="text-xs font-mono px-3 pb-2">
                    {allColumns.map((c) => (
                      <li
                        key={c.name}
                        className={`py-1 flex gap-2 items-baseline ${
                          c.is_system ? "text-gray-600" : "text-gray-200"
                        }`}
                      >
                        {c.is_primary_key && (
                          <Key className="w-3 h-3 text-amber-400 shrink-0" />
                        )}
                        <span className="text-amber-400">{c.name}</span>
                        <span className="text-gray-500">{c.pg_type}</span>
                        {!c.nullable && (
                          <span className="text-[10px] text-gray-600">NOT NULL</span>
                        )}
                      </li>
                    ))}
                  </ul>
                </details>

                {rowError && (
                  <p className="text-sm text-red-400">Erreur: {rowError}</p>
                )}
                {!rows && !rowError && (
                  <p className="text-sm text-gray-500">Chargement…</p>
                )}
                {rows && (
                  <div className="bg-gray-900 border border-gray-800 rounded-md overflow-hidden">
                    <div className="px-3 py-2 text-[11px] text-gray-500 border-b border-gray-800 font-mono flex justify-between">
                      <span>
                        Top 50 sur{" "}
                        {rows["@count"] != null ? rows["@count"] : "?"} lignes
                      </span>
                      <span>
                        {userColumns.length} columns
                      </span>
                    </div>
                    <div className="overflow-x-auto">
                      <table className="text-xs font-mono w-full">
                        <thead>
                          <tr className="border-b border-gray-800 text-gray-400">
                            {userColumns.map((c) => (
                              <th
                                key={c.name}
                                className="text-left px-3 py-1.5 whitespace-nowrap"
                              >
                                {c.name}
                              </th>
                            ))}
                          </tr>
                        </thead>
                        <tbody>
                          {rows.value.map((r, i) => (
                            <tr
                              key={i}
                              className="border-b border-gray-800/60 hover:bg-gray-800/30"
                            >
                              {userColumns.map((c) => {
                                const v = r[c.name];
                                let display: string;
                                if (v == null) display = "—";
                                else if (typeof v === "object")
                                  display = JSON.stringify(v);
                                else display = String(v);
                                return (
                                  <td
                                    key={c.name}
                                    className="px-3 py-1.5 max-w-xs truncate text-gray-200"
                                    title={display}
                                  >
                                    {v == null ? (
                                      <span className="text-gray-600">—</span>
                                    ) : (
                                      display
                                    )}
                                  </td>
                                );
                              })}
                            </tr>
                          ))}
                          {rows.value.length === 0 && (
                            <tr>
                              <td
                                colSpan={userColumns.length}
                                className="px-3 py-3 text-center text-gray-500"
                              >
                                Aucune ligne.
                              </td>
                            </tr>
                          )}
                        </tbody>
                      </table>
                    </div>
                  </div>
                )}
              </>
            )}
          </main>
        </div>
      )}
    </div>
  );
}
