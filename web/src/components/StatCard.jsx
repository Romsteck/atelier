import { Link } from 'react-router-dom';
import { ArrowRight } from 'lucide-react';

// Skeleton placeholder for a stat card
export function StatSkeleton() {
  return (
    <div className="bg-gray-800/50 border border-gray-700/50 rounded-lg p-4 animate-pulse">
      <div className="flex items-center gap-3">
        <div className="w-10 h-10 bg-gray-700 rounded-lg" />
        <div className="flex-1">
          <div className="h-3 bg-gray-700 rounded-sm w-16 mb-2" />
          <div className="h-6 bg-gray-700 rounded-sm w-24" />
        </div>
      </div>
    </div>
  );
}

export default function StatCard({ icon: Icon, label, value, sub, color = 'text-blue-400', to }) {
  const content = (
    <div className={`bg-gray-800/50 border border-gray-700/50 rounded-lg p-4 ${to ? 'hover:bg-gray-800 hover:border-gray-600 transition-colors cursor-pointer' : ''}`}>
      <div className="flex items-center gap-3">
        <div className={`w-10 h-10 rounded-lg bg-gray-700/50 flex items-center justify-center ${color}`}>
          <Icon className="w-5 h-5" />
        </div>
        <div className="flex-1 min-w-0">
          <div className="text-xs text-gray-500 uppercase tracking-wider">{label}</div>
          <div className={`text-xl font-bold ${color} leading-tight`}>{value ?? '-'}</div>
          {sub && <div className="text-xs text-gray-500 mt-0.5">{sub}</div>}
        </div>
        {to && <ArrowRight className="w-4 h-4 text-gray-600 shrink-0" />}
      </div>
    </div>
  );

  if (to) {
    return <Link to={to}>{content}</Link>;
  }
  return content;
}
