import { useState, useEffect, useCallback, useRef } from 'react';
import { Loader2, X } from 'lucide-react';
import DiffView from '../git/DiffView';
import FileStatusBadge from '../git/FileStatusBadge';
import { getSourceGitDiff } from '../../api/client';
import { useAgentConversations } from '../../context/AgentConversationsContext';
import useWebSocket from '../../hooks/useWebSocket';

// Diff d'un fichier MODIFIÉ du working tree (vs HEAD), rendu comme un panneau du
// split central (façon onglet VS Code) — comme les fichiers et les commits, plus
// l'aperçu condensé dans la sidebar. Retour à la ligne toujours actif ; rechargé
// AUTOMATIQUEMENT sur changement local (fin de tour agent + focus/visibilité) ;
// pas de bouton de rafraîchissement.

// +/- comptés depuis le patch (working tree = 1 fichier → rarement tronqué).
function diffStats(patch) {
  let add = 0;
  let del = 0;
  for (const ln of (patch || '').split('\n')) {
    if (ln.startsWith('+') && !ln.startsWith('+++')) add++;
    else if (ln.startsWith('-') && !ln.startsWith('---')) del++;
  }
  return { add, del };
}

export default function WorkingDiffPanel({ panelKey }) {
  const { slug, convos, closeConversation } = useAgentConversations();
  const convo = convos[panelKey];
  const path = convo?.path;

  const [diff, setDiff] = useState(null); // { patch, truncated } | { error }
  const [loading, setLoading] = useState(true);
  const loadedOnce = useRef(false);
  const debounce = useRef(null);

  const load = useCallback(() => {
    if (!path) return;
    if (!loadedOnce.current) setLoading(true);
    // convId capturé à l'ouverture → diff lu dans le worktree de la conversation (sinon src/).
    getSourceGitDiff(slug, path, convo?.convId)
      .then((r) => setDiff(r.data))
      .catch((e) => setDiff({ error: e.response?.data?.error || 'Erreur diff' }))
      .finally(() => { loadedOnce.current = true; setLoading(false); });
  }, [slug, path, convo?.convId]);

  useEffect(() => {
    loadedOnce.current = false;
    load();
    return () => clearTimeout(debounce.current);
  }, [load]);

  // Le diff évolue à chaque édition → recharge sur changement local détecté.
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

  const stats = diff?.patch && diff.patch.trim() ? diffStats(diff.patch) : null;
  const emptyDiff = !loading && !diff?.error && !(diff?.patch && diff.patch.trim());

  return (
    <div className="flex flex-col h-full min-h-0 bg-gray-900">
      <div className="flex items-center gap-2 h-[34px] shrink-0 px-3 border-b border-gray-800 text-[12px]">
        <FileStatusBadge status={convo.status} />
        <span className="font-mono text-gray-200 truncate" title={path}>{path}</span>
        {stats && (
          <span className="shrink-0 flex items-center gap-2 font-mono text-[11px]">
            <span className="text-green-400">+{stats.add}</span>
            <span className="text-red-400">-{stats.del}</span>
          </span>
        )}
        <button onClick={() => closeConversation(panelKey)} title="Fermer"
          className="ml-auto shrink-0 p-1 rounded-sm text-gray-500 hover:text-gray-200 hover:bg-gray-800">
          <X className="w-3.5 h-3.5" />
        </button>
      </div>

      <div className="flex-1 overflow-auto min-h-0 p-2">
        {loading ? (
          <div className="flex items-center justify-center py-10 text-gray-600"><Loader2 className="w-5 h-5 animate-spin" /></div>
        ) : diff?.error ? (
          <div className="p-2 text-[13px] text-red-400">{diff.error}</div>
        ) : emptyDiff ? (
          <div className="p-2 text-[13px] text-gray-600">Aucune modification (fichier committé ou revenu à HEAD).</div>
        ) : (
          <DiffView patch={diff?.patch} truncated={diff?.truncated} />
        )}
      </div>
    </div>
  );
}
