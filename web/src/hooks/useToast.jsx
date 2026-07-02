import { useCallback, useEffect, useState } from 'react';

// Toast auto-dismiss partagé — remplace le motif `useState(message) + setTimeout(null)`
// dupliqué page par page. Usage :
//   const { toast, showToast } = useToast();
//   showToast('Sauvegardé');            // succès (vert)
//   showToast(apiErr(e), 'error');      // erreur (rouge)
//   … et rendre <Toast toast={toast} /> une fois dans le JSX.
export function useToast(ttlMs = 4000) {
  const [toast, setToast] = useState(null); // { msg, type: 'ok'|'error' }
  const showToast = useCallback((msg, type = 'ok') => setToast({ msg, type }), []);
  const dismiss = useCallback(() => setToast(null), []);
  useEffect(() => {
    if (!toast) return undefined;
    const t = setTimeout(() => setToast(null), ttlMs);
    return () => clearTimeout(t);
  }, [toast, ttlMs]);
  return { toast, showToast, dismiss };
}

export function Toast({ toast }) {
  if (!toast) return null;
  return (
    <div className={`fixed bottom-4 right-4 z-50 px-4 py-2 rounded-lg text-sm shadow-lg ${
      toast.type === 'error' ? 'bg-red-500/90 text-white' : 'bg-green-500/90 text-white'
    }`}>
      {toast.msg}
    </div>
  );
}
