import { NavLink, useLocation } from 'react-router-dom';
import {
  LogOut, User, LayoutGrid, Database, Hammer,
  GitBranch, X, ExternalLink, TableProperties, ShieldAlert,
  Play, Square, Archive, Loader2, Settings2,
  CheckCircle2, BarChart3,
  ClipboardList, Bot,
} from 'lucide-react';
import { useAuth } from '../context/AuthContext';
import { useApps } from '../context/AppsContext';
import { usePilot } from '../context/PilotContext';
import { statusDot } from '../lib/appsUi';
import { openStudio } from '../lib/openStudio';
import { useEffect } from 'react';
import useBuildingApps from '../hooks/useBuildingApps';
import InstallButton from './InstallButton';
import NotificationsToggle from './NotificationsToggle';

// Navigation groupée par domaine : Applications (landing), Données (explorer),
// Autonomie (Pilote + assistant + signaux des agents), Plateforme (outillage).
const navGroups = [
  {
    label: null,
    items: [
      { to: '/', icon: LayoutGrid, label: 'Applications', highlight: true },
    ],
  },
  {
    label: 'Données',
    items: [
      { to: '/database', icon: Database, label: 'Base de données' },
      { to: '/schema', icon: TableProperties, label: 'Schéma' },
    ],
  },
  {
    label: 'Autonomie',
    items: [
      { to: '/backlog', icon: ClipboardList, label: 'Pilote' },
      // Ouvre le dock du chef de projet (PmAssistantDock, monté par le Layout).
      { action: 'pilot:open-assistant', icon: Bot, label: 'Chef de projet' },
      { to: '/surveillance', icon: ShieldAlert, label: 'Surveillance' },
    ],
  },
  {
    label: 'Plateforme',
    items: [
      { to: '/git', icon: GitBranch, label: 'Git' },
      { to: '/stats', icon: BarChart3, label: 'Statistiques' },
      { to: '/backup', icon: Archive, label: 'Sauvegarde' },
      { to: '/settings', icon: Settings2, label: 'Paramètres' },
    ],
  },
];

const linkClass = ({ isActive }) =>
  `flex items-center gap-3 px-4 py-2 transition-[background-color,color] duration-300 ease-out hover:duration-0 text-sm ${
    isActive
      ? 'border-l-3 border-amber-400 bg-gray-700/50 text-gray-50'
      : 'border-l-3 border-transparent text-gray-300 hover:bg-gray-700/30'
  }`;

function PilotBadge() {
  const { counts } = usePilot();
  if (counts.attention > 0) return <span className="bg-red-500 text-white text-[10px] font-bold rounded-full min-w-[18px] h-[18px] flex items-center justify-center px-1">{counts.attention > 99 ? '99+' : counts.attention}</span>;
  if (counts.running + counts.ready > 0) return <span className="bg-blue-500 text-white text-[10px] font-bold rounded-full min-w-[18px] h-[18px] flex items-center justify-center px-1">{counts.running + counts.ready}</span>;
  return <CheckCircle2 className="w-4 h-4 text-emerald-500" />;
}

