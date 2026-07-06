import { useMemo, useState } from 'react';
import {
  MessageSquareWarning, AlertOctagon, AlertTriangle, Lightbulb, CheckCircle2,
  Copy, Trash2, StickyNote, CircleCheck, CircleSlash, RotateCcw, ClipboardList,
} from 'lucide-react';
import { useIssues } from '../context/IssuesContext';
import PageHeader from '../components/PageHeader';
import StatCard from '../components/StatCard';
import { useToast, Toast } from '../hooks/useToast';

// Nature de la remontée — l'axe `kind` du store (error | limitation | suggestion).
const KINDS = {
  error:      { label: 'Erreur',     icon: AlertOctagon,  color: 'text-red-700 dark:text-red-300',    bg: 'bg-red-500/20 border-red-500/30' },
  limitation: { label: 'Limitation', icon: AlertTriangle, color: 'text-orange-700 dark:text-orange-300', bg: 'bg-orange-500/20 border-orange-500/30' },
  suggestion: { label: 'Suggestion', icon: Lightbulb,     color: 'text-blue-700 dark:text-blue-300',   bg: 'bg-blue-500/20 border-blue-500/30' },
};

const SEVERITIES = {
  high:   { label: 'high',   color: 'text-red-700 dark:text-red-300',       bg: 'bg-red-500/20 border-red-500/30' },
  medium: { label: 'medium', color: 'text-yellow-700 dark:text-yellow-300', bg: 'bg-yellow-500/20 border-yellow-500/30' },
  low:    { label: 'low',    color: 'text-blue-700 dark:text-blue-300',     bg: 'bg-blue-500/20 border-blue-500/30' },
};

const STATUS_TABS = [
  { key: 'open',      label: 'Ouvertes' },
  { key: 'resolved',  label: 'Résolues' },
  { key: 'dismissed', label: 'Écartées' },
  { key: 'all',       label: 'Toutes' },
];

const STATUS_META = {
  open:      { label: 'ouverte', color: 'text-amber-700 dark:text-amber-300' },
  resolved:  { label: 'résolue', color: 'text-emerald-700 dark:text-emerald-300' },
  dismissed: { label: 'écartée', color: 'text-gray-500' },
};

function fmtDate(iso) {
  if (!iso) return '?';
  const d = new Date(iso);
  return d.toLocaleString('fr-FR', { day: '2-digit', month: '2-digit', hour: '2-digit', minute: '2-digit' });
}

// Rapport markdown d'une remontée — le format « copiable » à coller dans une
// session dev Atelier (autonome : toutes les infos utiles au triage).
function issueToMarkdown(it) {
  const kind = KINDS[it.kind]?.label || it.kind;
  const lines = [
    `## [${kind} · ${it.severity}] ${it.title}`,
    `- App : ${it.app} · Area : ${it.area} · Statut : ${it.status} · Signalée : ${it.ts} (${it.id})`,
  ];
  if (it.context) lines.push('', '**Contexte :**', it.context);
  if (it.tried) lines.push('', '**Tenté :**', it.tried);
  if (it.note) lines.push('', '**Note :**', it.note);
  return lines.join('\n');
}

function Chip({ meta, children, title }) {
  return (
    <span title={title} className={`text-[11px] px-1.5 py-0.5 rounded-sm border ${meta.bg} ${meta.color} inline-flex items-center gap-1 whitespace-nowrap`}>
      {children}
    </span>
  );
}

function ActionButton({ icon: Icon, label, onClick, danger }) {
  return (
    <button
      type="button"
      onClick={onClick}
      className={`text-xs px-2 py-1 rounded-sm border border-gray-700/50 flex items-center gap-1.5 transition ${
        danger ? 'text-red-700 dark:text-red-300 hover:bg-red-500/10' : 'text-gray-300 hover:bg-gray-700/40'
      }`}
    >
      <Icon className="w-3.5 h-3.5" /> {label}
    </button>
  );
}

