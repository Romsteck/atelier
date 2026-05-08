import { useState, useEffect, useCallback } from 'react';
import { useParams, useNavigate, useSearchParams } from 'react-router-dom';
import useWebSocket from '../hooks/useWebSocket';
import {
  ArrowLeft,
  Play,
  Square,
  RefreshCw,
  Trash2,
  ExternalLink,
  Loader2,
  Save,
  Eye,
  EyeOff,
  Globe,
  Lock,
  Database,
  ScrollText,
  Settings as SettingsIcon,
  Activity,
  KeyRound,
  Code2,
} from 'lucide-react';
import {
  getApp,
  getAppStatus,
  getAppLogs,
  getAppEnv,
  updateAppEnv,
  controlApp,
  updateApp,
  deleteApp,
} from '../api/client';
import DbExplorer from './DbExplorer';

const TABS = [
  { key: 'overview', label: 'Overview', icon: Activity },
  { key: 'code', label: 'Code', icon: Code2 },
  { key: 'db', label: 'DB', icon: Database, requiresDb: true },
  { key: 'logs', label: 'Logs', icon: ScrollText },
  { key: 'env', label: 'Env', icon: KeyRound },
  { key: 'settings', label: 'Settings', icon: SettingsIcon },
];

function statusBadge(state) {
  const s = (state || '').toLowerCase();
  const map = {
    running: 'bg-green-500/15 text-green-400 border-green-500/20',
    stopped: 'bg-gray-600/30 text-gray-300 border-gray-600',
    crashed: 'bg-red-500/15 text-red-400 border-red-500/20',
    failed: 'bg-red-500/15 text-red-400 border-red-500/20',
    starting: 'bg-yellow-500/15 text-yellow-400 border-yellow-500/20',
  };
  const cls = map[s] || map.stopped;
  return (
    <span className={`inline-flex items-center px-2 py-0.5 rounded text-xs font-medium border ${cls}`}>
      {s.charAt(0).toUpperCase() + s.slice(1) || 'Stopped'}
    </span>
  );
}

