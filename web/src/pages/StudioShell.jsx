import { useState, useEffect, useCallback, useRef } from 'react';
import useWebSocket from '../hooks/useWebSocket';
import DbExplorer from './DbExplorer';
import PreviewTab from '../components/PreviewTab';
import SplitDivider from '../components/SplitDivider';
import DocsTab from '../components/docs/DocsTab';
import SurveillanceTab from '../components/SurveillanceTab';
import AgentWorkspace from '../components/AgentWorkspace';
import EnvTab from '../components/EnvTab';
import BuildBadge from '../components/BuildBadge';
import ThemeToggle from '../components/ThemeToggle';
import NotificationsToggle from '../components/NotificationsToggle';
import {
  Code2, BookOpen, Database, ScrollText, Settings as SettingsIcon,
  ExternalLink, Save, Loader2, Play, Square, Trash2, RefreshCw,
  ShieldAlert, Monitor, Columns2, KeyRound, ArrowLeft,
} from 'lucide-react';
import {
  controlApp, deleteApp, updateApp,
  getApp, getAppStatus, getAppLogs, getLogs,
  getStudioTab, setStudioTab,
} from '../api/client';
import { statusDot } from '../lib/appsUi';
import { pushRecentSlug } from '../lib/recentApps';
import { readStudioTabCache, writeStudioTabCache } from '../lib/openStudio';
import { useIsNarrow } from '../hooks/useMediaQuery';

// Onglet top-niveau : SOURCE DE VÉRITÉ = backend (`agent_open_tabs.studio_tab`).
// Le deep-link homepage→Studio passe par un PUT + broadcast WS `studio:tab` : un
// onglet déjà ouvert (connexion WS établie) bascule en direct, sans URL ni astuce
// cross-tab. localStorage n'est qu'un cache de rendu (graine anti-flash).

// Fallback : `?tab=`/`?kind=` en query. `openStudio` n'en met PLUS, mais on les
// LIT encore pour le service worker (clic notif agent → `/studio/<slug>?tab=code`)
// et d'anciens favoris / liens directs.
function urlParam(name) {
  try { return new URLSearchParams(window.location.search).get(name); } catch { return null; }
}

const TABS = [
  { id: 'code', label: 'Code', icon: Code2 },
  { id: 'preview', label: 'Preview', icon: Monitor },
  { id: 'db', label: 'DB', icon: Database, requiresDb: true },
  { id: 'logs', label: 'Logs', icon: ScrollText },
  { id: 'docs', label: 'Docs', icon: BookOpen },
  { id: 'env', label: 'Variables', icon: KeyRound },
  { id: 'surveillance', label: 'Surveillance', icon: ShieldAlert },
  { id: 'settings', label: 'Settings', icon: SettingsIcon },
];

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
    // Conteneur scrollable pleine largeur → la barre de défilement reste au bord
    // droit ; la colonne max-w-xl à l'intérieur garde le formulaire compact.
    <div className="p-6 overflow-y-auto h-full">
      <div className="space-y-4 max-w-xl">
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
      <p className="text-xs text-gray-500">Les variables d'environnement sont désormais dans l'onglet <span className="text-gray-300">Variables</span>.</p>
      <div className="pt-6 border-t border-gray-700">
        <button onClick={onDelete} className="px-4 py-2 text-sm bg-red-600 hover:bg-red-700 text-white rounded-sm flex items-center gap-1.5"><Trash2 className="w-4 h-4" /> Supprimer l'application</button>
      </div>
      </div>
    </div>
  );
}

// ══════════════════════════════════════════════════════════════════
// ██ STUDIO SHELL — éditeur focalisé sur UNE app (slug dans l'URL)
// ══════════════════════════════════════════════════════════════════

