import { useEffect, useState } from 'react';
import { Bot, CheckCircle2, Loader2, WifiOff, XCircle } from 'lucide-react';

import { usePilot } from '../../context/PilotContext';

// Ordre nominal des phases du worker Atelier détaché — pilote la checklist de
// l'overlay (les phases avant la courante sont implicitement terminées, ce qui
// rend la reprise post-restart correcte même sans historique de steps).
const PHASE_ORDER = ['checkpoint', 'agent', 'deploy', 'healthcheck', 'commit'];
const PHASE_LABEL = {
  checkpoint: 'Checkpoint git — sauvegarde de l’existant',
  agent: 'Agent autonome — modifications du code',
  deploy: 'Build & installation — redémarrage du service',
  healthcheck: 'Vérification de santé',
  commit: 'Commit & push',
};

// Libellés courts par phase, consommés par la page Backlog (carte + drawer de
// l'item Atelier en vol) — le worker détaché ne trace pas ces phases dans son
// transcript, c'est la seule visibilité sur le build/deploy en cours.
export const MAINTENANCE_STATUS = {
  checkpoint: 'Checkpoint git en cours…',
  agent: 'Agent autonome au travail…',
  deploy: 'Build & installation d’Atelier — redémarrage imminent…',
  healthcheck: 'Vérification de santé de la plateforme…',
  commit: 'Commit & push…',
  rollback: 'Échec — restauration de la version précédente…',
};

/**
 * Détection d'indisponibilité d'Atelier. Le WS (auto-reconnect) est le signal
 * primaire ; quand il est fermé, une sonde `/api/health` tranche entre « bref
 * hoquet » et « service down » (2 échecs consécutifs ≈ 5 s). Ce n'est PAS du
 * polling de données : la sonde ne tourne que quand le serveur est injoignable,
 * même esprit que le backoff de reconnexion du hook WS.
 */
function useAtelierDown(wsStatus) {
  const [down, setDown] = useState(false);
  const [offline, setOffline] = useState(typeof navigator !== 'undefined' && navigator.onLine === false);
  useEffect(() => {
    const on = () => setOffline(false);
    const off = () => setOffline(true);
    window.addEventListener('online', on);
    window.addEventListener('offline', off);
    return () => { window.removeEventListener('online', on); window.removeEventListener('offline', off); };
  }, []);
  useEffect(() => {
    if (wsStatus === 'open') { setDown(false); return undefined; }
    let alive = true;
    let failures = 0;
    const probe = async () => {
      try {
        const r = await fetch('/api/health', { cache: 'no-store' });
        if (!alive) return;
        if (r.ok) { failures = 0; setDown(false); return; }
        failures += 1;
      } catch { failures += 1; }
      if (alive && failures >= 2) setDown(true);
    };
    probe();
    const id = setInterval(probe, 2500);
    return () => { alive = false; clearInterval(id); };
  }, [wsStatus]);
  return { down, offline };
}

function PhaseChecklist({ phase }) {
  const isRollback = phase === 'rollback';
  const current = isRollback ? PHASE_ORDER.length : PHASE_ORDER.indexOf(phase);
  return (
    <ol className="space-y-2 text-left">
      {PHASE_ORDER.map((p, i) => (
        <li key={p} className={`flex items-center gap-2.5 text-sm ${i < current ? 'text-emerald-700 dark:text-emerald-400' : i === current ? 'text-blue-700 dark:text-blue-300 font-medium' : 'text-gray-500 dark:text-gray-500'}`}>
          {i < current
            ? <CheckCircle2 className="w-4 h-4 shrink-0" />
            : i === current
              ? <Loader2 className="w-4 h-4 shrink-0 animate-spin" />
              : <span className="w-4 h-4 shrink-0 flex items-center justify-center"><span className="w-1.5 h-1.5 rounded-full bg-current opacity-50" /></span>}
          <span>{PHASE_LABEL[p]}</span>
        </li>
      ))}
      {isRollback && (
        <li className="flex items-center gap-2.5 text-sm text-red-700 dark:text-red-400 font-medium">
          <Loader2 className="w-4 h-4 shrink-0 animate-spin" />
          <span>Échec — restauration de la version précédente</span>
        </li>
      )}
    </ol>
  );
}

/**
 * Surcouche de disponibilité d'Atelier, montée dans les DEUX builds (homepage
 * + Studio). Trois visages :
 *  - service injoignable → overlay plein écran qui masque l'app (enrichi des
 *    étapes de mise à jour si un worker Atelier détaché est en cours) ;
 *  - poste hors ligne → overlay « vous êtes hors ligne » (on n'accuse pas
 *    Atelier d'une coupure réseau locale) ;
 *  - maintenance active service up (deploy imminent / rollback) → bandeau fin.
 * Le retrait est automatique : la sonde /api/health repasse OK → l'overlay
 * tombe et le resync WS existant remet l'état à jour derrière.
 */
