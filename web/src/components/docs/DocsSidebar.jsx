import { useMemo, useState } from 'react';
import { ChevronRight, ChevronDown, Compass, Layout, Layers, Boxes, FileText } from 'lucide-react';

const STORAGE_KEY = 'docsSidebar:openGroups';

function loadOpen() {
  try {
    const raw = localStorage.getItem(STORAGE_KEY);
    if (!raw) return null;
    return JSON.parse(raw);
  } catch {
    return null;
  }
}

function saveOpen(state) {
  try { localStorage.setItem(STORAGE_KEY, JSON.stringify(state)); } catch { /* noop */ }
}

function GroupHeader({ icon: Icon, label, count, open, onToggle }) {
  return (
    <button
      onClick={onToggle}
      className="w-full flex items-center justify-between px-2 py-1.5 text-xs uppercase tracking-wider
                 text-gray-400 hover:text-gray-200 hover:bg-gray-800/40 rounded"
    >
      <span className="flex items-center gap-1.5">
        {open ? <ChevronDown className="w-3 h-3" /> : <ChevronRight className="w-3 h-3" />}
        <Icon className="w-3.5 h-3.5" />
        {label}
      </span>
      <span className="text-gray-500">{count}</span>
    </button>
  );
}

function EntryItem({ entry, selected, onSelect }) {
  const isSelected =
    selected &&
    selected.type === entry.doc_type &&
    selected.name === entry.name;
  return (
    <button
      onClick={() => onSelect({ type: entry.doc_type, name: entry.name })}
      className={`w-full text-left px-2 py-1.5 rounded text-sm flex items-start gap-2 transition-colors
                  ${isSelected
                    ? 'bg-blue-500/20 text-white border-l-2 border-blue-500'
                    : 'text-gray-300 hover:bg-gray-800/60'}
                  ${entry.has_diagram ? 'pr-6' : ''}`}
      title={entry.summary || entry.title || entry.name}
    >
      <span className="flex-1 min-w-0">
        <span className="font-medium truncate block">
          {entry.title || entry.name}
        </span>
        {entry.summary && (
          <span className="text-xs text-gray-500 truncate block">{entry.summary}</span>
        )}
      </span>
      {entry.has_diagram && (
        <span className="text-blue-400 text-[10px] mt-0.5" title="Diagramme attaché">◇</span>
      )}
    </button>
  );
}

export default function DocsSidebar({ overview, selected, onSelect }) {
  const initialOpen = loadOpen() || {
    overview: true,
    screens: true,
    features_global: true,
    features_per_screen: true,
    components: true,
  };
  const [open, setOpen] = useState(initialOpen);

  const toggle = (key) => {
    setOpen((prev) => {
      const next = { ...prev, [key]: !prev[key] };
      saveOpen(next);
      return next;
    });
  };

  const screens = overview?.index?.screens || [];
  const features = overview?.index?.features || [];
  const components = overview?.index?.components || [];

  // Split features by scope.
  const globalFeatures = features.filter((f) => f.scope === 'global');
  const perScreen = features.filter((f) => f.scope !== 'global');
  const perScreenGroups = useMemo(() => {
    const map = new Map();
    for (const f of perScreen) {
      const key = f.parent_screen || 'autres';
      if (!map.has(key)) map.set(key, []);
      map.get(key).push(f);
    }
    return [...map.entries()].sort((a, b) => a[0].localeCompare(b[0]));
  }, [perScreen]);

  const isOverviewSelected = selected?.type === 'overview';

  return (
    <aside className="w-60 flex-shrink-0 bg-gray-900/40 border-r border-gray-800 overflow-y-auto p-2">
      {/* Overview */}
      <button
        onClick={() => onSelect({ type: 'overview', name: 'overview' })}
        className={`w-full mb-2 px-2 py-2 rounded text-sm flex items-center gap-2 transition-colors
                    ${isOverviewSelected
                      ? 'bg-blue-500/20 text-white border-l-2 border-blue-500'
                      : 'text-gray-200 hover:bg-gray-800/60'}`}
      >
        <Compass className="w-4 h-4" />
        <span className="font-medium">Vue d'ensemble</span>
      </button>

      {/* Écrans */}
      <GroupHeader
        icon={Layout}
        label="Écrans"
        count={screens.length}
        open={open.screens}
        onToggle={() => toggle('screens')}
      />
      {open.screens && (
        <div className="mb-2 ml-1">
          {screens.length === 0 && <p className="px-2 py-1 text-xs text-gray-600 italic">aucun écran</p>}
          {screens.map((e) => (
            <EntryItem key={`screen-${e.name}`} entry={e} selected={selected} onSelect={onSelect} />
          ))}
        </div>
      )}

      {/* Features globales */}
      <GroupHeader
        icon={Layers}
        label="Features globales"
        count={globalFeatures.length}
        open={open.features_global}
        onToggle={() => toggle('features_global')}
      />
      {open.features_global && (
        <div className="mb-2 ml-1">
          {globalFeatures.length === 0 && (
            <p className="px-2 py-1 text-xs text-gray-600 italic">aucune feature globale</p>
          )}
          {globalFeatures.map((e) => (
            <EntryItem key={`feature-g-${e.name}`} entry={e} selected={selected} onSelect={onSelect} />
          ))}
        </div>
      )}

      {/* Features per-screen, groupées par écran parent */}
      <GroupHeader
        icon={Layers}
        label="Features par écran"
        count={perScreen.length}
        open={open.features_per_screen}
        onToggle={() => toggle('features_per_screen')}
      />
      {open.features_per_screen && (
        <div className="mb-2 ml-1">
          {perScreenGroups.length === 0 && (
            <p className="px-2 py-1 text-xs text-gray-600 italic">aucune feature per-screen</p>
          )}
          {perScreenGroups.map(([screen, list]) => (
            <div key={screen} className="mt-1">
              <div className="px-2 py-1 text-[10px] uppercase tracking-wider text-violet-400/70 flex items-center gap-1">
                <FileText className="w-3 h-3" />
                {screen}
              </div>
              {list.map((e) => (
                <EntryItem key={`feature-s-${screen}-${e.name}`} entry={e} selected={selected} onSelect={onSelect} />
              ))}
            </div>
          ))}
        </div>
      )}

      {/* Composants */}
      <GroupHeader
        icon={Boxes}
        label="Composants"
        count={components.length}
        open={open.components}
        onToggle={() => toggle('components')}
      />
      {open.components && (
        <div className="ml-1">
          {components.length === 0 && (
            <p className="px-2 py-1 text-xs text-gray-600 italic">aucun composant</p>
          )}
          {components.map((e) => (
            <EntryItem key={`component-${e.name}`} entry={e} selected={selected} onSelect={onSelect} />
          ))}
        </div>
      )}
    </aside>
  );
}