function OverviewTab({ app, status, busy, onControl }) {
  const isRunning = (status?.state || app.state || '').toLowerCase() === 'running';
  const domain = app.domain || `${app.slug}.mynetwk.biz`;
  const url = `https://${domain}`;

  return (
    <div className="space-y-4">
      <div className="grid grid-cols-1 sm:grid-cols-2 lg:grid-cols-4 gap-3">
        <div className="bg-gray-800 border border-gray-700 rounded-lg p-4">
          <div className="text-xs text-gray-400 uppercase tracking-wider">État</div>
          <div className="mt-2">{statusBadge(status?.state || app.state)}</div>
        </div>
        <div className="bg-gray-800 border border-gray-700 rounded-lg p-4">
          <div className="text-xs text-gray-400 uppercase tracking-wider">PID</div>
          <div className="mt-2 text-white font-mono text-sm">{status?.pid || '-'}</div>
        </div>
        <div className="bg-gray-800 border border-gray-700 rounded-lg p-4">
          <div className="text-xs text-gray-400 uppercase tracking-wider">Port</div>
          <div className="mt-2 text-white font-mono text-sm">{app.port || status?.port || '-'}</div>
        </div>
        <div className="bg-gray-800 border border-gray-700 rounded-lg p-4">
          <div className="text-xs text-gray-400 uppercase tracking-wider">Uptime</div>
          <div className="mt-2 text-white text-sm">{status?.uptime || '-'}</div>
        </div>
      </div>

      <div className="bg-gray-800 border border-gray-700 rounded-lg p-4">
        <div className="text-xs text-gray-400 uppercase tracking-wider mb-2">URL</div>
        <a
          href={url}
          target="_blank"
          rel="noopener noreferrer"
          className="inline-flex items-center gap-2 text-blue-400 hover:text-blue-300"
        >
          {url}
          <ExternalLink className="w-4 h-4" />
        </a>
      </div>

      {(status?.cpu_pct != null || status?.mem_mb != null) && (
        <div className="grid grid-cols-2 gap-3">
          <div className="bg-gray-800 border border-gray-700 rounded-lg p-4">
            <div className="text-xs text-gray-400 uppercase tracking-wider">CPU</div>
            <div className="mt-2 text-white text-sm">{status.cpu_pct != null ? `${status.cpu_pct.toFixed(1)}%` : '-'}</div>
          </div>
          <div className="bg-gray-800 border border-gray-700 rounded-lg p-4">
            <div className="text-xs text-gray-400 uppercase tracking-wider">Mémoire</div>
            <div className="mt-2 text-white text-sm">{status.mem_mb != null ? `${status.mem_mb} MB` : '-'}</div>
          </div>
        </div>
      )}

      <div className="bg-gray-800 border border-gray-700 rounded-lg p-4">
        <div className="text-xs text-gray-400 uppercase tracking-wider mb-3">Health Check</div>
        <div className="flex items-center gap-2">
          {status?.health === 'ok' ? (
            <span className="inline-flex items-center px-2 py-0.5 rounded text-xs font-medium bg-green-500/15 text-green-400 border border-green-500/20">OK</span>
          ) : status?.health === 'fail' ? (
            <span className="inline-flex items-center px-2 py-0.5 rounded text-xs font-medium bg-red-500/15 text-red-400 border border-red-500/20">Échec</span>
          ) : (
            <span className="inline-flex items-center px-2 py-0.5 rounded text-xs font-medium bg-gray-600/30 text-gray-300 border border-gray-600">Inconnu</span>
          )}
          {app.health_path && (
            <span className="text-xs text-gray-500 font-mono">{app.health_path}</span>
          )}
        </div>
      </div>

      <div className="bg-gray-800 border border-gray-700 rounded-lg p-4">
        <div className="text-xs text-gray-400 uppercase tracking-wider mb-3">Actions</div>
        <div className="flex flex-wrap gap-2">
          {!isRunning && (
            <button
              onClick={() => onControl('start')}
              disabled={busy === 'start'}
              className="inline-flex items-center gap-2 px-3 py-1.5 text-sm bg-green-500 hover:bg-green-600 text-white rounded-md disabled:opacity-50"
            >
              {busy === 'start' ? <Loader2 className="w-4 h-4 animate-spin" /> : <Play className="w-4 h-4" />}
              Démarrer
            </button>
          )}
          {isRunning && (
            <button
              onClick={() => onControl('stop')}
              disabled={busy === 'stop'}
              className="inline-flex items-center gap-2 px-3 py-1.5 text-sm bg-yellow-500 hover:bg-yellow-600 text-white rounded-md disabled:opacity-50"
            >
              {busy === 'stop' ? <Loader2 className="w-4 h-4 animate-spin" /> : <Square className="w-4 h-4" />}
              Arrêter
            </button>
          )}
          <button
            onClick={() => onControl('restart')}
            disabled={busy === 'restart'}
            className="inline-flex items-center gap-2 px-3 py-1.5 text-sm bg-blue-500 hover:bg-blue-600 text-white rounded-md disabled:opacity-50"
          >
            {busy === 'restart' ? <Loader2 className="w-4 h-4 animate-spin" /> : <RefreshCw className="w-4 h-4" />}
            Redémarrer
          </button>
        </div>
      </div>
    </div>
  );
}

function CodeTab({ slug }) {
  const codeServerUrl = `https://codeserver.mynetwk.biz/?folder=/opt/homeroute/apps/${slug}/src`;
  return (
    <div className="flex flex-col h-full" style={{ minHeight: '70vh' }}>
      <div className="flex items-center justify-between mb-2">
        <p className="text-sm text-gray-400">
          Code-server workspace : <code className="text-blue-400">/opt/homeroute/apps/{slug}/src</code>
        </p>
        <a
          href={codeServerUrl}
          target="_blank"
          rel="noopener noreferrer"
          className="text-sm text-blue-400 hover:text-blue-300 flex items-center gap-1"
        >
          Ouvrir en plein ecran <ExternalLink className="w-3.5 h-3.5" />
        </a>
      </div>
      <iframe
        src={codeServerUrl}
        className="flex-1 w-full rounded border border-gray-700 bg-gray-900"
        style={{ minHeight: '65vh' }}
        title={`Code-server - ${slug}`}
        allow="clipboard-read; clipboard-write"
      />
    </div>
  );
}

