// Plomberie partagée entre runner.js (agent Studio) et scan.js (surveillance).
// WHY : chaque runner avait sa copie d'emit/diag/fail + gardes auth + wiring MCP,
// et les copies ont divergé silencieusement (deux bugs distincts sur fail(),
// corrigés le 2026-07-02). Une seule implémentation canonique désormais.
import { existsSync } from 'node:fs';
import { join } from 'node:path';

// stdout = canal d'events NDJSON ; stderr = diagnostics uniquement (jamais parsé).
export function makeIo(tag) {
  const emit = (obj) => {
    process.stdout.write(JSON.stringify(obj) + '\n');
  };
  const diag = (msg) => {
    process.stderr.write(`[${tag}] ${msg}\n`);
  };
  // Terminal ET flush-safe : sort dans le callback du write (un process.exit synchrone
  // peut jeter la ligne d'erreur encore en vol sur le pipe → le driver Rust ne verrait
  // jamais la cause), et ne retourne JAMAIS (promesse non résolue) — chaque garde fait
  // `await fail(...)`, qui stoppe donc le flux comme un vrai exit.
  const fail = (message, code = 2) => {
    diag(message);
    process.stdout.write(JSON.stringify({ t: 'error', message }) + '\n', () => process.exit(code));
    return new Promise(() => {});
  };
  return { emit, diag, fail };
}

// Gardes auth (WHY) : une ANTHROPIC_API_KEY dans l'env bascule SILENCIEUSEMENT le SDK
// en facturation clé API au lieu de l'OAuth abonnement Max20x ; un fichier de creds
// manquant produit un 401 opaque. On échoue fort et tôt plutôt qu'en vol.
// `label` = nom du runner dans le message ("runner" | "scan runner").
// ⚠️ ORDRE : appeler cette garde APRÈS avoir parsé l'init stdin et posé
// `process.env.CLAUDE_CODE_OAUTH_TOKEN` (le token longue durée `setup-token`
// injecté par Atelier via stdin) — sinon la garde s'exécute avant que le token
// soit connu et échoue à tort quand aucun `.credentials.json` n'est présent.
export async function assertOAuthOnly(label, fail) {
  if (process.env.ANTHROPIC_API_KEY) {
    await fail(`ANTHROPIC_API_KEY présent dans l'env : le ${label} doit utiliser l'OAuth abonnement (hr-studio), pas une clé API. Abandon.`);
  }
  // Un token longue durée `claude setup-token` (CLAUDE_CODE_OAUTH_TOKEN) EST une
  // source d'auth abonnement valide à lui seul (reconnu top-level par le SDK) :
  // dans ce cas plus besoin d'un `.credentials.json` sur disque.
  if (process.env.CLAUDE_CODE_OAUTH_TOKEN) return;
  const configDir = process.env.CLAUDE_CONFIG_DIR || join(process.env.HOME || '', '.claude');
  if (!existsSync(join(configDir, '.credentials.json'))) {
    await fail(`Ni CLAUDE_CODE_OAUTH_TOKEN ni credentials OAuth (${configDir}/.credentials.json) — le ${label} doit tourner en hr-studio (login claude déjà présent) ou recevoir un token via Atelier (Paramètres → Authentification Claude).`);
  }
}

// Détection de l'échec d'auth OAuth abonnement du SDK (token longue durée mort/
// révoqué). Distincte de l'auth MCP (MCP_AUTH_RE ci-dessous) : ici c'est l'auth
// Anthropic elle-même. `authentication_failed`/`oauth_org_not_allowed` sont des
// valeurs de l'enum SDKAssistantMessageError (match EXACT sur le champ msg.error) ;
// la regex couvre le texte libre (result.errors[], exceptions jetées). On EXCLUT
// volontairement rate_limit/overloaded/billing_error (transitoires / non-auth).
export const SDK_AUTH_ERRORS = new Set(['authentication_failed', 'oauth_org_not_allowed']);
// Inclut le message TERRAIN d'un token mort observé sur le binaire natif 0.3.204 :
// « Not logged in · Please run /login » (l'accessToken local n'est pas expiré mais
// le serveur le rejette). Couvre aussi la formule de refresh échoué (invalid_grant/reauth).
export const SDK_AUTH_RE = /\bauthentication_failed\b|\boauth_org_not_allowed\b|OAuth token (?:has )?(?:expired|revoked)|invalid[ _-]?(?:oauth|bearer)[ _-]?token|invalid_grant|not logged in|please run \/login|\breauth\b/i;

// Reporter "une seule fois" : émet `{t:'error', code:'sdk_auth_failed', message}`
// (typé, calqué sur le précédent `mcp_auth_failed`). Le driver Rust (agent.rs /
// claude.rs) reconnaît ce code, marque le run FAILED et remonte UNE notification
// plateforme (dédup côté DB). `detail` = provenance (diagnostic, non secret).
export function makeSdkAuthReporter(emit) {
  let reported = false;
  return (detail) => {
    if (reported) return;
    reported = true;
    emit({
      t: 'error',
      code: 'sdk_auth_failed',
      message: `Claude SDK authentication_failed (${detail}) — token OAuth abonnement expiré/révoqué. Renouvelle-le via \`claude setup-token\` puis Paramètres → Authentification Claude.`,
    });
  };
}

// MCP (WHY) : le token arrive par l'init (stdin), pas par l'env — pour que sudo ne le
// journalise pas. Fallback env pour le smoke-test standalone.
export function buildMcpServers(mcpEndpoint, mcpToken, diag, missingTokenMsg) {
  const servers = {};
  const token = mcpToken || process.env.MCP_TOKEN;
  if (mcpEndpoint && token) {
    servers.studio = { type: 'http', url: mcpEndpoint, headers: { Authorization: `Bearer ${token}` } };
  } else if (mcpEndpoint) {
    diag(missingTokenMsg);
  }
  return servers;
}

export function toolResultText(content) {
  if (typeof content === 'string') return content;
  if (Array.isArray(content)) {
    return content.map((x) => (x && x.type === 'text' ? x.text : `[${x?.type || 'block'}]`)).join('');
  }
  return '';
}
