import { useEffect, useRef, useState } from 'react';
import { AlertCircle, Loader2 } from 'lucide-react';

// Lazy-load mermaid only when this component mounts (saves ~600KB on the initial bundle).
let mermaidPromise = null;
function loadMermaid() {
  if (!mermaidPromise) {
    mermaidPromise = import('mermaid').then((mod) => {
      const mermaid = mod.default || mod;
      mermaid.initialize({
        startOnLoad: false,
        theme: 'dark',
        securityLevel: 'strict',
        flowchart: { htmlLabels: true, curve: 'basis' },
        themeVariables: {
          primaryColor: '#1f2937',
          primaryTextColor: '#f3f4f6',
          primaryBorderColor: '#3b82f6',
          lineColor: '#9ca3af',
          fontSize: '14px',
        },
      });
      return mermaid;
    });
  }
  return mermaidPromise;
}

let uniqueId = 0;

export default function MermaidDiagram({ code, className = '' }) {
  const containerRef = useRef(null);
  const [error, setError] = useState(null);
  const [loading, setLoading] = useState(true);

  useEffect(() => {
    let cancelled = false;
    if (!code || !code.trim()) {
      setLoading(false);
      return undefined;
    }
    setLoading(true);
    setError(null);
    const id = `mermaid-${++uniqueId}`;
    loadMermaid()
      .then((mermaid) => {
        if (cancelled) return null;
        return mermaid.render(id, code);
      })
      .then((result) => {
        if (cancelled || !result) return;
        if (containerRef.current) {
          containerRef.current.innerHTML = result.svg;
          if (typeof result.bindFunctions === 'function') {
            try { result.bindFunctions(containerRef.current); } catch { /* noop */ }
          }
        }
      })
      .catch((err) => {
        if (cancelled) return;
        setError(err?.message || String(err));
      })
      .finally(() => {
        if (!cancelled) setLoading(false);
      });
    return () => {
      cancelled = true;
    };
  }, [code]);

  if (!code || !code.trim()) return null;

  return (
    <div className={`relative rounded-lg border border-gray-700 bg-gray-900 p-4 ${className}`}>
      {loading && (
        <div className="absolute inset-0 flex items-center justify-center bg-gray-900/70 rounded-lg">
          <Loader2 className="w-5 h-5 animate-spin text-blue-400" />
        </div>
      )}
      {error && (
        <div className="text-red-400 text-sm flex items-start gap-2">
          <AlertCircle className="w-4 h-4 mt-0.5 flex-shrink-0" />
          <div>
            <div className="font-medium">Erreur de rendu mermaid</div>
            <pre className="text-xs mt-1 opacity-80 whitespace-pre-wrap">{error}</pre>
          </div>
        </div>
      )}
      <div ref={containerRef} className="overflow-x-auto [&_svg]:max-w-full [&_svg]:h-auto" />
    </div>
  );
}
