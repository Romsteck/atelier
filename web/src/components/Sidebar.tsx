import { NavLink } from "react-router-dom";
import { Hammer, ExternalLink } from "lucide-react";
import { NAV } from "../nav";

export default function Sidebar() {
  return (
    <aside className="w-60 h-full bg-gray-900 border-r border-gray-800 flex flex-col shrink-0">
      <div className="px-4 py-4 border-b border-gray-800 flex items-center gap-2">
        <Hammer className="w-5 h-5 text-amber-400" />
        <div>
          <div className="text-base font-semibold leading-none">Atelier</div>
          <div className="text-[10px] text-gray-500 mt-0.5 uppercase tracking-wider">
            HomeRoute
          </div>
        </div>
      </div>

      <nav className="flex-1 py-2 overflow-y-auto">
        {NAV.map((group) => (
          <div key={group.label} className="mb-2">
            <div className="px-4 pt-3 pb-1 text-[10px] text-gray-600 uppercase tracking-wider">
              {group.label}
            </div>
            <ul className="space-y-0.5">
              {group.items.map((item) => {
                const Icon = item.icon;
                const disabled = !item.ready;
                const cls = ({ isActive }: { isActive: boolean }) =>
                  `flex items-center gap-2 px-4 py-1.5 text-[13px] border-l-2 transition-colors ${
                    disabled
                      ? "border-transparent text-gray-600 cursor-default"
                      : isActive
                        ? "border-amber-400 bg-gray-800/60 text-white"
                        : "border-transparent text-gray-300 hover:bg-gray-800/40"
                  }`;
                if (disabled) {
                  return (
                    <li key={item.to}>
                      <NavLink to={item.to} className={cls} end={false}>
                        <Icon className="w-4 h-4" />
                        <span className="flex-1">{item.label}</span>
                        <span className="badge !text-[9px] !px-1 !py-0">
                          P{item.phase}
                        </span>
                      </NavLink>
                    </li>
                  );
                }
                return (
                  <li key={item.to}>
                    <NavLink to={item.to} className={cls}>
                      <Icon className="w-4 h-4" />
                      <span className="flex-1">{item.label}</span>
                    </NavLink>
                  </li>
                );
              })}
            </ul>
          </div>
        ))}

        <div className="px-4 pt-3 pb-1 text-[10px] text-gray-600 uppercase tracking-wider">
          Réseau
        </div>
        <a
          href="https://proxy.mynetwk.biz"
          target="_blank"
          rel="noopener noreferrer"
          className="flex items-center gap-2 px-4 py-1.5 text-[13px] text-gray-400 hover:bg-gray-800/40 border-l-2 border-transparent"
        >
          <ExternalLink className="w-4 h-4" />
          <span className="flex-1">HomeRoute Dashboard</span>
        </a>
      </nav>

      <div className="px-4 py-3 border-t border-gray-800 text-[11px] text-gray-500">
        <div className="flex items-center justify-between">
          <span>v0.1.0</span>
          <span>CloudMaster</span>
        </div>
        <div className="mt-1 text-gray-600">Phase 2 — Docs read-only</div>
      </div>
    </aside>
  );
}
