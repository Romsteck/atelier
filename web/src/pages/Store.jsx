import { useState, useEffect, useCallback, useRef } from 'react';
import {
  Store as StoreIcon, Package, ArrowLeft, Download, CheckCircle,
  RefreshCw, Loader2, Tag, Search, X, ChevronDown, ChevronUp,
  Smartphone, AlertCircle, Info, HardDrive, Calendar, Star
} from 'lucide-react';
import PageHeader from '../components/PageHeader';
import Section from '../components/Section';
import Button from '../components/Button';
import {
  getStoreApps, getStoreApp, checkStoreUpdates
} from '../api/client';

// ── Constants ────────────────────────────────────────────────
const LS_KEY = 'hr_store_installed'; // { slug: version }

// ── Helpers ──────────────────────────────────────────────────
const formatSize = (bytes) =>
  bytes >= 1e6 ? (bytes / 1e6).toFixed(1) + ' MB' : (bytes / 1e3).toFixed(0) + ' KB';

const getInstalled = () => {
  try { return JSON.parse(localStorage.getItem(LS_KEY) || '{}'); }
  catch { return {}; }
};

const setInstalled = (slug, version) => {
  const data = getInstalled();
  data[slug] = version;
  localStorage.setItem(LS_KEY, JSON.stringify(data));
};

const getInstalledVersion = (slug) => getInstalled()[slug] || null;

const compareVersions = (a, b) => {
  const parse = (s) => s.split('.').map((x) => parseInt(x, 10) || 0);
  const va = parse(a); const vb = parse(b);
  const len = Math.max(va.length, vb.length);
  for (let i = 0; i < len; i++) {
    const diff = (va[i] || 0) - (vb[i] || 0);
    if (diff !== 0) return diff;
  }
  return 0;
};

const isNewer = (latest, installed) => compareVersions(latest, installed) > 0;

// Category colors
const CATEGORY_COLORS = {
  productivity: 'text-blue-400 bg-blue-400/10',
  media: 'text-purple-400 bg-purple-400/10',
  finance: 'text-green-400 bg-green-400/10',
  health: 'text-red-400 bg-red-400/10',
  tools: 'text-yellow-400 bg-yellow-400/10',
  other: 'text-gray-400 bg-gray-400/10',
};
const categoryStyle = (cat) => CATEGORY_COLORS[cat] || CATEGORY_COLORS.other;

// App icon placeholder (colored initials)
function AppIcon({ name, slug, size = 48 }) {
  const colors = ['from-blue-600 to-blue-800', 'from-purple-600 to-purple-800',
    'from-green-600 to-green-800', 'from-orange-600 to-orange-800',
    'from-red-600 to-red-800', 'from-teal-600 to-teal-800'];
  const idx = (slug || name || '').split('').reduce((a, c) => a + c.charCodeAt(0), 0) % colors.length;
  const initial = (name || '?')[0].toUpperCase();
  return (
    <div
      className={`flex items-center justify-center rounded-2xl bg-gradient-to-br ${colors[idx]} text-white font-bold flex-shrink-0`}
      style={{ width: size, height: size, fontSize: size * 0.4 }}
    >
      {initial}
    </div>
  );
}

