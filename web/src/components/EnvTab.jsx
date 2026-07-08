import { useState, useEffect, useCallback } from 'react';
import {
  Lock, Eye, EyeOff, Plus, Trash2, Pencil, Save, Loader2, X, Check,
  RefreshCw, KeyRound,
} from 'lucide-react';
import { getAppEnv, getAppEnvVar, setAppEnvVar, deleteAppEnvVar } from '../api/client';
import Button from './Button';

// Per-variable env editor — line by line (name | value), ownership-aware.
// Platform vars (PORT / HR_DV_* / ATELIER_*) are computed and locked; user
// vars (config + secrets) are editable. Secrets are masked until revealed
// per-row. Same model for both stacks (Node + Rust) — `scope` decides
// runtime vs build-time injection.

const ENV_KEY_RE = /^[A-Za-z_][A-Za-z0-9_]*$/;
const SCOPES = [
  { value: 'runtime', label: 'runtime' },
  { value: 'build', label: 'build' },
  { value: 'both', label: 'build+runtime' },
];

function ScopeBadge({ scope }) {
  const color = scope === 'build' ? 'bg-purple-500/15 text-purple-700 dark:text-purple-300'
    : scope === 'both' ? 'bg-amber-500/15 text-amber-700 dark:text-amber-300'
    : 'bg-gray-500/15 text-gray-400';
  return <span className={`px-1.5 py-0.5 rounded text-[10px] font-medium ${color}`}>{scope}</span>;
}

function OwnerBadge({ owner, secret }) {
  if (owner === 'platform') {
    return <span className="px-1.5 py-0.5 rounded text-[10px] font-medium bg-blue-500/15 text-blue-700 dark:text-blue-300">plateforme</span>;
  }
  return (
    <span className={`px-1.5 py-0.5 rounded text-[10px] font-medium ${secret ? 'bg-red-500/15 text-red-700 dark:text-red-300' : 'bg-green-500/15 text-green-700 dark:text-green-300'}`}>
      {secret ? 'secret' : 'config'}
    </span>
  );
}

