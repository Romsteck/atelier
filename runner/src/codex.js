// Atelier codex runner — shim NDJSON autour du SDK Codex (OpenAI).
// JUMEAU de runner.js (agent Claude) : MÊME protocole stdin/stdout (1re ligne stdin =
// init JSON, lignes suivantes = tours/contrôle ; stdout = 1 objet {t:…}/ligne), même
// discipline de flush, mêmes invariants (`system` émis une fois et AVANT tout binding,
// `result` PUIS `turn_done` à chaque fin de tour). Le driver Rust (routes/agent.rs) ne
// fait AUCUNE différence entre les deux moteurs au-delà du binaire lancé.
//
// Différences structurelles avec le SDK Claude (WHY le code diverge) :
//   - pas de streaming-input : un objet `Thread` gère tous les tours, mais CHAQUE tour
//     spawne un `codex exec … resume <id>` frais. On tient donc nous-mêmes la file de
//     tours et la boucle "un tour à la fois" (là où runner.js délègue au générateur).
//   - `threadOptions` est relu à CHAQUE tour → muter l'objet = set_mode/set_model/effort
//     appliqués au tour SUIVANT (pas d'API live, d'où le diag quand un tour est en vol).
//   - pas de dialogues interactifs (AskUserQuestion/ExitPlanMode) : `answer`/`plan_decision`
//     sont acceptés puis ignorés (diag) pour que le contrat HTTP reste commun.
//   - pas de MCP studio en v1 (aucun mcpEndpoint/mcpToken dans l'init de ce runner).
//   - PERSISTANCE : le CLI écrit ses rollouts dans $CODEX_HOME/sessions sous un format
//     interne INSTABLE. On ne le parse pas : on tient un sidecar (index + transcripts
//     NDJSON déjà normalisés) sous $CODEX_HOME/atelier — cf. section « sidecar ».
// Aucun secret en argv/env : le contenu d'auth.json arrive par stdin (init) et n'est
// écrit que dans un fichier 0600 ; aucune clé API n'est acceptée nulle part.
import { createInterface } from 'node:readline';
import { appendFileSync, existsSync, mkdtempSync, readFileSync } from 'node:fs';
import { promises as fsp } from 'node:fs';
import { homedir, tmpdir } from 'node:os';
import { join } from 'node:path';
import { makeIo, toolResultText } from './common.js';

const { emit, diag, fail } = makeIo('codex');

// Modèle unique côté Atelier (l'UI ne propose que celui-ci ; `set_model` peut le changer).
// WHY le suffixe `-sol` : le slug nu `gpt-5.6` N'EXISTE PAS côté CLI (run réel 0.144.6 :
// « Model metadata for `gpt-5.6` not found. Defaulting to fallback metadata; this can
// degrade performance and cause issues. »). Seuls gpt-5.6-{sol,terra,luna} sont connus du
// binaire ; `sol` est le tier codage. Libellé UI inchangé (« GPT 5.6 »).
const DEFAULT_MODEL = 'gpt-5.6-sol';
// Slug WIRE interne (aucun tier `-fast` côté CLI) : porte l'état « fast mode » de bout en
// bout (settings front → meta Postgres → set_model → events). On le DÉCODE ici : le modèle
// réel reste `sol`, le fast passe par le service tier (cf. makeCodex). Voir agentModels.js.
const FAST_WIRE = `${DEFAULT_MODEL}-fast`;
// Décode un slug wire venu du front/meta → { model CLI réel, fast, wire ré-émis }. Tout slug
// inconnu (luna d'une meta pré-fix, terra, `gpt-5.6` nu, vide) est COERCÉ sur sol standard :
// un seul modèle est offert par l'UI, un slug étranger ne doit jamais atteindre le CLI (→
// « model metadata not found » + dégradation). `changed` sert à logger la coercition.
function decodeModel(wire) {
  const w = (wire || '').trim();
  if (w === FAST_WIRE) return { model: DEFAULT_MODEL, fast: true, wire: FAST_WIRE, changed: false };
  const changed = w !== '' && w !== DEFAULT_MODEL;
  return { model: DEFAULT_MODEL, fast: false, wire: DEFAULT_MODEL, changed };
}
const EFFORTS = new Set(['minimal', 'low', 'medium', 'high', 'xhigh']);
// Corps de tool_result borné : l'UI n'affiche qu'un extrait, et un `aggregated_output`
// de build fait des mégaoctets (pipe saturé + buffer transcript inutilement gros).
const MAX_TOOL_RESULT = 700;
const SUMMARY_LEN = 80;

// Toolchain sur PATH (WHY) : identique à runner.js — le runner est spawné via
// `sudo -H -u hr-studio` qui réinitialise l'env sur son secure_path, où `~/.cargo/bin`
// et `~/.local/bin` sont absents. Le CLI codex hérite de ce process.env pour exécuter
// les commandes du sandbox → sans ça, `cargo: command not found` en mode bypass.
if (process.env.HOME) {
  process.env.PATH = `${process.env.HOME}/.cargo/bin:${process.env.HOME}/.local/bin:${process.env.PATH || ''}`;
}

// CODEX_HOME est lu par le CLI dans SON env : on le fige explicitement (et on le
// re-pose sur process.env) pour que le shim et le binaire regardent le MÊME dossier.
const CODEX_HOME = process.env.CODEX_HOME || join(homedir(), '.codex');
process.env.CODEX_HOME = CODEX_HOME;
const AUTH_PATH = join(CODEX_HOME, 'auth.json');
const SIDE_DIR = join(CODEX_HOME, 'atelier');
const INDEX_PATH = join(SIDE_DIR, 'index.json');
const TRANSCRIPT_DIR = join(SIDE_DIR, 'transcripts');

// Sources d'auth reconnues par le CLI 0.144.6 (WHY la liste exhaustive) : au-delà de
// CODEX_API_KEY/OPENAI_API_KEY, le binaire accepte aussi CODEX_ACCESS_TOKEN et CODEX_AUTH.
// Une seule liste partagée par la garde d'entrée ET le filtre d'env du child : les deux
// endroits doivent couvrir le MÊME ensemble, sinon une variable oubliée bascule
// silencieusement la facturation hors abonnement.
const AUTH_ENV_KEYS = ['CODEX_API_KEY', 'OPENAI_API_KEY', 'CODEX_ACCESS_TOKEN', 'CODEX_AUTH'];

// Garde abonnement (WHY) : une clé/token d'auth dans l'env bascule SILENCIEUSEMENT le CLI
// en facturation clé API au lieu de l'abonnement ChatGPT (le SDK pose lui-même
// `env.CODEX_API_KEY` quand `apiKey` est fourni — on n'en fournit jamais, mais l'env
// parent suffirait). On échoue fort et tôt.
// `needAuthFile` : chemins qui appellent réellement le modèle sans candidat en main.
async function assertSubscriptionOnly(needAuthFile) {
  for (const k of AUTH_ENV_KEYS) {
    if (process.env[k]) {
      await fail(`${k} présent dans l'env : le runner codex doit utiliser l'abonnement ChatGPT (OAuth), pas une clé API. Abandon.`);
    }
  }
  if (needAuthFile && !existsSync(AUTH_PATH)) {
    await fail(`Aucune authentification Codex (${AUTH_PATH} absent) — connecte le compte ChatGPT via Paramètres → Codex.`);
  }
}

