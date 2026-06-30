// Repli du flux NDJSON normalisé (live) en items de rendu. Les deltas consécutifs
// d'un même type s'accumulent dans le dernier item ouvert ; les autres kinds créent
// des items discrets. Partagé par le provider de conversations (routage WS) et tout
// rendu de fil. Le transcript persisté (op:messages) arrive DÉJÀ sous forme d'items
// (même forme) → utilisé tel quel, sans repasser par appendEvent.
export function appendEvent(items, ev) {
  const next = items.slice();
  const last = next[next.length - 1];
  switch (ev.kind) {
    case 'assistant_delta': {
      const text = ev.data?.text || '';
      if (last && last.type === 'assistant') {
        next[next.length - 1] = { ...last, text: last.text + text };
      } else next.push({ type: 'assistant', text });
      break;
    }
    case 'thinking_delta': {
      // On ne reçoit JAMAIS le texte de réflexion (le serveur n'envoie que le compteur) :
      // on accumule juste `chars` (→ count affiché, animé en live). Aucun détail au front.
      const dchars = ev.data?.chars || 0;
      if (last && last.type === 'thinking') {
        next[next.length - 1] = { ...last, chars: (last.chars || 0) + dchars };
      } else {
        next.push({ type: 'thinking', chars: dchars });
      }
      break;
    }
    case 'tool_use':
      next.push({ type: 'tool_use', name: ev.data?.name, input: ev.data?.input, id: ev.data?.id });
      break;
    case 'tool_result':
      next.push({ type: 'tool_result', text: ev.data?.text || '', isError: !!ev.data?.is_error, tool_use_id: ev.data?.tool_use_id });
      break;
    case 'result':
      next.push({ type: 'result', data: ev.data });
      break;
    case 'error':
      next.push({ type: 'error', message: ev.data?.message || 'erreur' });
      break;
    default:
      break; // system / started / done / turn_done / question : gérés par le routeur
  }
  return next;
}