function IssueCard({ it, onPatch, onDelete, onCopy }) {
  const kind = KINDS[it.kind] || KINDS.error;
  const sev = SEVERITIES[it.severity] || SEVERITIES.medium;
  const status = STATUS_META[it.status] || STATUS_META.open;
  const KindIcon = kind.icon;
  const [noteOpen, setNoteOpen] = useState(false);
  const [noteDraft, setNoteDraft] = useState(it.note || '');

  const saveNote = () => {
    onPatch(it.id, { note: noteDraft });
    setNoteOpen(false);
  };

  return (
    <div className="bg-gray-800/50 border border-gray-700/50 rounded-lg overflow-hidden">
      <div className="px-3 py-2 flex items-start gap-2">
        <KindIcon className={`w-4 h-4 mt-0.5 shrink-0 ${kind.color}`} />
        <div className="flex-1 min-w-0">
          <div className="flex items-center gap-2 flex-wrap">
            <span className="text-sm text-gray-50 font-medium">{it.title}</span>
            <Chip meta={kind}>{kind.label}</Chip>
            <Chip meta={sev} title="Sévérité">{sev.label}</Chip>
            <span className="text-[11px] px-1.5 py-0.5 rounded-sm border border-gray-700/50 text-gray-400">{it.area}</span>
            <span className="text-[11px] px-1.5 py-0.5 rounded-sm border border-gray-700/50 text-gray-400">{it.app}</span>
          </div>
          <div className="text-[11px] text-gray-500 mt-0.5">
            <span className={status.color}>{status.label}</span>
            {' · '}<span title={it.ts}>{fmtDate(it.ts)}</span>
            {it.updated_at && <span title={it.updated_at}> · maj {fmtDate(it.updated_at)}</span>}
            {' · '}{it.id}
          </div>
          {it.context && (
            <p className="text-xs text-gray-400 mt-2 whitespace-pre-wrap break-words">{it.context}</p>
          )}
          {it.tried && (
            <p className="text-xs text-gray-500 mt-1 whitespace-pre-wrap break-words">
              <span className="text-gray-400 font-medium">Tenté : </span>{it.tried}
            </p>
          )}
          {it.note && !noteOpen && (
            <p className="text-xs text-amber-700 dark:text-amber-300/80 mt-1 whitespace-pre-wrap break-words">
              <span className="font-medium">Note : </span>{it.note}
            </p>
          )}
          {noteOpen && (
            <div className="mt-2 flex items-start gap-2">
              <textarea
                value={noteDraft}
                onChange={(e) => setNoteDraft(e.target.value)}
                rows={2}
                placeholder="Note de triage (commit, explication…)"
                className="flex-1 text-xs bg-gray-900/60 border border-gray-700/50 rounded-sm px-2 py-1.5 text-gray-300 focus:outline-none focus:border-gray-500"
              />
              <button type="button" onClick={saveNote} className="text-xs px-2 py-1 rounded-sm border border-gray-700/50 text-gray-300 hover:bg-gray-700/40">
                Enregistrer
              </button>
            </div>
          )}
        </div>
      </div>
      <div className="px-3 py-2 border-t border-gray-700/50 flex items-center gap-2 flex-wrap">
        {it.status === 'open' ? (
          <>
            <ActionButton icon={CircleCheck} label="Résoudre" onClick={() => onPatch(it.id, { status: 'resolved' })} />
            <ActionButton icon={CircleSlash} label="Écarter" onClick={() => onPatch(it.id, { status: 'dismissed' })} />
          </>
        ) : (
          <ActionButton icon={RotateCcw} label="Rouvrir" onClick={() => onPatch(it.id, { status: 'open' })} />
        )}
        <ActionButton icon={StickyNote} label="Note" onClick={() => setNoteOpen((v) => !v)} />
        <span className="flex-1" />
        <ActionButton icon={Copy} label="Copier" onClick={() => onCopy(it)} />
        <ActionButton icon={Trash2} label="Supprimer" danger onClick={() => onDelete(it.id)} />
      </div>
    </div>
  );
}

