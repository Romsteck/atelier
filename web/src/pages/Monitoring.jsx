import { useState, useEffect, useMemo } from 'react';
import {
  Activity, Cpu, HardDrive, Thermometer, Clock,
  Lock, Globe, AlertTriangle, CheckCircle, Server, RefreshCw, Power
} from 'lucide-react';
import PageHeader from '../components/PageHeader';
import Section from '../components/Section';
import { getHosts, getEdgeStats } from '../api/client';
import useWebSocket from '../hooks/useWebSocket';

const formatBytes = (bytes) => {
  if (!bytes) return '0 B';
  const k = 1024;
  const sizes = ['B', 'KB', 'MB', 'GB', 'TB'];
  const i = Math.floor(Math.log(bytes) / Math.log(k));
  return parseFloat((bytes / Math.pow(k, i)).toFixed(1)) + ' ' + sizes[i];
};

const formatUptime = (seconds) => {
  if (!seconds) return '--';
  const d = Math.floor(seconds / 86400);
  const h = Math.floor((seconds % 86400) / 3600);
  const m = Math.floor((seconds % 3600) / 60);
  return `${d}j ${h}h ${m}m`;
};

const getDaysUntilExpiry = (expiresAt) => {
  const now = new Date();
  const expiry = new Date(expiresAt);
  return Math.ceil((expiry - now) / (1000 * 60 * 60 * 24));
};

