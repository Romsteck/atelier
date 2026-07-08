import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { useSearchParams } from 'react-router-dom';
import {
  Settings2, RefreshCw, Globe, Link2, Plug, CheckCircle2, XCircle,
  AlertTriangle, ExternalLink, Power, Trash2, RotateCw, Server, KeyRound, ShieldCheck,
} from 'lucide-react';
import PageHeader from '../components/PageHeader';
import Button from '../components/Button';
import useWebSocket from '../hooks/useWebSocket';
import {
  getHomerouteSettings, setHomerouteSettings, testHomeroute, registerHomeroute,
  getHomerouteAppRoutes, assignHomerouteRoute, removeHomerouteRoute, toggleHomerouteRoute,
  getAgentAuth, probeAgentAuth, setAgentAuth, clearAgentAuth, getSdkVersion, updateSdk,
  getAppsClaudeToken, probeAppsClaudeToken, setAppsClaudeToken, clearAppsClaudeToken,
} from '../api/client';
import { apiErr } from '../utils/apiErr';
import { useToast } from '../hooks/useToast';

const FIELD = 'w-full rounded-lg border border-gray-700 bg-gray-900/60 px-3 py-2 text-sm text-gray-100 focus:border-blue-500 focus:outline-none';
const LBL = 'mb-1 block text-xs font-medium text-gray-400';
const CARD = 'rounded-xl border border-gray-700/70 bg-gray-800/50 p-5';

// Onglets de la page. L'ordre = ordre d'affichage ; `id` = valeur de `?tab=`.
const TABS = [
  { id: 'auth', label: 'Authentification', icon: ShieldCheck },
  { id: 'homeroute', label: 'Liaison Homeroute', icon: Server },
  { id: 'hostnames', label: 'Hostnames', icon: Globe },
];

function fmtTime(iso) {
  if (!iso) return null;
  const d = new Date(iso);
  return Number.isFinite(d.getTime()) ? d.toLocaleString() : null;
}

// État d'auth SDK dérivé du statut masqué, partagé par le bandeau ET le badge
// d'onglet (une seule source de vérité). `error` = une erreur d'auth est plus
// récente que le dernier OK (ou aucun OK) → l'auth échoue activement, même sans
// token managé (le fallback .credentials.json est mort). `unconfigured` = pas de
// token managé et aucune erreur connue. Sinon `healthy`.
function authState(a) {
  if (!a) return 'unconfigured';
  const okAt = a.last_ok_at ? new Date(a.last_ok_at) : null;
  const errAt = a.last_error_at ? new Date(a.last_error_at) : null;
  if (errAt && (!okAt || errAt > okAt)) return 'error';
  if (!a.configured) return 'unconfigured';
  return 'healthy';
}

