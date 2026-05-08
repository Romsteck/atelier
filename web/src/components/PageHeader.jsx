import { createPortal } from 'react-dom';
import { usePageHeaderSlot } from '../context/PageHeaderSlot';

function PageHeader({ title, icon: Icon, children }) {
  const slot = usePageHeaderSlot();
  if (!slot) return null;
  return createPortal(
    <div className="flex items-center justify-between gap-3 flex-1 min-w-0">
      <h1 className="text-base font-semibold flex items-center gap-2 truncate">
        {Icon && <Icon className="w-4 h-4 text-blue-400 shrink-0" />}
        <span className="truncate">{title}</span>
      </h1>
      {children && <div className="flex items-center gap-2 flex-wrap shrink-0">{children}</div>}
    </div>,
    slot,
  );
}

export default PageHeader;
