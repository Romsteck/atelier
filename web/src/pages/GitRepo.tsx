import { useEffect, useState } from "react";
import { Link, useParams } from "react-router-dom";
import { ChevronLeft, Copy, GitBranch as GitBranchIcon } from "lucide-react";
import {
  GitBranch,
  GitCommit,
  GitRepo,
  formatBytes,
  getBranches,
  getCommits,
  getRepo,
} from "../api";

export default function GitRepoPage() {
  const { slug } = useParams();
  const [repo, setRepo] = useState<GitRepo | null>(null);
  const [commits, setCommits] = useState<GitCommit[] | null>(null);
  const [branches, setBranches] = useState<GitBranch[] | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    if (!slug) return;
    getRepo(slug)
      .then(setRepo)
      .catch((e) => setError(String(e)));
    getCommits(slug, 50).then(setCommits);
    getBranches(slug).then(setBranches);
  }, [slug]);

  if (error) return <p className="text-red-400">Erreur: {error}</p>;
  if (!repo) return <p className="text-gray-500">Chargement…</p>;

  const cloneUrl = `https://app.mynetwk.biz/api/git/repos/${repo.slug}.git`;

  return (
    <div className="max-w-4xl">
      <Link
        to="/git"
        className="inline-flex items-center gap-1 text-sm text-gray-400 hover:text-amber-400 mb-4"
      >
        <ChevronLeft className="w-4 h-4" />
        Tous les repos
      </Link>

      <div className="flex items-center gap-2 mb-2">
        <GitBranchIcon className="w-5 h-5 text-amber-400" />
        <h2 className="text-2xl font-semibold font-mono">{repo.slug}.git</h2>
      </div>
      <div className="flex flex-wrap gap-1.5 mb-4 text-[11px]">
        {repo.default_branch && (
          <span className="badge">default: {repo.default_branch}</span>
        )}
        <span className="badge">{repo.commit_count} commits</span>
        <span className="badge">{repo.branch_count} branches</span>
        <span className="badge">{formatBytes(repo.size_bytes)}</span>
        {repo.mirror?.enabled && (
          <span
            className="badge !text-blue-400 !border-blue-900"
            title={repo.mirror.github_url}
          >
            mirror: {repo.mirror.github_url.replace("https://github.com/", "")}
          </span>
        )}
      </div>

      <section className="mb-6">
        <h3 className="text-sm font-semibold text-gray-400 uppercase tracking-wider mb-2">
          Clone (read-only)
        </h3>
        <div className="flex items-center gap-2 bg-gray-900 border border-gray-800 rounded-md p-3 font-mono text-sm">
          <span className="text-gray-500">$</span>
          <code className="flex-1 truncate">git clone {cloneUrl}</code>
          <button
            onClick={() => navigator.clipboard.writeText(`git clone ${cloneUrl}`)}
            className="p-1 text-gray-500 hover:text-amber-400 shrink-0"
            title="Copier"
          >
            <Copy className="w-3.5 h-3.5" />
          </button>
        </div>
        <p className="text-[11px] text-gray-600 mt-1">
          Le push HTTP sur cet endpoint retourne 405 — pousser via{" "}
          <code className="font-mono">proxy.mynetwk.biz</code>.
        </p>
      </section>

      {branches && branches.length > 0 && (
        <section className="mb-6">
          <h3 className="text-sm font-semibold text-gray-400 uppercase tracking-wider mb-2">
            Branches ({branches.length})
          </h3>
          <ul className="flex flex-wrap gap-1.5">
            {branches.map((b) => (
              <li key={b.name}>
                <span
                  className={`badge font-mono ${
                    b.is_default
                      ? "!text-amber-400 !border-amber-900"
                      : ""
                  }`}
                  title={b.sha}
                >
                  {b.name}
                </span>
              </li>
            ))}
          </ul>
        </section>
      )}

      <section>
        <h3 className="text-sm font-semibold text-gray-400 uppercase tracking-wider mb-2">
          Recent commits {commits ? `(${commits.length})` : ""}
        </h3>
        {!commits ? (
          <p className="text-gray-500 text-sm">Chargement…</p>
        ) : commits.length === 0 ? (
          <p className="text-gray-500 text-sm">Aucun commit.</p>
        ) : (
          <ul className="border border-gray-800 rounded-md bg-gray-900 divide-y divide-gray-800">
            {commits.map((c) => (
              <li key={c.sha} className="px-4 py-2.5">
                <div className="flex items-center gap-2 mb-0.5">
                  <span className="font-mono text-[11px] text-amber-400">
                    {c.sha.slice(0, 8)}
                  </span>
                  <span className="text-[11px] text-gray-600">
                    {c.author_name}
                  </span>
                  <span className="text-[11px] text-gray-600">
                    · {new Date(c.date).toLocaleDateString()}
                  </span>
                </div>
                <p className="text-sm text-gray-200 truncate">{c.message}</p>
              </li>
            ))}
          </ul>
        )}
      </section>
    </div>
  );
}
