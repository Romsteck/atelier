import axios from 'axios';

const api = axios.create({
  baseURL: '/api',
  timeout: 30000,
  withCredentials: true  // Enable cookies for session-based auth
});

// Interceptor to handle session expiration
api.interceptors.response.use(
  (response) => {
    // Check if response indicates session expired
    if (response.data && response.data.success === false && response.data.error === 'Session expiree') {
      document.cookie = 'auth_session=; path=/; expires=Thu, 01 Jan 1970 00:00:00 UTC; domain=' + window.location.hostname;
      document.cookie = 'auth_session=; path=/; expires=Thu, 01 Jan 1970 00:00:00 UTC';
    }
    return response;
  },
  (error) => {
    // Handle 401 errors
    if (error.response && error.response.status === 401) {
      // Force cookie deletion
      document.cookie = 'auth_session=; path=/; expires=Thu, 01 Jan 1970 00:00:00 UTC; domain=' + window.location.hostname;
      document.cookie = 'auth_session=; path=/; expires=Thu, 01 Jan 1970 00:00:00 UTC';
    }
    return Promise.reject(error);
  }
);

/**
 * Unwrap the API envelope: {data: X, success: true} → X
 * Use this in new pages instead of accessing res.data.data manually.
 * Legacy pages that check res.data.success should NOT use this.
 */
export function unwrapApi(res) {
  const body = res.data;
  if (body && typeof body === 'object' && 'data' in body) return body.data;
  return body;
}

// Dashboard (aggregated)
export const getDashboard = () => api.get('/dashboard');

// Services Status
export const getServicesStatus = () => api.get('/services/status');

// DNS/DHCP
export const getDnsConfig = () => api.get('/dns-dhcp/config');
export const getDhcpLeases = () => api.get('/dns-dhcp/leases');

// AdBlock
export const getAdblockStats = () => api.get('/adblock/stats');
export const getWhitelist = () => api.get('/adblock/whitelist');
export const addToWhitelist = (domain) => api.post('/adblock/whitelist', { domain });
export const removeFromWhitelist = (domain) => api.delete(`/adblock/whitelist/${domain}`);
export const updateAdblockLists = () => api.post('/adblock/update');
export const searchBlocked = (query) => api.get('/adblock/search', { params: { q: query } });

// DDNS
export const getDdnsStatus = () => api.get('/ddns/status');
export const forceDdnsUpdate = () => api.post('/ddns/update');
export const updateDdnsToken = (token) => api.put('/ddns/token', { token });
export const updateDdnsConfig = (config) => api.put('/ddns/config', config);

// Homeroute — intégration reverse-proxy. Atelier appelle l'API hr-api EXISTANTE
// de Homeroute pour attribuer des hostnames aux apps (DNS + TLS wildcard auto).
export const getHomerouteSettings = () => api.get('/homeroute/settings');
export const setHomerouteSettings = (body) => api.put('/homeroute/settings', body);
export const testHomeroute = () => api.post('/homeroute/test');
export const getHomerouteAppRoutes = () => api.get('/homeroute/app-routes');
export const assignHomerouteRoute = (slug, body = {}) =>
  api.post(`/homeroute/app-routes/${slug}`, body);
export const removeHomerouteRoute = (slug) =>
  api.delete(`/homeroute/app-routes/${slug}`);
export const toggleHomerouteRoute = (slug) =>
  api.post(`/homeroute/app-routes/${slug}/toggle`);

// Auth - Session (login page)
export const login = (code, remember_me = false) => api.post('/auth/login', { code, remember_me });
export const logout = () => api.post('/auth/logout');
export const getMe = () => api.get('/auth/me');
export const changeCode = (new_code) => api.post('/auth/change-code', { new_code });

// System Updates
export const getUpdatesStatus = () => api.get('/updates/status');
export const getLastUpdatesCheck = () => api.get('/updates/last');
export const checkForUpdates = () => api.post('/updates/check', {}, { timeout: 300000 });
export const cancelUpdatesCheck = () => api.post('/updates/cancel');

