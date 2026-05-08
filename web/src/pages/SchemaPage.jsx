import { useState, useEffect } from 'react';
import { useSearchParams } from 'react-router-dom';
import { listApps, getAppDbTables } from '../api/client';
import { SchemaEditor } from '../components/db/SchemaEditor';
import { Database, Loader2 } from 'lucide-react';

function unwrap(res) {
  const d = res.data;
  return d && typeof d === 'object' && 'data' in d ? d.data : d;
}

export default function SchemaPage() {
  const [searchParams, setSearchParams] = useSearchParams();
  const selectedApp = searchParams.get('app') || null;

  const [apps, setApps] = useState([]);
  const [loading, setLoading] = useState(true);

  useEffect(() => {
    setLoading(true);
    listApps()
      .then(async (res) => {
        const all = unwrap(res)?.apps || unwrap(res) || [];
        const dbApps = (Array.isArray(all) ? all : []).filter(a => a.has_db);

        const results = await Promise.all(
          dbApps.map(async (app) => {
            try {
              const tablesRes = await getAppDbTables(app.slug);
              const raw = unwrap(tablesRes);
              const tables = raw?.tables || (Array.isArray(raw) ? raw : []);
              return { ...app, tableCount: tables.length };
            } catch {
              return { ...app, tableCount: 0 };
            }
          })
        );
        setApps(results);

        if (!selectedApp && results.length > 0) {
          setSearchParams({ app: results[0].slug }, { replace: true });
        }
      })
      .catch(() => {})
      .finally(() => setLoading(false));
  }, []); // eslint-disable-line

  return (
    <div className="flex h-full overflow-hidden rounded border border-gray-700">
      {/* App selector sidebar */}
      <div className="w-48 border-r border-gray-700 flex flex-col bg-gray-800/30 shrink-0">
        <div className="px-3 py-2 border-b border-gray-700">
          <span className="text-xs font-semibold text-gray-400 uppercase tracking-wider">Applications</span>
        </div>
        <div className="flex-1 overflow-y-auto">
          {loading ? (
            <div className="flex items-center justify-center py-8 text-gray-500">
              <Loader2 className="w-4 h-4 animate-spin" />
            </div>
          ) : apps.length === 0 ? (
            <div className="px-3 py-4 text-xs text-gray-500">Aucune app avec DB</div>
          ) : (
            apps.map(app => {
              const isPg = app.db_backend === 'postgres-dataverse';
              return (
                <button
                  key={app.slug}
                  onClick={() => setSearchParams({ app: app.slug })}
                  className={`w-full text-left px-3 py-2 text-sm border-none cursor-pointer flex items-center gap-2 ${
                    selectedApp === app.slug
                      ? 'bg-blue-500/10 text-blue-400'
                      : 'bg-transparent text-gray-400 hover:bg-gray-700/30 hover:text-white'
                  }`}
                  title={isPg ? 'Backend: postgres + GraphQL' : 'Backend: SQLite legacy'}
                >
                  <Database className={`w-3.5 h-3.5 shrink-0 ${isPg ? 'text-emerald-400' : ''}`} />
                  <span className="flex-1 truncate">{app.slug}</span>
                  {isPg && <span className="text-[9px] text-emerald-400 uppercase">pg</span>}
                  <span className="text-[10px] text-gray-600">{app.tableCount}</span>
                </button>
              );
            })
          )}
        </div>
      </div>

      {/* Schema editor */}
      <div className="flex-1 min-w-0">
        {selectedApp ? (
          <SchemaEditor appSlug={selectedApp} />
        ) : (
          <div className="flex flex-col items-center justify-center h-full text-gray-500">
            <Database className="w-12 h-12 mb-3 opacity-20" />
            <p className="text-sm">Selectionnez une application</p>
          </div>
        )}
      </div>
    </div>
  );
}
