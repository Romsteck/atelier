import { useState, useEffect } from 'react';
import { Server, Search, Globe, Network } from 'lucide-react';
import PageHeader from '../components/PageHeader';
import Section from '../components/Section';
import { getDnsConfig, getDhcpLeases } from '../api/client';

function Dns() {
  const [config, setConfig] = useState(null);
  const [leases, setLeases] = useState([]);
  const [search, setSearch] = useState('');
  const [loading, setLoading] = useState(true);
  const [activeTab, setActiveTab] = useState('dhcp');

  useEffect(() => {
    async function fetchData() {
      try {
        const [configRes, leasesRes] = await Promise.all([
          getDnsConfig(),
          getDhcpLeases()
        ]);

        if (configRes.data.success) {
          const raw = configRes.data.config;
          setConfig({
            interface: raw.dhcp?.interface,
            domain: raw.dhcp?.domain,
            dhcpRange: raw.dhcp?.range_start && raw.dhcp?.range_end
              ? `${raw.dhcp.range_start} - ${raw.dhcp.range_end}`
              : null,
            cacheSize: raw.dns?.cache_size,
            dnsServers: raw.dns?.upstream_servers,
            staticRecords: raw.dns?.static_records || [],
            ipv6: {
              raEnabled: raw.ipv6?.ra_enabled,
            }
          });
        }
        if (leasesRes.data.success) setLeases(leasesRes.data.leases);
      } catch (error) {
        console.error('Error:', error);
      } finally {
        setLoading(false);
      }
    }

    fetchData();
  }, []);

  const filteredLeases = leases.filter(lease =>
    lease.hostname?.toLowerCase().includes(search.toLowerCase()) ||
    lease.ip.includes(search) ||
    lease.mac.toLowerCase().includes(search.toLowerCase())
  );

  function renderDnsTab() {
    return (
      <div className="space-y-px">
        <div className="grid grid-cols-1 sm:grid-cols-2 gap-px">
          <Section title="Serveurs DNS upstream" className="!mb-0">
            <div className="space-y-1">
              {config?.dnsServers?.map(server => (
                <div key={server} className="font-mono text-sm bg-gray-900 px-2 py-1">
                  {server}
                </div>
              ))}
            </div>
          </Section>

          <Section title="Cache DNS" className="!mb-0">
            <div className="text-3xl font-bold text-blue-400">
              {config?.cacheSize || '-'}
            </div>
            <p className="text-sm text-gray-400 mt-1">entrées</p>
          </Section>
        </div>

        <Section title={`Enregistrements DNS (${config?.staticRecords?.length || 0})`}>
          {config?.staticRecords?.length > 0 ? (
            <div className="overflow-x-auto">
              <table className="w-full text-sm min-w-[600px]">
                <thead>
                  <tr className="text-left text-gray-400 border-b border-gray-700">
                    <th className="pb-2">Nom</th>
                    <th className="pb-2">Type</th>
                    <th className="pb-2">Valeur</th>
                    <th className="pb-2">TTL</th>
                    <th className="pb-2">Source</th>
                  </tr>
                </thead>
                <tbody>
                  {config.staticRecords.map((record, i) => (
                    <tr key={i} className="border-b border-gray-700/50 hover:bg-gray-700/30">
                      <td className="py-2 font-mono text-blue-400 break-all">{record.name}</td>
                      <td className="py-2">
                        <span className={`px-2 py-0.5 text-xs font-medium ${
                          record.record_type === 'A' ? 'bg-green-900/50 text-green-400' :
                          record.record_type === 'AAAA' ? 'bg-purple-900/50 text-purple-400' :
                          record.record_type === 'CNAME' ? 'bg-yellow-900/50 text-yellow-400' :
                          'bg-gray-700 text-gray-300'
                        }`}>
                          {record.record_type}
                        </span>
                      </td>
                      <td className="py-2 font-mono text-sm break-all">{record.value}</td>
                      <td className="py-2 text-gray-400">{record.ttl}s</td>
                      <td className="py-2">
                        {record.managed_by === 'hr-edge' ? (
                          <span className="px-2 py-0.5 text-xs font-medium bg-blue-900/50 text-blue-300" title="Auto-généré depuis le reverse proxy">
                            Auto (proxy)
                          </span>
                        ) : (
                          <span className="px-2 py-0.5 text-xs font-medium bg-gray-700 text-gray-300" title="Record utilisateur manuel">
                            Manuel
                          </span>
                        )}
                      </td>
                    </tr>
                  ))}
                </tbody>
              </table>
            </div>
          ) : (
            <p className="text-gray-500 text-sm">Aucun enregistrement DNS statique configuré</p>
          )}
        </Section>
      </div>
    );
  }

  function renderDhcpTab() {
    return (
      <div className="space-y-px">
        <div className="grid grid-cols-1 sm:grid-cols-2 gap-px">
          <Section title="Configuration DHCP" className="!mb-0">
            <dl className="space-y-3 text-sm">
              <div className="flex justify-between">
                <dt className="text-gray-400">Interface</dt>
                <dd className="font-mono text-blue-400">{config?.interface || '-'}</dd>
              </div>
              <div className="flex justify-between">
                <dt className="text-gray-400">Domaine</dt>
                <dd className="font-mono">{config?.domain || '-'}</dd>
              </div>
              <div className="flex justify-between">
                <dt className="text-gray-400">Plage DHCP</dt>
                <dd className="font-mono text-green-400">{config?.dhcpRange || '-'}</dd>
              </div>
            </dl>
          </Section>

          <Section title="Configuration IPv6" contrast className="!mb-0">
            <dl className="space-y-2 text-sm">
              <div className="flex justify-between">
                <dt className="text-gray-400">Router Advertisement</dt>
                <dd className="font-mono">
                  {config?.ipv6?.raEnabled ? (
                    <span className="text-green-400">Activé</span>
                  ) : (
                    <span className="text-gray-500">Désactivé</span>
                  )}
                </dd>
              </div>
            </dl>
          </Section>
        </div>

        <Section title={`Baux DHCP (${filteredLeases.length})`}>
          <div className="mb-4">
            <div className="relative">
              <Search className="w-4 h-4 absolute left-3 top-1/2 -translate-y-1/2 text-gray-400" />
              <input
                type="text"
                placeholder="Rechercher..."
                value={search}
                onChange={e => setSearch(e.target.value)}
                className="w-full sm:w-auto pl-9 pr-4 py-1.5 bg-gray-900 border border-gray-600 text-sm focus:outline-none focus:border-blue-500"
              />
            </div>
          </div>
          <div className="overflow-x-auto">
            <table className="w-full text-sm min-w-[500px]">
              <thead>
                <tr className="text-left text-gray-400 border-b border-gray-700">
                  <th className="pb-2">Hostname</th>
                  <th className="pb-2">Adresse IPv4</th>
                  <th className="pb-2">Adresse MAC</th>
                  <th className="pb-2 hidden sm:table-cell">Expiration</th>
                </tr>
              </thead>
              <tbody>
                {filteredLeases.map(lease => (
                  <tr key={lease.mac} className="border-b border-gray-700/50 hover:bg-gray-700/30">
                    <td className="py-2 font-mono">
                      {lease.hostname || <span className="text-gray-500">-</span>}
                    </td>
                    <td className="py-2 font-mono text-blue-400">{lease.ip}</td>
                    <td className="py-2 font-mono text-gray-400 text-xs">{lease.mac}</td>
                    <td className="py-2 text-gray-400 text-xs hidden sm:table-cell">
                      {new Date(lease.expiry * 1000).toLocaleString('fr-FR')}
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        </Section>
      </div>
    );
  }

  if (loading) {
    return (
      <div className="flex items-center justify-center h-full">
        <div className="animate-spin rounded-full h-12 w-12 border-b-2 border-blue-400"></div>
      </div>
    );
  }

  return (
    <div>
      <PageHeader title="DNS / DHCP" icon={Server} />

      {/* Horizontal tabs on mobile, vertical on desktop */}
      <div className="flex flex-col md:flex-row flex-1">
        {/* Tab bar - horizontal on mobile, vertical sidebar on desktop */}
        <div className="flex md:flex-col md:w-56 border-b md:border-b-0 md:border-r border-gray-700 bg-gray-800/50 flex-shrink-0 overflow-x-auto">
          <button
            onClick={() => setActiveTab('dns')}
            className={`flex items-center gap-2 px-4 py-2.5 text-sm text-left transition-colors whitespace-nowrap ${
              activeTab === 'dns'
                ? 'bg-gray-900 text-blue-400 border-b-2 md:border-b-0 md:border-l-2 border-blue-400'
                : 'text-gray-400 hover:bg-gray-800 hover:text-gray-300 border-b-2 md:border-b-0 md:border-l-2 border-transparent'
            }`}
          >
            <Globe className="w-4 h-4" />
            DNS
          </button>
          <button
            onClick={() => setActiveTab('dhcp')}
            className={`flex items-center gap-2 px-4 py-2.5 text-sm text-left transition-colors whitespace-nowrap ${
              activeTab === 'dhcp'
                ? 'bg-gray-900 text-blue-400 border-b-2 md:border-b-0 md:border-l-2 border-blue-400'
                : 'text-gray-400 hover:bg-gray-800 hover:text-gray-300 border-b-2 md:border-b-0 md:border-l-2 border-transparent'
            }`}
          >
            <Network className="w-4 h-4" />
            DHCP
          </button>
        </div>

        {/* Tab Content */}
        <div className="flex-1 overflow-auto">
          {activeTab === 'dns' && renderDnsTab()}
          {activeTab === 'dhcp' && renderDhcpTab()}
        </div>
      </div>
    </div>
  );
}

export default Dns;
