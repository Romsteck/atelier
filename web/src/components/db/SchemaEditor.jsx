import { useState, useEffect } from 'react';
import {
  getAppDbSchema,
  syncAppDbSchema,
  createAppDbTable,
  dropAppDbTable,
  addAppDbColumn,
  removeAppDbColumn,
  createAppDbRelation,
} from '../../api/client';
import {
  Plus, Trash2, RefreshCw, Database, Columns, Link2, X, Loader2,
  Type, Hash, Calendar, Mail, Globe, Phone, DollarSign, Percent, Clock,
  Braces, Key, List, Search, FunctionSquare, ToggleLeft,
} from 'lucide-react';

function unwrap(res) {
  const d = res.data;
  return d && typeof d === 'object' && 'data' in d ? d.data : d;
}

const FIELD_TYPES = [
  'Text', 'Number', 'Decimal', 'Boolean', 'DateTime', 'Date', 'Time',
  'Email', 'Url', 'Phone', 'Currency', 'Percent', 'Duration',
  'Json', 'Uuid', 'AutoIncrement', 'Choice', 'MultiChoice', 'Lookup', 'Formula',
];

const RELATION_TYPES = ['one_to_many', 'many_to_many', 'self_referential'];
const CASCADE_ACTIONS = ['restrict', 'cascade', 'set_null'];

function FieldTypeIcon({ type, className }) {
  const icons = {
    Text: Type, Number: Hash, Decimal: Hash, AutoIncrement: Hash,
    Boolean: ToggleLeft,
    DateTime: Calendar, Date: Calendar, Time: Clock,
    Email: Mail, Url: Globe, Phone: Phone,
    Currency: DollarSign, Percent: Percent, Duration: Clock,
    Json: Braces, Uuid: Key,
    Choice: List, MultiChoice: List,
    Lookup: Search, Formula: FunctionSquare,
  };
  const Icon = icons[type] || Columns;
  return <Icon className={className} />;
}

function fieldTypeBadge(type) {
  const styles = {
    Text: 'bg-gray-700 text-gray-300',
    Number: 'bg-emerald-500/15 text-emerald-400', Decimal: 'bg-emerald-500/15 text-emerald-400',
    AutoIncrement: 'bg-emerald-500/15 text-emerald-400',
    Boolean: 'bg-amber-500/15 text-amber-400',
    DateTime: 'bg-sky-500/15 text-sky-400', Date: 'bg-sky-500/15 text-sky-400', Time: 'bg-sky-500/15 text-sky-400',
    Email: 'bg-cyan-500/15 text-cyan-400', Url: 'bg-cyan-500/15 text-cyan-400', Phone: 'bg-cyan-500/15 text-cyan-400',
    Currency: 'bg-green-500/15 text-green-400', Percent: 'bg-green-500/15 text-green-400',
    Duration: 'bg-indigo-500/15 text-indigo-400',
    Json: 'bg-orange-500/15 text-orange-400', Uuid: 'bg-gray-500/15 text-gray-400',
    Choice: 'bg-violet-500/15 text-violet-400', MultiChoice: 'bg-violet-500/15 text-violet-400',
    Lookup: 'bg-blue-500/15 text-blue-400',
    Formula: 'bg-purple-500/15 text-purple-400',
  };
  return styles[type] || 'bg-gray-700 text-gray-400';
}

