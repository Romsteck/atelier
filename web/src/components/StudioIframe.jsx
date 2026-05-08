import { useEffect, useRef, useState } from 'react';
import { Loader2, Power, RefreshCw, AlertTriangle } from 'lucide-react';
import useCloudMasterStatus from '../hooks/useCloudMasterStatus';

const CODESERVER_BASE = 'https://codeserver.mynetwk.biz';
const WAKE_TIMEOUT_MS = 90_000;

export default function StudioIframe({ folder, title }) {
  const { status, wake } = useCloudMasterStatus();
  const [iframeKey, setIframeKey] = useState(0);
  const [wakeTimedOut, setWakeTimedOut] = useState(false);
  const wakeStartRef = useRef(null);
  const prevStatusRef = useRef(status);

  useEffect(() => {
    const prev = prevStatusRef.current;
    if (prev !== status) {
      console.info('[StudioIframe] status: %s -> %s', prev, status);
      if (prev === 'waking_up' && status === 'online') {
        setIframeKey(k => k + 1);
      }
      if (status === 'online' || status === 'offline') {
        wakeStartRef.current = null;
        setWakeTimedOut(false);
      }
      prevStatusRef.current = status;
    }
  }, [status]);

  useEffect(() => {
    if (status !== 'waking_up') return;
    if (wakeStartRef.current == null) wakeStartRef.current = Date.now();
    const elapsed = Date.now() - wakeStartRef.current;
    const remaining = Math.max(0, WAKE_TIMEOUT_MS - elapsed);
    const timer = setTimeout(() => setWakeTimedOut(true), remaining);
    return () => clearTimeout(timer);
  }, [status]);

  const handleRetry = async () => {
    wakeStartRef.current = Date.now();
    setWakeTimedOut(false);
    await wake();
  };

  if (status === 'loading') {
    return (
      <div className="flex flex-col items-center justify-center h-full text-gray-400">
        <Loader2 className="w-6 h-6 animate-spin mb-2" />
        <span className="text-sm">Chargement…</span>
      </div>
    );
  }

  if (status === 'offline') {
    return (
      <div className="flex items-center justify-center h-full bg-[#1e1e1e]">
        <div className="max-w-sm w-full mx-4 p-6 rounded-lg bg-gray-800 border border-gray-700 shadow-xl text-center">
          <div className="flex items-center justify-center w-12 h-12 mx-auto mb-3 rounded-full bg-yellow-500/15 text-yellow-400">
            <Power className="w-6 h-6" />
          </div>
          <h3 className="text-base font-semibold text-white mb-1">CloudMaster est éteint</h3>
          <p className="text-sm text-gray-400 mb-4">
            Le serveur de développement qui héberge le Studio doit être démarré pour ouvrir l'éditeur.
          </p>
          <button
            onClick={wake}
            className="w-full px-4 py-2 text-sm font-medium text-white bg-blue-500 hover:bg-blue-600 active:bg-blue-700 rounded-md flex items-center justify-center gap-2 transition-colors"
          >
            <Power className="w-4 h-4" />
            Démarrer CloudMaster
          </button>
        </div>
      </div>
    );
  }

  if (status === 'waking_up') {
    return (
      <div className="flex items-center justify-center h-full bg-[#1e1e1e]">
        <div className="max-w-sm w-full mx-4 p-6 rounded-lg bg-gray-800 border border-gray-700 shadow-xl text-center">
          {wakeTimedOut ? (
            <>
              <div className="flex items-center justify-center w-12 h-12 mx-auto mb-3 rounded-full bg-orange-500/15 text-orange-400">
                <AlertTriangle className="w-6 h-6" />
              </div>
              <h3 className="text-base font-semibold text-white mb-1">Démarrage prolongé</h3>
              <p className="text-sm text-gray-400 mb-4">
                Le démarrage prend plus de temps que prévu. Vérifie l'alimentation de CloudMaster.
              </p>
              <button
                onClick={handleRetry}
                className="w-full px-4 py-2 text-sm font-medium text-white bg-blue-500 hover:bg-blue-600 active:bg-blue-700 rounded-md flex items-center justify-center gap-2 transition-colors"
              >
                <RefreshCw className="w-4 h-4" />
                Réessayer
              </button>
            </>
          ) : (
            <>
              <Loader2 className="w-8 h-8 mx-auto mb-3 animate-spin text-blue-400" />
              <h3 className="text-base font-semibold text-white mb-1">Boot en cours…</h3>
              <p className="text-sm text-gray-400">
                CloudMaster démarre. Le Studio se chargera automatiquement dès qu'il sera prêt.
              </p>
            </>
          )}
        </div>
      </div>
    );
  }

  if (status === 'shutting_down' || status === 'rebooting') {
    return (
      <div className="flex items-center justify-center h-full bg-[#1e1e1e]">
        <div className="max-w-sm w-full mx-4 p-6 rounded-lg bg-gray-800 border border-gray-700 shadow-xl text-center">
          <Loader2 className="w-8 h-8 mx-auto mb-3 animate-spin text-yellow-400" />
          <h3 className="text-base font-semibold text-white mb-1">
            {status === 'rebooting' ? 'Redémarrage…' : 'Extinction…'}
          </h3>
          <p className="text-sm text-gray-400">
            CloudMaster est en cours de transition. Patiente quelques secondes.
          </p>
        </div>
      </div>
    );
  }

  const url = `${CODESERVER_BASE}/?folder=${encodeURIComponent(folder)}`;
  return (
    <iframe
      key={iframeKey}
      src={url}
      className="w-full h-full border-0 bg-[#1e1e1e]"
      title={title || 'Studio'}
      allow="clipboard-read; clipboard-write"
    />
  );
}
