import { useState, useEffect, useCallback, useRef } from 'react';
import { useSearchParams } from 'react-router-dom';
import useWebSocket from '../hooks/useWebSocket';
import { useStudio } from '../context/StudioContext';
import DbExplorer from './DbExplorer';
import TodosPanel from '../components/TodosPanel';
import StudioIframe from '../components/StudioIframe';
import DocsTab from '../components/docs/DocsTab';
import {
  Code2, BookOpen, Database, ScrollText, KeyRound, Settings as SettingsIcon,
  ExternalLink, Save, Loader2, Plus, Play, Square, Trash2, X, Globe, Lock,
  Eye, EyeOff, ChevronDown,
} from 'lucide-react';
import {
  listApps, createApp, controlApp, deleteApp, updateApp,
  getApp, getAppStatus, getAppLogs, getAppEnv, updateAppEnv, getLogs,
} from '../api/client';
import { Link } from 'react-router-dom';

export const CODESERVER_BASE = 'https://codeserver.mynetwk.biz';

const STACKS = [
  { value: 'next-js', label: 'Next.js' },
  { value: 'axum-vite', label: 'Vite+Rust' },
  { value: 'axum', label: 'Rust Only' },
];

const TABS = [
  { id: 'code', label: 'Code', icon: Code2 },
  { id: 'db', label: 'DB', icon: Database, requiresDb: true },
  { id: 'logs', label: 'Logs', icon: ScrollText },
  { id: 'docs', label: 'Docs', icon: BookOpen },
  { id: 'env', label: 'Env', icon: KeyRound },
  { id: 'settings', label: 'Settings', icon: SettingsIcon },
];

const SLUG_RE = /^[a-z][a-z0-9-]*$/;
function slugify(n) { return n.toLowerCase().replace(/\s+/g,'-').replace(/[^a-z0-9-]/g,'').replace(/-+/g,'-').replace(/^-|-$/g,''); }

export function statusDot(state) {
  const s = (state || '').toLowerCase();
  if (s === 'running') return 'bg-green-400';
  if (s === 'crashed' || s === 'failed') return 'bg-red-400';
  if (s === 'starting') return 'bg-yellow-400 animate-pulse';
  return 'bg-gray-500';
}

// ── Sidebar ──

function AppSidebar({ apps, selectedSlug, onSelect, onAdd, busy, onControl }) {
  return (
    <aside className="w-[220px] min-w-[220px] h-full bg-gray-800/50 border-r border-gray-700 flex flex-col">
      <div className="px-3 pt-3 pb-2 space-y-2">
        <span className="block text-[10px] font-semibold uppercase tracking-wider text-gray-500">Applications</span>
        <button
          onClick={onAdd}
          className="w-full flex items-center justify-center gap-1.5 px-3 py-2 text-xs font-medium text-white bg-blue-500 hover:bg-blue-600 active:bg-blue-700 rounded-md shadow-sm shadow-blue-500/20 transition-colors"
        >
          <Plus className="w-4 h-4" />
          Nouvelle application
        </button>
      </div>
      <div className="flex-1 overflow-y-auto pb-2">
        {apps.map(app => {
          const sel = app.slug === selectedSlug;
          const state = (app.state || '').toLowerCase();
          const isRunning = state === 'running';
          return (
            <div
              key={app.slug}
              className={`flex items-center gap-3 px-4 py-2 text-[13px] cursor-pointer transition-[background-color,color] duration-300 ease-out hover:duration-0 group ${
                sel
                  ? 'border-l-3 border-blue-400 bg-gray-700/50 text-white'
                  : 'border-l-3 border-transparent text-gray-300 hover:bg-gray-700/30'
              }`}
              onClick={() => onSelect(app.slug)}
            >
              <span className={`w-[7px] h-[7px] rounded-full shrink-0 ${statusDot(state)}`} />
              <span className="flex-1 truncate">{app.name}</span>
              <div className="flex items-center gap-0.5 opacity-0 group-hover:opacity-100 transition-opacity">
                {isRunning ? (
                  <button onClick={e => { e.stopPropagation(); onControl(app.slug, 'stop'); }} className="p-0.5 text-yellow-400 hover:bg-gray-600 rounded" title="Stop">
                    <Square className="w-3 h-3" />
                  </button>
                ) : (
                  <button onClick={e => { e.stopPropagation(); onControl(app.slug, 'start'); }} className="p-0.5 text-green-400 hover:bg-gray-600 rounded" title="Start">
                    <Play className="w-3 h-3" />
                  </button>
                )}
              </div>
            </div>
          );
        })}
        {apps.length === 0 && (
          <div className="text-center py-8 text-gray-500 text-xs">
            Aucune app
          </div>
        )}
      </div>
    </aside>
  );
}