export function SchemaEditor({ appSlug, onSchemaChanged }) {
  const [schema, setSchema] = useState(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState(null);
  const [toast, setToast] = useState(null);

  // Selected table for detail view
  const [selectedTable, setSelectedTable] = useState(null);

  // Modals
  const [showCreateTable, setShowCreateTable] = useState(false);
  const [showAddColumn, setShowAddColumn] = useState(null); // table name
  const [showCreateRelation, setShowCreateRelation] = useState(false);

  function showToast(msg, type = 'ok') {
    setToast({ msg, type });
    setTimeout(() => setToast(null), 3000);
  }

  async function loadSchema() {
    setLoading(true);
    setError(null);
    try {
      const res = await getAppDbSchema(appSlug);
      setSchema(unwrap(res));
    } catch (e) {
      setError(e.message);
    } finally {
      setLoading(false);
    }
  }

  useEffect(() => { loadSchema(); }, [appSlug]);

  async function handleSync() {
    try {
      const res = await syncAppDbSchema(appSlug);
      const data = unwrap(res);
      const msg = `Sync: ${data?.tables_added?.length || 0} tables, ${data?.columns_added?.length || 0} colonnes`;
      showToast(msg);
      await loadSchema();
      onSchemaChanged?.();
    } catch (e) {
      showToast(e.message, 'err');
    }
  }

  async function handleDropTable(table) {
    if (!confirm(`Supprimer la table "${table}" ? Cette action est irreversible.`)) return;
    try {
      await dropAppDbTable(appSlug, table);
      showToast(`Table "${table}" supprimee`);
      if (selectedTable === table) setSelectedTable(null);
      await loadSchema();
      onSchemaChanged?.();
    } catch (e) {
      showToast(e.message, 'err');
    }
  }

  async function handleRemoveColumn(table, column) {
    if (!confirm(`Supprimer la colonne "${column}" de "${table}" ?`)) return;
    try {
      await removeAppDbColumn(appSlug, table, column);
      showToast(`Colonne "${column}" supprimee`);
      await loadSchema();
      onSchemaChanged?.();
    } catch (e) {
      showToast(e.message, 'err');
    }
  }

  if (loading) {
    return (
      <div className="flex items-center justify-center h-full text-gray-500 text-sm">
        <Loader2 className="w-5 h-5 animate-spin mr-2" /> Chargement du schema...
      </div>
    );
  }

  return (
    <div className="flex h-full overflow-hidden">
      {/* Tables list sidebar */}
      <div className="w-56 border-r border-gray-700 flex flex-col bg-gray-800/30 shrink-0">
        <div className="flex items-center gap-1 px-3 py-2 border-b border-gray-700">
          <span className="text-xs font-semibold text-gray-400 uppercase tracking-wider flex-1">Tables</span>
          <button onClick={handleSync} className="p-1 text-gray-500 hover:text-yellow-400 hover:bg-gray-700 rounded border-none bg-transparent cursor-pointer" title="Sync">
            <RefreshCw className="w-3 h-3" />
          </button>
          <button onClick={() => setShowCreateTable(true)} className="p-1 text-gray-500 hover:text-green-400 hover:bg-gray-700 rounded border-none bg-transparent cursor-pointer" title="Nouvelle table">
            <Plus className="w-3 h-3" />
          </button>
        </div>
        <div className="flex-1 overflow-y-auto">
          {schema?.tables?.length > 0 ? (
            schema.tables.map(table => {
              const isSelected = selectedTable === table.name;
              return (
                <button
                  key={table.name}
                  onClick={() => setSelectedTable(isSelected ? null : table.name)}
                  className={`w-full text-left px-3 py-2 text-sm border-none cursor-pointer flex items-center gap-2 ${
                    isSelected
                      ? 'bg-blue-500/10 text-blue-400'
                      : 'bg-transparent text-gray-400 hover:bg-gray-700/30 hover:text-white'
                  }`}
                >
                  <Database className="w-3.5 h-3.5 shrink-0" />
                  <span className="flex-1 truncate">{table.name}</span>
                  <span className="text-[10px] text-gray-600">{table.columns.length}</span>
                </button>
              );
            })
          ) : (
            <div className="px-3 py-4 text-xs text-gray-500">Aucune table</div>
          )}
        </div>
      </div>

      {/* Column detail panel */}
      <div className="flex-1 min-w-0 flex flex-col">
        {error && (
          <div className="px-4 py-2 text-xs bg-red-500/10 text-red-400 border-b border-red-500/20">{error}</div>
        )}

        {selectedTable && schema?.tables ? (() => {
          const table = schema.tables.find(t => t.name === selectedTable);
          if (!table) return null;
          const tableRelations = schema.relations?.filter(
            r => r.from_table === table.name || r.to_table === table.name
          ) || [];
          return (
            <>
              {/* Table header */}
              <div className="flex items-center gap-2 px-4 py-2 border-b border-gray-700 shrink-0 bg-gray-800/50">
                <Database className="w-4 h-4 text-blue-400" />
                <span className="text-sm font-medium text-white">{table.name}</span>
                <span className="text-xs text-gray-500">{table.columns.length} colonnes</span>
                <div className="flex-1" />
                <button
                  onClick={() => setShowAddColumn(table.name)}
                  className="flex items-center gap-1 px-2 py-1 text-[11px] text-green-400 hover:bg-green-500/10 rounded border-none bg-transparent cursor-pointer"
                >
                  <Plus className="w-3 h-3" /> Colonne
                </button>
                <button
                  onClick={() => setShowCreateRelation(true)}
                  className="flex items-center gap-1 px-2 py-1 text-[11px] text-purple-400 hover:bg-purple-500/10 rounded border-none bg-transparent cursor-pointer"
                >
                  <Link2 className="w-3 h-3" /> Relation
                </button>
                <button
                  onClick={() => handleDropTable(table.name)}
                  className="flex items-center gap-1 px-2 py-1 text-[11px] text-red-400 hover:bg-red-500/10 rounded border-none bg-transparent cursor-pointer"
                >
                  <Trash2 className="w-3 h-3" />
                </button>
              </div>

              {/* Columns table */}
              <div className="flex-1 overflow-y-auto">
                <table className="w-full text-sm border-collapse">
                  <thead>
                    <tr className="text-left text-[11px] text-gray-500 uppercase tracking-wider bg-gray-800/50">
                      <th className="px-3 py-1.5 font-medium border-b border-gray-700" style={{width:24}}></th>
                      <th className="px-3 py-1.5 font-medium border-b border-gray-700">Nom</th>
                      <th className="px-3 py-1.5 font-medium border-b border-gray-700">Type</th>
                      <th className="px-3 py-1.5 font-medium border-b border-gray-700">Contraintes</th>
                      <th className="px-3 py-1.5 font-medium border-b border-gray-700">Details</th>
                      <th className="px-3 py-1.5 font-medium border-b border-gray-700" style={{width:32}}></th>
                    </tr>
                  </thead>
                  <tbody>
                    {table.columns.map(col => {
                      const isSystem = ['id', 'created_at', 'updated_at'].includes(col.name);
                      const extra = col.choices?.length > 0 ? col.choices.join(', ')
                        : col.formula_expression ? `= ${col.formula_expression}`
                        : col.default_value != null ? `defaut: ${col.default_value}`
                        : col.lookup_config ? `→ ${col.lookup_config.table}.${col.lookup_config.column}`
                        : '';
                      return (
                        <tr key={col.name} className={`group border-b border-gray-700/50 ${isSystem ? 'text-gray-500' : 'text-gray-300 hover:bg-gray-700/20'}`}>
                          <td className="px-3 py-1.5">
                            <FieldTypeIcon type={col.field_type} className={`w-3.5 h-3.5 ${isSystem ? 'text-gray-600' : 'text-gray-400'}`} />
                          </td>
                          <td className="px-3 py-1.5 font-mono">{col.name}</td>
                          <td className="px-3 py-1.5">
                            <span className={`px-1.5 py-0.5 rounded text-[11px] font-medium ${fieldTypeBadge(col.field_type)}`}>
                              {col.field_type}
                            </span>
                          </td>
                          <td className="px-3 py-1.5 text-xs">
                            {col.required && <span className="text-red-400 mr-2">requis</span>}
                            {col.unique && <span className="text-yellow-400">unique</span>}
                          </td>
                          <td className="px-3 py-1.5 text-xs text-gray-500 truncate max-w-[200px]">{extra}</td>
                          <td className="px-3 py-1.5">
                            {!isSystem && (
                              <button
                                onClick={() => handleRemoveColumn(table.name, col.name)}
                                className="p-0.5 text-gray-600 hover:text-red-400 border-none bg-transparent cursor-pointer opacity-0 group-hover:opacity-100"
                              >
                                <X className="w-3.5 h-3.5" />
                              </button>
                            )}
                          </td>
                        </tr>
                      );
                    })}
                  </tbody>
                </table>

                {/* Relations for this table */}
                {tableRelations.length > 0 && (
                  <table className="w-full text-xs border-collapse mt-0">
                    <thead>
                      <tr className="text-left text-[11px] text-gray-500 uppercase tracking-wider bg-gray-800/50">
                        <th className="px-3 py-1.5 font-medium border-b border-t border-gray-700" colSpan={5}>Relations</th>
                      </tr>
                    </thead>
                    <tbody>
                      {tableRelations.map((rel, i) => (
                        <tr key={i} className="border-b border-gray-700/50 text-gray-400">
                          <td className="px-3 py-1.5"><Link2 className="w-3.5 h-3.5 text-purple-400" /></td>
                          <td className="px-3 py-1.5 font-mono text-purple-300">{rel.from_table}.{rel.from_column}</td>
                          <td className="px-3 py-1.5 text-gray-600">→</td>
                          <td className="px-3 py-1.5 font-mono text-purple-300">{rel.to_table}.{rel.to_column}</td>
                          <td className="px-3 py-1.5 text-gray-500">{rel.relation_type}</td>
                        </tr>
                      ))}
                    </tbody>
                  </table>
                )}
              </div>
            </>
          );
        })() : (
          <div className="flex flex-col items-center justify-center h-full text-gray-500">
            <Columns className="w-10 h-10 mb-3 opacity-20" />
            <p className="text-sm">Selectionnez une table</p>
          </div>
        )}
      </div>

      {/* Toast */}
      {toast && (
        <div className={`fixed bottom-4 right-4 z-50 px-4 py-2 rounded-lg text-sm shadow-lg ${
          toast.type === 'ok' ? 'bg-green-500/90 text-white' : 'bg-red-500/90 text-white'
        }`}>
          {toast.msg}
        </div>
      )}

      {/* Create Table Modal */}
      {showCreateTable && (
        <CreateTableModal
          appSlug={appSlug}
          onCreated={() => { setShowCreateTable(false); loadSchema(); onSchemaChanged?.(); }}
          onClose={() => setShowCreateTable(false)}
          showToast={showToast}
        />
      )}

      {/* Add Column Modal */}
      {showAddColumn && (
        <AddColumnModal
          appSlug={appSlug}
          table={showAddColumn}
          onAdded={() => { setShowAddColumn(null); loadSchema(); onSchemaChanged?.(); }}
          onClose={() => setShowAddColumn(null)}
          showToast={showToast}
        />
      )}

      {/* Create Relation Modal */}
      {showCreateRelation && schema && (
        <CreateRelationModal
          appSlug={appSlug}
          tables={schema.tables}
          onCreated={() => { setShowCreateRelation(false); loadSchema(); onSchemaChanged?.(); }}
          onClose={() => setShowCreateRelation(false)}
          showToast={showToast}
        />
      )}
    </div>
  );
}

// ── Create Table Modal ──

function CreateTableModal({ appSlug, onCreated, onClose, showToast }) {
  const [name, setName] = useState('');
  const [description, setDescription] = useState('');
  const [columns, setColumns] = useState([{ name: '', field_type: 'Text', required: false, unique: false, choices: '', formula_expression: '' }]);
  const [saving, setSaving] = useState(false);

  function addColumn() {
    setColumns(prev => [...prev, { name: '', field_type: 'Text', required: false, unique: false, choices: '', formula_expression: '' }]);
  }

  function updateColumn(idx, field, value) {
    setColumns(prev => prev.map((c, i) => i === idx ? { ...c, [field]: value } : c));
  }

  function removeColumn(idx) {
    setColumns(prev => prev.filter((_, i) => i !== idx));
  }

  async function handleSubmit(e) {
    e.preventDefault();
    if (!name.trim()) return;
    setSaving(true);
    try {
      const cols = columns
        .filter(c => c.name.trim())
        .map(c => {
          const col = {
            name: c.name.trim(),
            field_type: c.field_type.toLowerCase(),
            required: c.required,
            unique: c.unique,
          };
          if ((c.field_type === 'Choice' || c.field_type === 'MultiChoice') && c.choices.trim()) {
            col.choices = c.choices.split(',').map(s => s.trim()).filter(Boolean);
          }
          if (c.field_type === 'Formula' && c.formula_expression.trim()) {
            col.formula_expression = c.formula_expression.trim();
          }
          return col;
        });

      const now = new Date().toISOString();
      await createAppDbTable(appSlug, {
        name: name.trim(),
        slug: name.trim().toLowerCase().replace(/[^a-z0-9_]/g, '_'),
        columns: cols,
        description: description.trim() || null,
        created_at: now,
        updated_at: now,
      });
      showToast(`Table "${name}" creee`);
      onCreated();
    } catch (err) {
      showToast(err.response?.data?.error || err.message, 'err');
    } finally {
      setSaving(false);
    }
  }

  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/50">
      <div className="bg-gray-800 rounded-lg border border-gray-700 shadow-xl w-full max-w-lg max-h-[85vh] flex flex-col">
        <div className="flex items-center justify-between px-4 py-3 border-b border-gray-700">
          <h3 className="text-sm font-semibold text-white flex items-center gap-2">
            <Database className="w-4 h-4 text-blue-400" /> Nouvelle table
          </h3>
          <button onClick={onClose} className="p-1 text-gray-400 hover:text-white rounded hover:bg-gray-700 border-none bg-transparent cursor-pointer">
            <X className="w-4 h-4" />
          </button>
        </div>
        <form onSubmit={handleSubmit} className="flex-1 overflow-y-auto p-4 space-y-3">
          <div>
            <label className="block text-xs text-gray-400 mb-1">Nom de la table <span className="text-red-400">*</span></label>
            <input type="text" value={name} onChange={e => setName(e.target.value)} required
              className="w-full bg-gray-900 text-white text-sm rounded px-3 py-1.5 border border-gray-600 outline-none focus:border-blue-500"
              placeholder="ex: products" />
          </div>
          <div>
            <label className="block text-xs text-gray-400 mb-1">Description</label>
            <input type="text" value={description} onChange={e => setDescription(e.target.value)}
              className="w-full bg-gray-900 text-white text-sm rounded px-3 py-1.5 border border-gray-600 outline-none focus:border-blue-500"
              placeholder="Optionnel" />
          </div>

          <div>
            <div className="flex items-center justify-between mb-2">
              <label className="text-xs text-gray-400">Colonnes</label>
              <button type="button" onClick={addColumn} className="text-xs text-blue-400 hover:text-blue-300 flex items-center gap-1 border-none bg-transparent cursor-pointer">
                <Plus className="w-3 h-3" /> Ajouter
              </button>
            </div>
            <div className="space-y-2">
              {columns.map((col, idx) => (
                <div key={idx} className="flex items-start gap-2 bg-gray-900/50 rounded p-2">
                  <div className="flex-1 space-y-1">
                    <input type="text" value={col.name} onChange={e => updateColumn(idx, 'name', e.target.value)}
                      className="w-full bg-gray-900 text-white text-xs rounded px-2 py-1 border border-gray-700 outline-none"
                      placeholder="nom_colonne" />
                    <div className="flex gap-2">
                      <select value={col.field_type} onChange={e => updateColumn(idx, 'field_type', e.target.value)}
                        className="bg-gray-900 text-white text-xs rounded px-2 py-1 border border-gray-700 flex-1">
                        {FIELD_TYPES.map(t => <option key={t} value={t}>{t}</option>)}
                      </select>
                      <label className="flex items-center gap-1 text-[10px] text-gray-400">
                        <input type="checkbox" checked={col.required} onChange={e => updateColumn(idx, 'required', e.target.checked)} className="w-3 h-3" />
                        req
                      </label>
                      <label className="flex items-center gap-1 text-[10px] text-gray-400">
                        <input type="checkbox" checked={col.unique} onChange={e => updateColumn(idx, 'unique', e.target.checked)} className="w-3 h-3" />
                        uniq
                      </label>
                    </div>
                    {(col.field_type === 'Choice' || col.field_type === 'MultiChoice') && (
                      <input type="text" value={col.choices} onChange={e => updateColumn(idx, 'choices', e.target.value)}
                        className="w-full bg-gray-900 text-white text-xs rounded px-2 py-1 border border-gray-700 outline-none"
                        placeholder="choix1, choix2, choix3" />
                    )}
                    {col.field_type === 'Formula' && (
                      <input type="text" value={col.formula_expression} onChange={e => updateColumn(idx, 'formula_expression', e.target.value)}
                        className="w-full bg-gray-900 text-white text-xs rounded px-2 py-1 border border-gray-700 outline-none font-mono"
                        placeholder="price * quantity" />
                    )}
                  </div>
                  <button type="button" onClick={() => removeColumn(idx)} className="p-1 text-gray-600 hover:text-red-400 border-none bg-transparent cursor-pointer mt-1">
                    <X className="w-3 h-3" />
                  </button>
                </div>
              ))}
            </div>
          </div>
        </form>
        <div className="flex justify-end gap-2 px-4 py-3 border-t border-gray-700">
          <button type="button" onClick={onClose} className="px-3 py-1.5 text-xs text-gray-400 rounded border-none bg-transparent cursor-pointer hover:text-white">Annuler</button>
          <button onClick={handleSubmit} disabled={saving} className="px-4 py-1.5 text-xs text-white bg-blue-500 rounded border-none cursor-pointer hover:bg-blue-600 disabled:opacity-50 flex items-center gap-1">
            {saving ? <Loader2 className="w-3 h-3 animate-spin" /> : <Plus className="w-3 h-3" />}
            Creer
          </button>
        </div>
      </div>
    </div>
  );
}

