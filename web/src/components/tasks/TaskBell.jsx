import { Bell } from 'lucide-react';
import { useTaskContext } from '../../context/TaskContext';

export default function TaskBell() {
  const { activeCount, isOpen, setIsOpen } = useTaskContext();

  return (
    <button
      onClick={() => setIsOpen(!isOpen)}
      className="relative p-2 text-gray-400 hover:text-white transition-colors"
      title="Activité"
    >
      <Bell className="w-5 h-5" />
      {activeCount > 0 && (
        <span className="absolute -top-0.5 -right-0.5 bg-blue-500 text-white text-[10px] font-bold rounded-full min-w-[18px] h-[18px] flex items-center justify-center px-1">
          {activeCount}
        </span>
      )}
    </button>
  );
}