function LogsTab({ slug }) {
  const [logs, setLogs] = useState([]);
  const [loading, setLoading] = useState(true);
  const [level, setLevel] = useState('all');
  const [error, setError] = useState(null);

  const fetchLogs = useCallback(async () => {
    setLoading(true);
    try {
      const params = { limit: 200 };
      if (level !== 'all') params.level = level;
      const res = await getAppLogs(slug, params);
      const d = res.data?.data || res.data;
      const data = d?.logs || (Array.isArray(d) ? d : []);
      setLogs(Array.isArray(data) ? data : []);
      setError(null);
    } catch (err) {
      setError(err.response?.data?.error || err.message || 'Erreur de chargement');
      setLogs([]);
    } finally {
      setLoading(false);
    }
  }, [slug, level]);

  useEffect(() => {
    fetchLogs();
  }, [fetchLogs]);

  const levelColor = (lvl) => {
    const l = (lvl || 'info').toLowerCase();
    if (l === 'error') return 'text-red-400';
    if (l === 'warn' || l === 'warning') return 'text-yellow-400';
    if (l === 'debug') return 'text-gray-500';
    return 'text-blue-300';
  };

  return (
    <div className="space-y-3">
      <div className="flex items-center gap-2 flex-wrap">
        <div className="flex gap-1">
          {['all', 'info', 'warn', 'error'].map((l) => (
            <button
              key={l}
              onClick={() => setLevel(l)}
              className={`px-2.5 py-1 text-xs rounded-md border ${
                level === l
                  ? 'bg-blue-500/20 text-blue-300 border-blue-500/30'
                  : 'bg-gray-800 text-gray-400 border-gray-700 hover:bg-gray-700'
              }`}
            >
              {l}
            </button>
          ))}
        </div>
        <button
          onClick={fetchLogs}
          disabled={loading}
          className="ml-auto inline-flex items-center gap-1.5 px-2.5 py-1 text-xs bg-gray-800 text-gray-300 border border-gray-700 rounded-md hover:bg-gray-700 disabled:opacity-50"
        >
          {loading ? <Loader2 className="w-3 h-3 animate-spin" /> : <RefreshCw className="w-3 h-3" />}
          Refresh
        </button>
      </div>

      {error && (
        <div className="bg-red-500/10 border border-red-500/30 rounded-md px-3 py-2 text-sm text-red-400">
          {error}
        </div>
      )}

      <pre className="bg-gray-900 border border-gray-700 rounded-lg p-3 text-xs font-mono text-gray-300 overflow-auto max-h-[60vh]">
        {logs.length === 0 ? (
          loading ? 'Chargement...' : 'Aucun log'
        ) : (
          logs.map((log, i) => {
            if (typeof log === 'string') return <div key={i}>{log}</div>;
            return (
              <div key={i} className="py-0.5">
                <span className="text-gray-500">{log.timestamp || ''}</span>{' '}
                <span className={levelColor(log.level)}>[{(log.level || 'info').toUpperCase()}]</span>{' '}
                <span>{log.message}</span>
              </div>
            );
          })
        )}
      </pre>
    </div>
  );
}

