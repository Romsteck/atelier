import { Loader2 } from 'lucide-react';

// Bouton standard « pilule teintée » — reproduit le traitement des chips de la
// barre Surveillance (« Tout scanner » / « Planification »). Couleur = sens :
// primary=bleu, success=émeraude, danger=rouge, warning=ambre, neutral=ghost gris.
//
// Lisibilité 2 thèmes : en clair fond = voile pâle sur blanc → texte foncé
// (-700/-800) ; en sombre fond = voile ténu sur quasi-noir → texte clair (-200).
// L'échelle grise (neutral) bascule seule via le mirror data-theme d'index.css
// (aucun dark: nécessaire) ; les teintes colorées NE sont PAS mirror → dark: explicite.
const VARIANTS = {
  primary: 'border-blue-500/30 bg-blue-500/20 text-blue-700 dark:text-blue-200 hover:bg-blue-500/30',
  success: 'border-emerald-500/30 bg-emerald-500/20 text-emerald-700 dark:text-emerald-200 hover:bg-emerald-500/30',
  danger: 'border-red-500/30 bg-red-500/20 text-red-700 dark:text-red-200 hover:bg-red-500/30',
  warning: 'border-amber-500/30 bg-amber-500/20 text-amber-800 dark:text-amber-200 hover:bg-amber-500/30',
  neutral: 'border-gray-700 text-gray-400 hover:text-gray-200 hover:border-gray-600',
  ghost: 'border-transparent text-gray-400 hover:text-gray-200 hover:bg-gray-700/40',
};
VARIANTS.secondary = VARIANTS.neutral; // alias rétro-compat (ancien variant)

// Tint « actif/sélectionné » — état activé de « Planification ».
const ACTIVE = 'border-emerald-500/40 bg-emerald-500/10 text-emerald-700 dark:text-emerald-300';

const SIZES = {
  xs: 'px-2 py-1 text-xs gap-1', // chip barre d'outils (défaut)
  sm: 'px-3 py-1.5 text-xs gap-1.5', // action compacte
  md: 'px-4 py-2 text-sm gap-2', // CTA de formulaire
};
const ICONS = { xs: 'w-3 h-3', sm: 'w-3.5 h-3.5', md: 'w-4 h-4' };

function Button({
  children,
  as: Tag = 'button',
  variant = 'primary',
  size = 'xs',
  icon: Icon,
  loading = false,
  active = false,
  disabled = false,
  className = '',
  type,
  ...rest
}) {
  const iconCls = ICONS[size] || ICONS.xs;
  const tone = active ? ACTIVE : (VARIANTS[variant] || VARIANTS.primary);
  // `<a>` et autres tags ne prennent pas type/disabled natifs.
  const native = Tag === 'button' ? { type: type || 'button', disabled: disabled || loading } : {};

  return (
    <Tag
      className={`border rounded-sm inline-flex items-center justify-center transition disabled:opacity-50 disabled:cursor-not-allowed ${SIZES[size] || SIZES.xs} ${tone} ${className}`}
      {...native}
      {...rest}
    >
      {loading
        ? <Loader2 className={`${iconCls} animate-spin`} />
        : Icon ? <Icon className={iconCls} /> : null}
      {children}
    </Tag>
  );
}

export default Button;
