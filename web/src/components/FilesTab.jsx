import { useState, useEffect, useCallback, useMemo, useRef } from 'react';
import {
  Folder, FolderOpen, File as FileIcon, ChevronRight, ChevronDown,
  Loader2,
} from 'lucide-react';
import { getSourceTree } from '../api/client';
import { useAgentConversations } from '../context/AgentConversationsContext';
import useWebSocket from '../hooks/useWebSocket';

// Explorateur du working tree de l'app (`…/{slug}/src`) — lecture seule, lazy
// (un niveau chargé par expansion). L'ouverture d'un fichier ne s'affiche plus dans
// la sidebar (trop étroit) : elle ouvre un onglet « fichier » dans le split central
// (cf. openFile du provider), façon éditeur VS Code.
//
// Rafraîchissement AUTOMATIQUE (plus de bouton manuel) : le backend détecte les
// changements via un watcher inotify et émet l'event WS `source:changed {slug}` ;
// on resynchronise alors l'arbre EN PLACE (sans remount → l'expansion est
// préservée, pas de spinner de fond) façon `useSourceGit`.

function TreeNode({ slug, entry, depth, onOpenFile, openPaths, refreshToken, convId }) {
  const [open, setOpen] = useState(false);
  const [children, setChildren] = useState(null); // null = pas encore chargé
  const [loading, setLoading] = useState(false);

  const toggle = useCallback(async () => {
    if (!entry.is_dir) {
      onOpenFile(entry);
      return;
    }
    const next = !open;
    setOpen(next);
    if (next && children === null) {
      setLoading(true);
      try {
        const r = await getSourceTree(slug, entry.path, convId);
        setChildren(r.data?.entries || []);
      } catch {
        setChildren([]);
      } finally {
        setLoading(false);
      }
    }
  }, [entry, open, children, slug, onOpenFile, convId]);

  // Resync en place sur signal backend : on ne re-fetch que les dossiers DÉJÀ
  // ouverts (children chargés), sans toucher à `open` → l'expansion est préservée.
  // React réconcilie par key={path} : les dossiers encore présents gardent leur
  // identité (donc leur état ouvert) ; les fichiers apparus/supprimés sont
  // ajoutés/retirés. En cas d'erreur on garde les enfants courants (pas de
  // blanchiment de l'arbre).
  useEffect(() => {
    if (refreshToken === 0) return;                          // pas de fetch au montage
    if (!entry.is_dir || !open || children === null) return; // dossiers ouverts seulement
    let alive = true;
    getSourceTree(slug, entry.path, convId)
      .then((r) => { if (alive) setChildren(r.data?.entries || []); })
      .catch(() => { /* garder les enfants courants */ });
    return () => { alive = false; };
  }, [refreshToken]); // eslint-disable-line react-hooks/exhaustive-deps

  const opened = !entry.is_dir && openPaths.has(entry.path);
  return (
    <div>
      <button
        onClick={toggle}
        title={entry.path}
        style={{ paddingLeft: depth * 12 + 8 }}
        className={`w-full flex items-center gap-1 py-[3px] pr-2 text-[13px] text-left rounded-sm ${
          opened ? 'bg-blue-500/20 text-blue-200' : 'text-gray-300 hover:bg-gray-700/40'
        }`}>
        {entry.is_dir ? (
          open ? <ChevronDown className="w-3.5 h-3.5 shrink-0 text-gray-500" />
               : <ChevronRight className="w-3.5 h-3.5 shrink-0 text-gray-500" />
        ) : (
          <span className="w-3.5 shrink-0" />
        )}
        {entry.is_dir ? (
          open ? <FolderOpen className="w-3.5 h-3.5 shrink-0 text-amber-400/80" />
               : <Folder className="w-3.5 h-3.5 shrink-0 text-amber-400/80" />
        ) : (
          <FileIcon className="w-3.5 h-3.5 shrink-0 text-gray-500" />
        )}
        <span className="truncate">{entry.name}</span>
      </button>
      {entry.is_dir && open && (
        loading ? (
          <div style={{ paddingLeft: (depth + 1) * 12 + 16 }} className="py-1 text-gray-600">
            <Loader2 className="w-3.5 h-3.5 animate-spin" />
          </div>
        ) : (
          (children || []).map((c) => (
            <TreeNode key={c.path} slug={slug} entry={c} depth={depth + 1}
              onOpenFile={onOpenFile} openPaths={openPaths} refreshToken={refreshToken} convId={convId} />
          ))
        )
      )}
    </div>
  );
}