function EnvTab({ slug }) {
  const [envVars, setEnvVars] = useState({});
  const [text, setText] = useState('');
  const [loading, setLoading] = useState(true);
  const [saving, setSaving] = useState(false);
  const [showValues, setShowValues] = useState(false);
  const [error, setError] = useState(null);
  const [success, setSuccess] = useState(false);

  const fetchEnv = useCallback(async () => {
    try {
      const res = await getAppEnv(slug);
      const data = res.data?.env || res.data || {};
      setEnvVars(data);
      setText(
        Object.entries(data)
          .map(([k, v]) => `${k}=${v}`)
          .join('\n')
      );
      setError(null);
    } catch (err) {
      setError(err.response?.data?.error || err.message || 'Erreur de chargement');
    } finally {
      setLoading(false);
    }
  }, [slug]);

  useEffect(() => {
    fetchEnv();
  }, [fetchEnv]);

  async function handleSave() {
    setSaving(true);
    setError(null);
    setSuccess(false);
    try {
      const newEnv = {};
      text.split('\n').forEach((line) => {
        const trimmed = line.trim();
        if (!trimmed || trimmed.startsWith('#')) return;
        const idx = trimmed.indexOf('=');
        if (idx === -1) return;
        const k = trimmed.substring(0, idx).trim();
        const v = trimmed.substring(idx + 1).trim();
        if (k) newEnv[k] = v;
      });
      await updateAppEnv(slug, newEnv);
      setEnvVars(newEnv);
      setSuccess(true);
      setTimeout(() => setSuccess(false), 2000);
    } catch (err) {
      setError(err.response?.data?.error || err.message || 'Sauvegarde échouée');
    } finally {
      setSaving(false);
    }
  }

  if (loading) {
    return (
      <div className="flex items-center justify-center py-16">
        <Loader2 className="w-6 h-6 animate-spin text-gray-500" />
      </div>
    );
  }

  return (
    <div className="space-y-3">
      <div className="flex items-center gap-2">
        <button
          onClick={() => setShowValues(!showValues)}
          className="inline-flex items-center gap-1.5 px-2.5 py-1 text-xs bg-gray-800 text-gray-300 border border-gray-700 rounded-md hover:bg-gray-700"
        >
          {showValues ? <EyeOff className="w-3 h-3" /> : <Eye className="w-3 h-3" />}
          {showValues ? 'Masquer' : 'Afficher'}
        </button>
        <span className="text-xs text-gray-500">{Object.keys(envVars).length} variables</span>
        <button
          onClick={handleSave}
          disabled={saving}
          className="ml-auto inline-flex items-center gap-1.5 px-3 py-1.5 text-sm bg-blue-500 hover:bg-blue-600 text-white rounded-md disabled:opacity-50"
        >
          {saving ? <Loader2 className="w-4 h-4 animate-spin" /> : <Save className="w-4 h-4" />}
          Sauvegarder
        </button>
      </div>

      {error && (
        <div className="bg-red-500/10 border border-red-500/30 rounded-md px-3 py-2 text-sm text-red-400">
          {error}
        </div>
      )}
      {success && (
        <div className="bg-green-500/10 border border-green-500/30 rounded-md px-3 py-2 text-sm text-green-400">
          Variables sauvegardées
        </div>
      )}

      {showValues ? (
        <textarea
          value={text}
          onChange={(e) => setText(e.target.value)}
          spellCheck={false}
          className="w-full h-96 px-3 py-2 text-xs font-mono bg-gray-900 border border-gray-700 text-gray-300 rounded-md focus:outline-none focus:border-blue-500"
          placeholder="KEY=value"
        />
      ) : (
        <div className="bg-gray-900 border border-gray-700 rounded-lg p-3 max-h-96 overflow-auto">
          {Object.entries(envVars).map(([k, v]) => (
            <div key={k} className="text-xs font-mono py-1 border-b border-gray-800 last:border-0">
              <span className="text-blue-300">{k}</span>
              <span className="text-gray-500">=</span>
              <span className="text-gray-500">{'•'.repeat(Math.min((v || '').length, 16) || 4)}</span>
            </div>
          ))}
        </div>
      )}
    </div>
  );
}