export default function StudioShell({ slug }) {
  // Onglet actif : graine = query de fallback (SW/favori), sinon cache localStorage
  // par app (graine instantanée anti-flash), sinon 'code'. Le backend (autoritaire)
  // est lu/appliqué dans un effet ci-dessous + à chaud via WS `studio:tab`.
  const [activeTab, setActiveTabState] = useState(
    () => urlParam('tab') || readStudioTabCache(slug)?.tab || 'code',
  );
  // Kind cible (onglet Surveillance) ; idem, confirmé par le backend / WS.
  const [pendingKind, setPendingKind] = useState(() => urlParam('kind') || readStudioTabCache(slug)?.kind || null);

  const [app, setApp] = useState(null);
  const [status, setStatus] = useState(null);
  const [loaded, setLoaded] = useState(false);
  const [notFound, setNotFound] = useState(false);
  const [busy, setBusy] = useState(false);
  const [buildState, setBuildState] = useState(null);

  // ── Disposition (mode 'tabs' classique vs 'split' agent+onglets) ──
  const contentRef = useRef(null);
  const [layoutMode, setLayoutMode] = useState(() => localStorage.getItem('studio:layoutMode') || 'tabs');
  // En mode split, l'onglet VISIBLE (panneau droit) est `rightTab`, PAS `activeTab`.
  // Un deep-link doit donc le viser aussi → on graine depuis le cache d'onglet
  // (anti-flash), sinon le dernier onglet droit mémorisé, sinon 'preview'.
  const [rightTab, setRightTab] = useState(() => {
    const t = urlParam('tab') || readStudioTabCache(slug)?.tab;
    return t && t !== 'code' ? t : (localStorage.getItem('studio:rightTab') || 'preview');
  });
  const [leftPct, setLeftPct] = useState(() => {
    const v = parseFloat(localStorage.getItem('studio:splitRatio'));
    return Number.isFinite(v) && v >= 20 && v <= 80 ? v : 50;
  });
  const [dragging, setDragging] = useState(false);
  const isNarrow = useIsNarrow(); // < lg : désactive le split (hook matchMedia partagé)

  // Lancement d'une conversation agent depuis l'onglet Surveillance (« Résoudre »).
  const [agentLaunch, setAgentLaunch] = useState(null);
  const nonceRef = useRef(0);

  // Ouverture par URL directe (pas seulement depuis la galerie homepage) → on
  // enregistre quand même l'app dans les récentes (visible par l'onglet homepage).
  useEffect(() => { pushRecentSlug(slug); }, [slug]);

  // Titre de l'onglet navigateur = nom de l'app ouverte (slug avant le 1er fetch,
  // affiné dès que le détail est chargé) → onglets Studio distinguables.
  useEffect(() => {
    document.title = `${app?.name || slug} · Studio`;
  }, [app?.name, slug]);

  // Changement d'onglet INITIÉ ICI (clic utilisateur, normalisation) → persiste au
  // backend (source de vérité, broadcast `studio:tab`) + cache local de rendu.
  const setActiveTab = useCallback((tab) => {
    setActiveTabState(tab);
    writeStudioTabCache(slug, tab, null);
    setStudioTab(slug, { tab, kind: null }).catch(() => { /* offline → cache local seul */ });
  }, [slug]);

  // Application d'un onglet venu d'AILLEURS (fetch backend au montage, ou broadcast
  // `studio:tab` = deep-link / autre PC) → met à jour l'état + le cache SANS re-PUT
  // (sinon boucle d'écho). On vise les DEUX états d'affichage pour être agnostique
  // au mode : `activeTab` (mode 'tabs') ET `rightTab` (panneau droit du mode
  // 'split', où les onglets non-'code' vivent). `kind` ne réinitialise jamais le
  // kind manuel de SurveillanceTab (son effet ignore un kind non valide / null).
  const applyRemoteTab = useCallback((tab, kind) => {
    if (!tab) return;
    setActiveTabState(tab);
    if (tab !== 'code') setRightTab(tab);
    setPendingKind(kind || null);
    writeStudioTabCache(slug, tab, kind || null);
  }, [slug]);

  // ── Onglet top-niveau : le backend est autoritaire ──
  // Au montage (onglet neuf / rechargement / autre PC) on lit l'état persisté et
  // on l'applique — sauf si l'URL force un onglet (SW notif / favori), qui gagne
  // pour ce chargement. Le cas « onglet déjà ouvert » est couvert par le WS
  // `studio:tab` (cf. useWebSocket plus bas) : bascule live sans rechargement.
  useEffect(() => {
    if (urlParam('tab')) return;
    let cancelled = false;
    getStudioTab(slug)
      .then((r) => { const d = r.data || {}; if (!cancelled && d.tab) applyRemoteTab(d.tab, d.kind); })
      .catch(() => { /* backend down → cache local seul */ });
    return () => { cancelled = true; };
  }, [slug, applyRemoteTab]);

  // ── Fetch app detail + status ──
  useEffect(() => {
    let cancelled = false;
    setLoaded(false); setNotFound(false);
    Promise.all([
      getApp(slug).then(r => r.data?.data || r.data).catch((e) => { if (e.response?.status === 404) throw e; return null; }),
      getAppStatus(slug).then(r => r.data?.data || r.data).catch(() => null),
    ])
      .then(([a, s]) => { if (cancelled) return; setApp(a); setStatus(s); setLoaded(true); })
      .catch(() => { if (!cancelled) { setNotFound(true); setLoaded(true); } });
    return () => { cancelled = true; };
  }, [slug]);

  // ── Persist disposition ──
  useEffect(() => { localStorage.setItem('studio:layoutMode', layoutMode); }, [layoutMode]);
  useEffect(() => { localStorage.setItem('studio:rightTab', rightTab); }, [rightTab]);
  useEffect(() => { localStorage.setItem('studio:splitRatio', String(leftPct)); }, [leftPct]);

  // ── L'onglet droit doit rester disponible (ex. app sans DB) → sinon preview ──
  useEffect(() => {
    const available = TABS.filter(t => t.id !== 'code' && (!t.requiresDb || app?.has_db)).map(t => t.id);
    if (!available.includes(rightTab)) setRightTab('preview');
  }, [app?.has_db, rightTab]);

  // Un tab retiré du code peut rester en localStorage/URL → normalise vers 'code'.
  useEffect(() => {
    if (!TABS.some(t => t.id === activeTab)) setActiveTab('code');
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [activeTab]);

  // ── Real-time via WS (statut + build + onglet, filtrés sur notre slug) ──
  useWebSocket({
    'app:state': (data) => {
      if (data.slug !== slug) return;
      setStatus(prev => ({ ...prev, ...data }));
      setApp(prev => prev ? { ...prev, state: data.state } : prev);
    },
    // Deep-link homepage→Studio (ou changement d'onglet d'un autre PC) : un onglet
    // DÉJÀ ouvert reçoit le broadcast et bascule en direct (aucun rechargement).
    'studio:tab': (data) => {
      if (data?.slug !== slug) return;
      applyRemoteTab(data.tab, data.kind);
    },
    'app:build': (data) => {
      if (data?.slug !== slug) return;
      setBuildState(data);
      if (data.status === 'finished') setTimeout(() => setBuildState(null), 2500);
      else if (data.status === 'error') setTimeout(() => setBuildState(null), 8000);
    },
  });

  // ── Handlers ──
  const handleControl = useCallback(async (action) => {
    setBusy(true);
    try { await controlApp(slug, action); } catch {}
    finally { setBusy(false); }
  }, [slug]);

  function handleSelectTab(tab) { setActiveTab(tab); }
  function handleSetLayoutMode(mode) { if (mode === 'split' && isNarrow) return; setLayoutMode(mode); }
  // En mode split, le panneau droit EST l'onglet visible → persiste comme tel
  // (backend + cache) pour que le state d'onglet par app reflète bien la vue.
  function handleSelectRightTab(tab) {
    setRightTab(tab);
    writeStudioTabCache(slug, tab, null);
    setStudioTab(slug, { tab, kind: null }).catch(() => { /* offline → cache local seul */ });
  }

  // Lance une conversation agent pré-remplie (bouton « Résoudre » surveillance).
  function openAgentWithPrompt(arg) {
    const prompt = typeof arg === 'string' ? arg : arg?.prompt;
    if (!prompt) return;
    const findingId = typeof arg === 'string' ? undefined : arg?.findingId;
    const effort = typeof arg === 'string' ? undefined : arg?.effort;
    setAgentLaunch({ prompt, findingId, effort, mode: 'plan', nonce: ++nonceRef.current });
    if (effectiveMode === 'tabs' && activeTab !== 'code') handleSelectTab('code');
  }

  async function handleUpdate(data) {
    await updateApp(slug, data);
    const res = await getApp(slug);
    setApp(res.data?.data || res.data);
  }

  async function handleDelete() {
    if (!confirm(`Supprimer "${slug}" ?`)) return;
    await deleteApp(slug);
    window.location.replace('/'); // plus rien à éditer → retour homepage
  }

  const currentApp = app;
  const visibleTabs = TABS.filter(t => !(t.requiresDb && !currentApp?.has_db));
  const rightTabs = visibleTabs.filter(t => t.id !== 'code'); // en split, le code est à gauche
  const effectiveMode = (layoutMode === 'split' && !isNarrow) ? 'split' : 'tabs';

  function renderTabContent(tab) {
    switch (tab) {
      case 'preview':      return <PreviewTab key={slug} slug={slug} status={status} onControl={handleControl} />;
      case 'db':           return currentApp?.has_db ? <DbExplorer appSlug={slug} embedded /> : null;
      case 'logs':         return <LogsTab slug={slug} />;
      case 'docs':         return <DocsTab slug={slug} />;
      case 'env':          return <EnvTab slug={slug} onRestart={() => handleControl('restart')} />;
      case 'surveillance': return <SurveillanceTab slug={slug} initialKind={pendingKind} onResolve={openAgentWithPrompt} />;
      case 'settings':     return <SettingsTab app={currentApp} onUpdate={handleUpdate} onDelete={handleDelete} />;
      default:             return null;
    }
  }

  const renderModeSwitcher = () => (
    <div className="ml-auto hidden md:flex items-center gap-1 pr-3">
      {[
        { id: 'tabs', Icon: Square, title: 'Onglets' },
        { id: 'split', Icon: Columns2, title: 'Split — agent + onglets' },
      ].map(({ id, Icon, title }) => (
        <button key={id} onClick={() => handleSetLayoutMode(id)}
          disabled={id === 'split' && isNarrow} title={title}
          className={`p-2 rounded-sm transition-colors ${layoutMode === id ? 'bg-gray-700 text-blue-400' : 'text-gray-500 hover:text-gray-300 hover:bg-gray-800'} disabled:opacity-30 disabled:cursor-not-allowed`}>
          <Icon className="w-5 h-5" />
        </button>
      ))}
    </div>
  );

  // ── Barre supérieure propre du Studio (statut + contrôles + retour Atelier) ──
  const state = (status?.state || currentApp?.state || 'stopped').toLowerCase();
  const isRunning = state === 'running';
  const appPath = `/apps/${slug}/`;
  const uptime = status?.uptime_secs != null
    ? `${Math.floor(status.uptime_secs / 60)}m ${status.uptime_secs % 60}s`
    : '-';

  const topBar = (
    <div className="flex items-center justify-between gap-3 px-3 py-2 bg-gray-800 border-b border-gray-700 shrink-0">
      <div className="flex items-center gap-3 min-w-0">
        <a href="/" className="flex items-center gap-1.5 text-[12px] text-gray-400 hover:text-gray-50 shrink-0" title="Retour à Atelier">
          <ArrowLeft className="w-4 h-4" /> <span className="hidden sm:inline">Atelier</span>
        </a>
        <span className="w-px h-5 bg-gray-700 shrink-0" />
        <div className="flex items-center gap-2 min-w-0">
          <Code2 className="w-4 h-4 text-blue-400 shrink-0" />
          <span className={`w-2 h-2 rounded-full shrink-0 ${statusDot(state)}`} />
          <BuildBadge build={buildState} onDismiss={() => setBuildState(null)} />
          <span className="text-[13px] font-medium text-gray-50 truncate max-w-[160px]">{currentApp?.name || slug}</span>
          {currentApp?.stack && <span className="px-1.5 py-0.5 rounded-sm text-[10px] bg-gray-700 text-gray-400 shrink-0">{currentApp.stack}</span>}
        </div>
        <a
          href={appPath}
          target="_blank"
          rel="noopener noreferrer"
          className="hidden md:flex items-center gap-1 text-[11px] text-blue-400 hover:text-blue-300 truncate max-w-[200px]"
          title={`Ouvrir ${slug} (${appPath})`}
        >
          <span className="truncate">{appPath}</span>
          <ExternalLink className="w-3 h-3 shrink-0" />
        </a>
        <div className="hidden lg:flex items-center gap-3 text-[11px] text-gray-400 shrink-0">
          <span>PID <span className="text-gray-200 font-mono">{status?.pid || '-'}</span></span>
          <span>Port <span className="text-gray-200 font-mono">{currentApp?.port || '-'}</span></span>
          <span>Up <span className="text-gray-200 font-mono">{uptime}</span></span>
        </div>
        <div className="flex items-center gap-1 shrink-0">
          {!isRunning ? (
            <button onClick={() => handleControl('start')} disabled={busy} className="p-2 sm:p-1 text-green-400 hover:bg-gray-700 rounded-sm disabled:opacity-50" title="Démarrer">
              <Play className="w-3.5 h-3.5" />
            </button>
          ) : (
            <button onClick={() => handleControl('stop')} disabled={busy} className="p-2 sm:p-1 text-yellow-400 hover:bg-gray-700 rounded-sm disabled:opacity-50" title="Arrêter">
              <Square className="w-3.5 h-3.5" />
            </button>
          )}
          <button onClick={() => handleControl('restart')} disabled={busy} className="p-2 sm:p-1 text-blue-400 hover:bg-gray-700 rounded-sm disabled:opacity-50" title="Redémarrer">
            <RefreshCw className="w-3.5 h-3.5" />
          </button>
        </div>
      </div>
      <div className="flex items-center gap-1 shrink-0">
        <NotificationsToggle compact />
        <ThemeToggle />
      </div>
    </div>
  );

  if (!loaded) {
    return (
      <div className="flex flex-col h-screen">
        {topBar}
        <div className="flex-1 flex items-center justify-center"><Loader2 className="w-8 h-8 animate-spin text-blue-400" /></div>
      </div>
    );
  }

  if (notFound) {
    return (
      <div className="flex flex-col h-screen">
        {topBar}
        <div className="flex-1 flex flex-col items-center justify-center gap-3 text-gray-400">
          <p className="text-sm">Application <span className="font-mono text-gray-200">{slug}</span> introuvable.</p>
          <a href="/" className="text-sm text-blue-400 hover:text-blue-300 inline-flex items-center gap-1.5"><ArrowLeft className="w-4 h-4" /> Retour à Atelier</a>
        </div>
      </div>
    );
  }

  return (
    <div className="flex flex-col h-screen overflow-hidden">
      {topBar}
      <div className="flex flex-1 min-h-0 overflow-hidden">
        <div className="flex flex-col flex-1 min-w-0 h-full">
          {/* Barre d'onglets (haut) — uniquement en mode 'tabs' */}
          {effectiveMode === 'tabs' && (
            <div className="flex items-center h-[44px] shrink-0 bg-gray-800/50 border-b border-gray-700 pl-4 overflow-x-auto">
              {visibleTabs.map(tab => {
                const active = tab.id === activeTab;
                const Icon = tab.icon;
                return (
                  <button key={tab.id} onClick={() => handleSelectTab(tab.id)}
                    className={`relative h-full px-3 sm:px-5 shrink-0 border-none cursor-pointer text-[14px] bg-transparent transition-[background-color,color] duration-300 ease-out hover:duration-0 flex items-center gap-2 ${active ? 'text-gray-50 font-medium' : 'text-gray-400 hover:bg-gray-700/30 hover:text-gray-200'}`}>
                    <Icon className="w-4 h-4" />
                    {tab.label}
                    {active && <span className="absolute bottom-0 left-3 right-3 h-0.5 rounded-full bg-blue-400" />}
                  </button>
                );
              })}
              {renderModeSwitcher()}
            </div>
          )}

          {/* Zone de contenu — l'agent vit DANS le "code slot" (cf. (A)) */}
          <div className="flex-1 min-w-0 overflow-hidden relative" ref={contentRef}>
            {/* (A) AgentWorkspace dans le "code slot" : plein écran (onglet Code en
                 mode tabs) ou pane gauche (split). */}
            {((effectiveMode === 'tabs' && activeTab === 'code') || effectiveMode === 'split') && (
              <div
                style={effectiveMode === 'split'
                  ? { position: 'absolute', top: 0, bottom: 0, left: 0, width: `${leftPct}%`, zIndex: 1 }
                  : { position: 'absolute', inset: 0, zIndex: 1 }}>
                <AgentWorkspace key={slug} slug={slug} launch={agentLaunch} onLaunchConsumed={() => setAgentLaunch(null)} />
              </div>
            )}

            {/* (C) Mode 'tabs' : onglet non-code plein écran */}
            {effectiveMode === 'tabs' && activeTab !== 'code' && (
              <div className="h-full">{renderTabContent(activeTab)}</div>
            )}

            {/* (D) Mode 'split' : panneau droit (divider + barre d'onglets + contenu) */}
            {effectiveMode === 'split' && (
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
      </div>
    </div>
  );
}
