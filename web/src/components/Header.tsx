import { useEffect, useState } from "react";
import { useLocation } from "react-router-dom";
import { RefreshCw } from "lucide-react";
import { findItem } from "../nav";

export default function Header() {
  const { pathname } = useLocation();
  const item = findItem(pathname);
  const title = item?.label ?? "Atelier";

  const [healthy, setHealthy] = useState<boolean | null>(null);
  useEffect(() => {
    let cancel = false;
    const check = () =>
      fetch("/api/health")
        .then((r) => !cancel && setHealthy(r.ok))
        .catch(() => !cancel && setHealthy(false));
    check();
    const t = setInterval(check, 30_000);
    return () => {
      cancel = true;
      clearInterval(t);
    };
  }, []);

  return (
    <header className="h-12 bg-gray-900 border-b border-gray-800 flex items-center px-4 shrink-0">
      <h1 className="text-sm font-semibold text-gray-100">{title}</h1>
      {item && (
        <span className="ml-2 badge">
          Phase {item.phase}
          {item.ready ? "" : " · planned"}
        </span>
      )}
      <div className="ml-auto flex items-center gap-3 text-[11px] text-gray-500">
        <span className="flex items-center gap-1">
          <span
            className={`w-2 h-2 rounded-full ${
              healthy === null
                ? "bg-gray-500"
                : healthy
                  ? "bg-emerald-500"
                  : "bg-red-500"
            }`}
          />
          {healthy === null ? "…" : healthy ? "API OK" : "API DOWN"}
        </span>
        <button
          onClick={() => window.location.reload()}
          className="p-1 hover:text-gray-200"
          title="Recharger"
        >
          <RefreshCw className="w-3.5 h-3.5" />
        </button>
      </div>
    </header>
  );
}
