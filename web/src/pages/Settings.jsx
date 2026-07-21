import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { useSearchParams } from 'react-router-dom';
import {
  Settings2, RefreshCw, Globe, Link2, Plug, CheckCircle2, XCircle,
  AlertTriangle, ExternalLink, Power, Trash2, RotateCw, Server, KeyRound, ShieldCheck,
  Bot, Smartphone, Loader2, HelpCircle,
} from 'lucide-react';
import PageHeader from '../components/PageHeader';
import Button from '../components/Button';
import useWebSocket from '../hooks/useWebSocket';
import {
  getHomerouteSettings, setHomerouteSettings, testHomeroute, registerHomeroute,
  getHomerouteAppRoutes, assignHomerouteRoute, removeHomerouteRoute, toggleHomerouteRoute,
  getAgentAuth, probeAgentAuth, setAgentAuth, clearAgentAuth, getSdkVersion, updateSdk,
  getAppsClaudeToken, probeAppsClaudeToken, setAppsClaudeToken, clearAppsClaudeToken,
  getCodexAuth, setCodexAuth, clearCodexAuth, getCodexSdkVersion, updateCodexSdk,
  startCodexDeviceLogin, getCodexDeviceLogin, cancelCodexDeviceLogin,
} from '../api/client';
import { apiErr } from '../utils/apiErr';
import { useToast } from '../hooks/useToast';

const FIELD = 'w-full rounded-lg border border-gray-700 bg-gray-900/60 px-3 py-2 text-sm text-gray-100 focus:border-blue-500 focus:outline-none';
const LBL = 'mb-1 block text-xs font-medium text-gray-400';
const CARD = 'rounded-xl border border-gray-700/70 bg-gray-800/50 p-5';

// Onglets de la page. L'ordre = ordre d'affichage ; `id` = valeur de `?tab=`.
const TABS = [
  { id: 'auth', label: 'Authentification', icon: ShieldCheck },
  { id: 'codex', label: 'Moteur Codex', icon: Bot },
  { id: 'homeroute', label: 'Liaison Homeroute', icon: Server },
  { id: 'hostnames', label: 'Hostnames', icon: Globe },
];

