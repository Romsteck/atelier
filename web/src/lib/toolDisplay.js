// Mise en forme par TYPE d'outil des appels d'agent (Read/Write/Bash/Edit/Agent/…).
// Logique pure, sans JSX : `describeTool` produit le descripteur de l'en-tête,
// `formatToolResult` produit le descripteur du résultat (résumé + corps + drapeaux
// de rendu). Le composant AgentPanel mappe `iconKey`→icône lucide et matérialise les
// cas riches (diff, liste de todos, markdown). Cf. plan « formatage par type d'outil ».

// --- Helpers -----------------------------------------------------------------

// Coupe un chemin en { dir, base } sur le dernier séparateur. Pas de slash → tout en base.
export function splitPath(p) {
  const s = String(p ?? '');
  const i = Math.max(s.lastIndexOf('/'), s.lastIndexOf('\\'));
  return i < 0 ? { dir: '', base: s } : { dir: s.slice(0, i), base: s.slice(i + 1) };
}

// Troncature « douce » : coupe à `max`, retire un éventuel demi-mot final, ajoute « … ».
export function truncate(s, max = 90) {
  const str = String(s ?? '');
  if (str.length <= max) return str;
  let cut = str.slice(0, max - 1);
  const sp = cut.lastIndexOf(' ');
  if (sp > max * 0.6) cut = cut.slice(0, sp);
  return cut + '…';
}

// Replie un texte multi-lignes sur une seule (pour les en-têtes) : \n → ⏎.
export function oneLine(s) {
  return String(s ?? '').replace(/\s*\n\s*/g, ' ⏎ ').trim();
}

// Nombre de lignes (ignore une ligne vide finale).
export function countLines(text) {
  if (!text) return 0;
  const n = String(text).split(/\r\n|\r|\n/).length;
  return String(text).endsWith('\n') ? n - 1 : n;
}

// Taille lisible (FR) depuis un nombre de caractères. Ordre de grandeur, pas exact.
export function humanSize(len) {
  const n = Number(len) || 0;
  if (n < 1024) return `${n} o`;
  if (n < 1024 * 1024) return `${(n / 1024).toLocaleString('fr-FR', { maximumFractionDigits: 1 })} Kio`;
  return `${(n / 1048576).toLocaleString('fr-FR', { maximumFractionDigits: 1 })} Mio`;
}

function firstLine(text) {
  const s = String(text ?? '').trim();
  const i = s.indexOf('\n');
  return i < 0 ? s : s.slice(0, i);
}

// Résumé compact key:value d'un input (outils inconnus / MCP).
function kvSummary(input, maxKeys = 3, maxLen = 50) {
  if (!input || typeof input !== 'object') return '';
  return Object.entries(input)
    .slice(0, maxKeys)
    .map(([k, v]) => {
      const val = typeof v === 'string' ? v : JSON.stringify(v);
      return `${k}: ${truncate(oneLine(val), maxLen)}`;
    })
    .join(' · ');
}

// Diff ligne-à-ligne (toutes les lignes `old` en « - », `new` en « + »). Pas de lib :
// suffisant et sûr pour les chaînes ciblées et courtes d'Edit/MultiEdit.
export function diffLines(oldStr, newStr) {
  const rm = String(oldStr ?? '').split('\n').map((l) => ({ t: '-', l }));
  const add = String(newStr ?? '').split('\n').map((l) => ({ t: '+', l }));
  return [...rm, ...add];
}

// `edits[]` (MultiEdit) ou la triade old/new/replace_all (Edit) → liste normalisée.
export function editsOf(input) {
  if (Array.isArray(input?.edits)) return input.edits;
  return [{ old_string: input?.old_string, new_string: input?.new_string, replace_all: input?.replace_all }];
}

// --- describeTool : descripteur de l'en-tête ---------------------------------

