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
      // On ne RETIENT pas le texte de réflexion (lourd, rarement lu) : juste le compteur
      // `chars` (→ count live animé) + l'ordinal `tidx`. Le texte du bloc EN COURS (tail)
      // est gardé le temps qu'il est actif pour un expand instantané ; il est libéré dès
      // qu'un autre item le supersède (cf. boucle ci-dessous) — rechargé alors via fetch.
      const text = ev.data?.text || '';
      if (last && last.type === 'thinking') {
        const t = (last.text || '') + text;
        next[next.length - 1] = { ...last, text: t, chars: t.length };
      } else {
        const tidx = next.reduce((n, it) => n + (it.type === 'thinking' ? 1 : 0), 0);
        next.push({ type: 'thinking', text, chars: text.length, tidx });
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
  // Libère le texte de réflexion dès qu'il n'est plus le dernier item : seul le bloc
  // ACTIF (tail) garde son texte (expand instantané) ; les précédents ne conservent que
  // `chars`+`tidx` et rechargent leur texte à la demande (getThinking).
  for (let i = 0; i < next.length - 1; i++) {
    if (next[i].type === 'thinking' && next[i].text != null) {
      next[i] = { ...next[i], text: undefined };
    }
  }
  return next;
}