// Auth Codex morte/absente : mêmes causes que côté Claude (token expiré/révoqué,
// login jamais fait), remontées au driver Rust par le MÊME code machine
// `sdk_auth_failed` → une notification plateforme dédupliquée.
// WHY cette liste (messages RÉELS observés sur le CLI 0.144.6, pas des suppositions) :
//   - « Your access token could not be refreshed because your refresh token was already
//     used. Please log out and sign in again. »  (refresh token périmé — cas le plus
//     fréquent, non couvert par la 1re version de cette regex)
//   - « missing field `id_token` »               (auth.json corrompu/tronqué)
//   - « 401 Unauthorized: Missing bearer or basic authentication in header » (aucun
//     credential du tout)
// Le CLI répète l'erreur ~12 fois par run (retries) → le reporter reste once-only.
const CODEX_AUTH_RE = new RegExp([
  '\\b401\\b',
  'unauthorized',
  'missing bearer',
  'access token could not be refreshed',
  'refresh token[^.]{0,80}already used',
  'log ?out and sign in',
  'missing field .?id_token',
  'invalid[ _-]?api[ _-]?key',
  'token (?:has )?(?:expired|revoked)',
  'not logged in',
  'login required',
  'please run .?codex login',
  'invalid_grant',
  'auth\\.json',
].join('|'), 'i');
// DEUX flags DISTINCTS, WHY (les confondre a produit deux bugs) :
//   - `authReported` = SIGNALEMENT. Armé par n'importe quelle ligne matchant la regex,
//     y compris les retries transitoires (« Reconnecting… 1/5 (401 Unauthorized) ») qui
//     précèdent une bascule WebSocket→HTTPS RÉUSSIE. Son seul rôle : n'émettre l'event
//     `sdk_auth_failed` qu'UNE fois par salve.
//   - `authFatal` = VERDICT. Armé UNIQUEMENT quand le tour se termine en échec sur un
//     message d'auth (`turn.failed` ou exception terminale du générateur), JAMAIS depuis
//     un event `error` de retry ni depuis un Stop utilisateur. C'est lui — et lui seul —
//     qui autorise la fermeture anticipée de la session en fin de boucle.
// Sans cette séparation : (a) un Stop pressé pendant une bascule transitoire fermait une
// session SAINE « pour authentification morte » ; (b) un 401 transitoire au tour 1 armait
// le flag pour toujours, si bien qu'un tour 3 échouant pour une cause SANS RAPPORT
// (sandbox, exit non nul) fermait la session avec un diagnostic trompeur.
let authReported = false;
let authFatal = false;
function reportCodexAuth(detail) {
  if (authReported) return;
  authReported = true;
  emit({
    t: 'error',
    code: 'sdk_auth_failed',
    message: `Authentification Codex expirée ou absente (${String(detail).slice(0, 160)}) — reconnecte le compte ChatGPT via Paramètres → Codex.`,
  });
}

// 'max' est le vocabulaire Atelier (hérité de Claude) ; côté Codex le palier haut est
// 'xhigh'. Toute valeur inconnue retombe sur le défaut produit ('medium').
function clampEffort(e) {
  const v = typeof e === 'string' ? e.toLowerCase() : '';
  if (v === 'max') return 'xhigh';
  return EFFORTS.has(v) ? v : 'medium';
}

function trunc(s, n) {
  const str = typeof s === 'string' ? s : '';
  return str.length > n ? str.slice(0, n) : str;
}

// Cause LISIBLE d'un échec du CLI (WHY) : le SDK jette `Codex Exec exited with code N:
// <stderr intégral>`, et le CLI préfixe volontiers son stderr d'avertissements de
// plomberie (ex. « WARNING: proceeding, even though we could not create PATH aliases…
// Refusing to create helper binaries under temporary dir »). Tronquer les 200 premiers
// caractères ne remonterait QUE ce bruit et masquerait la vraie cause (vérifié : un
// auth.json invalide renvoyait le warning au lieu de « missing field id_token »). On
// retire donc le préfixe du SDK + les lignes d'avertissement, et on garde la FIN.
function cleanCause(msg) {
  const raw = String(msg ?? '').replace(/^Codex Exec exited with [^:]*:\s*/i, '');
  const lines = raw.split('\n').map((l) => l.trim()).filter((l) => l && !/^WARNING:/i.test(l));
  return trunc(lines.slice(-2).join(' — ') || raw.trim(), 200);
}

// ---------------------------------------------------------------------------
// Sidecar de persistance ($CODEX_HOME/atelier)
// WHY : le CLI persiste ses rollouts dans un format interne non documenté et
// instable ; le parser nous exposerait à une casse silencieuse à chaque bump de
// version. On tient donc NOTRE index (métadonnées de conversation) et NOS
// transcripts (items DÉJÀ normalisés, forme identique à fold_item côté Rust et
// appendEvent côté front) — c'est ce que `op:list`/`op:messages` servent.
// ---------------------------------------------------------------------------

function safeId(id) {
  return String(id || '').replace(/[^A-Za-z0-9._-]/g, '_');
}
function transcriptPath(threadId) {
  return join(TRANSCRIPT_DIR, `${safeId(threadId)}.ndjson`);
}

// Validation d'un sessionId venu du path param HTTP (WHY) : `DELETE …/conversations/{sid}`
// le transmet BRUT, et il pilote des opérations disque (chemin du transcript sidecar +
// balayage/suppression des rollouts du CLI). Un `..`, un `/` ou un simple fragment commun
// ferait sortir du sandbox ou emporterait les fichiers d'AUTRES conversations. On valide
// donc AVANT toute opération disque, pour TOUTES les ops qui prennent un sessionId
// (messages/rename/tag/delete), pas seulement delete.
const SESSION_ID_RE = /^[A-Za-z0-9][A-Za-z0-9._-]{7,}$/;
function assertSessionId(id) {
  const s = String(id || '');
  if (!s) throw new Error('sessionId manquant');
  if (s === '.' || s === '..' || !SESSION_ID_RE.test(s)) throw new Error(`sessionId invalide: ${trunc(s, 80)}`);
  return s;
}

// Validation de FORME d'un auth.json candidat (WHY) : le CLI accepte DEUX modes et sa
// struct AuthDotJson porte la clé API au PREMIER niveau — {auth_mode, OPENAI_API_KEY,
// tokens:{id_token, access_token, refresh_token, account_id}, last_refresh, agent_identity,
// personal_access_token, bedrock_api_key}. Un fichier en mode clé API passe donc toutes les
// gardes d'env (qui ne regardent que process.env) et ferait facturer une clé au lieu de
// l'abonnement. C'est la 2e porte d'entrée du fichier (l'autre est set_codex_auth côté
// Rust) : on refuse tout porteur de clé et on exige POSITIVEMENT les tokens OAuth.
function assertSubscriptionAuthJson(raw) {
  let parsed;
  try { parsed = JSON.parse(String(raw)); } catch { throw new Error('authJson n\'est pas du JSON valide'); }
  if (!parsed || typeof parsed !== 'object' || Array.isArray(parsed)) {
    throw new Error('authJson invalide : un objet JSON est attendu');
  }
  const filled = (v) => typeof v === 'string' && v.trim() !== '';
  for (const k of ['OPENAI_API_KEY', 'personal_access_token', 'bedrock_api_key', 'api_key']) {
    if (filled(parsed[k])) {
      throw new Error(`authJson refusé : ce fichier est en mode clé API (champ "${k}" renseigné) — le moteur Codex n'accepte que l'abonnement ChatGPT`);
    }
  }
  if (parsed.auth_mode != null && String(parsed.auth_mode).toLowerCase() !== 'chatgpt') {
    throw new Error(`authJson refusé : auth_mode="${trunc(String(parsed.auth_mode), 40)}" — ce fichier est en mode clé API, le moteur Codex n'accepte que l'abonnement ChatGPT`);
  }
  const tokens = parsed.tokens;
  if (!tokens || typeof tokens !== 'object' || Array.isArray(tokens) || !filled(tokens.access_token) || !filled(tokens.refresh_token)) {
    throw new Error('authJson refusé : tokens.access_token et tokens.refresh_token sont requis — colle le auth.json d\'un `codex login` par abonnement ChatGPT');
  }
  return parsed;
}

