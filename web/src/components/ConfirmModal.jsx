import { useEffect, useRef } from 'react';
import { AlertTriangle, Trash2, PowerOff, X } from 'lucide-react';
import Button from './Button';

const icons = {
  danger: Trash2,
  warning: PowerOff,
  default: AlertTriangle,
};

function ConfirmModal({
  isOpen, onClose, onConfirm, title, message,
  confirmText = 'Confirmer', cancelText = 'Annuler',
  variant = 'danger', loading = false,
}) {
  const modalRef = useRef(null);

  useEffect(() => {
    function handleKeyDown(e) {
      if (e.key === 'Escape' && isOpen && !loading) onClose();
    }
    document.addEventListener('keydown', handleKeyDown);
    return () => document.removeEventListener('keydown', handleKeyDown);
  }, [isOpen, loading, onClose]);

  useEffect(() => {
    if (isOpen) document.body.style.overflow = 'hidden';
    else document.body.style.overflow = '';
    return () => { document.body.style.overflow = ''; };
  }, [isOpen]);

  if (!isOpen) return null;

  const Icon = icons[variant] || icons.default;
  const iconColors = {
    danger: 'text-red-400 bg-red-900/50',
    warning: 'text-yellow-400 bg-yellow-900/50',
    default: 'text-blue-400 bg-blue-900/50',
  };

  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center p-4">
      <div className="absolute inset-0 bg-black/70 backdrop-blur-sm" onClick={!loading ? onClose : undefined} />
      <div ref={modalRef} className="relative bg-gray-800 border border-gray-700 shadow-2xl w-full max-w-md mx-4 overflow-hidden">
        <button onClick={onClose} disabled={loading} className="absolute top-3 right-3 text-gray-400 hover:text-gray-200 disabled:opacity-50">
          <X className="w-5 h-5" />
        </button>
        <div className="p-6">
          <div className={`w-12 h-12 ${iconColors[variant]} flex items-center justify-center mx-auto mb-4`}>
            <Icon className="w-6 h-6" />
          </div>
          <h3 className="text-lg font-semibold text-center mb-2">{title}</h3>
          <p className="text-gray-400 text-center text-sm">{message}</p>
        </div>
        <div className="flex gap-3 p-4 bg-gray-900/50 border-t border-gray-700">
          <Button onClick={onClose} variant="secondary" disabled={loading} className="flex-1 justify-center">{cancelText}</Button>
          <Button onClick={onConfirm} variant={variant} loading={loading} className="flex-1 justify-center">{confirmText}</Button>
        </div>
      </div>
    </div>
  );
}

export default ConfirmModal;