// ── Code Tab ──

function CodeTab({ slug }) {
  return <StudioIframe folder={`/var/lib/atelier/apps/${slug}/src`} title={`Code - ${slug}`} />;
}

// ── Logs Tab ──

function LogsTab({ slug }) {
  const [logs, setLogs] = useState([]);
  const [filter, setFilter] = useState('');
  const [source, setSource] = useState('atelier');
  const [loading, setLoading] = useState(true);
  const [autoScroll, setAutoScroll] = useState(true);
  const ref = useRef(null);

  useEffect(() => {
    setLoading(true);
    if (source === 'atelier') {
      getLogs({ app_slug: slug, limit: 200 }).then(res => {
        const d = res.data?.logs || [];
        setLogs(Array.isArray(d) ? d : []);
      }).catch(() => setLogs([])).finally(() => setLoading(false));
    } else {
      getAppLogs(slug, { limit: 200 }).then(res => {
        const d = res.data?.data || res.data;
        const data = d?.logs || (Array.isArray(d) ? d : []);
        setLogs(Array.isArray(data) ? data : []);
      }).catch(() => setLogs([])).finally(() => setLoading(false));
    }
  }, [slug, source]);

  useEffect(() => { if (autoScroll && ref.current) ref.current.scrollTop = ref.current.scrollHeight; }, [logs, autoScroll]);

  useWebSocket({
    'log:entry': (data) => {
      if (source !== 'atelier') return;
      if (data?.app_slug !== slug) return;
      setLogs(prev => [...prev.slice(-499), data]);
    },
    'app:log': (data) => {
      if (source !== 'journalctl') return;
      if (data?.slug !== slug) return;
      setLogs(prev => [...prev.slice(-499), data]);
    },
  });

  const onScroll = () => { if (!ref.current) return; const { scrollTop, scrollHeight, clientHeight } = ref.current; setAutoScroll(scrollHeight - scrollTop - clientHeight < 50); };
  const filtered = filter ? logs.filter(l => (l.message||'').toLowerCase().includes(filter.toLowerCase()) || (l.level||'').toLowerCase().includes(filter.toLowerCase())) : logs;
  const levelCls = l => { const lw = (l||'').toLowerCase(); return lw === 'error' ? 'text-red-400' : lw === 'warn' || lw === 'warning' ? 'text-yellow-400' : 'text-gray-300'; };

  if (loading) return <div className="flex items-center justify-center h-full text-gray-500"><Loader2 className="w-5 h-5 animate-spin" /></div>;

  return (
    <div className="flex flex-col h-full">
      <div className="flex items-center gap-3 px-4 py-2 shrink-0 border-b border-gray-700">
        <div className="flex gap-1 text-xs">
          <button
            onClick={() => setSource('atelier')}
            className={`px-2 py-1 rounded ${source === 'atelier' ? 'bg-amber-600 text-white' : 'bg-gray-800 text-gray-400 hover:bg-gray-700'}`}
            title="Logs structurés Postgres (atelier-logging-shipper)"
          >
            Atelier
          </button>
          <button
            onClick={() => setSource('journalctl')}
            className={`px-2 py-1 rounded ${source === 'journalctl' ? 'bg-amber-600 text-white' : 'bg-gray-800 text-gray-400 hover:bg-gray-700'}`}
            title="Logs systemd bruts (journalctl)"
          >
            journalctl
          </button>
        </div>
        <input type="text" value={filter} onChange={e => setFilter(e.target.value)} placeholder="Filtrer..."
          className="flex-1 max-w-[300px] px-3 py-1 rounded text-sm outline-none bg-gray-900 text-white border border-gray-700" />
        <Link
          to={`/logs?app_slug=${encodeURIComponent(slug)}`}
          className="text-xs text-amber-400 hover:text-amber-300 inline-flex items-center gap-1"
          title="Voir tous les logs de cette app en plein écran"
        >
          plein écran <ExternalLink className="w-3 h-3" />
        </Link>
        <span className="text-xs text-gray-500 ml-2">{filtered.length} entrees{autoScroll ? ' (auto-scroll)' : ''}</span>
      </div>
      <div ref={ref} onScroll={onScroll} className="flex-1 overflow-y-auto p-4 font-mono text-xs">
        {filtered.map((e, i) => {
          const time = (e.timestamp||'').includes('T') ? e.timestamp.split('T')[1]?.replace('Z','').substring(0,12) : e.timestamp;
          return (
            <div key={i} className="flex gap-3 py-0.5 hover:bg-white/[0.02]">
              <span className="shrink-0 w-24 text-gray-500">{time}</span>
              <span className={`shrink-0 w-12 text-right ${levelCls(e.level)}`}>{(e.level||'').toUpperCase()}</span>
              <span className="text-gray-300">{e.message}</span>
            </div>
          );
        })}
        {filtered.length === 0 && <div className="text-center py-12 text-gray-500">{filter ? 'Aucun log correspondant' : 'Aucun log'}</div>}
      </div>
    </div>
  );
}

