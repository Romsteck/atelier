import { useState } from 'react';
import { X, Trash2, Loader2 } from 'lucide-react';

export function DeleteConfirmModal({ count, onConfirm, onClose }) {
  const [deleting, setDeleting] = useState(false);

  const handleDelete = async () => {
    setDeleting(true);
    try {
      await onConfirm();
      onClose();
    } catch {
      setDeleting(false);
    }
  };

  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/50">
      <div className="bg-gray-800 rounded-lg border border-gray-700 shadow-xl w-full max-w-sm">
        <div className="flex items-center justify-between px-4 py-3 border-b border-gray-700">
          <h3 className="text-sm font-semibold text-red-400 flex items-center gap-2">
            <Trash2 className="w-4 h-4" /> Supprimer
          </h3>
          <button onClick={onClose} className="p-1 text-gray-400 hover:text-white rounded hover:bg-gray-700 border-none bg-transparent cursor-pointer">
            <X className="w-4 h-4" />
          </button>
        </div>
        <div className="p-4">
          <p className="text-sm text-gray-300">
            Supprimer {count} ligne{count > 1 ? 's' : ''} selectionnee{count > 1 ? 's' : ''} ? Cette action est irreversible.
          </p>
        </div>
        <div className="flex justify-end gap-2 px-4 py-3 border-t border-gray-700">
          <button onClick={onClose} className="px-3 py-1.5 text-xs text-gray-400 rounded border-none bg-transparent cursor-pointer hover:text-white">
            Annuler
          </button>
          <button onClick={handleDelete} disabled={deleting} className="px-4 py-1.5 text-xs text-white bg-red-500 rounded border-none cursor-pointer hover:bg-red-600 disabled:opacity-50 flex items-center gap-1">
            {deleting ? <><Loader2 className="w-3 h-3 animate-spin" /> Suppression...</> : <><Trash2 className="w-3 h-3" /> Supprimer</>}
          </button>
        </div>
      </div>
    </div>
  );
}