function SettingsTab({ app, onUpdate, onDelete }) {
  const [name, setName] = useState(app.name || '');
  const [visibility, setVisibility] = useState(app.visibility || 'private');
  const [runCommand, setRunCommand] = useState(app.run_command || '');
  const [buildCommand, setBuildCommand] = useState(app.build_command || '');
  const [healthPath, setHealthPath] = useState(app.health_path || '');
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState(null);
  const [success, setSuccess] = useState(false);

  async function handleSave(e) {
    e.preventDefault();
    setSaving(true);
    setError(null);
    setSuccess(false);
    try {
      await onUpdate({
        name,
        visibility,
        run_command: runCommand,
        build_command: buildCommand,
        health_path: healthPath,
      });
      setSuccess(true);
      setTimeout(() => setSuccess(false), 2000);
    } catch (err) {
      setError(err.response?.data?.error || err.message || 'Sauvegarde échouée');
    } finally {
      setSaving(false);
    }
  }

  return (
    <form onSubmit={handleSave} className="space-y-4 max-w-2xl">
      {error && (
        <div className="bg-red-500/10 border border-red-500/30 rounded-md px-3 py-2 text-sm text-red-400">
          {error}
        </div>
      )}
      {success && (
        <div className="bg-green-500/10 border border-green-500/30 rounded-md px-3 py-2 text-sm text-green-400">
          Paramètres sauvegardés
        </div>
      )}

      <div>
        <label className="block text-xs font-medium text-gray-400 mb-1">Nom</label>
        <input
          type="text"
          value={name}
          onChange={(e) => setName(e.target.value)}
          className="w-full px-3 py-2 text-sm bg-gray-900 border border-gray-700 text-white rounded-md focus:outline-none focus:border-blue-500"
        />
      </div>

      <div>
        <label className="block text-xs font-medium text-gray-400 mb-2">Visibilité</label>
        <div className="flex gap-3">
          <label className="flex items-center gap-2 cursor-pointer">
            <input
              type="radio"
              checked={visibility === 'private'}
              onChange={() => setVisibility('private')}
              className="text-blue-500"
            />
            <span className="text-sm text-gray-300">Privée</span>
          </label>
          <label className="flex items-center gap-2 cursor-pointer">
            <input
              type="radio"
              checked={visibility === 'public'}
              onChange={() => setVisibility('public')}
              className="text-blue-500"
            />
            <span className="text-sm text-gray-300">Publique</span>
          </label>
        </div>
      </div>

      <div>
        <label className="block text-xs font-medium text-gray-400 mb-1">Run command</label>
        <input
          type="text"
          value={runCommand}
          onChange={(e) => setRunCommand(e.target.value)}
          placeholder="./app"
          className="w-full px-3 py-2 text-sm font-mono bg-gray-900 border border-gray-700 text-white rounded-md focus:outline-none focus:border-blue-500"
        />
      </div>

      <div>
        <label className="block text-xs font-medium text-gray-400 mb-1">Build command</label>
        <input
          type="text"
          value={buildCommand}
          onChange={(e) => setBuildCommand(e.target.value)}
          placeholder="cargo build --release"
          className="w-full px-3 py-2 text-sm font-mono bg-gray-900 border border-gray-700 text-white rounded-md focus:outline-none focus:border-blue-500"
        />
      </div>

      <div>
        <label className="block text-xs font-medium text-gray-400 mb-1">Health check path</label>
        <input
          type="text"
          value={healthPath}
          onChange={(e) => setHealthPath(e.target.value)}
          placeholder="/health"
          className="w-full px-3 py-2 text-sm font-mono bg-gray-900 border border-gray-700 text-white rounded-md focus:outline-none focus:border-blue-500"
        />
      </div>

      <div className="flex justify-end pt-2">
        <button
          type="submit"
          disabled={saving}
          className="inline-flex items-center gap-2 px-4 py-2 text-sm bg-blue-500 hover:bg-blue-600 text-white rounded-md disabled:opacity-50"
        >
          {saving ? <Loader2 className="w-4 h-4 animate-spin" /> : <Save className="w-4 h-4" />}
          Sauvegarder
        </button>
      </div>

      <div className="pt-6 mt-6 border-t border-gray-700">
        <h3 className="text-sm font-semibold text-red-400 mb-2">Zone dangereuse</h3>
        <p className="text-xs text-gray-400 mb-3">
          Supprimer cette application est irréversible. Tous les fichiers, la base de données et les variables d'environnement seront effacés.
        </p>
        <button
          type="button"
          onClick={onDelete}
          className="inline-flex items-center gap-2 px-3 py-1.5 text-sm bg-red-500/10 hover:bg-red-500/20 text-red-400 border border-red-500/30 rounded-md"
        >
          <Trash2 className="w-4 h-4" />
          Supprimer l'application
        </button>
      </div>
    </form>
  );
}

