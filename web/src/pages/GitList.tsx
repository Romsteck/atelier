import { useEffect, useState } from "react";
import { Link } from "react-router-dom";
import { GitBranch as GitBranchIcon, Cloud } from "lucide-react";
import { GitRepo, formatBytes, listRepos } from "../api";

function timeAgo(iso: string | null): string {
  if (!iso) return "—";
  const d = new Date(iso);
  const diff = Date.now() - d.getTime();
  const days = Math.floor(diff / 86400000);
  if (days === 0) return "today";
  if (days === 1) return "1 day ago";
  if (days < 30) return `${days} days ago`;
  const months = Math.floor(days / 30);
  if (months < 12) return `${months}mo ago`;
  return `${Math.floor(months / 12)}y ago`;
}

export default function GitList() {
  const [repos, setRepos] = useState<GitRepo[] | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    listRepos()
      .then(setRepos)
      .catch((e) => setError(String(e)));
  }, []);

  if (error) return <p className="text-red-400">Erreur: {error}</p>;
  if (!repos) return <p className="text-gray-500">Chargement…</p>;

  return (
    <div>
      <p className="text-sm text-gray-500 mb-6">
        {repos.length} bare repos · rsync depuis Medion (5 min) ·{" "}
        <span className="text-amber-400">read-only</span> côté Atelier (push via{" "}
        <a
          href="https://proxy.mynetwk.biz"
          className="hover:underline"
          target="_blank"
          rel="noopener noreferrer"
        >
          proxy.mynetwk.biz
        </a>
        ).
      </p>
      <ul className="grid grid-cols-1 md:grid-cols-2 gap-3">
        {repos.map((r) => (
          <li key={r.slug}>
            <Link
              to={`/git/${r.slug}`}
              className="block p-4 bg-gray-900 border border-gray-800 rounded-md hover:border-amber-400/50 transition-colors h-full"
            >
              <div className="flex items-center gap-2 mb-1">
                <GitBranchIcon className="w-4 h-4 text-amber-400 shrink-0" />
                <span className="font-mono font-semibold text-gray-100">
                  {r.slug}.git
                </span>
                {r.mirror?.enabled && (
                  <span title={r.mirror.github_url}>
                    <Cloud className="w-3.5 h-3.5 text-blue-400" />
                  </span>
                )}
              </div>
              {r.last_commit_message && (
                <p className="text-xs text-gray-400 truncate mb-2">
                  {r.last_commit_message}
                </p>
              )}
              <div className="flex flex-wrap gap-1.5 text-[11px] text-gray-500">
                <span className="badge">{r.commit_count} commits</span>
                <span className="badge">{r.branch_count} branches</span>
                {r.default_branch && (
                  <span className="badge">{r.default_branch}</span>
                )}
                <span className="badge">{formatBytes(r.size_bytes)}</span>
                <span className="badge">{timeAgo(r.last_commit_at)}</span>
              </div>
            </Link>
          </li>
        ))}
      </ul>
    </div>
  );
}
