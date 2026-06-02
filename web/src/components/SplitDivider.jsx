import { useCallback } from 'react';

// Séparateur vertical redimensionnable (pointer events : souris + tactile + stylet, un seul chemin de code).
// Il ne possède PAS le ratio : il remonte le nouveau pourcentage de largeur gauche via onResize ; le parent
// (Studio) détient `leftPct`. `containerRef` = la zone de contenu, pour convertir clientX → %.

const MIN_PCT = 20;
const MAX_PCT = 80;

export default function SplitDivider({ containerRef, onResize, setDragging }) {
  const handlePointerDown = useCallback((e) => {
    e.preventDefault();
    const el = containerRef.current;
    if (!el) return;
    setDragging(true);

    const onMove = (ev) => {
      const rect = el.getBoundingClientRect();
      if (rect.width === 0) return;
      let pct = ((ev.clientX - rect.left) / rect.width) * 100;
      pct = Math.min(MAX_PCT, Math.max(MIN_PCT, pct));
      onResize(pct);
    };
    const onUp = () => {
      setDragging(false);
      window.removeEventListener('pointermove', onMove);
      window.removeEventListener('pointerup', onUp);
    };
    window.addEventListener('pointermove', onMove);
    window.addEventListener('pointerup', onUp);
  }, [containerRef, onResize, setDragging]);

  return (
    <div
      onPointerDown={handlePointerDown}
      role="separator"
      aria-orientation="vertical"
      className="absolute top-0 bottom-0 left-0 -translate-x-1/2 w-2 z-20 flex items-stretch
                 justify-center cursor-col-resize group touch-none select-none"
    >
      <span className="w-px bg-gray-700 group-hover:bg-blue-400 group-hover:w-0.5 transition-all" />
    </div>
  );
}