// Install button with states
function InstallButton({ app, onInstalled }) {
  const [state, setState] = useState('idle'); // idle | downloading | done | installed
  const [progress, setProgress] = useState(0);
  const installedVer = getInstalledVersion(app.slug);
  const latestVer = app.latest_version;
  const hasUpdate = installedVer && latestVer && isNewer(latestVer, installedVer);
  const isInstalled = installedVer && !hasUpdate;

  const downloadAndInstall = async () => {
    if (state === 'downloading') return;
    setState('downloading');
    setProgress(0);

    const url = `/api/store/releases/${app.slug}/${latestVer}/download`;
    try {
      const response = await fetch(url);
      if (!response.ok) throw new Error('Download failed');

      const contentLength = response.headers.get('Content-Length');
      const total = contentLength ? parseInt(contentLength, 10) : 0;
      const reader = response.body.getReader();
      const chunks = [];
      let received = 0;

      while (true) {
        const { done, value } = await reader.read();
        if (done) break;
        chunks.push(value);
        received += value.length;
        if (total > 0) setProgress(Math.round((received / total) * 100));
      }

      // Assemble and trigger download
      const blob = new Blob(chunks, { type: 'application/vnd.android.package-archive' });
      const blobUrl = URL.createObjectURL(blob);
      const a = document.createElement('a');
      a.href = blobUrl;
      a.download = `${app.slug}-${latestVer}.apk`;
      document.body.appendChild(a);
      a.click();
      document.body.removeChild(a);
      setTimeout(() => URL.revokeObjectURL(blobUrl), 10000);

      setState('done');
    } catch (err) {
      console.error('Download error:', err);
      setState('idle');
    }
  };

  const markInstalled = () => {
    setInstalled(app.slug, latestVer);
    setState('installed');
    if (onInstalled) onInstalled(app.slug, latestVer);
  };

  if (isInstalled) {
    return (
      <button className="flex items-center gap-1.5 px-3 py-1.5 text-sm bg-gray-700 text-gray-400 rounded-lg cursor-default">
        <CheckCircle className="w-4 h-4 text-green-400" />
        Installé ✓
      </button>
    );
  }

  if (state === 'downloading') {
    return (
      <div className="flex flex-col items-center gap-1 min-w-[100px]">
        <div className="flex items-center gap-1.5 text-sm text-blue-400">
          <Loader2 className="w-4 h-4 animate-spin" />
          {progress > 0 ? `${progress}%` : 'Téléchargement...'}
        </div>
        {progress > 0 && (
          <div className="w-full bg-gray-700 rounded-full h-1.5">
            <div
              className="bg-blue-500 h-1.5 rounded-full transition-all duration-200"
              style={{ width: `${progress}%` }}
            />
          </div>
        )}
      </div>
    );
  }

  if (state === 'done') {
    return (
      <div className="flex flex-col items-center gap-1.5">
        <div className="flex items-center gap-1.5 text-sm text-green-400 font-medium">
          <Download className="w-4 h-4" />
          APK prêt — ouvrir le fichier
        </div>
        <button
          onClick={markInstalled}
          className="text-xs px-3 py-1 bg-green-700 hover:bg-green-600 text-white rounded transition-colors"
        >
          Marquer comme installé
        </button>
      </div>
    );
  }

  return (
    <button
      onClick={downloadAndInstall}
      className={`flex items-center gap-1.5 px-3 py-1.5 text-sm font-medium rounded-lg transition-colors ${
        hasUpdate
          ? 'bg-orange-600 hover:bg-orange-500 text-white'
          : 'bg-blue-600 hover:bg-blue-500 text-white'
      }`}
    >
      <Download className="w-4 h-4" />
      {hasUpdate ? `Mettre à jour (v${latestVer})` : 'Installer'}
    </button>
  );
}

// Install instructions modal
function InstallInstructions({ onClose }) {
  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/60 p-4" onClick={onClose}>
      <div className="bg-gray-800 rounded-xl border border-gray-600 p-6 max-w-md w-full shadow-2xl" onClick={(e) => e.stopPropagation()}>
        <div className="flex items-center justify-between mb-4">
          <h3 className="text-lg font-semibold text-white flex items-center gap-2">
            <Info className="w-5 h-5 text-blue-400" />
            Comment installer un APK
          </h3>
          <button onClick={onClose} className="text-gray-400 hover:text-white">
            <X className="w-5 h-5" />
          </button>
        </div>
        <ol className="space-y-3 text-sm text-gray-300">
          <li className="flex gap-3">
            <span className="flex-shrink-0 w-6 h-6 rounded-full bg-blue-600 text-white flex items-center justify-center text-xs font-bold">1</span>
            <span>Appuyez sur <strong className="text-white">Installer</strong> — le fichier APK sera téléchargé sur votre appareil.</span>
          </li>
          <li className="flex gap-3">
            <span className="flex-shrink-0 w-6 h-6 rounded-full bg-blue-600 text-white flex items-center justify-center text-xs font-bold">2</span>
            <span>Ouvrez le fichier depuis les <strong className="text-white">notifications</strong> ou le gestionnaire de fichiers.</span>
          </li>
          <li className="flex gap-3">
            <span className="flex-shrink-0 w-6 h-6 rounded-full bg-blue-600 text-white flex items-center justify-center text-xs font-bold">3</span>
            <span>Si Android bloque l'installation : <strong className="text-white">Paramètres → Sécurité → Sources inconnues</strong> → activer pour votre navigateur.</span>
          </li>
          <li className="flex gap-3">
            <span className="flex-shrink-0 w-6 h-6 rounded-full bg-blue-600 text-white flex items-center justify-center text-xs font-bold">4</span>
            <span>Confirmez l'installation, puis revenez ici pour marquer l'app comme installée.</span>
          </li>
        </ol>
        <button
          onClick={onClose}
          className="mt-5 w-full py-2 bg-blue-600 hover:bg-blue-500 text-white rounded-lg text-sm font-medium transition-colors"
        >
          Compris !
        </button>
      </div>
    </div>
  );
}

