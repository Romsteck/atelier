import { useState, useRef, useCallback } from 'react';
import {
  ArrowLeft, ArrowRight, RotateCw, Home, ExternalLink, Loader2,
  Monitor, Tablet, Smartphone, Power, Play,
} from 'lucide-react';

// Mini-navigateur embarqué : iframe même-origine sur /apps/{slug}/ (le proxy path-routing Atelier).
// Même-origine ⇒ on peut lire contentWindow.location / écouter `load` ⇒ vraie barre d'adresse + historique.
// `src` reste STABLE (base) ; toute navigation est impérative (iframe.src / location.reload) — sinon l'iframe
// se rechargerait à chaque frappe dans la barre d'adresse.

const VIEWPORTS = {
  desktop: { cls: 'w-full', Icon: Monitor },
  tablet: { cls: 'w-[768px] max-w-full', Icon: Tablet },
  mobile: { cls: 'w-[375px] max-w-full', Icon: Smartphone },
};

export default function PreviewTab({ slug, status, onControl }) {
  const base = `/apps/${slug}/`;
  const iframeRef = useRef(null);
  // Pile d'historique maison (refs = source de vérité, pas de closure périmée).
  const historyRef = useRef([]);
  const idxRef = useRef(-1);
  // navLock : la prochaine `load` provient d'une nav programmée (back/forward) → ne pas la pousser sur la pile.
  const navLockRef = useRef(false);
  // Dernière URL même-origine connue (fallback du reload si la page est partie cross-origin).
  const lastUrlRef = useRef(base);

  const [address, setAddress] = useState('/');
  const [loading, setLoading] = useState(true);
  const [viewport, setViewport] = useState('desktop');
  const [external, setExternal] = useState(false);
  const [nav, setNav] = useState({ canBack: false, canForward: false });

  const updateNav = useCallback(() => {
    setNav({
      canBack: idxRef.current > 0,
      canForward: idxRef.current < historyRef.current.length - 1,
    });
  }, []);

  // href absolu → chemin relatif sous /apps/{slug} pour l'affichage de la barre d'adresse.
  const toRelative = useCallback((href) => {
    try {
      const u = new URL(href, window.location.origin);
      const full = u.pathname + u.search + u.hash;
      if (!u.pathname.startsWith(base)) return full; // l'app a quitté son basePath
      return '/' + u.pathname.slice(base.length) + u.search + u.hash;
    } catch {
      return '/';
    }
  }, [base]);

  const toAbsolute = useCallback((rel) => base + String(rel).replace(/^\/+/, ''), [base]);

  const navigateTo = useCallback((absUrl, { record = true } = {}) => {
    const f = iframeRef.current;
    if (!f) return;
    setLoading(true);
    navLockRef.current = !record;
    f.src = absUrl;
  }, []);

  const handleLoad = useCallback(() => {
    setLoading(false);
    const f = iframeRef.current;
    let href;
    try {
      href = f?.contentWindow?.location?.href ?? null;
    } catch {
      href = null; // page cross-origin (redirection auth) : reads interdits
    }
    if (href == null) {
      setExternal(true);
      navLockRef.current = false;
      return;
    }
    setExternal(false);
    lastUrlRef.current = href;
    setAddress(toRelative(href));
    if (!navLockRef.current) {
      // nouvelle navigation utilisateur : tronquer l'avant + empiler
      const h = historyRef.current.slice(0, idxRef.current + 1);
      h.push(href);
      historyRef.current = h;
      idxRef.current = h.length - 1;
    }
    navLockRef.current = false;
    updateNav();
  }, [toRelative, updateNav]);

  const goBack = useCallback(() => {
    if (idxRef.current <= 0) return;
    idxRef.current -= 1;
    navigateTo(historyRef.current[idxRef.current], { record: false });
    updateNav();
  }, [navigateTo, updateNav]);

  const goForward = useCallback(() => {
    if (idxRef.current >= historyRef.current.length - 1) return;
    idxRef.current += 1;
    navigateTo(historyRef.current[idxRef.current], { record: false });
    updateNav();
  }, [navigateTo, updateNav]);

  const goHome = useCallback(() => navigateTo(base), [navigateTo, base]);

  const submitAddress = useCallback((e) => {
    e.preventDefault();
    navigateTo(toAbsolute(address));
  }, [navigateTo, toAbsolute, address]);

  const reload = useCallback(() => {
    const f = iframeRef.current;
    if (!f) return;
    setLoading(true);
    try {
      f.contentWindow.location.reload(); // même-origine, URL propre, doc frais (SW network-first)
    } catch {
      f.src = lastUrlRef.current || base; // fallback cross-origin : nav forcée
    }
  }, [base]);

  const openNewTab = useCallback(
    () => window.open(lastUrlRef.current || base, '_blank', 'noopener'),
    [base],
  );

  // ── État app non démarrée ──
  const state = (status?.state || '').toLowerCase();
  if (state !== 'running') {
    const starting = state === 'starting';
    return (
      <div className="flex items-center justify-center h-full bg-gray-900">
        <div className="max-w-sm w-full mx-4 p-6 rounded-lg bg-gray-800 border border-gray-700 shadow-xl text-center">
          {starting ? (
            <>
              <Loader2 className="w-8 h-8 mx-auto mb-3 animate-spin text-blue-400" />
              <h3 className="text-base font-semibold text-white mb-1">Démarrage…</h3>
              <p className="text-sm text-gray-400">L'aperçu se chargera dès que l'app sera prête.</p>
            </>
          ) : (
            <>
              <div className="flex items-center justify-center w-12 h-12 mx-auto mb-3 rounded-full bg-yellow-500/15 text-yellow-400">
                <Power className="w-6 h-6" />
              </div>
              <h3 className="text-base font-semibold text-white mb-1">Application arrêtée</h3>
              <p className="text-sm text-gray-400 mb-4">
                Démarre l'application pour afficher l'aperçu de{' '}
                <span className="font-mono text-gray-300">/apps/{slug}/</span>.
              </p>
              <button
                onClick={() => onControl?.(slug, 'start')}
                className="w-full px-4 py-2 text-sm font-medium text-white bg-blue-500 hover:bg-blue-600 active:bg-blue-700 rounded-md flex items-center justify-center gap-2 transition-colors"
              >
                <Play className="w-4 h-4" /> Démarrer l'app
              </button>
            </>
          )}
        </div>
      </div>
    );
  }

  const vp = VIEWPORTS[viewport];

  return (
    <div className="flex flex-col h-full bg-gray-900">
      {/* Barre d'outils navigateur */}
      <div className="flex items-center gap-1.5 px-2 py-1.5 shrink-0 bg-gray-800 border-b border-gray-700">
        <button
          onClick={goBack}
          disabled={!nav.canBack}
          title="Précédent"
          className="p-1.5 rounded-sm text-gray-300 hover:bg-gray-700 disabled:opacity-30 disabled:hover:bg-transparent"
        >
          <ArrowLeft className="w-4 h-4" />
        </button>
        <button
          onClick={goForward}
          disabled={!nav.canForward}
          title="Suivant"
          className="p-1.5 rounded-sm text-gray-300 hover:bg-gray-700 disabled:opacity-30 disabled:hover:bg-transparent"
        >
          <ArrowRight className="w-4 h-4" />
        </button>
        <button onClick={reload} title="Recharger" className="p-1.5 rounded-sm text-gray-300 hover:bg-gray-700">
          <RotateCw className="w-4 h-4" />
        </button>
        <button onClick={goHome} title="Racine de l'app" className="p-1.5 rounded-sm text-gray-300 hover:bg-gray-700">
          <Home className="w-4 h-4" />
        </button>

        {/* Barre d'adresse */}
        <form onSubmit={submitAddress} className="flex-1 flex items-center min-w-0">
          <div className="flex items-center w-full bg-gray-900 border border-gray-700 rounded-sm px-2 focus-within:border-blue-500">
            <span className="text-xs text-gray-500 font-mono shrink-0 select-none">/apps/{slug}</span>
            <input
              value={address}
              onChange={(e) => setAddress(e.target.value)}
              disabled={external}
              spellCheck={false}
              placeholder="/"
              title={external ? 'Page externe — édition désactivée' : undefined}
              className="flex-1 min-w-0 bg-transparent px-1 py-1 text-xs font-mono text-white outline-hidden disabled:text-gray-500"
            />
            {loading && <Loader2 className="w-3.5 h-3.5 text-blue-400 animate-spin shrink-0" />}
          </div>
        </form>

        {/* Tailles d'écran */}
        <div className="flex items-center gap-0.5 ml-1 shrink-0">
          {Object.entries(VIEWPORTS).map(([id, { Icon }]) => (
            <button
              key={id}
              onClick={() => setViewport(id)}
              title={id}
              className={`p-1.5 rounded-sm ${
                viewport === id ? 'bg-gray-700 text-blue-400' : 'text-gray-400 hover:bg-gray-700'
              }`}
            >
              <Icon className="w-4 h-4" />
            </button>
          ))}
        </div>

        <button
          onClick={openNewTab}
          title="Ouvrir dans un onglet"
          className="p-1.5 rounded-sm text-gray-300 hover:bg-gray-700 shrink-0"
        >
          <ExternalLink className="w-4 h-4" />
        </button>
      </div>

      {/* Iframe contraint à la largeur du viewport choisi */}
      <div className="flex-1 overflow-auto bg-gray-900 flex justify-center">
        <div
          className={`h-full ${vp.cls} transition-[width] duration-200 ${
            viewport !== 'desktop' ? 'border-x border-gray-700' : ''
          }`}
        >
          <iframe
            ref={iframeRef}
            src={base}
            onLoad={handleLoad}
            title={`Preview - ${slug}`}
            className="w-full h-full border-0 bg-white"
            allow="clipboard-read; clipboard-write; fullscreen"
          />
        </div>
      </div>
    </div>
  );
}