// ── Env Tab ──

function EnvTab({ slug }) {
  const [envText, setEnvText] = useState('');
  const [loading, setLoading] = useState(true);
  const [saving, setSaving] = useState(false);
  const [showValues, setShowValues] = useState(false);

  useEffect(() => {
    setLoading(true);
    getAppEnv(slug).then(res => {
      const d = res.data?.data || res.data;
      if (typeof d === 'object' && !Array.isArray(d)) {
        setEnvText(Object.entries(d).map(([k, v]) => `${k}=${v}`).join('\n'));
      } else {
        setEnvText('');
      }
    }).catch(() => {}).finally(() => setLoading(false));
  }, [slug]);

  const handleSave = async () => {
    setSaving(true);
    try {
      const vars = {};
      envText.split('\n').filter(l => l.trim() && !l.startsWith('#')).forEach(l => {
        const [k, ...rest] = l.split('=');
        if (k) vars[k.trim()] = rest.join('=').trim();
      });
      await updateAppEnv(slug, vars);
    } catch {}
    setSaving(false);
  };

  if (loading) return <div className="flex items-center justify-center h-full text-gray-500"><Loader2 className="w-5 h-5 animate-spin" /></div>;

  return (
    <div className="p-6 space-y-4 overflow-y-auto h-full">
      <div className="flex items-center justify-between">
        <h3 className="text-sm font-medium text-white">Variables d'environnement</h3>
        <button onClick={() => setShowValues(!showValues)} className="flex items-center gap-1 text-xs text-gray-400 hover:text-white">
          {showValues ? <EyeOff className="w-3.5 h-3.5" /> : <Eye className="w-3.5 h-3.5" />}
          {showValues ? 'Masquer' : 'Afficher'}
        </button>
      </div>
      <textarea
        value={showValues ? envText : envText.split('\n').map(l => { const [k] = l.split('='); return k ? `${k}=***` : l; }).join('\n')}
        onChange={e => { if (showValues) setEnvText(e.target.value); }}
        readOnly={!showValues}
        className="w-full h-64 p-3 rounded text-sm font-mono bg-gray-900 text-white border border-gray-700 outline-none resize-y"
        placeholder="KEY=value"
      />
      <button onClick={handleSave} disabled={saving} className="px-4 py-2 text-sm bg-blue-500 hover:bg-blue-600 text-white rounded disabled:opacity-50 flex items-center gap-1.5">
        {saving ? <Loader2 className="w-4 h-4 animate-spin" /> : <Save className="w-4 h-4" />} Sauvegarder
      </button>
    </div>
  );
}

// ── Settings Tab ──

