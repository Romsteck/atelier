function Section({ title, children, contrast = false, flush = false, className = '' }) {
  return (
    <div className={`border-b border-gray-700 ${contrast ? 'bg-gray-800/50' : 'bg-gray-900'} ${className}`}>
      {title && (
        <div className="px-4 py-2 sm:px-6 sm:py-3 border-b border-gray-700/50">
          <h2 className="text-sm font-semibold text-gray-400 uppercase tracking-wider">{title}</h2>
        </div>
      )}
      <div className={flush ? '' : 'px-4 py-3 sm:px-6'}>
        {children}
      </div>
    </div>
  );
}

export default Section;
