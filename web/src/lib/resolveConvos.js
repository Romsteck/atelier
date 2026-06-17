// Findings dont une conversation de résolution (« Résoudre ») est OUVERTE dans le
// workspace agent. WHY un store module-level plutôt qu'un contexte React : la
// surveillance (SurveillanceTab) n'est PAS dans l'arbre de l'AgentConversationsProvider
// — celui-ci vit dans AgentWorkspace, un onglet frère. Le provider, seul à connaître
// les onglets de conversation ouverts, pousse l'ensemble des findingId ici ; la
// surveillance le lit via le hook, où qu'elle soit montée. Sert à désactiver le bouton
// « Résoudre » d'un finding tant que sa conversation est ouverte (évite d'en lancer une
// seconde), et à le réactiver dès qu'on la ferme.
import { useEffect, useState } from 'react';

let openIds = new Set();
const listeners = new Set();

function sameSet(a, b) {
  if (a.size !== b.size) return false;
  for (const x of a) if (!b.has(x)) return false;
  return true;
}

// Remplace l'ensemble des findings ayant une conversation de résolution ouverte.
// No-op si identique (évite des re-renders inutiles chez les abonnés). Appelé par le
// provider à chaque changement de ses onglets.
export function setOpenResolveFindings(ids) {
  const next = ids instanceof Set ? ids : new Set(ids);
  if (sameSet(next, openIds)) return;
  openIds = next;
  for (const l of listeners) l(openIds);
}

// Hook abonné : renvoie le Set courant des findingId en cours de résolution.
export function useOpenResolveFindings() {
  const [ids, setIds] = useState(openIds);
  useEffect(() => {
    const l = (next) => setIds(next);
    listeners.add(l);
    // Resync si l'ensemble a changé entre le render et l'exécution de l'effet.
    if (openIds !== ids) setIds(openIds);
    return () => listeners.delete(l);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);
  return ids;
}