function Sidebar({ onClose, collapsed }) {
  const { user, logout } = useAuth();
  const location = useLocation();
  const { recentApps, control } = useApps();
  const buildingApps = useBuildingApps();

  useEffect(() => {
    onClose?.();
  }, [location.pathname, onClose]);

  const onHome = location.pathname === '/';

  // Rail replié (lg) → rétabli au survol de l'aside (group/aside). Gated lg: →
  // mobile + sidebar étendue intacts.
  //  - railRow   : centre l'icône dans le rail au repos, repasse à gauche au survol.
  //  - railLabel : sort le libellé du flux (display:none) → l'icône reste seule et
  //                centrée (pas d'écrasement) ; réapparaît au survol.
  //  - railText  : textes SANS icône (label de groupe, version) → on garde leur
  //                place (visibility) pour préserver le rythme vertical.
  const railRow = collapsed ? 'lg:justify-center lg:group-hover/aside:justify-start' : '';
  const railLabel = collapsed ? 'lg:hidden lg:group-hover/aside:block' : '';
  const railText = collapsed ? 'lg:invisible lg:group-hover/aside:visible' : '';

  // Selecting a recent app opens its Studio dans un nouvel onglet focalisé (le
  // Studio est une app séparée servie sous `/studio/<slug>`).
  function handleSelectApp(slug) {
    openStudio(slug);
    onClose?.();
  }

  return (
    <aside
      className={`group/aside w-64 h-full bg-gray-800 border-r border-gray-700 flex flex-col ${
        collapsed
          ? "lg:absolute lg:inset-y-0 lg:left-0 lg:w-16 lg:hover:w-64 lg:overflow-hidden lg:transition-[width] lg:duration-200 lg:ease-out lg:shadow-xl"
          : ""
      }`}
    >
      <div className={`p-4 border-b border-gray-700 flex items-center justify-between ${railRow}`}>
        <h1 className="text-xl font-bold flex items-center gap-2 whitespace-nowrap">
          <Hammer className="w-6 h-6 shrink-0 text-amber-400" />
          <span className={railLabel}>Atelier</span>
        </h1>
        {onClose && (
          <button
            onClick={onClose}
            className="lg:hidden p-1 text-gray-400 hover:text-gray-50"
          >
            <X className="w-5 h-5" />
          </button>
        )}
      </div>

      <nav className="flex-1 min-h-0 py-2 overflow-y-auto overflow-x-hidden">
        {navGroups.map((group, gi) => (
          <div key={gi}>
            {group.label && (
              <div className={`px-4 pt-4 pb-1 text-xs text-gray-500 uppercase tracking-wider whitespace-nowrap ${railText}`}>
                {group.label}
              </div>
            )}
            <ul className="space-y-0.5">
              {group.items.map((item) => {
                const { icon: Icon, label, highlight, external, href, to, action } = item;
                if (action) {
                  return (
                    <li key={action}>
                      <button
                        onClick={() => { window.dispatchEvent(new CustomEvent(action)); onClose?.(); }}
                        className={`w-full flex items-center gap-3 px-4 py-2 transition-[background-color,color] duration-300 ease-out hover:duration-0 text-sm border-l-3 border-transparent text-gray-300 hover:bg-gray-700/30 ${railRow}`}
                      >
                        <Icon className="w-5 h-5 shrink-0" />
                        <span className={`flex-1 text-left whitespace-nowrap ${railLabel}`}>{label}</span>
                      </button>
                    </li>
                  );
                }
                if (external) {
                  return (
                    <li key={href}>
                      <a
                        href={href}
                        target="_blank"
                        rel="noopener noreferrer"
                        className={`flex items-center gap-3 px-4 py-2 transition-[background-color,color] duration-300 ease-out hover:duration-0 text-sm border-l-3 border-transparent text-gray-300 hover:bg-gray-700/30 ${railRow}`}
                      >
                        <Icon className="w-5 h-5 shrink-0" />
                        <span className={`flex-1 whitespace-nowrap ${railLabel}`}>{label}</span>
                        <ExternalLink className="w-3.5 h-3.5 text-gray-500" />
                      </a>
                    </li>
                  );
                }
                // "Applications" entry: opens the gallery (landing `/`); the
                // sub-menu lists recently-opened apps → un clic ouvre leur Studio
                // dans un nouvel onglet.
                if (to === '/') {
                  return (
                    <li key={to}>
                      <NavLink to={to} end className={(s) => `${linkClass(s)} ${railRow}`}>
                        <Icon className={`w-5 h-5 shrink-0${highlight ? ' text-amber-400' : ''}`} />
                        <span className={`flex-1 whitespace-nowrap ${railLabel}`}>{label}</span>
                      </NavLink>
                      {onHome && (
                        <div className={`py-0.5 ${collapsed ? "lg:hidden lg:group-hover/aside:block" : ""}`}>
                          {recentApps.map((app) => {
                            const state = (app.state || '').toLowerCase();
                            const isRunning = state === 'running';
                            const isBuilding = buildingApps.has(app.slug);
                            return (
                              <div
                                key={app.slug}
                                onClick={() => handleSelectApp(app.slug)}
                                className="group flex items-center gap-2.5 pl-11 pr-3 py-1.5 text-[13px] cursor-pointer border-l-3 border-transparent text-gray-400 hover:bg-gray-700/30 transition-[background-color,color] duration-300 ease-out hover:duration-0"
                                title={`Ouvrir le Studio de ${app.name} (nouvel onglet)`}
                              >
                                {isBuilding ? (
                                  <Loader2 className="w-[11px] h-[11px] shrink-0 text-blue-400 animate-spin" title="Build en cours" />
                                ) : (
                                  <span className={`w-[7px] h-[7px] rounded-full shrink-0 ${statusDot(state)}`} />
                                )}
                                <span className="flex-1 truncate">{app.name}</span>
                                {/* Actions toujours visibles au tactile (pas de hover) ;
                                    révélées au survol sur desktop (lg). */}
                                <div className="flex items-center opacity-100 lg:opacity-0 lg:group-hover:opacity-100 transition-opacity">
                                  {isRunning ? (
                                    <button onClick={(e) => { e.stopPropagation(); control(app.slug, 'stop'); }} className="p-1.5 sm:p-0.5 text-yellow-400 hover:bg-gray-600 rounded-sm" title="Stop">
                                      <Square className="w-3 h-3" />
                                    </button>
                                  ) : (
                                    <button onClick={(e) => { e.stopPropagation(); control(app.slug, 'start'); }} className="p-1.5 sm:p-0.5 text-green-400 hover:bg-gray-600 rounded-sm" title="Start">
                                      <Play className="w-3 h-3" />
                                    </button>
                                  )}
                                </div>
                              </div>
                            );
                          })}
                          {recentApps.length === 0 && (
                            <div className="pl-11 pr-3 py-2 text-xs text-gray-600 italic">Aucune app récente</div>
                          )}
                        </div>
                      )}
                    </li>
                  );
                }
                return (
                  <li key={to}>
                    <NavLink to={to} className={(s) => `${linkClass(s)} ${railRow}`}>
                      <Icon className={`w-5 h-5 shrink-0${highlight ? ' text-amber-400' : ''}`} />
                      <span className={`flex-1 whitespace-nowrap ${railLabel}`}>{label}</span>
                      {to === '/backlog' && <span className={railLabel}><PilotBadge /></span>}
                    </NavLink>
                  </li>
                );
              })}
            </ul>
          </div>
        ))}
      </nav>

      <div className="p-4 border-t border-gray-700">
        {user && (
          <div className={`flex items-center justify-between ${collapsed ? 'lg:justify-center lg:group-hover/aside:justify-between' : ''}`}>
            <div className="flex items-center gap-2 min-w-0">
              <User className="w-4 h-4 text-gray-400 shrink-0" />
              <div className={`min-w-0 ${railLabel}`}>
                <p className="text-sm text-gray-300 truncate">
                  {user.displayName || user.username}
                </p>
                <p className="text-xs text-amber-400 whitespace-nowrap">CloudMaster</p>
              </div>
            </div>
            <button
              onClick={logout}
              className={`p-2 text-gray-400 hover:text-red-400 hover:bg-gray-700 transition-[background-color,color] duration-300 ease-out hover:duration-0 ${railLabel}`}
              title="Deconnexion"
            >
              <LogOut className="w-4 h-4" />
            </button>
          </div>
        )}
        <div className="mt-2">
          <NotificationsToggle collapsed={collapsed} />
          <InstallButton collapsed={collapsed} />
        </div>
        <p className={`text-xs text-gray-500 mt-2 ${railText}`}>Atelier · v0.1.0</p>
      </div>
    </aside>
  );
}

export default Sidebar;
