import { useState, useEffect } from 'react';
import {
  Globe,
  Plus,
  Trash2,
  Settings,
  RefreshCw,
  CheckCircle,
  XCircle,
  Power,
  ExternalLink,
  Lock,
  Server,
  Wifi,
  Pencil,
  Shield,
  Key,
  AlertTriangle,
  FileCode,
  Copy,
  ChevronDown,
  ChevronUp,
} from 'lucide-react';
import Card from '../components/Card';
import Button from '../components/Button';
import PageHeader from '../components/PageHeader';
import Section from '../components/Section';
import {
  getReverseProxyConfig,
  getReverseProxyStatus,
  getReverseProxyHosts,
  addReverseProxyHost,
  updateReverseProxyHost,
  deleteReverseProxyHost,
  toggleReverseProxyHost,
  updateBaseDomain,
  renewCertificates,
  reloadProxy,
  getCertificatesStatus,
  getRustProxyStatus,
  getSystemRoutes,
} from '../api/client';

function ReverseProxy() {
  const [config, setConfig] = useState(null);
  const [status, setStatus] = useState(null);
  const [hosts, setHosts] = useState([]);
  const [rustProxy, setRustProxy] = useState(null);
  const [loading, setLoading] = useState(true);
  const [message, setMessage] = useState(null);

  // System routes
  const [systemRoutes, setSystemRoutes] = useState([]);

  // Tab state
  const [activeTab, setActiveTab] = useState('standalone');

  // Certificate statuses
  const [certStatuses, setCertStatuses] = useState({});

  // Modal states
  const [showAddModal, setShowAddModal] = useState(false);
  const [showConfigModal, setShowConfigModal] = useState(false);
  const [showEditModal, setShowEditModal] = useState(false);
  const [showDomainRequiredModal, setShowDomainRequiredModal] = useState(false);
  const [editingHost, setEditingHost] = useState(null);

  // Form states
  const [hostType, setHostType] = useState('subdomain');
  const [newHost, setNewHost] = useState({ subdomain: '', customDomain: '', targetHost: 'localhost', targetPort: '', localOnly: false, requireAuth: false });
  const [editForm, setEditForm] = useState({ targetHost: '', targetPort: '', localOnly: false, requireAuth: false });
  const [configForm, setConfigForm] = useState({ baseDomain: '' });

  // Action states
  const [saving, setSaving] = useState(false);
  const [renewing, setRenewing] = useState(false);
  const [reloading, setReloading] = useState(false);

  // Integration section state
  const [showIntegration, setShowIntegration] = useState(false);
  const [instructionsCopied, setInstructionsCopied] = useState(false);

  const authInstructions = `# Authentification

Le cookie \`auth_session\` est partage sur votre domaine de base.

## Connexion / Deconnexion

Pour connecter un utilisateur, redirige vers :
\`https://proxy.<votre-domaine>/login?rd=URL_RETOUR\`

Pour deconnecter un utilisateur, redirige vers :
\`https://proxy.<votre-domaine>/logout?rd=URL_RETOUR\`

## Verifier l'utilisateur connecte

### GET /api/auth/me

**Entree :** Cookie \`auth_session\` (envoye automatiquement)

**Sortie (connecte) :**
- \`success\`: true
- \`user.username\`: nom d'utilisateur
- \`user.displayname\`: nom affiche
- \`user.email\`: email
- \`user.groups\`: liste des groupes (ex: ["users", "admins"])

**Sortie (non connecte) :** \`success\`: false

---

### GET /api/auth/check

Verification rapide (sans les details utilisateur).

**Entree :** Cookie \`auth_session\`

**Sortie (connecte) :** \`authenticated\`: true
**Sortie (non connecte) :** \`authenticated\`: false

## Groupes disponibles

- \`admins\` : administrateurs
- \`users\` : utilisateurs standards
- Groupes personnalises crees via la page Utilisateurs`;

  async function copyInstructions() {
    try {
      await navigator.clipboard.writeText(authInstructions);
      setInstructionsCopied(true);
      setTimeout(() => setInstructionsCopied(false), 2000);
    } catch (err) {
      console.error('Failed to copy:', err);
    }
  }

  useEffect(() => {
    fetchData();
  }, []);

  async function fetchData() {
    try {
      const [configRes, statusRes, hostsRes, certsRes, rustRes, sysRoutesRes] = await Promise.all([
        getReverseProxyConfig(),
        getReverseProxyStatus(),
        getReverseProxyHosts(),
        getCertificatesStatus(),
        getRustProxyStatus().catch(() => ({ data: { success: false } })),
        getSystemRoutes().catch(() => ({ data: { success: false } })),
      ]);

      if (configRes.data.success) {
        setConfig(configRes.data.config);
        setConfigForm({ baseDomain: configRes.data.config.baseDomain || '' });
        if (!configRes.data.config.baseDomain) {
          setShowDomainRequiredModal(true);
        }
      }
      if (statusRes.data.success) setStatus(statusRes.data);
      if (hostsRes.data.success) {
        setHosts(hostsRes.data.hosts || []);
      }
      if (certsRes.data.success) setCertStatuses(certsRes.data.certificates || {});
      if (rustRes.data.success) setRustProxy(rustRes.data);
      if (sysRoutesRes.data.success) setSystemRoutes(sysRoutesRes.data.routes || []);
    } catch (error) {
      console.error('Error:', error);
      setMessage({ type: 'error', text: 'Erreur de chargement' });
    } finally {
      setLoading(false);
    }
  }

  // ========== Host handlers ==========
  async function handleAddHost() {
    if (!newHost.targetHost || !newHost.targetPort) {
      setMessage({ type: 'error', text: 'Host et port requis' });
      return;
    }
    if (hostType === 'subdomain' && !newHost.subdomain) {
      setMessage({ type: 'error', text: 'Sous-domaine requis' });
      return;
    }
    if (hostType === 'custom' && !newHost.customDomain) {
      setMessage({ type: 'error', text: 'Domaine personnalise requis' });
      return;
    }

    setSaving(true);
    try {
      const payload = {
        targetHost: newHost.targetHost,
        targetPort: parseInt(newHost.targetPort),
        localOnly: newHost.localOnly,
        requireAuth: newHost.requireAuth
      };
      if (hostType === 'subdomain') payload.subdomain = newHost.subdomain;
      else payload.customDomain = newHost.customDomain;

      const res = await addReverseProxyHost(payload);
      if (res.data.success) {
        setMessage({ type: 'success', text: 'Hote ajoute' });
        setShowAddModal(false);
        setNewHost({ subdomain: '', customDomain: '', targetHost: 'localhost', targetPort: '', localOnly: false, requireAuth: false });
        fetchData();
      } else {
        setMessage({ type: 'error', text: res.data.error });
      }
    } catch (error) {
      setMessage({ type: 'error', text: error.response?.data?.error || 'Erreur' });
    } finally {
      setSaving(false);
    }
  }

  async function handleToggleHost(hostId, enabled) {
    try {
      const res = await toggleReverseProxyHost(hostId, enabled);
      if (res.data.success) fetchData();
      else setMessage({ type: 'error', text: res.data.error });
    } catch (error) {
      setMessage({ type: 'error', text: 'Erreur' });
    }
  }

  async function handleDeleteHost(hostId) {
    if (!confirm('Supprimer cet hote ?')) return;
    try {
      const res = await deleteReverseProxyHost(hostId);
      if (res.data.success) {
        setMessage({ type: 'success', text: 'Hote supprime' });
        fetchData();
      } else {
        setMessage({ type: 'error', text: res.data.error });
      }
    } catch (error) {
      setMessage({ type: 'error', text: 'Erreur' });
    }
  }

  function openEditModal(host) {
    setEditingHost(host);
    setEditForm({ targetHost: host.targetHost, targetPort: String(host.targetPort), localOnly: !!host.localOnly, requireAuth: !!host.requireAuth });
    setShowEditModal(true);
  }

  async function handleEditHost() {
    if (!editForm.targetHost || !editForm.targetPort) {
      setMessage({ type: 'error', text: 'Host et port requis' });
      return;
    }
    setSaving(true);
    try {
      const res = await updateReverseProxyHost(editingHost.id, {
        targetHost: editForm.targetHost,
        targetPort: parseInt(editForm.targetPort),
        localOnly: editForm.localOnly,
        requireAuth: editForm.requireAuth
      });
      if (res.data.success) {
        setMessage({ type: 'success', text: 'Hote modifie' });
        setShowEditModal(false);
        setEditingHost(null);
        fetchData();
      } else {
        setMessage({ type: 'error', text: res.data.error });
      }
    } catch (error) {
      setMessage({ type: 'error', text: error.response?.data?.error || 'Erreur' });
    } finally {
      setSaving(false);
    }
  }

  // ========== Config handlers ==========
  async function handleSaveConfig() {
    setSaving(true);
    setMessage(null);
    try {
      if (configForm.baseDomain !== config?.baseDomain) {
        const domainRes = await updateBaseDomain(configForm.baseDomain);
        if (!domainRes.data.success) {
          setMessage({ type: 'error', text: domainRes.data.error });
          setSaving(false);
          return;
        }
      }
      setMessage({ type: 'success', text: 'Configuration sauvegardee' });
      setShowConfigModal(false);
      setShowDomainRequiredModal(false);
      fetchData();
    } catch (error) {
      setMessage({ type: 'error', text: 'Erreur de sauvegarde' });
    } finally {
      setSaving(false);
    }
  }

  async function handleRenewCerts() {
    setRenewing(true);
    setMessage(null);
    try {
      const res = await renewCertificates();
      if (res.data.success) {
        setMessage({ type: 'success', text: 'Renouvellement declenche' });
        fetchData();
      } else {
        setMessage({ type: 'error', text: res.data.error });
      }
    } catch (error) {
      setMessage({ type: 'error', text: 'Erreur' });
    } finally {
      setRenewing(false);
    }
  }

  async function handleReload() {
    setReloading(true);
    try {
      const res = await reloadProxy();
      if (res.data.success) {
        setMessage({ type: 'success', text: 'Proxy recharge' });
        fetchData();
      } else {
        setMessage({ type: 'error', text: res.data.error });
      }
    } catch (error) {
      setMessage({ type: 'error', text: 'Erreur' });
    } finally {
      setReloading(false);
    }
  }

  if (loading) {
    return (
      <div className="flex items-center justify-center h-full">
        <div className="animate-spin rounded-full h-12 w-12 border-b-2 border-blue-400"></div>
      </div>
    );
  }

  const activeHosts = hosts.filter(h => h.enabled).length;

  const tabs = [
    { id: 'standalone', label: 'Standalone', icon: Globe, count: hosts.length },
    { id: 'system', label: 'System Routes', icon: Server, count: systemRoutes.length },
    { id: 'config', label: 'Configuration', icon: Settings }
  ];

  return (
    <div>
      <PageHeader title="Reverse Proxy" icon={Globe}>
        <Button onClick={handleReload} loading={reloading} variant="secondary">
          <RefreshCw className="w-4 h-4" />
          Recharger
        </Button>
        <Button onClick={handleRenewCerts} loading={renewing} variant="secondary">
          <Lock className="w-4 h-4" />
          Renouveler certs
        </Button>
        {activeTab === 'standalone' && (
          <Button onClick={() => setShowAddModal(true)} disabled={!config?.baseDomain}>
            <Plus className="w-4 h-4" />
            Ajouter hote
          </Button>
        )}
      </PageHeader>

      {/* Message */}
      {message && (
        <div className={`p-4 flex items-center gap-2 ${
          message.type === 'success' ? 'bg-green-900/50 text-green-400' : 'bg-red-900/50 text-red-400'
        }`}>
          {message.type === 'success' ? <CheckCircle className="w-5 h-5" /> : <XCircle className="w-5 h-5" />}
          {message.text}
        </div>
      )}

      {/* Status Cards */}
      <div className="grid grid-cols-1 md:grid-cols-2 lg:grid-cols-4 gap-px">
        <Card title="Proxy" icon={Server}>
          <div className="flex items-center gap-2">
            <div className={`w-3 h-3  ${rustProxy?.running ? 'bg-green-400' : 'bg-red-400'}`} />
            <span className={rustProxy?.running ? 'text-green-400' : 'text-red-400'}>
              {rustProxy?.running ? `Port ${rustProxy.httpsPort || 443}` : 'Hors ligne'}
            </span>
          </div>
          {rustProxy?.running && rustProxy.activeRoutes > 0 && (
            <p className="text-xs text-gray-500 mt-1">{rustProxy.activeRoutes} route{rustProxy.activeRoutes > 1 ? 's' : ''}</p>
          )}
        </Card>

        <Card title="Domaine" icon={Globe}>
          <div className="text-lg font-mono text-blue-400 truncate">
            {config?.baseDomain || 'Non configure'}
          </div>
        </Card>

        <Card title="Certificats" icon={Lock}>
          <div className="flex items-center gap-2">
            <div className="w-3 h-3  bg-green-400" />
            <span className="text-green-400">CA Locale</span>
          </div>
        </Card>

        <Card title="Hotes" icon={Wifi}>
          <div className="text-2xl font-bold text-green-400">
            {activeHosts}
          </div>
          <p className="text-xs text-gray-500">{activeHosts} standalone actif{activeHosts > 1 ? 's' : ''}</p>
        </Card>
      </div>

      {/* Vertical Tabs Layout */}
      <div className="flex flex-col md:flex-row flex-1">
        {/* Tab Sidebar */}
        <div className="flex md:flex-col md:w-56 border-b md:border-b-0 md:border-r border-gray-700 bg-gray-800/50 flex-shrink-0 overflow-x-auto">
          {tabs.map(tab => (
            <button
              key={tab.id}
              onClick={() => setActiveTab(tab.id)}
              className={`flex items-center gap-2 px-4 py-2.5 text-sm text-left transition-colors whitespace-nowrap md:w-full ${
                activeTab === tab.id
                  ? 'bg-gray-900 text-blue-400 border-b-2 md:border-b-0 md:border-l-2 border-blue-400'
                  : 'text-gray-400 hover:bg-gray-800 hover:text-gray-300 border-b-2 md:border-b-0 md:border-l-2 border-transparent'
              }`}
            >
              <tab.icon className="w-4 h-4" />
              {tab.label}
              {tab.count !== undefined && (
                <span className="text-xs bg-gray-700 px-2 py-0.5">{tab.count}</span>
              )}
            </button>
          ))}
        </div>

        {/* Tab Content */}
        <div className="flex-1 overflow-auto">

      {activeTab === 'standalone' && (
        <div className="grid grid-cols-1 lg:grid-cols-3 gap-px">
          <div className="lg:col-span-2">
            <Card title="Hotes standalone" icon={Globe}>
              {hosts.length === 0 ? (
                <div className="text-center py-8 text-gray-500">
                  <Globe className="w-12 h-12 mx-auto mb-2 opacity-50" />
                  <p>Aucun hote standalone</p>
                  <p className="text-xs mt-2">Pour les services qui ne suivent pas le pattern frontend/API</p>
                </div>
              ) : (
                <div className="overflow-x-auto">
                  <table className="w-full text-sm">
                    <thead>
                      <tr className="text-left text-gray-400 border-b border-gray-700">
                        <th className="pb-2">Domaine</th>
                        <th className="pb-2">Cible</th>
                        <th className="pb-2">SSL</th>
                        <th className="pb-2">Status</th>
                        <th className="pb-2 text-right">Actions</th>
                      </tr>
                    </thead>
                    <tbody>
                      {hosts.map(host => {
                        const certStatus = certStatuses[host.id];
                        return (
                          <tr key={host.id} className="border-b border-gray-700/50">
                            <td className="py-3">
                              <div className="flex items-center gap-2 flex-wrap">
                                <ExternalLink className="w-4 h-4 text-gray-500" />
                                <a
                                  href={`https://${host.customDomain || `${host.subdomain}.${config?.baseDomain}`}`}
                                  target="_blank"
                                  rel="noopener noreferrer"
                                  className="font-mono text-blue-400 hover:underline"
                                >
                                  {host.customDomain || `${host.subdomain}.${config?.baseDomain}`}
                                </a>
                                {host.localOnly && (
                                  <span className="flex items-center gap-1 text-xs text-yellow-400 bg-yellow-900/30 px-2 py-0.5">
                                    <Shield className="w-3 h-3" />
                                    Local
                                  </span>
                                )}
                                {host.requireAuth && (
                                  <span className="flex items-center gap-1 text-xs text-purple-400 bg-purple-900/30 px-2 py-0.5">
                                    <Key className="w-3 h-3" />
                                    Auth
                                  </span>
                                )}
                              </div>
                            </td>
                            <td className="py-3 font-mono text-sm text-gray-300">
                              {host.targetHost}:{host.targetPort}
                            </td>
                            <td className="py-3">
                              {certStatus ? (
                                <span className={`flex items-center gap-1 text-xs px-2 py-0.5${
                                  certStatus.valid
                                    ? certStatus.daysRemaining <= 14 ? 'text-yellow-400 bg-yellow-900/30' : 'text-green-400 bg-green-900/30'
                                    : 'text-red-400 bg-red-900/30'
                                }`}>
                                  <Lock className="w-3 h-3" />
                                  {certStatus.valid ? (certStatus.daysRemaining <= 14 ? `${certStatus.daysRemaining}j` : 'OK') : 'Erreur'}
                                </span>
                              ) : <span className="text-xs text-gray-500">-</span>}
                            </td>
                            <td className="py-3">
                              <button
                                onClick={() => handleToggleHost(host.id, !host.enabled)}
                                className={`p-1.5transition-colors ${
                                  host.enabled ? 'text-green-400 bg-green-900/30 hover:bg-green-900/50' : 'text-gray-500 bg-gray-700/30 hover:bg-gray-700/50'
                                }`}
                              >
                                <Power className="w-4 h-4" />
                              </button>
                            </td>
                            <td className="py-3 text-right">
                              <div className="flex justify-end gap-1">
                                <button onClick={() => openEditModal(host)} className="text-blue-400 hover:text-blue-300 p-1">
                                  <Pencil className="w-4 h-4" />
                                </button>
                                <button onClick={() => handleDeleteHost(host.id)} className="text-red-400 hover:text-red-300 p-1">
                                  <Trash2 className="w-4 h-4" />
                                </button>
                              </div>
                            </td>
                          </tr>
                        );
                      })}
                    </tbody>
                  </table>
                </div>
              )}
            </Card>
          </div>

          {/* Integration Card */}
          <div>
            <Card
              title="Integration Auth"
              icon={FileCode}
              actions={
                <div className="flex gap-2">
                  <Button onClick={copyInstructions} variant="secondary" className="text-sm">
                    {instructionsCopied ? <><CheckCircle className="w-4 h-4" /> Copie !</> : <><Copy className="w-4 h-4" /> Copier</>}
                  </Button>
                  <button onClick={() => setShowIntegration(!showIntegration)} className="text-gray-400 hover:text-gray-300 p-2">
                    {showIntegration ? <ChevronUp className="w-4 h-4" /> : <ChevronDown className="w-4 h-4" />}
                  </button>
                </div>
              }
            >
              <p className="text-sm text-gray-400 mb-3">Instructions pour integrer l&apos;authentification.</p>
              {showIntegration && (
                <pre className="text-xs text-gray-300 whitespace-pre-wrap font-mono overflow-x-auto max-h-96 overflow-y-auto bg-gray-900p-4">
                  {authInstructions}
                </pre>
              )}
            </Card>
          </div>
        </div>
      )}

      {activeTab === 'system' && (
        <Card title="System Routes" icon={Server}>
          {systemRoutes.length === 0 ? (
            <div className="text-center py-8 text-gray-500">
              <Server className="w-12 h-12 mx-auto mb-2 opacity-50" />
              <p>Aucune route systeme</p>
              <p className="text-xs mt-2">Les routes apparaissent automatiquement pour chaque application enregistree</p>
            </div>
          ) : (
            <div className="overflow-x-auto">
              <table className="w-full text-sm">
                <thead>
                  <tr className="text-left text-gray-400 border-b border-gray-700">
                    <th className="pb-2">Domaine</th>
                    <th className="pb-2">Port</th>
                    <th className="pb-2">Auth</th>
                  </tr>
                </thead>
                <tbody>
                  {systemRoutes.map((route, i) => (
                    <tr key={i} className="border-b border-gray-700/50">
                      <td className="py-3">
                        <div className="flex items-center gap-2">
                          <ExternalLink className="w-4 h-4 text-gray-500" />
                          <a
                            href={`https://${route.domain}`}
                            target="_blank"
                            rel="noopener noreferrer"
                            className="font-mono text-blue-400 hover:underline"
                          >
                            {route.domain}
                          </a>
                        </div>
                      </td>
                      <td className="py-3 font-mono text-sm text-gray-300">
                        :{(route.target || '').split(':').pop()}
                      </td>
                      <td className="py-3">
                        {route.auth_required ? (
                          <span className="text-xs bg-yellow-900/30 text-yellow-400 px-2 py-0.5 rounded">Auth</span>
                        ) : (
                          <span className="text-xs bg-green-900/30 text-green-400 px-2 py-0.5 rounded">Public</span>
                        )}
                      </td>
                    </tr>
                  ))}
                </tbody>
              </table>
            </div>
          )}
        </Card>
      )}

      {activeTab === 'config' && (
        <div className="space-y-px">
        <div className="grid grid-cols-1 lg:grid-cols-2 gap-px">
          {/* Domain Config */}
          <Card title="Domaine de base" icon={Globe}>
            <div className="space-y-4">
              <div>
                <label className="block text-sm text-gray-400 mb-1">Domaine</label>
                <div className="flex gap-2">
                  <input
                    type="text"
                    value={configForm.baseDomain}
                    onChange={e => setConfigForm({ ...configForm, baseDomain: e.target.value })}
                    className="flex-1 px-3 py-2 bg-gray-900 border border-gray-600text-sm"
                    placeholder="example.com"
                  />
                  <Button onClick={handleSaveConfig} loading={saving} disabled={configForm.baseDomain === config?.baseDomain}>
                    Sauver
                  </Button>
                </div>
              </div>
              <div className="text-xs text-gray-500 space-y-1">
                <p>Dashboard: <span className="font-mono text-blue-400">proxy.{configForm.baseDomain || 'domain.com'}</span></p>
                <p>Auth: <span className="font-mono text-blue-400">auth.{configForm.baseDomain || 'domain.com'}</span></p>
              </div>
            </div>
          </Card>

        </div>
        </div>
      )}

        </div>{/* end tab content */}
      </div>{/* end flex layout */}

      {/* Add Host Modal */}
      {showAddModal && (
        <div className="fixed inset-0 bg-black/50 flex items-center justify-center z-50 p-4">
          <div className="bg-gray-800 p-6 w-full max-w-md border border-gray-700">
            <h2 className="text-xl font-bold mb-4">Ajouter un hote standalone</h2>
            <div className="space-y-4">
              <div className="flex gap-2">
                <button onClick={() => setHostType('subdomain')} className={`flex-1 py-2text-sm ${hostType === 'subdomain' ? 'bg-blue-600 text-white' : 'bg-gray-700 text-gray-300'}`}>Sous-domaine</button>
                <button onClick={() => setHostType('custom')} className={`flex-1 py-2text-sm ${hostType === 'custom' ? 'bg-blue-600 text-white' : 'bg-gray-700 text-gray-300'}`}>Domaine perso</button>
              </div>

              {hostType === 'subdomain' ? (
                <div>
                  <label className="block text-sm text-gray-400 mb-1">Sous-domaine</label>
                  <div className="flex">
                    <input type="text" placeholder="app" value={newHost.subdomain} onChange={e => setNewHost({ ...newHost, subdomain: e.target.value })} className="flex-1 px-3 py-2 bg-gray-900 border border-gray-600 text-sm" />
                    <span className="px-3 py-2 bg-gray-700 border border-l-0 border-gray-600 text-gray-400 text-sm">.{config?.baseDomain}</span>
                  </div>
                </div>
              ) : (
                <div>
                  <label className="block text-sm text-gray-400 mb-1">Domaine complet</label>
                  <input type="text" placeholder="app.example.com" value={newHost.customDomain} onChange={e => setNewHost({ ...newHost, customDomain: e.target.value })} className="w-full px-3 py-2 bg-gray-900 border border-gray-600text-sm" />
                </div>
              )}

              <div>
                <label className="block text-sm text-gray-400 mb-1">Hote cible</label>
                <input type="text" placeholder="localhost" value={newHost.targetHost} onChange={e => setNewHost({ ...newHost, targetHost: e.target.value })} className="w-full px-3 py-2 bg-gray-900 border border-gray-600text-sm" />
              </div>

              <div>
                <label className="block text-sm text-gray-400 mb-1">Port</label>
                <input type="number" placeholder="3000" value={newHost.targetPort} onChange={e => setNewHost({ ...newHost, targetPort: e.target.value })} className="w-full px-3 py-2 bg-gray-900 border border-gray-600text-sm" />
              </div>

              <div onClick={() => setNewHost({ ...newHost, localOnly: !newHost.localOnly })} className={`flex items-center gap-3 p-3 border cursor-pointer ${newHost.localOnly ? 'bg-yellow-900/30 border-yellow-600' : 'bg-gray-900/50 border-gray-700'}`}>
                <Shield className="w-5 h-5" />
                <div className="flex-1"><div className="text-sm">Reseau local uniquement</div></div>
                <div className={`w-10 h-6  ${newHost.localOnly ? 'bg-yellow-600' : 'bg-gray-600'}`}><div className={`w-4 h-4 bg-white  mt-1 ${newHost.localOnly ? 'translate-x-5' : 'translate-x-1'}`} /></div>
              </div>

              <div onClick={() => setNewHost({ ...newHost, requireAuth: !newHost.requireAuth })} className={`flex items-center gap-3 p-3 border cursor-pointer ${newHost.requireAuth ? 'bg-purple-900/30 border-purple-600' : 'bg-gray-900/50 border-gray-700'}`}>
                <Key className="w-5 h-5" />
                <div className="flex-1"><div className="text-sm">Authentification requise</div></div>
                <div className={`w-10 h-6  ${newHost.requireAuth ? 'bg-purple-600' : 'bg-gray-600'}`}><div className={`w-4 h-4 bg-white  mt-1 ${newHost.requireAuth ? 'translate-x-5' : 'translate-x-1'}`} /></div>
              </div>
            </div>
            <div className="flex justify-end gap-2 mt-6">
              <Button variant="secondary" onClick={() => setShowAddModal(false)}>Annuler</Button>
              <Button onClick={handleAddHost} loading={saving}>Ajouter</Button>
            </div>
          </div>
        </div>
      )}

      {/* Edit Host Modal */}
      {showEditModal && editingHost && (
        <div className="fixed inset-0 bg-black/50 flex items-center justify-center z-50 p-4">
          <div className="bg-gray-800 p-6 w-full max-w-md border border-gray-700">
            <h2 className="text-xl font-bold mb-4">Modifier l&apos;hote</h2>
            <div className="space-y-4">
              <div>
                <label className="block text-sm text-gray-400 mb-1">Domaine</label>
                <div className="px-3 py-2 bg-gray-900/50 border border-gray-700text-sm font-mono text-gray-400">
                  {editingHost.customDomain || `${editingHost.subdomain}.${config?.baseDomain}`}
                </div>
              </div>
              <div>
                <label className="block text-sm text-gray-400 mb-1">Hote cible</label>
                <input type="text" value={editForm.targetHost} onChange={e => setEditForm({ ...editForm, targetHost: e.target.value })} className="w-full px-3 py-2 bg-gray-900 border border-gray-600text-sm" />
              </div>
              <div>
                <label className="block text-sm text-gray-400 mb-1">Port</label>
                <input type="number" value={editForm.targetPort} onChange={e => setEditForm({ ...editForm, targetPort: e.target.value })} className="w-full px-3 py-2 bg-gray-900 border border-gray-600text-sm" />
              </div>
              <div onClick={() => setEditForm({ ...editForm, localOnly: !editForm.localOnly })} className={`flex items-center gap-3 p-3 border cursor-pointer ${editForm.localOnly ? 'bg-yellow-900/30 border-yellow-600' : 'bg-gray-900/50 border-gray-700'}`}>
                <Shield className="w-5 h-5" />
                <div className="flex-1"><div className="text-sm">Reseau local uniquement</div></div>
                <div className={`w-10 h-6  ${editForm.localOnly ? 'bg-yellow-600' : 'bg-gray-600'}`}><div className={`w-4 h-4 bg-white  mt-1 ${editForm.localOnly ? 'translate-x-5' : 'translate-x-1'}`} /></div>
              </div>
              <div onClick={() => setEditForm({ ...editForm, requireAuth: !editForm.requireAuth })} className={`flex items-center gap-3 p-3 border cursor-pointer ${editForm.requireAuth ? 'bg-purple-900/30 border-purple-600' : 'bg-gray-900/50 border-gray-700'}`}>
                <Key className="w-5 h-5" />
                <div className="flex-1"><div className="text-sm">Authentification requise</div></div>
                <div className={`w-10 h-6  ${editForm.requireAuth ? 'bg-purple-600' : 'bg-gray-600'}`}><div className={`w-4 h-4 bg-white  mt-1 ${editForm.requireAuth ? 'translate-x-5' : 'translate-x-1'}`} /></div>
              </div>

            </div>
            <div className="flex justify-end gap-2 mt-6">
              <Button variant="secondary" onClick={() => { setShowEditModal(false); setEditingHost(null); }}>Annuler</Button>
              <Button onClick={handleEditHost} loading={saving}>Sauvegarder</Button>
            </div>
          </div>
        </div>
      )}

      {/* Domain Required Modal */}
      {showDomainRequiredModal && (
        <div className="fixed inset-0 bg-black/50 flex items-center justify-center z-50 p-4">
          <div className="bg-gray-800 p-6 w-full max-w-md border border-gray-700">
            <h2 className="text-xl font-bold mb-4 flex items-center gap-2">
              <AlertTriangle className="w-5 h-5 text-yellow-400" />
              Configuration requise
            </h2>
            <p className="text-gray-300 mb-4">Veuillez configurer un domaine de base.</p>
            <div className="space-y-4">
              <div>
                <label className="block text-sm text-gray-400 mb-1">Domaine de base</label>
                <input type="text" placeholder="example.com" value={configForm.baseDomain} onChange={e => setConfigForm({ ...configForm, baseDomain: e.target.value })} className="w-full px-3 py-2 bg-gray-900 border border-gray-600text-sm" />
              </div>
            </div>
            <div className="flex justify-end gap-2 mt-6">
              <Button onClick={handleSaveConfig} loading={saving} disabled={!configForm.baseDomain}>Configurer</Button>
            </div>
          </div>
        </div>
      )}

      {/* Config Modal (legacy, kept for edit button) */}
      {showConfigModal && (
        <div className="fixed inset-0 bg-black/50 flex items-center justify-center z-50 p-4">
          <div className="bg-gray-800 p-6 w-full max-w-md border border-gray-700">
            <h2 className="text-xl font-bold mb-4">Configuration</h2>
            <div className="space-y-4">
              <div>
                <label className="block text-sm text-gray-400 mb-1">Domaine de base</label>
                <input type="text" placeholder="example.com" value={configForm.baseDomain} onChange={e => setConfigForm({ ...configForm, baseDomain: e.target.value })} className="w-full px-3 py-2 bg-gray-900 border border-gray-600text-sm" />
              </div>
            </div>
            <div className="flex justify-end gap-2 mt-6">
              <Button variant="secondary" onClick={() => setShowConfigModal(false)}>Annuler</Button>
              <Button onClick={handleSaveConfig} loading={saving}>Sauvegarder</Button>
            </div>
          </div>
        </div>
      )}
    </div>
  );
}

export default ReverseProxy;