export default function Settings() {
  const [settings, setSettings] = useState(null);
  const [routes, setRoutes] = useState(null);
  const [form, setForm] = useState({
    base_url: '', environment_name: '', public_url: '', bearer_token: '',
  });
  const [loading, setLoading] = useState(true);
  const [saving, setSaving] = useState(false);
  const [testing, setTesting] = useState(false);
  const [registering, setRegistering] = useState(false);
  const [testResult, setTestResult] = useState(null);
  // Banner inline (rendu custom sous le header) piloté par le hook partagé.
  const { toast, showToast, dismiss } = useToast();
  const flash = (type, text) => showToast(text, type);
  const [busy, setBusy] = useState({});            // per-slug action in flight
  const [subdomains, setSubdomains] = useState({}); // per-slug editable subdomain
  const [requireAuth, setRequireAuth] = useState({}); // per-slug edge forward-auth
  const dirty = useRef(false);                       // user touched the per-row inputs

  // ── Authentification du Claude Agent SDK ─────────────────────────────
  const [sdkAuth, setSdkAuth] = useState(null);   // statut masqué { configured, last_ok_at, ... }
  const [sdk, setSdk] = useState(null);           // version SDK { installed, latest, update_available }
  const [authToken, setAuthToken] = useState(''); // saisie (write-only, jamais re-seedée)
  const [revealAuth, setRevealAuth] = useState(false);
  const [savingAuth, setSavingAuth] = useState(false);
  const [probingAuth, setProbingAuth] = useState(false);
  const [authProbe, setAuthProbe] = useState(null); // { ok, error } du test « Vérifier »
  const [updatingSdk, setUpdatingSdk] = useState(false);

  // ── Token Claude pour les APPS (séparé du token runner/scan ci-dessus) ──
  const [appsTok, setAppsTok] = useState(null);      // statut masqué
  const [appsTokInput, setAppsTokInput] = useState('');
  const [revealAppsTok, setRevealAppsTok] = useState(false);
  const [savingAppsTok, setSavingAppsTok] = useState(false);
  const [probingAppsTok, setProbingAppsTok] = useState(false);
  const [appsTokProbe, setAppsTokProbe] = useState(null);

  const loadAuth = useCallback(async () => {
    const [a, v, t] = await Promise.all([
      getAgentAuth().catch(() => null),
      getSdkVersion().catch(() => null),
      getAppsClaudeToken().catch(() => null),
    ]);
    if (a?.data) setSdkAuth(a.data);
    if (v?.data) setSdk(v.data);
    if (t?.data) setAppsTok(t.data);
  }, []);
  useEffect(() => { loadAuth(); }, [loadAuth]);

  const onSaveAppsTok = async () => {
    const tok = appsTokInput.trim();
    if (!tok) return;
    setSavingAppsTok(true); setAppsTokProbe(null);
    try {
      const r = await setAppsClaudeToken(tok);
      setAppsTok(r.data);
      setAppsTokInput('');
      flash('ok', 'Token apps validé et enregistré (injecté aux apps opt-in au prochain reconcile).');
    } catch (e) {
      flash('error', apiErr(e, 'échec de validation du token apps'));
    } finally { setSavingAppsTok(false); }
  };
  const onVerifyAppsTok = async () => {
    setProbingAppsTok(true); setAppsTokProbe(null);
    try {
      const r = await probeAppsClaudeToken();
      setAppsTok(r.data);
      setAppsTokProbe(r.data?.probe || null);
    } catch (e) {
      setAppsTokProbe({ ok: false, error: apiErr(e, 'échec') });
    } finally { setProbingAppsTok(false); }
  };
  const onClearAppsTok = async () => {
    try {
      const r = await clearAppsClaudeToken();
      setAppsTok(r.data);
      flash('ok', 'Token apps retiré (plus de CLAUDE_CODE_OAUTH_TOKEN injecté).');
    } catch (e) {
      flash('error', apiErr(e, 'échec du retrait'));
    }
  };

  const onSaveAuth = async () => {
    const tok = authToken.trim();
    if (!tok) return;
    setSavingAuth(true); setAuthProbe(null);
    try {
      const r = await setAgentAuth(tok);
      setSdkAuth(r.data);
      setAuthToken('');
      flash('ok', "Token validé et enregistré — l'agent et les scans repartent.");
    } catch (e) {
      flash('error', apiErr(e, 'échec de validation du token'));
    } finally { setSavingAuth(false); }
  };

  // « Vérifier » teste le token DÉJÀ configuré (vrai tour d'inférence) — utile pour
  // confirmer que l'auth live fonctionne encore. Pour un NOUVEAU token, « Enregistrer »
  // le valide avant de le persister.
  const onVerifyAuth = async () => {
    setProbingAuth(true); setAuthProbe(null);
    try {
      const r = await probeAgentAuth();
      setSdkAuth(r.data);
      setAuthProbe(r.data?.probe || null);
    } catch (e) {
      setAuthProbe({ ok: false, error: apiErr(e, 'échec') });
    } finally { setProbingAuth(false); }
  };

  const onClearAuth = async () => {
    try {
      const r = await clearAgentAuth();
      setSdkAuth(r.data);
      flash('ok', 'Token retiré (retour au fallback .credentials.json).');
    } catch (e) {
      flash('error', apiErr(e, 'échec du retrait'));
    }
  };

  const onUpdateSdk = async () => {
    setUpdatingSdk(true);
    try {
      const r = await updateSdk();
      setSdk((s) => ({ ...s, installed: r.data.installed, update_available: false }));
      flash('ok', `Agent SDK mis à jour (${r.data.installed}).`);
    } catch (e) {
      flash('error', apiErr(e, 'MAJ SDK échouée'));
    } finally { setUpdatingSdk(false); }
  };

  const reload = useCallback(async () => {
    try {
      const [s, r] = await Promise.all([
        getHomerouteSettings().catch(() => null),
        getHomerouteAppRoutes().catch(() => null),
      ]);
      const sd = s?.data?.settings || null;
      if (sd) {
        setSettings(sd);
        // Sync the form from the server unless the user has unsaved edits.
        // bearer_token is write-only → never seeded back (stays blank).
        setForm((f) => (f._touched ? f : {
          base_url: sd.base_url || '',
          environment_name: sd.environment_name || '',
          public_url: sd.public_url || '',
          bearer_token: '',
        }));
      }
      if (r?.data) setRoutes(r.data);
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => { reload(); }, [reload]);

  // Live: any route/settings/registration change rebroadcasts → reload.
  useWebSocket({ 'homeroute:routes': () => reload() });

  // Seed per-row inputs (subdomain + auth edge) from the server once (don't clobber edits).
  useEffect(() => {
    if (!routes?.apps || dirty.current) return;
    const seed = {};
    const auth = {};
    for (const a of routes.apps) {
      seed[a.slug] = a.subdomain || a.slug;
      auth[a.slug] = !!a.require_auth;
    }
    setSubdomains(seed);
    setRequireAuth(auth);
  }, [routes]);

  const setField = (k) => (e) => {
    const v = e.target.type === 'checkbox' ? e.target.checked : e.target.value;
    setForm((f) => ({ ...f, [k]: v, _touched: true }));
  };

  async function saveSettings() {
    setSaving(true);
    try {
      await setHomerouteSettings({
        base_url: form.base_url,
        environment_name: form.environment_name,
        public_url: form.public_url,
        bearer_token: form.bearer_token, // empty ⇒ serveur conserve l'existant
      });
      setForm((f) => ({ ...f, _touched: false, bearer_token: '' }));
      flash('ok', 'Liaison enregistrée');
      await reload();
    } catch (e) {
      flash('error', apiErr(e));
    } finally {
      setSaving(false);
    }
  }

  async function doTest() {
    setTesting(true);
    setTestResult(null);
    try {
      const res = await testHomeroute();
      setTestResult(res.data);
    } catch (e) {
      setTestResult({ reachable: false, error: apiErr(e) });
    } finally {
      setTesting(false);
    }
  }

  async function doRegister() {
    setRegistering(true);
    try {
      const res = await registerHomeroute();
      const name = res.data?.status?.environment_name || form.environment_name;
      flash('ok', `Rattaché à Homeroute en tant que « ${name} »`);
      await reload();
    } catch (e) {
      flash('error', apiErr(e));
    } finally {
      setRegistering(false);
    }
  }

  async function withBusy(slug, fn, okMsg) {
    setBusy((b) => ({ ...b, [slug]: true }));
    try {
      await fn();
      if (okMsg) flash('ok', okMsg);
      await reload();
    } catch (e) {
      flash('error', apiErr(e));
    } finally {
      setBusy((b) => ({ ...b, [slug]: false }));
    }
  }

  const assign = (a) =>
    withBusy(a.slug, () => assignHomerouteRoute(a.slug, {
      subdomain: subdomains[a.slug] || a.slug,
      require_auth: requireAuth[a.slug] ?? a.require_auth ?? false,
    }), `Hostname attribué à ${a.name}`);
  const remove = (a) => withBusy(a.slug, () => removeHomerouteRoute(a.slug), `Hostname retiré de ${a.name}`);
  const toggle = (a) => withBusy(a.slug, () => toggleHomerouteRoute(a.slug));

  const baseDomain = routes?.base_domain || '';
  const reachable = !!routes?.reachable;
  const hasToken = !!settings?.has_bearer_token;
  const configured = hasToken;            // configuré ⇒ actif (pas de toggle séparé)
  const registeredAt = fmtTime(settings?.registered_at);

  const sortedApps = useMemo(() => {
    if (!routes?.apps) return [];
    return [...routes.apps].sort((x, y) => x.name.localeCompare(y.name));
  }, [routes]);

  // Onglet actif porté par l'URL (?tab=) → deep-linkable + rechargeable. Défaut 'auth'.
  const [searchParams, setSearchParams] = useSearchParams();
  const tabParam = searchParams.get('tab');
  const activeTab = TABS.some((t) => t.id === tabParam) ? tabParam : 'auth';
  const setActiveTab = (id) => setSearchParams({ tab: id }, { replace: true });

  // Santé par onglet ('ok' | 'warn' | 'error') dérivée de l'état déjà calculé —
  // pilote le badge d'alerte sur chaque onglet.
  const authSt = authState(sdkAuth);
  const appsTokSt = authState(appsTok);
  const copyCmd = (cmd) => {
    navigator.clipboard?.writeText(cmd).then(
      () => flash('ok', 'Commande copiée.'),
      () => flash('error', 'Copie impossible.'),
    );
  };
  const anyHostIssue = sortedApps.some((a) => a.assigned && (a.drift || a.require_auth === false));
  const tabHealth = {
    auth: authSt === 'error' ? 'error' : authSt === 'unconfigured' ? 'warn' : 'ok',
    homeroute:
      (configured && !reachable) || (testResult && !testResult.reachable)
        ? 'error'
        : !configured
          ? 'warn'
          : 'ok',
    hostnames: configured && !reachable ? 'error' : !configured || anyHostIssue ? 'warn' : 'ok',
  };

  if (loading) {
    return (
      <div className="p-6 text-sm text-gray-400">
        <PageHeader title="Paramètres" icon={Settings2} />
        Chargement…
      </div>
    );
  }

  return (
    <div className="p-4 sm:p-6 space-y-6 max-w-5xl">
      <PageHeader title="Paramètres" icon={Settings2}>
        <Button onClick={reload} variant="secondary"><RefreshCw className="h-4 w-4" /> Rafraîchir</Button>
      </PageHeader>

      {toast && (
        <div className={`rounded-xl border px-4 py-3 text-sm ${
          toast.type === 'error'
            ? 'border-red-500/30 bg-red-500/10 text-red-700 dark:text-red-200'
            : 'border-emerald-500/30 bg-emerald-500/10 text-emerald-700 dark:text-emerald-200'}`}>
          {toast.msg}
          <button onClick={dismiss} className="ml-3 text-xs text-gray-400 hover:text-gray-200">fermer</button>
        </div>
      )}

      {/* ── Barre d'onglets (badge d'alerte par onglet en état dégradé) ─── */}
      <div className="flex items-center gap-1 overflow-x-auto border-b border-gray-700">
        {TABS.map((t) => {
          const active = t.id === activeTab;
          const h = tabHealth[t.id];
          const Icon = t.icon;
          return (
            <button
              key={t.id}
              onClick={() => setActiveTab(t.id)}
              className={`relative flex h-11 shrink-0 items-center gap-2 px-4 text-sm transition-colors ${
                active ? 'font-medium text-gray-50' : 'text-gray-400 hover:text-gray-200'}`}
              title={h === 'error' ? 'Erreur dans cet onglet' : h === 'warn' ? 'Avertissement dans cet onglet' : undefined}
            >
              <Icon className="h-4 w-4" />
              {t.label}
              {h !== 'ok' && (
                <AlertTriangle className={`h-3.5 w-3.5 ${h === 'error' ? 'text-red-500' : 'text-amber-500'}`} />
              )}
              {active && <span className="absolute inset-x-3 bottom-0 h-0.5 rounded-full bg-blue-400" />}
            </button>
          );
        })}
      </div>

      {/* ── Onglet Authentification Claude Agent SDK ────────────────────── */}
      {activeTab === 'auth' && (
      <section className={CARD}>
        <h2 className="mb-1 flex items-center gap-2 text-sm font-semibold text-gray-100">
          <ShieldCheck className="h-4 w-4 text-blue-400" /> Authentification Claude Agent SDK
        </h2>
        <p className="mb-4 text-xs text-gray-500">
          L&apos;agent et les scans de surveillance tournent en <code>hr-studio</code> avec l&apos;OAuth
          abonnement. Quand le token expire/est révoqué (<code>authentication_failed</code>), génère un
          token longue durée sur ton poste — <code>claude setup-token</code> (navigateur → token OAuth
          ~1&nbsp;an) — puis colle-le ci-dessous. Il est validé par un vrai tour d&apos;inférence puis
          injecté au runner sans redémarrage.
        </p>

        {/* Bandeau de statut (même source que le badge d'onglet : authState) */}
        <div className="mb-4 text-sm">
          {authSt === 'error' ? (
            <span className="inline-flex flex-wrap items-center gap-1.5 text-red-700 dark:text-red-300">
              <XCircle className="h-4 w-4" /> Expiré — ré-authentification requise
              {sdkAuth?.last_error_msg && <span className="text-gray-500">· {sdkAuth.last_error_msg}</span>}
            </span>
          ) : authSt === 'unconfigured' ? (
            <span className="inline-flex items-center gap-1.5 text-amber-700 dark:text-amber-300">
              <AlertTriangle className="h-4 w-4" /> Non configuré — le runner utilise le
              <code className="mx-1">.credentials.json</code> local (fallback). Colle un token ci-dessous.
            </span>
          ) : (
            <span className="inline-flex flex-wrap items-center gap-1.5 text-emerald-700 dark:text-emerald-300">
              <CheckCircle2 className="h-4 w-4" /> Authentifié (token longue durée)
              {sdkAuth?.last_ok_at && <span className="text-gray-500">· vérifié {fmtTime(sdkAuth.last_ok_at)}</span>}
            </span>
          )}
        </div>

        <div>
          <label className={LBL}>
            Token <code>claude setup-token</code>
            <button
              type="button"
              onClick={() => setRevealAuth((v) => !v)}
              className="ml-2 text-[10px] text-gray-400 hover:text-gray-200"
            >
              {revealAuth ? 'masquer' : 'afficher'}
            </button>
          </label>
          <textarea
            className={`${FIELD} h-20 font-mono ${revealAuth ? '' : '[-webkit-text-security:disc]'}`}
            value={authToken}
            onChange={(e) => setAuthToken(e.target.value)}
            autoComplete="off"
            spellCheck={false}
            placeholder="sk-ant-oat01-…"
          />
        </div>

        <div className="mt-4 flex flex-wrap items-center gap-3">
          <Button onClick={onSaveAuth} loading={savingAuth} disabled={!authToken.trim()}>
            <KeyRound className="h-4 w-4" /> Enregistrer
          </Button>
          <Button onClick={onVerifyAuth} variant="secondary" loading={probingAuth} disabled={!sdkAuth?.configured}>
            <RotateCw className="h-4 w-4" /> Vérifier l&apos;auth actuelle
          </Button>
          {sdkAuth?.configured && (
            <Button onClick={onClearAuth} variant="secondary">
              <Trash2 className="h-4 w-4" /> Retirer
            </Button>
          )}
          {authProbe && (
            authProbe.ok ? (
              <span className="inline-flex items-center gap-1.5 text-sm text-emerald-700 dark:text-emerald-300">
                <CheckCircle2 className="h-4 w-4" /> Auth OK (inférence réussie)
              </span>
            ) : (
              <span className="inline-flex items-center gap-1.5 text-sm text-red-700 dark:text-red-300">
                <XCircle className="h-4 w-4" /> {authProbe.error || 'authentification échouée'}
              </span>
            )
          )}
        </div>

        {sdk && (
          <div className="mt-3 text-xs text-gray-500">
            Agent SDK <code>{sdk.installed || '?'}</code>
            {sdk.update_available ? (
              <>
                {' → '}<code>{sdk.latest}</code> disponible.
                <button
                  onClick={onUpdateSdk}
                  disabled={updatingSdk}
                  className="ml-2 text-amber-500 hover:text-amber-400 disabled:opacity-50"
                >
                  {updatingSdk ? 'MAJ…' : 'Mettre à jour'}
                </button>
              </>
            ) : (
              <span> · à jour</span>
            )}
          </div>
        )}
      </section>
      )}

      {/* ── Token Claude pour les APPS (séparé du token runner/scan) ─────── */}
      {activeTab === 'auth' && (
      <section className={`${CARD} mt-5`}>
        <h2 className="mb-1 flex items-center gap-2 text-sm font-semibold text-gray-100">
          <KeyRound className="h-4 w-4 text-blue-400" /> Token Claude pour les apps
        </h2>
        <p className="mb-3 text-xs text-gray-500">
          Token <strong>séparé</strong> du token du runner/scan ci-dessus : il est injecté aux apps
          <em> opt-in</em> (case « Accès Claude » de leurs Paramètres) comme variable plateforme
          <code className="mx-1">CLAUDE_CODE_OAUTH_TOKEN</code>. Une app n&apos;a alors besoin d&apos;aucun
          <code className="mx-1">CLAUDE_CONFIG_DIR</code> ni fichier partagé. Génère-le sur ton poste puis
          colle-le ci-dessous (validé par un vrai tour d&apos;inférence avant enregistrement) :
        </p>

        {/* Commande copiable */}
        <div className="mb-4 flex items-center gap-2">
          <code className="flex-1 rounded-lg border border-gray-700 bg-gray-900/60 px-3 py-2 font-mono text-sm text-gray-200">
            claude setup-token
          </code>
          <Button variant="secondary" onClick={() => copyCmd('claude setup-token')}>
            Copier
          </Button>
        </div>

        {/* Bandeau de statut (unconfigured = normal : l'accès Claude des apps est optionnel) */}
        <div className="mb-4 text-sm">
          {appsTokSt === 'error' ? (
            <span className="inline-flex flex-wrap items-center gap-1.5 text-red-700 dark:text-red-300">
              <XCircle className="h-4 w-4" /> Token présent mais en échec — les apps opt-in échouent l&apos;auth
              {appsTok?.last_error_msg && <span className="text-gray-500">· {appsTok.last_error_msg}</span>}
            </span>
          ) : appsTokSt === 'unconfigured' ? (
            <span className="inline-flex items-center gap-1.5 text-gray-400">
              <AlertTriangle className="h-4 w-4" /> Aucun token — les apps opt-in n&apos;ont pas d&apos;accès Claude.
            </span>
          ) : (
            <span className="inline-flex flex-wrap items-center gap-1.5 text-emerald-700 dark:text-emerald-300">
              <CheckCircle2 className="h-4 w-4" /> Configuré
              {appsTok?.last_ok_at && <span className="text-gray-500">· vérifié {fmtTime(appsTok.last_ok_at)}</span>}
            </span>
          )}
        </div>

        <div>
          <label className={LBL}>
            Token <code>claude setup-token</code>
            <button
              type="button"
              onClick={() => setRevealAppsTok((v) => !v)}
              className="ml-2 text-[10px] text-gray-400 hover:text-gray-200"
            >
              {revealAppsTok ? 'masquer' : 'afficher'}
            </button>
          </label>
          <textarea
            className={`${FIELD} h-20 font-mono ${revealAppsTok ? '' : '[-webkit-text-security:disc]'}`}
            value={appsTokInput}
            onChange={(e) => setAppsTokInput(e.target.value)}
            autoComplete="off"
            spellCheck={false}
            placeholder="sk-ant-oat01-…"
          />
        </div>

        <div className="mt-4 flex flex-wrap items-center gap-3">
          <Button onClick={onSaveAppsTok} loading={savingAppsTok} disabled={!appsTokInput.trim()}>
            <KeyRound className="h-4 w-4" /> Enregistrer
          </Button>
          <Button onClick={onVerifyAppsTok} variant="secondary" loading={probingAppsTok} disabled={!appsTok?.configured}>
            <RotateCw className="h-4 w-4" /> Vérifier
          </Button>
          {appsTok?.configured && (
            <Button onClick={onClearAppsTok} variant="secondary">
              <Trash2 className="h-4 w-4" /> Retirer
            </Button>
          )}
          {appsTokProbe && (
            appsTokProbe.ok ? (
              <span className="inline-flex items-center gap-1.5 text-sm text-emerald-700 dark:text-emerald-300">
                <CheckCircle2 className="h-4 w-4" /> Auth OK (inférence réussie)
              </span>
            ) : (
              <span className="inline-flex items-center gap-1.5 text-sm text-red-700 dark:text-red-300">
                <XCircle className="h-4 w-4" /> {appsTokProbe.error || 'authentification échouée'}
              </span>
            )
          )}
        </div>
      </section>
      )}

      {/* ── Onglet Liaison Homeroute (identité + connexion) ─────────────── */}
      {activeTab === 'homeroute' && (
      <>
      {/* ── Identité & liaison ─────────────────────────────────────────── */}
      <section className={CARD}>
        <h2 className="mb-1 flex items-center gap-2 text-sm font-semibold text-gray-100">
          <Server className="h-4 w-4 text-blue-400" /> Identité & liaison Homeroute
        </h2>
        <p className="mb-4 text-xs text-gray-500">
          Cet environnement Atelier se déclare auprès du reverse proxy Homeroute et publie
          les apps en sous-domaine (DNS + TLS <code>*.{baseDomain || 'mynetwk.biz'}</code> auto).
          Le <strong>token</strong> s&apos;obtient dans Homeroute → <em>Environnements</em>.
        </p>

        <div className="space-y-4">
          <div className="grid gap-4 sm:grid-cols-2">
            <div>
              <label className={LBL}>Nom de cet environnement</label>
              <input className={FIELD} value={form.environment_name} onChange={setField('environment_name')}
                placeholder="medion" spellCheck={false} />
            </div>
            <div>
              <label className={LBL}>URL publique (lien retour)</label>
              <input className={FIELD} value={form.public_url} onChange={setField('public_url')}
                placeholder="https://atelier.mynetwk.biz" spellCheck={false} />
            </div>
            <div>
              <label className={LBL}>URL de hr-api</label>
              <input className={FIELD} value={form.base_url} onChange={setField('base_url')}
                placeholder="http://127.0.0.1:4000" spellCheck={false} />
            </div>
            <div>
              <label className={LBL}>
                Token de liaison
                {hasToken && (
                  <span className="ml-2 inline-flex items-center gap-1 rounded bg-emerald-500/15 px-1.5 py-0.5 text-[10px] text-emerald-700 dark:text-emerald-300">
                    <KeyRound className="h-3 w-3" /> configuré
                  </span>
                )}
              </label>
              <input className={FIELD} type="password" autoComplete="off" value={form.bearer_token}
                onChange={setField('bearer_token')}
                placeholder={hasToken ? '•••••••• (laisser vide pour conserver)' : 'collez le token de Homeroute'}
                spellCheck={false} />
            </div>
          </div>
        </div>

        <div className="mt-4 flex items-center gap-3">
          <Button onClick={saveSettings} loading={saving}>Enregistrer</Button>
          <span className="text-xs text-gray-500">
            La liaison est active dès qu&apos;un token est renseigné.
          </span>
        </div>
      </section>

      {/* ── Connexion / rattachement ───────────────────────────────────── */}
      <section className={CARD}>
        <h2 className="mb-1 flex items-center gap-2 text-sm font-semibold text-gray-100">
          <Plug className="h-4 w-4 text-blue-400" /> Connexion
        </h2>
        <p className="mb-4 text-xs text-gray-500">
          S&apos;enregistrer rattache cet environnement à Homeroute (visible dans sa page
          <em> Environnements</em>). Un heartbeat le maintient « en ligne ».
        </p>

        <div className="flex flex-wrap items-center gap-2">
          <Button onClick={doRegister} loading={registering} disabled={!configured}>
            <Plug className="h-4 w-4" /> Connecter / S&apos;enregistrer
          </Button>
          <Button onClick={doTest} variant="secondary" loading={testing}>
            <RotateCw className="h-4 w-4" /> Tester la connexion
          </Button>
          {testResult && (
            testResult.reachable ? (
              <span className="inline-flex items-center gap-1.5 text-sm text-emerald-700 dark:text-emerald-300">
                <CheckCircle2 className="h-4 w-4" />
                Joignable — domaine <code>{testResult.base_domain}</code>, {testResult.host_count} route(s)
              </span>
            ) : (
              <span className="inline-flex items-center gap-1.5 text-sm text-red-700 dark:text-red-300">
                <XCircle className="h-4 w-4" /> Injoignable — {testResult.error}
              </span>
            )
          )}
        </div>

        {/* Bandeau de statut de rattachement */}
        <div className="mt-4 text-sm">
          {!configured ? (
            <span className="inline-flex items-center gap-1.5 text-amber-700 dark:text-amber-300">
              <AlertTriangle className="h-4 w-4" /> Non configuré — générez le token dans Homeroute → Environnements et collez-le ci-dessus.
            </span>
          ) : registeredAt ? (
            <span className="inline-flex flex-wrap items-center gap-1.5 text-emerald-700 dark:text-emerald-300">
              <CheckCircle2 className="h-4 w-4" />
              Rattaché en tant que <code>{settings?.environment_name}</code>
              {baseDomain && <> · domaine <code>{baseDomain}</code></>}
              <span className="text-gray-500">· enregistré {registeredAt}</span>
            </span>
          ) : (
            <span className="inline-flex items-center gap-1.5 text-gray-400">
              <XCircle className="h-4 w-4" /> Non rattaché — cliquez sur « Connecter ».
            </span>
          )}
        </div>
      </section>

      </>
      )}

      {/* ── Onglet Hostnames des applications ───────────────────────────── */}
      {activeTab === 'hostnames' && (
      <section className={CARD}>
        <h2 className="mb-1 flex items-center gap-2 text-sm font-semibold text-gray-100">
          <Globe className="h-4 w-4 text-blue-400" /> Hostnames des applications
        </h2>
        <p className="mb-4 text-xs text-gray-500">
          Publie chaque app sur <code>{'{sous-domaine}'}.{baseDomain || 'mynetwk.biz'}</code> via
          Homeroute. Le hostname pointe vers Atelier, qui sert l&apos;app sous son chemin de build
          <code> /apps/{'{slug}'}/</code> (la racine y redirige automatiquement) — le reste
          d&apos;Atelier n&apos;est pas exposé sur ce hostname.
        </p>

        {!configured && (
          <div className="mb-4 flex items-center gap-2 rounded-lg border border-amber-500/30 bg-amber-500/10 px-3 py-2 text-sm text-amber-700 dark:text-amber-200">
            <AlertTriangle className="h-4 w-4 shrink-0" /> Aucun token configuré — renseignez-le ci-dessus pour gérer les hostnames.
          </div>
        )}
        {configured && !reachable && (
          <div className="mb-4 flex items-center gap-2 rounded-lg border border-red-500/30 bg-red-500/10 px-3 py-2 text-sm text-red-700 dark:text-red-200">
            <XCircle className="h-4 w-4 shrink-0" /> Homeroute injoignable{routes?.error ? ` — ${routes.error}` : ''}. Vérifiez l&apos;URL et que hr-api tourne.
          </div>
        )}

        <div className="overflow-x-auto">
          <table className="w-full text-sm">
            <thead>
              <tr className="border-b border-gray-700 text-left text-xs uppercase tracking-wider text-gray-500">
                <th className="py-2 pr-3 font-medium">Application</th>
                <th className="py-2 pr-3 font-medium">Sous-domaine</th>
                <th className="py-2 pr-3 font-medium">État</th>
                <th className="py-2 pl-3 text-right font-medium">Actions</th>
              </tr>
            </thead>
            <tbody className="divide-y divide-gray-800">
              {sortedApps.map((a) => {
                const rowBusy = !!busy[a.slug];
                const canAct = configured && reachable;
                return (
                  <tr key={a.slug}>
                    <td className="py-3 pr-3 align-top">
                      <div className="flex items-center gap-2">
                        <span className="font-medium text-gray-100">{a.name}</span>
                        <span className="rounded bg-gray-700/60 px-1.5 py-0.5 text-[11px] text-gray-400">:{a.port}</span>
                      </div>
                      <div className="mt-0.5 text-xs text-gray-500">
                        <code>{a.slug}</code> · {a.visibility}
                      </div>
                    </td>

                    <td className="py-3 pr-3 align-top">
                      <div className="flex items-center gap-1">
                        <input
                          className={`${FIELD} w-32 px-2 py-1`}
                          value={subdomains[a.slug] ?? a.slug}
                          onChange={(e) => { dirty.current = true; setSubdomains((s) => ({ ...s, [a.slug]: e.target.value })); }}
                          disabled={!canAct || rowBusy}
                          spellCheck={false}
                        />
                        <span className="text-xs text-gray-500">.{baseDomain || 'mynetwk.biz'}</span>
                      </div>
                      <label className="mt-1.5 flex w-fit cursor-pointer items-center gap-1.5 text-xs text-gray-400">
                        <input
                          type="checkbox"
                          className="h-3.5 w-3.5 accent-blue-500"
                          checked={requireAuth[a.slug] ?? !!a.require_auth}
                          onChange={(e) => { dirty.current = true; setRequireAuth((s) => ({ ...s, [a.slug]: e.target.checked })); }}
                          disabled={!canAct || rowBusy}
                        />
                        Auth edge (SSO Homeroute)
                      </label>
                      {a.assigned && a.hostname && (
                        <a
                          href={`https://${a.hostname}`}
                          target="_blank"
                          rel="noopener noreferrer"
                          className="mt-1 inline-flex items-center gap-1 text-xs text-blue-600 hover:text-blue-700 dark:text-blue-400 dark:hover:text-blue-300"
                        >
                          <Link2 className="h-3 w-3" /> {a.hostname} <ExternalLink className="h-3 w-3" />
                        </a>
                      )}
                    </td>

                    <td className="py-3 pr-3 align-top">
                      {a.assigned ? (
                        <div className="flex flex-col gap-1">
                          <span className={`inline-flex w-fit items-center gap-1 rounded px-1.5 py-0.5 text-[11px] ${
                            a.enabled === false
                              ? 'bg-gray-700/60 text-gray-500 dark:text-gray-400'
                              : 'bg-emerald-500/15 text-emerald-700 dark:text-emerald-300'}`}>
                            <Power className="h-3 w-3" /> {a.enabled === false ? 'désactivée' : 'active'}
                          </span>
                          {a.drift && (
                            <span className="inline-flex w-fit items-center gap-1 text-[11px] text-amber-700 dark:text-amber-400" title="La cible ou l'identifiant Homeroute ne correspond plus (ex. route legacy vers le port de l'app) — re-synchronisez.">
                              <AlertTriangle className="h-3 w-3" /> désynchronisé
                            </span>
                          )}
                          {a.require_auth === false && (
                            <span className="inline-flex w-fit items-center gap-1 text-[11px] text-amber-700 dark:text-amber-400" title="Accessible publiquement sans l'auth du reverse proxy — seule l'auth interne de l'app protège.">
                              <AlertTriangle className="h-3 w-3" /> sans auth edge
                            </span>
                          )}
                        </div>
                      ) : (
                        <span className="text-xs text-gray-600">—</span>
                      )}
                    </td>

                    <td className="py-3 pl-3 align-top">
                      <div className="flex items-center justify-end gap-1.5">
                        {a.assigned ? (
                          <>
                            <button
                              onClick={() => assign(a)} disabled={!canAct || rowBusy}
                              className="rounded-md border border-gray-700 px-2 py-1 text-xs text-gray-300 hover:bg-gray-700/40 disabled:opacity-40"
                              title="Re-synchroniser (port / config)"
                            >
                              <RotateCw className="h-3.5 w-3.5" />
                            </button>
                            <button
                              onClick={() => toggle(a)} disabled={!canAct || rowBusy}
                              className="rounded-md border border-gray-700 px-2 py-1 text-xs text-gray-300 hover:bg-gray-700/40 disabled:opacity-40"
                              title={a.enabled === false ? 'Activer' : 'Désactiver'}
                            >
                              <Power className="h-3.5 w-3.5" />
                            </button>
                            <button
                              onClick={() => remove(a)} disabled={!canAct || rowBusy}
                              className="rounded-md border border-red-500/40 px-2 py-1 text-xs text-red-600 hover:bg-red-500/10 disabled:opacity-40 dark:border-red-700/50 dark:text-red-300 dark:hover:bg-red-700/20"
                              title="Retirer le hostname"
                            >
                              <Trash2 className="h-3.5 w-3.5" />
                            </button>
                          </>
                        ) : (
                          <Button onClick={() => assign(a)} disabled={!canAct} loading={rowBusy} className="px-3 py-1 text-xs">
                            Attribuer
                          </Button>
                        )}
                      </div>
                    </td>
                  </tr>
                );
              })}
              {sortedApps.length === 0 && (
                <tr><td colSpan={4} className="py-6 text-center text-sm text-gray-500">Aucune application.</td></tr>
              )}
            </tbody>
          </table>
        </div>
      </section>
      )}
    </div>
  );
}
