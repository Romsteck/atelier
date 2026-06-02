import { useMemo } from 'react';
import { Activity } from 'lucide-react';

// Calendrier de contributions GitHub-like : 53 colonnes (semaines) × 7 lignes
// (jours), construit à rebours depuis aujourd'hui.

const WEEKS = 53;

// 5 niveaux de couleur — littéraux statiques (Tailwind v4 ne génère pas les
// classes construites dynamiquement).
const LEVELS = [
  'bg-gray-800',
  'bg-green-900',
  'bg-green-700',
  'bg-green-500',
  'bg-green-400',
];

const level = (c) => (c === 0 ? 0 : c <= 2 ? 1 : c <= 5 ? 2 : c <= 9 ? 3 : 4);

// Clé date "YYYY-MM-DD" en composants LOCAUX (jamais toISOString → dérive UTC).
const dateKey = (d) =>
  `${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, '0')}-${String(d.getDate()).padStart(2, '0')}`;

const DOW_BASE = ['Dim', 'Lun', 'Mar', 'Mer', 'Jeu', 'Ven', 'Sam'];

export default function CommitHeatmap({ data = [], weekStart = 0, loading = false, error = null }) {
  const { columns, monthLabels, total } = useMemo(() => {
    const counts = new Map((data || []).map((d) => [d.date, d.count]));

    const today = new Date();
    today.setHours(0, 0, 0, 0);
    const dow = (today.getDay() - weekStart + 7) % 7;

    // Cellule (0,0) = dimanche/lundi de la semaine 52 en arrière.
    const start = new Date(today);
    start.setDate(start.getDate() - (dow + (WEEKS - 1) * 7));

    const cols = [];
    const labels = [];
    let sum = 0;
    let prevMonth = -1;
    const cur = new Date(start);

    for (let w = 0; w < WEEKS; w++) {
      const col = [];
      let colMonth = null;
      for (let r = 0; r < 7; r++) {
        if (cur > today) {
          col.push({ future: true });
        } else {
          if (r === 0) colMonth = cur.getMonth();
          const k = dateKey(cur);
          const count = counts.get(k) || 0;
          sum += count;
          col.push({ date: k, count });
        }
        cur.setDate(cur.getDate() + 1);
      }
      // Label de mois quand la 1re cellule de la colonne change de mois.
      if (colMonth != null && colMonth !== prevMonth) {
        const labelDate = new Date(start);
        labelDate.setDate(labelDate.getDate() + w * 7);
        labels.push({ col: w, label: labelDate.toLocaleDateString('fr-FR', { month: 'short' }) });
        prevMonth = colMonth;
      }
      cols.push(col);
    }
    return { columns: cols, monthLabels: labels, total: sum };
  }, [data, weekStart]);

  const labelByCol = useMemo(() => {
    const m = new Map();
    monthLabels.forEach((l) => m.set(l.col, l.label));
    return m;
  }, [monthLabels]);

  return (
    <div className="px-4 sm:px-6 py-4 border-b border-gray-700/50">
      <div className="flex items-center gap-2 mb-3">
        <Activity className="w-3.5 h-3.5 text-gray-500" />
        <span className="text-xs text-gray-500 uppercase tracking-wider">Activité</span>
        {!loading && !error && (
          <span className="text-[11px] text-gray-600 ml-auto">{total} commits / an</span>
        )}
      </div>

      {error ? (
        <p className="text-xs text-red-400">Impossible de charger l'activité</p>
      ) : loading ? (
        <div className="flex gap-[3px] animate-pulse">
          {Array.from({ length: WEEKS }).map((_, w) => (
            <div key={w} className="flex flex-col gap-[3px]">
              {Array.from({ length: 7 }).map((_, r) => (
                <div key={r} className="w-[11px] h-[11px] rounded-[2px] bg-gray-800" />
              ))}
            </div>
          ))}
        </div>
      ) : (
        <div className="overflow-x-auto">
          <div className="inline-flex flex-col gap-1 min-w-max">
            {/* Labels de mois */}
            <div className="flex gap-[3px] pl-7">
              {columns.map((_, ci) => (
                <div key={ci} className="w-[11px] relative">
                  {labelByCol.has(ci) && (
                    <span className="absolute left-0 -top-px text-[9px] text-gray-500 whitespace-nowrap">
                      {labelByCol.get(ci)}
                    </span>
                  )}
                </div>
              ))}
            </div>

            <div className="flex gap-[3px]">
              {/* Labels jours */}
              <div className="flex flex-col gap-[3px] w-7 text-[9px] text-gray-600 pr-1 items-end">
                {Array.from({ length: 7 }).map((_, r) => (
                  <span key={r} className="h-[11px] leading-[11px]">
                    {r % 2 === 1 ? DOW_BASE[(r + weekStart) % 7] : ''}
                  </span>
                ))}
              </div>

              {/* Colonnes de semaines */}
              {columns.map((col, ci) => (
                <div key={ci} className="flex flex-col gap-[3px]">
                  {col.map((cell, ri) =>
                    cell.future ? (
                      <div key={ri} className="w-[11px] h-[11px]" />
                    ) : (
                      <div
                        key={ri}
                        title={`${cell.count} commit${cell.count > 1 ? 's' : ''} le ${cell.date}`}
                        className={`w-[11px] h-[11px] rounded-[2px] ${LEVELS[level(cell.count)]}`}
                      />
                    )
                  )}
                </div>
              ))}
            </div>

            {/* Légende */}
            <div className="flex items-center gap-1 justify-end text-[9px] text-gray-600 mt-1">
              Moins
              {LEVELS.map((c) => (
                <span key={c} className={`w-[11px] h-[11px] rounded-[2px] ${c}`} />
              ))}
              Plus
            </div>
            {total === 0 && (
              <p className="text-[11px] text-gray-600 mt-1">Aucune activité sur la période</p>
            )}
          </div>
        </div>
      )}
    </div>
  );
}
