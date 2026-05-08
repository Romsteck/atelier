import { useState, useRef, useEffect } from 'react';
import { Filter, X } from 'lucide-react';

const OPS = [
  { value: 'eq', label: '=' },
  { value: 'neq', label: '!=' },
  { value: 'gt', label: '>' },
  { value: 'gte', label: '>=' },
  { value: 'lt', label: '<' },
  { value: 'lte', label: '<=' },
  { value: 'like', label: 'LIKE' },
  { value: 'is_null', label: 'IS NULL' },
  { value: 'not_null', label: 'NOT NULL' },
];

export function FilterDropdown({ column, fieldType, choices, currentFilter, onFilterChange }) {
  const [open, setOpen] = useState(false);
  const [op, setOp] = useState(currentFilter?.op || 'eq');
  const [value, setValue] = useState(currentFilter?.value ?? '');
  const ref = useRef(null);

  useEffect(() => {
    const handler = (e) => { if (ref.current && !ref.current.contains(e.target)) setOpen(false); };
    document.addEventListener('mousedown', handler);
    return () => document.removeEventListener('mousedown', handler);
  }, []);

  const apply = () => {
    if (op === 'is_null' || op === 'not_null') {
      onFilterChange(column, { column, op, value: null });
    } else if (String(value).trim() !== '' || fieldType === 'Boolean') {
      onFilterChange(column, { column, op, value: fieldType === 'Boolean' ? value : String(value).trim() });
    }
    setOpen(false);
  };

  const clear = () => {
    onFilterChange(column, null);
    setOp('eq');
    setValue('');
    setOpen(false);
  };

  const hasFilter = !!currentFilter;
  const needsValue = op !== 'is_null' && op !== 'not_null';

  // Determine input type based on field type
  let inputType = 'text';
  if (fieldType === 'Number' || fieldType === 'Decimal' || fieldType === 'Currency' || fieldType === 'Percent') inputType = 'number';
  if (fieldType === 'Date') inputType = 'date';
  if (fieldType === 'DateTime') inputType = 'datetime-local';

  return (
    <div className="relative inline-block" ref={ref}>
      <button
        onClick={() => setOpen(!open)}
        className={`p-0.5 rounded border-none bg-transparent cursor-pointer ${
          hasFilter ? 'text-blue-400' : 'text-gray-600 hover:text-gray-400'
        }`}
        title={hasFilter ? `Filtre: ${currentFilter.op} ${currentFilter.value ?? ''}` : 'Filtrer'}
      >
        <Filter className="w-3 h-3" />
      </button>
      {open && (
        <div className="absolute top-full left-0 mt-1 z-50 bg-gray-800 border border-gray-600 rounded-lg shadow-xl p-3 min-w-[200px]">
          <div className="text-[10px] text-gray-400 uppercase tracking-wider mb-2">{column}</div>
          <select
            value={op}
            onChange={e => setOp(e.target.value)}
            className="w-full bg-gray-700 text-white text-xs rounded px-2 py-1.5 border border-gray-600 mb-2"
          >
            {OPS.map(o => <option key={o.value} value={o.value}>{o.label}</option>)}
          </select>
          {needsValue && fieldType === 'Boolean' && (
            <select
              value={value}
              onChange={e => setValue(e.target.value)}
              className="w-full bg-gray-900 text-white text-xs rounded px-2 py-1.5 border border-gray-600 mb-2"
            >
              <option value="1">Vrai</option>
              <option value="0">Faux</option>
            </select>
          )}
          {needsValue && fieldType === 'Choice' && choices?.length > 0 && (
            <select
              value={value}
              onChange={e => setValue(e.target.value)}
              className="w-full bg-gray-900 text-white text-xs rounded px-2 py-1.5 border border-gray-600 mb-2"
            >
              <option value="">--</option>
              {choices.map(c => <option key={c} value={c}>{c}</option>)}
            </select>
          )}
          {needsValue && fieldType !== 'Boolean' && !(fieldType === 'Choice' && choices?.length > 0) && (
            <input
              type={inputType}
              value={value}
              onChange={e => setValue(e.target.value)}
              onKeyDown={e => e.key === 'Enter' && apply()}
              placeholder="Valeur..."
              className="w-full bg-gray-900 text-white text-xs rounded px-2 py-1.5 border border-gray-600 mb-2 outline-none"
              autoFocus
            />
          )}
          <div className="flex justify-between">
            {hasFilter && (
              <button onClick={clear} className="text-xs text-red-400 hover:text-red-300 flex items-center gap-1 border-none bg-transparent cursor-pointer">
                <X className="w-3 h-3" /> Retirer
              </button>
            )}
            <button onClick={apply} className="text-xs text-blue-400 hover:text-blue-300 ml-auto border-none bg-transparent cursor-pointer">
              Appliquer
            </button>
          </div>
        </div>
      )}
    </div>
  );
}