export default function FilesTab({ slug, active = true, convId }) {
  const { order, convos, openFile } = useAgentConversations();
  const [root, setRoot] = useState([]);
  const [rootLoading, setRootLoading] = useState(true);
  const [refreshToken, setRefreshToken] = useState(0); // bump → resync en place (racine + dossiers ouverts)
  const loadedOnce = useRef(false); // spinner plein-panneau au 1er chargement par app seulement
  const debounce = useRef(null);

  // Fichiers actuellement ouverts en onglet → surlignés dans l'arbre.
  const openPaths = useMemo(
    () => new Set(order.filter((k) => convos[k]?.type === 'file').map((k) => convos[k].path)),
    [order, convos],
  );

  const loadRoot = useCallback(() => {
    if (!loadedOnce.current) setRootLoading(true); // spinner au 1er chargement seulement
    getSourceTree(slug, '', convId)
      .then((r) => setRoot(r.data?.entries || []))
      .catch(() => { /* on garde l'arbre courant en cas d'erreur de fond */ })
      .finally(() => { loadedOnce.current = true; setRootLoading(false); });
  }, [slug, convId]);

  // Reset du flag « déjà chargé » à chaque app OU worktree → spinner au changement.
  useEffect(() => { loadedOnce.current = false; }, [slug, convId]);

  // Charge à l'ouverture, au resync (refreshToken) et à chaque réactivation du pane.
  useEffect(() => { if (active) loadRoot(); }, [active, loadRoot, refreshToken]);

  // Le backend signale « quelque chose a changé sous {slug}/src » (watcher inotify)
  // → resync en place, debouncé pour coalescer les rafales (save-all, agent…).
  const bumpSoon = useCallback(() => {
    clearTimeout(debounce.current);
    debounce.current = setTimeout(() => setRefreshToken((k) => k + 1), 400);
  }, []);
  useWebSocket({
    'source:changed': (d) => { if (!d || d.slug === slug) bumpSoon(); },
  });
  useEffect(() => () => clearTimeout(debounce.current), []);

  return (
    <div className="flex flex-col h-full min-h-0 bg-gray-900">
      {/* En-tête explorateur */}
      <div className="flex items-center gap-2 h-[34px] shrink-0 px-3 border-b border-gray-800 text-[12px] text-gray-400">
        <span className="truncate uppercase tracking-wider text-[11px] text-gray-500" title={convId ? `branche conv/${convId}` : `${slug}/src`}>
          {slug}{convId ? ` · conv/${convId}` : '/src'}
        </span>
      </div>

      {/* Arbre — pleine hauteur (le contenu des fichiers s'ouvre dans le split central) */}
      <div className="flex-1 min-h-0 overflow-auto py-1">
        {rootLoading ? (
          <div className="flex items-center justify-center py-6 text-gray-600"><Loader2 className="w-4 h-4 animate-spin" /></div>
        ) : root.length === 0 ? (
          <div className="text-[12px] text-gray-600 text-center py-6">Vide</div>
        ) : (
          <div>
            {root.map((e) => (
              <TreeNode key={e.path} slug={slug} entry={e} depth={0}
                onOpenFile={openFile} openPaths={openPaths} refreshToken={refreshToken} convId={convId} />
            ))}
          </div>
        )}
      </div>
    </div>
  );
}
