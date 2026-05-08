import { Layout, Layers, Boxes, Compass } from 'lucide-react';

const STYLES = {
  overview:  { bg: 'bg-gray-700/40',   text: 'text-gray-200', border: 'border-gray-500/40', label: 'Overview',  Icon: Compass },
  screen:    { bg: 'bg-blue-500/15',   text: 'text-blue-300', border: 'border-blue-500/40', label: 'Écran',     Icon: Layout },
  feature_g: { bg: 'bg-violet-500/20', text: 'text-violet-200', border: 'border-violet-500/40', label: 'Feature globale',  Icon: Layers },
  feature_s: { bg: 'bg-violet-500/10', text: 'text-violet-300', border: 'border-violet-400/30', label: 'Feature',          Icon: Layers },
  component: { bg: 'bg-emerald-500/15',text: 'text-emerald-200',border:'border-emerald-500/40', label: 'Composant',Icon: Boxes },
};

function styleFor(type, scope) {
  if (type === 'feature') {
    return scope === 'global' ? STYLES.feature_g : STYLES.feature_s;
  }
  return STYLES[type] || STYLES.screen;
}

export default function EntryTypeBadge({ type, scope, parentScreen, className = '' }) {
  const s = styleFor(type, scope);
  const { Icon } = s;
  const suffix = type === 'feature' && scope !== 'global' && parentScreen ? `: ${parentScreen}` : '';
  return (
    <span
      className={`inline-flex items-center gap-1 px-2 py-0.5 text-xs rounded-md border
                  ${s.bg} ${s.text} ${s.border} ${className}`}
    >
      <Icon className="w-3 h-3" />
      <span>{s.label}{suffix}</span>
    </span>
  );
}
