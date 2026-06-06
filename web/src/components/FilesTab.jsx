import { useState, useEffect, useCallback } from 'react';
import {
  Folder, FolderOpen, File as FileIcon, ChevronRight, ChevronDown,
  Loader2, RefreshCw, FileWarning, X,
} from 'lucide-react';
import { getSourceTree, getSourceFile } from '../api/client';

// Explorateur du working tree de l'app (`…/{slug}/src`) — lecture seule, lazy
// (un niveau chargé par expansion). Disposition VERTICALE (pensée pour la sidebar) :
// l'arbre occupe le haut ; à l'ouverture d'un fichier, un viewer apparaît en bas.

function humanSize(n) {
  if (n == null) return '';
  if (n < 1024) return `${n} o`;
  if (n < 1024 * 1024) return `${(n / 1024).toFixed(1)} Ko`;
  return `${(n / 1024 / 1024).toFixed(1)} Mo`;
}

function TreeNode({ slug, entry, depth, onOpenFile, selectedPath }) {
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

  const selected = !entry.is_dir && selectedPath === entry.path;
  return (
    <div>
      <button
        onClick={toggle}
        title={entry.path}
        style={{ paddingLeft: depth * 12 + 8 }}
        className={`w-full flex items-center gap-1 py-[3px] pr-2 text-[13px] text-left rounded-sm ${
          selected ? 'bg-blue-500/20 text-blue-200' : 'text-gray-300 hover:bg-gray-700/40'
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
              onOpenFile={onOpenFile} selectedPath={selectedPath} />
          ))
        )
      )}
    </div>
  );
}

export default function FilesTab({ slug, active = true }) {
  const [root, setRoot] = useState([]);
  const [rootLoading, setRootLoading] = useState(true);
  const [selected, setSelected] = useState(null); // entry
  const [file, setFile] = useState(null); // { content, size, binary, truncated }
  const [fileLoading, setFileLoading] = useState(false);
  const [treeKey, setTreeKey] = useState(0); // bump → reload racine (refresh)
  const [wrap, setWrap] = useState(false);

  const loadRoot = useCallback(() => {
    setRootLoading(true);
    getSourceTree(slug, '')
      .then((r) => setRoot(r.data?.entries || []))
      .catch(() => setRoot([]))
      .finally(() => setRootLoading(false));
  }, [slug]);

  // Charge à l'ouverture, au refresh (treeKey) et à chaque réactivation du pane.
  useEffect(() => { if (active) loadRoot(); }, [active, loadRoot, treeKey]);

  const openFile = useCallback((entry) => {
    setSelected(entry);
    setFile(null);
    setFileLoading(true);
    getSourceFile(slug, entry.path)
      .then((r) => setFile(r.data))
      .catch((e) => setFile({ error: e.response?.data?.error || 'Erreur de lecture' }))
      .finally(() => setFileLoading(false));
  }, [slug]);

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

      {/* Arbre — plein si aucun fichier ouvert, sinon partagé avec le viewer */}
      <div className={`overflow-auto py-1 ${selected ? 'shrink-0 max-h-[45%]' : 'flex-1 min-h-0'}`}>
        {rootLoading ? (
          <div className="flex items-center justify-center py-6 text-gray-600"><Loader2 className="w-4 h-4 animate-spin" /></div>
        ) : root.length === 0 ? (
          <div className="text-[12px] text-gray-600 text-center py-6">Vide</div>
        ) : (
          <div key={treeKey}>
            {root.map((e) => (
              <TreeNode key={e.path} slug={slug} entry={e} depth={0}
                onOpenFile={openFile} selectedPath={selected?.path} />
            ))}
          </div>
        )}
      </div>

      {/* Viewer (apparaît à l'ouverture d'un fichier) */}
      {selected && (
        <div className="flex-1 min-h-0 flex flex-col border-t border-gray-800">
          <div className="flex items-center gap-2 h-[30px] shrink-0 px-3 border-b border-gray-800 text-[12px]">
            <span className="font-mono text-gray-300 truncate">{selected.name}</span>
            {file && !file.binary && <span className="text-gray-600 shrink-0">{humanSize(file.size)}</span>}
            <button onClick={() => setWrap((w) => !w)}
              className={`ml-auto shrink-0 px-1.5 py-0.5 rounded-sm text-[11px] ${wrap ? 'bg-blue-500/20 text-blue-300' : 'text-gray-500 hover:text-gray-300 hover:bg-gray-800'}`}>
              wrap
            </button>
            <button onClick={() => { setSelected(null); setFile(null); }} title="Fermer"
              className="shrink-0 p-0.5 rounded-sm text-gray-500 hover:text-gray-200 hover:bg-gray-800">
              <X className="w-3.5 h-3.5" />
            </button>
          </div>
          <div className="flex-1 overflow-auto min-h-0">
            {fileLoading ? (
              <div className="flex items-center justify-center py-8 text-gray-600"><Loader2 className="w-5 h-5 animate-spin" /></div>
            ) : file?.error ? (
              <div className="p-4 text-[13px] text-red-400">{file.error}</div>
            ) : file?.binary ? (
              <div className="p-6 text-[13px] text-gray-500 flex items-center gap-2">
                <FileWarning className="w-4 h-4" /> Fichier binaire — non affiché ({humanSize(file.size)}).
              </div>
            ) : (
              <>
                {file?.truncated && (
                  <div className="text-[11px] text-yellow-400 bg-yellow-900/20 border-b border-yellow-800 px-3 py-1.5">
                    Fichier tronqué (256 premiers Ko).
                  </div>
                )}
                <pre className={`text-[12px] font-mono text-gray-200 leading-5 px-3 py-2 ${wrap ? 'whitespace-pre-wrap break-words' : 'whitespace-pre'}`}>
                  {file?.content || ''}
                </pre>
              </>
            )}
          </div>
        </div>
      )}
    </div>
  );
}