async function readIndex() {
  try {
    const parsed = JSON.parse(await fsp.readFile(INDEX_PATH, 'utf8'));
    return parsed && typeof parsed === 'object' && !Array.isArray(parsed) ? parsed : {};
  } catch {
    return {}; // absent / corrompu → repart d'un index vide (best-effort, non critique)
  }
}

// Écriture ATOMIQUE avec RELECTURE juste avant (WHY) : plusieurs sessions Codex
// tournent en parallèle et écrivent le même fichier. Garder l'index en mémoire et
// le réécrire en bloc écraserait les conversations des autres process. On sérialise
// aussi les écritures de CE process via une chaîne de promesses.
let indexChain = Promise.resolve();
function mutateIndex(fn) {
  indexChain = indexChain
    .then(async () => {
      const idx = await readIndex();
      fn(idx);
      const tmp = `${INDEX_PATH}.${process.pid}.tmp`;
      await fsp.writeFile(tmp, JSON.stringify(idx), { mode: 0o600 });
      await fsp.rename(tmp, INDEX_PATH);
    })
    .catch((e) => diag(`index.json : écriture échouée (${e?.message || e})`));
  return indexChain;
}

// `summaryIfEmpty` : le résumé n'est posé qu'à la CRÉATION de l'entrée (WHY) — chaque tour
// spawne un `codex exec … resume <id>` qui ré-émet `thread.started`, donc un patch
// `summary` inconditionnel remplacerait le titre de la conversation par le DERNIER message
// à chaque tour. `cwd` et `lastModified`, eux, continuent d'être rafraîchis à chaque tour.
function touchIndex(threadId, patch, summaryIfEmpty) {
  if (!threadId) return;
  mutateIndex((idx) => {
    const cur = idx[threadId] || { cwd: null, summary: '', customTitle: null, tag: null };
    const next = { ...cur, ...patch, lastModified: Date.now() };
    if (summaryIfEmpty && !next.summary) next.summary = summaryIfEmpty;
    idx[threadId] = next;
  });
}

// Append ligne à ligne, FLUSH IMMÉDIAT (écriture synchrone) : un SIGKILL du reaper ne
// doit pas emporter les items déjà rendus à l'utilisateur. Le lecteur tolère une
// dernière ligne tronquée (write interrompu en plein vol).
// WHY le tampon `pending` : sur une session NEUVE, l'id du thread n'existe qu'à
// `thread.started`, donc APRÈS le tour utilisateur qui l'a déclenché. Sans tampon, le
// tout premier message de chaque conversation manquerait au transcript rechargé.
const pending = [];
function writeTranscriptLine(threadId, item) {
  try {
    appendFileSync(transcriptPath(threadId), `${JSON.stringify(item)}\n`, { mode: 0o600 });
  } catch (e) {
    diag(`transcript : append échoué (${e?.message || e})`);
  }
}
function flushPending(threadId) {
  if (!threadId || !pending.length) return;
  for (const it of pending.splice(0)) writeTranscriptLine(threadId, it);
}
function appendTranscript(threadId, item) {
  if (!threadId) { pending.push(item); return; }
  flushPending(threadId); // ordre chronologique préservé
  writeTranscriptLine(threadId, item);
}

function readTranscript(threadId) {
  let raw;
  try {
    raw = readFileSync(transcriptPath(threadId), 'utf8');
  } catch {
    return []; // conversation sans transcript (jamais démarrée / purgée) = fil vide
  }
  const items = [];
  for (const line of raw.split('\n')) {
    const s = line.trim();
    if (!s) continue;
    try {
      items.push(JSON.parse(s));
    } catch {
      diag('transcript : dernière ligne invalide ignorée (écriture interrompue)');
    }
  }
  return items;
}

// ---------------------------------------------------------------------------
// Init stdin + canal de contrôle
// ---------------------------------------------------------------------------

const inputQ = []; // tours utilisateur en attente (un tour à la fois côté Codex)
let qResolve = null;
let inputClosed = false;
let onInit;
const initPromise = new Promise((r) => { onInit = r; });
let gotInit = false;

// État de session (rempli par le chemin chat)
let threadOptions = null;
let sessionId = null;
let liveMode = 'plan';
// Slug WIRE courant (`sol` ou `sol-fast`) : ce que l'UI/meta voient. `threadOptions.model`
// reste toujours le slug CLI réel (`sol`) ; l'axe fast vit sur l'instance Codex, pas dans
// threadOptions (le SDK n'a pas de champ service_tier). `codex`/`thread` sont recréés au
// toggle fast (cf. set_model).
let liveWire = DEFAULT_MODEL;
let liveFast = false;
let codex = null;
let thread = null;
let turnActive = false;
let currentAbort = null;
let interruptRequested = false;

function wake() {
  if (qResolve) { const r = qResolve; qResolve = null; r(); }
}
function pushTurn(turn) {
  inputQ.push(turn);
  wake();
}
function closeInput() {
  inputClosed = true;
  // Un arrêt demandé JETTE les tours non commencés (même comportement que le moteur
  // Claude). WHY : le drain d'Atelier = interrupt + EOF avec un budget de 45 s ; si la
  // file survivait à l'EOF, chaque tour en attente spawnerait encore un `codex exec` frais
  // APRÈS la fermeture de stdin → budget dépassé → SIGKILL → transcript tronqué.
  inputQ.length = 0;
  // …et AVORTE le tour EN VOL. Vider la file ne suffit pas (vérifié en rejeu) : le drain
  // écrit `{"type":"interrupt"}` PUIS ferme stdin, et entre les deux le tour avorté finit
  // sa boucle — le suivant est alors DÉJÀ sorti de la file (fenêtre mesurée ~15-35 ms, le
  // temps que l'abort tue le `codex exec` et que le générateur rende la main). Ce tour-là
  // spawnait un `codex exec` frais qui survivait à l'EOF : plus aucun EOF ne pouvait
  // l'arrêter, la session pendait jusqu'au SIGKILL du drain (transcript tronqué), soit
  // exactement ce que la purge de la file cherche à éviter. Aucun tour ne peut plus arriver
  // après un EOF : couper est toujours le bon verdict. Idempotent (abort() rejoué = no-op).
  if (turnActive && currentAbort) { interruptRequested = true; currentAbort.abort(); }
  wake();
}