// System Updates - Upgrade actions
export const getUpgradeStatus = () => api.get('/updates/upgrade/status');
export const runAptUpgrade = () => api.post('/updates/upgrade/apt', {}, { timeout: 1800000 });
export const runAptFullUpgrade = () => api.post('/updates/upgrade/apt-full', {}, { timeout: 1800000 });
export const runSnapRefresh = () => api.post('/updates/upgrade/snap', {}, { timeout: 1800000 });
export const cancelUpgrade = () => api.post('/updates/upgrade/cancel');

// Energy
export const getEnergyHosts = () => api.get('/energy/hosts');
export const getCpuInfo = (host = 'medion') => api.get('/energy/cpu', { params: { host } });
export const getCurrentEnergyMode = (host = 'medion') => api.get('/energy/mode', { params: { host } });
export const setEnergyMode = (mode, host = 'medion') => api.post(`/energy/mode/${mode}`, null, { params: { host } });
export const getEnergySchedule = () => api.get('/energy/schedule');
export const saveEnergySchedule = (config) => api.post('/energy/schedule', config);
export const getBenchmarkStatus = () => api.get('/energy/benchmark');
export const startBenchmark = (duration = 60) => api.post('/energy/benchmark/start', { duration });
export const stopBenchmark = () => api.post('/energy/benchmark/stop');
export const setGovernorCore = (core, governor, host = 'medion') =>
  api.post(`/energy/governor/${core}`, { governor }, { params: { host } });
export const setGovernorAll = (governor, host = 'medion') =>
  api.post('/energy/governor/all', { governor }, { params: { host } });


export default api;

// ========== Hosts (unified servers + WoL) ==========

export const getHosts = () => api.get('/hosts');
export const addHost = (data) => api.post('/hosts', data);
export const updateHost = (id, data) => api.put(`/hosts/${id}`, data);
export const deleteHost = (id) => api.delete(`/hosts/${id}`);
export const testHostConnection = (id) => api.post(`/hosts/${id}/test`);
// Hosts - Power actions
export const wakeHost = (id) => api.post(`/hosts/${id}/wake`);
export const shutdownHost = (id) => api.post(`/hosts/${id}/shutdown`);
export const rebootHost = (id) => api.post(`/hosts/${id}/reboot`);

export const setWolMac = (id, mac) => api.post(`/hosts/${id}/wol-mac`, { mac });
export const setHostRole = (id, role) => api.put(`/hosts/${id}/role`, { role });
export const updateHostAgents = () => api.post('/hosts/agents/update');
export const updateLocalHostConfig = (data) => api.put('/hosts/local/config', data);
export const getLocalInterfaces = () => api.get('/hosts/local/interfaces');


// Edge Stats
export const getEdgeStats = () => api.get('/edge/stats');

// ========== Git ==========
export const getGitRepos = () => api.get('/git/repos');
export const getGitRepo = (slug) => api.get(`/git/repos/${slug}`);
export const deleteGitRepo = (slug) => api.delete(`/git/repos/${slug}`);
export const getGitCommits = (slug, limit = 50) => api.get(`/git/repos/${slug}/commits`, { params: { limit } });
export const getGitCommitDetail = (slug, sha) => api.get(`/git/repos/${slug}/commits/${sha}`);
export const getGitActivity = (slug, days = 365) => api.get(`/git/repos/${slug}/activity`, { params: { days } });
export const getGitBranches = (slug) => api.get(`/git/repos/${slug}/branches`);
export const triggerGitMirrorSync = (slug) => api.post(`/git/repos/${slug}/mirror/sync`);
export const syncAllGitRepos = () => api.post('/git/repos/sync-all');
export const getGitSshKey = () => api.get('/git/ssh-key');
export const generateGitSshKey = () => api.post('/git/ssh-key');
export const getGitConfig = () => api.get('/git/config');
export const updateGitConfig = (config) => api.put('/git/config', config);
export const getStoreApp = (slug) => api.get(`/store/apps/${slug}`);
export const checkStoreUpdates = (installed) => {
  const param = installed.map(i => `${i.slug}:${i.version}`).join(',');
  return api.get(`/store/updates?installed=${param}`);
};
export const downloadStoreRelease = (slug, version) => {
  const a = document.createElement('a');
  a.href = `/api/store/releases/${slug}/${version}/download`;
  a.download = '';
  document.body.appendChild(a);
  a.click();
  document.body.removeChild(a);
};

