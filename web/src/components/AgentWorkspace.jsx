import { useState, useCallback, useRef, useEffect } from 'react';
import { FolderTree, GitBranch, MessagesSquare } from 'lucide-react';
import FilesTab from './FilesTab';
import GitTab from './GitTab';
import ConversationsSplit from './agent/ConversationsSplit';
import ConversationsHistoryPanel from './agent/ConversationsHistoryPanel';
import { AgentConversationsProvider, useAgentConversations } from '../context/AgentConversationsContext';
import useSourceGit from '../hooks/useSourceGit';

// Espace de travail de l'app, disposition VS Code (remplace l'ancien éditeur code-server) :
//   [barre d'activité] [sidebar repliable : Explorateur / Git / Conversations] [split au centre]
// Le centre rend plusieurs conversations côte à côte (ConversationsSplit). La sidebar
// montre l'explorateur, le contrôle de source OU l'historique des conversations.
// Tout l'état des conversations vit dans AgentConversationsProvider (un seul WebSocket),
// monté une fois par slug. Le shell (sidebar + split) est un enfant DU provider pour
// pouvoir lire la conversation active (`activeConvId`) → la sidebar git/explorateur suit
// son worktree.
const PANELS = [
  { id: 'files', label: 'Explorateur', Icon: FolderTree },
  { id: 'git', label: 'Contrôle de source (local)', Icon: GitBranch },
  { id: 'history', label: 'Conversations', Icon: MessagesSquare },
];

export default function AgentWorkspace({ slug, launch, onLaunchConsumed }) {
  return (
    <AgentConversationsProvider slug={slug} launch={launch} onLaunchConsumed={onLaunchConsumed}>
      <WorkspaceShell slug={slug} />
    </AgentConversationsProvider>
  );
}

function WorkspaceShell({ slug }) {
  // Worktree de la conversation active → le panneau git/explorateur le suit (sinon src/).
  const { activeConvId } = useAgentConversations();
  const [panel, setPanel] = useState(null); // null = sidebar repliée
  const [opened, setOpened] = useState(() => new Set()); // panneaux déjà montés
  const [dragging, setDragging] = useState(false);
  // Largeur par défaut réduite (~30% de moins qu'avant) ; clé localStorage versionnée
  // (`…W2`) pour repartir de ce défaut même si une ancienne valeur (340) est stockée.
  const [width, setWidth] = useState(() => {
    const v = parseInt(localStorage.getItem('agent:sidebarW2'), 10);
    return Number.isFinite(v) && v >= 220 && v <= 760 ? v : 238;
  });
  const rootRef = useRef(null);
  const git = useSourceGit(slug, activeConvId); // status du worktree actif : badge + onglet Git

  useEffect(() => { localStorage.setItem('agent:sidebarW2', String(width)); }, [width]);

  const toggle = useCallback((id) => {
    setPanel((cur) => (cur === id ? null : id));
    setOpened((s) => (s.has(id) ? s : new Set(s).add(id)));
  }, []);

  // Redimensionnement de la sidebar (pointer). Overlay plein écran pendant le drag
  // pour capter le pointeur même au-dessus des iframes (preview/browser).
  useEffect(() => {
    if (!dragging) return;
    function onMove(e) {
      if (!rootRef.current) return;
      const rect = rootRef.current.getBoundingClientRect();
      const maxW = Math.max(260, rect.width - 48 - 260); // garde ≥260px au chat
      const w = e.clientX - rect.left - 48; // - barre d'activité (rail 48px)
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
      {/* Barre d'activité — icônes flush (carrées), taille VS Code (glyphe 24px / rail 48px). */}
      <div className="w-12 shrink-0 border-r border-gray-800 flex flex-col">
        {PANELS.map(({ id, label, Icon }) => (
          <button key={id} onClick={() => toggle(id)} aria-label={label}
            className={`group/act relative w-full h-12 flex items-center justify-center border-l-3 transition-[background-color,color] duration-300 ease-out hover:duration-0 ${
              panel === id
                ? 'border-amber-400 bg-gray-700/50 text-gray-50'
                : 'border-transparent text-gray-400 hover:bg-gray-700/30 hover:text-gray-100'
            }`}>
            <Icon className="w-6 h-6" />
            {id === 'git' && git.count > 0 && (
              <span className="absolute top-1 right-1 min-w-[16px] h-4 px-1 rounded-full bg-sky-500 text-white text-[10px] font-semibold leading-4 text-center shadow"
                aria-label={`${git.count} changement(s) en attente`}>
                {git.count > 99 ? '99+' : git.count}
              </span>
            )}
            <span className="pointer-events-none absolute left-full top-1/2 z-50 ml-2 -translate-y-1/2 whitespace-nowrap rounded-md bg-gray-950 px-2 py-1 text-xs font-medium text-gray-100 shadow-lg ring-1 ring-gray-700/80 opacity-0 transition-opacity duration-300 ease-out group-hover/act:opacity-100 group-hover/act:duration-0">
              {label}
              <span className="absolute left-0 top-1/2 h-2 w-2 -translate-x-1/2 -translate-y-1/2 rotate-45 bg-gray-950" />
            </span>
          </button>
        ))}
      </div>

      {/* Sidebar — panneaux montés gardés montés (état préservé), un seul visible */}
      {panel && (
        <div className="shrink-0 min-h-0 border-r border-gray-800" style={{ width }}>
          {opened.has('files') && (
            <div className={panel === 'files' ? 'h-full' : 'hidden'}>
              <FilesTab slug={slug} active={panel === 'files'} convId={activeConvId} />
            </div>
          )}
          {opened.has('git') && (
            <div className={panel === 'git' ? 'h-full' : 'hidden'}>
              <GitTab slug={slug} active={panel === 'git'} status={git.status} statusLoading={git.loading} onRefresh={git.refresh} convId={activeConvId} />
            </div>
          )}
          {opened.has('history') && (
            <div className={panel === 'history' ? 'h-full' : 'hidden'}>
              <ConversationsHistoryPanel active={panel === 'history'} />
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

      {/* Conversations — split à panneaux égaux (défaut), repli onglets si étroit */}
      <div className="flex-1 min-w-0 h-full">
        <ConversationsSplit />
      </div>

      {dragging && <div className="fixed inset-0 z-50 cursor-col-resize" />}
    </div>
  );
}
