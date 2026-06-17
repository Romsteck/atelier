import { useCallback, useEffect, useState } from "react";
import { useLocation } from "react-router-dom";
import Sidebar from "./Sidebar";
import { Menu, Code2, ExternalLink, Play, Square, RefreshCw, Loader2, Check, AlertCircle, X, Sun, Moon } from "lucide-react";
import { useTheme } from "../context/ThemeContext";
import TaskBell from "./tasks/TaskBell";
import TaskDropdown from "./tasks/TaskDropdown";
import Studio, { statusDot } from "../pages/Studio";
import { useStudio } from "../context/StudioContext";
import { PageHeaderSlotProvider, usePageHeaderSlotRegister } from "../context/PageHeaderSlot";
import useWebSocket from "../hooks/useWebSocket";

function BuildBadge({ build, onDismiss }) {
  if (!build) return null;
  const status = build.status;
  const step = build.step;
  const total = build.total_steps ?? 5;
  const phase = build.phase;

  if (status === 'started' || status === 'step') {
    // Les builds locaux (skill 0-build) n'émettent pas de compteur d'étapes —
    // seuls les builds MCP distants ont step/total. Sans compteur, on affiche
    // juste « Build · {phase} » au lieu d'un trompeur « Build …/5 ».
    const hasCounter = step != null || build.total_steps != null;
    return (
      <div
        role="status"
        aria-live="polite"
        className="flex items-center gap-1.5 px-2 py-0.5 rounded-sm text-[11px] bg-blue-500/15 border border-blue-500/30 text-blue-300 shrink-0"
      >
        <Loader2 className="w-3 h-3 animate-spin" />
        <span>Build{hasCounter ? ` ${step ?? '…'}/${total}` : ''}</span>
        {phase && <span className="opacity-70">· {phase}</span>}
      </div>
    );
  }

  if (status === 'finished') {
    const secs = build.duration_ms != null ? Math.round(build.duration_ms / 1000) : null;
    return (
      <div
        role="status"
        aria-live="polite"
        className="flex items-center gap-1.5 px-2 py-0.5 rounded-sm text-[11px] bg-emerald-500/15 border border-emerald-500/30 text-emerald-300 shrink-0 transition-opacity duration-300"
      >
        <Check className="w-3 h-3" />
        <span>Build OK{secs != null ? ` · ${secs}s` : ''}</span>
      </div>
    );
  }

  if (status === 'error') {
    return (
      <div
        role="status"
        aria-live="polite"
        title={build.error || build.message || 'Build failed'}
        className="flex items-center gap-1.5 px-2 py-0.5 rounded-sm text-[11px] bg-red-500/15 border border-red-500/30 text-red-300 shrink-0"
      >
        <AlertCircle className="w-3 h-3" />
        <span>Build failed</span>
        <button
          onClick={onDismiss}
          aria-label="Dismiss build error"
          className="ml-0.5 p-0.5 rounded-sm hover:bg-red-500/20"
        >
          <X className="w-3 h-3" />
        </button>
      </div>
    );
  }

  return null;
}

function StudioHeaderInfo() {
  const { currentApp, status, busy, onControl } = useStudio();
  const [buildState, setBuildState] = useState(null);

  useWebSocket({
    'app:build': (data) => {
      if (!currentApp || data?.slug !== currentApp.slug) return;
      setBuildState(data);
      if (data.status === 'finished') {
        setTimeout(() => setBuildState(null), 2500);
      }
    },
  });

  useEffect(() => {
    setBuildState(null);
  }, [currentApp?.slug]);

  if (!currentApp) return null;

  const state = (status?.state || currentApp.state || 'stopped').toLowerCase();
  const isRunning = state === 'running';
  // Apps are path-routed via Atelier under /apps/{slug}/. Open links use the
  // relative URL so the link works regardless of which subdomain hits us.
  const appPath = `/apps/${currentApp.slug}/`;
  const uptime = status?.uptime_secs != null
    ? `${Math.floor(status.uptime_secs / 60)}m ${status.uptime_secs % 60}s`
    : '-';

  return (
    <div className="flex items-center gap-3 min-w-0">
      <div className="flex items-center gap-2 shrink-0">
        <Code2 className="w-4 h-4 text-blue-400" />
        <span className={`w-2 h-2 rounded-full ${statusDot(state)}`} />
        <BuildBadge build={buildState} onDismiss={() => setBuildState(null)} />
        <span className="text-[13px] font-medium text-gray-50 truncate max-w-[140px]">{currentApp.name}</span>
        <span className="px-1.5 py-0.5 rounded-sm text-[10px] bg-gray-700 text-gray-400">{currentApp.stack}</span>
      </div>
      <a
        href={appPath}
        target="_blank"
        rel="noopener noreferrer"
        className="hidden md:flex items-center gap-1 text-[11px] text-blue-400 hover:text-blue-300 truncate max-w-[200px]"
        title={`Ouvrir ${currentApp.slug} (${appPath})`}
      >
        <span className="truncate">{appPath}</span>
        <ExternalLink className="w-3 h-3 shrink-0" />
      </a>
      <div className="hidden lg:flex items-center gap-3 text-[11px] text-gray-400 shrink-0">
        <span>PID <span className="text-gray-200 font-mono">{status?.pid || '-'}</span></span>
        <span>Port <span className="text-gray-200 font-mono">{currentApp.port || '-'}</span></span>
        <span>Up <span className="text-gray-200 font-mono">{uptime}</span></span>
      </div>
      {onControl && (
        <div className="flex items-center gap-1 shrink-0">
          {!isRunning ? (
            <button
              onClick={() => onControl('start')}
              disabled={busy}
              className="p-1 text-green-400 hover:bg-gray-700 rounded-sm disabled:opacity-50"
              title="Démarrer"
            >
              <Play className="w-3.5 h-3.5" />
            </button>
          ) : (
            <button
              onClick={() => onControl('stop')}
              disabled={busy}
              className="p-1 text-yellow-400 hover:bg-gray-700 rounded-sm disabled:opacity-50"
              title="Arrêter"
            >
              <Square className="w-3.5 h-3.5" />
            </button>
          )}
          <button
            onClick={() => onControl('restart')}
            disabled={busy}
            className="p-1 text-blue-400 hover:bg-gray-700 rounded-sm disabled:opacity-50"
            title="Redémarrer"
          >
            <RefreshCw className="w-3.5 h-3.5" />
          </button>
        </div>
      )}
    </div>
  );
}

