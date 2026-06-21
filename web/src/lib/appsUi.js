// Helpers UI partagés autour des apps — extraits de l'ancien `pages/Studio.jsx`
// pour être consommés à la fois par la homepage (Apps, Sidebar) et l'app Studio.

export const STACKS = [
  { value: 'next-js', label: 'Next.js' },
  { value: 'axum-vite', label: 'Vite+Rust' },
  { value: 'axum', label: 'Rust Only' },
];

export const stackLabel = (s) => STACKS.find((st) => st.value === s)?.label || s;

export const SLUG_RE = /^[a-z][a-z0-9-]*$/;

export function slugify(n) {
  return n
    .toLowerCase()
    .replace(/\s+/g, '-')
    .replace(/[^a-z0-9-]/g, '')
    .replace(/-+/g, '-')
    .replace(/^-|-$/g, '');
}

export function statusDot(state) {
  const s = (state || '').toLowerCase();
  if (s === 'running') return 'bg-green-400';
  if (s === 'crashed' || s === 'failed') return 'bg-red-400';
  if (s === 'starting') return 'bg-yellow-400 animate-pulse';
  return 'bg-gray-500';
}
