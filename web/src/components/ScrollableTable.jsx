// Enveloppe une table large dans un conteneur à scroll horizontal : sur
// téléphone, les colonnes gardent leur largeur naturelle (table en `min-w-max`)
// et c'est le wrapper qui défile, au lieu d'écraser/déborder la page.
export default function ScrollableTable({ children, className = '' }) {
  return <div className={`w-full overflow-x-auto ${className}`}>{children}</div>;
}
