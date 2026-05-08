import { useEffect, useRef, useState } from 'react';
import { Search, X, Loader2 } from 'lucide-react';
import { searchDocs } from '../../api/client';
import EntryTypeBadge from './EntryTypeBadge';

const TYPES = [
  { key: null, label: 'Tous' },
  { key: 'screen', label: 'Écrans' },
  { key: 'feature', label: 'Features' },
  { key: 'component', label: 'Composants' },
];

function highlight(snippet) {
  // The backend wraps matches with <mark>…</mark>. We trust them since rehype-sanitize is
  // not used here (the snippet is plain text already, but we still escape any non-mark HTML).
  // Simpler approach: split on the marks and render via React.
  if (!snippet) return null;
  const parts = snippet.split(/(<mark>|<\/mark>)/g);
  let inMark = false;
  return parts
    .filter((p) => p !== '<mark>' && p !== '</mark>' || !!(inMark = (p === '<mark>' ? true : p === '</mark>' ? false : inMark)))
    .map((p, i) => {
      if (p === '<mark>') { inMark = true; return null; }
      if (p === '</mark>') { inMark = false; return null; }
      return inMark ? (
        <mark key={i} className="bg-yellow-400/30 text-yellow-200 px-0.5 rounded">{p}</mark>
      ) : (
        <span key={i}>{p}</span>
      );
    });
}

export default function DocsSearch({ appId, onPick }) {
  const [query, setQuery] = useState('');
  const [type, setType] = useState(null);
  const [results, setResults] = useState([]);
  const [loading, setLoading] = useState(false);
  const [open, setOpen] = useState(false);
  const debounceRef = useRef(null);
  const inputRef = useRef(null);

  // Debounce search
  useEffect(() => {
    if (debounceRef.current) clearTimeout(debounceRef.current);
    if (!query.trim()) {
      setResults([]);
      setOpen(false);
      return undefined;
    }
    setLoading(true);
    debounceRef.current = setTimeout(async () => {
      try {
        const params = { q: query, app_id: appId, limit: 20 };
        if (type) params.type = type;
        const res = await searchDocs(params);
        setResults(res.data?.results || []);
        setOpen(true);
      } catch (e) {
        setResults([]);
      } finally {
        setLoading(false);
      }
    }, 250);
    return () => debounceRef.current && clearTimeout(debounceRef.current);
  }, [query, type, appId]);

  // Ctrl/Cmd+K shortcut to focus
  useEffect(() => {
    const onKey = (e) => {
      if ((e.ctrlKey || e.metaKey) && e.key === 'k') {
        e.preventDefault();
        inputRef.current?.focus();
      }
      if (e.key === 'Escape') {
        setOpen(false);
      }
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, []);

  return (
    <div className="relative">
      <div className="flex items-center gap-2">
        <div className="relative flex-1">
          <Search className="w-4 h-4 absolute left-3 top-1/2 -translate-y-1/2 text-gray-500" />
          <input
            ref={inputRef}
            type="text"
            value={query}
            onChange={(e) => setQuery(e.target.value)}
            onFocus={() => query.trim() && setOpen(true)}
            placeholder="Rechercher dans la documentation… (Ctrl+K)"
            className="w-full pl-9 pr-9 py-2 bg-gray-800 border border-gray-700 rounded-lg
                       text-sm text-gray-200 placeholder-gray-500
                       focus:outline-none focus:ring-2 focus:ring-blue-500/40 focus:border-blue-500"
          />
          {query && (
            <button
              onClick={() => { setQuery(''); setResults([]); setOpen(false); inputRef.current?.focus(); }}
              className="absolute right-3 top-1/2 -translate-y-1/2 text-gray-500 hover:text-gray-300"
            >
              <X className="w-4 h-4" />
            </button>
          )}
          {loading && (
            <Loader2 className="w-4 h-4 absolute right-9 top-1/2 -translate-y-1/2 text-blue-400 animate-spin" />
          )}
        </div>
        <div className="flex gap-1">
          {TYPES.map((t) => (
            <button
              key={t.label}
              onClick={() => setType(t.key)}
              className={`px-2 py-1 text-xs rounded border transition-colors
                          ${type === t.key
                            ? 'bg-blue-500/20 border-blue-500 text-blue-200'
                            : 'border-gray-700 text-gray-400 hover:border-gray-500 hover:text-gray-200'}`}
            >
              {t.label}
            </button>
          ))}
        </div>
      </div>

      {open && results.length > 0 && (
        <div className="absolute z-20 mt-2 w-full max-h-[60vh] overflow-y-auto
                        bg-gray-900 border border-gray-700 rounded-lg shadow-xl">
          {results.map((hit, idx) => (
            <button
              key={`${hit.app_id}-${hit.doc_type}-${hit.name}-${idx}`}
              onClick={() => {
                onPick({ type: hit.doc_type, name: hit.name });
                setOpen(false);
              }}
              className="w-full text-left px-3 py-2 hover:bg-gray-800 border-b border-gray-800 last:border-b-0"
            >
              <div className="flex items-center gap-2 mb-1">
                <EntryTypeBadge type={hit.doc_type} />
                <span className="text-sm font-medium text-white truncate">
                  {hit.title || hit.name}
                </span>
                <span className="text-xs text-gray-500 ml-auto">{hit.name}</span>
              </div>
              <div className="text-xs text-gray-400 line-clamp-2">
                {highlight(hit.snippet)}
              </div>
            </button>
          ))}
        </div>
      )}

      {open && results.length === 0 && !loading && query.trim() && (
        <div className="absolute z-20 mt-2 w-full bg-gray-900 border border-gray-700 rounded-lg shadow-xl px-3 py-4 text-sm text-gray-500 text-center">
          Aucun résultat pour « {query} »
        </div>
      )}
    </div>
  );
}
