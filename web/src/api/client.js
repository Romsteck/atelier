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

// Reverse Proxy
export const getReverseProxyConfig = () => api.get('/reverseproxy/config');
export const getReverseProxyStatus = () => api.get('/reverseproxy/status');
export const getReverseProxyHosts = () => api.get('/reverseproxy/hosts');
export const addReverseProxyHost = (host) => api.post('/reverseproxy/hosts', host);
export const updateReverseProxyHost = (id, updates) => api.put(`/reverseproxy/hosts/${id}`, updates);
export const deleteReverseProxyHost = (id) => api.delete(`/reverseproxy/hosts/${id}`);
export const toggleReverseProxyHost = (id, enabled) => api.post(`/reverseproxy/hosts/${id}/toggle`, { enabled });
export const updateBaseDomain = (baseDomain) => api.put('/reverseproxy/config/domain', { baseDomain });
export const renewCertificates = () => api.post('/reverseproxy/certificates/renew');
export const reloadProxy = () => api.post('/reverseproxy/reload');
export const getCertificatesStatus = () => api.get('/reverseproxy/certificates/status');
export const getSystemRoutes = () => api.get('/reverseproxy/system-routes');

// Rust Proxy
export const getRustProxyStatus = () => api.get('/rust-proxy/status');

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

// Store
export const getStoreApps = () => api.get('/store/apps');

// ========== Git ==========
export const getGitRepos = () => api.get('/git/repos');
export const getGitRepo = (slug) => api.get(`/git/repos/${slug}`);
export const getGitCommits = (slug, limit = 50) => api.get(`/git/repos/${slug}/commits`, { params: { limit } });
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

// ========== Backup ==========
export const getBackupStatus = () => api.get('/backup/status');
export const getBackupRepos = () => api.get('/backup/repos');
export const getBackupJobs = () => api.get('/backup/jobs');
export const triggerBackup = () => api.post('/backup/trigger');

// ========== Apps ==========
export const listApps = () => api.get('/apps');
export const getApp = (slug) => api.get(`/apps/${slug}`);
export const createApp = (data) => api.post('/apps', data);
export const updateApp = (slug, data) => api.patch(`/apps/${slug}`, data);
export const deleteApp = (slug) => api.delete(`/apps/${slug}`);
export const controlApp = (slug, action) => api.post(`/apps/${slug}/control`, { action });
export const getAppStatus = (slug) => api.get(`/apps/${slug}/status`);
export const getAppLogs = (slug, params) => api.get(`/apps/${slug}/logs`, { params });
export const getAppEnv = (slug) => api.get(`/apps/${slug}/env`);
export const updateAppEnv = (slug, env) => api.put(`/apps/${slug}/env`, { env });
// Apps DB
export const getAppDbTables = (slug) => api.get(`/apps/${slug}/db/tables`);
export const getAppDbTable = (slug, table) => api.get(`/apps/${slug}/db/tables/${table}`);
export const queryAppDb = (slug, sql, params) => api.post(`/apps/${slug}/db/query`, { sql, params });
export const executeAppDb = (slug, sql, params) => api.post(`/apps/${slug}/db/execute`, { sql, params });
export const queryAppDbRows = (slug, table, body) => api.post(`/apps/${slug}/db/tables/${table}/rows`, body);
export const getAppDbSchema = (slug) => api.get(`/apps/${slug}/db/schema`);
export const syncAppDbSchema = (slug) => api.post(`/apps/${slug}/db/sync`);
export const createAppDbTable = (slug, body) => api.post(`/apps/${slug}/db/tables`, body);
export const dropAppDbTable = (slug, table) => api.delete(`/apps/${slug}/db/tables/${table}`);
export const addAppDbColumn = (slug, table, body) => api.post(`/apps/${slug}/db/tables/${table}/columns`, body);
export const removeAppDbColumn = (slug, table, column) => api.delete(`/apps/${slug}/db/tables/${table}/columns/${column}`);
export const createAppDbRelation = (slug, body) => api.post(`/apps/${slug}/db/relations`, body);

// ── Postgres-Dataverse only ─────────────────────────────────────
// These endpoints return an explicit error for apps still on the legacy
// SQLite backend. The DbExplorer / SchemaPage branches on `app.db_backend`
// to decide whether to use them (vs queryAppDb / executeAppDb).
//
// graphqlAppDb sends a GraphQL request via the canonical
// {query, variables, operationName} envelope. Returns the standard
// `{ data: {...}, errors: [...]? }` GraphQL response shape.
export const graphqlAppDb = (slug, query, variables, operationName) =>
  api.post(`/apps/${slug}/db/graphql`, { query, variables, operationName });

// introspectAppDbSdl returns the SDL string of the app's GraphQL schema —
// preferred over crafting client-side `__schema` queries when you just
// need the types one shot (model discovery, autocompletion).
export const introspectAppDbSdl = (slug) => api.get(`/apps/${slug}/db/introspect`);

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