const rl = createInterface({ input: process.stdin });
rl.on('line', (line) => {
  const s = line.trim();
  if (!s) return;
  let msg;
  try { msg = JSON.parse(s); } catch { diag(`ligne stdin non-JSON ignorée : ${s.slice(0, 200)}`); return; }
  if (!gotInit) { gotInit = true; onInit(msg); return; }
  switch (msg.type) {
    case 'user_message':
      if (typeof msg.text === 'string' || Array.isArray(msg.images)) {
        pushTurn({ text: msg.text == null ? '' : String(msg.text), images: msg.images });
      }
      break;
    // Codex n'a ni AskUserQuestion ni ExitPlanMode : le contrat HTTP est commun aux deux
    // moteurs, on accepte donc ces messages sans les traiter (jamais d'erreur au client).
    case 'answer':
    case 'plan_decision':
      diag(`message '${msg.type}' ignoré : Codex n'a pas de dialogue interactif`);
      break;
    // set_mode / set_model : `threadOptions` est relu par le SDK à CHAQUE tour → muter
    // l'objet suffit, mais l'effet est différé au tour suivant si un tour est en vol.
    case 'set_mode':
      if (threadOptions && (msg.mode === 'plan' || msg.mode === 'bypass')) {
        applyMode(msg.mode);
        emit({ t: 'permission_mode', mode: liveMode });
        if (turnActive) diag(`set_mode(${msg.mode}) : appliqué au prochain tour (tour en vol)`);
      }
      break;
    case 'set_model':
      if (threadOptions) {
        const dec = decodeModel(msg.model);
        if (dec.changed) diag(`set_model('${msg.model}') inconnu → coercé sur ${DEFAULT_MODEL}`);
        threadOptions.model = dec.model; // toujours sol côté CLI
        const fastChanged = dec.fast !== liveFast;
        liveFast = dec.fast;
        liveWire = dec.wire;
        // Le fast vit sur l'instance Codex → un changement de tier impose de recréer
        // codex + thread (resume sur le sessionId courant si le thread est déjà démarré).
        if (fastChanged) { codex = makeCodex(liveFast); bindThread(); }
        emit({ t: 'model', model: liveWire });
        if (turnActive) diag(`set_model(${liveWire}) : appliqué au prochain tour (tour en vol)`);
      }
      break;
    // Stop : tue le child `codex exec` du tour en cours ; la session (le thread) survit.
    case 'interrupt':
      if (turnActive && currentAbort) {
        interruptRequested = true;
        currentAbort.abort();
      }
      break;
    case 'end': closeInput(); break;
    default: diag(`type de message stdin inconnu: ${msg.type}`);
  }
});
rl.on('close', () => {
  diag('stdin EOF (rl close) → closeInput');
  // EOF avant une ligne d'init exploitable : on débloque l'attente avec `null` pour
  // échouer proprement (event `error` + exit 2) au lieu de rester pendu sur une
  // promesse jamais résolue (« unsettled top-level await », exit 13 sans NDJSON).
  if (!gotInit) { gotInit = true; onInit(null); }
  closeInput();
});

// Backstop signal (drain piloté par Atelier = interrupt + EOF sur stdin). On avorte le
// tour en vol puis on ferme l'entrée ; on NE fait PAS process.exit() pour laisser le
// `finally` du tour émettre result+turn_done et flush le transcript sidecar.
let shuttingDown = false;
function onShutdownSignal(sig) {
  if (shuttingDown) return;
  shuttingDown = true;
  diag(`signal ${sig} reçu → interrupt du tour + fin de session`);
  closeInput(); // avorte le tour en vol ET jette la file (invariant unique, cf. closeInput)
}
process.on('SIGTERM', () => onShutdownSignal('SIGTERM'));
process.on('SIGINT', () => onShutdownSignal('SIGINT'));

// Arborescence sidecar (0700 : elle contient des transcripts de conversation).
try {
  await fsp.mkdir(TRANSCRIPT_DIR, { recursive: true, mode: 0o700 });
} catch (e) {
  diag(`sidecar : mkdir échoué (${e?.message || e})`);
}

let init;
try {
  init = await initPromise;
  if (!init || typeof init !== 'object') await fail('init JSON invalide sur stdin.');
} catch (e) {
  await fail(`Init JSON invalide sur stdin : ${e?.message || e}`);
}

const { op, prompt, effort, cwd, permissionMode = 'plan', resume, model } = init || {};

// ---------------------------------------------------------------------------
// Ops one-shot (AVANT tout import du SDK : les ops disque doivent démarrer vite)
// ---------------------------------------------------------------------------

// Sortie flush-safe : un transcript volumineux dépasse le buffer du pipe et un
// process.exit() synchrone le tronquerait — on sort dans le callback du write.
function emitOnceAndExit(result) {
  rl.close();
  process.stdin.destroy();
  process.stdout.write(`${JSON.stringify(result)}\n`, () => process.exit(0));
  return new Promise(() => {}); // sortie EXCLUSIVE par le callback ci-dessus
}

// Purge best-effort du rollout interne du CLI (on ne connaît que la convention
// « l'id apparaît dans le nom de fichier »). Non bloquant : l'autorité de la liste
// est NOTRE index, un rollout orphelin n'est qu'un résidu disque.
// WHY la correspondance ANCRÉE (et pas `includes`) : un id qui serait le préfixe/fragment
// d'un autre emporterait les rollouts de conversations tierces. On n'accepte que le nom
// exact, le préfixe `id.` ou le suffixe `-id.jsonl` (convention `rollout-…-<uuid>.jsonl`).
function rolloutMatches(name, threadId) {
  return name === threadId || name.startsWith(`${threadId}.`) || name.endsWith(`-${threadId}.jsonl`);
}

async function removeRollout(threadId) {
  const root = join(CODEX_HOME, 'sessions');
  const stack = [root];
  let visited = 0;
  while (stack.length && visited < 10000) { // borne dure : jamais de balayage sans fin
    visited += 1;
    const dir = stack.pop();
    let entries;
    try { entries = await fsp.readdir(dir, { withFileTypes: true }); } catch { continue; }
    for (const e of entries) {
      const p = join(dir, e.name);
      if (e.isDirectory()) stack.push(p);
      else if (rolloutMatches(e.name, threadId)) {
        try { await fsp.rm(p, { force: true }); } catch { /* résidu sans conséquence */ }
      }
    }
  }
}

