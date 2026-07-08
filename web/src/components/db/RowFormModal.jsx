import { useState } from 'react';
import { X, Plus, Save } from 'lucide-react';
import Button from '../Button';
import { getFieldConfig, isReadOnly, coerceValue } from './fieldTypes';
import { LookupCombobox } from './LookupCombobox';

/**
 * Formulaire de ligne, type-aware, partagé entre l'ajout et l'édition.
 *
 * - `mode="add"`   → champs vides, `onSubmit(row)` insère une nouvelle ligne.
 * - `mode="edit"`  → champs pré-remplis depuis `initialRow`, `onSubmit(id, patch)`
 *   met à jour la ligne (l'`id` PK est affiché en lecture seule).
 *
 * Remplace l'ancienne édition in-line cellule-par-cellule : toute la ligne est
 * éditée dans un seul formulaire cohérent avec l'ajout.
 */
export function RowFormModal({ mode = 'add', columns, relations, appSlug, initialRow, onSubmit, onClose }) {
  const isEdit = mode === 'edit';
  const editableCols = (columns || []).filter(c => !c.primary_key && !isReadOnly(c.field_type));

  const [values, setValues] = useState(() => {
    const init = {};
    editableCols.forEach(c => {
      const raw = isEdit ? initialRow?.[c.name] : undefined;
      if (c.field_type === 'Boolean') {
        init[c.name] = raw === true || raw === 1 || raw === 'true';
      } else if (raw == null) {
        init[c.name] = '';
      } else if (c.field_type === 'Json') {
        init[c.name] = typeof raw === 'string' ? raw : JSON.stringify(raw, null, 2);
      } else if (c.field_type === 'MultiChoice') {
        init[c.name] = typeof raw === 'string' ? raw : JSON.stringify(raw);
      } else {
        init[c.name] = String(raw);
      }
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
          row[c.name] = coerceValue(v, 'Boolean');
        } else if (v === '' || v == null) {
          // En édition on envoie explicitement null (mise à null possible) ;
          // en ajout on n'envoie la clé que si la colonne n'est pas requise.
          if (isEdit || !c.required) row[c.name] = null;
        } else {
          row[c.name] = coerceValue(v, c.field_type);
        }
      });
      if (isEdit) {
        const id = initialRow?.id;
        // `version` = verrou optimiste serveur (header If-Match côté client API).
        await onSubmit(id, row, initialRow?.version);
      } else {
        await onSubmit(row);
      }
      onClose();
    } catch (err) {
      setError(err?.response?.data?.error || err.message || 'Erreur');
    } finally {
      setSaving(false);
    }
  };

  const setValue = (name, val) => setValues(prev => ({ ...prev, [name]: val }));

  const Icon = isEdit ? Save : Plus;
  const title = isEdit ? 'Modifier la ligne' : 'Ajouter une ligne';
  const submitLabel = isEdit ? 'Enregistrer' : 'Ajouter';
  const savingLabel = isEdit ? 'Enregistrement...' : 'Ajout...';

  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center p-4 bg-black/50">
      <div className="bg-gray-800 rounded-lg border border-gray-700 shadow-xl w-full max-w-md max-h-[85vh] flex flex-col">
        <div className="flex items-center justify-between px-4 py-3 border-b border-gray-700">
          <h3 className="text-sm font-semibold text-gray-50 flex items-center gap-2">
            <Icon className="w-4 h-4 text-blue-400" /> {title}
          </h3>
          <button onClick={onClose} className="p-1 text-gray-400 hover:text-gray-50 rounded-sm hover:bg-gray-700 border-none bg-transparent cursor-pointer">
            <X className="w-4 h-4" />
          </button>
        </div>
        <form onSubmit={handleSubmit} className="flex-1 overflow-y-auto p-4 space-y-3">
          {isEdit && initialRow?.id != null && (
            <div className="flex items-center gap-2 text-xs text-gray-500 pb-1">
              <span className="text-[10px] uppercase tracking-wide text-gray-600">id</span>
              <span className="font-mono text-gray-400">{String(initialRow.id)}</span>
            </div>
          )}
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
          {editableCols.length === 0 && (
            <div className="text-xs text-gray-500 text-center py-4">Aucune colonne éditable</div>
          )}
          {error && <div className="text-xs text-red-400 bg-red-500/10 rounded-sm px-3 py-2">{error}</div>}
        </form>
        <div className="flex justify-end gap-2 px-4 py-3 border-t border-gray-700">
          <Button variant="neutral" size="sm" onClick={onClose}>Annuler</Button>
          <Button variant="primary" size="sm" icon={Icon} loading={saving} disabled={saving} onClick={handleSubmit}>
            {saving ? savingLabel : submitLabel}
          </Button>
        </div>
      </div>
    </div>
  );
}

// Valeur MultiChoice stockée : normalement un tableau JSON, mais une donnée
// legacy/saisie à la main (`"a,b"`) ou corrompue ne doit pas faire crasher le
// rendu (un JSON.parse qui throw blanchirait tout le Studio) → fallback tolérant.
function parseMultiChoice(value) {
  if (!value) return [];
  if (Array.isArray(value)) return value;
  if (typeof value !== 'string') return [];
  try {
    const parsed = JSON.parse(value);
    return Array.isArray(parsed) ? parsed : [];
  } catch {
    return value.split(',').map(s => s.trim()).filter(Boolean);
  }
}

function FieldInput({ col, cfg, relation, appSlug, value, onChange }) {
  const baseClass = "w-full bg-gray-900 text-gray-50 text-sm rounded-sm px-3 py-1.5 border border-gray-600 outline-hidden focus:border-blue-500";

  // Boolean → toggle
  if (col.field_type === 'Boolean') {
    return (
      <label className="flex items-center gap-2 cursor-pointer">
        <input
          type="checkbox"
          checked={!!value}
          onChange={e => onChange(e.target.checked)}
          className="w-4 h-4 rounded-sm"
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
    const selected = parseMultiChoice(value);
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
