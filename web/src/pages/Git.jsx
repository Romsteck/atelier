import { useState, useEffect, useCallback } from 'react';
import {
  GitBranch, GitCommit, Key, RefreshCw, ExternalLink, Copy,
  Eye, EyeOff, Settings, Loader2, Save, Clock, HardDrive,
  GitMerge, ArrowUpCircle
} from 'lucide-react';
import PageHeader from '../components/PageHeader';
import Button from '../components/Button';
import {
  getGitRepos, getGitCommits, getGitBranches,
  triggerGitMirrorSync, syncAllGitRepos, getGitSshKey,
  generateGitSshKey, getGitConfig, updateGitConfig
} from '../api/client';

const timeAgo = (dateStr) => {
  if (!dateStr) return '--';
  const now = Date.now();
  const d = new Date(dateStr).getTime();
  const diff = Math.floor((now - d) / 1000);
  if (diff < 60) return 'quelques secondes';
  if (diff < 3600) return `${Math.floor(diff / 60)}min`;
  if (diff < 86400) return `${Math.floor(diff / 3600)}h`;
  if (diff < 604800) return `${Math.floor(diff / 86400)}j`;
  return new Date(dateStr).toLocaleDateString('fr-FR');
};

const formatBytes = (bytes) => {
  if (!bytes || bytes === 0) return '0 B';
  const k = 1024;
  const sizes = ['B', 'KB', 'MB', 'GB'];
  const i = Math.floor(Math.log(bytes) / Math.log(k));
  return `${parseFloat((bytes / Math.pow(k, i)).toFixed(1))} ${sizes[i]}`;
};

