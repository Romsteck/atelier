import { NavLink, useLocation, useNavigate } from 'react-router-dom';
import {
  LogOut, User, Code2, Database, Hammer,
  GitBranch, X, ExternalLink, TableProperties, ShieldAlert,
  Play, Square,
} from 'lucide-react';
import { useAuth } from '../context/AuthContext';
import { useStudio } from '../context/StudioContext';
import { statusDot } from '../pages/Studio';
import { useEffect } from 'react';
import InstallButton from './InstallButton';

const navGroups = [
  {
    label: 'Applications',
    items: [
      { to: '/studio', icon: Code2, label: 'Studio', highlight: true },
      { to: '/database', icon: Database, label: 'Base de donnees' },
      { to: '/schema', icon: TableProperties, label: 'Schema' },
      { to: '/git', icon: GitBranch, label: 'Git' },
      { to: '/surveillance', icon: ShieldAlert, label: 'Surveillance' },
    ],
  },
];

const linkClass = ({ isActive }) =>
  `flex items-center gap-3 px-4 py-2 transition-[background-color,color] duration-300 ease-out hover:duration-0 text-sm ${
    isActive
      ? 'border-l-3 border-amber-400 bg-gray-700/50 text-white'
      : 'border-l-3 border-transparent text-gray-300 hover:bg-gray-700/30'
  }`;

function Sidebar({ onClose, collapsed }) {
  const { user, logout } = useAuth();
  const location = useLocation();
  const navigate = useNavigate();
  const { recentApps, selectedSlug, activeTab, onControl } = useStudio();

  useEffect(() => {
    onClose?.();
  }, [location.pathname, onClose]);

  const onStudio = location.pathname === '/studio';

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

  // Selecting a recent app opens its studio.
  function handleSelectApp(slug) {
    navigate(`/studio?app=${encodeURIComponent(slug)}&tab=${activeTab || 'code'}`);
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
            className="lg:hidden p-1 text-gray-400 hover:text-white"
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
                const { icon: Icon, label, highlight, external, href, to } = item;
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
                // "Studio" entry: clicking it opens the gallery; the sub-menu
                // below lists the recently-opened apps for quick switching.
                if (to === '/studio') {
                  return (
                    <li key={to}>
                      <NavLink to={to} className={(s) => `${linkClass(s)} ${railRow}`}>
                        <Icon className={`w-5 h-5 shrink-0${highlight ? ' text-amber-400' : ''}`} />
                        <span className={`flex-1 whitespace-nowrap ${railLabel}`}>{label}</span>
                      </NavLink>
                      {onStudio && (
                        <div className={`py-0.5 ${collapsed ? "lg:hidden lg:group-hover/aside:block" : ""}`}>
                          {recentApps.map((app) => {
                            const state = (app.state || '').toLowerCase();
                            const isRunning = state === 'running';
                            const sel = app.slug === selectedSlug;
                            return (
                              <div
                                key={app.slug}
                                onClick={() => handleSelectApp(app.slug)}
                                className={`group flex items-center gap-2.5 pl-11 pr-3 py-1.5 text-[13px] cursor-pointer border-l-3 transition-[background-color,color] duration-300 ease-out hover:duration-0 ${
                                  sel
                                    ? 'border-amber-400 bg-gray-700/50 text-white'
                                    : 'border-transparent text-gray-400 hover:bg-gray-700/30'
                                }`}
                              >
                                <span className={`w-[7px] h-[7px] rounded-full shrink-0 ${statusDot(state)}`} />
                                <span className="flex-1 truncate">{app.name}</span>
                                <div className="flex items-center opacity-0 group-hover:opacity-100 transition-opacity">
                                  {isRunning ? (
                                    <button onClick={(e) => { e.stopPropagation(); onControl?.(app.slug, 'stop'); }} className="p-0.5 text-yellow-400 hover:bg-gray-600 rounded-sm" title="Stop">
                                      <Square className="w-3 h-3" />
                                    </button>
                                  ) : (
                                    <button onClick={(e) => { e.stopPropagation(); onControl?.(app.slug, 'start'); }} className="p-0.5 text-green-400 hover:bg-gray-600 rounded-sm" title="Start">
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
          <InstallButton collapsed={collapsed} />
        </div>
        <p className={`text-xs text-gray-500 mt-2 ${railText}`}>Atelier · v0.1.0</p>
      </div>
    </aside>
  );
}

export default Sidebar;
