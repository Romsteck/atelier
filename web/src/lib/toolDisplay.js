// Mise en forme COMPACTE par TYPE d'outil des appels d'agent (Read/Write/Bash/Edit/…).
// Logique pure, sans JSX : `describeTool` produit un descripteur minimal (verbe + cible)
// pour le libellé d'activité du chat. Le backend (runner.js) ne transmet plus que les
// champs nécessaires à ce libellé (pas de contenu de fichier ni de corps de résultat) —
// d'où la simplicité ici. Le composant AgentPanel mappe `iconKey`→icône lucide et
// matérialise le seul cas riche restant (TodoWrite → checklist épinglée).

// Coupe un chemin en { dir, base } sur le dernier séparateur. Pas de slash → tout en base.
export function splitPath(p) {
  const s = String(p ?? '');
  const i = Math.max(s.lastIndexOf('/'), s.lastIndexOf('\\'));
  return i < 0 ? { dir: '', base: s } : { dir: s.slice(0, i), base: s.slice(i + 1) };
}

// Troncature « douce » : coupe à `max`, retire un éventuel demi-mot final, ajoute « … ».
function truncate(s, max = 90) {
  const str = String(s ?? '');
  if (str.length <= max) return str;
  let cut = str.slice(0, max - 1);
  const sp = cut.lastIndexOf(' ');
  if (sp > max * 0.6) cut = cut.slice(0, sp);
  return cut + '…';
}

// Replie un texte multi-lignes sur une seule (pour les libellés) : \n → ⏎.
function oneLine(s) {
  return String(s ?? '').replace(/\s*\n\s*/g, ' ⏎ ').trim();
}

function safeJson(v) {
  try { return JSON.stringify(v, null, 2); } catch { return ''; }
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

// Retourne { iconKey, verb, primary, primaryTitle?, primaryPath?, primaryMono?, badge?,
//            todos? }. Tous les accès à `input` sont gardés : une forme inattendue dégrade
// vers le rendu générique, ne jette jamais (un throw au rendu tuerait tout le panneau).
export function describeTool(name, input) {
  const inp = input && typeof input === 'object' && !Array.isArray(input) ? input : {};
  const base = { iconKey: 'tool', verb: name || 'outil', primary: '', primaryPath: false, primaryMono: false };

  switch (name) {
    case 'Read':
      return { ...base, iconKey: 'read', verb: 'Read', primary: inp.file_path || '', primaryPath: true };
    case 'Write':
      return { ...base, iconKey: 'write', verb: 'Write', primary: inp.file_path || '', primaryPath: true };
    case 'Edit':
      return { ...base, iconKey: 'edit', verb: 'Edit', primary: inp.file_path || '', primaryPath: true };
    case 'MultiEdit':
      return { ...base, iconKey: 'edit', verb: 'MultiEdit', primary: inp.file_path || '', primaryPath: true };
    case 'NotebookEdit':
      return { ...base, iconKey: 'notebook', verb: 'Notebook', primary: inp.notebook_path || '', primaryPath: true, badge: inp.cell_type };
    case 'Bash':
      return { ...base, iconKey: 'bash', verb: 'Bash', primary: oneLine(inp.command || ''), primaryTitle: inp.command || '', primaryMono: true };
    case 'Glob':
      return { ...base, iconKey: 'glob', verb: 'Glob', primary: inp.pattern || '', primaryMono: true };
    case 'Grep':
      return { ...base, iconKey: 'search', verb: 'Grep', primary: inp.pattern || '', primaryMono: true };
    case 'WebFetch': {
      let host = inp.url || '';
      try { host = new URL(inp.url).host; } catch { /* url incomplète */ }
      return { ...base, iconKey: 'web', verb: 'WebFetch', primary: host, primaryTitle: inp.url || '' };
    }
    case 'WebSearch':
      return { ...base, iconKey: 'web', verb: 'WebSearch', primary: inp.query || '' };
    case 'TodoWrite':
      return { ...base, iconKey: 'todo', verb: 'Todos', todos: Array.isArray(inp.todos) ? inp.todos : [] };
    case 'Task':
    case 'Agent':
      return { ...base, iconKey: 'agent', verb: 'Agent', primary: inp.description || '', primaryTitle: inp.description || '', badge: inp.subagent_type };
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

  // Inconnu : liste key:value de l'input.
  return { ...base, primary: kvSummary(inp), primaryTitle: safeJson(inp) };
}
