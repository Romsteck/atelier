import { useEffect, useMemo, useState } from 'react';
import { Bot, Pause } from 'lucide-react';
import { usePilot } from '../../context/PilotContext';

function usePilotCountdown() {
  const { schedule, night } = usePilot();
  const [now, setNow] = useState(Date.now());
  useEffect(() => {
    const id = setInterval(() => setNow(Date.now()), 30_000);
    return () => clearInterval(id);
  }, []);
  return useMemo(() => {
    const active = ['running', 'waiting_atelier'].includes(night?.status);
    const next = schedule?.next_run_at ? new Date(schedule.next_run_at).getTime() : null;
    return { active, seconds: next ? Math.max(0, Math.floor((next - now) / 1000)) : null };
  }, [schedule?.next_run_at, night?.status, now]);
}

function countdownText(seconds) {
  if (seconds == null) return null;
  if (seconds < 60) return 'moins d’une minute';
  const minutes = Math.ceil(seconds / 60);
  return minutes < 60 ? `${minutes} min` : `${Math.floor(minutes / 60)} h ${minutes % 60} min`;
}

export function PilotStatusChip() {
  const { night } = usePilot();
  const { active, seconds } = usePilotCountdown();
  if (!active && (seconds == null || seconds > 3600)) return null;
  // Thème : le texte bleu porte ses deux variantes (seuls les gris sont mirrorés).
  return <span className="hidden xl:inline-flex items-center gap-1.5 px-2 py-1 rounded-md text-[10px] border border-blue-500/30 bg-blue-500/10 text-blue-700 dark:text-blue-300">
    <Bot className="w-3 h-3" />{active ? `Pilote · ${night?.stats?.done || 0} livré(s)` : `Pilote dans ${countdownText(seconds)}`}
  </span>;
}

export default function PilotBanner() {
  const { schedule, night, saveSchedule } = usePilot();
  const { active, seconds } = usePilotCountdown();
  if (!active && (seconds == null || seconds > 3600)) return null;
  return (
    <div className="h-8 shrink-0 px-4 border-b border-blue-500/25 bg-blue-500/10 flex items-center gap-2 text-[11px] text-blue-800 dark:text-blue-200">
      <Bot className={`w-3.5 h-3.5 ${active ? 'animate-pulse' : ''}`} />
      <span>{active ? `Changements autonomes en cours — ${night?.stats?.done || 0} livré(s)` : `Changements autonomes dans ${countdownText(seconds)}`}</span>
      {!active && schedule?.enabled && <button onClick={() => saveSchedule({ enabled: false })}
        className="ml-auto inline-flex items-center gap-1 px-2 py-0.5 rounded-sm border border-blue-400/25 hover:bg-blue-500/15">
        <Pause className="w-3 h-3" /> Pause
      </button>}
    </div>
  );
}
