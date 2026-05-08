import { useState, useEffect, useRef } from 'react';
import { queryAppDb } from '../../api/client';
import { ChevronDown, Loader2 } from 'lucide-react';

/**
 * Searchable combobox for Lookup (foreign key) fields.
 * Fetches options from the related table and renders a filterable dropdown.
 */
export function LookupCombobox({ appSlug, relation, value, onChange, required }) {
  const [options, setOptions] = useState([]);
  const [loading, setLoading] = useState(false);
  const [open, setOpen] = useState(false);
  const [search, setSearch] = useState('');
  const ref = useRef(null);

  useEffect(() => {
    if (!appSlug || !relation) return;
    setLoading(true);
    const sql = `SELECT "${relation.to_column}", "${relation.display_column}" FROM "${relation.to_table}" ORDER BY "${relation.display_column}" LIMIT 500`;
    queryAppDb(appSlug, sql)
      .then((res) => {
        const d = res.data;
        const data = d && typeof d === 'object' && 'data' in d ? d.data : d;
        setOptions(data?.rows || []);
      })
      .catch(() => setOptions([]))
      .finally(() => setLoading(false));
  }, [appSlug, relation?.to_table, relation?.to_column, relation?.display_column]);

  useEffect(() => {
    const handler = (e) => {
      if (ref.current && !ref.current.contains(e.target)) setOpen(false);
    };
    document.addEventListener('mousedown', handler);
    return () => document.removeEventListener('mousedown', handler);
  }, []);

  const filtered = options.filter((o) => {
    if (!search) return true;
    const display = String(o[relation.display_column] ?? o[relation.to_column] ?? '');
    return display.toLowerCase().includes(search.toLowerCase());
  });

  const selectedDisplay = options.find(
    (o) => o[relation.to_column] == value
  )?.[relation.display_column];

  return (
    <div ref={ref} className="relative">
      <div
        className="flex items-center w-full bg-gray-900 text-white text-sm rounded px-3 py-1.5 border border-gray-600 cursor-pointer"
        onClick={() => setOpen(!open)}
      >
        <span className={`flex-1 truncate ${!value && value !== 0 ? 'text-gray-500' : ''}`}>
          {value != null ? `${selectedDisplay ?? value}` : (required ? 'Requis' : 'Aucun')}
        </span>
        {loading ? (
          <Loader2 className="w-3 h-3 animate-spin text-gray-400 ml-1" />
        ) : (
          <ChevronDown className="w-3 h-3 text-gray-400 ml-1" />
        )}
      </div>
      {open && (
        <div className="absolute z-50 mt-1 w-full bg-gray-800 border border-gray-600 rounded shadow-xl max-h-48 flex flex-col">
          <input
            type="text"
            value={search}
            onChange={(e) => setSearch(e.target.value)}
            placeholder="Rechercher..."
            className="px-2 py-1 text-xs bg-gray-900 text-white border-b border-gray-700 outline-none rounded-t"
            autoFocus
          />
          <div className="overflow-y-auto flex-1">
            {!required && (
              <div
                className="px-3 py-1.5 text-xs text-gray-500 hover:bg-gray-700 cursor-pointer"
                onClick={() => { onChange(null); setOpen(false); setSearch(''); }}
              >
                (aucun)
              </div>
            )}
            {filtered.map((o) => {
              const id = o[relation.to_column];
              const display = o[relation.display_column] ?? id;
              return (
                <div
                  key={id}
                  className={`px-3 py-1.5 text-xs cursor-pointer hover:bg-gray-700 flex justify-between ${
                    id == value ? 'bg-blue-500/20 text-blue-300' : 'text-gray-300'
                  }`}
                  onClick={() => { onChange(id); setOpen(false); setSearch(''); }}
                >
                  <span className="truncate">{display}</span>
                  <span className="text-gray-600 ml-2">#{id}</span>
                </div>
              );
            })}
            {filtered.length === 0 && (
              <div className="px-3 py-2 text-xs text-gray-500">Aucun resultat</div>
            )}
          </div>
        </div>
      )}
    </div>
  );
}
