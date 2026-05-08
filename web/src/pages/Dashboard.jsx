import { useState, useEffect } from 'react';
import { Link } from 'react-router-dom';
import {
  LayoutDashboard,
  Shield,
  Wifi,
  Container,
  Clock,
  Cpu,
  MemoryStick,
  ArrowRight,
  RefreshCw,
  Globe,
  Server,
  Box,
  GitBranch,
  HardDrive,
  ShieldCheck,
  MonitorSpeaker,
  Zap,
} from 'lucide-react';
import PageHeader from '../components/PageHeader';
import { getDashboard } from '../api/client';

function formatUptime(secs) {
  if (!secs && secs !== 0) return '-';
  const d = Math.floor(secs / 86400);
  const h = Math.floor((secs % 86400) / 3600);
  const m = Math.floor((secs % 3600) / 60);
  if (d > 0) return `${d}j ${h}h ${m}m`;
  if (h > 0) return `${h}h ${m}m`;
  return `${m}m`;
}

// Skeleton placeholder for a stat card
function StatSkeleton() {
  return (
    <div className="bg-gray-800/50 border border-gray-700/50 rounded-lg p-4 animate-pulse">
      <div className="flex items-center gap-3">
        <div className="w-10 h-10 bg-gray-700 rounded-lg" />
        <div className="flex-1">
          <div className="h-3 bg-gray-700 rounded w-16 mb-2" />
          <div className="h-6 bg-gray-700 rounded w-24" />
        </div>
      </div>
    </div>
  );
}

