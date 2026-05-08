function StatusBadge({ status, children }) {
  const colors = {
    up: 'bg-green-500/20 text-green-400 border-green-500/30',
    down: 'bg-red-500/20 text-red-400 border-red-500/30',
    unknown: 'bg-yellow-500/20 text-yellow-400 border-yellow-500/30',
    active: 'bg-blue-500/20 text-blue-400 border-blue-500/30',
  };

  return (
    <span className={`px-2 py-0.5 text-xs font-medium border ${colors[status] || colors.unknown}`}>
      {children}
    </span>
  );
}

export default StatusBadge;
