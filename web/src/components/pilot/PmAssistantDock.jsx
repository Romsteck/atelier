import { useEffect, useMemo, useState } from 'react';
import { Bot, MessageSquarePlus, X } from 'lucide-react';
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

export default function PmAssistantDock({ slug = '@pilot', className = '', defaultOpen = false, reopenPill = false }) {
  const { state } = usePilot();
  const storageKey = `pilot:pmDock:${slug}`;
  // Ouvert/fermé (plus de mode « replié ») : fermé = invisible, l'ouverture se
  // fait depuis la navigation (entrée « Chef de projet »), l'event global ou la
  // pastille de réouverture (Studio).
  const initialOpen = () => {
    const saved = localStorage.getItem(`${storageKey}:open`);
    return saved == null ? defaultOpen : saved === '1';
  };
  const [open, setOpen] = useState(initialOpen);
  // Monté à la PREMIÈRE ouverture puis seulement CACHÉ à la fermeture : le
  // contexte (conversations, streaming en cours) survit aux allers-retours.
  // Lazy au départ : monté d'office, le provider déclenchait les requêtes
  // @pilot (list/resume) sur chaque page sans que le CP soit jamais ouvert.
  const [everOpened, setEverOpened] = useState(initialOpen);
  const [mode, setModeState] = useState(() => localStorage.getItem(`${storageKey}:mode`) || 'normal');
  const setMode = (next) => {
    const safe = next === 'brainstorm' ? 'brainstorm' : 'normal';
    setModeState(safe);
    localStorage.setItem(`${storageKey}:mode`, safe);
  };
  const openDock = () => {
    localStorage.setItem(`${storageKey}:open`, '1');
    setEverOpened(true);
    setOpen(true);
  };
  const close = () => {
    setOpen(false);
    localStorage.setItem(`${storageKey}:open`, '0');
  };
  // Le CP est LA porte d'entrée du backlog : la Sidebar (entrée « Chef de
  // projet ») et tout autre point d'entrée demandent l'ouverture via cet event.
  useEffect(() => {
    const handler = () => openDock();
    window.addEventListener('pilot:open-assistant', handler);
    return () => window.removeEventListener('pilot:open-assistant', handler);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [storageKey]);

  return (
    <>
      {!open && reopenPill && (
        <button onClick={openDock}
          className="fixed bottom-4 right-4 z-40 inline-flex items-center gap-1.5 px-3 py-2 rounded-full border border-blue-500/40 bg-gray-900 text-blue-700 dark:text-blue-300 text-xs font-medium shadow-lg hover:bg-gray-800"
          title="Ouvrir le chef de projet">
          <Bot className="w-4 h-4" /> Chef de projet
        </button>
      )}
      {everOpened && (
        <aside className={`${open ? '' : 'hidden'} fixed inset-y-0 right-0 z-50 lg:static lg:z-auto shrink-0 min-h-0 border-l border-gray-700 bg-gray-900 shadow-2xl lg:shadow-none w-[min(430px,92vw)] lg:w-[min(430px,40vw)] ${className}`}>
          <div className="h-11 border-b border-gray-700 flex items-center px-3 gap-2">
            <Bot className="w-4 h-4 text-blue-600 dark:text-blue-400" />
            <span className="text-[13px] font-medium text-gray-100">Chef de projet</span>
            <span className="text-[10px] text-gray-500 truncate" title="Le PM reste sur Opus 4.8; ce badge décrit le routeur des workers autonomes.">
              Opus 4.8 · workers auto 4.8/5.6 {state?.engines?.auto_router ? 'actif' : 'en attente'}
            </span>
            <button onClick={close} className="ml-auto p-1 rounded-sm text-gray-500 hover:text-gray-200 hover:bg-gray-800"
              title="Fermer le chef de projet (la conversation reste en l'état)">
              <X className="w-4 h-4" />
            </button>
          </div>
          <div className="h-[calc(100%_-_2.75rem)]">
            <AgentConversationsProvider slug={slug} profile="pm" pmMode={mode}>
              <PmDockBody slug={slug} mode={mode} setMode={setMode} />
            </AgentConversationsProvider>
          </div>
        </aside>
      )}
    </>
  );
}