// Unified Updates
export const scanAllUpdates = () => api.post('/updates/scan-all');
export const getScanResults = () => api.get('/updates/scan-all/results');
export const upgradeTarget = (targetId, category) =>
  api.post('/updates/upgrade-target', { target_id: targetId, category }, { timeout: 1800000 });
export const getUpdateHistory = (limit = 50) => api.get('/updates/history', { params: { limit } });
export const getUpdateCount = () => api.get('/updates/count');
export const upgradeAllHosts = () => api.post('/updates/upgrade-hosts', {}, { timeout: 1800000 });

// ========== Backup (restic + rclone → Samba) ==========
export const getBackupStatus = () => api.get('/backup/status');
export const getBackupTarget = () => api.get('/backup/target');
export const setBackupTarget = (body) => api.put('/backup/target', body);
export const testBackupTarget = () => api.post('/backup/target/test');
export const discoverShares = (body) => api.post('/backup/discover', body);
export const revealResticPassword = () => api.get('/backup/repo/password');
export const runBackup = () => api.post('/backup/run');
export const cancelBackup = (id) => api.post(`/backup/run/${id}/cancel`);
export const getBackupRuns = (limit = 50, offset = 0) =>
  api.get('/backup/runs', { params: { limit, offset } });
export const getBackupRunDetail = (id) => api.get(`/backup/runs/${id}`);

// ========== Apps ==========
export const listApps = () => api.get('/apps');
export const getApp = (slug) => api.get(`/apps/${slug}`);
export const createApp = (data) => api.post('/apps', data);
export const updateApp = (slug, data) => api.patch(`/apps/${slug}`, data);
export const deleteApp = (slug) => api.delete(`/apps/${slug}`);
export const controlApp = (slug, action) => api.post(`/apps/${slug}/control`, { action });
export const getAppStatus = (slug) => api.get(`/apps/${slug}/status`);
export const getAppLogs = (slug, params) => api.get(`/apps/${slug}/logs`, { params });
// Env management (structured, ownership-aware). getAppEnv returns the full
// view (platform + user tiers); secret values are masked unless reveal=true.
// Per-variable user CRUD via setAppEnvVar / deleteAppEnvVar.
export const getAppEnv = (slug, reveal = false) =>
  api.get(`/apps/${slug}/env`, { params: reveal ? { reveal: 1 } : {} });
export const getAppEnvVar = (slug, key) =>
  api.get(`/apps/${slug}/env/${encodeURIComponent(key)}`);
export const setAppEnvVar = (slug, key, body) =>
  api.put(`/apps/${slug}/env/${encodeURIComponent(key)}`, body);
export const deleteAppEnvVar = (slug, key) =>
  api.delete(`/apps/${slug}/env/${encodeURIComponent(key)}`);
// Apps DB
export const getAppDbTables = (slug, { counts } = {}) =>
  api.get(`/apps/${slug}/db/tables`, counts ? { params: { counts: 1 } } : undefined);
export const getAppDbTable = (slug, table) => api.get(`/apps/${slug}/db/tables/${table}`);
export const queryAppDbRows = (slug, table, body) => api.post(`/apps/${slug}/db/tables/${table}/rows`, body);
// Admin row writes — routed through the dataverse engine (postgres-dataverse).
// No raw SQL: inserts/updates/deletes go through these typed endpoints.
export const insertAppDbRow = (slug, table, row) => api.post(`/apps/${slug}/db/tables/${table}/insert`, row);
export const updateAppDbRow = (slug, table, id, patch) => api.patch(`/apps/${slug}/db/tables/${table}/rows/${id}`, patch);
export const deleteAppDbRow = (slug, table, id) => api.delete(`/apps/${slug}/db/tables/${table}/rows/${id}`);
export const getAppDbSchema = (slug) => api.get(`/apps/${slug}/db/schema`);
export const syncAppDbSchema = (slug) => api.post(`/apps/${slug}/db/sync`);
export const createAppDbTable = (slug, body) => api.post(`/apps/${slug}/db/tables`, body);
export const dropAppDbTable = (slug, table) => api.delete(`/apps/${slug}/db/tables/${table}`);
export const addAppDbColumn = (slug, table, body) => api.post(`/apps/${slug}/db/tables/${table}/columns`, body);
export const removeAppDbColumn = (slug, table, column) => api.delete(`/apps/${slug}/db/tables/${table}/columns/${column}`);
export const createAppDbRelation = (slug, body) => api.post(`/apps/${slug}/db/relations`, body);

