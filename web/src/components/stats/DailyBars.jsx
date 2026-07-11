import { useMemo } from 'react';

// Mini-graphe en barres, fait main (aucune lib de chart — cf. CommitHeatmap).
// data = [{ label, value, danger? }]. Hauteur en px inline (valeur numérique →
// `style`, jamais une classe Tailwind dynamique, non générée par v4). Couleurs
// = classes littérales statiques.
export default function DailyBars({
  data = [],
  height = 44,
  color = 'bg-blue-500/70',
  dangerColor = 'bg-red-500/70',
  format,
}) {
  const max = useMemo(() => Math.max(1, ...data.map((d) => d.value || 0)), [data]);
  if (!data.length) {
    return <div className="text-xs text-gray-500">Aucune donnée sur la période</div>;
  }
  return (
    <div className="flex items-end gap-0.5" style={{ height: `${height}px` }}>
      {data.map((d, i) => {
        const h = Math.max(2, Math.round(((d.value || 0) / max) * height));
        const tip = format ? format(d.value || 0) : `${d.value || 0}`;
        return (
          <div
            key={d.label ?? i}
            className={`flex-1 min-w-[2px] rounded-sm transition-colors ${d.danger ? dangerColor : color}`}
            style={{ height: `${h}px` }}
            title={`${d.label ?? ''} · ${tip}`}
          />
        );
      })}
    </div>
  );
}