export default function EnvTab({ slug, onRestart }) {
  const [vars, setVars] = useState([]);
  const [loading, setLoading] = useState(true);
  const [err, setErr] = useState('');
  const [restartNeeded, setRestartNeeded] = useState(false);
  const [restarting, setRestarting] = useState(false);

  // key -> revealed plaintext (lazy fetch for secrets)
  const [revealed, setRevealed] = useState({});
  // key currently being edited -> { value, secret, scope, saving }
  const [editing, setEditing] = useState(null);
  // add-form state
  const [adding, setAdding] = useState(false);
  const [form, setForm] = useState({ key: '', value: '', secret: false, scope: 'runtime', saving: false });

  const load = useCallback(() => {
    setLoading(true);
    setErr('');
    getAppEnv(slug)
      .then(res => setVars(res.data?.data || []))
      .catch(e => setErr(e?.response?.data?.error || 'Chargement impossible'))
      .finally(() => setLoading(false));
  }, [slug]);

  useEffect(() => {
    setRevealed({});
    setEditing(null);
    setAdding(false);
    setRestartNeeded(false);
    load();
  }, [slug, load]);

  const reveal = async (key) => {
    if (revealed[key] !== undefined) {
      setRevealed(prev => { const n = { ...prev }; delete n[key]; return n; }); // toggle off
      return;
    }
    try {
      const res = await getAppEnvVar(slug, key);
      setRevealed(prev => ({ ...prev, [key]: res.data?.data?.value ?? '' }));
    } catch (e) {
      setErr(e?.response?.data?.error || `Révélation de ${key} impossible`);
    }
  };

  const startEdit = async (row) => {
    let value = row.value ?? '';
    if (row.secret) {
      try {
        const res = await getAppEnvVar(slug, row.key);
        value = res.data?.data?.value ?? '';
      } catch (e) {
        setErr(e?.response?.data?.error || `Lecture de ${row.key} impossible`);
        return;
      }
    }
    setEditing({ key: row.key, value, secret: row.secret, scope: row.scope, saving: false });
  };

  const saveEdit = async () => {
    if (!editing) return;
    setEditing(e => ({ ...e, saving: true }));
    try {
      await setAppEnvVar(slug, editing.key, { value: editing.value, secret: editing.secret, scope: editing.scope });
      setEditing(null);
      setRestartNeeded(true);
      load();
    } catch (e) {
      setErr(e?.response?.data?.error || 'Enregistrement échoué');
      setEditing(ed => ({ ...ed, saving: false }));
    }
  };

  const removeVar = async (key) => {
    if (!confirm(`Supprimer la variable ${key} ?`)) return;
    try {
      await deleteAppEnvVar(slug, key);
      setRestartNeeded(true);
      load();
    } catch (e) {
      setErr(e?.response?.data?.error || 'Suppression échouée');
    }
  };

  const addVar = async () => {
    const key = form.key.trim();
    if (!ENV_KEY_RE.test(key)) { setErr('Nom invalide (^[A-Za-z_][A-Za-z0-9_]*$)'); return; }
    setForm(f => ({ ...f, saving: true }));
    try {
      await setAppEnvVar(slug, key, { value: form.value, secret: form.secret, scope: form.scope });
      setForm({ key: '', value: '', secret: false, scope: 'runtime', saving: false });
      setAdding(false);
      setRestartNeeded(true);
      load();
    } catch (e) {
      setErr(e?.response?.data?.error || 'Ajout échoué');
      setForm(f => ({ ...f, saving: false }));
    }
  };

  const doRestart = async () => {
    if (!onRestart) return;
    setRestarting(true);
    try { await onRestart(); setRestartNeeded(false); } catch { /* surfaced elsewhere */ }
    finally { setRestarting(false); }
  };

  const platformVars = vars.filter(v => v.owner === 'platform');
  const userVars = vars.filter(v => v.owner !== 'platform');

  return (
    <div className="p-6 overflow-y-auto h-full">
      <div className="max-w-3xl space-y-4">
        <div className="flex items-center justify-between">
          <div className="flex items-center gap-2">
            <KeyRound className="w-4 h-4 text-gray-400" />
            <h3 className="text-sm font-medium text-gray-50">Variables d&apos;environnement</h3>
          </div>
          <Button onClick={load} icon={RefreshCw} variant="ghost" size="sm">Rafraîchir</Button>
        </div>

        {restartNeeded && (
          <div className="flex items-center justify-between gap-3 px-3 py-2 rounded-sm bg-amber-500/10 border border-amber-500/30 text-amber-700 dark:text-amber-200 text-xs">
            <span>Les changements s&apos;appliquent au prochain démarrage du process.</span>
            {onRestart && (
              <Button onClick={doRestart} disabled={restarting} loading={restarting} icon={RefreshCw} variant="warning" size="sm">Redémarrer</Button>
            )}
          </div>
        )}

        {err && (
          <div className="flex items-center justify-between gap-3 px-3 py-2 rounded-sm bg-red-500/10 border border-red-500/30 text-red-700 dark:text-red-200 text-xs">
            <span>{err}</span>
            <button onClick={() => setErr('')}><X className="w-3.5 h-3.5" /></button>
          </div>
        )}

        {loading ? (
          <div className="flex items-center justify-center py-10 text-gray-500"><Loader2 className="w-5 h-5 animate-spin" /></div>
        ) : (
          <div className="rounded-sm border border-gray-700 overflow-hidden">
            <table className="w-full text-sm">
              <thead>
                <tr className="bg-gray-800/60 text-gray-400 text-[11px] uppercase tracking-wide">
                  <th className="text-left font-medium px-3 py-2 w-[34%]">Nom</th>
                  <th className="text-left font-medium px-3 py-2">Valeur</th>
                  <th className="text-right font-medium px-3 py-2 w-[1%]"></th>
                </tr>
              </thead>
              <tbody className="divide-y divide-gray-800">
                {/* Platform tier — locked */}
                {platformVars.map(row => (
                  <tr key={row.key} className="bg-gray-900/40">
                    <td className="px-3 py-2 align-top">
                      <div className="flex items-center gap-2">
                        <Lock className="w-3 h-3 text-gray-500 shrink-0" />
                        <span className="font-mono text-gray-300 break-all">{row.key}</span>
                      </div>
                      <div className="mt-1 flex items-center gap-1 pl-5"><OwnerBadge owner={row.owner} secret={row.secret} /><ScopeBadge scope={row.scope} /></div>
                    </td>
                    <td className="px-3 py-2 align-top font-mono text-gray-400 break-all">
                      {row.secret
                        ? (revealed[row.key] !== undefined ? revealed[row.key] : '••••••••')
                        : (row.value ?? '')}
                    </td>
                    <td className="px-3 py-2 align-top text-right">
                      {row.secret && (
                        <button onClick={() => reveal(row.key)} className="text-gray-500 hover:text-gray-200" title="Afficher / masquer">
                          {revealed[row.key] !== undefined ? <EyeOff className="w-4 h-4" /> : <Eye className="w-4 h-4" />}
                        </button>
                      )}
                    </td>
                  </tr>
                ))}

                {/* User tier — editable */}
                {userVars.map(row => {
                  const isEditing = editing?.key === row.key;
                  return (
                    <tr key={row.key} className="hover:bg-gray-800/30">
                      <td className="px-3 py-2 align-top">
                        <span className="font-mono text-gray-100 break-all">{row.key}</span>
                        <div className="mt-1 flex items-center gap-1"><OwnerBadge owner={row.owner} secret={row.secret} /><ScopeBadge scope={isEditing ? editing.scope : row.scope} /></div>
                      </td>
                      <td className="px-3 py-2 align-top">
                        {isEditing ? (
                          <div className="space-y-2">
                            <input
                              type={editing.secret ? 'text' : 'text'}
                              value={editing.value}
                              onChange={e => setEditing(ed => ({ ...ed, value: e.target.value }))}
                              className="w-full px-2 py-1 font-mono text-xs bg-gray-950 border border-gray-700 text-gray-50 rounded-sm outline-hidden focus:border-blue-500"
                              autoFocus
                            />
                            <div className="flex items-center gap-3 text-xs text-gray-400">
                              <label className="flex items-center gap-1.5 cursor-pointer">
                                <input type="checkbox" checked={editing.secret} onChange={e => setEditing(ed => ({ ...ed, secret: e.target.checked }))} /> secret
                              </label>
                              <select value={editing.scope} onChange={e => setEditing(ed => ({ ...ed, scope: e.target.value }))}
                                className="bg-gray-950 border border-gray-700 rounded-sm px-1.5 py-0.5 text-gray-200 outline-hidden">
                                {SCOPES.map(s => <option key={s.value} value={s.value}>{s.label}</option>)}
                              </select>
                            </div>
                          </div>
                        ) : (
                          <span className="font-mono text-gray-300 break-all">
                            {row.secret
                              ? (revealed[row.key] !== undefined ? revealed[row.key] : '••••••••')
                              : (row.value ?? '')}
                          </span>
                        )}
                      </td>
                      <td className="px-3 py-2 align-top text-right whitespace-nowrap">
                        {isEditing ? (
                          <div className="flex items-center gap-2 justify-end">
                            <button onClick={saveEdit} disabled={editing.saving} className="text-green-600 dark:text-green-400 hover:text-green-700 dark:hover:text-green-300 disabled:opacity-50" title="Enregistrer">
                              {editing.saving ? <Loader2 className="w-4 h-4 animate-spin" /> : <Check className="w-4 h-4" />}
                            </button>
                            <button onClick={() => setEditing(null)} className="text-gray-500 hover:text-gray-300" title="Annuler"><X className="w-4 h-4" /></button>
                          </div>
                        ) : (
                          <div className="flex items-center gap-2 justify-end">
                            {row.secret && (
                              <button onClick={() => reveal(row.key)} className="text-gray-500 hover:text-gray-200" title="Afficher / masquer">
                                {revealed[row.key] !== undefined ? <EyeOff className="w-4 h-4" /> : <Eye className="w-4 h-4" />}
                              </button>
                            )}
                            <button onClick={() => startEdit(row)} className="text-gray-500 hover:text-blue-600 dark:hover:text-blue-300" title="Modifier"><Pencil className="w-4 h-4" /></button>
                            <button onClick={() => removeVar(row.key)} className="text-gray-500 hover:text-red-400" title="Supprimer"><Trash2 className="w-4 h-4" /></button>
                          </div>
                        )}
                      </td>
                    </tr>
                  );
                })}

                {userVars.length === 0 && (
                  <tr><td colSpan={3} className="px-3 py-3 text-xs text-gray-500 italic">Aucune variable applicative. Les variables plateforme ci-dessus sont gérées automatiquement.</td></tr>
                )}
              </tbody>
            </table>

            {/* Add row */}
            <div className="border-t border-gray-700 bg-gray-900/40 px-3 py-2">
              {adding ? (
                <div className="space-y-2">
                  <div className="flex flex-wrap items-center gap-2">
                    <input
                      placeholder="NOM"
                      value={form.key}
                      onChange={e => setForm(f => ({ ...f, key: e.target.value }))}
                      className="px-2 py-1 font-mono text-xs bg-gray-950 border border-gray-700 text-gray-50 rounded-sm outline-hidden focus:border-blue-500 w-44"
                    />
                    <input
                      placeholder="valeur"
                      value={form.value}
                      onChange={e => setForm(f => ({ ...f, value: e.target.value }))}
                      className="px-2 py-1 font-mono text-xs bg-gray-950 border border-gray-700 text-gray-50 rounded-sm outline-hidden focus:border-blue-500 flex-1 min-w-[10rem]"
                    />
                    <label className="flex items-center gap-1.5 text-xs text-gray-400 cursor-pointer">
                      <input type="checkbox" checked={form.secret} onChange={e => setForm(f => ({ ...f, secret: e.target.checked }))} /> secret
                    </label>
                    <select value={form.scope} onChange={e => setForm(f => ({ ...f, scope: e.target.value }))}
                      className="bg-gray-950 border border-gray-700 rounded-sm px-1.5 py-1 text-xs text-gray-200 outline-hidden">
                      {SCOPES.map(s => <option key={s.value} value={s.value}>{s.label}</option>)}
                    </select>
                  </div>
                  <div className="flex items-center gap-2">
                    <Button onClick={addVar} disabled={form.saving || !form.key.trim()} loading={form.saving} icon={Save} variant="primary" size="sm">Ajouter</Button>
                    <Button onClick={() => { setAdding(false); setForm({ key: '', value: '', secret: false, scope: 'runtime', saving: false }); }} variant="neutral" size="sm">Annuler</Button>
                  </div>
                </div>
              ) : (
                <Button onClick={() => setAdding(true)} icon={Plus} variant="ghost" size="xs">Ajouter une variable</Button>
              )}
            </div>
          </div>
        )}
      </div>
    </div>
  );
}
