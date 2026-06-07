import { useRef, useState, useEffect } from 'react';
import { Plus, MessageSquarePlus, X } from 'lucide-react';
import AgentPanel from '../AgentPanel';
import { useAgentConversations } from '../../context/AgentConversationsContext';

// Vue des conversations ouvertes. Défaut = panneaux côte à côte de taille égale (CSS
// grid `repeat(n, minmax(0,1fr))`). Repli en onglets quand il y en a trop ou que la
// largeur par panneau devient trop petite (les panneaux restent montés → état/scroll
// préservés, l'état vivant est de toute façon dans le provider).
const MIN_PANEL_W = 340;
const MAX_SPLIT = 3;

export default function ConversationsSplit() {
  const { order, convos, newConversation, closeConversation } = useAgentConversations();
  const ref = useRef(null);
  const [width, setWidth] = useState(0);
  const [active, setActive] = useState(null);

  useEffect(() => {
    const el = ref.current;
    if (!el || typeof ResizeObserver === 'undefined') return;
    const ro = new ResizeObserver((entries) => {
      for (const e of entries) setWidth(e.contentRect.width);
    });
    ro.observe(el);
    return () => ro.disconnect();
  }, []);

  useEffect(() => {
    if (!order.length) { setActive(null); return; }
    if (!active || !order.includes(active)) setActive(order[order.length - 1]);
  }, [order, active]);

  if (!order.length) {
    return (
      <div ref={ref} className="h-full min-h-0 flex flex-col items-center justify-center bg-gray-900 text-center px-4">
        <MessageSquarePlus className="w-10 h-10 text-gray-700 mb-3" />
        <div className="text-[13px] text-gray-500 mb-3">Aucune conversation ouverte.</div>
        <button onClick={newConversation}
          className="px-3 py-1.5 rounded-md text-[13px] bg-blue-500 text-white hover:bg-blue-600 flex items-center gap-1.5">
          <Plus className="w-4 h-4" /> Nouvelle conversation
        </button>
      </div>
    );
  }

  const tabbed = order.length > MAX_SPLIT || (width > 0 && width / order.length < MIN_PANEL_W);
  const title = (key) => convos[key]?.title || 'Conversation';

  return (
    <div ref={ref} className="h-full min-h-0 flex flex-col bg-gray-900">
      {/* Bandeau : onglets (mode replié) + bouton « nouvelle conversation » */}
      <div className="flex items-stretch shrink-0 border-b border-gray-800 h-8 overflow-x-auto">
        {tabbed ? (
          order.map((key) => (
            <button key={key} onClick={() => setActive(key)}
              className={`group flex items-center gap-1.5 px-3 text-[12px] whitespace-nowrap border-r border-gray-800 ${
                active === key ? 'bg-gray-800 text-gray-100' : 'text-gray-500 hover:text-gray-300 hover:bg-gray-800/50'
              }`}>
              {convos[key]?.running && <span className="w-1.5 h-1.5 rounded-full bg-blue-400 animate-pulse" />}
              <span className="truncate max-w-[140px]">{title(key)}</span>
              <X className="w-3 h-3 opacity-0 group-hover:opacity-60 hover:!opacity-100"
                onClick={(e) => { e.stopPropagation(); closeConversation(key); }} />
            </button>
          ))
        ) : (
          <div className="flex-1" />
        )}
        <button onClick={newConversation} title="Nouvelle conversation"
          className="px-2.5 shrink-0 text-gray-500 hover:text-gray-200 hover:bg-gray-800">
          <Plus className="w-4 h-4" />
        </button>
      </div>

      {tabbed ? (
        <div className="flex-1 min-h-0 relative">
          {order.map((key) => (
            <div key={key} className={`absolute inset-0 ${active === key ? '' : 'hidden'}`}>
              <AgentPanel panelKey={key} />
            </div>
          ))}
        </div>
      ) : (
        <div className="flex-1 min-h-0 grid" style={{ gridTemplateColumns: `repeat(${order.length}, minmax(0, 1fr))` }}>
          {order.map((key) => (
            <div key={key} className="min-w-0 min-h-0 overflow-hidden border-r border-gray-800 last:border-r-0">
              <AgentPanel panelKey={key} />
            </div>
          ))}
        </div>
      )}
    </div>
  );
}
