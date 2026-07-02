// Source unique des réglages agent (modèle / mode / effort). Partagé par le panneau de
// conversation (AgentPanel) et l'auto-lancement depuis la surveillance (provider).

// Modèles sélectionnables. Opus 4.8 par défaut = on N'ENVOIE PAS de model → le CLI
// résout le défaut de l'abonnement, soit `claude-opus-4-8[1m]` (contexte 1M).
// `efforts` = niveaux supportés (Fable 5 = même gamme qu'Opus 4.7/4.8, thinking toujours
// actif côté modèle — rien à configurer ici). Sonnet/Haiku retirés du sélecteur (2026-07-02).
export const MODELS = [
  { id: 'opus-4-8', label: 'Opus 4.8', model: null, efforts: ['low', 'medium', 'high', 'xhigh', 'max'] },
  { id: 'fable-5', label: 'Fable 5', model: 'claude-fable-5', efforts: ['low', 'medium', 'high', 'xhigh', 'max'] },
  { id: 'opus-4-7', label: 'Opus 4.7', model: 'claude-opus-4-7', efforts: ['low', 'medium', 'high', 'xhigh', 'max'] },
];

// Normalise un id de modèle persisté (localStorage) : un id retiré du sélecteur
// (ex. 'sonnet-4-6', 'haiku-4-5') retombe sur le défaut Opus — sans ça, le <select>
// afficherait une value orpheline (vide) et la préférence stale ne serait jamais nettoyée.
export function resolveModelId(saved) {
  return MODELS.some((m) => m.id === saved) ? saved : 'opus-4-8';
}

// Deux modes seulement (cf. runner.js) : Plan = lecture seule (explore + planifie),
// Bypass = pleine capacité (édite/exécute, relu via l'onglet Git).
export const MODES = [
  { id: 'plan', label: 'Plan', pm: 'plan', title: 'Lecture seule : explore et planifie, n’écrit rien.' },
  { id: 'bypass', label: 'Bypass', pm: 'bypassPermissions', title: 'Pleine capacité : édite les fichiers, exécute, MCP. À relire dans l’onglet Git.' },
];

// Construit le payload `settings` envoyé à /agent/query (permission_mode + model + effort).
// Opus 4.8 (model:null) → on omet `model` pour conserver le défaut [1m]. Haiku → pas d'effort.
export function buildSettings({ modelId, effort, mode }) {
  const m = MODELS.find((x) => x.id === modelId) || MODELS[0];
  const permission_mode = MODES.find((x) => x.id === mode)?.pm || 'plan';
  const settings = { permission_mode };
  if (m.model) settings.model = m.model;
  if (m.efforts.length) settings.effort = m.efforts.includes(effort) ? effort : m.efforts.at(-1);
  return settings;
}
