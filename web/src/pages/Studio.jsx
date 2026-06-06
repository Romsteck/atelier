import { useState, useEffect, useCallback, useRef, useMemo } from 'react';
import { useSearchParams, useLocation } from 'react-router-dom';
import useWebSocket from '../hooks/useWebSocket';
import { useStudio } from '../context/StudioContext';
import DbExplorer from './DbExplorer';
import StudioIframe from '../components/StudioIframe';
import PreviewTab from '../components/PreviewTab';
import SplitDivider from '../components/SplitDivider';
import DocsTab from '../components/docs/DocsTab';
import SurveillanceTab from '../components/SurveillanceTab';
import AgentWorkspace from '../components/AgentWorkspace';
import {
  Code2, BookOpen, Database, ScrollText, KeyRound, Settings as SettingsIcon,
  ExternalLink, Save, Loader2, Plus, Play, Square, Trash2, X,
  Eye, EyeOff, ShieldAlert, Monitor, Columns2, Bot,
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
  { id: 'preview', label: 'Preview', icon: Monitor },
  { id: 'db', label: 'DB', icon: Database, requiresDb: true },
  { id: 'logs', label: 'Logs', icon: ScrollText },
  { id: 'docs', label: 'Docs', icon: BookOpen },
  { id: 'surveillance', label: 'Surveillance', icon: ShieldAlert },
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
            className={`px-2 py-1 rounded-sm ${source === 'atelier' ? 'bg-amber-600 text-white' : 'bg-gray-800 text-gray-400 hover:bg-gray-700'}`}
            title="Logs structurés Postgres (atelier-logging-shipper)"
          >
            Atelier
          </button>
          <button
            onClick={() => setSource('journalctl')}
            className={`px-2 py-1 rounded-sm ${source === 'journalctl' ? 'bg-amber-600 text-white' : 'bg-gray-800 text-gray-400 hover:bg-gray-700'}`}
            title="Logs systemd bruts (journalctl)"
          >
            journalctl
          </button>
        </div>
        <input type="text" value={filter} onChange={e => setFilter(e.target.value)} placeholder="Filtrer..."
          className="flex-1 max-w-[300px] px-3 py-1 rounded-sm text-sm outline-hidden bg-gray-900 text-gray-50 border border-gray-700" />
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
            <div key={i} className="flex gap-3 py-0.5 hover:bg-gray-400/10">
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
        <h3 className="text-sm font-medium text-gray-50">Variables d'environnement</h3>
        <button onClick={() => setShowValues(!showValues)} className="flex items-center gap-1 text-xs text-gray-400 hover:text-gray-50">
          {showValues ? <EyeOff className="w-3.5 h-3.5" /> : <Eye className="w-3.5 h-3.5" />}
          {showValues ? 'Masquer' : 'Afficher'}
        </button>
      </div>
      <textarea
        value={showValues ? envText : envText.split('\n').map(l => { const [k] = l.split('='); return k ? `${k}=***` : l; }).join('\n')}
        onChange={e => { if (showValues) setEnvText(e.target.value); }}
        readOnly={!showValues}
        className="w-full h-64 p-3 rounded-sm text-sm font-mono bg-gray-900 text-gray-50 border border-gray-700 outline-hidden resize-y"
        placeholder="KEY=value"
      />
      <button onClick={handleSave} disabled={saving} className="px-4 py-2 text-sm bg-blue-500 hover:bg-blue-600 text-white rounded-sm disabled:opacity-50 flex items-center gap-1.5">
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
          <input type="text" value={value} onChange={e => set(e.target.value)} className={`w-full px-3 py-2 text-sm bg-gray-900 border border-gray-700 text-gray-50 rounded-sm outline-hidden focus:border-blue-500 ${mono ? 'font-mono' : ''}`} />
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
      <button onClick={handleSave} disabled={saving} className="px-4 py-2 text-sm bg-blue-500 hover:bg-blue-600 text-white rounded-sm disabled:opacity-50 flex items-center gap-1.5">
        {saving ? <Loader2 className="w-4 h-4 animate-spin" /> : <Save className="w-4 h-4" />} Sauvegarder
      </button>
      <div className="pt-6 border-t border-gray-700">
        <button onClick={onDelete} className="px-4 py-2 text-sm bg-red-600 hover:bg-red-700 text-white rounded-sm flex items-center gap-1.5"><Trash2 className="w-4 h-4" /> Supprimer l'application</button>
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
  const visibility = 'private';
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
          <h2 className="text-lg font-semibold text-gray-50">Nouvelle application</h2>
          <button onClick={onClose} className="text-gray-400 hover:text-gray-50"><X className="w-5 h-5" /></button>
        </div>
        <form onSubmit={handleSubmit} className="p-5 space-y-4">
          {error && <div className="bg-red-500/10 border border-red-500/30 rounded-sm px-3 py-2 text-sm text-red-400">{error}</div>}
          <div><label className="block text-xs text-gray-400 mb-1">Nom</label><input type="text" value={name} onChange={e => { setName(e.target.value); if (!slugManual) setSlug(slugify(e.target.value)); }} autoFocus className="w-full px-3 py-2 text-sm bg-gray-900 border border-gray-700 text-gray-50 rounded-sm outline-hidden" /></div>
          <div><label className="block text-xs text-gray-400 mb-1">Slug</label><input type="text" value={slug} onChange={e => { setSlugManual(true); setSlug(slugify(e.target.value)); }} className="w-full px-3 py-2 text-sm bg-gray-900 border border-gray-700 text-gray-50 font-mono rounded-sm outline-hidden" /></div>
          <div><label className="block text-xs text-gray-400 mb-1">Stack</label><select value={stack} onChange={e => setStack(e.target.value)} className="w-full px-3 py-2 text-sm bg-gray-900 border border-gray-700 text-gray-50 rounded-sm outline-hidden">{STACKS.map(s => <option key={s.value} value={s.value}>{s.label}</option>)}</select></div>
          <div className="flex justify-end gap-2 pt-3 border-t border-gray-700">
            <button type="button" onClick={onClose} className="px-4 py-2 text-sm text-gray-300 bg-gray-700 rounded-sm">Annuler</button>
            <button type="submit" disabled={submitting} className="px-4 py-2 text-sm text-white bg-blue-500 rounded-sm disabled:opacity-50 flex items-center gap-2">{submitting && <Loader2 className="w-4 h-4 animate-spin" />}Creer</button>
          </div>
        </form>
      </div>
    </div>
  );
}

