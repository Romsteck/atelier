import { Database, ChevronRight, ChevronLeft } from 'lucide-react';

export function TableSidebar({ appsWithTables, selectedAppSlug, selectedTable, onSelectTable, onChangeApp, loading, open }) {
  // Desktop : colonne in-flow. Téléphone (<lg) : tiroir en overlay (slide), piloté
  // par `open` ; sinon il mange ~60% de la largeur et écrase la grille.
  const drawer = `transform transition-transform duration-200 ease-out absolute inset-y-0 left-0 z-40 w-64 max-w-[80vw] bg-gray-900 shadow-xl lg:static lg:z-auto lg:w-56 lg:max-w-none lg:shrink-0 lg:bg-transparent lg:shadow-none lg:translate-x-0 ${open ? 'translate-x-0' : '-translate-x-full'}`;

  if (loading) {
    return (
      <div className={`border-r border-gray-700 p-4 ${drawer}`}>
        <div className="animate-pulse space-y-3">
          {[1, 2, 3].map(i => <div key={i} className="h-6 bg-gray-700 rounded-sm" />)}
        </div>
      </div>
    );
  }

  return (
    <aside className={`border-r border-gray-700 overflow-y-auto flex flex-col ${drawer}`}>
      {onChangeApp ? (
        <button
          onClick={onChangeApp}
          className="flex items-center gap-1.5 px-3 pt-4 pb-2 text-[10px] font-semibold uppercase tracking-wider text-gray-500 hover:text-gray-300 border-none bg-transparent cursor-pointer text-left"
          title="Changer d'application"
        >
          <ChevronLeft className="w-3 h-3" /> Changer d'app
        </button>
      ) : (
        <div className="px-3 pt-4 pb-2 text-[10px] font-semibold uppercase tracking-wider text-gray-500">
          Bases de donnees
        </div>
      )}
      <div className="flex-1 overflow-y-auto px-2 pb-2">
        {appsWithTables.length === 0 && (
          <div className="text-xs text-gray-500 text-center py-4">Aucune app avec DB</div>
        )}
        {appsWithTables.map(({ app, tables }) => (
          <div key={app.slug} className="mb-3">
            <div className="flex items-center gap-2 px-2 py-1.5">
              <Database className="w-3.5 h-3.5 text-blue-400 shrink-0" />
              <span className="text-xs font-medium text-gray-300 truncate">{app.name}</span>
              <span className="text-[10px] text-gray-500 ml-auto">{tables.length}</span>
            </div>
            {tables.map(t => {
              const name = typeof t === 'string' ? t : t.name;
              const count = typeof t === 'object' ? t.row_count : null;
              const sel = selectedAppSlug === app.slug && selectedTable === name;
              return (
                <button
                  key={name}
                  onClick={() => onSelectTable(app.slug, name)}
                  className={`flex items-center justify-between w-full px-3 py-1.5 ml-2 rounded-sm text-xs text-left cursor-pointer border-none transition-colors ${
                    sel ? 'bg-blue-500/15 text-blue-400' : 'bg-transparent text-gray-400 hover:bg-gray-700/50 hover:text-gray-200'
                  }`}
                >
                  <span className="flex items-center gap-1 truncate">
                    {sel && <ChevronRight className="w-3 h-3 shrink-0" />}
                    {name}
                  </span>
                  {count != null && <span className="text-[10px] text-gray-600 tabular-nums">{count.toLocaleString()}</span>}
                </button>
              );
            })}
          </div>
        ))}
      </div>
    </aside>
  );
}
