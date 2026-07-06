import { useState } from 'react';
import {
  LayoutGrid, Database, ExternalLink, Plus, Play, Square, Loader2, X,
} from 'lucide-react';
import { createApp } from '../api/client';
import { apiErr } from '../utils/apiErr';
import PageHeader from '../components/PageHeader';
import ScrollableTable from '../components/ScrollableTable';
import { useApps } from '../context/AppsContext';
import { openStudio } from '../lib/openStudio';
import { SLUG_RE, slugify, stackLabel, statusDot } from '../lib/appsUi';

// ── Create Modal ──

function CreateAppModal({ onClose, onCreated }) {
  const [name, setName] = useState('');
  const [slug, setSlug] = useState('');
  const [slugManual, setSlugManual] = useState(false);
  const [stack, setStack] = useState('');
  const visibility = 'private';
  const [submitting, setSubmitting] = useState(false);
  const [error, setError] = useState(null);

  async function handleSubmit(e) {
    e.preventDefault();
    if (!name.trim()) { setError('Nom requis'); return; }
    if (!SLUG_RE.test(slug)) { setError('Slug invalide'); return; }
    setSubmitting(true); setError(null);
    try { await createApp({ name: name.trim(), slug, stack: stack.trim(), visibility }); onCreated(); }
    catch (err) { setError(apiErr(err)); }
    finally { setSubmitting(false); }
  }

  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center p-4 bg-black/60" onClick={onClose}>
      <div className="w-full max-w-md bg-gray-800 border border-gray-700 rounded-lg shadow-xl" onClick={e => e.stopPropagation()}>
        <div className="flex items-center justify-between px-5 py-4 border-b border-gray-700">
          <h2 className="text-lg font-semibold text-gray-50">Nouvelle application</h2>
          <button onClick={onClose} className="text-gray-400 hover:text-gray-50"><X className="w-5 h-5" /></button>
        </div>
        <form onSubmit={handleSubmit} className="p-5 space-y-4">
          {error && <div className="bg-red-500/10 border border-red-500/30 rounded-sm px-3 py-2 text-sm text-red-400">{error}</div>}
          <div><label className="block text-xs text-gray-400 mb-1">Nom</label><input type="text" value={name} onChange={e => { setName(e.target.value); if (!slugManual) setSlug(slugify(e.target.value)); }} autoFocus className="w-full px-3 py-2 text-sm bg-gray-900 border border-gray-700 text-gray-50 rounded-sm outline-hidden" /></div>
          <div><label className="block text-xs text-gray-400 mb-1">Slug</label><input type="text" value={slug} onChange={e => { setSlugManual(true); setSlug(slugify(e.target.value)); }} className="w-full px-3 py-2 text-sm bg-gray-900 border border-gray-700 text-gray-50 font-mono rounded-sm outline-hidden" /></div>
          <div><label className="block text-xs text-gray-400 mb-1">Stack <span className="text-gray-500">(label libre, optionnel)</span></label><input type="text" value={stack} onChange={e => setStack(e.target.value)} maxLength={64} placeholder="ex : Vite+Rust, Next.js, Python FastAPI…" className="w-full px-3 py-2 text-sm bg-gray-900 border border-gray-700 text-gray-50 rounded-sm outline-hidden placeholder:text-gray-600" /><p className="mt-1 text-[11px] text-gray-500">L'app naît vide : c'est la première conversation Studio qui génère le projet (n'importe quelle stack) et configure build/run.</p></div>
          <div className="flex justify-end gap-2 pt-3 border-t border-gray-700">
            <button type="button" onClick={onClose} className="px-4 py-2 text-sm text-gray-300 bg-gray-700 rounded-sm">Annuler</button>
            <button type="submit" disabled={submitting} className="px-4 py-2 text-sm text-white bg-blue-500 rounded-sm disabled:opacity-50 flex items-center gap-2">{submitting && <Loader2 className="w-4 h-4 animate-spin" />}Creer</button>
          </div>
        </form>
      </div>
    </div>
  );
}

// ── Apps gallery (homepage landing) ──
// Cliquer une app OUVRE son Studio dans un nouvel onglet focalisé (cf. openStudio).

