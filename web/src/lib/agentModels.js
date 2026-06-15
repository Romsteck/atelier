// Source unique des réglages agent (modèle / mode / effort). Partagé par le panneau de
// conversation (AgentPanel) et l'auto-lancement depuis la surveillance (provider).

// Modèles sélectionnables. Opus 4.8 par défaut = on N'ENVOIE PAS de model → le CLI
// résout le défaut de l'abonnement, soit `claude-opus-4-8[1m]` (contexte 1M).
// `efforts` = niveaux supportés (xhigh/max = Opus ; Haiku n'a aucun param effort).
export const MODELS = [
  { id: 'opus-4-8', label: 'Opus 4.8', model: null, efforts: ['low', 'medium', 'high', 'xhigh', 'max'] },
  { id: 'opus-4-7', label: 'Opus 4.7', model: 'claude-opus-4-7', efforts: ['low', 'medium', 'high', 'xhigh', 'max'] },
  { id: 'sonnet-4-6', label: 'Sonnet 4.6', model: 'claude-sonnet-4-6', efforts: ['low', 'medium', 'high'] },
  { id: 'haiku-4-5', label: 'Haiku 4.5', model: 'claude-haiku-4-5', efforts: [] },
];

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
