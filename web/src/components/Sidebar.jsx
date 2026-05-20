import { NavLink, useLocation } from 'react-router-dom';
import {
  LogOut, User, Code2, Database, Hammer,
  GitBranch, X, ExternalLink, TableProperties, Workflow
} from 'lucide-react';
import { useAuth } from '../context/AuthContext';
import { useEffect } from 'react';

const navGroups = [
  {
    label: 'Applications',
    items: [
      { to: '/studio', icon: Code2, label: 'Studio', highlight: true },
      { to: '/database', icon: Database, label: 'Base de donnees' },
      { to: '/schema', icon: TableProperties, label: 'Schema' },
      { to: '/git', icon: GitBranch, label: 'Git' },
      { to: '/flows-stats', icon: Workflow, label: 'Flow Stats' },
    ],
  },
];

function Sidebar({ onClose }) {
  const { user, logout } = useAuth();
  const location = useLocation();

  useEffect(() => {
    if (onClose) onClose();
  }, [location.pathname]);

  return (
    <aside className="w-64 h-full bg-gray-800 border-r border-gray-700 flex flex-col">
      <div className="p-4 border-b border-gray-700 flex items-center justify-between">
        <h1 className="text-xl font-bold flex items-center gap-2">
          <Hammer className="w-6 h-6 text-amber-400" />
          Atelier
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

      <nav className="flex-1 py-2 overflow-y-auto">
        {navGroups.map((group, gi) => (
          <div key={gi}>
            {group.label && (
              <div className="px-4 pt-4 pb-1 text-xs text-gray-500 uppercase tracking-wider">
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
                        className="flex items-center gap-3 px-4 py-2 transition-[background-color,color] duration-300 ease-out hover:duration-0 text-sm border-l-3 border-transparent text-gray-300 hover:bg-gray-700/30"
                      >
                        <Icon className="w-5 h-5" />
                        <span className="flex-1">{label}</span>
                        <ExternalLink className="w-3.5 h-3.5 text-gray-500" />
                      </a>
                    </li>
                  );
                }
                return (
                  <li key={to}>
                    <NavLink
                      to={to}
                      className={({ isActive }) =>
                        `flex items-center gap-3 px-4 py-2 transition-[background-color,color] duration-300 ease-out hover:duration-0 text-sm ${
                          isActive
                            ? 'border-l-3 border-amber-400 bg-gray-700/50 text-white'
                            : 'border-l-3 border-transparent text-gray-300 hover:bg-gray-700/30'
                        }`
                      }
                    >
                      <Icon className={`w-5 h-5${highlight ? ' text-amber-400' : ''}`} />
                      <span className="flex-1">{label}</span>
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
          <div className="flex items-center justify-between">
            <div className="flex items-center gap-2 min-w-0">
              <User className="w-4 h-4 text-gray-400 flex-shrink-0" />
              <div className="min-w-0">
                <p className="text-sm text-gray-300 truncate">
                  {user.displayName || user.username}
                </p>
                <p className="text-xs text-amber-400">CloudMaster</p>
              </div>
            </div>
            <button
              onClick={logout}
              className="p-2 text-gray-400 hover:text-red-400 hover:bg-gray-700 transition-[background-color,color] duration-300 ease-out hover:duration-0"
              title="Deconnexion"
            >
              <LogOut className="w-4 h-4" />
            </button>
          </div>
        )}
        <p className="text-xs text-gray-500 mt-2">Atelier · v0.1.0</p>
      </div>
    </aside>
  );
}

export default Sidebar;
