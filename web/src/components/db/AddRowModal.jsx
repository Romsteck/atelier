import { useState } from 'react';
import { X, Plus, Loader2 } from 'lucide-react';
import { getFieldConfig, isReadOnly } from './fieldTypes';
import { LookupCombobox } from './LookupCombobox';

export function AddRowModal({ columns, relations, appSlug, onInsert, onClose }) {
  const editableCols = (columns || []).filter(c => !c.primary_key && !isReadOnly(c.field_type));
  const [values, setValues] = useState(() => {
    const init = {};
    editableCols.forEach(c => {
      init[c.name] = c.field_type === 'Boolean' ? false : '';
    });
    return init;
  });
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState(null);

  const relationMap = {};
  if (relations) {
    relations.forEach(r => { relationMap[r.from_column] = r; });
  }

  const handleSubmit = async (e) => {
    e.preventDefault();
    setSaving(true);
    setError(null);
    try {
      const row = {};
      editableCols.forEach(c => {
        const v = values[c.name];
        if (c.field_type === 'Boolean') {
          row[c.name] = v ? 1 : 0;
        } else if (v === '' || v == null) {
          if (!c.required) row[c.name] = null;
        } else {
          row[c.name] = coerce(v, c.field_type);
        }
      });
      await onInsert(row);
      onClose();
    } catch (err) {
      setError(err.message || 'Erreur');
    } finally {
      setSaving(false);
    }
  };

  const setValue = (name, val) => setValues(prev => ({ ...prev, [name]: val }));

  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/50">
      <div className="bg-gray-800 rounded-lg border border-gray-700 shadow-xl w-full max-w-md max-h-[80vh] flex flex-col">
        <div className="flex items-center justify-between px-4 py-3 border-b border-gray-700">
          <h3 className="text-sm font-semibold text-white flex items-center gap-2">
            <Plus className="w-4 h-4 text-blue-400" /> Ajouter une ligne
          </h3>
          <button onClick={onClose} className="p-1 text-gray-400 hover:text-white rounded hover:bg-gray-700 border-none bg-transparent cursor-pointer">
            <X className="w-4 h-4" />
          </button>
        </div>
        <form onSubmit={handleSubmit} className="flex-1 overflow-y-auto p-4 space-y-3">
          {editableCols.map(col => {
            const cfg = getFieldConfig(col.field_type);
            const rel = relationMap[col.name];
            return (
              <div key={col.name}>
                <label className="block text-xs text-gray-400 mb-1">
                  {col.name}
                  {col.required && <span className="text-red-400 ml-1">*</span>}
                  <span className="text-gray-600 ml-1">({col.field_type})</span>
                </label>
                <FieldInput
                  col={col}
                  cfg={cfg}
                  relation={rel}
                  appSlug={appSlug}
                  value={values[col.name]}
                  onChange={(v) => setValue(col.name, v)}
                />
              </div>
            );
          })}
          {error && <div className="text-xs text-red-400 bg-red-500/10 rounded px-3 py-2">{error}</div>}
        </form>
        <div className="flex justify-end gap-2 px-4 py-3 border-t border-gray-700">
          <button type="button" onClick={onClose} className="px-3 py-1.5 text-xs text-gray-400 rounded border-none bg-transparent cursor-pointer hover:text-white">
            Annuler
          </button>
          <button onClick={handleSubmit} disabled={saving} className="px-4 py-1.5 text-xs text-white bg-blue-500 rounded border-none cursor-pointer hover:bg-blue-600 disabled:opacity-50 flex items-center gap-1">
            {saving ? <><Loader2 className="w-3 h-3 animate-spin" /> Ajout...</> : <><Plus className="w-3 h-3" /> Ajouter</>}
          </button>
        </div>
      </div>
    </div>
  );
}

function FieldInput({ col, cfg, relation, appSlug, value, onChange }) {
  const baseClass = "w-full bg-gray-900 text-white text-sm rounded px-3 py-1.5 border border-gray-600 outline-none focus:border-blue-500";

  // Boolean → toggle
  if (col.field_type === 'Boolean') {
    return (
      <label className="flex items-center gap-2 cursor-pointer">
        <input
          type="checkbox"
          checked={!!value}
          onChange={e => onChange(e.target.checked)}
          className="w-4 h-4 rounded"
        />
        <span className="text-sm text-gray-300">{value ? 'Vrai' : 'Faux'}</span>
      </label>
    );
  }

  // Choice → select
  if (col.field_type === 'Choice' && col.choices?.length > 0) {
    return (
      <select value={value} onChange={e => onChange(e.target.value)} className={baseClass}>
        {!col.required && <option value="">-- Aucun --</option>}
        {col.choices.map(c => <option key={c} value={c}>{c}</option>)}
      </select>
    );
  }

  // MultiChoice → checkboxes
  if (col.field_type === 'MultiChoice' && col.choices?.length > 0) {
    const selected = value ? (typeof value === 'string' ? JSON.parse(value || '[]') : value) : [];
    return (
      <div className="flex flex-wrap gap-2">
        {col.choices.map(c => (
          <label key={c} className="flex items-center gap-1 text-xs text-gray-300 cursor-pointer">
            <input
              type="checkbox"
              checked={selected.includes(c)}
              onChange={e => {
                const next = e.target.checked ? [...selected, c] : selected.filter(s => s !== c);
                onChange(JSON.stringify(next));
              }}
              className="w-3 h-3"
            />
            {c}
          </label>
        ))}
      </div>
    );
  }

  // Lookup → combobox
  if (col.field_type === 'Lookup' && relation) {
    return (
      <LookupCombobox
        appSlug={appSlug}
        relation={relation}
        value={value || null}
        onChange={onChange}
        required={col.required}
      />
    );
  }

  // Json → textarea
  if (col.field_type === 'Json') {
    return (
      <textarea
        value={value}
        onChange={e => onChange(e.target.value)}
        className={`${baseClass} font-mono text-xs h-20 resize-y`}
        placeholder={col.required ? 'Requis (JSON)' : 'Optionnel (null)'}
      />
    );
  }

  // Default: typed input
  return (
    <input
      type={cfg.inputType || 'text'}
      step={cfg.step}
      value={value}
      onChange={e => onChange(e.target.value)}
      required={col.required}
      className={`${baseClass} ${cfg.mono ? 'font-mono text-xs' : ''}`}
      placeholder={col.required ? 'Requis' : 'Optionnel (null)'}
    />
  );
}

function coerce(value, fieldType) {
  switch (fieldType) {
    case 'Number':
    case 'AutoIncrement':
    case 'Lookup': {
      const n = parseInt(value, 10);
      return isNaN(n) ? value : n;
    }
    case 'Decimal':
    case 'Currency':
    case 'Percent': {
      const n = parseFloat(value);
      return isNaN(n) ? value : n;
    }
    case 'Boolean':
      return value ? 1 : 0;
    default:
      return value;
  }
}
