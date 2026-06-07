import { useEffect, useState } from 'react';
import { Plus, Trash2, Pencil, Check, X } from 'lucide-react';
import { useAgentConversations } from '../../context/AgentConversationsContext';
import ConfirmModal from '../ConfirmModal';

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
  const { allConvos, order, convos, refreshAll, openConversation, newConversation, renameBySid, removeBySid } =
    useAgentConversations();
  const [editing, setEditing] = useState(null); // sid en cours de renommage
  const [editTitle, setEditTitle] = useState('');
  const [confirmDel, setConfirmDel] = useState(null); // sid à supprimer

  useEffect(() => { if (active) refreshAll(); }, [active, refreshAll]);

  const openSids = new Set(order.map((k) => convos[k]?.sid).filter(Boolean));

  const startEdit = (sid, current) => { setEditing(sid); setEditTitle(current || ''); };
  const commitEdit = (sid) => {
    const t = editTitle.trim();
    if (t) renameBySid(sid, t);
    setEditing(null);
  };

  return (
    <div className="h-full min-h-0 flex flex-col bg-gray-900">
      <div className="flex items-center justify-between px-3 h-9 shrink-0 border-b border-gray-800">
        <span className="text-[12px] text-gray-400">Conversations</span>
        <button onClick={newConversation} title="Nouvelle conversation"
          className="p-1 rounded-sm text-gray-500 hover:text-gray-200 hover:bg-gray-800">
          <Plus className="w-4 h-4" />
        </button>
      </div>

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
              onClick={() => editing !== sid && openConversation(sid)}>
              {editing === sid ? (
                <div className="flex items-center gap-1" onClick={(e) => e.stopPropagation()}>
                  <input autoFocus value={editTitle} onChange={(e) => setEditTitle(e.target.value)}
                    onKeyDown={(e) => { if (e.key === 'Enter') commitEdit(sid); if (e.key === 'Escape') setEditing(null); }}
                    className="flex-1 bg-gray-800 border border-gray-700 rounded-sm px-1.5 py-0.5 text-[12px] text-gray-100 focus:outline-none focus:border-blue-500" />
                  <button onClick={() => commitEdit(sid)} className="p-1 text-green-400 hover:bg-gray-800 rounded-sm"><Check className="w-3.5 h-3.5" /></button>
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
                      <button onClick={(e) => { e.stopPropagation(); setConfirmDel(sid); }}
                        title="Supprimer" className="p-1 text-gray-500 hover:text-red-400 hover:bg-gray-800 rounded-sm"><Trash2 className="w-3 h-3" /></button>
                    </div>
                  </div>
                  <div className="flex items-center gap-2 text-[10px] text-gray-600 mt-0.5">
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
        onConfirm={() => { removeBySid(confirmDel); setConfirmDel(null); }}
        title="Supprimer la conversation"
        message="La session et son historique seront définitivement supprimés du disque."
        confirmText="Supprimer"
        variant="danger"
      />
    </div>
  );
}