// ========== Logs ==========
export const getLogs = (params = {}) => api.get('/logs', { params });
export const getLogStats = () => api.get('/logs/stats');

// ========== Docs (v2: structured overview/screens/features/components + mermaid) ==========
// Read-only — mutations go through MCP from the agent.
export const listDocsApps = () => api.get('/docs');
export const getDocsOverview = (appId) => api.get(`/docs/${appId}/overview`);
export const listDocsEntries = (appId, params = {}) =>
  api.get(`/docs/${appId}/entries`, { params });
export const getDocsEntry = (appId, type, name) =>
  api.get(`/docs/${appId}/${type}/${encodeURIComponent(name)}`);
export const getDocsDiagram = (appId, type, name) =>
  api.get(`/docs/${appId}/${type}/${encodeURIComponent(name)}/diagram`);
export const searchDocs = (params) => api.get('/docs/search', { params });
export const getDocsCompleteness = (appId) => api.get(`/docs/${appId}/completeness`);

// ========== Surveillance IA (3 scans : security, code_review, business) ==========
// Snapshot agrégé pour le dashboard global : par app × kind, compteurs open par
// sévérité + dernier run, plus totaux. Le détail vit dans l'onglet Studio per-app.
export const getSurveillanceOverview = () => api.get('/surveillance/overview');
export const getAppFindings = (slug, params = {}) =>
  api.get(`/apps/${slug}/findings`, { params });
// Run one of the app's three scans (kind: security | code_review | business).
export const runSurveillance = (slug, kind, trigger) =>
  api.post(`/apps/${slug}/surveillance/run`, { kind, ...(trigger ? { trigger } : {}) });
export const cancelSurveillanceRun = (slug, runId) =>
  api.post(`/apps/${slug}/surveillance/runs/${runId}/cancel`);
export const getSurveillanceTranscript = (slug, runId) =>
  api.get(`/apps/${slug}/surveillance/runs/${runId}/transcript`);
export const listSurveillanceRuns = (slug, params = {}) =>
  api.get(`/apps/${slug}/surveillance/runs`, { params });
// HARD-delete a finding (irreversible — distinct from dismiss/resolve).
export const deleteFinding = (slug, id) =>
  api.post(`/apps/${slug}/findings/${id}/delete`);
// The app's BUSINESS scan definition (label/prompt/cadence/gate/categories).
export const getScan = (slug) =>
  api.get(`/apps/${slug}/surveillance/scan`);

// Findings with an open resolution conversation right now (across all apps).
export const getResolvingFindings = () => api.get('/surveillance/resolving');

// ── Automatic sweep (app-by-app, 3 scans each; manual + scheduled) ──
export const getSurveillanceSweep = () => api.get('/surveillance/sweep');
export const startSurveillanceSweep = () => api.post('/surveillance/sweep');
export const cancelSurveillanceSweep = () => api.post('/surveillance/sweep/cancel');
export const getSweepSchedule = () => api.get('/surveillance/sweep/schedule');
export const putSweepSchedule = (body) => api.put('/surveillance/sweep/schedule', body);

// ========== Agent (Claude Agent SDK chat — session streaming) ==========
// Démarre une SESSION (1er tour) : renvoie { run_id }. Le flux arrive ensuite par
// WebSocket (type "agent:event", filtré par run_id côté UI).
// body: { prompt, effort?, images?: [{media_type, data(base64)}], ... }.
export const startAgentQuery = (slug, body) =>
  api.post(`/apps/${slug}/agent/query`, body);
// Tour utilisateur suivant dans la MÊME session (mémoire conservée).
// body: { text, images?: [{media_type, data(base64)}] }.
export const sendAgentMessage = (slug, runId, body) =>
  api.post(`/apps/${slug}/agent/runs/${runId}/message`, body);
export const cancelAgentRun = (slug, runId) =>
  api.post(`/apps/${slug}/agent/runs/${runId}/cancel`);
// Interrompt le TOUR courant (Stop) sans fermer la session : abort côté SDK, la
// conversation reste vivante pour le tour suivant. (≠ cancel qui termine la session.)
export const interruptAgentRun = (slug, runId) =>
  api.post(`/apps/${slug}/agent/runs/${runId}/interrupt`);