function Git() {
  const [repos, setRepos] = useState([]);
  const [selectedRepo, setSelectedRepo] = useState(null);
  const [commits, setCommits] = useState([]);
  const [branches, setBranches] = useState([]);
  const [sshKey, setSshKey] = useState(null);
  const [config, setConfig] = useState(null);
  const [loading, setLoading] = useState(true);
  const [showConfig, setShowConfig] = useState(false);
  const [message, setMessage] = useState(null);
  const [showToken, setShowToken] = useState(false);
  const [tokenInput, setTokenInput] = useState('');
  const [orgInput, setOrgInput] = useState('');
  const [savingConfig, setSavingConfig] = useState(false);
  const [generatingKey, setGeneratingKey] = useState(false);
  const [syncing, setSyncing] = useState({});
  const [syncingAll, setSyncingAll] = useState(false);
  const [loadingDetail, setLoadingDetail] = useState(false);
  const [activityLog, setActivityLog] = useState([]);

  const addActivity = (action, slug = null) => {
    const id = Date.now();
    setActivityLog(prev => [{ id, time: new Date(), action, slug, status: 'pending' }, ...prev]);
    return id;
  };

  const updateActivity = (id, status, detail = null) => {
    setActivityLog(prev => prev.map(e => e.id === id ? { ...e, status, detail } : e));
  };

  const fetchRepos = useCallback(async () => {
    try {
      const res = await getGitRepos();
      setRepos(res.data?.repos || res.data || []);
    } catch {
      setMessage({ type: 'error', text: 'Erreur lors du chargement des depots' });
    } finally {
      setLoading(false);
    }
  }, []);

  const fetchSshKey = useCallback(async () => {
    try {
      const res = await getGitSshKey();
      setSshKey(res.data);
    } catch {
      setSshKey(null);
    }
  }, []);

  const fetchConfig = useCallback(async () => {
    try {
      const res = await getGitConfig();
      setConfig(res.data);
      // Don't prefill with masked token — keep empty unless user types a new one
      const t = res.data?.github_token || '';
      setTokenInput(t.includes('...') ? '' : t);
      setOrgInput(res.data?.github_org || '');
    } catch {
      setConfig(null);
    }
  }, []);

  useEffect(() => {
    fetchRepos();
    fetchSshKey();
    fetchConfig();
  }, [fetchRepos, fetchSshKey, fetchConfig]);

  const handleSelectRepo = async (slug) => {
    if (selectedRepo === slug) return;
    setSelectedRepo(slug);
    setLoadingDetail(true);
    setCommits([]);
    setBranches([]);
    try {
      const [commitsRes, branchesRes] = await Promise.all([
        getGitCommits(slug).catch(() => ({ data: { commits: [] } })),
        getGitBranches(slug).catch(() => ({ data: { branches: [] } })),
      ]);
      setCommits(commitsRes.data?.commits || commitsRes.data || []);
      setBranches(branchesRes.data?.branches || branchesRes.data || []);
    } catch {
      setMessage({ type: 'error', text: 'Erreur lors du chargement du depot' });
    } finally {
      setLoadingDetail(false);
    }
  };

  const handleGenerateKey = async () => {
    const logId = addActivity('Cle SSH');
    setGeneratingKey(true);
    try {
      const res = await generateGitSshKey();
      setSshKey(res.data);
      setMessage({ type: 'success', text: 'Cle SSH generee' });
      updateActivity(logId, 'ok');
    } catch {
      setMessage({ type: 'error', text: 'Erreur lors de la generation de la cle' });
      updateActivity(logId, 'error', 'Erreur lors de la generation de la cle');
    } finally {
      setGeneratingKey(false);
    }
  };

  const handleSaveConfig = async () => {
    const hasExistingToken = !!config?.github_token;
    if ((!tokenInput.trim() && !hasExistingToken) || !orgInput.trim()) {
      setMessage({ type: 'error', text: 'Le token et l\'organisation sont requis' });
      return;
    }
    const logId = addActivity('Config');
    setSavingConfig(true);
    const payload = { github_org: orgInput };
    if (tokenInput.trim()) payload.github_token = tokenInput;
    try {
      await updateGitConfig(payload);
      setMessage({ type: 'success', text: 'Configuration sauvegardee' });
      updateActivity(logId, 'ok');
      fetchConfig();
    } catch {
      setMessage({ type: 'error', text: 'Erreur lors de la sauvegarde' });
      updateActivity(logId, 'error', 'Erreur lors de la sauvegarde');
    } finally {
      setSavingConfig(false);
    }
  };

  const handleCopyKey = () => {
    const key = sshKey?.public_key || sshKey?.key || '';
    if (key) {
      navigator.clipboard.writeText(key);
      setMessage({ type: 'success', text: 'Cle copiee' });
    }
  };

  const handleSync = async (slug) => {
    const logId = addActivity('Sync', slug);
    setSyncing(prev => ({ ...prev, [slug]: true }));
    try {
      await triggerGitMirrorSync(slug);
      setMessage({ type: 'success', text: `Synchronisation de ${slug} lancee` });
      updateActivity(logId, 'ok');
      fetchRepos();
      fetchConfig();
    } catch {
      setMessage({ type: 'error', text: `Erreur de synchronisation pour ${slug}` });
      updateActivity(logId, 'error', `Erreur de synchronisation pour ${slug}`);
    } finally {
      setSyncing(prev => ({ ...prev, [slug]: false }));
    }
  };

  const handleSyncAll = async () => {
    const logId = addActivity('Sync All');
    setSyncingAll(true);
    try {
      const res = await syncAllGitRepos();
      const count = res.data?.synced || res.data?.count || 'tous les';
      setMessage({ type: 'success', text: `Synchronisation de ${count} depots lancee` });
      updateActivity(logId, 'ok', `${count} depots synchronises`);
      if (res.data?.mirrors) {
        setConfig(prev => ({ ...prev, mirrors: res.data.mirrors }));
      }
      fetchRepos();
      fetchConfig();
    } catch {
      setMessage({ type: 'error', text: 'Erreur lors de la synchronisation globale' });
      updateActivity(logId, 'error', 'Erreur lors de la synchronisation globale');
    } finally {
      setSyncingAll(false);
    }
  };

  // Auto-dismiss messages
  useEffect(() => {
    if (!message) return;
    const t = setTimeout(() => setMessage(null), 4000);
    return () => clearTimeout(t);
  }, [message]);

  const selectedRepoData = repos.find(r => r.slug === selectedRepo);
  const mc = selectedRepo ? (config?.mirrors?.[selectedRepo] || {}) : {};

  if (loading) {
    return (
      <div className="h-full flex flex-col">
        <PageHeader icon={GitBranch} title="Git" />
        <div className="flex-1 flex items-center justify-center">
          <Loader2 className="w-8 h-8 text-blue-400 animate-spin" />
        </div>
      </div>
    );
  }

  return (
    <div className="h-full flex flex-col">
      <PageHeader icon={GitBranch} title="Git">
        {orgInput && config?.github_token && (
          <Button variant="secondary" onClick={handleSyncAll} loading={syncingAll} title="Sync All">
            <ArrowUpCircle className="w-4 h-4" />
          </Button>
        )}
        <Button variant="secondary" onClick={() => setShowConfig(!showConfig)}>
          <Settings className="w-4 h-4" />
        </Button>
        <Button variant="secondary" onClick={() => { setLoading(true); fetchRepos(); }}>
          <RefreshCw className="w-4 h-4" />
        </Button>
      </PageHeader>

      {/* Toast message */}
      {message && (
        <div className={`mx-6 mt-3 text-sm px-3 py-2 flex items-center justify-between ${
          message.type === 'error'
            ? 'text-red-400 bg-red-900/20 border border-red-800'
            : 'text-green-400 bg-green-900/20 border border-green-800'
        }`}>
          <span>{message.text}</span>
          <button onClick={() => setMessage(null)} className="ml-3 text-gray-500 hover:text-gray-300">&times;</button>
        </div>
      )}

      {/* Config panel (SSH key + GitHub token) */}
      {showConfig && (
        <div className="border-b border-gray-700 bg-gray-900">
          <div className="px-4 sm:px-6 py-3 border-b border-gray-700/50">
            <h2 className="text-sm font-semibold text-gray-400 uppercase tracking-wider">Configuration GitHub</h2>
          </div>
          <div className="px-4 sm:px-6 py-4 space-y-4">
            {/* SSH Key */}
            <div>
              <label className="block text-xs font-medium text-gray-400 uppercase tracking-wider mb-2">
                Cle SSH pour le mirroring
              </label>
              <p className="text-xs text-gray-500 mb-2">
                Ajoutez cette cle publique comme Deploy Key sur GitHub pour autoriser le push automatique.
              </p>
              {sshKey?.public_key || sshKey?.key ? (
                <div className="flex gap-2">
                  <div className="flex-1 bg-gray-800 border border-gray-700 text-gray-300 text-xs font-mono px-3 py-2 break-all select-all">
                    {sshKey.public_key || sshKey.key || ''}
                  </div>
                  <button
                    onClick={handleCopyKey}
                    className="p-2 text-gray-400 hover:text-white hover:bg-gray-700 transition-colors self-start"
                    title="Copier"
                  >
                    <Copy className="w-4 h-4" />
                  </button>
                </div>
              ) : (
                <div className="flex items-center gap-3">
                  <span className="text-sm text-gray-500">Aucune cle generee</span>
                  <Button onClick={handleGenerateKey} loading={generatingKey} className="text-xs px-3 py-1.5">
                    <Key className="w-3.5 h-3.5" /> Generer
                  </Button>
                </div>
              )}
            </div>

            {/* GitHub Token + Organisation */}
            <div>
              <label className="block text-xs font-medium text-gray-400 uppercase tracking-wider mb-2">
                Token GitHub (Personal Access Token)
              </label>
              <p className="text-xs text-gray-500 mb-2">
                Necessaire pour creer automatiquement les repos sur GitHub lors de l'activation du mirror.
              </p>
              <div className="flex items-center gap-2">
                <div className="relative flex-1">
                  <input
                    type={showToken ? 'text' : 'password'}
                    value={tokenInput}
                    onChange={(e) => setTokenInput(e.target.value)}
                    placeholder={config?.github_token ? 'Token configure (laisser vide pour garder)' : 'ghp_...'}
                    className="w-full bg-gray-800 border border-gray-700 text-gray-300 text-sm font-mono px-3 py-2 pr-10 focus:outline-none focus:border-blue-500"
                  />
                  <button
                    onClick={() => setShowToken(!showToken)}
                    className="absolute right-2 top-1/2 -translate-y-1/2 text-gray-500 hover:text-gray-300"
                  >
                    {showToken ? <EyeOff className="w-4 h-4" /> : <Eye className="w-4 h-4" />}
                  </button>
                </div>
                <input
                  type="text"
                  value={orgInput}
                  onChange={(e) => setOrgInput(e.target.value)}
                  placeholder="Organisation GitHub"
                  className="bg-gray-800 border border-gray-700 text-gray-300 text-sm px-3 py-2 w-48 focus:outline-none focus:border-blue-500"
                />
                <Button onClick={handleSaveConfig} loading={savingConfig} className="text-xs px-3 py-1.5">
                  <Save className="w-3.5 h-3.5" /> Sauvegarder
                </Button>
              </div>
              <a
                href="https://github.com/settings/tokens/new?scopes=repo&description=HomeRoute"
                target="_blank"
                rel="noopener noreferrer"
                className="inline-flex items-center gap-1 text-xs text-blue-400 hover:text-blue-300 mt-2"
              >
                <ExternalLink className="w-3 h-3" /> Generer un token
              </a>
            </div>
          </div>
        </div>
      )}

      {/* Main 3-column layout */}
      <div className="flex-1 min-h-0 flex">
        {/* Left: Repo list */}
        <div className="w-72 flex-shrink-0 border-r border-gray-700 bg-gray-800/50 flex flex-col">
          {/* List header */}
          <div className="px-4 py-2 border-b border-gray-700 bg-gray-900/80">
            <div className="flex items-center justify-between">
              <span className="text-[11px] text-gray-500 uppercase tracking-wider">
                Depots ({repos.length})
              </span>
              <span className="text-[11px] text-gray-600">
                {repos.reduce((sum, r) => sum + (r.commit_count || 0), 0)} commits
              </span>
            </div>
          </div>

          {/* List */}
          <div className="flex-1 overflow-y-auto">
            {repos.length === 0 ? (
              <div className="px-4 py-8 text-center">
                <GitBranch className="w-8 h-8 text-gray-700 mx-auto mb-2" />
                <p className="text-sm text-gray-500">Aucun depot</p>
                <p className="text-xs text-gray-600 mt-1">
                  Les depots sont crees automatiquement avec chaque container DEV.
                </p>
              </div>
            ) : (
              repos.map((repo) => {
                const isSelected = selectedRepo === repo.slug;
                const repoMc = config?.mirrors?.[repo.slug] || {};
                return (
                  <button
                    key={repo.slug}
                    onClick={() => handleSelectRepo(repo.slug)}
                    className={`w-full flex items-center gap-3 px-4 py-2.5 text-left border-l-2 transition-[background-color,color] duration-300 ease-out hover:duration-0 border-b border-b-gray-700/30 ${
                      isSelected
                        ? 'border-l-blue-400 bg-gray-900 text-white'
                        : 'border-l-transparent text-gray-300 hover:bg-gray-700/30 hover:text-gray-200'
                    }`}
                  >
                    <GitBranch className={`w-4 h-4 flex-shrink-0 ${isSelected ? 'text-blue-400' : 'text-gray-500'}`} />
                    <div className="flex-1 min-w-0">
                      <div className="flex items-center gap-2">
                        <span className="text-sm font-medium truncate">{repo.slug}</span>
                        {repoMc.enabled && (
                          <ArrowUpCircle className="w-3 h-3 text-green-500 flex-shrink-0" title="Mirror actif" />
                        )}
                      </div>
                      <div className="flex items-center gap-3 text-[11px] text-gray-500 mt-0.5">
                        <span>{repo.commit_count || 0} commits</span>
                        {repo.last_commit && (
                          <span>{timeAgo(repo.last_commit)}</span>
                        )}
                      </div>
                    </div>
                  </button>
                );
              })
            )}
          </div>
        </div>

        {/* Center: Detail panel */}
        <div className="flex-1 flex flex-col min-w-0 overflow-hidden">
          {!selectedRepo ? (
            <div className="flex-1 flex items-center justify-center">
              <div className="text-center">
                <GitBranch className="w-12 h-12 text-gray-700 mx-auto mb-3" />
                <p className="text-gray-500 text-sm">Selectionnez un depot pour voir son historique</p>
                <p className="text-gray-600 text-xs mt-1">Commits, branches et configuration du mirroring GitHub</p>
              </div>
            </div>
          ) : loadingDetail ? (
            <div className="flex-1 flex items-center justify-center">
              <Loader2 className="w-6 h-6 text-blue-400 animate-spin" />
            </div>
          ) : (
            <div className="flex-1 overflow-y-auto">
              {/* Repo header */}
              <div className="px-4 sm:px-6 py-4 border-b border-gray-700 bg-gray-800/30">
                <div className="flex items-center justify-between">
                  <div>
                    <h2 className="text-lg font-semibold text-white flex items-center gap-2">
                      {selectedRepo}
                      {orgInput && (
                        <a
                          href={`https://github.com/${orgInput}/${selectedRepo}`}
                          target="_blank"
                          rel="noopener noreferrer"
                          className="text-gray-500 hover:text-blue-400 transition-colors"
                          title={`github.com/${orgInput}/${selectedRepo}`}
                        >
                          <ExternalLink className="w-4 h-4" />
                        </a>
                      )}
                    </h2>
                    <div className="flex items-center gap-4 mt-1 text-xs text-gray-500">
                      {selectedRepoData?.head_ref && (
                        <span className="flex items-center gap-1">
                          <GitBranch className="w-3 h-3" />
                          {selectedRepoData.head_ref}
                        </span>
                      )}
                      <span className="flex items-center gap-1">
                        <GitCommit className="w-3 h-3" />
                        {selectedRepoData?.commit_count || 0} commits
                      </span>
                      {selectedRepoData?.size_bytes > 0 && (
                        <span className="flex items-center gap-1">
                          <HardDrive className="w-3 h-3" />
                          {formatBytes(selectedRepoData.size_bytes)}
                        </span>
                      )}
                      {selectedRepoData?.last_commit && (
                        <span className="flex items-center gap-1">
                          <Clock className="w-3 h-3" />
                          {timeAgo(selectedRepoData.last_commit)}
                        </span>
                      )}
                    </div>
                  </div>
                  {orgInput && config?.github_token && (
                    <Button
                      variant="secondary"
                      onClick={() => handleSync(selectedRepo)}
                      loading={syncing[selectedRepo]}
                      className="text-xs px-3 py-1.5"
                    >
                      <RefreshCw className="w-3.5 h-3.5" /> Sync GitHub
                    </Button>
                  )}
                </div>
              </div>

              {/* Branches */}
              {branches.length > 0 && (
                <div className="px-4 sm:px-6 py-3 border-b border-gray-700/50">
                  <div className="flex items-center gap-2 flex-wrap">
                    <GitMerge className="w-3.5 h-3.5 text-gray-500" />
                    <span className="text-xs text-gray-500 uppercase tracking-wider mr-1">Branches</span>
                    {branches.map((b) => (
                      <span
                        key={b.name || b}
                        className={`px-2 py-0.5 text-xs font-mono ${
                          (b.is_head || b.current)
                            ? 'bg-blue-900/30 text-blue-400 border border-blue-800/50'
                            : 'bg-gray-800 text-gray-400 border border-gray-700'
                        }`}
                      >
                        {b.name || b}
                      </span>
                    ))}
                  </div>
                </div>
              )}

              {/* Mirror / Sync details */}
              {orgInput && config?.github_token && (
                <div className="px-4 sm:px-6 py-3 border-b border-gray-700/50 bg-gray-800/20">
                  <div className="flex items-center gap-2 mb-2">
                    <ArrowUpCircle className="w-3.5 h-3.5 text-gray-500" />
                    <span className="text-xs text-gray-500 uppercase tracking-wider">GitHub Mirror</span>
                  </div>
                  <div className="space-y-1.5 text-xs">
                    <div className="flex items-center gap-2">
                      <span className="text-gray-500 w-20">SSH URL</span>
                      <span className="font-mono text-gray-400 truncate">git@github.com:{orgInput}/{selectedRepo}.git</span>
                    </div>
                    <div className="flex items-center gap-2">
                      <span className="text-gray-500 w-20">Statut</span>
                      {mc.enabled ? (
                        <span className="flex items-center gap-1.5 text-green-400">
                          <span className="w-2 h-2 rounded-full bg-green-500" /> Actif
                        </span>
                      ) : (
                        <span className="text-gray-600">Non configure</span>
                      )}
                    </div>
                    <div className="flex items-center gap-2">
                      <span className="text-gray-500 w-20">Derniere sync</span>
                      <span className="text-gray-400">{mc.last_sync ? timeAgo(mc.last_sync) : '--'}</span>
                    </div>
                    {mc.last_error && (
                      <div className="flex items-start gap-2">
                        <span className="text-gray-500 w-20 flex-shrink-0">Erreur</span>
                        <span className="text-red-400 break-all">{mc.last_error}</span>
                      </div>
                    )}
                  </div>
                </div>
              )}

              {/* Commits */}
              <div>
                <div className="px-4 sm:px-6 py-2 border-b border-gray-700 bg-gray-900/80 sticky top-0 z-10">
                  <div className="flex items-center gap-2">
                    <GitCommit className="w-3.5 h-3.5 text-gray-500" />
                    <span className="text-xs text-gray-500 uppercase tracking-wider">
                      Historique ({commits.length})
                    </span>
                  </div>
                </div>
                {commits.length === 0 ? (
                  <div className="px-4 sm:px-6 py-8 text-center">
                    <GitCommit className="w-8 h-8 text-gray-700 mx-auto mb-2" />
                    <p className="text-sm text-gray-500">Aucun commit</p>
                    <p className="text-xs text-gray-600 mt-1">
                      Poussez du code depuis votre container pour voir l'historique ici.
                    </p>
                  </div>
                ) : (
                  <div>
                    {commits.map((c, i) => (
                      <div
                        key={c.hash || i}
                        className="px-4 sm:px-6 py-2.5 border-b border-gray-700/30 hover:bg-gray-800/50 transition-colors"
                      >
                        <div className="flex items-start gap-3">
                          <span className="text-xs font-mono text-blue-400 bg-blue-900/20 px-1.5 py-0.5 mt-0.5 flex-shrink-0">
                            {(c.hash || '').substring(0, 7)}
                          </span>
                          <div className="flex-1 min-w-0">
                            <p className="text-sm text-gray-200 truncate">
                              {c.message || '--'}
                            </p>
                            <p className="text-xs text-gray-500 mt-0.5">
                              {c.author || c.author_name || '--'}
                              <span className="mx-1.5 text-gray-700">&middot;</span>
                              {timeAgo(c.date || c.timestamp)}
                            </p>
                          </div>
                        </div>
                      </div>
                    ))}
                  </div>
                )}
              </div>
            </div>
          )}
        </div>

        {/* Right: Activity panel */}
        <div className="w-80 flex-shrink-0 border-l border-gray-700 bg-gray-800/30 flex flex-col">
          <div className="px-4 py-2 border-b border-gray-700 bg-gray-900/80">
            <span className="text-[11px] text-gray-500 uppercase tracking-wider">
              Activite ({activityLog.length})
            </span>
          </div>
          <div className="flex-1 overflow-y-auto">
            {activityLog.length === 0 ? (
              <div className="px-4 py-8 text-center">
                <p className="text-xs text-gray-600">Aucune activite</p>
              </div>
            ) : (
              activityLog.map(entry => (
                <div key={entry.id} className="px-4 py-2 border-b border-gray-700/30 text-xs">
                  <div className="flex items-center gap-2">
                    {entry.status === 'pending' && <Loader2 className="w-3 h-3 text-blue-400 animate-spin flex-shrink-0" />}
                    {entry.status === 'ok' && <span className="w-2 h-2 rounded-full bg-green-500 flex-shrink-0" />}
                    {entry.status === 'error' && <span className="w-2 h-2 rounded-full bg-red-500 flex-shrink-0" />}
                    <span className="text-gray-300 font-medium">{entry.action}</span>
                    {entry.slug && <span className="text-gray-500 font-mono truncate">{entry.slug}</span>}
                    <span className="text-gray-600 ml-auto flex-shrink-0">{timeAgo(entry.time)}</span>
                  </div>
                  {entry.detail && (
                    <p className={`mt-1 pl-5 truncate ${entry.status === 'error' ? 'text-red-400' : 'text-gray-500'}`}>
                      {entry.detail}
                    </p>
                  )}
                </div>
              ))
            )}
          </div>
        </div>
      </div>
    </div>
  );
}

export default Git;
