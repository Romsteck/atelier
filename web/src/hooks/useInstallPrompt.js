import { useCallback, useEffect, useState } from 'react';

// Pilote l'installation PWA depuis l'UI. Deux mondes :
//  - Chromium (desktop + Android) émet `beforeinstallprompt` quand les critères
//    d'installabilité sont remplis. On le capte, on le diffère, et on le rejoue
//    sur clic utilisateur (`prompt()` doit partir d'un geste utilisateur).
//  - iOS Safari n'émet jamais cet event et n'expose aucun déclenchement
//    programmatique : seul un guide « Partager → Sur l'écran d'accueil » est possible.

function standalone() {
  return (
    window.matchMedia?.('(display-mode: standalone)').matches ||
    window.navigator.standalone === true // iOS Safari (non standard)
  );
}

function detectIOS() {
  const ua = navigator.userAgent || '';
  const iPhoneish = /iphone|ipad|ipod/i.test(ua);
  // iPadOS 13+ se présente comme « Macintosh » mais a un écran tactile.
  const iPadOS = navigator.platform === 'MacIntel' && navigator.maxTouchPoints > 1;
  return iPhoneish || iPadOS;
}

export default function useInstallPrompt() {
  const [deferred, setDeferred] = useState(null);
  const [installed, setInstalled] = useState(() => standalone());
  const isIOS = detectIOS();

  useEffect(() => {
    const onBeforeInstall = (e) => {
      e.preventDefault(); // empêche le mini-infobar Chrome → on pilote nous-mêmes
      setDeferred(e);
    };
    const onInstalled = () => {
      setDeferred(null);
      setInstalled(true);
    };
    window.addEventListener('beforeinstallprompt', onBeforeInstall);
    window.addEventListener('appinstalled', onInstalled);

    // Bascule en mode app après installation sans rechargement.
    const mq = window.matchMedia?.('(display-mode: standalone)');
    const onDisplayChange = (e) => e.matches && setInstalled(true);
    mq?.addEventListener?.('change', onDisplayChange);

    return () => {
      window.removeEventListener('beforeinstallprompt', onBeforeInstall);
      window.removeEventListener('appinstalled', onInstalled);
      mq?.removeEventListener?.('change', onDisplayChange);
    };
  }, []);

  const promptInstall = useCallback(async () => {
    if (!deferred) return false;
    deferred.prompt();
    const { outcome } = await deferred.userChoice;
    setDeferred(null); // l'event n'est utilisable qu'une fois
    return outcome === 'accepted';
  }, [deferred]);

  return {
    installed,
    canInstall: !installed && deferred !== null,
    iosHint: !installed && isIOS && deferred === null,
    promptInstall,
  };
}