if (op && op !== 'auth_check') {
  await assertSubscriptionOnly(false);
  let result;
  try {
    if (op === 'list') {
      const idx = await readIndex();
      const sessions = Object.entries(idx)
        // Scope projet : la liste d'une app ne montre que SES conversations (comme
        // `listSessions({dir})` côté Claude).
        .filter(([, v]) => !cwd || !v?.cwd || v.cwd === cwd)
        .map(([sessionId_, v]) => ({
          sessionId: sessionId_,
          summary: v?.summary || '',
          customTitle: v?.customTitle ?? null,
          tag: v?.tag ?? null,
          lastModified: v?.lastModified || 0,
        }))
        .sort((a, b) => (b.lastModified || 0) - (a.lastModified || 0));
      result = { t: 'sessions', sessions };
    } else if (op === 'messages') {
      const sid = assertSessionId(init.sessionId);
      result = { t: 'transcript', items: readTranscript(sid) };
    } else if (op === 'rename') {
      const sid = assertSessionId(init.sessionId);
      await mutateIndex((idx) => {
        const cur = idx[sid] || { cwd: cwd || null, summary: '', tag: null };
        idx[sid] = { ...cur, customTitle: String(init.title || ''), lastModified: cur.lastModified || Date.now() };
      });
      result = { t: 'ok' };
    } else if (op === 'tag') {
      const sid = assertSessionId(init.sessionId);
      await mutateIndex((idx) => {
        const cur = idx[sid] || { cwd: cwd || null, summary: '', customTitle: null };
        idx[sid] = { ...cur, tag: init.tag ?? null, lastModified: cur.lastModified || Date.now() };
      });
      result = { t: 'ok' };
    } else if (op === 'delete') {
      // Validation AVANT toute opération disque (le sid vient du path param HTTP).
      const sid = assertSessionId(init.sessionId);
      await mutateIndex((idx) => { delete idx[sid]; });
      try { await fsp.rm(transcriptPath(sid), { force: true }); } catch { /* déjà absent */ }
      await removeRollout(sid);
      result = { t: 'ok' };
    } else if (op === 'set_auth_json') {
      if (!init.authJson) throw new Error('authJson manquant');
      // Le CLI meurt sur un auth.json invalide avec un message obscur, et un fichier en
      // mode clé API contournerait la garde abonnement → on valide la FORME ici (2e porte
      // d'entrée du fichier, cf. assertSubscriptionAuthJson) avant de l'écrire.
      assertSubscriptionAuthJson(init.authJson);
      await fsp.mkdir(CODEX_HOME, { recursive: true, mode: 0o700 });
      // chmod explicite APRÈS l'écriture : le `mode` de writeFile est masqué par l'umask
      // du process (0022 → 0644), or ce fichier porte le refresh token de l'abonnement.
      await fsp.writeFile(AUTH_PATH, String(init.authJson), { mode: 0o600 });
      await fsp.chmod(AUTH_PATH, 0o600);
      result = { t: 'ok' };
    } else if (op === 'clear_auth_json') {
      try { await fsp.rm(AUTH_PATH, { force: true }); } catch { /* déjà absent */ }
      result = { t: 'ok' };
    } else if (op === 'auth_status') {
      // Pas d'appel modèle : simple présence du fichier (le probe live, c'est auth_check).
      const present = existsSync(AUTH_PATH);
      result = { t: 'auth_status', auth_file: present, logged_in: present };
    } else {
      result = { t: 'error', message: `op inconnue: ${op}` };
    }
  } catch (e) {
    result = { t: 'error', message: `op ${op} échouée: ${e?.message || e}` };
  }
  await emitOnceAndExit(result);
}

// ---------------------------------------------------------------------------
// Import PARESSEUX du SDK : seuls les chemins qui appellent le modèle en ont besoin
// (les ops disque ci-dessus sortent sans jamais charger le SDK ni résoudre le binaire).
// ---------------------------------------------------------------------------
async function loadCodex() {
  const mod = await import('@openai/codex-sdk');
  return mod.Codex;
}

// Env transmis au CLI : le SDK n'hérite PAS de process.env dès qu'on fournit `env`
// (cf. exec.ts). On reconstruit donc un env COMPLET, purgé des clés API (garde
// abonnement) et pointé sur le CODEX_HOME voulu.
function childEnv(codexHome) {
  const env = {};
  for (const [k, v] of Object.entries(process.env)) {
    if (v === undefined) continue;
    if (AUTH_ENV_KEYS.includes(k)) continue;
    env[k] = v;
  }
  env.CODEX_HOME = codexHome;
  return env;
}

// Sonde d'auth one-shot : mini-tour RÉEL (un `codex login status` ne dirait rien d'un
// refresh token révoqué côté serveur). Avec `authJson`, la validation est ISOLÉE dans un
// CODEX_HOME temporaire : sinon un auth.json réel déjà présent masquerait un candidat
// invalide (piège vécu côté token apps Claude).
if (op === 'auth_check') {
  await assertSubscriptionOnly(!init.authJson);
  const Codex = await loadCodex();
  let homeDir = null;
  let workDir = null;
  const ac = new AbortController();
  const timer = setTimeout(() => ac.abort(), 90_000);
  let out;
  try {
    if (init.authJson) {
      // Défense en profondeur (WHY) : c'est la 2e porte d'entrée du fichier au même titre
      // que `op:set_auth_json`. Sans cette validation, un auth.json en mode clé API serait
      // validé PAR SA CLÉ par le mini-tour ci-dessous et renverrait `auth_ok` — le candidat
      // partirait ensuite en base comme s'il s'agissait d'un abonnement. Inatteignable
      // aujourd'hui (le Rust valide en amont), mais la garde ne doit pas dépendre de ça.
      assertSubscriptionAuthJson(init.authJson);
      homeDir = mkdtempSync(join(tmpdir(), 'atelier-codexauth-'));
      await fsp.writeFile(join(homeDir, 'auth.json'), String(init.authJson), { mode: 0o600 });
      await fsp.chmod(join(homeDir, 'auth.json'), 0o600);
    }
    workDir = mkdtempSync(join(tmpdir(), 'atelier-codexprobe-'));
    const codex = new Codex({ env: childEnv(homeDir || CODEX_HOME) });
    const thread = codex.startThread({
      model: DEFAULT_MODEL,
      modelReasoningEffort: 'low',
      sandboxMode: 'read-only',
      networkAccessEnabled: false,
      approvalPolicy: 'never',
      skipGitRepoCheck: true,
      workingDirectory: workDir,
    });
    const { events } = await thread.runStreamed('Réponds exactement: ok', { signal: ac.signal });
    let ok = false;
    let detail = '';
    for await (const ev of events) {
      if (ev.type === 'turn.completed') { ok = true; break; }
      if (ev.type === 'turn.failed') { detail = ev.error?.message || 'turn.failed'; break; }
      if (ev.type === 'error') { detail = ev.message || 'error'; break; }
    }
    out = ok
      ? { t: 'auth_ok' }
      : CODEX_AUTH_RE.test(detail)
        ? { t: 'error', code: 'sdk_auth_failed', message: cleanCause(detail) }
        : { t: 'error', message: `auth_check: ${cleanCause(detail) || 'échec inconnu'}` };
  } catch (e) {
    // Le test d'auth porte sur le message BRUT (la cause peut être n'importe où) ; seul
    // l'affichage est nettoyé.
    const raw = String(e?.message || e);
    out = CODEX_AUTH_RE.test(raw)
      ? { t: 'error', code: 'sdk_auth_failed', message: cleanCause(raw) }
      : { t: 'error', message: `auth_check: ${cleanCause(raw) || 'échec inconnu'}` };
  }
  // Nettoyage AVANT l'émission : `emitOnceAndExit` ne rend jamais la main (il sort dans
  // le callback du write) — un `finally` placé après ne s'exécuterait JAMAIS et laisserait
  // le CODEX_HOME temporaire (qui contient un auth.json candidat) sur le disque.
  clearTimeout(timer);
  for (const d of [homeDir, workDir]) {
    if (d) { try { await fsp.rm(d, { recursive: true, force: true }); } catch { /* /tmp */ } }
  }
  await emitOnceAndExit(out);
}

// ---------------------------------------------------------------------------
// Mode session (chat)
// ---------------------------------------------------------------------------

await assertSubscriptionOnly(true);

// prompt vide TOLÉRÉ si des images sont jointes (tour image-only), comme runner.js.
if (!prompt && !(Array.isArray(init.images) && init.images.length)) {
  await fail('Champ "prompt" manquant dans l\'init.');
}