// App Card for the grid
function AppCard({ app, onClick, onInstalled }) {
  const installedVer = getInstalledVersion(app.slug);
  const hasUpdate = installedVer && app.latest_version && isNewer(app.latest_version, installedVer);
  const isInstalled = installedVer && !hasUpdate;

  return (
    <div
      className="bg-gray-800 hover:bg-gray-750 border border-gray-700 hover:border-gray-500 rounded-xl p-4 transition-all cursor-pointer group"
      onClick={onClick}
    >
      <div className="flex items-start gap-3 mb-3">
        <AppIcon name={app.name} slug={app.slug} size={48} />
        <div className="flex-1 min-w-0">
          <div className="flex items-center gap-2 flex-wrap">
            <h3 className="font-semibold text-white truncate">{app.name}</h3>
            {isInstalled && (
              <span className="flex items-center gap-0.5 px-1.5 py-0.5 text-xs bg-green-900/40 text-green-400 rounded-full border border-green-800/50">
                <CheckCircle className="w-3 h-3" /> installé
              </span>
            )}
            {hasUpdate && (
              <span className="px-1.5 py-0.5 text-xs bg-orange-900/40 text-orange-400 rounded-full border border-orange-800/50">
                mise à jour dispo
              </span>
            )}
          </div>
          <div className="flex items-center gap-2 mt-0.5 flex-wrap">
            <span className={`text-xs px-1.5 py-0.5 rounded-full font-medium ${categoryStyle(app.category)}`}>
              {app.category || 'other'}
            </span>
            {app.latest_version && (
              <span className="text-xs text-gray-500 font-mono">v{app.latest_version}</span>
            )}
          </div>
        </div>
      </div>

      {app.description && (
        <p className="text-sm text-gray-400 line-clamp-2 mb-3">{app.description}</p>
      )}

      <div className="flex items-center justify-between text-xs text-gray-500 mb-3">
        <div className="flex items-center gap-3">
          {app.latest_size_bytes && (
            <span className="flex items-center gap-1">
              <HardDrive className="w-3 h-3" />
              {formatSize(app.latest_size_bytes)}
            </span>
          )}
          <span className="flex items-center gap-1">
            <Tag className="w-3 h-3" />
            {app.release_count} release{app.release_count !== 1 ? 's' : ''}
          </span>
        </div>
        {installedVer && (
          <span className="text-gray-600 font-mono">installé: v{installedVer}</span>
        )}
      </div>

      {/* Install button — stop propagation so click doesn't open detail */}
      <div onClick={(e) => e.stopPropagation()}>
        <InstallButton app={app} onInstalled={onInstalled} />
      </div>
    </div>
  );
}

