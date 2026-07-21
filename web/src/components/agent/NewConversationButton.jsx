import { useEffect, useLayoutEffect, useRef, useState } from 'react';
import { createPortal } from 'react-dom';
import { Plus } from 'lucide-react';
import { ENGINES, MODELS, resolveModelId } from '../../lib/agentModels';
import { useAgentConversations } from '../../context/AgentConversationsContext';

const MENU_W = 208; // px — largeur fixe : sert au calcul de position (clamp bord droit)

// Bouton « nouvelle conversation » AVEC choix du moteur.
//
// WHY un menu plutôt que le seul sélecteur « Modèle » du panneau : le moteur est la
// décision la plus structurante d'une conversation (il se fige au binding, un thread
// Codex n'étant pas reprenable par Claude) et c'est au moment de CRÉER qu'on la prend.
// Le cacher dans la liste des modèles, en bas du panneau, le rendait indécouvrable.
//
// WHY un PORTAL en position fixed : la barre d'onglets qui héberge le « + » est en
// `overflow-x-auto` (les onglets défilent). Un menu en `position:absolute` y était
// ROGNÉ et allongeait la zone scrollable — il apparaissait derrière une barre de
// défilement au lieu de flotter. Le portal l'ancre au body, hors de tout conteneur
// à overflow ; la position est calculée depuis le rect du bouton.
export default function NewConversationButton({ variant = 'icon', className = '' }) {
  const { newConversation } = useAgentConversations();
  const [open, setOpen] = useState(false);
  const [pos, setPos] = useState(null); // {top, left} en coordonnées viewport
  const btnRef = useRef(null);
  const menuRef = useRef(null);

  // Position calculée AVANT peinture (useLayoutEffect) : sinon le menu clignote en
  // haut-gauche le temps d'un frame.
  useLayoutEffect(() => {
    if (!open || !btnRef.current) return;
    const r = btnRef.current.getBoundingClientRect();
    setPos({
      top: r.bottom + 4,
      // Aligné à droite du bouton, borné à la fenêtre (le « + » est souvent collé au bord).
      left: Math.max(8, Math.min(r.right - MENU_W, window.innerWidth - MENU_W - 8)),
    });
  }, [open]);

  // Fermeture : clic extérieur (bouton ET menu sont hors du même arbre DOM → tester
  // les deux), Échap, et tout scroll/resize (le menu est ancré au viewport : le
  // laisser ouvert le détacherait visuellement de son bouton).
  useEffect(() => {
    if (!open) return;
    const onDown = (e) => {
      if (btnRef.current?.contains(e.target) || menuRef.current?.contains(e.target)) return;
      setOpen(false);
    };
    const onKey = (e) => { if (e.key === 'Escape') setOpen(false); };
    const close = () => setOpen(false);
    document.addEventListener('mousedown', onDown);
    document.addEventListener('keydown', onKey);
    window.addEventListener('resize', close);
    window.addEventListener('scroll', close, true); // capture : scroll d'un conteneur interne
    return () => {
      document.removeEventListener('mousedown', onDown);
      document.removeEventListener('keydown', onKey);
      window.removeEventListener('resize', close);
      window.removeEventListener('scroll', close, true);
    };
  }, [open]);

  const start = (engineId) => {
    setOpen(false);
    newConversation({ modelId: resolveModelId(localStorage.getItem('agent:model'), engineId) });
  };

  const trigger =
    variant === 'wide' ? (
      <button
        ref={btnRef}
        onClick={() => setOpen((v) => !v)}
        className="inline-flex items-center gap-1.5 px-3 py-1.5 rounded-md text-[12px] font-medium bg-blue-600/15 text-blue-300 border border-blue-500/30 hover:bg-blue-600/25"
      >
        <Plus className="w-4 h-4" /> Nouvelle conversation
      </button>
    ) : (
      <button
        ref={btnRef}
        onClick={() => setOpen((v) => !v)}
        title="Nouvelle conversation"
        // `shrink-0` : le « + » vit dans une barre d'onglets flex scrollable — sans lui
        // il se fait écraser dès que les onglets débordent.
        className={`shrink-0 ${className || 'p-1 rounded-sm text-gray-500 hover:text-gray-200 hover:bg-gray-800'}`}
      >
        <Plus className="w-4 h-4" />
      </button>
    );

  return (
    <>
      {trigger}
      {open && pos && createPortal(
        <div
          ref={menuRef}
          style={{ position: 'fixed', top: pos.top, left: pos.left, width: MENU_W }}
          className="z-[60] rounded-md border border-gray-700 bg-gray-900 shadow-2xl py-1"
        >
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
        </div>,
        document.body,
      )}
    </>
  );
}