export default function Monitoring() {
  const [hosts, setHosts] = useState([]);
  const [certificates, setCertificates] = useState([]);
  const [edgeStats, setEdgeStats] = useState(null);
  const [loading, setLoading] = useState(true);
  const [refreshing, setRefreshing] = useState(false);
  const [wakingBackup, setWakingBackup] = useState(false);

  useWebSocket({
    'hosts:status': (data) => {
      setHosts(prev =>
        prev.map(h =>
          h.id === data.hostId
            ? { ...h, status: data.status, latency: data.latency, lastSeen: data.lastSeen }
            : h
        )
      );
    },
    'hosts:metrics': (data) => {
      setHosts(prev =>
        prev.map(h =>
          h.id === data.hostId
            ? { ...h, metrics: data }
            : h
        )
      );
    },
  });

  const fetchAll = async (isRefresh = false) => {
    if (isRefresh) setRefreshing(true);
    try {
      const [hostsRes, certsRes, edgeRes] = await Promise.allSettled([
        getHosts(),
        fetch('/api/acme/certificates').then(r => r.json()),
        getEdgeStats(),
      ]);

      if (hostsRes.status === 'fulfilled') setHosts(hostsRes.value.data.hosts || []);
      if (certsRes.status === 'fulfilled' && certsRes.value.success) {
        // Filter out legacy_code certs (dev code-server, no longer in use)
        setCertificates((certsRes.value.certificates || []).filter(c => c.type !== 'legacy_code'));
      }
      if (edgeRes.status === 'fulfilled') setEdgeStats(edgeRes.value.data);
    } catch (error) {
      console.error('Failed to load monitoring data:', error);
    } finally {
      setLoading(false);
      setRefreshing(false);
    }
  };

  useEffect(() => {
    fetchAll();
    const interval = setInterval(() => fetchAll(), 30000);
    return () => clearInterval(interval);
  }, []);

  const alerts = useMemo(() => {
    const result = [];

    // Host alerts
    hosts.forEach(host => {
      const isOnline = host.is_local || host.status === 'online';
      const m = host.metrics;
      const name = host.is_local ? 'HomeRoute' : host.name;

      // Host offline (not local, status not online)
      // Skip backup server — being offline is normal behavior
      const BACKUP_HOST_ID = '877bcb76-4fb8-4164-940c-707201adf9bc';
      if (!host.is_local && !isOnline && host.id !== BACKUP_HOST_ID) {
        result.push({ severity: 'critical', source: `host:${host.id}`, message: `Host '${name}' is offline` });
      }

      if (m) {
        // Disk > 80%
        if (m.diskTotalBytes > 0) {
          const pct = (m.diskUsedBytes / m.diskTotalBytes) * 100;
          if (pct > 80) {
            result.push({
              severity: pct > 95 ? 'critical' : 'warning',
              source: `host:${host.id}`,
              message: `Disk usage ${pct.toFixed(1)}% on '${name}'`,
            });
          }
        }

        // RAM > 90%
        if (m.memoryTotalBytes > 0) {
          const pct = (m.memoryUsedBytes / m.memoryTotalBytes) * 100;
          if (pct > 90) {
            result.push({
              severity: pct > 95 ? 'critical' : 'warning',
              source: `host:${host.id}`,
              message: `RAM usage ${pct.toFixed(1)}% on '${name}'`,
            });
          }
        }

        // CPU > 80%
        if ((m.cpuPercent || 0) > 80) {
          result.push({
            severity: m.cpuPercent > 95 ? 'critical' : 'warning',
            source: `host:${host.id}`,
            message: `CPU ${m.cpuPercent.toFixed(1)}% on '${name}'`,
          });
        }
      }
    });

    // TLS certificates expiring < 30 days
    certificates.forEach(cert => {
      const days = getDaysUntilExpiry(cert.expires_at);
      const domain = cert.domains?.[0] || cert.id || 'unknown';
      if (days < 0) {
        result.push({ severity: 'critical', source: `cert:${domain}`, message: `TLS cert for '${domain}' expired ${Math.abs(days)} days ago` });
      } else if (days < 30) {
        result.push({
          severity: days < 7 ? 'critical' : 'warning',
          source: `cert:${domain}`,
          message: `TLS cert for '${domain}' expires in ${days} days`,
        });
      }
    });

    // Sort: critical first
    result.sort((a, b) => (a.severity === 'critical' ? 0 : 1) - (b.severity === 'critical' ? 0 : 1));
    return result;
  }, [hosts, certificates]);

  if (loading) {
    return (
      <div className="flex items-center justify-center h-full">
        <div className="animate-spin h-12 w-12 border-b-2 border-blue-400"></div>
      </div>
    );
  }

  const onlineHosts = hosts.filter(h => h.is_local || h.status === 'online');
  const backupHost = hosts.find(h => h.id === '877bcb76-4fb8-4164-940c-707201adf9bc');

  const handleWakeBackup = async () => {
    setWakingBackup(true);
    try {
      await fetch('/api/hosts/877bcb76-4fb8-4164-940c-707201adf9bc/wake', { method: 'POST' });
    } catch (e) {
      console.error('Wake failed:', e);
    } finally {
      setWakingBackup(false);
    }
  };

  return (
    <div>
      <PageHeader title="Monitoring" icon={Activity}>
        <button
          onClick={() => fetchAll(true)}
          className="flex items-center gap-1.5 px-3 py-1.5 text-sm text-gray-300 bg-gray-700 border border-gray-600 hover:bg-gray-600 transition-colors"
        >
          <RefreshCw className={`w-3.5 h-3.5 ${refreshing ? 'animate-spin' : ''}`} />
          Actualiser
        </button>
      </PageHeader>

      {/* Alerts */}
      {alerts.length > 0 && (
        <Section title={`Alertes (${alerts.length})`}>
          <div className="space-y-2">
            {alerts.map((alert, i) => (
              <div
                key={i}
                className={`flex items-start gap-3 p-3 border ${
                  alert.severity === 'critical'
                    ? 'bg-red-900/20 border-red-500/30'
                    : 'bg-yellow-900/20 border-yellow-500/30'
                }`}
              >
                <AlertTriangle className={`w-4 h-4 mt-0.5 shrink-0 ${
                  alert.severity === 'critical' ? 'text-red-400' : 'text-yellow-400'
                }`} />
                <div className="flex-1 min-w-0">
                  <p className={`text-sm font-medium ${
                    alert.severity === 'critical' ? 'text-red-300' : 'text-yellow-300'
                  }`}>
                    {alert.message}
                  </p>
                  <p className="text-xs text-gray-500 mt-0.5">{alert.source}</p>
                </div>
                <span className={`px-1.5 py-0.5 text-[10px] font-bold uppercase shrink-0 ${
                  alert.severity === 'critical'
                    ? 'bg-red-500/20 text-red-400 border border-red-500/30'
                    : 'bg-yellow-500/20 text-yellow-400 border border-yellow-500/30'
                }`}>
                  {alert.severity}
                </span>
              </div>
            ))}
          </div>
        </Section>
      )}

      {/* Hosts */}
      <Section title={`Hotes (${onlineHosts.filter(h => h.id !== '877bcb76-4fb8-4164-940c-707201adf9bc').length}/${hosts.filter(h => h.id !== '877bcb76-4fb8-4164-940c-707201adf9bc').length})`}>
        {hosts.length === 0 ? (
          <div className="text-center py-4 text-gray-400 text-sm">Aucun hote configure.</div>
        ) : (
          <div className="grid grid-cols-1 md:grid-cols-2 xl:grid-cols-3 gap-3">
            {hosts.filter(h => h.id !== '877bcb76-4fb8-4164-940c-707201adf9bc').map((host) => {
              const isOnline = host.is_local || host.status === 'online';
              const m = host.metrics;
              return (
                <div key={host.id} className={`bg-gray-800 border border-gray-700 p-3 ${!isOnline ? 'opacity-50' : ''}`}>
                  <div className="flex items-center justify-between mb-2">
                    <div className="flex items-center gap-2">
                      <Server className={`w-4 h-4 ${isOnline ? 'text-green-400' : 'text-gray-500'}`} />
                      <span className="text-sm font-medium text-white">{host.is_local ? 'HomeRoute' : host.name}</span>
                    </div>
                    <span className={`px-1.5 py-0.5 text-[10px] font-medium ${
                      isOnline ? 'bg-green-500/20 text-green-400 border border-green-500/30' : 'bg-gray-500/20 text-gray-400 border border-gray-500/30'
                    }`}>
                      {isOnline ? 'EN LIGNE' : 'HORS LIGNE'}
                    </span>
                  </div>

                  {m ? (
                    <div className="space-y-1.5">
                      {/* CPU */}
                      <div className="flex items-center gap-2">
                        <Cpu className="w-3.5 h-3.5 text-blue-400 shrink-0" />
                        <div className="flex-1">
                          <div className="w-full bg-gray-700 h-2">
                            <div className="h-2 bg-blue-500 transition-all" style={{ width: `${Math.min(m.cpuPercent || 0, 100)}%` }} />
                          </div>
                        </div>
                        <span className="text-xs text-gray-400 w-10 text-right">{(m.cpuPercent || 0).toFixed(0)}%</span>
                      </div>

                      {/* RAM */}
                      <div className="flex items-center gap-2">
                        <HardDrive className="w-3.5 h-3.5 text-green-400 shrink-0" />
                        <div className="flex-1">
                          <div className="w-full bg-gray-700 h-2">
                            <div className="h-2 bg-green-500 transition-all" style={{ width: `${m.memoryTotalBytes ? (m.memoryUsedBytes / m.memoryTotalBytes * 100).toFixed(0) : 0}%` }} />
                          </div>
                        </div>
                        <span className="text-xs text-gray-400 w-20 text-right">{formatBytes(m.memoryUsedBytes)}/{formatBytes(m.memoryTotalBytes)}</span>
                      </div>

                      {/* Disk */}
                      {m.diskTotalBytes > 0 && (
                        <div className="flex items-center gap-2">
                          <HardDrive className="w-3.5 h-3.5 text-amber-400 shrink-0" />
                          <div className="flex-1">
                            <div className="w-full bg-gray-700 h-2">
                              <div className="h-2 bg-amber-500 transition-all" style={{ width: `${(m.diskUsedBytes / m.diskTotalBytes * 100).toFixed(0)}%` }} />
                            </div>
                          </div>
                          <span className="text-xs text-gray-400 w-20 text-right">{formatBytes(m.diskUsedBytes)}/{formatBytes(m.diskTotalBytes)}</span>
                        </div>
                      )}

                      {/* Load / Temp / Uptime */}
                      <div className="flex items-center gap-3 pt-1 text-xs text-gray-400">
                        {m.loadAvg1 != null && (
                          <span title="Load average 1/5/15">
                            Load: {m.loadAvg1.toFixed(2)} / {m.loadAvg5.toFixed(2)} / {m.loadAvg15.toFixed(2)}
                          </span>
                        )}
                      </div>
                      <div className="flex items-center gap-3 text-xs text-gray-400">
                        {m.temperature != null && (
                          <span className="flex items-center gap-1">
                            <Thermometer className={`w-3 h-3 ${m.temperature > 80 ? 'text-red-400' : 'text-gray-400'}`} />
                            <span className={m.temperature > 80 ? 'text-red-400' : ''}>{m.temperature.toFixed(0)}C</span>
                          </span>
                        )}
                        {m.uptimeSeconds != null && (
                          <span className="flex items-center gap-1">
                            <Clock className="w-3 h-3" />
                            {formatUptime(m.uptimeSeconds)}
                          </span>
                        )}
                      </div>
                    </div>
                  ) : (
                    <div className="text-xs text-gray-500">Pas de metriques disponibles</div>
                  )}
                </div>
              );
            })}
          </div>
        )}
      </Section>

      {/* Backup Server */}
      <Section title="Backup Server">
        {!backupHost ? (
          <div className="text-center py-4 text-gray-400 text-sm">Backup server non configure</div>
        ) : (
          (() => {
            const isOnline = backupHost.status === 'online';
            const m = backupHost.metrics;
            return (
              <div className={`bg-gray-800 border border-gray-700 p-4 ${!isOnline ? 'opacity-70' : ''}`}>
                <div className="flex items-center justify-between mb-3">
                  <div className="flex items-center gap-2">
                    <Server className={`w-4 h-4 ${isOnline ? 'text-green-400' : 'text-gray-500'}`} />
                    <span className="text-sm font-medium text-white">{backupHost.name}</span>
                  </div>
                  <div className="flex items-center gap-2">
                    {!isOnline && (
                      <button
                        onClick={handleWakeBackup}
                        disabled={wakingBackup}
                        className="flex items-center gap-1.5 px-2.5 py-1 text-xs text-green-300 bg-green-500/10 border border-green-500/30 hover:bg-green-500/20 transition-colors disabled:opacity-50 disabled:cursor-not-allowed"
                      >
                        <Power className={`w-3.5 h-3.5 ${wakingBackup ? 'animate-pulse' : ''}`} />
                        {wakingBackup ? 'Envoi WOL...' : 'Reveiller'}
                      </button>
                    )}
                    <span className={`px-1.5 py-0.5 text-[10px] font-medium ${
                      isOnline ? 'bg-green-500/20 text-green-400 border border-green-500/30' : 'bg-gray-500/20 text-gray-400 border border-gray-500/30'
                    }`}>
                      {isOnline ? 'EN LIGNE' : 'HORS LIGNE'}
                    </span>
                  </div>
                </div>

                {m ? (
                  <div className="space-y-1.5">
                    {/* CPU */}
                    <div className="flex items-center gap-2">
                      <Cpu className="w-3.5 h-3.5 text-blue-400 shrink-0" />
                      <div className="flex-1">
                        <div className="w-full bg-gray-700 h-2">
                          <div className="h-2 bg-blue-500 transition-all" style={{ width: `${Math.min(m.cpuPercent || 0, 100)}%` }} />
                        </div>
                      </div>
                      <span className="text-xs text-gray-400 w-10 text-right">{(m.cpuPercent || 0).toFixed(0)}%</span>
                    </div>

                    {/* RAM */}
                    <div className="flex items-center gap-2">
                      <HardDrive className="w-3.5 h-3.5 text-green-400 shrink-0" />
                      <div className="flex-1">
                        <div className="w-full bg-gray-700 h-2">
                          <div className="h-2 bg-green-500 transition-all" style={{ width: `${m.memoryTotalBytes ? (m.memoryUsedBytes / m.memoryTotalBytes * 100).toFixed(0) : 0}%` }} />
                        </div>
                      </div>
                      <span className="text-xs text-gray-400 w-20 text-right">{formatBytes(m.memoryUsedBytes)}/{formatBytes(m.memoryTotalBytes)}</span>
                    </div>

                    {/* Disk */}
                    {m.diskTotalBytes > 0 && (
                      <div className="flex items-center gap-2">
                        <HardDrive className="w-3.5 h-3.5 text-amber-400 shrink-0" />
                        <div className="flex-1">
                          <div className="w-full bg-gray-700 h-2">
                            <div className="h-2 bg-amber-500 transition-all" style={{ width: `${(m.diskUsedBytes / m.diskTotalBytes * 100).toFixed(0)}%` }} />
                          </div>
                        </div>
                        <span className="text-xs text-gray-400 w-20 text-right">{formatBytes(m.diskUsedBytes)}/{formatBytes(m.diskTotalBytes)}</span>
                      </div>
                    )}

                    {/* Temp / Uptime */}
                    <div className="flex items-center gap-3 pt-1 text-xs text-gray-400">
                      {m.temperature != null && (
                        <span className="flex items-center gap-1">
                          <Thermometer className={`w-3 h-3 ${m.temperature > 80 ? 'text-red-400' : 'text-gray-400'}`} />
                          <span className={m.temperature > 80 ? 'text-red-400' : ''}>{m.temperature.toFixed(0)}C</span>
                        </span>
                      )}
                      {m.uptimeSeconds != null && (
                        <span className="flex items-center gap-1">
                          <Clock className="w-3 h-3" />
                          {formatUptime(m.uptimeSeconds)}
                        </span>
                      )}
                    </div>
                  </div>
                ) : (
                  <div className="text-xs text-gray-500">{isOnline ? 'Metriques en attente...' : 'Serveur hors ligne'}</div>
                )}
              </div>
            );
          })()
        )}
      </Section>

      {/* Certificates TLS */}
      <Section title={`Certificats TLS (${certificates.length})`}>
        {certificates.length === 0 ? (
          <div className="text-center py-4 text-gray-400 text-sm">Aucun certificat.</div>
        ) : (
          <div className="overflow-x-auto">
            <table className="w-full text-sm min-w-[500px]">
              <thead>
                <tr className="text-xs text-gray-500 uppercase tracking-wider border-b border-gray-700/50">
                  <th className="text-left py-1.5 font-medium">Domaine</th>
                  <th className="text-left py-1.5 font-medium">Expiration</th>
                  <th className="text-left py-1.5 font-medium">Jours restants</th>
                  <th className="text-left py-1.5 font-medium">Statut</th>
                </tr>
              </thead>
              <tbody>
                {certificates.map((cert) => {
                  const days = getDaysUntilExpiry(cert.expires_at);
                  const expired = days < 0;
                  const warning = days >= 0 && days < 30;
                  return (
                    <tr
                      key={cert.id}
                      className={`border-b border-gray-800 ${
                        expired ? 'bg-red-900/10' : warning ? 'bg-orange-900/10' : ''
                      }`}
                    >
                      <td className="py-1.5">
                        <div className="flex items-center gap-1.5">
                          <Globe className="w-3.5 h-3.5 text-blue-400 shrink-0" />
                          <span className="font-medium text-gray-200">
                            {cert.domains && cert.domains.length > 0 ? cert.domains[0] : cert.id || 'Certificat'}
                          </span>
                        </div>
                      </td>
                      <td className="py-1.5 text-gray-400">
                        {new Date(cert.expires_at).toLocaleDateString('fr-FR', { day: 'numeric', month: 'short', year: 'numeric' })}
                      </td>
                      <td className="py-1.5">
                        <span className={expired ? 'text-red-400 font-medium' : warning ? 'text-orange-400 font-medium' : 'text-green-400'}>
                          {expired ? `${Math.abs(days)}j expire` : `${days}j`}
                        </span>
                      </td>
                      <td className="py-1.5">
                        {expired ? (
                          <span className="flex items-center gap-1 text-red-400">
                            <AlertTriangle className="w-3.5 h-3.5" />
                            Expire
                          </span>
                        ) : warning ? (
                          <span className="flex items-center gap-1 text-orange-400">
                            <AlertTriangle className="w-3.5 h-3.5" />
                            Bientot
                          </span>
                        ) : (
                          <span className="flex items-center gap-1 text-green-400">
                            <CheckCircle className="w-3.5 h-3.5" />
                            Valide
                          </span>
                        )}
                      </td>
                    </tr>
                  );
                })}
              </tbody>
            </table>
          </div>
        )}
      </Section>

      {/* Edge Stats */}
      <Section title="Edge Stats">
        {!edgeStats ? (
          <div className="text-center py-4 text-gray-400 text-sm">Statistiques indisponibles.</div>
        ) : (
          <div className="space-y-4">
            {/* Global stats */}
            {edgeStats.global && (
              <div className="grid grid-cols-2 md:grid-cols-3 xl:grid-cols-6 gap-3">
                {[
                  { label: 'Requetes totales', value: edgeStats.global.total_requests?.toLocaleString() || '0' },
                  { label: '2xx', value: edgeStats.global.status_2xx?.toLocaleString() || '0', color: 'text-green-400' },
                  { label: '4xx', value: edgeStats.global.status_4xx?.toLocaleString() || '0', color: 'text-orange-400' },
                  { label: '5xx', value: edgeStats.global.status_5xx?.toLocaleString() || '0', color: 'text-red-400' },
                  { label: 'Req/s', value: edgeStats.global.requests_per_second?.toFixed(1) || '0' },
                  { label: 'Uptime', value: formatUptime(edgeStats.global.uptime_secs) },
                ].map((stat, i) => (
                  <div key={i} className="bg-gray-800 border border-gray-700 p-3 text-center">
                    <div className={`text-lg font-bold ${stat.color || 'text-white'}`}>{stat.value}</div>
                    <div className="text-xs text-gray-500 mt-0.5">{stat.label}</div>
                  </div>
                ))}
              </div>
            )}

            {/* Domains table */}
            {edgeStats.domains && edgeStats.domains.length > 0 && (
              <div className="overflow-x-auto">
                <table className="w-full text-sm min-w-[400px]">
                  <thead>
                    <tr className="text-xs text-gray-500 uppercase tracking-wider border-b border-gray-700/50">
                      <th className="text-left py-1.5 font-medium">Domaine</th>
                      <th className="text-right py-1.5 font-medium">Requetes</th>
                      <th className="text-right py-1.5 font-medium">Erreurs 5xx</th>
                    </tr>
                  </thead>
                  <tbody>
                    {[...edgeStats.domains]
                      .sort((a, b) => (b.total_requests || 0) - (a.total_requests || 0))
                      .map((domain, i) => (
                        <tr
                          key={i}
                          className={`border-b border-gray-800 ${
                            (domain.errors_5xx || 0) > 0 ? 'bg-red-900/10' : ''
                          }`}
                        >
                          <td className="py-1.5 text-gray-200">{domain.domain || domain.name}</td>
                          <td className="py-1.5 text-right text-gray-400">{(domain.total_requests || 0).toLocaleString()}</td>
                          <td className={`py-1.5 text-right ${(domain.errors_5xx || 0) > 0 ? 'text-red-400 font-medium' : 'text-gray-400'}`}>
                            {(domain.errors_5xx || 0).toLocaleString()}
                          </td>
                        </tr>
                      ))}
                  </tbody>
                </table>
              </div>
            )}
          </div>
        )}
      </Section>
    </div>
  );
}
