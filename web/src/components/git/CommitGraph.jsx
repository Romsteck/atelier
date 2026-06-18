import { useMemo } from 'react';
import { GitCommit, GitMerge } from 'lucide-react';

// Graphe de commits façon VSCode : lanes colorées, points par commit, arêtes vers
// les parents (les merges = 2 parents → arête diagonale), puces de décoration de
// branche. Les commits arrivent ordonnés `--topo-order --branches` (enfants avant
// parents) avec `parents` (SHAs) + `refs` (noms de branches) fournis par le backend.

const LANE_W = 14;   // largeur d'une lane (px)
const ROW_H = 30;    // hauteur d'une ligne de commit (px)
const DOT_R = 4;     // rayon d'un node
// Palette de lanes (cycle) — distinctes + lisibles sur fond sombre.
const LANE_COLORS = ['#60a5fa', '#a78bfa', '#34d399', '#fbbf24', '#f472b6', '#22d3ee', '#fb923c', '#a3e635'];

function ago(iso) {
  if (!iso) return '';
  const d = new Date(iso).getTime();
  if (!Number.isFinite(d)) return '';
  const s = Math.max(0, (Date.now() - d) / 1000);
  if (s < 60) return `${Math.floor(s)} s`;
  if (s < 3600) return `${Math.floor(s / 60)} min`;
  if (s < 86400) return `${Math.floor(s / 3600)} h`;
  if (s < 2592000) return `${Math.floor(s / 86400)} j`;
  return new Date(iso).toLocaleDateString();
}

// Assigne une lane (colonne) + couleur à chaque commit + calcule les arêtes vers
// les parents. Algorithme classique de git-graph : chaque lane « attend » un SHA
// (= le prochain commit à dessiner dans cette colonne).
function computeGraph(commits) {
  const indexOf = new Map(commits.map((c, i) => [c.sha, i]));
  const lanes = [];       // sha attendu par lane (null = libre)
  const laneColor = [];   // couleur par lane
  let colorN = 0;
  const nextColor = () => LANE_COLORS[colorN++ % LANE_COLORS.length];
  const rows = [];
  let maxLanes = 1;

  for (const c of commits) {
    let col = lanes.indexOf(c.sha);
    if (col === -1) {
      // tip de branche (aucune lane ne l'attendait) → lane libre ou nouvelle.
      col = lanes.indexOf(null);
      if (col === -1) { col = lanes.length; lanes.push(null); laneColor.push(null); }
      laneColor[col] = nextColor();
    }
    const color = laneColor[col];
    const parents = c.parents || [];
    const edges = []; // { toCol, color, parent }

    if (parents.length === 0) {
      lanes[col] = null; // racine : la lane se ferme
    }
    parents.forEach((p, idx) => {
      if (idx === 0) {
        lanes[col] = p; // la lane du commit continue vers son 1er parent
        edges.push({ toCol: col, color, parent: p });
      } else {
        // parent de merge : lane existante qui l'attend, sinon nouvelle.
        let pcol = lanes.indexOf(p);
        if (pcol === -1) {
          pcol = lanes.indexOf(null);
          if (pcol === -1) { pcol = lanes.length; lanes.push(null); laneColor.push(null); }
          laneColor[pcol] = nextColor();
          lanes[pcol] = p;
        }
        edges.push({ toCol: pcol, color: laneColor[pcol], parent: p });
      }
    });

    maxLanes = Math.max(maxLanes, lanes.length);
    rows.push({ commit: c, col, color, edges, merge: parents.length > 1 });
  }
  return { rows, indexOf, width: maxLanes * LANE_W };
}

export default function CommitGraph({ commits, openShas, onOpen, aheadShas }) {
  const { rows, indexOf, width } = useMemo(() => computeGraph(commits || []), [commits]);
  if (!rows.length) return null;

  const H = rows.length * ROW_H;
  const cx = (col) => col * LANE_W + LANE_W / 2;
  const cy = (i) => i * ROW_H + ROW_H / 2;

  // Arêtes (commit → chacun de ses parents). Parent hors fenêtre (limite atteinte)
  // → on file vers le bas (stub). Même colonne = vertical ; sinon coude dans la 1ʳᵉ
  // ligne puis vertical jusqu'au parent.
  const paths = [];
  rows.forEach((r, i) => {
    r.edges.forEach((e, k) => {
      const j = indexOf.has(e.parent) ? indexOf.get(e.parent) : rows.length; // hors fenêtre → bas
      const x0 = cx(r.col), y0 = cy(i);
      const x1 = cx(e.toCol), y1 = j < rows.length ? cy(j) : H;
      const d = x0 === x1
        ? `M ${x0} ${y0} L ${x1} ${y1}`
        : `M ${x0} ${y0} C ${x0} ${y0 + ROW_H * 0.5}, ${x1} ${y0 + ROW_H * 0.5}, ${x1} ${Math.min(y0 + ROW_H, y1)} L ${x1} ${y1}`;
      paths.push(<path key={`${i}-${k}`} d={d} fill="none" stroke={e.color} strokeWidth="1.5" opacity="0.85" />);
    });
  });

  return (
    <div className="relative" style={{ minHeight: H }}>
      {/* Gutter graphe (SVG absolu à gauche) */}
      <svg width={width} height={H} className="absolute top-0 left-0 pointer-events-none" style={{ overflow: 'visible' }}>
        {paths}
        {rows.map((r, i) => (
          r.merge
            ? <circle key={i} cx={cx(r.col)} cy={cy(i)} r={DOT_R} fill="#1f2937" stroke={r.color} strokeWidth="1.5" />
            : <circle key={i} cx={cx(r.col)} cy={cy(i)} r={DOT_R} fill={r.color} />
        ))}
      </svg>

      {/* Lignes de commit, alignées sur la grille du graphe (padding-left = largeur gutter) */}
      <div style={{ paddingLeft: width + 6 }}>
        {rows.map((r, i) => {
          const c = r.commit;
          const open = openShas?.has(c.sha);
          const unpushed = aheadShas?.has(c.sha);
          return (
            <button
              key={c.sha}
              onClick={() => onOpen?.(c)}
              title={c.subject}
              style={{ height: ROW_H }}
              className={`w-full flex items-center gap-2 pr-3 text-left text-[12px] ${
                open ? 'bg-blue-500/20' : 'hover:bg-gray-700/40'
              }`}
            >
              {r.merge ? <GitMerge className="w-3 h-3 shrink-0" style={{ color: r.color }} />
                       : <GitCommit className="w-3 h-3 shrink-0" style={{ color: r.color }} />}
              {/* Puces de branche (décorations) */}
              {(c.refs || []).map((ref) => (
                <span key={ref}
                  className="shrink-0 rounded-sm px-1 text-[10px] font-mono font-medium leading-4"
                  style={{ color: r.color, backgroundColor: `${r.color}22`, border: `1px solid ${r.color}55` }}>
                  {ref}
                </span>
              ))}
              <span className="truncate text-gray-200 flex items-center gap-1.5">
                {unpushed && <span className="w-1.5 h-1.5 rounded-full bg-green-400 shrink-0" title="non poussé" />}
                {c.subject}
              </span>
              <span className="ml-auto shrink-0 font-mono text-[11px]" style={{ color: r.color }}>{c.short}</span>
              <span className="shrink-0 text-gray-500 text-[11px] w-10 text-right">{ago(c.date)}</span>
            </button>
          );
        })}
      </div>
    </div>
  );
}
