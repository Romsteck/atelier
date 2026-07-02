// Kinds de scan (security/code_review/business) dont une conversation de résolution
// groupée (« Résoudre tout ») est OUVERTE dans le workspace agent du slug courant.
// WHY un store module-level plutôt qu'un contexte React : la surveillance
// (SurveillanceTab) n'est PAS dans l'arbre de l'AgentConversationsProvider — celui-ci
// vit dans AgentWorkspace, un onglet frère. Le provider, seul à connaître les onglets
// de conversation ouverts, pousse l'ensemble des kinds ici ; la surveillance le lit
// via le hook, où qu'elle soit montée. Sert à désactiver le bouton « Résoudre tout »
// d'un scan tant que sa conversation est ouverte (évite d'en lancer une seconde), et
// à le réactiver dès qu'on la ferme. (Un Set de kinds suffit : le Studio est per-app,
// le store ne voit que les conversations du slug de la page.)
import { useEffect, useState } from 'react';

let openKinds = new Set();
const listeners = new Set();

function sameSet(a, b) {
  if (a.size !== b.size) return false;
  for (const x of a) if (!b.has(x)) return false;
  return true;
}

// Remplace l'ensemble des kinds ayant une conversation de résolution ouverte.
// No-op si identique (évite des re-renders inutiles chez les abonnés). Appelé par le
// provider à chaque changement de ses onglets.
export function setOpenResolveScans(kinds) {
  const next = kinds instanceof Set ? kinds : new Set(kinds);
  if (sameSet(next, openKinds)) return;
  openKinds = next;
  for (const l of listeners) l(openKinds);
}

// Hook abonné : renvoie le Set courant des kinds en cours de résolution.
export function useOpenResolveScans() {
  const [kinds, setKinds] = useState(openKinds);
  useEffect(() => {
    const l = (next) => setKinds(next);
    listeners.add(l);
    // Resync si l'ensemble a changé entre le render et l'exécution de l'effet.
    if (openKinds !== kinds) setKinds(openKinds);
    return () => listeners.delete(l);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);
  return kinds;
}
