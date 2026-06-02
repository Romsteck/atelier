// Petite barre de carrés proportionnels vert/rouge/gris à la GitHub.
// Couleurs en littéraux statiques (Tailwind v4 ne génère pas les classes dynamiques).

export default function DiffStatBar({ additions = 0, deletions = 0, max = 5 }) {
  const total = additions + deletions;
  let greens = 0;
  let reds = 0;
  if (total > 0) {
    greens = Math.max(additions ? 1 : 0, Math.round((additions / total) * max));
    greens = Math.min(greens, max - (deletions ? 1 : 0));
    reds = Math.max(deletions ? 1 : 0, max - greens);
    reds = Math.min(reds, max - greens);
  }
  const greys = max - greens - reds;

  return (
    <span className="inline-flex gap-[2px] items-center" aria-hidden="true">
      {Array.from({ length: greens }).map((_, i) => (
        <span key={`g${i}`} className="w-2 h-2 bg-green-500 rounded-[1px]" />
      ))}
      {Array.from({ length: reds }).map((_, i) => (
        <span key={`r${i}`} className="w-2 h-2 bg-red-500 rounded-[1px]" />
      ))}
      {Array.from({ length: greys }).map((_, i) => (
        <span key={`x${i}`} className="w-2 h-2 bg-gray-700 rounded-[1px]" />
      ))}
    </span>
  );
}