export default function MaintenanceOverlay() {
  const { maintenance, wsStatus } = usePilot();
  const { down, offline } = useAtelierDown(wsStatus);
  const [dismissedRun, setDismissedRun] = useState(null);

  // Verdict terminal (active:false + outcome) : affiché ~8 s puis auto-masqué.
  useEffect(() => {
    if (maintenance && !maintenance.active && maintenance.outcome && dismissedRun !== maintenance.run_id) {
      const id = setTimeout(() => setDismissedRun(maintenance.run_id), 8000);
      return () => clearTimeout(id);
    }
    return undefined;
  }, [maintenance, dismissedRun]);

  if (down) {
    return (
      <div className="fixed inset-0 z-[200] flex items-center justify-center p-4 bg-gray-950/85 backdrop-blur-sm">
        <div className="w-full max-w-md rounded-xl border border-gray-200 dark:border-gray-700 bg-white dark:bg-gray-900 shadow-2xl p-6 text-center space-y-4">
          {offline ? (
            <>
              <WifiOff className="w-10 h-10 mx-auto text-gray-400 dark:text-gray-500" />
              <h2 className="text-lg font-semibold text-gray-900 dark:text-gray-50">Vous êtes hors ligne</h2>
              <p className="text-sm text-gray-600 dark:text-gray-400">La connexion reprendra automatiquement au retour du réseau.</p>
            </>
          ) : maintenance?.active ? (
            <>
              <Bot className="w-10 h-10 mx-auto text-blue-600 dark:text-blue-400 animate-pulse" />
              <h2 className="text-lg font-semibold text-gray-900 dark:text-gray-50">Atelier se met à jour</h2>
              <p className="text-sm text-gray-600 dark:text-gray-400">
                Mise à jour autonome en cours{maintenance.title ? <> — <span className="font-medium text-gray-800 dark:text-gray-200">{maintenance.title}</span></> : null}.
                Le service redémarre brièvement, cette page se rétablira toute seule.
              </p>
              <div className="rounded-lg border border-gray-200 dark:border-gray-800 bg-gray-50 dark:bg-gray-950 p-4">
                <PhaseChecklist phase={maintenance.phase} />
              </div>
            </>
          ) : (
            <>
              <Loader2 className="w-10 h-10 mx-auto text-blue-600 dark:text-blue-400 animate-spin" />
              <h2 className="text-lg font-semibold text-gray-900 dark:text-gray-50">Atelier est momentanément indisponible</h2>
              <p className="text-sm text-gray-600 dark:text-gray-400">Redémarrage ou maintenance en cours — reconnexion automatique…</p>
            </>
          )}
        </div>
      </div>
    );
  }

  // Service up : bandeau fin pendant la fenêtre sensible (deploy → rollback),
  // puis verdict terminal éphémère. Les phases amont (checkpoint/agent) restent
  // silencieuses ici — la page Backlog les montre déjà en live.
  if (maintenance?.active && ['deploy', 'healthcheck', 'rollback'].includes(maintenance.phase)) {
    const rollback = maintenance.phase === 'rollback';
    return (
      <div className={`fixed top-2 left-1/2 -translate-x-1/2 z-[190] flex items-center gap-2 px-3 py-1.5 rounded-full border shadow-lg text-xs ${rollback ? 'border-red-500/40 bg-red-50 dark:bg-red-950 text-red-700 dark:text-red-300' : 'border-amber-500/40 bg-amber-50 dark:bg-amber-950 text-amber-800 dark:text-amber-300'}`}>
        <Bot className="w-3.5 h-3.5 animate-pulse" />
        <span>{rollback ? 'Mise à jour échouée — restauration en cours' : 'Mise à jour d’Atelier en cours — bref redémarrage imminent'}</span>
      </div>
    );
  }
  if (maintenance && !maintenance.active && maintenance.outcome && dismissedRun !== maintenance.run_id) {
    const ok = maintenance.outcome === 'success';
    return (
      <div className={`fixed top-2 left-1/2 -translate-x-1/2 z-[190] flex items-center gap-2 px-3 py-1.5 rounded-full border shadow-lg text-xs ${ok ? 'border-emerald-500/40 bg-emerald-50 dark:bg-emerald-950 text-emerald-700 dark:text-emerald-300' : 'border-red-500/40 bg-red-50 dark:bg-red-950 text-red-700 dark:text-red-300'}`}>
        {ok ? <CheckCircle2 className="w-3.5 h-3.5" /> : <XCircle className="w-3.5 h-3.5" />}
        <span>{ok ? 'Mise à jour d’Atelier terminée' : maintenance.outcome === 'needs_user' ? 'Mise à jour interrompue — décision requise' : 'Mise à jour échouée — version précédente restaurée'}</span>
      </div>
    );
  }
  return null;
}
