import { useState, useEffect } from 'react';
import { RefreshCw, Package, Shield, Server, Loader2, Clock, CheckCircle, AlertTriangle } from 'lucide-react';
import Card from '../components/Card';
import Button from '../components/Button';
import ConfirmModal from '../components/ConfirmModal';
import PageHeader from '../components/PageHeader';
import { UpdateTableHead, UpdateTableRow } from '../components/TargetUpdateCard';
import {
  scanAllUpdates,
  getScanResults,
  upgradeTarget,
  getUpdateHistory,
  upgradeAllHosts,
} from '../api/client';
import useWebSocket from '../hooks/useWebSocket';

function Updates() {
  const [loading, setLoading] = useState(true);
  const [scanning, setScanning] = useState(false);
  const [targets, setTargets] = useState({});
  const [upgradeStates, setUpgradeStates] = useState({});
  const [history, setHistory] = useState([]);
  const [message, setMessage] = useState(null);
  const [confirmModal, setConfirmModal] = useState(null);

  // WebSocket events
  useWebSocket({
    'updates:scan:started': () => {
      setScanning(true);
      setMessage(null);
    },
    'updates:scan:target': (data) => {
      if (data.target) {
        setTargets(prev => ({ ...prev, [data.target.id]: data.target }));
      }
    },
    'updates:scan:complete': () => {
      setScanning(false);
      setMessage({ type: 'success', text: 'Scan terminé' });
      setTimeout(() => setMessage(null), 3000);
    },
    'updates:upgrade-target:started': (data) => {
      setUpgradeStates(prev => ({
        ...prev,
        [data.targetId]: {
          ...prev[data.targetId],
          [data.category]: { running: true, output: [] }
        }
      }));
    },
    'updates:upgrade-target:output': (data) => {
      setUpgradeStates(prev => {
        const targetState = prev[data.targetId] || {};
        const catState = targetState[data.category] || { running: true, output: [] };
        return {
          ...prev,
          [data.targetId]: {
            ...targetState,
            [data.category]: {
              ...catState,
              output: [...catState.output.slice(-100), data.line]
            }
          }
        };
      });
    },
    'updates:upgrade-target:complete': (data) => {
      setUpgradeStates(prev => {
        const targetState = prev[data.targetId] || {};
        return {
          ...prev,
          [data.targetId]: {
            ...targetState,
            [data.category]: { running: false, output: targetState[data.category]?.output || [] }
          }
        };
      });
      setMessage({
        type: data.success ? 'success' : 'error',
        text: data.success
          ? `Mise à jour terminée (${data.category})`
          : `Échec : ${data.error || 'Erreur inconnue'}`
      });
      getScanResults().then(r => {
        if (r.data.targets) setTargets(r.data.targets);
      }).catch(() => {});
      getUpdateHistory().then(r => {
        if (r.data.entries) setHistory(r.data.entries);
      }).catch(() => {});
    },
  });

  // Initial load
  useEffect(() => {
    Promise.all([
      getScanResults().catch(() => ({ data: { targets: {} } })),
      getUpdateHistory().catch(() => ({ data: { entries: [] } })),
    ]).then(([scanRes, histRes]) => {
      if (scanRes.data.targets) setTargets(scanRes.data.targets);
      if (histRes.data.entries) setHistory(histRes.data.entries);
      setLoading(false);
    });
  }, []);

  const handleScan = async () => {
    try {
      setScanning(true);
      setTargets({});
      await scanAllUpdates();
    } catch (e) {
      setScanning(false);
      setMessage({ type: 'error', text: 'Erreur lors du scan' });
    }
  };

  const handleUpgrade = (targetId, category) => {
    const target = targets[targetId];
    const categoryLabels = {
      apt: 'OS (APT)',
      claude_cli: 'Claude Code',
      claude_ext: 'Extension Claude',
      hr_agent: 'Agent',
    };
    setConfirmModal({
      targetId,
      category,
      title: `Mettre à jour ${categoryLabels[category] || category}`,
      message: `Voulez-vous mettre à jour ${categoryLabels[category] || category} sur ${target?.name || targetId} ?`,
    });
  };

  const confirmUpgrade = async () => {
    if (!confirmModal) return;
    const { targetId, category } = confirmModal;
    setConfirmModal(null);
    try {
      await upgradeTarget(targetId, category);
    } catch (e) {
      setMessage({ type: 'error', text: `Erreur : ${e.message}` });
    }
  };

  // Check if any upgrade is in progress
  const anyUpgradeRunning = Object.values(upgradeStates).some(
    ts => Object.values(ts).some(cat => cat.running)
  );

  const handleUpgradeHosts = () => {
    setConfirmModal({
      title: 'Mettre à jour tous les hôtes',
      message: 'Voulez-vous lancer la mise à jour APT sur tous les hôtes ayant des mises à jour disponibles ?',
      onConfirm: async () => {
        setConfirmModal(null);
        try {
          await upgradeAllHosts();
        } catch (e) {
          setMessage({ type: 'error', text: `Erreur : ${e.message}` });
        }
      },
    });
  };


  // Derived stats
  const targetList = Object.values(targets);
  const totalOsUpdates = targetList.reduce((s, t) => s + (t.os_upgradable || 0), 0);
  const totalSecurity = targetList.reduce((s, t) => s + (t.os_security || 0), 0);
  const agentsOutdated = targetList.filter(t =>
    t.agent_version && t.agent_version_latest && t.agent_version !== t.agent_version_latest
  ).length;
  const hosts = targetList.filter(t => t.target_type === 'remote_host' || t.target_type === 'main_host');

  const hostsWithUpdates = hosts.filter(t => t.os_upgradable > 0).length;

  if (loading) {
    return (
      <div className="flex items-center justify-center h-full">
        <div className="animate-spin h-12 w-12 border-b-2 border-blue-400" />
      </div>
    );
  }

  return (
    <div className="flex flex-col h-full overflow-y-auto">
      <PageHeader title="Mises à jour" icon={RefreshCw}>
        <div className="flex items-center gap-2">
          {hostsWithUpdates > 0 && (
            <Button
              variant="secondary"
              onClick={handleUpgradeHosts}
              disabled={scanning || anyUpgradeRunning}
            >
              <Server className="w-4 h-4 mr-2" /> MàJ hôtes ({hostsWithUpdates})
            </Button>
          )}
          <Button
            variant="primary"
            onClick={handleScan}
            disabled={scanning || anyUpgradeRunning}
          >
            {scanning ? (
              <><Loader2 className="w-4 h-4 animate-spin mr-2" /> Scan en cours...</>
            ) : (
              <><RefreshCw className="w-4 h-4 mr-2" /> Scanner tout</>
            )}
          </Button>
        </div>
      </PageHeader>

      <div className="p-6 space-y-6">
        {/* Message banner */}
        {message && (
          <div className={`p-3 text-sm ${
            message.type === 'success' ? 'bg-green-500/20 text-green-400' :
            message.type === 'error' ? 'bg-red-500/20 text-red-400' :
            'bg-yellow-500/20 text-yellow-400'
          }`}>
            {message.text}
          </div>
        )}

        {/* Summary stats */}
        {targetList.length > 0 && (
          <div className="grid grid-cols-3 gap-px bg-gray-700">
            <StatCard icon={Package} label="MàJ OS" value={totalOsUpdates} color={totalOsUpdates > 0 ? 'text-orange-400' : 'text-green-400'} />
            <StatCard icon={Shield} label="Sécurité" value={totalSecurity} color={totalSecurity > 0 ? 'text-red-400' : 'text-green-400'} />
            <StatCard icon={Server} label="Agents outdated" value={agentsOutdated} color={agentsOutdated > 0 ? 'text-orange-400' : 'text-green-400'} />
          </div>
        )}

        {/* Scanning indicator */}
        {scanning && (
          <div className="flex items-center gap-3 text-blue-400 py-2">
            <Loader2 className="w-4 h-4 animate-spin" />
            <span className="text-sm">Interrogation de tous les hôtes et environnements...</span>
          </div>
        )}

        {/* Hosts */}
        {hosts.length > 0 && (
          <Section title={`Hôtes (${hosts.length})`}>
            <div className="overflow-x-auto">
              <table className="w-full text-sm min-w-[500px]">
                <UpdateTableHead />
                <tbody>
                  {hosts.map(t => (
                    <UpdateTableRow
                      key={t.id}
                      target={t}
                      upgradeState={upgradeStates[t.id] || {}}
                      onUpgrade={handleUpgrade}
                    />
                  ))}
                </tbody>
              </table>
            </div>
          </Section>
        )}


        {/* History */}
        {history.length > 0 && (
          <Card title="Historique des mises à jour" icon={Clock}>
            <div className="overflow-x-auto">
              <table className="w-full text-sm min-w-[500px]">
                <thead>
                  <tr className="text-left text-gray-400 border-b border-gray-700">
                    <th className="pb-2 pr-4">Date</th>
                    <th className="pb-2 pr-4">Cible</th>
                    <th className="pb-2 pr-4">Catégorie</th>
                    <th className="pb-2 pr-4">Versions</th>
                    <th className="pb-2">Statut</th>
                  </tr>
                </thead>
                <tbody>
                  {history.slice(0, 20).map((entry) => (
                    <tr key={entry.id} className="border-b border-gray-700/50">
                      <td className="py-2 pr-4 text-gray-400">
                        {new Date(entry.timestamp * 1000).toLocaleString('fr-FR', {
                          day: '2-digit', month: '2-digit', hour: '2-digit', minute: '2-digit'
                        })}
                      </td>
                      <td className="py-2 pr-4">{entry.target_name}</td>
                      <td className="py-2 pr-4 font-mono text-xs text-gray-400">{entry.category}</td>
                      <td className="py-2 pr-4 font-mono text-xs">
                        {entry.version_before && entry.version_after
                          ? <>{entry.version_before} <span className="text-gray-500">→</span> {entry.version_after}</>
                          : <span className="text-gray-500">—</span>
                        }
                      </td>
                      <td className="py-2">
                        {entry.status === 'success' && (
                          <span className="text-green-400 flex items-center gap-1 text-xs">
                            <CheckCircle className="w-3 h-3" /> Succès
                          </span>
                        )}
                        {entry.status === 'failed' && (
                          <span className="text-red-400 flex items-center gap-1 text-xs" title={entry.error}>
                            <AlertTriangle className="w-3 h-3" /> Échec
                          </span>
                        )}
                        {entry.status === 'started' && (
                          <span className="text-blue-400 flex items-center gap-1 text-xs">
                            <Loader2 className="w-3 h-3 animate-spin" /> En cours
                          </span>
                        )}
                      </td>
                    </tr>
                  ))}
                </tbody>
              </table>
            </div>
          </Card>
        )}

        {/* Empty state */}
        {targetList.length === 0 && !scanning && (
          <div className="text-center py-12 text-gray-500">
            <RefreshCw className="w-12 h-12 mx-auto mb-4 opacity-30" />
            <p>Aucun résultat de scan disponible</p>
            <p className="text-sm mt-1">Cliquez sur "Scanner tout" pour démarrer</p>
          </div>
        )}
      </div>

      {/* Confirm modal */}
      <ConfirmModal
        isOpen={!!confirmModal}
        onClose={() => setConfirmModal(null)}
        onConfirm={confirmModal?.onConfirm || confirmUpgrade}
        title={confirmModal?.title || ''}
        message={confirmModal?.message || ''}
        confirmText="Mettre à jour"
        variant="warning"
      />
    </div>
  );
}

function StatCard({ icon: Icon, label, value, color }) {
  return (
    <div className="bg-gray-800 p-4">
      <div className="flex items-center gap-2 text-gray-400 text-sm mb-1">
        <Icon className="w-4 h-4" />
        {label}
      </div>
      <div className={`text-2xl font-bold ${color}`}>{value}</div>
    </div>
  );
}

function Section({ title, children }) {
  return (
    <div>
      <h3 className="text-sm font-semibold text-gray-400 uppercase tracking-wider mb-2">{title}</h3>
      <div className="bg-gray-800 border border-gray-700">
        {children}
      </div>
    </div>
  );
}

export default Updates;
