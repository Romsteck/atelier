import { Loader2, Check, AlertCircle, X } from 'lucide-react';

// Badge de build affiché dans la barre supérieure du Studio. Alimenté par le
// canal WS `app:build` (cf. StudioShell). Extrait de l'ancien Layout.jsx.
export default function BuildBadge({ build, onDismiss }) {
  if (!build) return null;
  const status = build.status;
  const step = build.step;
  const total = build.total_steps ?? 5;
  const phase = build.phase;

  if (status === 'started' || status === 'step') {
    // Les builds locaux (skill 0-build) n'émettent pas de compteur d'étapes —
    // seuls les builds MCP distants ont step/total. Sans compteur, on affiche
    // juste « Build · {phase} » au lieu d'un trompeur « Build …/5 ».
    const hasCounter = step != null || build.total_steps != null;
    return (
      <div
        role="status"
        aria-live="polite"
        className="flex items-center gap-1.5 px-2 py-0.5 rounded-sm text-[11px] bg-blue-500/15 border border-blue-500/30 text-blue-300 shrink-0"
      >
        <Loader2 className="w-3 h-3 animate-spin" />
        <span>Build{hasCounter ? ` ${step ?? '…'}/${total}` : ''}</span>
        {phase && <span className="opacity-70">· {phase}</span>}
      </div>
    );
  }

  if (status === 'finished') {
    const secs = build.duration_ms != null ? Math.round(build.duration_ms / 1000) : null;
    return (
      <div
        role="status"
        aria-live="polite"
        className="flex items-center gap-1.5 px-2 py-0.5 rounded-sm text-[11px] bg-emerald-500/15 border border-emerald-500/30 text-emerald-300 shrink-0 transition-opacity duration-300"
      >
        <Check className="w-3 h-3" />
        <span>Build OK{secs != null ? ` · ${secs}s` : ''}</span>
      </div>
    );
  }

  if (status === 'error') {
    return (
      <div
        role="status"
        aria-live="polite"
        title={build.error || build.message || 'Build failed'}
        className="flex items-center gap-1.5 px-2 py-0.5 rounded-sm text-[11px] bg-red-500/15 border border-red-500/30 text-red-300 shrink-0"
      >
        <AlertCircle className="w-3 h-3" />
        <span>Build failed</span>
        <button
          onClick={onDismiss}
          aria-label="Dismiss build error"
          className="ml-0.5 p-0.5 rounded-sm hover:bg-red-500/20"
        >
          <X className="w-3 h-3" />
        </button>
      </div>
    );
  }

  return null;
}