export default function Apps() {
  const { apps, loading, control, reload } = useApps();
  const [showCreate, setShowCreate] = useState(false);

  return (
    <div className="h-full flex flex-col">
      <PageHeader title="Applications" icon={LayoutGrid}>
        <button
          onClick={() => setShowCreate(true)}
          className="flex items-center gap-1.5 px-3 py-1.5 text-xs font-medium text-white bg-blue-500 hover:bg-blue-600 active:bg-blue-700 rounded-md transition-colors"
        >
          <Plus className="w-3.5 h-3.5" /> Nouvelle application
        </button>
      </PageHeader>

      <div className="flex-1 overflow-y-auto p-5">
        {loading ? (
          <div className="flex items-center justify-center py-20"><Loader2 className="w-8 h-8 animate-spin text-blue-400" /></div>
        ) : (
          <ScrollableTable>
          <table className="w-full min-w-max text-[13px] border-collapse">
            <thead>
              <tr className="text-left text-[11px] uppercase tracking-wider text-gray-500 border-b border-gray-700">
                <th className="w-0 py-2 pl-3 pr-2" />
                <th className="font-medium py-2 px-2">Nom</th>
                <th className="font-medium py-2 px-2">Stack</th>
                <th className="font-medium py-2 px-2 hidden md:table-cell">Lien</th>
                <th className="font-medium py-2 px-2">Port</th>
                <th className="w-0 py-2 pr-3 pl-2" />
              </tr>
            </thead>
            <tbody>
              {apps.map(app => {
                const state = (app.state || '').toLowerCase();
                const isRunning = state === 'running';
                return (
                  <tr
                    key={app.slug}
                    onClick={() => openStudio(app.slug)}
                    className="group cursor-pointer border-b border-gray-800 transition-[background-color,color] duration-300 ease-out hover:duration-0 hover:bg-gray-700/30"
                    title={`Ouvrir le Studio de ${app.name} (nouvel onglet)`}
                  >
                    <td className="py-2 pl-3 pr-2">
                      <span className={`block w-[9px] h-[9px] rounded-full ${statusDot(state)}`} title={state || 'unknown'} />
                    </td>
                    <td className="py-2 px-2 font-medium text-gray-200 group-hover:text-gray-50">
                      <span className="inline-flex items-center gap-1.5">
                        {app.name}
                        {app.has_db && <Database className="w-3 h-3 text-gray-500" title="Base de données" />}
                      </span>
                    </td>
                    <td className="py-2 px-2 text-gray-400">{stackLabel(app.stack)}</td>
                    <td className="py-2 px-2 hidden md:table-cell">
                      <a
                        href={`/apps/${app.slug}/`}
                        target="_blank"
                        rel="noopener noreferrer"
                        onClick={e => e.stopPropagation()}
                        className="inline-flex items-center gap-1 text-blue-600 dark:text-blue-400 hover:text-blue-700 dark:hover:text-blue-300"
                        title={`Ouvrir /apps/${app.slug}/`}
                      >
                        /apps/{app.slug}/ <ExternalLink className="w-3 h-3" />
                      </a>
                    </td>
                    <td className="py-2 px-2 text-gray-400 font-mono">{app.port ?? '-'}</td>
                    <td className="py-2 pr-3 pl-2 text-right">
                      <span className="inline-flex opacity-0 group-hover:opacity-100 transition-opacity">
                        {isRunning ? (
                          <button onClick={e => { e.stopPropagation(); control(app.slug, 'stop'); }} className="p-1 text-yellow-400 hover:bg-gray-600 rounded-sm" title="Stop">
                            <Square className="w-3.5 h-3.5" />
                          </button>
                        ) : (
                          <button onClick={e => { e.stopPropagation(); control(app.slug, 'start'); }} className="p-1 text-green-400 hover:bg-gray-600 rounded-sm" title="Start">
                            <Play className="w-3.5 h-3.5" />
                          </button>
                        )}
                      </span>
                    </td>
                  </tr>
                );
              })}
              {apps.length === 0 && (
                <tr><td colSpan={6} className="py-8 text-center text-gray-600">Aucune application</td></tr>
              )}
            </tbody>
          </table>
          </ScrollableTable>
        )}
      </div>

      {showCreate && <CreateAppModal onClose={() => setShowCreate(false)} onCreated={() => { setShowCreate(false); reload(); }} />}
    </div>
  );
}