function SettingsTab({ app, onUpdate, onDelete }) {
  const [name, setName] = useState(app?.name || '');
  const [visibility, setVisibility] = useState(app?.visibility || 'private');
  const [runCmd, setRunCmd] = useState(app?.run_command || '');
  const [buildCmd, setBuildCmd] = useState(app?.build_command || '');
  const [healthPath, setHealthPath] = useState(app?.health_path || '/api/health');
  const [saving, setSaving] = useState(false);

  useEffect(() => {
    if (app) { setName(app.name); setVisibility(app.visibility); setRunCmd(app.run_command); setBuildCmd(app.build_command || ''); setHealthPath(app.health_path); }
  }, [app]);

  const handleSave = async () => {
    setSaving(true);
    try { await onUpdate({ name, visibility, run_command: runCmd, build_command: buildCmd || null, health_path: healthPath }); } catch {}
    setSaving(false);
  };

  return (
    <div className="p-6 space-y-4 overflow-y-auto h-full max-w-xl">
      {[
        { label: 'Nom', value: name, set: setName },
        { label: 'Run command', value: runCmd, set: setRunCmd, mono: true },
        { label: 'Build command', value: buildCmd, set: setBuildCmd, mono: true },
        { label: 'Health path', value: healthPath, set: setHealthPath, mono: true },
      ].map(({ label, value, set, mono }) => (
        <div key={label}>
          <label className="block text-xs text-gray-400 mb-1">{label}</label>
          <input type="text" value={value} onChange={e => set(e.target.value)} className={`w-full px-3 py-2 text-sm bg-gray-900 border border-gray-700 text-white rounded outline-none focus:border-blue-500 ${mono ? 'font-mono' : ''}`} />
        </div>
      ))}
      <div>
        <label className="block text-xs text-gray-400 mb-1">Visibilite</label>
        <div className="flex gap-3">
          {['private', 'public'].map(v => (
            <label key={v} className="flex items-center gap-2 cursor-pointer">
              <input type="radio" checked={visibility === v} onChange={() => setVisibility(v)} className="text-blue-500" />
              <span className="text-sm text-gray-300">{v === 'private' ? 'Privee' : 'Publique'}</span>
            </label>
          ))}
        </div>
      </div>
      <button onClick={handleSave} disabled={saving} className="px-4 py-2 text-sm bg-blue-500 hover:bg-blue-600 text-white rounded disabled:opacity-50 flex items-center gap-1.5">
        {saving ? <Loader2 className="w-4 h-4 animate-spin" /> : <Save className="w-4 h-4" />} Sauvegarder
      </button>
      <div className="pt-6 border-t border-gray-700">
        <button onClick={onDelete} className="px-4 py-2 text-sm bg-red-600 hover:bg-red-700 text-white rounded flex items-center gap-1.5"><Trash2 className="w-4 h-4" /> Supprimer l'application</button>
      </div>
    </div>
  );
}

// ── Create Modal ──

function CreateAppModal({ onClose, onCreated }) {
  const [name, setName] = useState('');
  const [slug, setSlug] = useState('');
  const [slugManual, setSlugManual] = useState(false);
  const [stack, setStack] = useState('axum-vite');
  const [visibility, setVisibility] = useState('private');
  const [submitting, setSubmitting] = useState(false);
  const [error, setError] = useState(null);

  async function handleSubmit(e) {
    e.preventDefault();
    if (!name.trim()) { setError('Nom requis'); return; }
    if (!SLUG_RE.test(slug)) { setError('Slug invalide'); return; }
    setSubmitting(true); setError(null);
    try { await createApp({ name: name.trim(), slug, stack, visibility }); onCreated(); }
    catch (err) { setError(err.response?.data?.error || err.message); }
    finally { setSubmitting(false); }
  }

  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/60" onClick={onClose}>
      <div className="w-full max-w-md bg-gray-800 border border-gray-700 rounded-lg shadow-xl" onClick={e => e.stopPropagation()}>
        <div className="flex items-center justify-between px-5 py-4 border-b border-gray-700">
          <h2 className="text-lg font-semibold text-white">Nouvelle application</h2>
          <button onClick={onClose} className="text-gray-400 hover:text-white"><X className="w-5 h-5" /></button>
        </div>
        <form onSubmit={handleSubmit} className="p-5 space-y-4">
          {error && <div className="bg-red-500/10 border border-red-500/30 rounded px-3 py-2 text-sm text-red-400">{error}</div>}
          <div><label className="block text-xs text-gray-400 mb-1">Nom</label><input type="text" value={name} onChange={e => { setName(e.target.value); if (!slugManual) setSlug(slugify(e.target.value)); }} autoFocus className="w-full px-3 py-2 text-sm bg-gray-900 border border-gray-700 text-white rounded outline-none" /></div>
          <div><label className="block text-xs text-gray-400 mb-1">Slug</label><input type="text" value={slug} onChange={e => { setSlugManual(true); setSlug(slugify(e.target.value)); }} className="w-full px-3 py-2 text-sm bg-gray-900 border border-gray-700 text-white font-mono rounded outline-none" /></div>
          <div><label className="block text-xs text-gray-400 mb-1">Stack</label><select value={stack} onChange={e => setStack(e.target.value)} className="w-full px-3 py-2 text-sm bg-gray-900 border border-gray-700 text-white rounded outline-none">{STACKS.map(s => <option key={s.value} value={s.value}>{s.label}</option>)}</select></div>
          <div className="flex justify-end gap-2 pt-3 border-t border-gray-700">
            <button type="button" onClick={onClose} className="px-4 py-2 text-sm text-gray-300 bg-gray-700 rounded">Annuler</button>
            <button type="submit" disabled={submitting} className="px-4 py-2 text-sm text-white bg-blue-500 rounded disabled:opacity-50 flex items-center gap-2">{submitting && <Loader2 className="w-4 h-4 animate-spin" />}Creer</button>
          </div>
        </form>
      </div>
    </div>
  );
}

