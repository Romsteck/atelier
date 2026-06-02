// Rendu d'un diff unifié, coloré ligne par ligne selon le 1er caractère.
// Volontairement maison : le projet n'importe aucun thème highlight.js, donc
// une fence ```diff via rehype-highlight rendrait un diff sans couleur (inutile).

// Au-delà, on tronque côté front pour ne pas générer trop de nœuds DOM.
const MAX_LINES = 2000;

function lineClass(line) {
  if (line.startsWith('+') && !line.startsWith('+++')) return 'text-green-400 bg-green-900/15';
  if (line.startsWith('-') && !line.startsWith('---')) return 'text-red-400 bg-red-900/15';
  if (line.startsWith('@@')) return 'text-cyan-400 bg-cyan-900/20';
  if (
    line.startsWith('diff ') ||
    line.startsWith('index ') ||
    line.startsWith('+++') ||
    line.startsWith('---') ||
    line.startsWith('new file') ||
    line.startsWith('deleted file') ||
    line.startsWith('rename ') ||
    line.startsWith('similarity ')
  ) {
    return 'text-gray-500';
  }
  return 'text-gray-300';
}

export default function DiffView({ patch, truncated = false }) {
  const text = patch || '';
  if (!text.trim()) {
    return (
      <div className="text-xs text-gray-600 italic px-3 py-4">
        Aucun diff (commit vide ou merge).
      </div>
    );
  }

  const allLines = text.split('\n');
  const lines = allLines.slice(0, MAX_LINES);
  const frontTruncated = allLines.length > MAX_LINES;

  return (
    <div>
      <div className="border border-gray-700 bg-gray-900 overflow-x-auto text-xs font-mono leading-5">
        {lines.map((ln, i) => (
          <div key={i} className={`px-3 whitespace-pre ${lineClass(ln)}`}>
            {ln || ' '}
          </div>
        ))}
      </div>
      {(truncated || frontTruncated) && (
        <div className="text-[11px] text-yellow-400 bg-yellow-900/20 border border-yellow-800 px-3 py-1.5 mt-1">
          Diff tronqué — affichage partiel
          {frontTruncated && ` (${allLines.length - MAX_LINES} lignes masquées)`}.
        </div>
      )}
    </div>
  );
}
