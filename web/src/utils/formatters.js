// Helpers de formatage partagés (temps, durée, octets, fraîcheur).

export const timeAgo = (dateStr, fallback = 'Jamais') => {
  if (!dateStr) return fallback;
  const diff = Math.floor((Date.now() - new Date(dateStr).getTime()) / 1000);
  if (diff < 60) return 'à l’instant';
  if (diff < 3600) return `il y a ${Math.floor(diff / 60)} min`;
  if (diff < 86400) return `il y a ${Math.floor(diff / 3600)} h`;
  if (diff < 604800) return `il y a ${Math.floor(diff / 86400)} j`;
  return new Date(dateStr).toLocaleDateString('fr-FR');
};

export const formatDate = (dateStr) =>
  dateStr
    ? new Date(dateStr).toLocaleString('fr-FR', {
        day: '2-digit',
        month: '2-digit',
        year: 'numeric',
        hour: '2-digit',
        minute: '2-digit',
      })
    : '—';

export const formatDuration = (secs) => {
  if (secs === null || secs === undefined) return '—';
  secs = Math.max(0, Math.round(secs));
  if (secs < 60) return `${secs}s`;
  const h = Math.floor(secs / 3600);
  const m = Math.floor((secs % 3600) / 60);
  const s = secs % 60;
  if (h > 0) return `${h}h ${m}m`;
  return `${m}m${s ? ` ${s}s` : ''}`;
};

// Durée entre deux dates ISO, en secondes (ou null).
export const durationSecs = (startStr, endStr) => {
  if (!startStr || !endStr) return null;
  return (new Date(endStr).getTime() - new Date(startStr).getTime()) / 1000;
};

export const formatBytes = (bytes) => {
  if (bytes === null || bytes === undefined) return '—';
  if (bytes < 1024) return `${bytes} o`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} Ko`;
  if (bytes < 1024 * 1024 * 1024) return `${(bytes / (1024 * 1024)).toFixed(1)} Mo`;
  return `${(bytes / (1024 * 1024 * 1024)).toFixed(2)} Go`;
};

// Classes Tailwind selon la fraîcheur (vert <24h / ambre <7j / rouge sinon).
export const freshnessClasses = (dateStr) => {
  if (!dateStr) return 'border-red-500/30 bg-red-500/10 text-red-700 dark:text-red-200';
  const ageHours = (Date.now() - new Date(dateStr).getTime()) / 3600000;
  if (ageHours < 24) return 'border-emerald-500/30 bg-emerald-500/10 text-emerald-700 dark:text-emerald-200';
  if (ageHours < 24 * 7) return 'border-amber-500/30 bg-amber-500/10 text-amber-700 dark:text-amber-200';
  return 'border-red-500/30 bg-red-500/10 text-red-700 dark:text-red-200';
};