function StatCard({ icon: Icon, label, value, sub, color = 'text-blue-400', to }) {
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

function ProgressBar({ percent, color = 'bg-blue-500' }) {
  const pct = percent ?? 0;
  const barColor = pct > 90 ? 'bg-red-500' : pct > 70 ? 'bg-yellow-500' : color;
  return (
    <div className="w-full h-1.5 bg-gray-700 rounded-full overflow-hidden mt-1">
      <div
        className={`h-full ${barColor} rounded-full transition-all duration-500`}
        style={{ width: `${Math.min(pct, 100)}%` }}
      />
    </div>
  );
}

function ServiceDot({ name, state }) {
  const colors = {
    running: 'bg-green-400',
    starting: 'bg-yellow-400 animate-pulse',
    stopped: 'bg-gray-500',
    failed: 'bg-red-500',
    disabled: 'bg-gray-600',
  };

  return (
    <div className="flex items-center gap-2 px-3 py-1.5">
      <div className={`w-2 h-2 rounded-full ${colors[state] || 'bg-gray-500'}`} />
      <span className="text-sm text-gray-300">{name}</span>
    </div>
  );
}

function Dashboard() {
  const [data, setData] = useState(null);
  const [loading, setLoading] = useState(true);

  const fetchData = async () => {
    try {
      const res = await getDashboard();
      if (res.data.success) {
        setData(res.data);
      }
    } catch (error) {
      console.error('Dashboard fetch error:', error);
    } finally {
      setLoading(false);
    }
  };

  useEffect(() => {
    fetchData();
    const interval = setInterval(fetchData, 15000);
    return () => clearInterval(interval);
  }, []);

  const quickLinks = [
    { to: '/apps', icon: Box, label: 'Applications', color: 'text-purple-400' },
    { to: '/dns', icon: Globe, label: 'DNS / DHCP', color: 'text-blue-400' },
    { to: '/reverseproxy', icon: Server, label: 'Reverse Proxy', color: 'text-cyan-400' },
    { to: '/adblock', icon: ShieldCheck, label: 'AdBlock', color: 'text-green-400' },
    { to: '/updates', icon: RefreshCw, label: 'Mises a jour', color: 'text-yellow-400' },
    { to: '/hosts', icon: MonitorSpeaker, label: 'Hosts', color: 'text-orange-400' },
    { to: '/git', icon: GitBranch, label: 'Git', color: 'text-rose-400' },
    { to: '/energy', icon: Zap, label: 'Energie', color: 'text-emerald-400' },
  ];

  return (
    <div>
      <PageHeader title="Dashboard" icon={LayoutDashboard} />

      {/* System Stats */}
      <div className="p-4 sm:p-6 border-b border-gray-700">
        <div className="grid grid-cols-2 lg:grid-cols-4 gap-3 sm:gap-4">
          {loading ? (
            <>
              <StatSkeleton />
              <StatSkeleton />
              <StatSkeleton />
              <StatSkeleton />
            </>
          ) : (
            <>
              <StatCard
                icon={Clock}
                label="Uptime"
                value={formatUptime(data?.uptime_secs)}
                color="text-emerald-400"
              />
              <div className="bg-gray-800/50 border border-gray-700/50 rounded-lg p-4">
                <div className="flex items-center gap-3">
                  <div className="w-10 h-10 rounded-lg bg-gray-700/50 flex items-center justify-center text-blue-400">
                    <Cpu className="w-5 h-5" />
                  </div>
                  <div className="flex-1 min-w-0">
                    <div className="text-xs text-gray-500 uppercase tracking-wider">CPU</div>
                    <div className="text-xl font-bold text-blue-400 leading-tight">
                      {data?.cpu_percent != null ? `${data.cpu_percent}%` : '-'}
                    </div>
                    <ProgressBar percent={data?.cpu_percent} />
                  </div>
                </div>
              </div>
              <div className="bg-gray-800/50 border border-gray-700/50 rounded-lg p-4">
                <div className="flex items-center gap-3">
                  <div className="w-10 h-10 rounded-lg bg-gray-700/50 flex items-center justify-center text-violet-400">
                    <MemoryStick className="w-5 h-5" />
                  </div>
                  <div className="flex-1 min-w-0">
                    <div className="text-xs text-gray-500 uppercase tracking-wider">RAM</div>
                    <div className="text-xl font-bold text-violet-400 leading-tight">
                      {data?.ram_percent != null ? `${data.ram_percent}%` : '-'}
                    </div>
                    <ProgressBar percent={data?.ram_percent} color="bg-violet-500" />
                  </div>
                </div>
              </div>
              <StatCard
                icon={Container}
                label="Applications"
                value={data?.apps_total != null ? `${data.apps_running || 0}/${data.apps_total}` : '-'}
                sub="actives"
                color="text-purple-400"
                to="/apps"
              />
            </>
          )}
        </div>
      </div>

      {/* Secondary Stats */}
      <div className="p-4 sm:p-6 border-b border-gray-700">
        <div className="grid grid-cols-2 lg:grid-cols-4 gap-3 sm:gap-4">
          {loading ? (
            <>
              <StatSkeleton />
              <StatSkeleton />
              <StatSkeleton />
              <StatSkeleton />
            </>
          ) : (
            <>
              <StatCard
                icon={HardDrive}
                label="Applications"
                value={data?.apps_running != null ? `${data.apps_running}/${data.apps_total}` : '-'}
                sub="connectees"
                color="text-cyan-400"
                to="/apps"
              />
              <StatCard
                icon={Wifi}
                label="DHCP"
                value={data?.dhcp_leases != null ? data.dhcp_leases : '-'}
                sub="appareils"
                color="text-blue-400"
                to="/dns"
              />
              <StatCard
                icon={Shield}
                label="AdBlock"
                value={data?.adblock_domains != null ? data.adblock_domains.toLocaleString() : '-'}
                sub={data?.adblock_enabled ? 'domaines bloques' : 'desactive'}
                color="text-green-400"
                to="/adblock"
              />
              <StatCard
                icon={RefreshCw}
                label="Mises a jour"
                value={data?.updates_available != null ? data.updates_available : '-'}
                sub="disponibles"
                color={data?.updates_available > 0 ? 'text-yellow-400' : 'text-gray-400'}
                to="/updates"
              />
            </>
          )}
        </div>
      </div>

      {/* Services Status */}
      {!loading && data?.services && (
        <div className="p-4 sm:p-6 border-b border-gray-700">
          <h2 className="text-sm font-semibold text-gray-400 uppercase tracking-wider mb-3">Services</h2>
          <div className="bg-gray-800/50 border border-gray-700/50 rounded-lg overflow-hidden">
            <div className="grid grid-cols-2 sm:grid-cols-3 lg:grid-cols-4 divide-x divide-y divide-gray-700/50">
              {data.services.map((svc) => (
                <ServiceDot key={svc.name} name={svc.name} state={svc.state} />
              ))}
            </div>
          </div>
        </div>
      )}

      {/* Quick Links */}
      <div className="p-4 sm:p-6">
        <h2 className="text-sm font-semibold text-gray-400 uppercase tracking-wider mb-3">Acces rapide</h2>
        <div className="grid grid-cols-2 sm:grid-cols-4 gap-2">
          {quickLinks.map(({ to, icon: Icon, label, color }) => (
            <Link
              key={to}
              to={to}
              className="flex items-center gap-3 px-4 py-3 bg-gray-800/50 border border-gray-700/50 rounded-lg hover:bg-gray-800 hover:border-gray-600 transition-colors"
            >
              <Icon className={`w-4 h-4 ${color} shrink-0`} />
              <span className="text-sm text-gray-300">{label}</span>
            </Link>
          ))}
        </div>
      </div>
    </div>
  );
}

export default Dashboard;
