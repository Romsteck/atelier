import { useState, useEffect } from 'react';
import {
  Lock,
  RefreshCw,
  CheckCircle,
  AlertTriangle,
  Globe,
  ExternalLink,
  Shield
} from 'lucide-react';
import Button from '../components/Button';
import PageHeader from '../components/PageHeader';
import Section from '../components/Section';

const Certificates = () => {
  const [status, setStatus] = useState(null);
  const [certificates, setCertificates] = useState([]);
  const [loading, setLoading] = useState(true);
  const [renewing, setRenewing] = useState(false);
  const [message, setMessage] = useState(null);

  useEffect(() => {
    fetchData();
  }, []);

  async function fetchData() {
    try {
      const [statusRes, certsRes] = await Promise.all([
        fetch('/api/acme/status'),
        fetch('/api/acme/certificates'),
      ]);

      const statusData = await statusRes.json();
      const certsData = await certsRes.json();

      setStatus(statusData);
      if (certsData.success) {
        setCertificates(certsData.certificates || []);
      }
    } catch (error) {
      console.error('Error fetching ACME data:', error);
      setMessage({ type: 'error', text: 'Erreur de chargement' });
    } finally {
      setLoading(false);
    }
  }

  async function handleRenewAll() {
    setRenewing(true);
    setMessage(null);

    try {
      const response = await fetch('/api/acme/renew', {
        method: 'POST',
      });

      const data = await response.json();

      if (data.success) {
        setMessage({ type: 'success', text: 'Renouvellement effectue avec succes' });
        fetchData();
      } else {
        setMessage({ type: 'error', text: data.error || 'Erreur lors du renouvellement' });
      }
    } catch (error) {
      setMessage({ type: 'error', text: error.message });
    } finally {
      setRenewing(false);
    }
  }

  function formatDate(dateString) {
    return new Date(dateString).toLocaleDateString('fr-FR', {
      day: 'numeric',
      month: 'short',
      year: 'numeric',
    });
  }

  function getDaysUntilExpiry(expiresAt) {
    const now = new Date();
    const expiry = new Date(expiresAt);
    const diffTime = expiry - now;
    const diffDays = Math.ceil(diffTime / (1000 * 60 * 60 * 24));
    return diffDays;
  }

  function getTypeLabel(cert) {
    if (cert.type_display) return cert.type_display;
    switch (cert.type) {
      case 'global':
        return 'Global (Dashboard)';
      case 'legacy_code':
        return 'Code Server (Legacy)';
      case 'app':
        return `App: ${cert.id?.replace('app-', '') || ''}`;
      default:
        return cert.type || cert.wildcard_type || '';
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
      <PageHeader title="Certificats TLS" icon={Lock} />

      {message && (
        <div
          className={`px-6 py-2 text-sm ${
            message.type === 'error'
              ? 'bg-red-500/20 text-red-400'
              : 'bg-green-500/20 text-green-400'
          }`}
        >
          {message.text}
        </div>
      )}

      <Section title="Fournisseur">
        <div className="flex items-center justify-between">
          <div className="flex items-center gap-3">
            <Shield className="w-4 h-4 text-blue-400" />
            <span className="text-sm font-medium">Let's Encrypt</span>
            <CheckCircle className="w-4 h-4 text-green-400" />
            <span className="text-sm text-green-400">Actif</span>
            <span className="px-1.5 py-0.5 bg-blue-900/30 text-blue-300 text-xs rounded">
              Wildcards
            </span>
            <span className="text-xs text-gray-500">Renouvellement auto 30j avant expiration</span>
          </div>
          <div className="flex items-center gap-3">
            <Button
              onClick={handleRenewAll}
              disabled={renewing}
              variant="outline"
              size="sm"
            >
              <RefreshCw className={`w-3.5 h-3.5 mr-1.5 ${renewing ? 'animate-spin' : ''}`} />
              {renewing ? 'Renouvellement...' : 'Forcer le renouvellement'}
            </Button>
            <a
              href="https://letsencrypt.org/"
              target="_blank"
              rel="noopener noreferrer"
              className="flex items-center gap-1 text-xs text-gray-500 hover:text-blue-300"
            >
              <ExternalLink className="w-3 h-3" />
              letsencrypt.org
            </a>
          </div>
        </div>
      </Section>

      <Section title={`Certificats (${certificates.length})`}>
        {certificates.length === 0 ? (
          <div className="text-center py-4 text-gray-400 text-sm">
            Aucun certificat. Les certificats seront emis automatiquement.
          </div>
        ) : (
          <div className="overflow-x-auto"><table className="w-full text-sm min-w-[600px]">
            <thead>
              <tr className="text-xs text-gray-500 uppercase tracking-wider border-b border-gray-700/50">
                <th className="text-left py-1.5 font-medium">Domaine</th>
                <th className="text-left py-1.5 font-medium">Type</th>
                <th className="text-left py-1.5 font-medium">Emission</th>
                <th className="text-left py-1.5 font-medium">Expiration</th>
                <th className="text-left py-1.5 font-medium">Statut</th>
                <th className="text-right py-1.5">
                  <button
                    onClick={fetchData}
                    className="text-gray-500 hover:text-gray-300 p-0.5"
                  >
                    <RefreshCw className="w-3.5 h-3.5" />
                  </button>
                </th>
              </tr>
            </thead>
            <tbody>
              {certificates.map((cert) => {
                const daysUntilExpiry = getDaysUntilExpiry(cert.expires_at);
                const needsRenewal = daysUntilExpiry < 30;
                const expired = daysUntilExpiry < 0;

                return (
                  <tr
                    key={cert.id}
                    className={`border-b border-gray-800 hover:bg-gray-800/30 ${
                      expired
                        ? 'bg-red-900/5'
                        : needsRenewal
                        ? 'bg-orange-900/5'
                        : ''
                    }`}
                  >
                    <td className="py-1.5">
                      <div className="flex items-center gap-1.5">
                        <Globe className="w-3.5 h-3.5 text-blue-400 shrink-0" />
                        <span className="font-medium">
                          {cert.domains && cert.domains.length > 0
                            ? cert.domains[0]
                            : cert.id || 'Certificat'}
                        </span>
                      </div>
                    </td>
                    <td className="py-1.5">
                      <span className={`px-1.5 py-0.5 text-xs rounded ${
                        cert.type === 'app'
                          ? 'bg-purple-900/30 text-purple-300'
                          : cert.type === 'global'
                          ? 'bg-blue-900/30 text-blue-300'
                          : 'bg-gray-700/50 text-gray-400'
                      }`}>
                        {getTypeLabel(cert)}
                      </span>
                    </td>
                    <td className="py-1.5 text-gray-400">
                      {formatDate(cert.issued_at)}
                    </td>
                    <td className="py-1.5 text-gray-400">
                      {formatDate(cert.expires_at)}
                    </td>
                    <td className="py-1.5">
                      {expired ? (
                        <span className="flex items-center gap-1 text-red-400">
                          <AlertTriangle className="w-3.5 h-3.5" />
                          Expire ({Math.abs(daysUntilExpiry)}j)
                        </span>
                      ) : needsRenewal ? (
                        <span className="flex items-center gap-1 text-orange-400">
                          <AlertTriangle className="w-3.5 h-3.5" />
                          Renouvellement ({daysUntilExpiry}j)
                        </span>
                      ) : (
                        <span className="flex items-center gap-1 text-green-400">
                          <CheckCircle className="w-3.5 h-3.5" />
                          Valide ({daysUntilExpiry}j)
                        </span>
                      )}
                    </td>
                    <td></td>
                  </tr>
                );
              })}
            </tbody>
          </table></div>
        )}
      </Section>
    </div>
  );
};

export default Certificates;
