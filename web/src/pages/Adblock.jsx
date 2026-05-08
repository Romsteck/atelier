import { useState, useEffect } from 'react';
import { Shield, RefreshCw, Plus, Trash2, Search, ExternalLink } from 'lucide-react';
import Card from '../components/Card';
import Button from '../components/Button';
import PageHeader from '../components/PageHeader';
import Section from '../components/Section';
import {
  getAdblockStats,
  getWhitelist,
  addToWhitelist,
  removeFromWhitelist,
  updateAdblockLists,
  searchBlocked
} from '../api/client';

function Adblock() {
  const [stats, setStats] = useState(null);
  const [whitelist, setWhitelist] = useState([]);
  const [newDomain, setNewDomain] = useState('');
  const [searchQuery, setSearchQuery] = useState('');
  const [searchResults, setSearchResults] = useState([]);
  const [loading, setLoading] = useState(true);
  const [updating, setUpdating] = useState(false);
  const [searching, setSearching] = useState(false);

  useEffect(() => {
    fetchData();
  }, []);

  async function fetchData() {
    try {
      const [statsRes, whitelistRes] = await Promise.all([
        getAdblockStats(),
        getWhitelist()
      ]);

      if (statsRes.data.success) setStats(statsRes.data.stats);
      if (whitelistRes.data.success) setWhitelist(whitelistRes.data.domains);
    } catch (error) {
      console.error('Error:', error);
    } finally {
      setLoading(false);
    }
  }

  async function handleUpdate() {
    setUpdating(true);
    try {
      await updateAdblockLists();
      await fetchData();
    } catch (error) {
      console.error('Error updating:', error);
    } finally {
      setUpdating(false);
    }
  }

  async function handleAddWhitelist() {
    if (!newDomain) return;
    try {
      await addToWhitelist(newDomain);
      setNewDomain('');
      await fetchData();
    } catch (error) {
      console.error('Error adding to whitelist:', error);
    }
  }

  async function handleRemoveWhitelist(domain) {
    try {
      await removeFromWhitelist(domain);
      await fetchData();
    } catch (error) {
      console.error('Error removing from whitelist:', error);
    }
  }

  async function handleSearch() {
    if (searchQuery.length < 3) return;
    setSearching(true);
    try {
      const res = await searchBlocked(searchQuery);
      if (res.data.success) {
        setSearchResults(res.data.results);
      }
    } catch (error) {
      console.error('Error searching:', error);
    } finally {
      setSearching(false);
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
      <PageHeader title="AdBlock DNS" icon={Shield}>
        <Button onClick={handleUpdate} loading={updating} variant="success">
          <RefreshCw className="w-4 h-4" />
          Mettre à jour les listes
        </Button>
      </PageHeader>

      <Section title="Vue d'ensemble">
        <div className="grid grid-cols-1 md:grid-cols-3 gap-px">
          <Card title="Domaines Bloqués" icon={Shield}>
            <div className="text-4xl font-bold text-green-400">
              {stats?.domainCount?.toLocaleString() || 0}
            </div>
            <p className="text-sm text-gray-400 mt-2">
              Dernière MAJ: {stats?.lastUpdate ? new Date(stats.lastUpdate).toLocaleString('fr-FR') : '-'}
            </p>
          </Card>

          <Card title="Sources" icon={ExternalLink}>
            <div className="text-4xl font-bold text-blue-400">
              {stats?.sources?.length || 0}
            </div>
            <p className="text-sm text-gray-400 mt-2">listes actives</p>
          </Card>

          <Card title="Whitelist" icon={Shield}>
            <div className="text-4xl font-bold text-yellow-400">
              {whitelist.length}
            </div>
            <p className="text-sm text-gray-400 mt-2">domaines autorisés</p>
          </Card>
        </div>
      </Section>

      <Section title="Sources / Whitelist" contrast>
        <div className="grid grid-cols-1 lg:grid-cols-2 gap-px">
          <Card title="Sources de blocage" icon={ExternalLink}>
            <div className="space-y-3">
              {stats?.sources?.map((source, i) => (
                <div key={i} className="bg-gray-900 p-3">
                  <div className="font-semibold text-sm">{source.name}</div>
                  <a
                    href={source.url}
                    target="_blank"
                    rel="noopener noreferrer"
                    className="text-xs text-blue-400 hover:underline break-all"
                  >
                    {source.url}
                  </a>
                </div>
              ))}
            </div>
          </Card>

          <Card title="Whitelist" icon={Shield}>
            <div className="space-y-px">
              <div className="flex gap-2">
                <input
                  type="text"
                  placeholder="domaine.com"
                  value={newDomain}
                  onChange={e => setNewDomain(e.target.value)}
                  onKeyDown={e => e.key === 'Enter' && handleAddWhitelist()}
                  className="flex-1 px-3 py-2 bg-gray-900 border border-gray-600 text-sm focus:outline-none focus:border-blue-500"
                />
                <Button onClick={handleAddWhitelist}>
                  <Plus className="w-4 h-4" />
                </Button>
              </div>

              <div className="space-y-2 max-h-64 overflow-y-auto">
                {whitelist.length === 0 ? (
                  <p className="text-gray-500 text-sm text-center py-4">Whitelist vide</p>
                ) : (
                  whitelist.map(domain => (
                    <div
                      key={domain}
                      className="flex items-center justify-between bg-gray-900 px-3 py-2"
                    >
                      <span className="font-mono text-sm">{domain}</span>
                      <button
                        onClick={() => handleRemoveWhitelist(domain)}
                        className="text-red-400 hover:text-red-300"
                      >
                        <Trash2 className="w-4 h-4" />
                      </button>
                    </div>
                  ))
                )}
              </div>
            </div>
          </Card>
        </div>
      </Section>

      <Section title="Recherche">
        <Card title="Rechercher un domaine bloqué" icon={Search}>
          <div className="space-y-px">
            <div className="flex gap-2">
              <input
                type="text"
                placeholder="Rechercher (min. 3 caractères)..."
                value={searchQuery}
                onChange={e => setSearchQuery(e.target.value)}
                onKeyDown={e => e.key === 'Enter' && handleSearch()}
                className="flex-1 px-3 py-2 bg-gray-900 border border-gray-600 text-sm focus:outline-none focus:border-blue-500"
              />
              <Button onClick={handleSearch} loading={searching}>
                <Search className="w-4 h-4" />
                Rechercher
              </Button>
            </div>

            {searchResults.length > 0 && (
              <div className="bg-gray-900 p-3 max-h-64 overflow-y-auto">
                <p className="text-sm text-gray-400 mb-2">{searchResults.length} résultats</p>
                <div className="space-y-1">
                  {searchResults.map((domain, i) => (
                    <div key={i} className="font-mono text-xs text-red-400">
                      {domain}
                    </div>
                  ))}
                </div>
              </div>
            )}
          </div>
        </Card>
      </Section>

      <Section title="Logs" contrast>
        <Card title="Logs récents" icon={Shield}>
          <div className="bg-gray-900 p-3 max-h-64 overflow-y-auto font-mono text-xs">
            {stats?.logs?.slice(-20).map((log, i) => (
              <div key={i} className="text-gray-400 py-0.5">
                {log}
              </div>
            ))}
          </div>
        </Card>
      </Section>
    </div>
  );
}

export default Adblock;