export default function Issues() {
  const { items, patch, remove } = useIssues();
  const { toast, showToast } = useToast();
  const [statusFilter, setStatusFilter] = useState('open');
  const [kindFilter, setKindFilter] = useState('all');
  const [appFilter, setAppFilter] = useState('all');

  const apps = useMemo(() => [...new Set(items.map((it) => it.app))].sort(), [items]);

  const statusCounts = useMemo(() => {
    const c = { open: 0, resolved: 0, dismissed: 0, all: items.length };
    for (const it of items) c[it.status] = (c[it.status] || 0) + 1;
    return c;
  }, [items]);

  const openByKind = useMemo(() => {
    const c = { error: 0, limitation: 0, suggestion: 0 };
    for (const it of items) {
      if (it.status === 'open') c[it.kind] = (c[it.kind] || 0) + 1;
    }
    return c;
  }, [items]);

  const filtered = useMemo(() => items.filter((it) =>
    (statusFilter === 'all' || it.status === statusFilter)
    && (kindFilter === 'all' || it.kind === kindFilter)
    && (appFilter === 'all' || it.app === appFilter)
  ), [items, statusFilter, kindFilter, appFilter]);

  const copyText = (text, msg) => {
    navigator.clipboard.writeText(text)
      .then(() => showToast(msg))
      .catch(() => showToast('Copie impossible (presse-papier indisponible)', 'error'));
  };

  const copyOne = (it) => copyText(issueToMarkdown(it), 'Rapport copié');
  const copyAll = () => {
    if (!filtered.length) return;
    copyText(
      filtered.map(issueToMarkdown).join('\n\n---\n\n'),
      `${filtered.length} rapport${filtered.length > 1 ? 's' : ''} copié${filtered.length > 1 ? 's' : ''}`,
    );
  };

  const handleDelete = (id) => {
    remove(id);
    showToast('Remontée supprimée');
  };

  return (
    <div className="h-full flex flex-col">
      <Toast toast={toast} />
      <PageHeader title="Remontées plateforme" icon={MessageSquareWarning}>
        <button
          type="button"
          onClick={copyAll}
          disabled={!filtered.length}
          className="text-xs px-2.5 py-1.5 rounded-sm border border-gray-700/50 text-gray-300 hover:bg-gray-700/40 flex items-center gap-1.5 disabled:opacity-40 disabled:cursor-not-allowed"
          title="Copie les rapports du filtre courant en markdown"
        >
          <ClipboardList className="w-3.5 h-3.5" /> Copier tout ({filtered.length})
        </button>
      </PageHeader>

      <div className="flex-1 overflow-y-auto p-4 space-y-4">
        <div className="grid grid-cols-2 md:grid-cols-4 gap-3">
          <StatCard icon={AlertOctagon} label="Erreurs ouvertes" value={openByKind.error} color="text-red-400" />
          <StatCard icon={AlertTriangle} label="Limitations ouvertes" value={openByKind.limitation} color="text-orange-400" />
          <StatCard icon={Lightbulb} label="Suggestions ouvertes" value={openByKind.suggestion} color="text-blue-400" />
          <StatCard icon={CheckCircle2} label="Traitées" value={statusCounts.resolved + statusCounts.dismissed} sub={`${statusCounts.resolved} résolues · ${statusCounts.dismissed} écartées`} color="text-emerald-400" />
        </div>

        <div className="flex items-center gap-2 flex-wrap">
          <div className="flex rounded-sm border border-gray-700/50 overflow-hidden">
            {STATUS_TABS.map((t) => (
              <button
                key={t.key}
                type="button"
                onClick={() => setStatusFilter(t.key)}
                className={`text-xs px-2.5 py-1.5 transition ${
                  statusFilter === t.key ? 'bg-gray-700/60 text-gray-50' : 'text-gray-400 hover:bg-gray-700/30'
                }`}
              >
                {t.label} ({statusCounts[t.key] ?? 0})
              </button>
            ))}
          </div>
          <div className="flex rounded-sm border border-gray-700/50 overflow-hidden">
            <button
              type="button"
              onClick={() => setKindFilter('all')}
              className={`text-xs px-2.5 py-1.5 transition ${kindFilter === 'all' ? 'bg-gray-700/60 text-gray-50' : 'text-gray-400 hover:bg-gray-700/30'}`}
            >
              Toutes natures
            </button>
            {Object.entries(KINDS).map(([key, meta]) => (
              <button
                key={key}
                type="button"
                onClick={() => setKindFilter(key)}
                className={`text-xs px-2.5 py-1.5 transition flex items-center gap-1 ${
                  kindFilter === key ? `bg-gray-700/60 ${meta.color}` : 'text-gray-400 hover:bg-gray-700/30'
                }`}
              >
                <meta.icon className="w-3 h-3" /> {meta.label}s
              </button>
            ))}
          </div>
          {apps.length > 1 && (
            <select
              value={appFilter}
              onChange={(e) => setAppFilter(e.target.value)}
              className="text-xs bg-gray-800 border border-gray-700/50 rounded-sm px-2 py-1.5 text-gray-300 focus:outline-none"
            >
              <option value="all">Toutes les apps</option>
              {apps.map((a) => <option key={a} value={a}>{a}</option>)}
            </select>
          )}
        </div>

        {filtered.length === 0 ? (
          <div className="text-center py-12 text-gray-500">
            <CheckCircle2 className="w-8 h-8 mx-auto mb-2 text-emerald-700 dark:text-emerald-400" />
            {statusFilter === 'open' && kindFilter === 'all' && appFilter === 'all'
              ? 'Aucune remontée ouverte — rien à traiter.'
              : 'Aucune remontée ne correspond au filtre.'}
          </div>
        ) : (
          <div className="space-y-3 max-w-4xl">
            {filtered.map((it) => (
              <IssueCard key={it.id} it={it} onPatch={patch} onDelete={handleDelete} onCopy={copyOne} />
            ))}
          </div>
        )}
      </div>
    </div>
  );
}
