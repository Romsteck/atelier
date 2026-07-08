import { useState } from 'react';
import { X, Trash2 } from 'lucide-react';
import Button from '../Button';

export function DeleteConfirmModal({ count, onConfirm, onClose }) {
  const [deleting, setDeleting] = useState(false);
  const [error, setError] = useState(null);

  const handleDelete = async () => {
    setDeleting(true);
    setError(null);
    try {
      await onConfirm();
      onClose();
    } catch (err) {
      // Échec (total ou partiel) : la modale reste ouverte et affiche le récapitulatif
      // remonté par onConfirm (la grille a déjà été rafraîchie côté DbExplorer).
      setError(err?.response?.data?.error || err?.message || 'Erreur');
      setDeleting(false);
    }
  };

  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center p-4 bg-black/50">
      <div className="bg-gray-800 rounded-lg border border-gray-700 shadow-xl w-full max-w-sm">
        <div className="flex items-center justify-between px-4 py-3 border-b border-gray-700">
          <h3 className="text-sm font-semibold text-red-400 flex items-center gap-2">
            <Trash2 className="w-4 h-4" /> Supprimer
          </h3>
          <button onClick={onClose} className="p-1 text-gray-400 hover:text-gray-50 rounded-sm hover:bg-gray-700 border-none bg-transparent cursor-pointer">
            <X className="w-4 h-4" />
          </button>
        </div>
        <div className="p-4 space-y-3">
          <p className="text-sm text-gray-300">
            Supprimer {count} ligne{count > 1 ? 's' : ''} selectionnee{count > 1 ? 's' : ''} ? Cette action est irreversible.
          </p>
          {error && <div className="text-xs text-red-400 bg-red-500/10 rounded-sm px-3 py-2">{error}</div>}
        </div>
        <div className="flex justify-end gap-2 px-4 py-3 border-t border-gray-700">
          <Button variant="neutral" size="sm" onClick={onClose}>Annuler</Button>
          <Button variant="danger" size="sm" icon={Trash2} loading={deleting} disabled={deleting} onClick={handleDelete}>
            {deleting ? 'Suppression...' : 'Supprimer'}
          </Button>
        </div>
      </div>
    </div>
  );
}
