import { ArrowUp, CheckCircle, Loader2 } from 'lucide-react';

function VersionCell({ installed, latest, category, targetId, onUpgrade, upgrading }) {
  if (!installed && !latest) return <td className="py-2 px-3 text-gray-600 text-center">—</td>;

  const isUpToDate = installed && latest && installed === latest;
  const hasUpdate = installed && latest && installed !== latest;

  return (
    <td className="py-2 px-3">
      <div className="flex items-center gap-1.5">
        <span className="font-mono text-xs text-gray-300">{installed || '—'}</span>
        {hasUpdate && (
          <>
            <span className="text-gray-600">→</span>
            <span className="font-mono text-xs text-orange-400">{latest}</span>
            {!upgrading && (
              <button
                onClick={() => onUpgrade(targetId, category)}
                className="ml-1 text-orange-400 hover:text-orange-300 p-0.5"
                title="Mettre à jour"
              >
                <ArrowUp className="w-3 h-3" />
              </button>
            )}
            {upgrading && <Loader2 className="w-3 h-3 animate-spin text-blue-400 ml-1" />}
          </>
        )}
        {isUpToDate && <CheckCircle className="w-3 h-3 text-green-500/60" />}
      </div>
    </td>
  );
}

function OsCell({ target, onUpgrade, upgrading }) {
  const count = target.os_upgradable || 0;
  const security = target.os_security || 0;

  if (count === 0) {
    return (
      <td className="py-2 px-3">
        <span className="flex items-center gap-1 text-xs text-green-400">
          <CheckCircle className="w-3 h-3" /> À jour
        </span>
      </td>
    );
  }

  return (
    <td className="py-2 px-3">
      <div className="flex items-center gap-1.5">
        <span className="font-mono text-xs text-orange-400">{count} pkg</span>
        {security > 0 && <span className="font-mono text-xs text-red-400">({security} sécu)</span>}
        {!upgrading && (
          <button
            onClick={() => onUpgrade(target.id, 'apt')}
            className="ml-1 text-orange-400 hover:text-orange-300 p-0.5"
            title="Mettre à jour APT"
          >
            <ArrowUp className="w-3 h-3" />
          </button>
        )}
        {upgrading && <Loader2 className="w-3 h-3 animate-spin text-blue-400 ml-1" />}
      </div>
    </td>
  );
}

export function UpdateTableHead() {
  return (
    <thead>
      <tr className="text-left text-gray-500 text-xs uppercase tracking-wider border-b border-gray-700">
        <th className="py-2 px-3">Cible</th>
        <th className="py-2 px-3">OS (APT)</th>
        <th className="py-2 px-3">Agent</th>
      </tr>
    </thead>
  );
}

export function UpdateTableRow({ target, upgradeState = {}, onUpgrade }) {
  const isOnline = target.online;
  const isUpgrading = (cat) => upgradeState[cat]?.running;

  return (
    <tr className={`border-b border-gray-700/40 ${!isOnline ? 'opacity-40' : 'hover:bg-gray-700/20'}`}>
      <td className="py-2 px-3">
        <div className="flex items-center gap-2">
          <div className={`w-1.5 h-1.5 shrink-0 ${isOnline ? 'bg-green-400' : 'bg-gray-500'}`} />
          <span className="text-sm text-gray-200 truncate">{target.name}</span>
          {target.environment && (
            <span className={`text-[10px] px-1 py-px shrink-0 ${
              target.environment === 'development' ? 'bg-blue-500/20 text-blue-400' :
              target.environment === 'acceptance' ? 'bg-yellow-500/20 text-yellow-400' :
              'bg-purple-500/20 text-purple-400'
            }`}>
              {target.environment === 'development' ? 'DEV' :
               target.environment === 'acceptance' ? 'ACC' : 'PROD'}
            </span>
          )}
          {target.scan_error && (
            <span className="text-red-400/60 text-[10px]" title={target.scan_error}>⚠</span>
          )}
        </div>
      </td>
      <OsCell target={target} onUpgrade={onUpgrade} upgrading={isUpgrading('apt')} />
      <VersionCell
        installed={target.agent_version}
        latest={target.agent_version_latest}
        category="hr_agent"
        targetId={target.id}
        onUpgrade={onUpgrade}
        upgrading={isUpgrading('hr_agent')}
      />
    </tr>
  );
}
