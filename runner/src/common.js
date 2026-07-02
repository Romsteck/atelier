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
export async function assertOAuthOnly(label, fail) {
  if (process.env.ANTHROPIC_API_KEY) {
    await fail(`ANTHROPIC_API_KEY présent dans l'env : le ${label} doit utiliser l'OAuth abonnement (hr-studio), pas une clé API. Abandon.`);
  }
  const configDir = process.env.CLAUDE_CONFIG_DIR || join(process.env.HOME || '', '.claude');
  if (!existsSync(join(configDir, '.credentials.json'))) {
    await fail(`Credentials OAuth introuvables sous ${configDir}/.credentials.json — le ${label} doit tourner en hr-studio (login claude déjà présent).`);
  }
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
