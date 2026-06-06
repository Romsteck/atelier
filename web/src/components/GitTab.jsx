import { useState, useEffect, useCallback } from 'react';
import { Loader2, RefreshCw, GitBranch, GitCommit, ArrowUp, ArrowDown, X } from 'lucide-react';
import DiffView from './git/DiffView';
import {
  getSourceGitStatus, getSourceGitDiff, getSourceGitLog, getSourceGitShow,
} from '../api/client';

// Contrôle de source du working tree (`…/{slug}/src`) — façon onglet « Source
// Control » de code-server. Disposition VERTICALE (sidebar) : liste en haut, diff
// en bas. Deux vues : Modifs (working tree vs HEAD) et Historique. Sert surtout à
// relire ce que l'agent (mode Bypass) vient de modifier. Lecture seule.

const STATUS_STYLE = {
  A: 'text-green-400 bg-green-900/30',
  M: 'text-yellow-400 bg-yellow-900/30',
  D: 'text-red-400 bg-red-900/30',
  R: 'text-blue-400 bg-blue-900/30',
  C: 'text-cyan-400 bg-cyan-900/30',
  T: 'text-purple-400 bg-purple-900/30',
  U: 'text-orange-400 bg-orange-900/30',
};
function FileStatusBadge({ status }) {
  const s = (status || 'X').toUpperCase().charAt(0);
  return (
    <span className={`w-5 text-center text-[11px] font-mono font-bold shrink-0 rounded-sm ${STATUS_STYLE[s] || 'text-gray-400 bg-gray-700/40'}`}>
      {s}
    </span>
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

export default function GitTab({ slug, active = true }) {
  const [view, setView] = useState('changes'); // 'changes' | 'history'

  const [status, setStatus] = useState(null);
  const [statusLoading, setStatusLoading] = useState(true);
  const [selChange, setSelChange] = useState(null);
  const [diff, setDiff] = useState(null);
  const [diffLoading, setDiffLoading] = useState(false);

  const [commits, setCommits] = useState(null);
  const [commitsLoading, setCommitsLoading] = useState(false);
  const [selSha, setSelSha] = useState(null);
  const [show, setShow] = useState(null);
  const [showLoading, setShowLoading] = useState(false);

  const loadStatus = useCallback(() => {
    setStatusLoading(true);
    setSelChange(null);
    setDiff(null);
    getSourceGitStatus(slug)
      .then((r) => setStatus(r.data))
      .catch((e) => setStatus({ error: e.response?.data?.error || 'Erreur git status' }))
      .finally(() => setStatusLoading(false));
  }, [slug]);

  const loadCommits = useCallback(() => {
    setCommitsLoading(true);
    getSourceGitLog(slug, 100)
      .then((r) => setCommits(r.data?.commits || []))
      .catch(() => setCommits([]))
      .finally(() => setCommitsLoading(false));
  }, [slug]);

  // Rafraîchit le status à l'ouverture et à chaque réactivation du pane (pour
  // relire ce que l'agent vient de modifier sans cliquer sur Rafraîchir).
  useEffect(() => { if (active) loadStatus(); }, [active, loadStatus]);
  useEffect(() => { if (view === 'history' && commits === null) loadCommits(); }, [view, commits, loadCommits]);

  const openChange = useCallback((path) => {
    setSelChange(path);
    setDiff(null);
    setDiffLoading(true);
    getSourceGitDiff(slug, path)
      .then((r) => setDiff(r.data))
      .catch((e) => setDiff({ error: e.response?.data?.error || 'Erreur diff' }))
      .finally(() => setDiffLoading(false));
  }, [slug]);

  const openCommit = useCallback((sha) => {
    setSelSha(sha);
    setShow(null);
    setShowLoading(true);
    getSourceGitShow(slug, sha)
      .then((r) => setShow(r.data))
      .catch((e) => setShow({ error: e.response?.data?.error || 'Erreur show' }))
      .finally(() => setShowLoading(false));
  }, [slug]);

  const hasDetail = view === 'changes' ? !!selChange : !!selSha;
  const closeDetail = () => { setSelChange(null); setSelSha(null); setDiff(null); setShow(null); };

  return (
    <div className="flex flex-col h-full min-h-0 bg-gray-900">
      {/* Barre : vue + branche + refresh */}
      <div className="flex items-center gap-2 h-[34px] shrink-0 px-3 border-b border-gray-800 text-[12px]">
        <button onClick={() => setView('changes')}
          className={`px-2 py-0.5 rounded-sm ${view === 'changes' ? 'bg-gray-700 text-gray-100' : 'text-gray-500 hover:text-gray-300'}`}>
          Modifs{status?.files?.length ? ` (${status.files.length})` : ''}
        </button>
        <button onClick={() => setView('history')}
          className={`px-2 py-0.5 rounded-sm ${view === 'history' ? 'bg-gray-700 text-gray-100' : 'text-gray-500 hover:text-gray-300'}`}>
          Historique
        </button>
        <div className="ml-auto flex items-center gap-2 text-gray-500">
          {status?.branch && (
            <span className="flex items-center gap-1 truncate max-w-[120px]" title={`branche : ${status.branch}`}>
              <GitBranch className="w-3.5 h-3.5 shrink-0" /> <span className="truncate">{status.branch}</span>
            </span>
          )}
          {status?.ahead > 0 && <span className="flex items-center text-green-400" title="en avance"><ArrowUp className="w-3 h-3" />{status.ahead}</span>}
          {status?.behind > 0 && <span className="flex items-center text-amber-400" title="en retard"><ArrowDown className="w-3 h-3" />{status.behind}</span>}
          <button onClick={() => { loadStatus(); if (view === 'history') loadCommits(); }} title="Rafraîchir"
            className="p-1 rounded-sm hover:text-gray-200 hover:bg-gray-800">
            <RefreshCw className="w-3.5 h-3.5" />
          </button>
        </div>
      </div>

      {/* Liste — pleine hauteur, ou partagée avec le diff quand un item est ouvert */}
      <div className={`overflow-auto ${hasDetail ? 'shrink-0 max-h-[45%]' : 'flex-1 min-h-0'}`}>
        {view === 'changes' ? (
          statusLoading ? (
            <div className="flex justify-center py-6 text-gray-600"><Loader2 className="w-4 h-4 animate-spin" /></div>
          ) : status?.error ? (
            <div className="p-3 text-[12px] text-red-400">{status.error}</div>
          ) : status?.clean ? (
            <div className="p-4 text-[12px] text-gray-600 text-center">Working tree propre.</div>
          ) : (
            (status?.files || []).map((f) => (
              <button key={f.path} onClick={() => openChange(f.path)} title={f.path}
                className={`w-full flex items-center gap-2 px-2 py-1 text-[12px] text-left ${selChange === f.path ? 'bg-blue-500/20' : 'hover:bg-gray-700/40'}`}>
                <FileStatusBadge status={f.status} />
                <span className="font-mono text-gray-300 truncate">
                  {f.old_path ? `${f.old_path} → ${f.path}` : f.path}
                </span>
              </button>
            ))
          )
        ) : (
          commitsLoading ? (
            <div className="flex justify-center py-6 text-gray-600"><Loader2 className="w-4 h-4 animate-spin" /></div>
          ) : (commits || []).length === 0 ? (
            <div className="p-4 text-[12px] text-gray-600 text-center">Aucun commit.</div>
          ) : (
            (commits || []).map((c) => (
              <button key={c.sha} onClick={() => openCommit(c.sha)} title={c.subject}
                className={`w-full flex flex-col gap-0.5 px-3 py-1.5 text-left border-b border-gray-800/60 ${selSha === c.sha ? 'bg-blue-500/20' : 'hover:bg-gray-700/40'}`}>
                <span className="text-[12px] text-gray-200 truncate">{c.subject}</span>
                <span className="text-[11px] text-gray-500 flex items-center gap-1.5">
                  <GitCommit className="w-3 h-3 shrink-0" />
                  <span className="font-mono">{c.short}</span>
                  <span className="truncate">{c.author}</span>
                  <span className="ml-auto shrink-0">{ago(c.date)}</span>
                </span>
              </button>
            ))
          )
        )}
      </div>

      {/* Diff (apparaît quand un fichier/commit est sélectionné) */}
      {hasDetail && (
        <div className="flex-1 min-h-0 flex flex-col border-t border-gray-800">
          <div className="flex items-center gap-2 h-[28px] shrink-0 px-3 text-[11px] text-gray-500 border-b border-gray-800/60">
            <span className="font-mono truncate">{view === 'changes' ? selChange : (selSha || '').slice(0, 10)}</span>
            <button onClick={closeDetail} title="Fermer" className="ml-auto shrink-0 p-0.5 rounded-sm hover:text-gray-200 hover:bg-gray-800">
              <X className="w-3.5 h-3.5" />
            </button>
          </div>
          <div className="flex-1 overflow-auto min-h-0 p-2">
            {view === 'changes' ? (
              diffLoading ? (
                <div className="flex justify-center py-8 text-gray-600"><Loader2 className="w-5 h-5 animate-spin" /></div>
              ) : diff?.error ? (
                <div className="text-[13px] text-red-400 p-2">{diff.error}</div>
              ) : (
                <DiffView patch={diff?.patch} truncated={diff?.truncated} />
              )
            ) : showLoading ? (
              <div className="flex justify-center py-8 text-gray-600"><Loader2 className="w-5 h-5 animate-spin" /></div>
            ) : show?.error ? (
              <div className="text-[13px] text-red-400 p-2">{show.error}</div>
            ) : (
              <DiffView patch={show?.patch} truncated={show?.truncated} />
            )}
          </div>
        </div>
      )}
    </div>
  );
}