// ── Add Column Modal ──

function AddColumnModal({ appSlug, table, onAdded, onClose, showToast }) {
  const [name, setName] = useState('');
  const [fieldType, setFieldType] = useState('Text');
  const [required, setRequired] = useState(false);
  const [unique, setUnique] = useState(false);
  const [choices, setChoices] = useState('');
  const [defaultValue, setDefaultValue] = useState('');
  const [saving, setSaving] = useState(false);

  async function handleSubmit(e) {
    e.preventDefault();
    if (!name.trim()) return;
    setSaving(true);
    try {
      const body = {
        name: name.trim(),
        field_type: fieldType.toLowerCase(),
        required,
        unique,
      };
      if (defaultValue.trim()) body.default_value = defaultValue.trim();
      if ((fieldType === 'Choice' || fieldType === 'MultiChoice') && choices.trim()) {
        body.choices = choices.split(',').map(s => s.trim()).filter(Boolean);
      }
      await addAppDbColumn(appSlug, table, body);
      showToast(`Colonne "${name}" ajoutee a "${table}"`);
      onAdded();
    } catch (err) {
      showToast(err.response?.data?.error || err.message, 'err');
    } finally {
      setSaving(false);
    }
  }

  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/50">
      <div className="bg-gray-800 rounded-lg border border-gray-700 shadow-xl w-full max-w-sm">
        <div className="flex items-center justify-between px-4 py-3 border-b border-gray-700">
          <h3 className="text-sm font-semibold text-white">Ajouter colonne a "{table}"</h3>
          <button onClick={onClose} className="p-1 text-gray-400 hover:text-white rounded hover:bg-gray-700 border-none bg-transparent cursor-pointer"><X className="w-4 h-4" /></button>
        </div>
        <form onSubmit={handleSubmit} className="p-4 space-y-3">
          <div>
            <label className="block text-xs text-gray-400 mb-1">Nom</label>
            <input type="text" value={name} onChange={e => setName(e.target.value)} required
              className="w-full bg-gray-900 text-white text-sm rounded px-3 py-1.5 border border-gray-600 outline-none focus:border-blue-500" />
          </div>
          <div>
            <label className="block text-xs text-gray-400 mb-1">Type</label>
            <select value={fieldType} onChange={e => setFieldType(e.target.value)}
              className="w-full bg-gray-900 text-white text-sm rounded px-3 py-1.5 border border-gray-600">
              {FIELD_TYPES.filter(t => t !== 'Formula').map(t => <option key={t} value={t}>{t}</option>)}
            </select>
          </div>
          <div className="flex gap-4">
            <label className="flex items-center gap-1 text-xs text-gray-400">
              <input type="checkbox" checked={required} onChange={e => setRequired(e.target.checked)} /> Requis
            </label>
            <label className="flex items-center gap-1 text-xs text-gray-400">
              <input type="checkbox" checked={unique} onChange={e => setUnique(e.target.checked)} /> Unique
            </label>
          </div>
          {required && (
            <div>
              <label className="block text-xs text-gray-400 mb-1">Valeur par defaut (requis pour NOT NULL)</label>
              <input type="text" value={defaultValue} onChange={e => setDefaultValue(e.target.value)}
                className="w-full bg-gray-900 text-white text-sm rounded px-3 py-1.5 border border-gray-600 outline-none" />
            </div>
          )}
          {(fieldType === 'Choice' || fieldType === 'MultiChoice') && (
            <div>
              <label className="block text-xs text-gray-400 mb-1">Choix (separes par virgule)</label>
              <input type="text" value={choices} onChange={e => setChoices(e.target.value)}
                className="w-full bg-gray-900 text-white text-sm rounded px-3 py-1.5 border border-gray-600 outline-none" />
            </div>
          )}
        </form>
        <div className="flex justify-end gap-2 px-4 py-3 border-t border-gray-700">
          <button type="button" onClick={onClose} className="px-3 py-1.5 text-xs text-gray-400 rounded border-none bg-transparent cursor-pointer hover:text-white">Annuler</button>
          <button onClick={handleSubmit} disabled={saving} className="px-4 py-1.5 text-xs text-white bg-blue-500 rounded border-none cursor-pointer hover:bg-blue-600 disabled:opacity-50">
            {saving ? <Loader2 className="w-3 h-3 animate-spin" /> : 'Ajouter'}
          </button>
        </div>
      </div>
    </div>
  );
}