// Detail page
function AppDetail({ app, onBack, onInstalled }) {
  const [, forceUpdate] = useState(0);
  const releases = [...(app.releases || [])].reverse();
  const latestRelease = releases[0];
  const installedVer = getInstalledVersion(app.slug);
  const hasUpdate = installedVer && latestRelease && isNewer(latestRelease.version, installedVer);
  const isInstalled = installedVer && !hasUpdate;
  const [expandedRelease, setExpandedRelease] = useState(null);

  const handleInstalled = (slug, ver) => {
    forceUpdate(n => n + 1);
    if (onInstalled) onInstalled(slug, ver);
  };

  return (
    <div className="h-full flex flex-col overflow-y-auto">
      <PageHeader icon={StoreIcon} title="Store">
        <Button variant="secondary" onClick={onBack}>
          <ArrowLeft className="w-4 h-4" /> Retour
        </Button>
      </PageHeader>

      <div className="px-6 py-5 space-y-6">
        {/* App header */}
        <div className="flex items-start gap-5">
          <AppIcon name={app.name} slug={app.slug} size={72} />
          <div className="flex-1">
            <div className="flex items-center gap-3 flex-wrap">
              <h1 className="text-2xl font-bold text-white">{app.name}</h1>
              {isInstalled && (
                <span className="flex items-center gap-1 px-2 py-1 text-sm bg-green-900/40 text-green-400 rounded-full border border-green-800/50">
                  <CheckCircle className="w-4 h-4" /> Installé
                </span>
              )}
              {hasUpdate && (
                <span className="px-2 py-1 text-sm bg-orange-900/40 text-orange-400 rounded-full border border-orange-800/50">
                  🔄 Mise à jour disponible
                </span>
              )}
            </div>
            <div className="flex items-center gap-3 mt-1 flex-wrap">
              <span className={`text-sm px-2 py-0.5 rounded-full font-medium ${categoryStyle(app.category)}`}>
                {app.category || 'other'}
              </span>
              {latestRelease && (
                <span className="text-sm text-gray-400 font-mono">v{latestRelease.version}</span>
              )}
              {latestRelease?.size_bytes && (
                <span className="text-sm text-gray-500">{formatSize(latestRelease.size_bytes)}</span>
              )}
            </div>
            {installedVer && (
              <div className="mt-1 text-sm text-gray-500">
                Installé: <span className="font-mono">v{installedVer}</span>
                {hasUpdate && latestRelease && (
                  <span className="ml-2 text-orange-400">→ v{latestRelease.version} disponible</span>
                )}
              </div>
            )}
          </div>
        </div>

        {/* Description */}
        {app.description && (
          <div className="bg-gray-800 rounded-xl border border-gray-700 p-4">
            <h2 className="text-sm font-semibold text-gray-400 uppercase mb-2">Description</h2>
            <p className="text-gray-300 text-sm leading-relaxed">{app.description}</p>
          </div>
        )}

        {/* Install action */}
        {latestRelease && (
          <div className="bg-gray-800 rounded-xl border border-gray-700 p-4">
            <h2 className="text-sm font-semibold text-gray-400 uppercase mb-3">Installation</h2>
            <div className="flex items-center gap-4" onClick={(e) => e.stopPropagation()}>
              <InstallButton
                app={{ ...app, latest_version: latestRelease.version }}
                onInstalled={handleInstalled}
              />
              {!isInstalled && (
                <p className="text-xs text-gray-500">
                  Téléchargez le fichier APK et ouvrez-le depuis les notifications Android.
                </p>
              )}
            </div>
          </div>
        )}

        {/* Releases history */}
        <div className="bg-gray-800 rounded-xl border border-gray-700 p-4">
          <h2 className="text-sm font-semibold text-gray-400 uppercase mb-3">
            Historique des versions ({releases.length})
          </h2>
          <div className="space-y-2">
            {releases.map((rel, i) => (
              <div key={rel.version} className="border border-gray-700 rounded-lg overflow-hidden">
                <button
                  className="w-full flex items-center justify-between px-4 py-3 bg-gray-750 hover:bg-gray-700/50 transition-colors text-left"
                  onClick={() => setExpandedRelease(expandedRelease === rel.version ? null : rel.version)}
                >
                  <div className="flex items-center gap-3">
                    <span className={`font-mono font-semibold ${i === 0 ? 'text-blue-400' : 'text-gray-300'}`}>
                      v{rel.version}
                    </span>
                    {i === 0 && (
                      <span className="text-xs px-1.5 py-0.5 bg-blue-900/40 text-blue-400 rounded-full border border-blue-800/50">
                        Dernière
                      </span>
                    )}
                    {installedVer === rel.version && (
                      <span className="text-xs px-1.5 py-0.5 bg-green-900/40 text-green-400 rounded-full border border-green-800/50">
                        Installée
                      </span>
                    )}
                    <span className="text-xs text-gray-500">{formatSize(rel.size_bytes)}</span>
                  </div>
                  <div className="flex items-center gap-2 text-gray-500">
                    <span className="text-xs">
                      {new Date(rel.created_at).toLocaleDateString('fr-FR')}
                    </span>
                    {expandedRelease === rel.version ? <ChevronUp className="w-4 h-4" /> : <ChevronDown className="w-4 h-4" />}
                  </div>
                </button>
                {expandedRelease === rel.version && (
                  <div className="px-4 py-3 bg-gray-900/40 border-t border-gray-700 space-y-2">
                    {rel.changelog ? (
                      <p className="text-sm text-gray-300 whitespace-pre-line">{rel.changelog}</p>
                    ) : (
                      <p className="text-sm text-gray-600 italic">Pas de changelog.</p>
                    )}
                    <div className="text-xs text-gray-600 font-mono truncate">SHA-256: {rel.sha256}</div>
                  </div>
                )}
              </div>
            ))}
          </div>
        </div>
      </div>
    </div>
  );
}

