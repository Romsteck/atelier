import { useCallback } from 'react';

// Atelier tourne sur CloudMaster. La notion de "CloudMaster offline" depuis le
// frontend n'a pas de sens ici puisque servir cette page implique que CloudMaster
// est joignable. On stub donc le hook pour qu'il rapporte toujours 'online' et
// désactive le bouton WOL — homeroute reste seul propriétaire du wake.

export default function useCloudMasterStatus() {
  const wake = useCallback(async () => {
    // No-op — Atelier ne pilote pas l'alimentation hosts.
  }, []);
  return { status: 'online', hostId: null, wake };
}