// ── Apps list (default view when no app is selected) ──

const stackLabel = (s) => STACKS.find(st => st.value === s)?.label || s;

function AppsGallery({ apps, onOpen, onAdd, onControl }) {
  return (
    <div className="h-full overflow-y-auto p-5">
      <div className="flex items-center justify-between mb-3">
        <h2 className="text-xs font-semibold uppercase tracking-wider text-gray-500">Applications</h2>
        <button
          onClick={onAdd}
          className="flex items-center gap-1.5 px-3 py-1.5 text-xs font-medium text-white bg-blue-500 hover:bg-blue-600 active:bg-blue-700 rounded-md transition-colors"
        >
          <Plus className="w-3.5 h-3.5" /> Nouvelle application
        </button>
      </div>
      <table className="w-full text-[13px] border-collapse">
        <thead>
          <tr className="text-left text-[11px] uppercase tracking-wider text-gray-500 border-b border-gray-700">
            <th className="w-0 py-2 pl-3 pr-2" />
            <th className="font-medium py-2 px-2">Nom</th>
            <th className="font-medium py-2 px-2">Stack</th>
            <th className="font-medium py-2 px-2 hidden md:table-cell">Lien</th>
            <th className="font-medium py-2 px-2">Port</th>
            <th className="w-0 py-2 pr-3 pl-2" />
          </tr>
        </thead>
        <tbody>
          {apps.map(app => {
            const state = (app.state || '').toLowerCase();
            const isRunning = state === 'running';
            return (
              <tr
                key={app.slug}
                onClick={() => onOpen(app.slug)}
                className="group cursor-pointer border-b border-gray-800 transition-[background-color,color] duration-300 ease-out hover:duration-0 hover:bg-gray-700/30"
              >
                <td className="py-2 pl-3 pr-2">
                  <span className={`block w-[9px] h-[9px] rounded-full ${statusDot(state)}`} title={state || 'unknown'} />
                </td>
                <td className="py-2 px-2 font-medium text-gray-200 group-hover:text-gray-50">
                  <span className="inline-flex items-center gap-1.5">
                    {app.name}
                    {app.has_db && <Database className="w-3 h-3 text-gray-500" title="Base de données" />}
                  </span>
                </td>
                <td className="py-2 px-2 text-gray-400">{stackLabel(app.stack)}</td>
                <td className="py-2 px-2 hidden md:table-cell">
                  <a
                    href={`/apps/${app.slug}/`}
                    target="_blank"
                    rel="noopener noreferrer"
                    onClick={e => e.stopPropagation()}
                    className="inline-flex items-center gap-1 text-blue-400 hover:text-blue-300"
                    title={`Ouvrir /apps/${app.slug}/`}
                  >
                    /apps/{app.slug}/ <ExternalLink className="w-3 h-3" />
                  </a>
                </td>
                <td className="py-2 px-2 text-gray-400 font-mono">{app.port ?? '-'}</td>
                <td className="py-2 pr-3 pl-2 text-right">
                  <span className="inline-flex opacity-0 group-hover:opacity-100 transition-opacity">
                    {isRunning ? (
                      <button onClick={e => { e.stopPropagation(); onControl(app.slug, 'stop'); }} className="p-1 text-yellow-400 hover:bg-gray-600 rounded-sm" title="Stop">
                        <Square className="w-3.5 h-3.5" />
                      </button>
                    ) : (
                      <button onClick={e => { e.stopPropagation(); onControl(app.slug, 'start'); }} className="p-1 text-green-400 hover:bg-gray-600 rounded-sm" title="Start">
                        <Play className="w-3.5 h-3.5" />
                      </button>
                    )}
                  </span>
                </td>
              </tr>
            );
          })}
          {apps.length === 0 && (
            <tr><td colSpan={6} className="py-8 text-center text-gray-600">Aucune application</td></tr>
          )}
        </tbody>
      </table>
    </div>
  );
}

