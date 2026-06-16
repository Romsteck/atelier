import { useState, useEffect, useCallback, useMemo } from 'react';
import {
  Folder, FolderOpen, File as FileIcon, ChevronRight, ChevronDown,
  Loader2, RefreshCw,
} from 'lucide-react';
import { getSourceTree } from '../api/client';
import { useAgentConversations } from '../context/AgentConversationsContext';

// Explorateur du working tree de l'app (`…/{slug}/src`) — lecture seule, lazy
// (un niveau chargé par expansion). L'ouverture d'un fichier ne s'affiche plus dans
// la sidebar (trop étroit) : elle ouvre un onglet « fichier » dans le split central
// (cf. openFile du provider), façon éditeur VS Code.

function TreeNode({ slug, entry, depth, onOpenFile, openPaths }) {
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
        const r = await getSourceTree(slug, entry.path);
        setChildren(r.data?.entries || []);
      } catch {
        setChildren([]);
      } finally {
        setLoading(false);
      }
    }
  }, [entry, open, children, slug, onOpenFile]);

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
              onOpenFile={onOpenFile} openPaths={openPaths} />
          ))
        )
      )}
    </div>
  );
}

export default function FilesTab({ slug, active = true }) {
  const { order, convos, openFile } = useAgentConversations();
  const [root, setRoot] = useState([]);
  const [rootLoading, setRootLoading] = useState(true);
  const [treeKey, setTreeKey] = useState(0); // bump → reload racine (refresh)

  // Fichiers actuellement ouverts en onglet → surlignés dans l'arbre.
  const openPaths = useMemo(
    () => new Set(order.filter((k) => convos[k]?.type === 'file').map((k) => convos[k].path)),
    [order, convos],
  );

  const loadRoot = useCallback(() => {
    setRootLoading(true);
    getSourceTree(slug, '')
      .then((r) => setRoot(r.data?.entries || []))
      .catch(() => setRoot([]))
      .finally(() => setRootLoading(false));
  }, [slug]);

  // Charge à l'ouverture, au refresh (treeKey) et à chaque réactivation du pane.
  useEffect(() => { if (active) loadRoot(); }, [active, loadRoot, treeKey]);

  return (
    <div className="flex flex-col h-full min-h-0 bg-gray-900">
      {/* En-tête explorateur */}
      <div className="flex items-center gap-2 h-[34px] shrink-0 px-3 border-b border-gray-800 text-[12px] text-gray-400">
        <span className="truncate uppercase tracking-wider text-[11px] text-gray-500">{slug}/src</span>
        <button onClick={() => setTreeKey((k) => k + 1)} title="Rafraîchir"
          className="ml-auto p-1 rounded-sm text-gray-500 hover:text-gray-200 hover:bg-gray-800">
          <RefreshCw className="w-3.5 h-3.5" />
        </button>
      </div>

      {/* Arbre — pleine hauteur (le contenu des fichiers s'ouvre dans le split central) */}
      <div className="flex-1 min-h-0 overflow-auto py-1">
        {rootLoading ? (
          <div className="flex items-center justify-center py-6 text-gray-600"><Loader2 className="w-4 h-4 animate-spin" /></div>
        ) : root.length === 0 ? (
          <div className="text-[12px] text-gray-600 text-center py-6">Vide</div>
        ) : (
          <div key={treeKey}>
            {root.map((e) => (
              <TreeNode key={e.path} slug={slug} entry={e} depth={0}
                onOpenFile={openFile} openPaths={openPaths} />
            ))}
          </div>
        )}
      </div>
    </div>
  );
}
