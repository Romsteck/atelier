import { useState, useCallback, useRef, useEffect } from 'react';
import { FolderTree, GitBranch } from 'lucide-react';
import AgentPanel from './AgentPanel';
import FilesTab from './FilesTab';
import GitTab from './GitTab';

// « Notre code-server » en disposition VS Code :
//   [barre d'activité] [sidebar repliable/redimensionnable : Explorateur ou Git] [chat au centre]
// Le chat (AgentPanel) reste TOUJOURS monté au centre (conversation + WebSocket
// préservés) ; la sidebar montre l'explorateur OU le contrôle de source (working
// tree local), togglés depuis la barre d'activité.
const PANELS = [
  { id: 'files', label: 'Explorateur', Icon: FolderTree },
  { id: 'git', label: 'Contrôle de source (local)', Icon: GitBranch },
];

export default function AgentWorkspace({ slug }) {
  const [panel, setPanel] = useState(null); // null = sidebar repliée
  const [opened, setOpened] = useState(() => new Set()); // panneaux déjà montés
  const [dragging, setDragging] = useState(false);
  const [width, setWidth] = useState(() => {
    const v = parseInt(localStorage.getItem('agent:sidebarW'), 10);
    return Number.isFinite(v) && v >= 220 && v <= 760 ? v : 340;
  });
  const rootRef = useRef(null);

  useEffect(() => { localStorage.setItem('agent:sidebarW', String(width)); }, [width]);

  const toggle = useCallback((id) => {
    setPanel((cur) => (cur === id ? null : id));
    setOpened((s) => (s.has(id) ? s : new Set(s).add(id)));
  }, []);

  // Redimensionnement de la sidebar (pointer). Overlay plein écran pendant le drag
  // pour capter le pointeur même au-dessus des iframes (code-server / browser).
  useEffect(() => {
    if (!dragging) return;
    function onMove(e) {
      if (!rootRef.current) return;
      const rect = rootRef.current.getBoundingClientRect();
      const maxW = Math.max(260, rect.width - 44 - 260); // garde ≥260px au chat
      const w = e.clientX - rect.left - 44; // - barre d'activité
      setWidth(Math.max(220, Math.min(maxW, w)));
    }
    function onUp() { setDragging(false); }
    window.addEventListener('pointermove', onMove);
    window.addEventListener('pointerup', onUp);
    return () => {
      window.removeEventListener('pointermove', onMove);
      window.removeEventListener('pointerup', onUp);
    };
  }, [dragging]);

  return (
    <div ref={rootRef} className="flex h-full min-h-0 bg-gray-900 relative">
      {/* Barre d'activité */}
      <div className="w-11 shrink-0 border-r border-gray-800 flex flex-col items-center py-2 gap-1">
        {PANELS.map(({ id, label, Icon }) => (
          <button key={id} onClick={() => toggle(id)} title={label}
            className={`relative w-9 h-9 flex items-center justify-center rounded-md transition-colors ${
              panel === id ? 'text-blue-400 bg-gray-700/60' : 'text-gray-500 hover:text-gray-200 hover:bg-gray-800'
            }`}>
            {panel === id && <span className="absolute left-0 top-1.5 bottom-1.5 w-0.5 rounded-full bg-blue-400" />}
            <Icon className="w-5 h-5" />
          </button>
        ))}
      </div>

      {/* Sidebar — panneaux montés gardés montés (état préservé), un seul visible */}
      {panel && (
        <div className="shrink-0 min-h-0 border-r border-gray-800" style={{ width }}>
          {opened.has('files') && (
            <div className={panel === 'files' ? 'h-full' : 'hidden'}>
              <FilesTab slug={slug} active={panel === 'files'} />
            </div>
          )}
          {opened.has('git') && (
            <div className={panel === 'git' ? 'h-full' : 'hidden'}>
              <GitTab slug={slug} active={panel === 'git'} />
            </div>
          )}
        </div>
      )}

      {/* Poignée de redimensionnement */}
      {panel && (
        <div
          onPointerDown={(e) => { e.preventDefault(); setDragging(true); }}
          title="Redimensionner"
          className="w-1 shrink-0 cursor-col-resize bg-gray-800 hover:bg-blue-500/60" />
      )}

      {/* Chat — toujours au centre, jamais démonté */}
      <div className="flex-1 min-w-0 h-full">
        <AgentPanel slug={slug} />
      </div>

      {dragging && <div className="fixed inset-0 z-50 cursor-col-resize" />}
    </div>
  );
}