export default function AppDetail() {
  const { slug } = useParams();
  const navigate = useNavigate();
  const [searchParams, setSearchParams] = useSearchParams();
  const initialTab = searchParams.get('tab') || 'overview';
  const [tab, setTab] = useState(initialTab);
  const [app, setApp] = useState(null);
  const [status, setStatus] = useState(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState(null);
  const [busy, setBusy] = useState(null);

  const fetchApp = useCallback(async () => {
    try {
      const res = await getApp(slug);
      setApp(res.data?.data || res.data);
      setError(null);
    } catch (err) {
      setError(err.response?.data?.error || err.message || 'Erreur de chargement');
    } finally {
      setLoading(false);
    }
  }, [slug]);

  const fetchStatus = useCallback(async () => {
    try {
      const res = await getAppStatus(slug);
      setStatus(res.data?.data || res.data);
    } catch {
      // ignore
    }
  }, [slug]);

  useEffect(() => {
    fetchApp();
    fetchStatus();
  }, [fetchApp, fetchStatus]);

  // Real-time state updates via WebSocket (no polling)
  useWebSocket({
    'app:state': (data) => {
      if (data.slug === slug) {
        setStatus(prev => ({ ...prev, ...data }));
        setApp(prev => prev ? { ...prev, state: data.state } : prev);
      }
    },
  });

  function changeTab(key) {
    setTab(key);
    setSearchParams({ tab: key });
  }

  async function handleControl(action) {
    setBusy(action);
    try {
      await controlApp(slug, action);
      await new Promise((r) => setTimeout(r, 800));
      await Promise.all([fetchApp(), fetchStatus()]);
    } catch (err) {
      setError(err.response?.data?.error || err.message || 'Action échouée');
    } finally {
      setBusy(null);
    }
  }

  async function handleUpdate(data) {
    await updateApp(slug, data);
    await fetchApp();
  }

  async function handleDelete() {
    if (!confirm(`Supprimer définitivement "${slug}" ?`)) return;
    try {
      await deleteApp(slug);
      navigate('/apps');
    } catch (err) {
      setError(err.response?.data?.error || err.message || 'Suppression échouée');
    }
  }

  if (loading) {
    return (
      <div className="flex items-center justify-center h-full">
        <Loader2 className="w-6 h-6 animate-spin text-gray-500" />
      </div>
    );
  }

  if (error && !app) {
    return (
      <div className="p-6">
        <button
          onClick={() => navigate('/apps')}
          className="inline-flex items-center gap-2 text-sm text-gray-400 hover:text-white mb-4"
        >
          <ArrowLeft className="w-4 h-4" />
          Retour
        </button>
        <div className="bg-red-500/10 border border-red-500/30 rounded-md px-4 py-3 text-sm text-red-400">
          {error}
        </div>
      </div>
    );
  }

  if (!app) return null;

  const domain = app.domain || `${slug}.mynetwk.biz`;
  const url = `https://${domain}`;
  const visibleTabs = TABS.filter((t) => !t.requiresDb || app.has_db);

  return (
    <div className="flex flex-col h-full">
      <div className="bg-gray-800 border-b border-gray-700 px-4 sm:px-6 py-4">
        <button
          onClick={() => navigate('/apps')}
          className="inline-flex items-center gap-1 text-xs text-gray-400 hover:text-white mb-3"
        >
          <ArrowLeft className="w-3 h-3" />
          Applications
        </button>
        <div className="flex items-start justify-between gap-3 flex-wrap">
          <div>
            <div className="flex items-center gap-3 flex-wrap">
              <h1 className="text-xl font-semibold text-white">{app.name || slug}</h1>
              {statusBadge(status?.state || app.state)}
              <span className="inline-flex items-center px-2 py-0.5 rounded text-xs font-medium bg-purple-500/10 text-purple-300 border border-purple-500/20">
                {app.stack || '-'}
              </span>
              <span className={`inline-flex items-center gap-1 px-2 py-0.5 rounded text-xs font-medium border ${
                app.visibility === 'public'
                  ? 'bg-blue-500/10 text-blue-300 border-blue-500/20'
                  : 'bg-gray-700 text-gray-300 border-gray-600'
              }`}>
                {app.visibility === 'public' ? <Globe className="w-3 h-3" /> : <Lock className="w-3 h-3" />}
                {app.visibility === 'public' ? 'Public' : 'Privé'}
              </span>
            </div>
            <div className="mt-2 flex items-center gap-3 text-xs text-gray-400">
              <span className="font-mono">{slug}</span>
              <a
                href={url}
                target="_blank"
                rel="noopener noreferrer"
                className="inline-flex items-center gap-1 text-blue-400 hover:text-blue-300"
              >
                {domain}
                <ExternalLink className="w-3 h-3" />
              </a>
            </div>
          </div>
        </div>

        <div className="mt-4 flex gap-1 border-b border-gray-700 -mb-4">
          {visibleTabs.map((t) => {
            const Icon = t.icon;
            const isActive = tab === t.key;
            return (
              <button
                key={t.key}
                onClick={() => changeTab(t.key)}
                className={`inline-flex items-center gap-1.5 px-3 py-2 text-sm border-b-2 transition-colors ${
                  isActive
                    ? 'border-blue-400 text-blue-400'
                    : 'border-transparent text-gray-400 hover:text-gray-200'
                }`}
              >
                <Icon className="w-4 h-4" />
                {t.label}
              </button>
            );
          })}
        </div>
      </div>

      <div className="flex-1 overflow-y-auto p-4 sm:p-6">
        {error && (
          <div className="mb-4 bg-red-500/10 border border-red-500/30 rounded-md px-4 py-3 text-sm text-red-400">
            {error}
          </div>
        )}
        {tab === 'overview' && <OverviewTab app={app} status={status} busy={busy} onControl={handleControl} />}
        {tab === 'code' && <CodeTab slug={slug} />}
        {tab === 'db' && app.has_db && <DbExplorer appSlug={slug} embedded={true} />}
        {tab === 'logs' && <LogsTab slug={slug} />}
        {tab === 'env' && <EnvTab slug={slug} />}
        {tab === 'settings' && <SettingsTab app={app} onUpdate={handleUpdate} onDelete={handleDelete} />}
      </div>
    </div>
  );
}
