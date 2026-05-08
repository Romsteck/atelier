function Card({ title, icon: Icon, children, className = '', actions }) {
  return (
    <div className={`bg-gray-800 border border-gray-700 ${className}`}>
      <div className="flex items-center justify-between px-4 py-3 border-b border-gray-700 bg-gray-800/60">
        <h3 className="font-semibold flex items-center gap-2 text-sm">
          {Icon && <Icon className="w-4 h-4 text-blue-400" />}
          {title}
        </h3>
        {actions && <div className="flex gap-2">{actions}</div>}
      </div>
      <div className="p-4">
        {children}
      </div>
    </div>
  );
}

export default Card;