// ══════════════════════════════════════════════════════════════════
// ██ MAIN STUDIO COMPONENT
// ══════════════════════════════════════════════════════════════════

export default function Studio() {
  const [searchParams, setSearchParams] = useSearchParams();
  const location = useLocation();
  const [apps, setApps] = useState([]);
  const [selectedSlug, setSelectedSlug] = useState(() => searchParams.get('app') || '');
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
    const s = searchParams.get('app');
    if (s && (searchParams.get('tab') || 'code') === 'code') init.add(s);
    return init;
  });

  // Recently-opened apps (slugs, most-recent-first) — feeds the nav sub-menu
  const [recentSlugs, setRecentSlugs] = useState(() => {
    try { return JSON.parse(localStorage.getItem('studio:recentApps')) || []; }
    catch { return []; }
  });

  // ── Disposition (mode 'tabs' classique vs 'split' éditeur+onglets) ──
  const contentRef = useRef(null);
  const [layoutMode, setLayoutMode] = useState(() => localStorage.getItem('studio:layoutMode') || 'tabs');
  const [rightTab, setRightTab] = useState(() => localStorage.getItem('studio:rightTab') || 'preview');
  const [leftPct, setLeftPct] = useState(() => {
    const v = parseFloat(localStorage.getItem('studio:splitRatio'));
    return Number.isFinite(v) && v >= 20 && v <= 80 ? v : 50;
  });
  const [dragging, setDragging] = useState(false);
  const [isNarrow, setIsNarrow] = useState(() => typeof window !== 'undefined' && window.innerWidth < 900);
  // Switch éditeur ⇄ agent : l'agent REMPLACE l'iframe code-server dans le même
  // emplacement ("code slot"). En split, l'agent occupe le pane gauche → le
  // preview/browser reste visible à droite. 'editor' | 'agent'.
  const [codeView, setCodeView] = useState(() => localStorage.getItem('studio:codeView') || 'editor');

  // ── Fetch apps list ──
  const fetchApps = useCallback(async () => {
    try {
      const res = await listApps();
      const d = res.data?.data || res.data;
      const list = d?.apps || (Array.isArray(d) ? d : []);
      setApps(Array.isArray(list) ? list : []);
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

  // ── Persist last-used tab ──
  useEffect(() => { localStorage.setItem('studio:activeTab', activeTab); }, [activeTab]);

  // ── Persist disposition ──
  useEffect(() => { localStorage.setItem('studio:layoutMode', layoutMode); }, [layoutMode]);
  useEffect(() => { localStorage.setItem('studio:rightTab', rightTab); }, [rightTab]);
  useEffect(() => { localStorage.setItem('studio:splitRatio', String(leftPct)); }, [leftPct]);
  useEffect(() => { localStorage.setItem('studio:codeView', codeView); }, [codeView]);

  // ── Détection écran étroit (désactive le split) ──
  useEffect(() => {
    const onResize = () => setIsNarrow(window.innerWidth < 900);
    window.addEventListener('resize', onResize);
    return () => window.removeEventListener('resize', onResize);
  }, []);

  // ── L'onglet droit doit rester disponible (ex. app sans DB) → sinon retomber sur preview ──
  useEffect(() => {
    const available = TABS.filter(t => t.id !== 'code' && (!t.requiresDb || app?.has_db)).map(t => t.id);
    if (!available.includes(rightTab)) setRightTab('preview');
  }, [app?.has_db, rightTab]);

  // ── Track recently-opened apps ──
  useEffect(() => {
    if (!selectedSlug) return;
    setRecentSlugs(prev => {
      const next = [selectedSlug, ...prev.filter(s => s !== selectedSlug)].slice(0, 8);
      try { localStorage.setItem('studio:recentApps', JSON.stringify(next)); } catch {}
      return next;
    });
  }, [selectedSlug]);

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

  // ── Sync selection + tab from the URL (nav clicks, deep-links, back/forward) ──
  // On /studio, no ?app= means "show the gallery" → clear the selection.
  useEffect(() => {
    if (location.pathname !== '/studio') return;
    const urlApp = searchParams.get('app') || '';
    const urlTab = searchParams.get('tab');
    if (urlApp !== selectedSlug) setSelectedSlug(urlApp);
    if (urlTab && urlTab !== activeTab) setActiveTab(urlTab);
  }, [searchParams, location.pathname, selectedSlug, activeTab]);

  // ── Keep the code-server iframe mounted for the selected app ──
  useEffect(() => {
    if (activeTab === 'code' && selectedSlug) {
      setOpenedCode(prev => prev.has(selectedSlug) ? prev : new Set(prev).add(selectedSlug));
    }
  }, [activeTab, selectedSlug]);

  // ── Handlers ──
  const handleAddApp = useCallback(() => setShowCreate(true), []);

  function handleOpenApp(slug) {
    setSearchParams({ app: slug, tab: activeTab || 'code' });
  }

  function handleSelectTab(tab) {
    setActiveTab(tab);
    setSearchParams({ app: selectedSlug, tab });
    if (tab === 'code' && selectedSlug) {
      setOpenedCode(prev => { if (prev.has(selectedSlug)) return prev; const n = new Set(prev); n.add(selectedSlug); return n; });
    }
  }

  function handleSetLayoutMode(mode) {
    if (mode === 'split' && isNarrow) return;
    setLayoutMode(mode);
    // entrer en split impose d'avoir l'éditeur monté (il est épinglé à gauche)
    if (mode === 'split' && selectedSlug) {
      setOpenedCode(prev => prev.has(selectedSlug) ? prev : new Set(prev).add(selectedSlug));
    }
  }

  function handleSelectRightTab(tab) { setRightTab(tab); }

  // Bascule éditeur ⇄ agent dans le "code slot". En mode tabs, l'agent vit sur
  // l'onglet Code → on s'y place pour le rendre visible. Le code-server reste
  // monté (caché) pour un retour instantané.
  function toggleAgent() {
    setCodeView(prev => {
      const next = prev === 'agent' ? 'editor' : 'agent';
      if (next === 'agent') {
        if (effectiveMode === 'tabs' && activeTab !== 'code') handleSelectTab('code');
        if (selectedSlug) setOpenedCode(p => p.has(selectedSlug) ? p : new Set(p).add(selectedSlug));
      }
      return next;
    });
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
    setSearchParams({});
    setApp(null);
    fetchApps();
  }

  const currentApp = app || apps.find(a => a.slug === selectedSlug);
  const visibleTabs = TABS.filter(t => {
    if (t.requiresDb && !currentApp?.has_db) return false;
    return true;
  });
  const rightTabs = visibleTabs.filter(t => t.id !== 'code'); // en split, le code est à gauche
  const effectiveMode = (layoutMode === 'split' && !isNarrow) ? 'split' : 'tabs';

  // Contenu d'un onglet — helper unique partagé par le mode 'tabs' et le panneau droit du 'split'
  // (une seule des deux branches est montée à la fois → pas de double instance des composants lourds).
  function renderTabContent(tab) {
    switch (tab) {
      case 'preview':      return <PreviewTab key={selectedSlug} slug={selectedSlug} status={status} onControl={handleControl} />;
      case 'db':           return currentApp?.has_db ? <DbExplorer appSlug={selectedSlug} embedded /> : null;
      case 'logs':         return <LogsTab slug={selectedSlug} />;
      case 'docs':         return <DocsTab slug={selectedSlug} />;
      case 'surveillance': return <SurveillanceTab slug={selectedSlug} />;
      case 'env':          return <EnvTab slug={selectedSlug} />;
      case 'settings':     return <SettingsTab app={currentApp} onUpdate={handleUpdate} onDelete={handleDelete} />;
      default:             return null;
    }
  }

  const renderModeSwitcher = () => (
    <div className="ml-auto flex items-center gap-1 pr-3">
      {[
        { id: 'tabs', Icon: Square, title: 'Onglets' },
        { id: 'split', Icon: Columns2, title: 'Split — éditeur + onglets' },
      ].map(({ id, Icon, title }) => (
        <button key={id} onClick={() => handleSetLayoutMode(id)}
          disabled={id === 'split' && isNarrow} title={title}
          className={`p-2 rounded-sm transition-colors ${layoutMode === id ? 'bg-gray-700 text-blue-400' : 'text-gray-500 hover:text-gray-300 hover:bg-gray-800'} disabled:opacity-30 disabled:cursor-not-allowed`}>
          <Icon className="w-5 h-5" />
        </button>
      ))}
      <span className="w-px h-5 bg-gray-700 mx-0.5" />
      <button onClick={toggleAgent} title={codeView === 'agent' ? 'Revenir à code-server' : 'Remplacer code-server par l’agent'}
        className={`p-2 rounded-sm transition-colors ${codeView === 'agent' ? 'bg-gray-700 text-blue-400' : 'text-gray-500 hover:text-gray-300 hover:bg-gray-800'}`}>
        <Bot className="w-5 h-5" />
      </button>
    </div>
  );

  // 4 most-recently-opened apps, resolved to live app objects, for the nav sub-menu
  const recentApps = useMemo(
    () => recentSlugs.map(s => apps.find(a => a.slug === s)).filter(Boolean).slice(0, 4),
    [recentSlugs, apps]
  );

  // Publish studio state to global context so Layout's top bar + nav can render it
  const { setStudio } = useStudio();
  useEffect(() => {
    setStudio({ currentApp, status, selectedSlug, activeTab, busy, onControl: handleControl, recentApps, onAddApp: handleAddApp });
  }, [currentApp, status, selectedSlug, activeTab, busy, handleControl, recentApps, handleAddApp, setStudio]);

  if (loading) return <div className="flex items-center justify-center h-full"><Loader2 className="w-8 h-8 animate-spin text-blue-400" /></div>;

  return (
    <div className="flex h-full overflow-hidden">
      <div className="flex flex-col flex-1 min-w-0 h-full">
        {/* Barre d'onglets (haut) — uniquement en mode 'tabs' ; en split les onglets passent à droite */}
        {selectedSlug && effectiveMode === 'tabs' && (
          <div className="flex items-center h-[44px] shrink-0 bg-gray-800/50 border-b border-gray-700 pl-4">
            {visibleTabs.map(tab => {
              const active = tab.id === activeTab;
              const Icon = tab.icon;
              return (
                <button key={tab.id} onClick={() => handleSelectTab(tab.id)}
                  className={`relative h-full px-5 border-none cursor-pointer text-[14px] bg-transparent transition-[background-color,color] duration-300 ease-out hover:duration-0 flex items-center gap-2 ${active ? 'text-gray-50 font-medium' : 'text-gray-400 hover:bg-gray-700/30 hover:text-gray-200'}`}>
                  <Icon className="w-4 h-4" />
                  {tab.label}
                  {active && <span className="absolute bottom-0 left-3 right-3 h-0.5 rounded-full bg-blue-400" />}
                </button>
              );
            })}
            {renderModeSwitcher()}
          </div>
        )}

        {/* Zone de contenu — l'agent vit DANS le "code slot" (cf. (A')), pas en colonne séparée */}
        <div className="flex-1 min-w-0 overflow-hidden relative" ref={contentRef}>
          {/* (A) Pool keep-alive code-server — jamais remonté ; masqué (mais gardé
               monté) quand le slot affiche l'agent (codeView==='agent') → retour instantané. */}
          {[...openedCode].map(slug => {
            const isSel = selectedSlug === slug;
            const slotActive = isSel && (
              (effectiveMode === 'tabs' && activeTab === 'code') || effectiveMode === 'split'
            );
            const codeVisible = slotActive && codeView === 'editor';
            const vis = { visibility: codeVisible ? 'visible' : 'hidden', zIndex: codeVisible ? 1 : 0, pointerEvents: codeVisible ? 'auto' : 'none' };
            const style = effectiveMode === 'split' && isSel
              ? { position: 'absolute', top: 0, bottom: 0, left: 0, width: `${leftPct}%`, ...vis }
              : { position: 'absolute', inset: 0, ...vis };
            return <div key={slug} style={style}><CodeTab slug={slug} /></div>;
          })}

          {/* (A') Agent dans le "code slot" — REMPLACE l'iframe code-server quand le
               switch (Bot) est sur agent. Plein écran (tabs + onglet Code) ou pane
               gauche (split) → le preview/browser reste visible à droite. */}
          {selectedSlug && codeView === 'agent' &&
            ((effectiveMode === 'tabs' && activeTab === 'code') || effectiveMode === 'split') && (
              <div
                style={effectiveMode === 'split'
                  ? { position: 'absolute', top: 0, bottom: 0, left: 0, width: `${leftPct}%`, zIndex: 1 }
                  : { position: 'absolute', inset: 0, zIndex: 1 }}>
                <AgentWorkspace key={selectedSlug} slug={selectedSlug} />
              </div>
            )}

          {/* (B) Gallery (aucune app sélectionnée) */}
          {!selectedSlug && (
            <AppsGallery apps={apps} onOpen={handleOpenApp} onAdd={() => setShowCreate(true)} onControl={handleControl} />
          )}

          {/* (C) Mode 'tabs' : onglet non-code plein écran */}
          {selectedSlug && effectiveMode === 'tabs' && activeTab !== 'code' && (
            <div className="h-full">{renderTabContent(activeTab)}</div>
          )}

          {/* (D) Mode 'split' : panneau droit (divider + barre d'onglets droite + contenu) */}
          {selectedSlug && effectiveMode === 'split' && (
            <div className="absolute top-0 bottom-0 right-0 flex flex-col bg-gray-900" style={{ width: `${100 - leftPct}%`, zIndex: 2 }}>
              <SplitDivider containerRef={contentRef} onResize={setLeftPct} setDragging={setDragging} />
              <div className="flex items-center h-[40px] shrink-0 border-b border-gray-700 bg-gray-800/40 pl-3 overflow-x-auto">
                {rightTabs.map(tab => {
                  const active = tab.id === rightTab;
                  const Icon = tab.icon;
                  return (
                    <button key={tab.id} onClick={() => handleSelectRightTab(tab.id)}
                      className={`relative h-full px-4 border-none cursor-pointer text-[14px] bg-transparent transition-[background-color,color] duration-300 ease-out hover:duration-0 flex items-center gap-2 shrink-0 ${active ? 'text-gray-50 font-medium' : 'text-gray-400 hover:bg-gray-700/30 hover:text-gray-200'}`}>
                      <Icon className="w-4 h-4" />
                      {tab.label}
                      {active && <span className="absolute bottom-0 left-3 right-3 h-0.5 rounded-full bg-blue-400" />}
                    </button>
                  );
                })}
                {renderModeSwitcher()}
              </div>
              <div className="flex-1 overflow-hidden">{renderTabContent(rightTab)}</div>
            </div>
          )}

          {/* (E) Overlay de drag — empêche les iframes d'avaler pointermove */}
          {dragging && <div className="absolute inset-0 z-50 cursor-col-resize" />}
        </div>
      </div>

      {showCreate && <CreateAppModal onClose={() => setShowCreate(false)} onCreated={() => { setShowCreate(false); fetchApps(); }} />}
    </div>
  );
}