function ThemeToggle() {
  const { theme, toggleTheme } = useTheme();
  const isDark = theme === 'dark';
  return (
    <button
      onClick={toggleTheme}
      className="p-1.5 text-gray-400 hover:text-gray-100 hover:bg-gray-700 rounded-sm transition-colors"
      aria-label={isDark ? 'Passer en thème clair' : 'Passer en thème sombre'}
      title={isDark ? 'Thème clair' : 'Thème sombre'}
    >
      {isDark ? <Sun className="w-5 h-5" /> : <Moon className="w-5 h-5" />}
    </button>
  );
}

function LayoutInner({ children }) {
  const [sidebarOpen, setSidebarOpen] = useState(false);
  const closeSidebar = useCallback(() => setSidebarOpen(false), []);
  const location = useLocation();
  const isStudio = location.pathname === '/studio';
  const { selectedSlug } = useStudio();
  // En Studio avec une app ouverte : le menu de gauche se replie en rail
  // d'icônes (place rendue à l'éditeur/preview), réétendu au survol en overlay.
  const collapsed = isStudio && Boolean(selectedSlug);
  const registerSlot = usePageHeaderSlotRegister();

  return (
    <div className="flex h-screen">
      {sidebarOpen && (
        <div
          className="fixed inset-0 bg-black/60 z-40 lg:hidden"
          onClick={() => setSidebarOpen(false)}
        />
      )}

      <div
        className={`fixed inset-y-0 left-0 z-50 w-64 transform transition-transform duration-200 ease-out lg:relative lg:translate-x-0 lg:transition-[width] ${
          collapsed ? "lg:w-16" : "lg:w-64"
        } ${sidebarOpen ? "translate-x-0" : "-translate-x-full"}`}
      >
        <Sidebar onClose={closeSidebar} collapsed={collapsed} />
      </div>

      <div className="flex-1 flex flex-col min-w-0">
        <div className="flex items-center justify-between gap-3 px-4 py-2 bg-gray-800 border-b border-gray-700">
          <div className="flex items-center gap-3 min-w-0">
            <button
              onClick={() => setSidebarOpen(true)}
              className="lg:hidden p-1 text-gray-400 hover:text-gray-50 shrink-0"
            >
              <Menu className="w-6 h-6" />
            </button>
            <h1 className="lg:hidden text-lg font-bold shrink-0">Atelier</h1>
            {isStudio ? (
              <StudioHeaderInfo />
            ) : (
              <div ref={registerSlot} className="flex-1 flex items-center min-w-0" />
            )}
          </div>
          <div className="flex items-center gap-1 shrink-0">
            <ThemeToggle />
            <div className="relative">
              <TaskBell />
              <TaskDropdown />
            </div>
          </div>
        </div>
        <main className="flex-1 overflow-hidden relative">
          <div
            className={isStudio ? "absolute inset-0" : "hidden"}
            aria-hidden={!isStudio}
          >
            <Studio />
          </div>
          {!isStudio && (
            <div className="h-full overflow-auto">{children}</div>
          )}
        </main>
      </div>
    </div>
  );
}

function Layout({ children }) {
  return (
    <PageHeaderSlotProvider>
      <LayoutInner>{children}</LayoutInner>
    </PageHeaderSlotProvider>
  );
}

export default Layout;