// Cadence de consultation du flow device-login. WHY du polling ici (alors que la
// plateforme proscrit le polling au profit du WebSocket) : ce flow est PONCTUEL,
// déclenché par un clic, borné dans le temps (code valable 15 min) et piloté par un
// process externe `codex login` — pas un flux d'état live à diffuser.
const DEVICE_POLL_MS = 2000;
const DEVICE_POLL_MAX = Math.ceil((16 * 60 * 1000) / DEVICE_POLL_MS); // garde-fou : ~16 min

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

  // ── Moteur Codex (OAuth abonnement ChatGPT — jamais de clé API) ────────
  const [codexAuth, setCodexAuthState] = useState(null); // statut masqué
  const [codexSdk, setCodexSdk] = useState(null);        // { installed, latest, update_available }
  const [codexJson, setCodexJson] = useState('');        // saisie auth.json (write-only)
  const [revealCodexJson, setRevealCodexJson] = useState(false);
  const [savingCodex, setSavingCodex] = useState(false);
  const [probingCodex, setProbingCodex] = useState(false);
  const [codexProbe, setCodexProbe] = useState(null);
  const [updatingCodexSdk, setUpdatingCodexSdk] = useState(false);
  // Flow device-login : { status:'idle'|'pending'|'ok'|'error', url?, code?, error? }
  const [device, setDevice] = useState({ status: 'idle' });
  const [startingDevice, setStartingDevice] = useState(false);

  const loadAuth = useCallback(async () => {
    const [a, v, t, cx, cv] = await Promise.all([
      getAgentAuth().catch(() => null),
      getSdkVersion().catch(() => null),
      getAppsClaudeToken().catch(() => null),
      getCodexAuth().catch(() => null),
      getCodexSdkVersion().catch(() => null),
    ]);
    if (a?.data) setSdkAuth(a.data);
    if (v?.data) setSdk(v.data);
    if (t?.data) setAppsTok(t.data);
    if (cx?.data) setCodexAuthState(cx.data);
    if (cv?.data) setCodexSdk(cv.data);
  }, []);
  useEffect(() => { loadAuth(); }, [loadAuth]);

  // Reprend l'affichage d'un flow déjà en cours (démarré depuis un autre onglet/PC).
  useEffect(() => {
    let cancelled = false;
    getCodexDeviceLogin()
      .then((r) => { if (!cancelled && r.data?.status === 'pending') setDevice(r.data); })
      .catch(() => {});
    return () => { cancelled = true; };
  }, []);

  // Consultation bornée du flow tant qu'il est `pending` ; s'arrête sur ok/error, à
  // l'annulation, au démontage, et au plafond (code expiré côté OpenAI).
  const refreshCodexAuth = useCallback(async () => {
    const r = await getCodexAuth().catch(() => null);
    if (r?.data) setCodexAuthState(r.data);
  }, []);
  useEffect(() => {
    if (device.status !== 'pending') return undefined;
    let stopped = false;
    let ticks = 0;
    const id = setInterval(async () => {
      if (stopped) return;
      if (++ticks > DEVICE_POLL_MAX) {
        setDevice({ status: 'error', error: 'Code expiré — relance la connexion.' });
        return;
      }
      try {
        const r = await getCodexDeviceLogin();
        const d = r.data || {};
        if (stopped) return;
        if (d.status === 'pending') {
          // L'URL/le code peuvent arriver après le 202 (lecture du stdout de `codex login`).
          setDevice((prev) => ({ ...prev, ...d, status: 'pending' }));
          return;
        }
        setDevice(d.status ? d : { status: 'idle' });
        if (d.status === 'ok') {
          flash('ok', 'Connexion ChatGPT réussie — le moteur Codex est authentifié.');
          refreshCodexAuth();
        } else if (d.status === 'error') {
          flash('error', d.error || 'Connexion par code d’appareil échouée.');
        }
      } catch {
        /* transitoire (API qui redémarre) : on retente au tick suivant */
      }
    }, DEVICE_POLL_MS);
    return () => { stopped = true; clearInterval(id); };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [device.status, refreshCodexAuth]);

  const onStartDevice = async () => {
    setStartingDevice(true);
    setCodexProbe(null);
    try {
      const r = await startCodexDeviceLogin();
      setDevice({ status: 'pending', ...(r.data || {}) });
    } catch (e) {
      // 409 = un flow tourne déjà (autre onglet) → on l'affiche au lieu d'échouer.
      if (e.response?.status === 409) {
        const cur = await getCodexDeviceLogin().catch(() => null);
        if (cur?.data) setDevice(cur.data);
        else flash('error', 'Une connexion est déjà en cours.');
      } else {
        flash('error', apiErr(e, 'impossible de démarrer la connexion'));
      }
    } finally { setStartingDevice(false); }
  };

  const onCancelDevice = async () => {
    try {
      await cancelCodexDeviceLogin();
    } catch {
      /* déjà terminé */
    }
    setDevice({ status: 'idle' });
  };

  const onSaveCodexJson = async () => {
    const json = codexJson.trim();
    if (!json) return;
    setSavingCodex(true); setCodexProbe(null);
    try {
      const r = await setCodexAuth(json);
      setCodexAuthState(r.data);
      setCodexJson('');
      flash('ok', 'auth.json validé et enregistré — le moteur Codex est prêt.');
    } catch (e) {
      flash('error', apiErr(e, 'échec de validation de l’auth.json'));
    } finally { setSavingCodex(false); }
  };

  const onVerifyCodex = async () => {
    setProbingCodex(true); setCodexProbe(null);
    try {
      const r = await getCodexAuth(true);
      setCodexAuthState(r.data);
      setCodexProbe(r.data?.probe || null);
    } catch (e) {
      setCodexProbe({ ok: false, error: apiErr(e, 'échec') });
    } finally { setProbingCodex(false); }
  };

  const onClearCodex = async () => {
    try {
      const r = await clearCodexAuth();
      setCodexAuthState(r.data);
      setCodexProbe(null);
      flash('ok', 'Authentification Codex retirée.');
    } catch (e) {
      flash('error', apiErr(e, 'échec du retrait'));
    }
  };

  const onUpdateCodexSdk = async () => {
    setUpdatingCodexSdk(true);
    try {
      const r = await updateCodexSdk();
      setCodexSdk((s) => ({ ...s, installed: r.data?.installed, update_available: false }));
      flash('ok', `SDK Codex mis à jour (${r.data?.installed || '?'}).`);
    } catch (e) {
      flash('error', apiErr(e, 'MAJ SDK Codex échouée'));
    } finally { setUpdatingCodexSdk(false); }
  };

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
  // WHY : côté Codex, la PREUVE d'authentification est le fichier `~/.codex/auth.json`
  // (écrit directement par le CLI lors du device-login) ; le seed en base n'est qu'une
  // sauvegarde et n'est JAMAIS écrit par ce flow. Sans ce mapping, un device-login réussi
  // resterait affiché « non authentifié » pour toujours (bandeau + badge d'onglet).
  // `auth_file` a TROIS états : true (présent), false (absent), null/absent (INCONNU —
  // la sonde de présence a échoué). Un inconnu n'est PAS un « non authentifié » : sans
  // cette distinction, un serveur qui ne répond pas ferait accuser l'auth à tort.
  const codexAuthFile = typeof codexAuth?.auth_file === 'boolean' ? codexAuth.auth_file : null;
  const codexRaw = authState(codexAuthFile ? { ...codexAuth, configured: true } : codexAuth);
  // Une erreur d'auth datée reste prioritaire (signal réel) ; seul le « rien de connu »
  // devient `unknown` quand la présence du fichier n'a pas pu être établie.
  const codexSt = codexRaw === 'unconfigured' && codexAuthFile === null ? 'unknown' : codexRaw;
  // Il n'y a vraiment rien à vérifier/retirer QUE si l'absence est établie (false) et
  // qu'aucun seed n'existe en base. En état inconnu on garde les actions ouvertes : le
  // bouton « Vérifier » est justement ce qui lève le doute.
  const codexNothingKnown = codexAuthFile === false && !codexAuth?.configured;
  const copyValue = (value, msg = 'Copié.') => {
    navigator.clipboard?.writeText(value).then(
      () => flash('ok', msg),
      () => flash('error', 'Copie impossible.'),
    );
  };
  const copyCmd = (cmd) => copyValue(cmd, 'Commande copiée.');
  const anyHostIssue = sortedApps.some((a) => a.assigned && (a.drift || a.require_auth === false));
  const tabHealth = {
    auth: authSt === 'error' ? 'error' : authSt === 'unconfigured' ? 'warn' : 'ok',
    // `unknown` (présence du fichier indéterminée) ne porte AUCUN badge : on n'alerte pas
    // sur une information qu'on n'a pas — seul un échec avéré ou une absence avérée le fait.
    codex: codexSt === 'error' ? 'error' : codexSt === 'unconfigured' ? 'warn' : 'ok',
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
        <Button onClick={reload} variant="neutral" size="md" icon={RefreshCw}>Rafraîchir</Button>
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
          <Button onClick={onSaveAuth} variant="primary" size="md" icon={KeyRound} loading={savingAuth} disabled={!authToken.trim()}>
            Enregistrer
          </Button>
          <Button onClick={onVerifyAuth} variant="neutral" size="md" icon={RotateCw} loading={probingAuth} disabled={!sdkAuth?.configured}>
            Vérifier l&apos;auth actuelle
          </Button>
          {sdkAuth?.configured && (
            <Button onClick={onClearAuth} variant="neutral" size="md" icon={Trash2}>
              Retirer
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
                {' → '}<code>{sdk.latest}</code> disponible.{' '}
                <Button onClick={onUpdateSdk} variant="warning" size="xs" loading={updatingSdk}>
                  Mettre à jour
                </Button>
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
          <Button variant="neutral" size="md" onClick={() => copyCmd('claude setup-token')}>
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
          <Button onClick={onSaveAppsTok} variant="primary" size="md" icon={KeyRound} loading={savingAppsTok} disabled={!appsTokInput.trim()}>
            Enregistrer
          </Button>
          <Button onClick={onVerifyAppsTok} variant="neutral" size="md" icon={RotateCw} loading={probingAppsTok} disabled={!appsTok?.configured}>
            Vérifier
          </Button>
          {appsTok?.configured && (
            <Button onClick={onClearAppsTok} variant="neutral" size="md" icon={Trash2}>
              Retirer
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

      {/* ── Onglet Moteur Codex (OAuth abonnement ChatGPT uniquement) ───── */}
      {activeTab === 'codex' && (
      <>
      <section className={CARD}>
        <h2 className="mb-1 flex items-center gap-2 text-sm font-semibold text-gray-100">
          <Smartphone className="h-4 w-4 text-blue-400" /> Connexion abonnement ChatGPT (recommandé)
        </h2>
        <p className="mb-4 text-xs text-gray-500">
          Le moteur Codex s&apos;authentifie avec ton <strong>abonnement ChatGPT</strong> — jamais avec
          une clé API. Le serveur étant headless, la connexion passe par un <em>code d&apos;appareil</em> :
          Atelier lance <code>codex login</code>, affiche le lien et le code, tu valides depuis
          n&apos;importe quel navigateur connecté, et l&apos;<code>auth.json</code> est écrit sur le serveur.
        </p>

        {/* Bandeau de statut (même source que le badge d'onglet : authState) */}
        <div className="mb-4 text-sm">
          {codexSt === 'error' ? (
            <span className="inline-flex flex-wrap items-center gap-1.5 text-red-700 dark:text-red-300">
              <XCircle className="h-4 w-4" /> Authentification en échec — reconnecte-toi
              {codexAuth?.last_error_msg && <span className="text-gray-500">· {codexAuth.last_error_msg}</span>}
            </span>
          ) : codexSt === 'unconfigured' ? (
            <span className="inline-flex items-center gap-1.5 text-amber-700 dark:text-amber-300">
              <AlertTriangle className="h-4 w-4" /> Non authentifié — le moteur Codex est indisponible
              tant qu&apos;aucun <code className="mx-1">auth.json</code> n&apos;est présent.
            </span>
          ) : codexSt === 'unknown' ? (
            // Ton NEUTRE volontaire : on ne sait pas si l'auth.json est là (sonde en échec),
            // ce n'est ni un succès ni un échec d'authentification.
            <span className="inline-flex flex-wrap items-center gap-1.5 text-gray-400">
              <HelpCircle className="h-4 w-4" /> État indéterminé — la présence de
              <code className="mx-1">auth.json</code> n&apos;a pas pu être vérifiée ; relance la vérification.
            </span>
          ) : (
            <span className="inline-flex flex-wrap items-center gap-1.5 text-emerald-700 dark:text-emerald-300">
              <CheckCircle2 className="h-4 w-4" /> Authentifié (abonnement ChatGPT)
              {codexAuth?.last_ok_at && <span className="text-gray-500">· vérifié {fmtTime(codexAuth.last_ok_at)}</span>}
            </span>
          )}
        </div>

        {device.status === 'pending' ? (
          <div className="rounded-lg border border-blue-500/30 bg-blue-500/10 p-4">
            <div className="mb-3 inline-flex items-center gap-2 text-sm text-blue-700 dark:text-blue-200">
              <Loader2 className="h-4 w-4 animate-spin" /> En attente de ton approbation…
            </div>

            <div className="mb-3">
              <div className={LBL}>1. Ouvre ce lien</div>
              <div className="flex flex-wrap items-center gap-2">
                <a
                  href={device.url || 'https://auth.openai.com/codex/device'}
                  target="_blank"
                  rel="noopener noreferrer"
                  className="inline-flex items-center gap-1 break-all font-mono text-sm text-blue-600 hover:text-blue-700 dark:text-blue-400 dark:hover:text-blue-300"
                >
                  {device.url || 'https://auth.openai.com/codex/device'} <ExternalLink className="h-3 w-3 shrink-0" />
                </a>
                <Button variant="neutral" size="sm"
                  onClick={() => copyValue(device.url || 'https://auth.openai.com/codex/device', 'Lien copié.')}>
                  Copier le lien
                </Button>
              </div>
            </div>

            <div className="mb-3">
              <div className={LBL}>2. Saisis ce code (valable 15 minutes)</div>
              <div className="flex flex-wrap items-center gap-3">
                <code className="rounded-lg border border-gray-700 bg-gray-900/60 px-4 py-2 font-mono text-2xl tracking-[0.2em] text-gray-100">
                  {device.code || '…'}
                </code>
                <Button variant="neutral" size="md" disabled={!device.code}
                  onClick={() => copyValue(device.code, 'Code copié.')}>
                  Copier le code
                </Button>
              </div>
            </div>

            <Button variant="neutral" size="md" icon={XCircle} onClick={onCancelDevice}>
              Annuler
            </Button>
          </div>
        ) : (
          <div className="flex flex-wrap items-center gap-3">
            <Button onClick={onStartDevice} variant="primary" size="md" icon={Smartphone} loading={startingDevice}>
              Connexion par code d&apos;appareil
            </Button>
            <Button onClick={onVerifyCodex} variant="neutral" size="md" icon={RotateCw} loading={probingCodex}
              disabled={codexNothingKnown}>
              Vérifier l&apos;auth actuelle
            </Button>
            {!codexNothingKnown && (
              <Button onClick={onClearCodex} variant="neutral" size="md" icon={Trash2}>
                Retirer
              </Button>
            )}
            {device.status === 'error' && (
              <span className="inline-flex items-center gap-1.5 text-sm text-red-700 dark:text-red-300">
                <XCircle className="h-4 w-4" /> {device.error || 'connexion échouée'}
              </span>
            )}
            {codexProbe && (
              codexProbe.ok ? (
                <span className="inline-flex items-center gap-1.5 text-sm text-emerald-700 dark:text-emerald-300">
                  <CheckCircle2 className="h-4 w-4" /> Auth OK (inférence réussie)
                </span>
              ) : (
                <span className="inline-flex items-center gap-1.5 text-sm text-red-700 dark:text-red-300">
                  <XCircle className="h-4 w-4" /> {codexProbe.error || 'authentification échouée'}
                </span>
              )
            )}
          </div>
        )}

        <p className="mt-4 text-xs text-gray-500">
          Si la validation est refusée, active l&apos;autorisation par appareil dans les
          paramètres de sécurité de ton compte ChatGPT, puis relance la connexion.
        </p>
      </section>

      {/* ── Alternative : coller un auth.json généré sur un poste ────────── */}
      <section className={`${CARD} mt-5`}>
        <h2 className="mb-1 flex items-center gap-2 text-sm font-semibold text-gray-100">
          <KeyRound className="h-4 w-4 text-blue-400" /> Coller un auth.json (alternative)
        </h2>
        <p className="mb-3 text-xs text-gray-500">
          Si le code d&apos;appareil n&apos;aboutit pas : connecte-toi sur <strong>ton poste</strong>,
          puis colle le contenu du fichier ci-dessous. Il est validé par un vrai tour d&apos;inférence
          avant d&apos;être écrit côté serveur.
        </p>

        <div className="mb-4 space-y-2">
          {['codex login', 'cat ~/.codex/auth.json'].map((cmd) => (
            <div key={cmd} className="flex items-center gap-2">
              <code className="flex-1 rounded-lg border border-gray-700 bg-gray-900/60 px-3 py-2 font-mono text-sm text-gray-200">
                {cmd}
              </code>
              <Button variant="neutral" size="md" onClick={() => copyCmd(cmd)}>Copier</Button>
            </div>
          ))}
        </div>

        <div>
          <label className={LBL}>
            Contenu de <code>~/.codex/auth.json</code>
            <button
              type="button"
              onClick={() => setRevealCodexJson((v) => !v)}
              className="ml-2 text-[10px] text-gray-400 hover:text-gray-200"
            >
              {revealCodexJson ? 'masquer' : 'afficher'}
            </button>
          </label>
          <textarea
            className={`${FIELD} h-24 font-mono ${revealCodexJson ? '' : '[-webkit-text-security:disc]'}`}
            value={codexJson}
            onChange={(e) => setCodexJson(e.target.value)}
            autoComplete="off"
            spellCheck={false}
            placeholder={'{"OPENAI_API_KEY":null,"tokens":{…}}'}
          />
        </div>

        <div className="mt-4 flex flex-wrap items-center gap-3">
          <Button onClick={onSaveCodexJson} variant="primary" size="md" icon={KeyRound}
            loading={savingCodex} disabled={!codexJson.trim()}>
            Enregistrer
          </Button>
          <Button onClick={onVerifyCodex} variant="neutral" size="md" icon={RotateCw} loading={probingCodex}
            disabled={codexNothingKnown}>
            Vérifier
          </Button>
          {!codexNothingKnown && (
            <Button onClick={onClearCodex} variant="neutral" size="md" icon={Trash2}>
              Retirer
            </Button>
          )}
        </div>
      </section>

      {/* ── SDK Codex (même pattern que le SDK Claude) ───────────────────── */}
      <section className={`${CARD} mt-5`}>
        <h2 className="mb-1 flex items-center gap-2 text-sm font-semibold text-gray-100">
          <Bot className="h-4 w-4 text-blue-400" /> SDK Codex
        </h2>
        <p className="mb-3 text-xs text-gray-500">
          Version de <code>@openai/codex-sdk</code> embarquée par le runner (avec le binaire CLI
          <code className="mx-1">codex</code> correspondant). La MAJ est appliquée au runner déployé.
        </p>
        {codexSdk ? (
          <div className="text-xs text-gray-500">
            SDK Codex <code>{codexSdk.installed || '?'}</code>
            {codexSdk.update_available ? (
              <>
                {' → '}<code>{codexSdk.latest}</code> disponible.{' '}
                <Button onClick={onUpdateCodexSdk} variant="warning" size="xs" loading={updatingCodexSdk}>
                  Mettre à jour
                </Button>
              </>
            ) : (
              <span> · à jour</span>
            )}
          </div>
        ) : (
          <div className="text-xs text-gray-500">Version indisponible.</div>
        )}
      </section>
      </>
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
          <Button onClick={saveSettings} variant="primary" size="md" loading={saving}>Enregistrer</Button>
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
          <Button onClick={doRegister} variant="primary" size="md" icon={Plug} loading={registering} disabled={!configured}>
            Connecter / S&apos;enregistrer
          </Button>
          <Button onClick={doTest} variant="neutral" size="md" icon={RotateCw} loading={testing}>
            Tester la connexion
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
                          <Button onClick={() => assign(a)} variant="primary" size="sm" disabled={!canAct} loading={rowBusy}>
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
