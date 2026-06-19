import { useEffect, useCallback, useMemo, useState } from 'react';
import { Loader2, GitBranch, GitCommit, ArrowUp, ArrowDown } from 'lucide-react';
import FileStatusBadge from './git/FileStatusBadge';
import { getSourceGitLog, pushSource } from '../api/client';
import { useAgentConversations } from '../context/AgentConversationsContext';

// Contrôle de source du working tree (`…/{slug}/src`) — façon onglet « Source
// Control » de VS Code. Disposition VERTICALE empilée : modifs en haut,
// historique en dessous. Cliquer un fichier modifié OU un commit ouvre son diff en
// onglet central plein écran (plus d'aperçu condensé). Le status vient du parent
// (hook useSourceGit, auto-rafraîchi). On peut POUSSER ici ; le commit se fait via
// l'agent.

function SectionHeader({ children }) {
  return (
    <div className="sticky top-0 z-10 bg-gray-900/95 backdrop-blur px-3 pt-2 pb-1 text-[10px] uppercase tracking-wider text-gray-500 font-semibold">
      {children}
    </div>
  );
}

// Petit "il y a X" autonome (évite un import utilitaire transverse).
function ago(iso) {
  if (!iso) return '';
  const d = new Date(iso).getTime();
  if (!Number.isFinite(d)) return '';
  const s = Math.max(0, (Date.now() - d) / 1000);
  if (s < 60) return `il y a ${Math.floor(s)} s`;
  if (s < 3600) return `il y a ${Math.floor(s / 60)} min`;
  if (s < 86400) return `il y a ${Math.floor(s / 3600)} h`;
  if (s < 2592000) return `il y a ${Math.floor(s / 86400)} j`;
  return new Date(iso).toLocaleDateString();
}

