const stateConfig = {
  running:  { dot: 'bg-green-400',  label: 'Actif' },
  failed:   { dot: 'bg-red-400',    label: 'Erreur' },
  stopped:  { dot: 'bg-gray-400',   label: 'Arrete' },
  starting: { dot: 'bg-yellow-400', label: 'Demarrage' },
  disabled: { dot: 'bg-gray-600',   label: 'Desactive' },
};

function ServiceStatusPanel({ services }) {
  if (!services || services.length === 0) return null;

  return (
    <div className="flex flex-wrap items-center">
      {services.map(svc => {
        const cfg = stateConfig[svc.state] || stateConfig.stopped;
        return (
          <div key={svc.name} className="flex items-center gap-1.5 px-3 py-2" title={svc.error || cfg.label}>
            <span className={`w-2 h-2 ${cfg.dot}`} />
            <span className="text-xs font-mono text-gray-300">{svc.name}</span>
            {svc.restartCount > 0 && <span className="text-xs text-yellow-500">{svc.restartCount}x</span>}
          </div>
        );
      })}
    </div>
  );
}

export default ServiceStatusPanel;