// ── Create Relation Modal ──

function CreateRelationModal({ appSlug, tables, onCreated, onClose, showToast }) {
  const [fromTable, setFromTable] = useState(tables[0]?.name || '');
  const [fromColumn, setFromColumn] = useState('');
  const [toTable, setToTable] = useState(tables[0]?.name || '');
  const [toColumn, setToColumn] = useState('id');
  const [relationType, setRelationType] = useState('one_to_many');
  const [onDelete, setOnDelete] = useState('restrict');
  const [onUpdate, setOnUpdate] = useState('cascade');
  const [saving, setSaving] = useState(false);

  const fromCols = tables.find(t => t.name === fromTable)?.columns || [];
  const toCols = tables.find(t => t.name === toTable)?.columns || [];

  async function handleSubmit(e) {
    e.preventDefault();
    if (!fromTable || !fromColumn || !toTable || !toColumn) return;
    setSaving(true);
    try {
      await createAppDbRelation(appSlug, {
        from_table: fromTable,
        from_column: fromColumn,
        to_table: toTable,
        to_column: toColumn,
        relation_type: relationType,
        cascade: { on_delete: onDelete, on_update: onUpdate },
      });
      showToast(`Relation ${fromTable}.${fromColumn} → ${toTable}.${toColumn} creee`);
      onCreated();
    } catch (err) {
      showToast(err.response?.data?.error || err.message, 'err');
    } finally {
      setSaving(false);
    }
  }

  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/50">
      <div className="bg-gray-800 rounded-lg border border-gray-700 shadow-xl w-full max-w-sm">
        <div className="flex items-center justify-between px-4 py-3 border-b border-gray-700">
          <h3 className="text-sm font-semibold text-white flex items-center gap-2">
            <Link2 className="w-4 h-4 text-purple-400" /> Nouvelle relation
          </h3>
          <button onClick={onClose} className="p-1 text-gray-400 hover:text-white rounded hover:bg-gray-700 border-none bg-transparent cursor-pointer"><X className="w-4 h-4" /></button>
        </div>
        <form onSubmit={handleSubmit} className="p-4 space-y-3">
          <div className="grid grid-cols-2 gap-2">
            <div>
              <label className="block text-xs text-gray-400 mb-1">Table source</label>
              <select value={fromTable} onChange={e => setFromTable(e.target.value)}
                className="w-full bg-gray-900 text-white text-xs rounded px-2 py-1.5 border border-gray-600">
                {tables.map(t => <option key={t.name} value={t.name}>{t.name}</option>)}
              </select>
            </div>
            <div>
              <label className="block text-xs text-gray-400 mb-1">Colonne source</label>
              <select value={fromColumn} onChange={e => setFromColumn(e.target.value)}
                className="w-full bg-gray-900 text-white text-xs rounded px-2 py-1.5 border border-gray-600">
                <option value="">--</option>
                {fromCols.map(c => <option key={c.name} value={c.name}>{c.name}</option>)}
              </select>
            </div>
            <div>
              <label className="block text-xs text-gray-400 mb-1">Table cible</label>
              <select value={toTable} onChange={e => setToTable(e.target.value)}
                className="w-full bg-gray-900 text-white text-xs rounded px-2 py-1.5 border border-gray-600">
                {tables.map(t => <option key={t.name} value={t.name}>{t.name}</option>)}
              </select>
            </div>
            <div>
              <label className="block text-xs text-gray-400 mb-1">Colonne cible</label>
              <select value={toColumn} onChange={e => setToColumn(e.target.value)}
                className="w-full bg-gray-900 text-white text-xs rounded px-2 py-1.5 border border-gray-600">
                {toCols.map(c => <option key={c.name} value={c.name}>{c.name}</option>)}
              </select>
            </div>
          </div>
          <div>
            <label className="block text-xs text-gray-400 mb-1">Type</label>
            <select value={relationType} onChange={e => setRelationType(e.target.value)}
              className="w-full bg-gray-900 text-white text-xs rounded px-2 py-1.5 border border-gray-600">
              {RELATION_TYPES.map(t => <option key={t} value={t}>{t}</option>)}
            </select>
          </div>
          <div className="grid grid-cols-2 gap-2">
            <div>
              <label className="block text-xs text-gray-400 mb-1">On delete</label>
              <select value={onDelete} onChange={e => setOnDelete(e.target.value)}
                className="w-full bg-gray-900 text-white text-xs rounded px-2 py-1.5 border border-gray-600">
                {CASCADE_ACTIONS.map(a => <option key={a} value={a}>{a}</option>)}
              </select>
            </div>
            <div>
              <label className="block text-xs text-gray-400 mb-1">On update</label>
              <select value={onUpdate} onChange={e => setOnUpdate(e.target.value)}
                className="w-full bg-gray-900 text-white text-xs rounded px-2 py-1.5 border border-gray-600">
                {CASCADE_ACTIONS.map(a => <option key={a} value={a}>{a}</option>)}
              </select>
            </div>
          </div>
        </form>
        <div className="flex justify-end gap-2 px-4 py-3 border-t border-gray-700">
          <button type="button" onClick={onClose} className="px-3 py-1.5 text-xs text-gray-400 rounded border-none bg-transparent cursor-pointer hover:text-white">Annuler</button>
          <button onClick={handleSubmit} disabled={saving} className="px-4 py-1.5 text-xs text-white bg-purple-500 rounded border-none cursor-pointer hover:bg-purple-600 disabled:opacity-50">
            {saving ? <Loader2 className="w-3 h-3 animate-spin" /> : 'Creer'}
          </button>
        </div>
      </div>
    </div>
  );
}