export default function GitTab({ slug, active = true, status, statusLoading = false, onRefresh }) {
  // Modifs et commits s'ouvrent en onglet central plein écran (façon fichier).
  const { openCommit, openDiff, order, convos } = useAgentConversations();
  const openShas = useMemo(
    () => new Set(order.filter((k) => convos[k]?.type === 'commit').map((k) => convos[k].sha)),
    [order, convos],
  );
  const openDiffPaths = useMemo(
    () => new Set(order.filter((k) => convos[k]?.type === 'diff').map((k) => convos[k].path)),
    [order, convos],
  );

  const [commits, setCommits] = useState(null);
  const [pushing, setPushing] = useState(false);
  const [actionError, setActionError] = useState(null);

  const loadCommits = useCallback(() => {
    getSourceGitLog(slug, 100)
      .then((r) => setCommits(r.data?.commits || []))
      .catch(() => setCommits([]));
  }, [slug]);

  // Resynchronise le status (parent) à l'ouverture / réactivation du pane.
  useEffect(() => { if (active) onRefresh?.(); }, [active, onRefresh]);

  // L'historique suit le status : rechargé à chaque rafraîchissement (fin de tour
  // agent / focus) → nouveaux commits + marquage « non poussé ». Silencieux après le 1er.
  useEffect(() => {
    loadCommits();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [status]);

  const doPush = useCallback(() => {
    if (pushing) return;
    setActionError(null);
    setPushing(true);
    pushSource(slug)
      .then(() => onRefresh?.())
      .catch((e) => setActionError(e.response?.data?.error || 'Échec du push'))
      .finally(() => setPushing(false));
  }, [slug, pushing, onRefresh]);

  const fileCount = status?.files?.length || 0;
  const ahead = status?.ahead || 0;

  return (
    <div className="flex flex-col h-full min-h-0 bg-gray-900">
      {/* Barre : branche + behind + bouton Push (bleu, façon VS Code) */}
      <div className="flex items-center gap-2 h-[34px] shrink-0 px-3 border-b border-gray-800 text-[12px]">
        {status?.branch && (
          <span className="flex items-center gap-1 truncate text-gray-400" title={`branche : ${status.branch}`}>
            <GitBranch className="w-3.5 h-3.5 shrink-0" /> <span className="truncate">{status.branch}</span>
          </span>
        )}
        <div className="ml-auto flex items-center gap-2 text-gray-500">
          {status?.behind > 0 && (
            <span className="flex items-center text-amber-400" title="en retard sur l'upstream"><ArrowDown className="w-3 h-3" />{status.behind}</span>
          )}
          {ahead > 0 && (
            <button onClick={doPush} disabled={pushing} title={`Pousser ${ahead} commit(s) vers l'upstream`}
              className="flex items-center gap-1 px-2 py-0.5 rounded-sm bg-blue-500 text-white hover:bg-blue-600 disabled:opacity-50 text-[11px] font-medium">
              {pushing ? <Loader2 className="w-3 h-3 animate-spin" /> : <ArrowUp className="w-3 h-3" />}
              Push {ahead}
            </button>
          )}
        </div>
      </div>

      {/* Erreur d'action (ex. push) */}
      {actionError && (
        <div className="shrink-0 border-b border-gray-800 px-3 py-1.5 text-[11px] text-red-400 truncate" title={actionError}>{actionError}</div>
      )}

      {/* Modifications + Historique empilés (un seul scroll) */}
      <div className="flex-1 min-h-0 overflow-auto">
        {/* — Modifications — */}
        <SectionHeader>Modifications{fileCount ? ` (${fileCount})` : ''}</SectionHeader>
        {statusLoading && !status ? (
          <div className="flex justify-center py-4 text-gray-600"><Loader2 className="w-4 h-4 animate-spin" /></div>
        ) : status?.error ? (
          <div className="px-3 py-2 text-[12px] text-red-400">{status.error}</div>
        ) : status?.clean ? (
          <div className="px-3 py-2 text-[12px] text-gray-600">Working tree propre.</div>
        ) : (
          (status?.files || []).map((f) => (
            <button key={f.path} onClick={() => openDiff(f)} title={f.path}
              className={`w-full flex items-center gap-2 px-2 py-1 text-[12px] text-left ${openDiffPaths.has(f.path) ? 'bg-blue-500/20' : 'hover:bg-gray-700/40'}`}>
              <FileStatusBadge status={f.status} />
              <span className="font-mono text-gray-300 truncate">
                {f.old_path ? `${f.old_path} → ${f.path}` : f.path}
              </span>
            </button>
          ))
        )}

        {/* — Historique — */}
        <SectionHeader>Historique</SectionHeader>
        {commits === null ? (
          <div className="flex justify-center py-4 text-gray-600"><Loader2 className="w-4 h-4 animate-spin" /></div>
        ) : commits.length === 0 ? (
          <div className="px-3 py-2 text-[12px] text-gray-600">Aucun commit.</div>
        ) : (
          commits.map((c, i) => {
            const unpushed = i < ahead; // les `ahead` plus récents ne sont pas sur l'upstream
            return (
              <button key={c.sha} onClick={() => openCommit(c)} title={unpushed ? `${c.subject}\n(non poussé)` : c.subject}
                className={`w-full flex flex-col gap-0.5 pl-3 pr-3 py-1.5 text-left border-l-2 border-b border-b-gray-800/60 ${
                  openShas.has(c.sha) ? 'bg-blue-500/20' : unpushed ? 'bg-green-500/5 hover:bg-green-500/10' : 'hover:bg-gray-700/40'
                } ${unpushed ? 'border-l-green-500' : 'border-l-transparent'}`}>
                <span className="text-[12px] text-gray-200 truncate flex items-center gap-1.5">
                  {unpushed && <ArrowUp className="w-3 h-3 shrink-0 text-green-400" />}
                  <span className="truncate">{c.subject}</span>
                </span>
                <span className="text-[11px] text-gray-500 flex items-center gap-1.5">
                  <GitCommit className="w-3 h-3 shrink-0" />
                  <span className={`font-mono ${unpushed ? 'text-green-400' : ''}`}>{c.short}</span>
                  <span className="truncate">{c.author}</span>
                  <span className="ml-auto shrink-0">{ago(c.date)}</span>
                </span>
              </button>
            );
          })
        )}
      </div>
    </div>
  );
}