// ══════════════════════════════════════════════════════════════════
// ██ MAIN STUDIO COMPONENT
// ══════════════════════════════════════════════════════════════════

export default function Studio() {
  const [searchParams, setSearchParams] = useSearchParams();
  const [apps, setApps] = useState([]);
  const [selectedSlug, setSelectedSlug] = useState(() => searchParams.get('app') || localStorage.getItem('studio:selectedApp') || '');
  const [activeTab, setActiveTab] = useState(() => searchParams.get('tab') || localStorage.getItem('studio:activeTab') || 'code');
  const [loading, setLoading] = useState(true);
  const [showCreate, setShowCreate] = useState(false);
  const [busy, setBusy] = useState(false);

  // Current app detail
  const [app, setApp] = useState(null);
  const [status, setStatus] = useState(null);

  // Code-server lazy-load: keep opened iframes alive
  const [openedCode, setOpenedCode] = useState(() => {
    const init = new Set();
    const s = searchParams.get('app') || localStorage.getItem('studio:selectedApp');
    if (s && (searchParams.get('tab') || localStorage.getItem('studio:activeTab') || 'code') === 'code') init.add(s);
    return init;
  });

  // ── Fetch apps list ──
  const fetchApps = useCallback(async () => {
    try {
      const res = await listApps();
      const d = res.data?.data || res.data;
      const list = d?.apps || (Array.isArray(d) ? d : []);
      setApps(Array.isArray(list) ? list : []);
      if (list.length > 0 && !selectedSlug) setSelectedSlug(list[0].slug);
    } catch {}
    finally { setLoading(false); }
  }, []);

  useEffect(() => { fetchApps(); }, [fetchApps]);

  // ── Fetch selected app detail ──
  useEffect(() => {
    if (!selectedSlug) { setApp(null); setStatus(null); return; }
    getApp(selectedSlug).then(r => setApp(r.data?.data || r.data)).catch(() => {});
    getAppStatus(selectedSlug).then(r => setStatus(r.data?.data || r.data)).catch(() => {});
  }, [selectedSlug]);

  // ── Persist selection ──
  useEffect(() => { if (selectedSlug) localStorage.setItem('studio:selectedApp', selectedSlug); }, [selectedSlug]);
  useEffect(() => { localStorage.setItem('studio:activeTab', activeTab); }, [activeTab]);

  // ── Real-time via WS ──
  useWebSocket({
    'app:state': (data) => {
      setApps(prev => prev.map(a => a.slug === data.slug ? { ...a, state: data.state, port: data.port || a.port } : a));
      if (data.slug === selectedSlug) {
        setStatus(prev => ({ ...prev, ...data }));
        setApp(prev => prev ? { ...prev, state: data.state } : prev);
      }
    },
  });

  // ── Handlers ──
  function handleSelectApp(slug) {
    setSelectedSlug(slug);
    setSearchParams({ app: slug, tab: activeTab });
    if (activeTab === 'code') {
      setOpenedCode(prev => { if (prev.has(slug)) return prev; const n = new Set(prev); n.add(slug); return n; });
    }
  }

  function handleSelectTab(tab) {
    setActiveTab(tab);
    setSearchParams({ app: selectedSlug, tab });
    if (tab === 'code' && selectedSlug) {
      setOpenedCode(prev => { if (prev.has(selectedSlug)) return prev; const n = new Set(prev); n.add(selectedSlug); return n; });
    }
  }

  const handleControl = useCallback(async (slugOrAction, actionOpt) => {
    const slug = actionOpt ? slugOrAction : selectedSlug;
    const action = actionOpt || slugOrAction;
    setBusy(true);
    try { await controlApp(slug, action); } catch {}
    finally { setBusy(false); }
  }, [selectedSlug]);

  async function handleUpdate(data) {
    if (!selectedSlug) return;
    await updateApp(selectedSlug, data);
    const res = await getApp(selectedSlug);
    setApp(res.data?.data || res.data);
    fetchApps();
  }

  async function handleDelete() {
    if (!selectedSlug || !confirm(`Supprimer "${selectedSlug}" ?`)) return;
    await deleteApp(selectedSlug);
    setSelectedSlug('');
    setApp(null);
    fetchApps();
  }

  const currentApp = app || apps.find(a => a.slug === selectedSlug);
  const visibleTabs = TABS.filter(t => {
    if (t.requiresDb && !currentApp?.has_db) return false;
    return true;
  });

  // Publish studio state to global context so Layout's top bar can render it
  const { setStudio } = useStudio();
  useEffect(() => {
    setStudio({ currentApp, status, selectedSlug, activeTab, busy, onControl: handleControl });
  }, [currentApp, status, selectedSlug, activeTab, busy, handleControl, setStudio]);

  if (loading) return <div className="flex items-center justify-center h-full"><Loader2 className="w-8 h-8 animate-spin text-blue-400" /></div>;

  return (
    <div className="flex h-full overflow-hidden">
      <AppSidebar
        apps={apps}
        selectedSlug={selectedSlug}
        onSelect={handleSelectApp}
        onAdd={() => setShowCreate(true)}
        busy={busy}
        onControl={handleControl}
      />

      <div className="flex flex-col flex-1 min-w-0 h-full">
        {/* Tabs */}
        <div className="flex items-center h-[38px] shrink-0 bg-gray-800/50 border-b border-gray-700 pl-4">
          {visibleTabs.map(tab => {
            const active = tab.id === activeTab;
            const Icon = tab.icon;
            return (
              <button key={tab.id} onClick={() => handleSelectTab(tab.id)}
                className={`relative h-full px-4 border-none cursor-pointer text-[13px] bg-transparent transition-colors flex items-center gap-1.5 ${active ? 'text-white font-medium' : 'text-gray-400 hover:text-gray-200'}`}>
                <Icon className="w-3.5 h-3.5" />
                {tab.label}
                {active && <span className="absolute bottom-0 left-3 right-3 h-0.5 rounded-full bg-blue-400" />}
              </button>
            );
          })}
        </div>

        {/* Content */}
        <div className="flex-1 overflow-hidden relative">
          {!selectedSlug ? (
            <div className="flex flex-col items-center justify-center h-full text-gray-500">
              <Code2 className="w-12 h-12 mb-3 opacity-20" />
              <p className="text-sm">Selectionnez un projet pour commencer</p>
              <button onClick={() => setShowCreate(true)} className="mt-4 px-4 py-2 text-sm bg-blue-500 hover:bg-blue-600 text-white rounded flex items-center gap-2"><Plus className="w-4 h-4" /> Nouvelle app</button>
            </div>
          ) : (
            <>
              {/* Code iframes: lazy-loaded, kept alive when hidden */}
              {[...openedCode].map(slug => {
                const visible = activeTab === 'code' && selectedSlug === slug;
                return (
                  <div key={slug} className="absolute inset-0" style={visible ? { visibility: 'visible', zIndex: 1 } : { visibility: 'hidden', zIndex: 0, pointerEvents: 'none' }}>
                    <CodeTab slug={slug} />
                  </div>
                );
              })}
              {/* Other tabs */}
              {activeTab !== 'code' && (
                <div className="h-full">
                  {activeTab === 'db' && currentApp?.has_db && <DbExplorer appSlug={selectedSlug} embedded />}
                  {activeTab === 'logs' && <LogsTab slug={selectedSlug} />}
                  {activeTab === 'docs' && <DocsTab slug={selectedSlug} />}
                  {activeTab === 'env' && <EnvTab slug={selectedSlug} />}
                  {activeTab === 'settings' && <SettingsTab app={currentApp} onUpdate={handleUpdate} onDelete={handleDelete} />}
                </div>
              )}
            </>
          )}
        </div>
      </div>

      {selectedSlug && <TodosPanel slug={selectedSlug} />}

      {showCreate && <CreateAppModal onClose={() => setShowCreate(false)} onCreated={() => { setShowCreate(false); fetchApps(); }} />}
    </div>
  );
}
