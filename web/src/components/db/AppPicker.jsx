import { Database, Table2, ChevronRight } from 'lucide-react';

/**
 * Écran de sélection d'app pour la vue globale Base de données.
 *
 * Parcours « choisir l'app d'abord » : on liste les apps avec base, puis le clic
 * scope le sidebar aux tables de cette app (les `COUNT(*)` ne sont alors chargés
 * que pour l'app retenue).
 */
export function AppPicker({ apps, onSelect, loading }) {
  if (loading) {
    return (
      <div className="flex-1 p-6">
        <div className="grid grid-cols-2 md:grid-cols-3 gap-3 max-w-3xl">
          {[1, 2, 3, 4].map(i => (
            <div key={i} className="h-20 rounded-lg bg-gray-800/50 border border-gray-700 animate-pulse" />
          ))}
        </div>
      </div>
    );
  }

  if (!apps || apps.length === 0) {
    return (
      <div className="flex-1 flex flex-col items-center justify-center text-gray-500">
        <Database className="w-12 h-12 mb-3 opacity-20" />
        <p className="text-sm">Aucune app avec base de données</p>
      </div>
    );
  }

  return (
    <div className="flex-1 overflow-y-auto p-6">
      <div className="text-xs text-gray-500 mb-4">Choisissez une application pour explorer ses tables.</div>
      <div className="grid grid-cols-2 md:grid-cols-3 gap-3 max-w-3xl">
        {apps.map(({ app, tableCount }) => (
          <button
            key={app.slug}
            onClick={() => onSelect(app.slug)}
            className="group flex items-center gap-3 px-4 py-3 rounded-lg bg-gray-800/50 border border-gray-700 hover:border-blue-500/50 hover:bg-gray-800 cursor-pointer text-left transition-colors"
          >
            <div className="w-9 h-9 rounded-md bg-blue-500/15 flex items-center justify-center shrink-0">
              <Database className="w-4.5 h-4.5 text-blue-400" />
            </div>
            <div className="min-w-0 flex-1">
              <div className="text-sm font-medium text-gray-100 truncate">{app.name}</div>
              <div className="flex items-center gap-1 text-[11px] text-gray-500">
                <Table2 className="w-3 h-3" />
                {tableCount == null ? '…' : `${tableCount} table${tableCount > 1 ? 's' : ''}`}
              </div>
            </div>
            <ChevronRight className="w-4 h-4 text-gray-600 group-hover:text-blue-400 shrink-0" />
          </button>
        ))}
      </div>
    </div>
  );
}
