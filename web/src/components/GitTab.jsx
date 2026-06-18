import { useEffect, useCallback, useMemo, useState } from 'react';
import { Loader2, GitBranch, ArrowUp, ArrowDown, GitMerge, Rocket } from 'lucide-react';
import FileStatusBadge from './git/FileStatusBadge';
import CommitGraph from './git/CommitGraph';
import { getSourceGitLog, pushSource, listWorktrees, mergeWorktree } from '../api/client';
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

export default function GitTab({ slug, active = true, status, statusLoading = false, onRefresh, convId }) {
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
  // conv_ids des conversations actuellement EN COURS → on bloque leur Merge & Deploy
  // (merger une conversation qui tourne l'interromprait et risquerait du travail non commité).
  const runningConvIds = useMemo(
    () => new Set(order.filter((k) => convos[k]?.running).map((k) => convos[k]?.convId).filter(Boolean)),
    [order, convos],
  );

  const [commits, setCommits] = useState(null);
  const [pushing, setPushing] = useState(false);
  const [actionError, setActionError] = useState(null);
  // Branches de conversation (worktrees) : isolation Phase 1. Chacune est une
  // conversation en cours ; « Merge & Deploy » la ramène dans main + rebuild + restart.
  const [worktrees, setWorktrees] = useState([]);
  const [merging, setMerging] = useState(null); // convId en cours de merge
  const [mergeMsg, setMergeMsg] = useState(null);
  const [conflicts, setConflicts] = useState(null); // { branch, files } sur 409

  const loadCommits = useCallback(() => {
    getSourceGitLog(slug, 100, convId)
      .then((r) => setCommits(r.data?.commits || []))
      .catch(() => setCommits([]));
  }, [slug, convId]);

  const loadWorktrees = useCallback(() => {
    listWorktrees(slug)
      .then((r) => setWorktrees((r.data?.worktrees || []).filter((w) => !w.is_main)))
      .catch(() => setWorktrees([]));
  }, [slug]);

  // Resynchronise le status (parent) à l'ouverture / réactivation du pane.
  useEffect(() => { if (active) onRefresh?.(); }, [active, onRefresh]);

  // L'historique + les branches suivent le status : rechargés à chaque rafraîchissement
  // (fin de tour agent / focus). Silencieux après le 1er.
  useEffect(() => {
    loadCommits();
    loadWorktrees();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [status]);

  const doMerge = useCallback(
    async (convId, branch) => {
      if (merging) return;
      // Confirmation (9a) : le merge interrompt les conversations en cours + déploie.
      if (!window.confirm(
        `Merger « ${branch} » dans main et déployer ${slug} ?\n\n` +
        `Les conversations en cours de cette app seront interrompues, l'app rebuildée puis redémarrée.`,
      )) return;
      setActionError(null);
      setMergeMsg(null);
      setConflicts(null);
      setMerging(convId);
      try {
        const r = await mergeWorktree(slug, convId);
        setMergeMsg(`« ${r.data?.merged || branch} » mergé et déployé.`);
        loadWorktrees();
        onRefresh?.();
      } catch (e) {
        const data = e.response?.data;
        if (e.response?.status === 409 && Array.isArray(data?.conflicts)) {
          setConflicts({ branch, files: data.conflicts });
        } else {
          setActionError(data?.error || data?.detail || 'Échec du merge & deploy');
        }
      } finally {
        setMerging(null);
      }
    },
    [slug, merging, loadWorktrees, onRefresh],
  );

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
        {/* — Branches de conversation (worktrees) : Merge & Deploy — */}
        {worktrees.length > 0 && (
          <>
            <SectionHeader>Branches de conversation ({worktrees.length})</SectionHeader>
            {worktrees.map((w) => {
              const running = runningConvIds.has(w.conv_id);
              return (
                <div key={w.branch} className="flex items-center gap-2 px-2 py-1.5 border-b border-gray-800/60">
                  <GitBranch className={`w-3.5 h-3.5 shrink-0 ${running ? 'text-blue-400' : 'text-purple-400'}`} />
                  <span className="font-mono text-[12px] text-gray-300 truncate flex-1" title={w.branch}>
                    {w.branch}
                    {running && <span className="ml-1.5 text-[10px] text-blue-400">en cours…</span>}
                  </span>
                  <button
                    onClick={() => doMerge(w.conv_id, w.branch)}
                    disabled={!!merging || !w.conv_id || running}
                    title={
                      running
                        ? 'Conversation en cours — attends qu\'elle finisse et ait commité son travail'
                        : w.conv_id
                        ? `Merger ${w.branch} dans main + déployer ${slug}`
                        : 'branche sans conv_id'
                    }
                    className="flex items-center gap-1 px-2 py-0.5 rounded-sm bg-purple-600 text-white hover:bg-purple-700 disabled:opacity-40 disabled:cursor-not-allowed text-[11px] font-medium shrink-0"
                  >
                    {merging === w.conv_id ? <Loader2 className="w-3 h-3 animate-spin" /> : <GitMerge className="w-3 h-3" />}
                    {merging === w.conv_id ? 'Déploiement…' : 'Merge & Deploy'}
                  </button>
                </div>
              );
            })}
            {mergeMsg && (
              <div className="flex items-center gap-1.5 px-3 py-1.5 text-[11px] text-green-400">
                <Rocket className="w-3 h-3 shrink-0" />{mergeMsg}
              </div>
            )}
            {conflicts && (
              <div className="px-3 py-1.5 text-[11px] text-amber-400">
                Conflit sur « {conflicts.branch} » — à résoudre manuellement :
                <ul className="mt-1 font-mono text-amber-300/90">
                  {conflicts.files.map((f) => <li key={f} className="truncate">· {f}</li>)}
                </ul>
              </div>
            )}
          </>
        )}

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
          // Graphe multi-branches façon VSCode (lanes + merges + puces de branche).
          <CommitGraph commits={commits} openShas={openShas} onOpen={openCommit} />
        )}
      </div>
    </div>
  );
}
