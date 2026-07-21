import { useEffect, useRef, useState } from 'react';
import { Plus } from 'lucide-react';
import { ENGINES, MODELS, resolveModelId } from '../../lib/agentModels';
import { useAgentConversations } from '../../context/AgentConversationsContext';

// Bouton « nouvelle conversation » AVEC choix du moteur.
//
// WHY un menu plutôt que le seul sélecteur « Modèle » du panneau : le moteur est la
// décision la plus structurante d'une conversation (il se fige au binding, un thread
// Codex n'étant pas reprenable par Claude) et c'est au moment de CRÉER qu'on la prend.
// Le cacher dans la liste des modèles, en bas du panneau, le rendait indécouvrable.
//
// Le choix ne fait que SEEDER le modèle du panneau (`seedModelId`) : rien n'est figé
// tant qu'aucun tour n'est parti, le sélecteur du panneau reste libre. On repart de la
// préférence globale quand elle appartient au moteur choisi, sinon du défaut du moteur.
export default function NewConversationButton({ variant = 'icon', className = '' }) {
  const { newConversation } = useAgentConversations();
  const [open, setOpen] = useState(false);
  const ref = useRef(null);

  // Fermeture au clic extérieur / Échap : un menu ouvert ne doit jamais survivre à
  // une interaction ailleurs (il flotte au-dessus du chat).
  useEffect(() => {
    if (!open) return;
    const onDown = (e) => { if (ref.current && !ref.current.contains(e.target)) setOpen(false); };
    const onKey = (e) => { if (e.key === 'Escape') setOpen(false); };
    document.addEventListener('mousedown', onDown);
    document.addEventListener('keydown', onKey);
    return () => {
      document.removeEventListener('mousedown', onDown);
      document.removeEventListener('keydown', onKey);
    };
  }, [open]);

  const start = (engineId) => {
    setOpen(false);
    newConversation({ modelId: resolveModelId(localStorage.getItem('agent:model'), engineId) });
  };

  const trigger =
    variant === 'wide' ? (
      <button
        onClick={() => setOpen((v) => !v)}
        className="inline-flex items-center gap-1.5 px-3 py-1.5 rounded-md text-[12px] font-medium bg-blue-600/15 text-blue-300 border border-blue-500/30 hover:bg-blue-600/25"
      >
        <Plus className="w-4 h-4" /> Nouvelle conversation
      </button>
    ) : (
      <button
        onClick={() => setOpen((v) => !v)}
        title="Nouvelle conversation"
        className={className || 'p-1 rounded-sm text-gray-500 hover:text-gray-200 hover:bg-gray-800'}
      >
        <Plus className="w-4 h-4" />
      </button>
    );

  return (
    <div ref={ref} className="relative shrink-0">
      {trigger}
      {open && (
        <div className="absolute right-0 top-full mt-1 z-30 w-52 rounded-md border border-gray-700 bg-gray-900 shadow-xl py-1">
          <div className="px-2.5 py-1 text-[10px] uppercase tracking-wider text-gray-500">Moteur</div>
          {Object.values(ENGINES).map((en) => {
            const m = MODELS.find((x) => x.id === resolveModelId(localStorage.getItem('agent:model'), en.id));
            return (
              <button
                key={en.id}
                onClick={() => start(en.id)}
                className="w-full flex items-center justify-between gap-2 px-2.5 py-1.5 text-left text-[12px] text-gray-200 hover:bg-gray-800"
              >
                <span>{en.label}</span>
                <span className="text-[10px] text-gray-500">{m?.label}</span>
              </button>
            );
          })}
        </div>
      )}
    </div>
  );
}