// Répond à une question interactive (AskUserQuestion). body: { request_id, answers?, response?, cancelled? }
export const answerAgentRun = (slug, runId, body) =>
  api.post(`/apps/${slug}/agent/runs/${runId}/answer`, body);
// Décision sur un plan proposé (ExitPlanMode). body: { request_id, approved, feedback? }
export const planDecisionAgentRun = (slug, runId, body) =>
  api.post(`/apps/${slug}/agent/runs/${runId}/plan_decision`, body);
// Change le mode EN COURS de session (setPermissionMode). body: { mode: 'plan'|'bypass' }
export const setAgentMode = (slug, runId, mode) =>
  api.post(`/apps/${slug}/agent/runs/${runId}/set_mode`, { mode });
// Change le modèle EN COURS de session (setModel). model null → défaut abonnement.
export const setAgentModel = (slug, runId, model) =>
  api.post(`/apps/${slug}/agent/runs/${runId}/set_model`, { model });
// Reprend une conversation FERMÉE (session sur disque) : nouveau process, même
// session_id, mémoire complète. = startAgentQuery avec body.resume = session_id.
export const resumeAgentQuery = (slug, sid, body) =>
  api.post(`/apps/${slug}/agent/query`, { ...body, resume: sid });
// Conversations = sessions SDK persistées. La clé stable est le session_id.
export const listConversations = (slug) =>
  api.get(`/apps/${slug}/agent/conversations`);
// Snapshot d'une conversation : { items, live, run_id }. items = transcript normalisé.
export const getConversation = (slug, sid) =>
  api.get(`/apps/${slug}/agent/conversations/${sid}`);
// Texte d'UN bloc de réflexion (chargé à la demande à l'expand ; le snapshot ne porte
// que le compteur). Renvoie { text }.
export const getThinking = (slug, sid, tidx) =>
  api.get(`/apps/${slug}/agent/conversations/${sid}/thinking/${tidx}`);
export const renameConversation = (slug, sid, title) =>
  api.patch(`/apps/${slug}/agent/conversations/${sid}`, { title });
export const deleteConversation = (slug, sid) =>
  api.delete(`/apps/${slug}/agent/conversations/${sid}`);
// État d'UI des onglets ouverts (sync cross-PC) : { tabs, active }. Autoritaire
// côté serveur ; le PUT déclenche un broadcast WS `agent:open-tabs`.
export const getAgentOpenTabs = (slug) =>
  api.get(`/apps/${slug}/agent/open-tabs`);
export const setAgentOpenTabs = (slug, body) =>
  api.put(`/apps/${slug}/agent/open-tabs`, body);
// Version SDK installée vs dernière (npm) + MAJ in-place (éphémère) du runner.
export const getSdkVersion = () => api.get('/agent/sdk/version');
// timeout long : `npm install` côté serveur peut dépasser les 30 s par défaut du client.
export const updateSdk = (version) =>
  api.post('/agent/sdk/update', version ? { version } : {}, { timeout: 200000 });

// ========== Source (explorateur fichiers + git du working tree — Studio) ==========
// Lit l'arbre de travail réel `…/{slug}/src` (≠ /git/repos qui sert les bare repos).
export const getSourceTree = (slug, path = '') =>
  api.get(`/apps/${slug}/source/tree`, { params: { path } });
export const getSourceFile = (slug, path) =>
  api.get(`/apps/${slug}/source/file`, { params: { path } });
export const getSourceGitStatus = (slug) => api.get(`/apps/${slug}/source/git/status`);
export const getSourceGitDiff = (slug, path) =>
  api.get(`/apps/${slug}/source/git/diff`, { params: { path } });
export const getSourceGitLog = (slug, limit = 50) =>
  api.get(`/apps/${slug}/source/git/log`, { params: { limit } });
export const getSourceGitShow = (slug, sha) =>
  api.get(`/apps/${slug}/source/git/show`, { params: { sha } });
// Mutations du working tree : commit (stage-all + commit) et push vers l'upstream.
export const commitSource = (slug, message) =>
  api.post(`/apps/${slug}/source/git/commit`, { message });
export const pushSource = (slug) =>
  api.post(`/apps/${slug}/source/git/push`, {}, { timeout: 60000 });
