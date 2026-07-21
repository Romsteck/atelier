import { useEffect, useState } from 'react';
import { Trash2, Pencil, Check, X } from 'lucide-react';
import { useAgentConversations } from '../../context/AgentConversationsContext';
import { ENGINES } from '../../lib/agentModels';
import ConfirmModal from '../ConfirmModal';
import NewConversationButton from './NewConversationButton';

// Pastille du moteur d'une conversation. Une entrée sans `engine` date d'avant le
// second moteur → Claude. Teintée par moteur pour distinguer les deux d'un coup d'œil.
const ENGINE_CHIP = {
  claude: 'border-blue-500/30 bg-blue-500/15 text-blue-700 dark:text-blue-300',
  codex: 'border-emerald-500/30 bg-emerald-500/15 text-emerald-700 dark:text-emerald-300',
};
function EngineBadge({ engine }) {
  const e = ENGINES[engine] ? engine : 'claude';
  return (
    <span className={`shrink-0 rounded-sm border px-1 py-px text-[9px] uppercase tracking-wider ${ENGINE_CHIP[e]}`}
      title={`moteur : ${ENGINES[e].label}`}>
      {ENGINES[e].label}
    </span>
  );
}

// Date relative compacte (mêmes conventions que GitTab).
function ago(ms) {
  if (!ms) return '';
  const s = Math.max(0, Math.floor((Date.now() - ms) / 1000));
  if (s < 60) return "à l'instant";
  const m = Math.floor(s / 60);
  if (m < 60) return `il y a ${m} min`;
  const h = Math.floor(m / 60);
  if (h < 24) return `il y a ${h} h`;
  const d = Math.floor(h / 24);
  return `il y a ${d} j`;
}

// Panneau latéral listant TOUTES les conversations de l'app (sessions SDK). Ouvrir =
// (re)mettre dans le split ; les conversations fermées sont ré-ouvrables ici.
export default function ConversationsHistoryPanel({ active }) {
  const { allConvos, unavailableEngines, order, convos, refreshAll, openConversation, renameBySid, removeBySid } =
    useAgentConversations();
  const [editing, setEditing] = useState(null); // sid en cours de renommage
  const [editTitle, setEditTitle] = useState('');
  const [confirmDel, setConfirmDel] = useState(null); // { sid, engine } à supprimer

  useEffect(() => { if (active) refreshAll(); }, [active, refreshAll]);

  const openSids = new Set(order.map((k) => convos[k]?.sid).filter(Boolean));

  const startEdit = (sid, current) => { setEditing(sid); setEditTitle(current || ''); };
  // Le moteur accompagne chaque mutation : rename/delete sont scopés par moteur côté
  // serveur (stores de sessions distincts).
  const commitEdit = (sid, engine) => {
    const t = editTitle.trim();
    if (t) renameBySid(sid, t, engine);
    setEditing(null);
  };

  return (
    <div className="h-full min-h-0 flex flex-col bg-gray-900">
      <div className="flex items-center justify-between px-3 h-9 shrink-0 border-b border-gray-800">
        <span className="text-[12px] text-gray-400">Conversations</span>
        <NewConversationButton />
      </div>

      {/* Historique partiel : un moteur n'a pas répondu au dernier refresh. Les entrées
          de ce moteur affichées viennent du refresh précédent (elles ne sont PAS perdues),
          mais elles peuvent être périmées → on l'annonce sur une ligne, sans bloquer. */}
      {unavailableEngines?.length > 0 && (
        <div className="shrink-0 px-3 py-1.5 border-b border-amber-500/20 bg-amber-500/10 text-[11px] text-amber-300/90">
          Historique partiel — moteur {unavailableEngines.map((e) => ENGINES[e]?.label || e).join(', ')} indisponible
        </div>
      )}

      <div className="flex-1 min-h-0 overflow-y-auto">
        {allConvos.length === 0 && (
          <div className="text-[12px] text-gray-600 p-3 text-center">Aucune conversation.</div>
        )}
        {allConvos.map((c) => {
          const sid = c.sessionId;
          const isOpen = openSids.has(sid);
          const display = c.customTitle || c.summary || c.firstPrompt || (sid ? sid.slice(0, 8) : '?');
          const when = c.lastModified || c.createdAt;
          return (
            <div key={sid}
              className={`group px-3 py-2 border-b border-gray-800/40 cursor-pointer ${isOpen ? 'bg-gray-800/40' : 'hover:bg-gray-800/30'}`}
              onClick={() => editing !== sid && openConversation(sid, c.engine)}>
              {editing === sid ? (
                <div className="flex items-center gap-1" onClick={(e) => e.stopPropagation()}>
                  <input autoFocus value={editTitle} onChange={(e) => setEditTitle(e.target.value)}
                    onKeyDown={(e) => { if (e.key === 'Enter') commitEdit(sid, c.engine); if (e.key === 'Escape') setEditing(null); }}
                    className="flex-1 bg-gray-800 border border-gray-700 rounded-sm px-1.5 py-0.5 text-[12px] text-gray-100 focus:outline-none focus:border-blue-500" />
                  <button onClick={() => commitEdit(sid, c.engine)} className="p-1 text-green-400 hover:bg-gray-800 rounded-sm"><Check className="w-3.5 h-3.5" /></button>
                  <button onClick={() => setEditing(null)} className="p-1 text-gray-500 hover:bg-gray-800 rounded-sm"><X className="w-3.5 h-3.5" /></button>
                </div>
              ) : (
                <>
                  <div className="flex items-center gap-1.5">
                    {c.live && <span className="w-1.5 h-1.5 rounded-full bg-green-400 shrink-0" title="conversation vivante" />}
                    <span className="text-[13px] text-gray-200 truncate flex-1" title={display}>{display}</span>
                    <div className="flex items-center gap-0.5 opacity-0 group-hover:opacity-100">
                      <button onClick={(e) => { e.stopPropagation(); startEdit(sid, c.customTitle || c.summary); }}
                        title="Renommer" className="p-1 text-gray-500 hover:text-gray-200 hover:bg-gray-800 rounded-sm"><Pencil className="w-3 h-3" /></button>
                      <button onClick={(e) => { e.stopPropagation(); setConfirmDel({ sid, engine: c.engine }); }}
                        title="Supprimer" className="p-1 text-gray-500 hover:text-red-400 hover:bg-gray-800 rounded-sm"><Trash2 className="w-3 h-3" /></button>
                    </div>
                  </div>
                  <div className="flex items-center gap-2 text-[10px] text-gray-600 mt-0.5">
                    <EngineBadge engine={c.engine} />
                    {isOpen && <span className="text-blue-400/80">ouverte</span>}
                    <span>{ago(when)}</span>
                  </div>
                </>
              )}
            </div>
          );
        })}
      </div>

      <ConfirmModal
        isOpen={!!confirmDel}
        onClose={() => setConfirmDel(null)}
        onConfirm={() => { removeBySid(confirmDel.sid, confirmDel.engine); setConfirmDel(null); }}
        title="Supprimer la conversation"
        message="La session et son historique seront définitivement supprimés du disque."
        confirmText="Supprimer"
        variant="danger"
      />
    </div>
  );
}
