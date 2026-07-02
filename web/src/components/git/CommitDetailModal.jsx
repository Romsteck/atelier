import { useEffect, useState } from 'react';
import { X, Loader2, GitCommit, ExternalLink } from 'lucide-react';
import Button from '../Button';
import DiffView from './DiffView';
import { getGitCommitDetail } from '../../api/client';
import { timeAgo } from '../../utils/formatters';

// Badge de statut de fichier (A/M/D/R/C/T/U) — sémantique git, distincte du
// StatusBadge de service, d'où une petite version locale.
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
    <span className={`w-5 text-center text-[11px] font-mono font-bold shrink-0 ${STATUS_STYLE[s] || 'text-gray-400 bg-gray-700/40'}`}>
      {s}
    </span>
  );
}

export default function CommitDetailModal({ slug, sha, org, onClose }) {
  const [detail, setDetail] = useState(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState(null);

  useEffect(() => {
    function onKey(e) {
      if (e.key === 'Escape') onClose();
    }
    document.addEventListener('keydown', onKey);
    document.body.style.overflow = 'hidden';
    return () => {
      document.removeEventListener('keydown', onKey);
      document.body.style.overflow = '';
    };
  }, [onClose]);

  useEffect(() => {
    let cancelled = false;
    setLoading(true);
    setError(null);
    getGitCommitDetail(slug, sha)
      .then((res) => {
        if (!cancelled) setDetail(res.data?.commit || res.data);
      })
      .catch((e) => {
        if (!cancelled) setError(e.response?.data?.error || 'Erreur de chargement du commit');
      })
      .finally(() => {
        if (!cancelled) setLoading(false);
      });
    return () => {
      cancelled = true;
    };
  }, [slug, sha]);

  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center p-4">
      <div className="absolute inset-0 bg-black/70 backdrop-blur-xs" onClick={onClose} />
      <div className="relative bg-gray-800 border border-gray-700 shadow-2xl w-full max-w-5xl max-h-[85vh] flex flex-col">
        {/* Header */}
        <div className="shrink-0 border-b border-gray-700 px-5 py-3 pr-12">
          <div className="flex items-center gap-2 mb-1">
            <GitCommit className="w-4 h-4 text-blue-400 shrink-0" />
            <span className="text-sm font-mono text-blue-400">{(sha || '').substring(0, 12)}</span>
            {org && (
              <a
                href={`https://github.com/${org}/${slug}/commit/${sha}`}
                target="_blank"
                rel="noopener noreferrer"
                className="text-gray-500 hover:text-blue-400 transition-colors"
                title="Voir sur GitHub"
              >
                <ExternalLink className="w-3.5 h-3.5" />
              </a>
            )}
          </div>
          {detail && (
            <>
              <p className="text-sm text-gray-100 font-medium">{detail.subject || '--'}</p>
              {detail.body && (
                <p className="text-xs text-gray-400 mt-1 whitespace-pre-wrap">{detail.body}</p>
              )}
              <div className="flex flex-wrap items-center gap-x-4 gap-y-0.5 mt-2 text-[11px] text-gray-500">
                <span>
                  {detail.author_name} &lt;{detail.author_email}&gt;
                </span>
                <span>{timeAgo(detail.author_date)}</span>
                <span className="text-green-500">+{detail.additions}</span>
                <span className="text-red-500">−{detail.deletions}</span>
                {detail.parents?.length > 0 && (
                  <span className="font-mono text-gray-600">
                    parent {detail.parents.map((p) => p.substring(0, 7)).join(' ')}
                  </span>
                )}
              </div>
            </>
          )}
        </div>

        <button onClick={onClose} className="absolute top-3 right-3 text-gray-400 hover:text-gray-200">
          <X className="w-5 h-5" />
        </button>

        {/* Body */}
        <div className="flex-1 overflow-y-auto p-4 space-y-3">
          {loading ? (
            <div className="flex items-center justify-center py-12">
              <Loader2 className="w-6 h-6 text-blue-400 animate-spin" />
            </div>
          ) : error ? (
            <p className="text-sm text-red-400">{error}</p>
          ) : detail ? (
            <>
              {detail.files?.length > 0 && (
                <div className="space-y-0.5">
                  {detail.files.map((f, i) => (
                    <div key={i} className="flex items-center gap-2 text-xs">
                      <FileStatusBadge status={f.status} />
                      <span className="font-mono text-gray-300 truncate">
                        {f.old_path ? `${f.old_path} → ${f.path}` : f.path}
                      </span>
                      <span className="ml-auto shrink-0 text-green-500">+{f.additions}</span>
                      <span className="shrink-0 text-red-500">−{f.deletions}</span>
                    </div>
                  ))}
                </div>
              )}
              <DiffView patch={detail.patch} truncated={detail.truncated} />
            </>
          ) : null}
        </div>

        {/* Footer */}
        <div className="shrink-0 border-t border-gray-700 px-4 py-3 flex justify-end bg-gray-900/50">
          <Button variant="secondary" onClick={onClose}>
            Fermer
          </Button>
        </div>
      </div>
    </div>
  );
}