// Deux modes produit, mappés sur le SANDBOX du CLI (Codex n'a pas de permissionMode) :
//   - 'plan' (défaut, SÛR) : sandbox 'read-only' + réseau coupé → le modèle peut lire et
//     raisonner, jamais écrire ni sortir. C'est l'équivalent fonctionnel du plan-mode SDK.
//   - 'bypass' : sandbox DÉSACTIVÉ ('danger-full-access') + réseau autorisé.
// `approvalPolicy:'never'` dans les deux cas : aucun humain sur le TTY du CLI, toute
// demande d'approbation bloquerait le tour jusqu'au timeout.
//
// WHY 'danger-full-access' et pas 'workspace-write' en bypass : le sandbox du CLI
// force `.git` en LECTURE SEULE — et ce montage est appliqué APRÈS les `writable_roots`,
// donc ni `writable_roots` ni `--add-dir` ne peuvent l'outrepasser (bugs amont ouverts
// openai/codex #7071, #14338, #15505). Tout `git add`/`commit`/`fetch` échoue alors sur
// « Unable to create .git/index.lock: Read-only file system ». Observé en vrai : l'agent
// a contourné en clonant dans /tmp pour committer, laissant l'index du workspace
// désynchronisé du remote — pire que pas de sandbox du tout.
// Le périmètre de confiance ne change pas pour autant : en bypass, l'agent Claude tourne
// DÉJÀ sans aucun sandbox OS (mode `acceptEdits`, Bash libre en `hr-studio`). Les deux
// moteurs ont donc la même frontière — le compte `hr-studio` — et le mode plan reste, lui,
// strictement confiné côté Codex.
function applyMode(mode) {
  liveMode = mode === 'bypass' ? 'bypass' : 'plan';
  threadOptions.sandboxMode = liveMode === 'bypass' ? 'danger-full-access' : 'read-only';
  threadOptions.networkAccessEnabled = liveMode === 'bypass';
}

const initModel = decodeModel(model);
if (initModel.changed) diag(`modèle '${model}' inconnu → coercé sur ${DEFAULT_MODEL}`);
liveWire = initModel.wire;
liveFast = initModel.fast;
threadOptions = {
  model: initModel.model, // slug CLI réel (toujours sol) ; le fast passe par le service tier
  modelReasoningEffort: clampEffort(effort),
  workingDirectory: cwd,
  // Le workspace d'une app n'est pas toujours un dépôt git (le bare repo vit ailleurs) :
  // sans ce flag le CLI refuse de tourner hors dépôt.
  skipGitRepoCheck: true,
  approvalPolicy: 'never',
};
applyMode(permissionMode === 'bypassPermissions' ? 'bypass' : 'plan');

const Codex = await loadCodex();
// Fabrique l'instance Codex pour le service tier voulu. WHY sur l'instance et pas dans
// threadOptions : le SDK 0.144.6 n'a pas de champ service_tier ; `CodexOptions.config` est
// aplati en `--config key=value` à chaque spawn `codex exec`. `fast` → tier officiel Codex
// (même modèle sol, ~1.5× plus rapide). Un rebuild recrée aussi le thread (resume si connu).
function makeCodex(fast) {
  const opts = { env: childEnv(CODEX_HOME) };
  if (fast) opts.config = { service_tier: 'fast', features: { fast_mode: true } };
  return new Codex(opts);
}
function bindThread() {
  // sessionId connu (thread déjà démarré, ou resume) → on reprend ; sinon nouveau thread.
  thread = sessionId ? codex.resumeThread(sessionId, threadOptions) : codex.startThread(threadOptions);
}
codex = makeCodex(liveFast);
if (resume) sessionId = resume;
bindThread();

// Mode initial AVANT tout (invariant partagé avec runner.js : l'UI et le buffer backend
// connaissent la vérité terrain dès le départ).
emit({ t: 'permission_mode', mode: liveMode });

let sessionEmitted = false;
function emitSystem(id) {
  if (sessionEmitted) return;
  sessionEmitted = true;
  sessionId = id;
  // `liveWire` (sol / sol-fast), PAS threadOptions.model (toujours sol) : c'est le slug wire
  // que la meta Rust persiste et que le front lit pour restaurer le chip Fast.
  emit({ t: 'system', subtype: 'init', session_id: id, model: liveWire });
  flushPending(id); // items écrits avant que l'id du thread n'existe (1er tour utilisateur)
}
// Reprise : le binding run_id↔session_id côté Rust dépend de l'event `system`, or
// `thread.started` n'arrive qu'au premier tour (donc après le seed du buffer). On connaît
// déjà l'id → on l'émet IMMÉDIATEMENT.
if (resume) emitSystem(resume);

// --- Streaming par diff de préfixe -----------------------------------------
// Les items agent_message/reasoning portent le texte CUMULÉ à chaque item.updated (pas un
// delta) : on garde le dernier texte vu par item.id et on n'émet que le suffixe. Une
// réécriture non-préfixe (le modèle réécrit son message) est signalée en diag et
// resynchronise sur un saut de ligne plutôt que de rejouer tout le texte en double.
const streamed = new Map(); // item.id → dernier texte vu
function streamDelta(id, text) {
  const t = typeof text === 'string' ? text : '';
  const prev = streamed.get(id) || '';
  if (t === prev) return '';
  streamed.set(id, t);
  if (t.startsWith(prev)) return t.slice(prev.length);
  diag(`réécriture non-préfixe de l'item ${id} → resynchronisation`);
  return `\n${t}`;
}

// --- Mapping des items Codex vers le vocabulaire d'outils du front ----------
// Le rendu (web/src/lib/toolDisplay.js) connaît Read/Write/Edit/Bash/Glob/Grep/
// WebSearch/TodoWrite/Task + les `mcp__serveur__outil`. On mappe donc les items Codex
// sur ces noms-là pour hériter des icônes et libellés existants, plutôt que d'inventer
// des noms qui retomberaient sur le rendu générique key:value.
// `emittedTools` dédoublonne l'émission d'un tool_use quand `item.started` ET
// `item.completed` portent le même item (et permet une émission tardive si le `started`
// a été manqué). Vidé À CHAQUE TOUR : la corrélation tool_use↔tool_result est INTRA-tour,
// et un id d'item réutilisé au tour suivant produirait sinon un tool_result orphelin
// (résultat rendu sans son appel) — vérifié en rejeu de séquence.
const emittedTools = new Set();

function emitTool(id, name, input) {
  if (emittedTools.has(id)) return;
  emittedTools.add(id);
  emit({ t: 'tool_use', id, name, input });
  appendTranscript(sessionId, { type: 'tool_use', id, name, input });
}
function emitToolResult(id, isError, text) {
  const body = trunc(text, MAX_TOOL_RESULT);
  emit({ t: 'tool_result', tool_use_id: id, is_error: !!isError, text: body });
  // `isError` (et non `is_error`) : forme des items normalisés attendue par le front
  // (appendEvent) et par fold_item côté Rust. On garde les deux clés pour que le
  // transcript relu soit interchangeable avec le flux live.
  appendTranscript(sessionId, { type: 'tool_result', tool_use_id: id, isError: !!isError, is_error: !!isError, text: body });
}

let lastTodoSig = null;
let todoSeq = 0;

