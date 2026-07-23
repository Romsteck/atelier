// Source unique des réglages agent (moteur / modèle / mode / effort). Partagé par le
// panneau de conversation (AgentPanel) et l'auto-lancement depuis la surveillance.

// Moteurs disponibles. Le moteur d'une conversation est FIGÉ au binding de session
// (un thread Codex ne peut pas être repris par Claude et réciproquement) : l'UI ne
// propose le choix que sur un brouillon, puis verrouille sur le moteur de la session.
export const ENGINES = {
  claude: { id: 'claude', label: 'Claude' },
  codex: { id: 'codex', label: 'Codex' },
};

// Modèles sélectionnables, par moteur. Claude/Opus 4.8 par défaut = on N'ENVOIE PAS de
// model → le CLI résout le défaut de l'abonnement, soit `claude-opus-4-8[1m]` (contexte 1M).
// `efforts` = niveaux supportés ; `defaultEffort` = effort d'une NOUVELLE conversation ;
// `effortAlias` = palier équivalent quand un niveau demandé n'existe pas ici (repli ciblé,
// à ne pas confondre avec `defaultEffort`). `fastModel` = tier rapide de la même famille,
// activé par la bascule « Fast » (axe INDÉPENDANT de l'effort). Sonnet/Haiku retirés du
// sélecteur (2026-07-02), Opus 4.7 (2026-07-07).
export const MODELS = [
  { id: 'opus-4-8', engine: 'claude', label: 'Opus 4.8', model: null, efforts: ['low', 'medium', 'high', 'xhigh', 'max'], defaultEffort: 'max' },
  { id: 'fable-5', engine: 'claude', label: 'Fable 5', model: 'claude-fable-5', efforts: ['low', 'medium', 'high', 'xhigh', 'max'], defaultEffort: 'max' },
  {
    // WHY le suffixe `-sol` PARTOUT (id ET model) : le CLI codex ne connaît QUE les tiers
    // `gpt-5.6-{sol,terra,luna}`. Un slug nu déclenche « Model metadata for `gpt-5.6` not
    // found. Defaulting to fallback metadata » à chaque run → métadonnées dégradées.
    // `sol` = tier codage. L'id du sélecteur porte le MÊME texte que le slug wire pour
    // qu'aucun littéral nu ne traîne dans le code (un lecteur pressé pourrait le recopier
    // là où un vrai slug est attendu).
    id: 'gpt-5.6-sol',
    engine: 'codex',
    label: 'GPT 5.6',
    model: 'gpt-5.6-sol',
    efforts: ['low', 'medium', 'high', 'xhigh'],
    // Bascule « Fast » = FAST MODE officiel de Codex (service tier), PAS un autre modèle :
    // même `gpt-5.6-sol`, ~1.5× plus rapide, ~2.5× les crédits (réservé à l'auth abonnement).
    // `gpt-5.6-sol-fast` est un slug WIRE INTERNE Atelier (aucun tier `-fast` côté CLI) :
    // il ne sert qu'à transporter l'état Fast de bout en bout (settings → meta Postgres →
    // events `t:'model'` → restauration du chip). codex.js le DÉCODE (→ modèle réel `sol`
    // + `--config service_tier=fast features.fast_mode=true`) et ne le passe JAMAIS au CLI.
    // WHY pas luna : luna est un AUTRE modèle (tier rapide au rabais) — on veut sol partout.
    fastModel: 'gpt-5.6-sol-fast',
    // WHY : un `max` explicitement demandé (ex. « Résoudre tout ») vaut le palier le plus
    // haut de Codex, pas un repli sur le défaut `medium` — le shim clampe déjà max → xhigh.
    effortAlias: { max: 'xhigh' },
    defaultEffort: 'medium',
  },
];

// Modèle par défaut de chaque moteur (repli de resolveModelId / modelIdFromApi).
const DEFAULT_MODEL_ID = { claude: 'opus-4-8', codex: 'gpt-5.6-sol' };

export const DEFAULT_ENGINE = 'claude';

// Modèles proposables pour un moteur donné (sélecteur d'une conversation LIÉE).
export function modelsForEngine(engine) {
  const e = ENGINES[engine] ? engine : DEFAULT_ENGINE;
  return MODELS.filter((m) => m.engine === e);
}

// Moteur d'un id de modèle du sélecteur (id inconnu → moteur par défaut).
export function engineOfModelId(id) {
  return MODELS.find((m) => m.id === id)?.engine || DEFAULT_ENGINE;
}