// ── Main Store component ──────────────────────────────────────
function Store() {
  const [apps, setApps] = useState([]);
  const [loading, setLoading] = useState(true);
  const [message, setMessage] = useState(null);
  const [selectedApp, setSelectedApp] = useState(null);
  const [loadingDetail, setLoadingDetail] = useState(false);
  const [search, setSearch] = useState('');
  const [categoryFilter, setCategoryFilter] = useState('all');
  const [showInstructions, setShowInstructions] = useState(false);
  const [installedMap, setInstalledMap] = useState(getInstalled());

  const fetchApps = useCallback(async () => {
    try {
      const res = await getStoreApps();
      setApps(res.data?.apps || []);
    } catch {
      setMessage({ type: 'error', text: 'Erreur lors du chargement du store' });
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => { fetchApps(); }, [fetchApps]);

  const selectApp = async (slug) => {
    setLoadingDetail(true);
    try {
      const res = await getStoreApp(slug);
      setSelectedApp(res.data?.app || null);
    } catch {
      setMessage({ type: 'error', text: 'Erreur lors du chargement des détails' });
    } finally {
      setLoadingDetail(false);
    }
  };

  const handleInstalled = (slug, version) => {
    setInstalled(slug, version);
    setInstalledMap(getInstalled());
  };

  const handleRefresh = () => {
    setLoading(true);
    fetchApps();
  };

  // Enrich apps with installed info
  const enrichedApps = apps.map(app => ({
    ...app,
    _installedVer: installedMap[app.slug] || null,
    _hasUpdate: installedMap[app.slug] && app.latest_version &&
      isNewer(app.latest_version, installedMap[app.slug]),
  }));

  // Stats
  const totalInstalled = enrichedApps.filter(a => a._installedVer).length;
  const totalUpdates = enrichedApps.filter(a => a._hasUpdate).length;

  // Categories
  const categories = ['all', ...Array.from(new Set(apps.map(a => a.category || 'other').filter(Boolean)))];

  // Filtered apps
  const filteredApps = enrichedApps.filter(app => {
    const matchSearch = !search ||
      app.name.toLowerCase().includes(search.toLowerCase()) ||
      (app.description || '').toLowerCase().includes(search.toLowerCase());
    const matchCat = categoryFilter === 'all' || (app.category || 'other') === categoryFilter;
    return matchSearch && matchCat;
  });

  if (loading) {
    return (
      <div className="h-full flex flex-col">
        <PageHeader icon={StoreIcon} title="Store" />
        <div className="flex-1 flex items-center justify-center">
          <Loader2 className="w-8 h-8 text-blue-400 animate-spin" />
        </div>
      </div>
    );
  }

  if (selectedApp) {
    return (
      <AppDetail
        app={selectedApp}
        onBack={() => setSelectedApp(null)}
        onInstalled={handleInstalled}
      />
    );
  }

  return (
    <div className="h-full flex flex-col overflow-y-auto">
      <PageHeader icon={StoreIcon} title="Store">
        <button
          onClick={() => setShowInstructions(true)}
          className="flex items-center gap-1.5 px-3 py-1.5 text-sm text-gray-400 hover:text-white border border-gray-600 hover:border-gray-400 rounded transition-colors"
        >
          <Info className="w-4 h-4" />
          Guide
        </button>
        <a
          href="/api/store/client/apk"
          download
          className="flex items-center gap-1.5 px-3 py-1.5 text-sm font-medium text-white bg-blue-600 hover:bg-blue-500 rounded transition-colors"
        >
          <Smartphone className="w-4 h-4" />
          App Android
        </a>
        <Button variant="secondary" onClick={handleRefresh}>
          <RefreshCw className="w-4 h-4" />
        </Button>
      </PageHeader>

      {/* Stats bar */}
      <div className="px-6 py-3 flex items-center gap-6 text-sm border-b border-gray-800">
        <div className="flex items-center gap-2 text-gray-400">
          <Package className="w-4 h-4 text-blue-400" />
          <span>{apps.length} app{apps.length !== 1 ? 's' : ''}</span>
        </div>
        {totalInstalled > 0 && (
          <div className="flex items-center gap-2 text-gray-400">
            <CheckCircle className="w-4 h-4 text-green-400" />
            <span>{totalInstalled} installée{totalInstalled !== 1 ? 's' : ''}</span>
          </div>
        )}
        {totalUpdates > 0 && (
          <div className="flex items-center gap-2 text-orange-400 font-medium">
            <AlertCircle className="w-4 h-4" />
            <span>{totalUpdates} mise{totalUpdates !== 1 ? 's' : ''} à jour</span>
          </div>
        )}
      </div>

      {message && (
        <div className={`mx-6 mt-3 text-sm rounded px-3 py-2 ${
          message.type === 'error'
            ? 'text-red-400 bg-red-900/20 border border-red-800'
            : 'text-green-400 bg-green-900/20 border border-green-800'
        }`}>
          {message.text}
        </div>
      )}

      {/* Search + filter */}
      <div className="px-6 py-4 flex flex-col sm:flex-row gap-3">
        <div className="relative flex-1">
          <Search className="absolute left-3 top-1/2 -translate-y-1/2 w-4 h-4 text-gray-500" />
          <input
            type="text"
            placeholder="Rechercher une application..."
            value={search}
            onChange={(e) => setSearch(e.target.value)}
            className="w-full pl-9 pr-4 py-2 bg-gray-800 border border-gray-700 rounded-lg text-sm text-white placeholder-gray-500 focus:outline-none focus:border-blue-500 transition-colors"
          />
          {search && (
            <button
              onClick={() => setSearch('')}
              className="absolute right-3 top-1/2 -translate-y-1/2 text-gray-500 hover:text-white"
            >
              <X className="w-4 h-4" />
            </button>
          )}
        </div>
        <div className="flex gap-2 flex-wrap">
          {categories.map(cat => (
            <button
              key={cat}
              onClick={() => setCategoryFilter(cat)}
              className={`px-3 py-1.5 rounded-lg text-sm font-medium transition-colors ${
                categoryFilter === cat
                  ? 'bg-blue-600 text-white'
                  : 'bg-gray-800 text-gray-400 hover:text-white border border-gray-700 hover:border-gray-500'
              }`}
            >
              {cat === 'all' ? 'Tout' : cat}
            </button>
          ))}
        </div>
      </div>

      {/* App grid */}
      <div className="px-6 pb-6">
        {filteredApps.length === 0 ? (
          <div className="text-center py-12 text-gray-400 text-sm">
            {search || categoryFilter !== 'all'
              ? 'Aucune application ne correspond à votre recherche.'
              : 'Aucune application. Les publications sont gérées via MCP.'}
          </div>
        ) : (
          <div className="grid grid-cols-1 sm:grid-cols-2 lg:grid-cols-3 gap-4">
            {filteredApps.map((app) => (
              <AppCard
                key={app.slug}
                app={app}
                onClick={() => selectApp(app.slug)}
                onInstalled={handleInstalled}
              />
            ))}
          </div>
        )}
      </div>

      {loadingDetail && (
        <div className="fixed inset-0 z-40 flex items-center justify-center bg-black/40">
          <Loader2 className="w-8 h-8 text-blue-400 animate-spin" />
        </div>
      )}

      {showInstructions && <InstallInstructions onClose={() => setShowInstructions(false)} />}
    </div>
  );
}

export default Store;
