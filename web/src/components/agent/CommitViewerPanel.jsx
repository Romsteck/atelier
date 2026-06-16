import { useState, useEffect, useCallback } from 'react';
import { Loader2, X, GitCommit, User } from 'lucide-react';
import DiffView from '../git/DiffView';
import { getSourceGitShow } from '../../api/client';
import { useAgentConversations } from '../../context/AgentConversationsContext';

// Diff d'un commit rendu comme un panneau du split central (façon onglet VS Code) :
// entête formaté (sujet, auteur, date, sha, totaux +/-, corps) + diff pleine page,
// retour à la ligne toujours actif. Un commit est immuable → chargé une fois.

function fmtDate(iso) {
  if (!iso) return '';
  const d = new Date(iso);
  if (Number.isNaN(d.getTime())) return '';
  try {
    return d.toLocaleString('fr-FR', { dateStyle: 'medium', timeStyle: 'short' });
  } catch {
    return d.toLocaleString();
  }
}

export default function CommitViewerPanel({ panelKey }) {
  const { slug, convos, closeConversation } = useAgentConversations();
  const convo = convos[panelKey];
  const sha = convo?.sha;

  const [data, setData] = useState(null); // { subject, author, ..., additions, deletions, patch } | { error }
  const [loading, setLoading] = useState(true);

  const load = useCallback(() => {
    if (!sha) return;
    setLoading(true);
    getSourceGitShow(slug, sha)
      .then((r) => setData(r.data))
      .catch((e) => setData({ error: e.response?.data?.error || 'Erreur show' }))
      .finally(() => setLoading(false));
  }, [slug, sha]);

  useEffect(() => { load(); }, [load]);

  if (!convo) return null;

  const short = data?.short || convo.short || (sha || '').slice(0, 7);
  const subject = data?.subject || convo.subject || 'Commit';
  const files = data?.files_changed || 0;

  return (
    <div className="flex flex-col h-full min-h-0 bg-gray-900">
      {/* Barre d'onglet (identité compacte) */}
      <div className="flex items-center gap-2 h-[34px] shrink-0 px-3 border-b border-gray-800 text-[12px]">
        <GitCommit className="w-3.5 h-3.5 shrink-0 text-gray-500" />
        <span className="font-mono text-gray-300 shrink-0">{short}</span>
        <button onClick={() => closeConversation(panelKey)} title="Fermer"
          className="ml-auto shrink-0 p-1 rounded-sm text-gray-500 hover:text-gray-200 hover:bg-gray-800">
          <X className="w-3.5 h-3.5" />
        </button>
      </div>

      <div className="flex-1 overflow-auto min-h-0">
        {loading ? (
          <div className="flex items-center justify-center py-10 text-gray-600"><Loader2 className="w-5 h-5 animate-spin" /></div>
        ) : data?.error ? (
          <div className="p-4 text-[13px] text-red-400">{data.error}</div>
        ) : (
          <>
            {/* En-tête formaté du commit */}
            <div className="px-4 py-3 border-b border-gray-800">
              <div className="text-[14px] font-semibold text-gray-100 break-words leading-snug">{subject}</div>
              <div className="mt-1.5 flex flex-wrap items-center gap-x-2 gap-y-0.5 text-[11px] text-gray-500">
                <span className="flex items-center gap-1 text-gray-400">
                  <User className="w-3 h-3 shrink-0" />{data?.author}
                </span>
                {data?.email && <span className="text-gray-600 truncate">{`<${data.email}>`}</span>}
                <span>·</span>
                <span title={data?.author_date}>{fmtDate(data?.author_date)}</span>
                <span>·</span>
                <span className="font-mono">{(data?.sha || sha || '').slice(0, 10)}</span>
              </div>
              {/* Totaux lignes ajoutées / supprimées */}
              <div className="mt-2 flex items-center gap-3 text-[11px] font-mono">
                <span className="text-gray-500">{files} fichier{files > 1 ? 's' : ''}</span>
                <span className="text-green-400">+{data?.additions || 0}</span>
                <span className="text-red-400">-{data?.deletions || 0}</span>
              </div>
              {/* Corps du message */}
              {data?.body && data.body.trim() && (
                <pre className="mt-3 text-[12px] text-gray-400 whitespace-pre-wrap break-words font-sans">{data.body.trim()}</pre>
              )}
            </div>

            {/* Diff */}
            <div className="p-2">
              <DiffView patch={data?.patch} truncated={data?.truncated} />
            </div>
          </>
        )}
      </div>
    </div>
  );
}
