import { useEffect, useMemo, useState } from 'react';
import { Bot, ChevronLeft, ChevronRight, MessageSquarePlus } from 'lucide-react';
import AgentPanel from '../AgentPanel';
import { AgentConversationsProvider, useAgentConversations } from '../../context/AgentConversationsContext';
import { usePilot } from '../../context/PilotContext';
import { setConversationPmMode } from '../../api/client';

function PmDockBody({ slug, mode, setMode }) {
  const { order, active, convos, allConvos, newConversation, openConversation, setActive } = useAgentConversations();

  useEffect(() => {
    if (order.length === 0) newConversation({ modelId: 'opus-4-8' });
  }, [order.length, newConversation]);

  const panelKey = active || order[order.length - 1];
  const known = useMemo(() => new Set(order.map((key) => convos[key]?.sid).filter(Boolean)), [order, convos]);

  return (
    <div className="flex flex-col h-full min-h-0">
      <div className="shrink-0 border-b border-gray-800 px-3 py-2 space-y-2">
        <div className="flex items-center gap-2">
          <div className="inline-flex rounded-md border border-gray-700 bg-gray-900 p-0.5">
            {[
              ['normal', 'Normal'],
              ['brainstorm', 'Brainstorm'],
            ].map(([id, label]) => (
              <button key={id} onClick={() => {
                setMode(id);
                const conversation = convos[active];
                if (conversation?.sid) setConversationPmMode(slug, conversation.sid, id).catch(() => {});
              }}
                className={`px-2 py-1 rounded-sm text-[11px] ${mode === id ? 'bg-blue-500/20 text-blue-700 dark:text-blue-300' : 'text-gray-500 hover:text-gray-300'}`}>
                {label}
              </button>
            ))}
          </div>
          <button onClick={() => newConversation({ modelId: 'opus-4-8' })}
            className="ml-auto p-1.5 rounded-sm text-gray-500 hover:text-gray-200 hover:bg-gray-800" title="Nouvelle conversation">
            <MessageSquarePlus className="w-4 h-4" />
          </button>
        </div>
        {allConvos.length > 0 && (
          <select value="" onChange={(e) => {
            const session = allConvos.find((c) => c.sessionId === e.target.value);
            if (!session) return;
            if (known.has(session.sessionId)) {
              const key = order.find((k) => convos[k]?.sid === session.sessionId);
              if (key) setActive(key);
            } else {
              openConversation(session.sessionId, session.engine || 'claude');
            }
          }} className="w-full h-7 rounded-sm bg-gray-900 border border-gray-700 text-[11px] text-gray-400 px-2">
            <option value="">Reprendre une conversation…</option>
            {allConvos.map((c) => <option key={c.sessionId} value={c.sessionId}>{c.customTitle || c.summary || c.firstPrompt || c.sessionId}</option>)}
          </select>
        )}
      </div>
      <div className="flex-1 min-h-0">
        {panelKey ? <AgentPanel panelKey={panelKey} variant="pm" /> : null}
      </div>
    </div>
  );
}

export default function PmAssistantDock({ slug = '@pilot', className = '' }) {
  const { state } = usePilot();
  const storageKey = `pilot:pmDock:${slug}`;
  const [collapsed, setCollapsed] = useState(() => {
    const saved = localStorage.getItem(`${storageKey}:collapsed`);
    return saved == null ? window.matchMedia('(max-width: 1023px)').matches : saved === '1';
  });
  const [mode, setModeState] = useState(() => localStorage.getItem(`${storageKey}:mode`) || 'normal');
  const setMode = (next) => {
    const safe = next === 'brainstorm' ? 'brainstorm' : 'normal';
    setModeState(safe);
    localStorage.setItem(`${storageKey}:mode`, safe);
  };
  const toggle = () => setCollapsed((value) => {
    localStorage.setItem(`${storageKey}:collapsed`, value ? '0' : '1');
    return !value;
  });

  return (
    <aside className={`fixed inset-y-0 right-0 z-50 lg:static lg:z-auto shrink-0 min-h-0 border-l border-gray-700 bg-gray-900 shadow-2xl lg:shadow-none transition-[width] duration-200 ${collapsed ? 'w-11' : 'w-[min(430px,92vw)] lg:w-[min(430px,40vw)]'} ${className}`}>
      <div className={`h-11 border-b border-gray-700 flex items-center ${collapsed ? 'justify-center' : 'px-3 gap-2'}`}>
        {!collapsed && <>
          <Bot className="w-4 h-4 text-blue-600 dark:text-blue-400" />
          <span className="text-[13px] font-medium text-gray-100">Chef de projet</span>
          <span className="text-[10px] text-gray-500 truncate" title="Le PM reste sur Opus 4.8; ce badge décrit le routeur des workers autonomes.">
            Opus 4.8 · workers auto 4.8/5.6 {state?.engines?.auto_router ? 'actif' : 'en attente'}
          </span>
        </>}
        <button onClick={toggle} className={`${collapsed ? '' : 'ml-auto'} p-1 rounded-sm text-gray-500 hover:text-gray-200 hover:bg-gray-800`}
          title={collapsed ? 'Ouvrir le chef de projet' : 'Replier le chef de projet'}>
          {collapsed ? <ChevronLeft className="w-4 h-4" /> : <ChevronRight className="w-4 h-4" />}
        </button>
      </div>
      {/* Provider monté UNIQUEMENT dock ouvert : monté caché il déclenchait les
          requêtes @pilot (list conversations, resume) sur chaque page. Replier
          démonte — les sessions vivent côté serveur, la réouverture les recharge. */}
      {!collapsed && <div className="h-[calc(100%_-_2.75rem)]">
        <AgentConversationsProvider slug={slug} profile="pm" pmMode={mode}>
          <PmDockBody slug={slug} mode={mode} setMode={setMode} />
        </AgentConversationsProvider>
      </div>}
    </aside>
  );
}