function handleItem(ev, item) {
  const phase = ev.type; // item.started | item.updated | item.completed
  switch (item.type) {
    case 'agent_message': {
      const d = streamDelta(item.id, item.text);
      if (d) emit({ t: 'assistant_delta', text: d });
      if (phase === 'item.completed') {
        streamed.delete(item.id);
        if (item.text) appendTranscript(sessionId, { type: 'assistant', text: item.text });
      }
      break;
    }
    case 'reasoning': {
      const d = streamDelta(item.id, item.text);
      // Le TEXTE de réflexion ne quitte jamais le runner côté Claude ; ici il transite
      // par le pipe interne runner→API, qui n'en diffuse que le compteur de caractères.
      if (d) emit({ t: 'thinking_delta', text: d });
      if (phase === 'item.completed') {
        streamed.delete(item.id);
        if (item.text) appendTranscript(sessionId, { type: 'thinking', chars: item.text.length });
      }
      break;
    }
    case 'command_execution': {
      emitTool(item.id, 'Bash', { command: trunc(item.command, 300) });
      if (phase === 'item.completed') {
        const failed = item.status === 'failed' || (item.exit_code !== undefined && item.exit_code !== 0);
        emitToolResult(item.id, failed, item.aggregated_output || '');
      }
      break;
    }
    case 'file_change': {
      // Émis une seule fois (patch terminé) : un tool_use par fichier touché, pour que
      // le fil montre la liste des fichiers comme avec Write/Edit côté Claude.
      if (phase !== 'item.completed') break;
      const changes = Array.isArray(item.changes) ? item.changes : [];
      changes.forEach((ch, i) => {
        const id = `${item.id}#${i}`;
        const name = ch.kind === 'add' ? 'Write' : 'Edit';
        const input = ch.kind === 'update' ? { file_path: ch.path } : { file_path: ch.path, kind: ch.kind };
        emitTool(id, name, input);
        emitToolResult(id, item.status === 'failed', ch.kind || 'update');
      });
      break;
    }
    case 'mcp_tool_call': {
      emitTool(item.id, `mcp__${item.server}__${item.tool}`, item.arguments);
      if (phase === 'item.completed') {
        const text = item.error?.message || toolResultText(item.result?.content) || '';
        emitToolResult(item.id, !!item.error || item.status === 'failed', text);
      }
      break;
    }
    case 'web_search': {
      emitTool(item.id, 'WebSearch', { query: item.query });
      if (phase === 'item.completed') emitToolResult(item.id, false, item.query || '');
      break;
    }
    case 'todo_list': {
      // Codex ne donne qu'un booléen `completed` par entrée (pas d'état « en cours ») ;
      // la bannière du front attend {content, status, activeForm}. Dédup par signature :
      // item.updated est émis à chaque micro-changement, on n'ajoute une ligne au fil que
      // si la liste a réellement changé.
      const todos = (Array.isArray(item.items) ? item.items : []).map((x) => ({
        content: x?.text || '',
        activeForm: x?.text || '',
        status: x?.completed ? 'completed' : 'pending',
      }));
      const sig = JSON.stringify(todos);
      if (sig === lastTodoSig) break;
      lastTodoSig = sig;
      emitTool(`${item.id}#todo${++todoSeq}`, 'TodoWrite', { todos });
      break;
    }
    case 'error': {
      const message = item.message || 'erreur';
      if (CODEX_AUTH_RE.test(message)) reportCodexAuth(`item.error: ${message}`);
      else {
        emit({ t: 'error', message });
        appendTranscript(sessionId, { type: 'error', message });
      }
      break;
    }
    default:
      diag(`item Codex non mappé : ${item.type}`);
      break;
  }
}

// --- Images ----------------------------------------------------------------
const IMG_EXT = { 'image/png': '.png', 'image/jpeg': '.jpg', 'image/jpg': '.jpg', 'image/gif': '.gif', 'image/webp': '.webp' };
// Codex ne consomme les images que par CHEMIN de fichier ({type:'local_image'}), pas en
// base64 inline : on matérialise chaque image dans un dossier temp, nettoyé après le tour.
async function buildInput(turn) {
  const text = turn.text || '';
  const images = Array.isArray(turn.images) ? turn.images.filter((i) => i && i.media_type && i.data) : [];
  if (!images.length) return { input: text, cleanup: async () => {} };
  let dir = null;
  const parts = [];
  if (text.trim()) parts.push({ type: 'text', text });
  try {
    dir = await fsp.mkdtemp(join(tmpdir(), 'atelier-codeximg-'));
    for (let i = 0; i < images.length; i += 1) {
      const p = join(dir, `img${i}${IMG_EXT[images[i].media_type] || '.png'}`);
      await fsp.writeFile(p, Buffer.from(images[i].data, 'base64'), { mode: 0o600 });
      parts.push({ type: 'local_image', path: p });
    }
  } catch (e) {
    diag(`images : matérialisation échouée (${e?.message || e}) — tour envoyé en texte seul`);
  }
  const cleanup = async () => {
    if (dir) { try { await fsp.rm(dir, { recursive: true, force: true }); } catch { /* /tmp */ } }
  };
  if (!parts.some((p) => p.type === 'local_image')) return { input: text, cleanup };
  return { input: parts, cleanup };
}

// --- Boucle de tours -------------------------------------------------------

async function nextTurn() {
  for (;;) {
    // `inputClosed` TESTÉ EN PREMIER : la fermeture prime sur la file (cf. closeInput()).
    if (inputClosed) return null;
    if (inputQ.length) return inputQ.shift();
    await new Promise((r) => { qResolve = r; });
  }
}

// INVARIANT ABSOLU (partagé avec runner.js) : tout tour se termine par `result` PUIS
// `turn_done`, exactement une fois, quoi qu'il arrive (succès, échec, interrupt,
// exception, shutdown). Le front reste sinon bloqué en « running ».
let turnEnded = true;
function endTurn(subtype, isError, usage, startedAt) {
  if (turnEnded) return;
  turnEnded = true;
  const data = {
    subtype,
    is_error: !!isError,
    session_id: sessionId,
    usage: {
      input_tokens: usage?.input_tokens ?? 0,
      output_tokens: usage?.output_tokens ?? 0,
      cache_read_input_tokens: usage?.cached_input_tokens ?? 0,
      // Codex ne facture pas d'écriture de cache : champ présent (contrat commun) à 0.
      cache_creation_input_tokens: 0,
    },
    num_turns: 1,
    duration_ms: Date.now() - startedAt,
  };
  emit({ t: 'result', ...data });
  appendTranscript(sessionId, { type: 'result', data });
  turnActive = false;
  emit({ t: 'turn_done' });
}