// Normalise un id de modèle persisté (localStorage) : un id retiré du sélecteur
// (ex. 'sonnet-4-6', 'opus-4-7') retombe sur le défaut — sans ça, le <select>
// afficherait une value orpheline (vide) et la préférence stale ne serait jamais
// nettoyée. `engine` fourni (conversation liée) → on impose le moteur : une préférence
// d'un AUTRE moteur retombe sur le défaut de CE moteur. `engine` omis (brouillon) →
// toute préférence connue est conservée, quel que soit son moteur.
export function resolveModelId(saved, engine) {
  const m = MODELS.find((x) => x.id === saved);
  if (!engine) return m ? m.id : DEFAULT_MODEL_ID[DEFAULT_ENGINE];
  const e = ENGINES[engine] ? engine : DEFAULT_ENGINE;
  return m && m.engine === e ? m.id : DEFAULT_MODEL_ID[e];
}

// Id sélecteur depuis un nom de modèle SERVEUR — demandé (`settings.model`, ex.
// 'claude-fable-5' ou null = défaut abonnement) OU résolu (`activeModel`, ex.
// 'claude-opus-4-8[1m]'). Un modèle retiré/inconnu retombe sur le défaut du moteur.
// WHY `engine` : côté Codex un `model` absent/inconnu retomberait sinon sur Opus.
export function modelIdFromApi(model, engine) {
  const e = ENGINES[engine] ? engine : DEFAULT_ENGINE;
  if (e !== 'claude') return DEFAULT_MODEL_ID[e];
  if (!model) return DEFAULT_MODEL_ID.claude;
  const m = MODELS.find((x) => x.engine === 'claude' && x.model && model.includes(x.model));
  return m ? m.id : DEFAULT_MODEL_ID.claude;
}

// Deux modes seulement (cf. runner.js / codex.js) : Plan = lecture seule (explore et
// planifie), Bypass = pleine capacité (édite/exécute, relu via l'onglet Git).
export const MODES = [
  { id: 'plan', label: 'Plan', pm: 'plan', title: 'Lecture seule : explore et planifie, n’écrit rien.' },
  { id: 'bypass', label: 'Bypass', pm: 'bypassPermissions', title: 'Pleine capacité : édite les fichiers, exécute, MCP. À relire dans l’onglet Git.' },
];

// Effort réellement applicable à un modèle. Ordre : niveau supporté tel quel → palier
// équivalent (`effortAlias`, ex. Codex max→xhigh : un `max` demandé ne doit PAS retomber
// sur le défaut d'une conversation neuve) → `defaultEffort` (niveau inconnu du modèle).
export function effortFor(m, effort) {
  if (!m?.efforts?.length) return undefined;
  if (m.efforts.includes(effort)) return effort;
  const alias = m.effortAlias?.[effort];
  if (alias && m.efforts.includes(alias)) return alias;
  return m.defaultEffort || m.efforts[m.efforts.length - 1];
}

// Construit le payload `settings` envoyé à /agent/query (engine + permission_mode +
// model + effort). Opus 4.8 (model:null) → on omet `model` pour conserver le défaut [1m].
export function buildSettings({ modelId, effort, mode, fast }) {
  const m = MODELS.find((x) => x.id === modelId) || MODELS[0];
  const permission_mode = MODES.find((x) => x.id === mode)?.pm || 'plan';
  const settings = { engine: m.engine, permission_mode };
  const wire = wireModel(m, fast);
  if (wire) settings.model = wire;
  const eff = effortFor(m, effort);
  if (eff) settings.effort = eff;
  return settings;
}

// Slug WIRE envoyé au backend : le `fastModel` quand « Fast » est actif et que le modèle
// en a un, sinon le modèle nominal. WHY un axe séparé de l'effort : ce sont deux questions
// distinctes (vitesse de service vs profondeur de raisonnement), combinables. Côté Codex,
// « Fast » est le service tier `fast` du CLI (même modèle sol, ~1.5× plus rapide) : il est
// porté par le slug wire `gpt-5.6-sol-fast`, décodé par codex.js (l'effort reste libre
// par-dessus). Côté Claude, `fastModel` est un vrai tier de modèle distinct.
export function wireModel(m, fast) {
  return (fast && m?.fastModel) || m?.model || null;
}

// Vrai si le modèle renvoyé par le serveur est le tier rapide de cette entrée (sert à
// restaurer la bascule « Fast » à la reprise d'une conversation).
export function isFastWire(m, serverModel) {
  return !!(m?.fastModel && serverModel && serverModel.includes(m.fastModel));
}

// Le modèle a-t-il un tier rapide (⇒ afficher la bascule « Fast ») ?
export function hasFast(m) {
  return !!m?.fastModel;
}
