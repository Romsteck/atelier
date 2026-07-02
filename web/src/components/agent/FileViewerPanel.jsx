import { useState, useEffect, useCallback, useRef } from 'react';
import { Loader2, X, FileWarning, FileCode2 } from 'lucide-react';
import { getSourceFile } from '../../api/client';
import { useAgentConversations } from '../../context/AgentConversationsContext';
import useWebSocket from '../../hooks/useWebSocket';
import { formatBytes } from '../../utils/formatters';

// Visionneuse de fichier rendue comme un panneau du split central (à côté des
// conversations), façon éditeur VS Code. Lecture seule, retour à la ligne TOUJOURS
// actif (pas de scroll horizontal). Le contenu se rafraîchit AUTOMATIQUEMENT quand
// un changement local est détecté (fin de tour de l'agent via WebSocket + retour de
// focus/visibilité pour les éditions hors-agent) — pas de bouton de rafraîchissement.

export default function FileViewerPanel({ panelKey }) {
  const { slug, convos, closeConversation } = useAgentConversations();
  const convo = convos[panelKey];
  const path = convo?.path;
  const name = convo?.name;

  const [file, setFile] = useState(null); // { content, size, binary, truncated } | { error }
  const [loading, setLoading] = useState(true);
  const loadedOnce = useRef(false);
  const debounce = useRef(null);

  // Spinner uniquement au 1er chargement ; les recharges (auto) échangent le contenu
  // en place, sans clignotement.
  const load = useCallback(() => {
    if (!path) return;
    if (!loadedOnce.current) setLoading(true);
    getSourceFile(slug, path)
      .then((r) => setFile(r.data))
      .catch((e) => setFile({ error: e.response?.data?.error || 'Erreur de lecture' }))
      .finally(() => { loadedOnce.current = true; setLoading(false); });
  }, [slug, path]);

  useEffect(() => {
    loadedOnce.current = false;
    load();
    return () => clearTimeout(debounce.current);
  }, [load]);

  // Changement local détecté → recharge et renvoie la MAJ à l'UI :
  //  - l'agent vient d'éditer des fichiers (fin de tour, debounce anti-rafale) ;
  //  - retour de focus/visibilité de la fenêtre (édition hors-agent).
  useWebSocket({
    'agent:event': (d) => {
      if (d && (d.kind === 'turn_done' || d.kind === 'done' || d.kind === 'result')) {
        clearTimeout(debounce.current);
        debounce.current = setTimeout(load, 600);
      }
    },
  });
  useEffect(() => {
    const onFocus = () => load();
    const onVis = () => { if (document.visibilityState === 'visible') load(); };
    window.addEventListener('focus', onFocus);
    document.addEventListener('visibilitychange', onVis);
    return () => {
      window.removeEventListener('focus', onFocus);
      document.removeEventListener('visibilitychange', onVis);
    };
  }, [load]);

  if (!convo) return null;

  return (
    <div className="flex flex-col h-full min-h-0 bg-gray-900">
      <div className="flex items-center gap-2 h-[34px] shrink-0 px-3 border-b border-gray-800 text-[12px]">
        <FileCode2 className="w-3.5 h-3.5 shrink-0 text-gray-500" />
        <span className="font-mono text-gray-200 truncate" title={path}>{name}</span>
        {file && !file.binary && !file.error && (
          <span className="text-gray-600 shrink-0">{formatBytes(file.size)}</span>
        )}
        <button onClick={() => closeConversation(panelKey)} title="Fermer"
          className="ml-auto shrink-0 p-1 rounded-sm text-gray-500 hover:text-gray-200 hover:bg-gray-800">
          <X className="w-3.5 h-3.5" />
        </button>
      </div>

      <div className="flex-1 overflow-auto min-h-0">
        {loading ? (
          <div className="flex items-center justify-center py-10 text-gray-600"><Loader2 className="w-5 h-5 animate-spin" /></div>
        ) : file?.error ? (
          <div className="p-4 text-[13px] text-red-400">{file.error}</div>
        ) : file?.binary ? (
          <div className="p-6 text-[13px] text-gray-500 flex items-center gap-2">
            <FileWarning className="w-4 h-4" /> Fichier binaire — non affiché ({formatBytes(file.size)}).
          </div>
        ) : (
          <>
            {file?.truncated && (
              <div className="text-[11px] text-yellow-400 bg-yellow-900/20 border-b border-yellow-800 px-3 py-1.5">
                Fichier tronqué (256 premiers Ko).
              </div>
            )}
            <pre className="text-[12px] font-mono text-gray-200 leading-5 px-3 py-2 whitespace-pre-wrap break-words">
              {file?.content || ''}
            </pre>
          </>
        )}
      </div>
    </div>
  );
}
