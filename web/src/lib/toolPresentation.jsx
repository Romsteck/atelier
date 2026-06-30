// Couche présentation PARTAGÉE (chat + suivi de scan) : mapping iconKey→icône lucide et le
// hook de compteur lissé. Séparé de `toolDisplay.js` (pur, sans dépendance React/lucide) car
// ces deux pièces importent du React-land. Consommé par AgentPanel (chat) et ScanStepsView.
import { useState, useRef, useEffect } from 'react';
import {
  FileText, FilePlus, FilePen, Terminal, FolderSearch, Search, Globe,
  Bot, ListChecks, NotebookPen, Plug, Wrench, Flag,
} from 'lucide-react';

// iconKey (toolDisplay) → composant lucide. `flag` sert aux outils findings_* du scan.
export const TOOL_ICONS = {
  read: FileText, write: FilePlus, edit: FilePen, bash: Terminal,
  glob: FolderSearch, search: Search, web: Globe, agent: Bot,
  todo: ListChecks, notebook: NotebookPen, mcp: Plug, tool: Wrench, flag: Flag,
};

// Compteur LISSÉ : `target` saute par paliers (les tokens de réflexion arrivent en lots), on
// l'affiche en montant graduellement (≈12 %/frame + 1 min) via rAF pour éviter les à-coups.
// `active` faux (réflexion finie / bloc d'historique) → snap direct.
// SLOWDOWN : on n'avance qu'une frame sur 8 → animation 2× plus lente que ~15 Hz (~7,5 Hz).
const SMOOTH_FRAME_SKIP = 8;
export function useSmoothCount(target, active) {
  const [shown, setShown] = useState(target);
  const targetRef = useRef(target);
  const shownRef = useRef(target);
  targetRef.current = target;

  useEffect(() => {
    if (!active) {
      shownRef.current = targetRef.current;
      setShown(targetRef.current);
      return;
    }
    let raf;
    let frame = 0;
    const tick = () => {
      if (++frame % SMOOTH_FRAME_SKIP === 0) {
        const t = targetRef.current;
        const cur = shownRef.current;
        if (cur !== t) {
          const gap = t - cur;
          const step = Math.sign(gap) * Math.max(1, Math.ceil(Math.abs(gap) * 0.12));
          let nextVal = cur + step;
          if ((gap > 0 && nextVal > t) || (gap < 0 && nextVal < t)) nextVal = t;
          shownRef.current = nextVal;
          setShown(nextVal);
        }
      }
      raf = requestAnimationFrame(tick);
    };
    raf = requestAnimationFrame(tick);
    return () => cancelAnimationFrame(raf);
  }, [active]);

  return shown;
}