// Retourne { iconKey, verb, primary, primaryTitle, primaryPath, primaryMono,
//            secondary?, badge?, todos?, suppressResult? }. Tous les accès à `input`
// sont gardés : une forme inattendue dégrade vers le rendu générique, ne jette jamais
// (un throw au rendu tuerait tout le panneau).
export function describeTool(name, input) {
  const inp = input && typeof input === 'object' && !Array.isArray(input) ? input : {};
  const base = { iconKey: 'tool', verb: name || 'outil', primary: '', primaryPath: false, primaryMono: false };

  switch (name) {
    case 'Read': {
      const sec = readRange(inp);
      return { ...base, iconKey: 'read', verb: 'Read', primary: inp.file_path || '', primaryPath: true, secondary: sec };
    }
    case 'Write':
      return {
        ...base, iconKey: 'write', verb: 'Write', primary: inp.file_path || '', primaryPath: true,
        secondary: inp.content != null ? `${countLines(inp.content)} lignes · ${humanSize(String(inp.content).length)}` : undefined,
      };
    case 'Edit':
      return {
        ...base, iconKey: 'edit', verb: 'Edit', primary: inp.file_path || '', primaryPath: true,
        secondary: inp.replace_all ? 'tout remplacer' : undefined,
      };
    case 'MultiEdit':
      return {
        ...base, iconKey: 'edit', verb: 'MultiEdit', primary: inp.file_path || '', primaryPath: true,
        secondary: `${editsOf(inp).length} éditions`,
      };
    case 'NotebookEdit':
      return {
        ...base, iconKey: 'notebook',
        verb: inp.edit_mode === 'insert' ? 'Notebook (insert)' : inp.edit_mode === 'delete' ? 'Notebook (delete)' : 'Notebook',
        primary: inp.notebook_path || '', primaryPath: true, badge: inp.cell_type,
      };
    case 'Bash':
      return {
        ...base, iconKey: 'bash', verb: 'Bash', primary: oneLine(inp.command || ''), primaryTitle: inp.command || '',
        primaryMono: true, secondary: inp.description ? truncate(inp.description, 60) : (inp.run_in_background ? 'arrière-plan' : undefined),
      };
    case 'Glob':
      return {
        ...base, iconKey: 'glob', verb: 'Glob', primary: inp.pattern || '', primaryMono: true,
        secondary: inp.path ? `dans ${splitPath(inp.path).base}` : undefined,
      };
    case 'Grep':
      return { ...base, iconKey: 'search', verb: 'Grep', primary: inp.pattern || '', primaryMono: true, secondary: grepScope(inp) };
    case 'WebFetch': {
      let host = inp.url || '';
      try { host = new URL(inp.url).host; } catch { /* url incomplète */ }
      return { ...base, iconKey: 'web', verb: 'WebFetch', primary: host, primaryTitle: inp.url || '', secondary: inp.prompt ? truncate(inp.prompt, 50) : undefined };
    }
    case 'WebSearch':
      return { ...base, iconKey: 'web', verb: 'WebSearch', primary: inp.query || '' };
    case 'TodoWrite':
      return { ...base, iconKey: 'todo', verb: 'Todos', todos: Array.isArray(inp.todos) ? inp.todos : [], suppressResult: true };
    case 'Task':
    case 'Agent':
      return {
        ...base, iconKey: 'agent', verb: 'Agent',
        primary: inp.description || truncate(inp.prompt || '', 80), primaryTitle: inp.prompt || inp.description || '',
        badge: inp.subagent_type,
      };
    default:
      break;
  }

  // Outils MCP : mcp__{serveur}__{outil}
  if (typeof name === 'string' && name.startsWith('mcp__')) {
    const parts = name.slice(5).split('__');
    const server = parts.shift() || 'mcp';
    const tool = parts.join('__') || name;
    return { ...base, iconKey: 'mcp', verb: tool.replace(/_/g, ' '), badge: server, primary: kvSummary(inp) };
  }

  // Inconnu : liste key:value de l'input (remplace l'ancien JSON.stringify brut).
  return { ...base, primary: kvSummary(inp), primaryTitle: safeJson(inp) };
}

function readRange(inp) {
  const { offset, limit } = inp;
  if (offset != null && limit != null) return `lignes ${offset}–${Number(offset) + Number(limit)}`;
  if (limit != null) return `${limit} lignes`;
  if (offset != null) return `dès ligne ${offset}`;
  return undefined;
}

function grepScope(inp) {
  const parts = [];
  if (inp.glob) parts.push(inp.glob);
  if (inp.type) parts.push(inp.type);
  if (inp.path) parts.push(`dans ${splitPath(inp.path).base}`);
  if (inp['-i']) parts.push('insensible casse');
  if (inp.multiline) parts.push('multiligne');
  return parts.length ? parts.join(' · ') : undefined;
}

function safeJson(v) {
  try { return JSON.stringify(v, null, 2); } catch { return ''; }
}

// --- formatToolResult : descripteur du résultat ------------------------------

// Retourne { summary, body, markdown?, diff?, mono? }. `diff:true` → AgentPanel rend
// <EditDiff input> ; `markdown:true` → <MarkdownView>{body} ; sinon <pre>{body}.
// `body` vide → résumé seul (pas de section repliable).
export function formatToolResult(name, input, text, isError) {
  const t = text || '';
  if (isError) return { summary: firstLine(t) || '(erreur)', body: t };

  switch (name) {
    case 'Read':
      return { summary: `${countLines(t)} lignes`, body: t };
    case 'Write':
      return { summary: '✓ écrit', body: '' };
    case 'Edit':
    case 'MultiEdit':
      return { summary: 'diff', body: '', diff: true };
    case 'Bash':
      return t.trim() ? { summary: `${countLines(t)} lignes`, body: t } : { summary: '(aucune sortie)', body: '' };
    case 'Glob':
      return { summary: `${countLines(t)} fichier(s)`, body: t };
    case 'Grep':
      return { summary: `${countLines(t)} ligne(s)`, body: t };
    case 'WebSearch':
      return { summary: 'résultats', body: t };
    case 'Task':
    case 'Agent':
      return { summary: 'réponse', body: t, markdown: true };
    case 'WebFetch':
      return { summary: 'contenu', body: t, markdown: true };
    case 'NotebookEdit':
      return { summary: 'cellule', body: t };
    default:
      break;
  }

  if (typeof name === 'string' && name.startsWith('mcp__')) {
    try { return { summary: 'résultat', body: JSON.stringify(JSON.parse(t), null, 2), mono: true }; }
    catch { return { summary: 'résultat', body: t }; }
  }
  return { summary: 'résultat', body: t };
}
