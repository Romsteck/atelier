import { useCallback, useEffect, useState } from 'react';
import { createPortal } from 'react-dom';
import { AlertTriangle, X } from 'lucide-react';
import Button from './Button';

// Modale de confirmation réutilisable — remplace `window.confirm` par un vrai
// dialogue thématisé (portail → document.body, backdrop, Escape, focus du bouton
// d'action). API impérative façon window.confirm : `await confirm({...}) → bool`.
//
//   const { confirm, dialog } = useConfirm();
//   if (!(await confirm({ title, message, confirmLabel, variant }))) return;
//   ...  // rendre {dialog} une fois dans le composant
export function useConfirm() {
  const [state, setState] = useState(null);

  const confirm = useCallback(
    (opts) => new Promise((resolve) => setState({ ...opts, resolve })),
    [],
  );

  const settle = useCallback(
    (value) => setState((s) => { s?.resolve(value); return null; }),
    [],
  );

  useEffect(() => {
    if (!state) return;
    const onKey = (e) => { if (e.key === 'Escape') settle(false); };
    document.addEventListener('keydown', onKey);
    const prev = document.body.style.overflow;
    document.body.style.overflow = 'hidden';
    return () => {
      document.removeEventListener('keydown', onKey);
      document.body.style.overflow = prev;
    };
  }, [state, settle]);

  const variant = state?.variant || 'primary';
  const danger = variant === 'danger';

  const dialog = state
    ? createPortal(
        // stopPropagation : un portail bubble les events par l'ARBRE REACT, pas
        // le DOM — sans ça, un clic dans le dialogue remonte au parent (carte
        // cliquable) et rouvre le drawer derrière la confirmation.
        <div
          className="fixed inset-0 z-[60] flex items-center justify-center p-4"
          onClick={(e) => e.stopPropagation()}
          onMouseDown={(e) => e.stopPropagation()}
        >
          <div className="absolute inset-0 bg-black/60 backdrop-blur-xs" onClick={() => settle(false)} />
          <div role="dialog" aria-modal="true" className="relative w-full max-w-md rounded-lg border border-gray-700 bg-gray-800 shadow-2xl">
            <div className="flex items-center gap-2 px-4 py-3 border-b border-gray-700">
              {danger && <AlertTriangle className="w-4 h-4 shrink-0 text-red-600 dark:text-red-400" />}
              <h3 className="text-sm font-semibold text-gray-50 flex-1">{state.title}</h3>
              <button onClick={() => settle(false)} className="text-gray-400 hover:text-gray-50" title="Fermer"><X className="w-4 h-4" /></button>
            </div>
            {state.message && <div className="px-4 py-4 text-sm text-gray-300 whitespace-pre-line">{state.message}</div>}
            <div className="flex items-center justify-end gap-2 px-4 py-3 border-t border-gray-700">
              <Button size="md" variant="neutral" onClick={() => settle(false)}>{state.cancelLabel || 'Annuler'}</Button>
              <Button size="md" variant={variant} autoFocus onClick={() => settle(true)}>{state.confirmLabel || 'Confirmer'}</Button>
            </div>
          </div>
        </div>,
        document.body,
      )
    : null;

  return { confirm, dialog };
}