async function runTurn(turn) {
  const startedAt = Date.now();
  turnActive = true;
  turnEnded = false;
  interruptRequested = false;
  currentAbort = new AbortController();
  emittedTools.clear();
  streamed.clear();
  const { input, cleanup } = await buildInput(turn);
  let usage = null;
  let completed = false; // un `turn.completed` a été vu → succès
  let lastError = null; // dernier message d'erreur non-auth vu (dédupliqué)
  // Les events `{type:'error'}` du flux ne sont PAS fatals en pratique : un run non
  // authentifié en émet ~12 (« Reconnecting… 1/5 (401 Unauthorized) », bascule
  // WebSocket→HTTPS) avant le vrai `turn.failed` terminal. Les traiter comme une fin de
  // tour clôturerait le tour au premier retry, puis on ignorerait le verdict réel.
  const noteError = (m) => {
    const message = m || 'erreur';
    if (CODEX_AUTH_RE.test(message)) { reportCodexAuth(message); return; }
    if (message === lastError) return; // retries répétés : une seule ligne dans le fil
    lastError = message;
    emit({ t: 'error', message });
    appendTranscript(sessionId, { type: 'error', message });
  };
  try {
    const { events } = await thread.runStreamed(input, { signal: currentAbort.signal });
    for await (const ev of events) {
      switch (ev.type) {
        case 'thread.started':
          emitSystem(ev.thread_id);
          // Seed de l'index dès que l'id existe (le résumé de la conversation = début du
          // 1er prompt, comme le `summary` des sessions Claude) — posé UNE SEULE FOIS, à la
          // création : `thread.started` est ré-émis à chaque tour (chaque tour = un
          // `codex exec … resume <id>`), un patch inconditionnel écraserait le titre.
          touchIndex(sessionId, { cwd: cwd || null }, trunc(turn.text || '', SUMMARY_LEN));
          break;
        case 'turn.started':
          break;
        case 'item.started':
        case 'item.updated':
        case 'item.completed':
          if (ev.item) handleItem(ev, ev.item);
          break;
        case 'turn.completed':
          usage = ev.usage || null;
          completed = true;
          break;
        // Verdict TERMINAL du tour. Le générateur jette ensuite (`codex exec` sort en 1) :
        // le flag `turnEnded` garantit qu'on n'émet pas un deuxième couple result+turn_done.
        case 'turn.failed': {
          const failure = ev.error?.message || 'turn.failed';
          // VERDICT du tour : c'est le seul canal (avec l'exception terminale ci-dessous)
          // qui prouve que l'auth est morte, par opposition aux events `error` de retry.
          if (CODEX_AUTH_RE.test(failure)) authFatal = true;
          noteError(failure);
          endTurn('error_during_execution', true, usage, startedAt);
          break;
        }
        case 'error':
          noteError(ev.message);
          break;
        default:
          diag(`event Codex non mappé : ${ev.type}`);
          break;
      }
    }
    // Flux terminé sans `turn.completed` ni `turn.failed` : si des erreurs ont défilé,
    // ne PAS conclure au succès (le front afficherait un tour vert sur un tour mort).
    endTurn(completed || !lastError ? 'success' : 'error_during_execution', !completed && !!lastError, usage, startedAt);
  } catch (e) {
    // Le générateur du SDK JETTE quand `codex exec` sort non-zéro (message = stderr) ou
    // quand l'AbortSignal tue le child. Un interrupt utilisateur n'est PAS une erreur :
    // on ne pollue pas le fil d'un item rouge (le front affiche « interrompu » via le
    // subtype du result) — contrairement à un vrai échec.
    const m = String(e?.message || e);
    const aborted = interruptRequested || e?.name === 'AbortError';
    // Verdict terminal du générateur (exit non nul du CLI) : 2e — et dernier — point
    // d'armement de `authFatal`. Le garde `!aborted` est essentiel : un Stop utilisateur
    // pendant une bascule transitoire ne doit JAMAIS conclure à une auth morte.
    if (!aborted && CODEX_AUTH_RE.test(m)) authFatal = true;
    if (aborted) {
      diag(`tour interrompu : ${trunc(m, 160)}`);
      endTurn('interrupted', false, usage, startedAt);
    } else if (turnEnded) {
      // `turn.failed` a DÉJÀ clos le tour et rapporté la cause ; l'exception qui suit
      // (exit code non nul du CLI) est le même échec vu par un autre canal — la réémettre
      // ajouterait une carte d'erreur APRÈS le turn_done.
      diag(`exception post-verdict ignorée : ${cleanCause(m)}`);
    } else {
      noteError(cleanCause(m) ? `codex exec a échoué : ${cleanCause(m)}` : m);
      endTurn('error_during_execution', true, usage, startedAt);
    }
  } finally {
    endTurn('error_during_execution', true, usage, startedAt); // filet : jamais de tour sans fin
    // RÉARMEMENT (WHY) : un tour ABOUTI prouve que l'auth est vivante — le 401 qui a pu
    // défiler avant n'était qu'un retour de bascule WebSocket→HTTPS. On relâche donc le
    // verdict ET le once-only du signalement, sans quoi une VRAIE expiration survenant
    // plus tard dans la même session ne serait jamais notifiée à l'utilisateur (et un
    // échec ultérieur sans rapport hériterait d'un diagnostic « auth morte » trompeur).
    if (completed) { authFatal = false; authReported = false; }
    currentAbort = null;
    turnActive = false;
    streamed.clear();
    touchIndex(sessionId, {});
    await cleanup();
  }
}

// Le prompt d'init = premier tour de la session (avec ses images éventuelles).
pushTurn({ text: prompt == null ? '' : String(prompt), images: init.images });

for (;;) {
  const turn = await nextTurn();
  if (!turn) break; // EOF / {type:'end'} = fin de session
  // Le tour utilisateur est appendé au sidecar : le driver Rust seed le sien dans son
  // buffer MÉMOIRE, mais `op:messages` relit CE fichier après un reload — sans ça, les
  // questions de l'utilisateur disparaîtraient du fil rechargé.
  appendTranscript(sessionId, { type: 'user', text: turn.text || '' });
  await runTurn(turn);
  // Session SANS AVENIR (WHY) : agent.rs tient stdin OUVERT pour toute la vie de la
  // conversation, donc un runner qui attend un tour de plus n'est reapé qu'au timeout
  // d'inactivité (`ATELIER_AGENT_IDLE_SECS`, défaut 1800 s) — une entrée RUNS orpheline
  // pendant 30 min, et un fil bloqué en « running » côté UI. On ferme donc l'entrée dès
  // qu'aucun tour suivant ne peut aboutir → sortie propre → `done` côté agent.rs.
  // Deux cas terminaux :
  //   1. aucun `thread.started` : le binding conversation↔session côté Rust dépend de
  //      l'event `system` ; le CLI est mort avant (config invalide, binaire absent) et
  //      aucune session n'est liée.
  //   2. `authFatal` — VERDICT d'auth morte (et non simple signalement) : chaque tour
  //      spawne un `codex exec` FRAIS qui relit le MÊME auth.json → il échouerait à
  //      l'identique. Le cas 1 ne suffit PAS (vérifié sur CLI 0.144.6, refresh token
  //      périmé) : le thread démarre quand même, `system` est émis, PUIS le tour meurt en
  //      401 — la session restait pendue 1800 s. On lit `authFatal` et NON `authReported`
  //      pour ne pas fermer une session SAINE dans deux cas réels : (a) Stop utilisateur
  //      pressé pendant une bascule WebSocket→HTTPS transitoire (tour non abouti, mais
  //      l'auth va très bien) ; (b) 401 transitoire au tour 1 suivi, plus tard, d'un échec
  //      de cause tout autre (sandbox, exit non nul) qui hériterait du diagnostic.
  //      Se reconnecter reste possible : le tour suivant repart d'un runner neuf (resume),
  //      qui relit auth.json au spawn.
  if (!sessionEmitted || authFatal) {
    diag(authFatal
      ? 'authentification Codex morte → fin de session immédiate (tout tour suivant échouerait pareil)'
      : 'aucun thread.started (le CLI a échoué avant le binding) → fin de session immédiate');
    closeInput();
  }
}

// Fin de session : on libère stdin pour que le process sorte et que le backend voie
// l'EOF de stdout. Les écritures d'index en vol sont attendues (flush du sidecar).
await indexChain;
rl.close();
process.stdin.destroy();
