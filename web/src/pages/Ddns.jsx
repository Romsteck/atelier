import { useState, useEffect } from 'react';
import { Globe, RefreshCw, Clock, Wifi, Pencil, Check, X } from 'lucide-react';
import Card from '../components/Card';
import Button from '../components/Button';
import StatusBadge from '../components/StatusBadge';
import PageHeader from '../components/PageHeader';
import Section from '../components/Section';
import { getDdnsStatus, forceDdnsUpdate, updateDdnsToken, updateDdnsConfig } from '../api/client';

function Ddns() {
  const [status, setStatus] = useState(null);
  const [loading, setLoading] = useState(true);
  const [updating, setUpdating] = useState(false);
  const [editingToken, setEditingToken] = useState(false);
  const [tokenValue, setTokenValue] = useState('');
  const [savingToken, setSavingToken] = useState(false);
  const [tokenError, setTokenError] = useState(null);
  const [editingZoneId, setEditingZoneId] = useState(false);
  const [zoneIdValue, setZoneIdValue] = useState('');
  const [savingZoneId, setSavingZoneId] = useState(false);
  const [zoneIdError, setZoneIdError] = useState(null);
  const [savingProxied, setSavingProxied] = useState(false);

  useEffect(() => {
    fetchStatus();
    const interval = setInterval(fetchStatus, 60000);
    return () => clearInterval(interval);
  }, []);

  async function fetchStatus() {
    try {
      const res = await getDdnsStatus();
      if (res.data.success) {
        setStatus(res.data.status);
      }
    } catch (error) {
      console.error('Error:', error);
    } finally {
      setLoading(false);
    }
  }

  async function handleUpdate() {
    setUpdating(true);
    try {
      await forceDdnsUpdate();
      await fetchStatus();
    } catch (error) {
      console.error('Error updating:', error);
    } finally {
      setUpdating(false);
    }
  }

  async function handleSaveToken() {
    if (!tokenValue.trim()) return;
    setSavingToken(true);
    setTokenError(null);
    try {
      const res = await updateDdnsToken(tokenValue.trim());
      if (res.data.success) {
        setEditingToken(false);
        setTokenValue('');
        await fetchStatus();
      } else {
        setTokenError(res.data.error || 'Erreur');
      }
    } catch (error) {
      setTokenError(error.response?.data?.error || 'Erreur de connexion');
    } finally {
      setSavingToken(false);
    }
  }

  async function handleSaveZoneId() {
    if (!zoneIdValue.trim()) return;
    setSavingZoneId(true);
    setZoneIdError(null);
    try {
      const res = await updateDdnsConfig({ zone_id: zoneIdValue.trim() });
      if (res.data.success) {
        setEditingZoneId(false);
        setZoneIdValue('');
        await fetchStatus();
      } else {
        setZoneIdError(res.data.error || 'Erreur');
      }
    } catch (error) {
      setZoneIdError(error.response?.data?.error || 'Erreur de connexion');
    } finally {
      setSavingZoneId(false);
    }
  }

  async function handleToggleProxied() {
    setSavingProxied(true);
    try {
      const newValue = !status?.config?.proxied;
      const res = await updateDdnsConfig({ proxied: newValue });
      if (res.data.success) {
        await fetchStatus();
      }
    } catch (error) {
      console.error('Error updating proxied:', error);
    } finally {
      setSavingProxied(false);
    }
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
      <PageHeader title="Dynamic DNS (Cloudflare)" icon={Globe}>
        <Button onClick={handleUpdate} loading={updating}>
          <RefreshCw className="w-4 h-4" />
          Forcer la mise à jour
        </Button>
      </PageHeader>


      <Section title="Configuration">
        <div className="grid grid-cols-1 md:grid-cols-2 lg:grid-cols-5 gap-px">
          <Card title="Enregistrement" icon={Globe}>
            <div className="text-lg font-mono text-blue-400 break-all">
              {status?.config?.recordName || '-'}
            </div>
            <p className="text-xs text-gray-500 mt-2">{status?.recordType || 'AAAA'} Record</p>
          </Card>

          <Card title="IPv6 Actuelle" icon={Wifi}>
            <div className="text-sm font-mono text-green-400 break-all">
              {status?.currentIpv6 || 'Non disponible'}
            </div>
            <p className="text-xs text-gray-500 mt-2">
              {`Interface ${status?.interface || 'enp5s0'}`}
            </p>
          </Card>

          <Card title="Zone ID" icon={Globe}>
            {editingZoneId ? (
              <div className="space-y-2">
                <input
                  type="text"
                  value={zoneIdValue}
                  onChange={(e) => setZoneIdValue(e.target.value)}
                  placeholder="Zone ID Cloudflare"
                  className="w-full bg-gray-800 border border-gray-600 px-2 py-1 text-sm font-mono text-white focus:outline-none focus:border-blue-500"
                  autoFocus
                  onKeyDown={(e) => {
                    if (e.key === 'Enter') handleSaveZoneId();
                    if (e.key === 'Escape') { setEditingZoneId(false); setZoneIdValue(''); setZoneIdError(null); }
                  }}
                />
                {zoneIdError && <p className="text-xs text-red-400">{zoneIdError}</p>}
                <div className="flex gap-2">
                  <button
                    onClick={handleSaveZoneId}
                    disabled={savingZoneId || !zoneIdValue.trim()}
                    className="flex items-center gap-1 px-2 py-1 text-xs bg-green-600 hover:bg-green-700 disabled:opacity-50 text-white"
                  >
                    <Check className="w-3 h-3" />
                    {savingZoneId ? '...' : 'Valider'}
                  </button>
                  <button
                    onClick={() => { setEditingZoneId(false); setZoneIdValue(''); setZoneIdError(null); }}
                    className="flex items-center gap-1 px-2 py-1 text-xs bg-gray-600 hover:bg-gray-700 text-white"
                  >
                    <X className="w-3 h-3" />
                    Annuler
                  </button>
                </div>
              </div>
            ) : (
              <div className="flex items-center justify-between">
                <div className="text-xs font-mono text-gray-400 break-all">
                  {status?.config?.zoneId || '-'}
                </div>
                <button
                  onClick={() => { setEditingZoneId(true); setZoneIdValue(status?.config?.zoneId || ''); }}
                  className="p-1 text-gray-500 hover:text-blue-400 transition-colors"
                  title="Modifier le Zone ID"
                >
                  <Pencil className="w-4 h-4" />
                </button>
              </div>
            )}
          </Card>

          <Card title="API Token" icon={Globe}>
            {editingToken ? (
              <div className="space-y-2">
                <input
                  type="password"
                  value={tokenValue}
                  onChange={(e) => setTokenValue(e.target.value)}
                  placeholder="Nouveau token API"
                  className="w-full bg-gray-800 border border-gray-600 px-2 py-1 text-sm font-mono text-white focus:outline-none focus:border-blue-500"
                  autoFocus
                  onKeyDown={(e) => {
                    if (e.key === 'Enter') handleSaveToken();
                    if (e.key === 'Escape') { setEditingToken(false); setTokenValue(''); setTokenError(null); }
                  }}
                />
                {tokenError && <p className="text-xs text-red-400">{tokenError}</p>}
                <div className="flex gap-2">
                  <button
                    onClick={handleSaveToken}
                    disabled={savingToken || !tokenValue.trim()}
                    className="flex items-center gap-1 px-2 py-1 text-xs bg-green-600 hover:bg-green-700 disabled:opacity-50 text-white"
                  >
                    <Check className="w-3 h-3" />
                    {savingToken ? '...' : 'Valider'}
                  </button>
                  <button
                    onClick={() => { setEditingToken(false); setTokenValue(''); setTokenError(null); }}
                    className="flex items-center gap-1 px-2 py-1 text-xs bg-gray-600 hover:bg-gray-700 text-white"
                  >
                    <X className="w-3 h-3" />
                    Annuler
                  </button>
                </div>
              </div>
            ) : (
              <div className="flex items-center justify-between">
                <div className="text-sm font-mono text-gray-500">
                  {status?.config?.apiToken || '-'}
                </div>
                <button
                  onClick={() => setEditingToken(true)}
                  className="p-1 text-gray-500 hover:text-blue-400 transition-colors"
                  title="Modifier le token"
                >
                  <Pencil className="w-4 h-4" />
                </button>
              </div>
            )}
          </Card>

          <Card title="Cloudflare Proxy" icon={Globe}>
            <label className="flex items-center gap-3 cursor-pointer">
              <input
                type="checkbox"
                checked={status?.config?.proxied ?? true}
                onChange={handleToggleProxied}
                disabled={savingProxied}
                className="w-4 h-4 border-gray-600 bg-gray-800 text-blue-500 focus:ring-blue-500 focus:ring-offset-0"
              />
              <span className="text-sm text-gray-300">
                {savingProxied ? 'Enregistrement...' : 'Proxied'}
              </span>
            </label>
            <p className="text-xs text-gray-500 mt-2">
              Activer le proxy Cloudflare sur l&apos;enregistrement DNS
            </p>
          </Card>
        </div>
      </Section>

      <Section title="État / Automatisation" contrast>
        <div className="grid grid-cols-1 lg:grid-cols-2 gap-px">
          <Card title="État" icon={Clock}>
            <dl className="space-y-4">
              <div className="flex justify-between items-center">
                <dt className="text-gray-400">Mode</dt>
                <dd className="font-mono text-sm">
                  <StatusBadge status="unknown">Direct (AAAA)</StatusBadge>
                </dd>
              </div>
              <div className="flex justify-between items-center">
                <dt className="text-gray-400">Dernière mise à jour</dt>
                <dd className="font-mono text-sm">
                  {status?.lastUpdate || 'Jamais'}
                </dd>
              </div>
              <div className="flex justify-between items-center">
                <dt className="text-gray-400">IP Cloudflare</dt>
                <dd className="font-mono text-sm text-purple-400 break-all">
                  {status?.cloudflareIp || '-'}
                </dd>
              </div>
              <div className="flex justify-between items-center">
                <dt className="text-gray-400">Synchronisation</dt>
                <dd>
                  {status?.inSync ? (
                    <StatusBadge status="up">En sync</StatusBadge>
                  ) : (
                    <StatusBadge status="down">Desynchronise</StatusBadge>
                  )}
                </dd>
              </div>
            </dl>
          </Card>

          <Card title="Gestion DNS" icon={RefreshCw}>
            <div className="space-y-3">
              <div className="bg-gray-900 p-3">
                <div className="text-sm font-semibold mb-1">Mise à jour</div>
                <p className="text-xs text-gray-400">
                  Utilisez le bouton &quot;Forcer la mise à jour&quot; pour synchroniser l&apos;enregistrement DNS Cloudflare.
                </p>
              </div>
              <div className="bg-gray-900 p-3">
                <div className="text-sm font-semibold mb-1">Type d&apos;enregistrement</div>
                <code className="text-xs text-green-400">
                  {`AAAA → ${status?.currentIpv6 || '?'} (direct IPv6)`}
                </code>
              </div>
            </div>
          </Card>
        </div>
      </Section>

      <Section title="Logs">
        <Card title="Logs récents" icon={Clock}>
          <div className="bg-gray-900 p-3 max-h-96 overflow-y-auto font-mono text-xs">
            {status?.logs?.length > 0 ? (
              status.logs.map((log, i) => (
                <div
                  key={i}
                  className={`py-1 ${
                    log.includes('ERREUR') ? 'text-red-400' :
                    log.includes('MAJ') || log.includes('CREE') ? 'text-green-400' :
                    'text-gray-400'
                  }`}
                >
                  {log}
                </div>
              ))
            ) : (
              <p className="text-gray-500 text-center py-4">Aucun log</p>
            )}
          </div>
        </Card>
      </Section>
    </div>
  );
}

export default Ddns;
