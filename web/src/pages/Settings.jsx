import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import {
  Settings2, RefreshCw, Globe, Link2, Plug, CheckCircle2, XCircle,
  AlertTriangle, ExternalLink, Power, Trash2, RotateCw, Server, KeyRound,
} from 'lucide-react';
import PageHeader from '../components/PageHeader';
import Button from '../components/Button';
import useWebSocket from '../hooks/useWebSocket';
import {
  getHomerouteSettings, setHomerouteSettings, testHomeroute, registerHomeroute,
  getHomerouteAppRoutes, assignHomerouteRoute, removeHomerouteRoute, toggleHomerouteRoute,
} from '../api/client';
import { apiErr } from '../utils/apiErr';
import { useToast } from '../hooks/useToast';

const FIELD = 'w-full rounded-lg border border-gray-700 bg-gray-900/60 px-3 py-2 text-sm text-gray-100 focus:border-blue-500 focus:outline-none';
const LBL = 'mb-1 block text-xs font-medium text-gray-400';
const CARD = 'rounded-xl border border-gray-700/70 bg-gray-800/50 p-5';

function fmtTime(iso) {
  if (!iso) return null;
  const d = new Date(iso);
  return Number.isFinite(d.getTime()) ? d.toLocaleString() : null;
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
  const dirty = useRef(false);                       // user touched the subdomain inputs

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

  // Seed per-row subdomain inputs from the server once (don't clobber edits).
  useEffect(() => {
    if (!routes?.apps || dirty.current) return;
    const seed = {};
    for (const a of routes.apps) seed[a.slug] = a.subdomain || a.slug;
    setSubdomains(seed);
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
    withBusy(a.slug, () => assignHomerouteRoute(a.slug, { subdomain: subdomains[a.slug] || a.slug }),
      `Hostname attribué à ${a.name}`);
  const remove = (a) => withBusy(a.slug, () => removeHomerouteRoute(a.slug), `Hostname retiré de ${a.name}`);
  const toggle = (a) => withBusy(a.slug, () => toggleHomerouteRoute(a.slug));

  const baseDomain = routes?.base_domain || '';
  const reachable = !!routes?.reachable;
  const hasToken = !!settings?.has_bearer_token;
  const configured = hasToken;            // configuré ⇒ actif (pas de toggle séparé)
  const registeredAt = fmtTime(settings?.registered_at);

  const sortedApps = useMemo(() => {
    if (!routes?.apps) return [];
    return [...routes.apps].sort((x, y) =>
      (Number(y.eligible) - Number(x.eligible)) || x.name.localeCompare(y.name));
  }, [routes]);

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

      {/* ── Hostnames des apps ────────────────────────────────────────── */}
      <section className={CARD}>
        <h2 className="mb-1 flex items-center gap-2 text-sm font-semibold text-gray-100">
          <Globe className="h-4 w-4 text-blue-400" /> Hostnames des applications
        </h2>
        <p className="mb-4 text-xs text-gray-500">
          Publie chaque app sur <code>{'{sous-domaine}'}.{baseDomain || 'mynetwk.biz'}</code> →
          <code> 127.0.0.1:{'{port}'}</code> via Homeroute (route gérée par cet environnement).
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
                const canAct = configured && reachable && a.eligible;
                return (
                  <tr key={a.slug} className={a.eligible ? '' : 'opacity-60'}>
                    <td className="py-3 pr-3 align-top">
                      <div className="flex items-center gap-2">
                        <span className="font-medium text-gray-100">{a.name}</span>
                        <span className="rounded bg-gray-700/60 px-1.5 py-0.5 text-[11px] text-gray-400">:{a.port}</span>
                      </div>
                      <div className="mt-0.5 text-xs text-gray-500">
                        <code>{a.slug}</code> · {a.visibility}
                        {!a.eligible && a.ineligible_reason && (
                          <span className="ml-1 text-amber-700 dark:text-amber-400/80" title={a.ineligible_reason}> · non éligible</span>
                        )}
                      </div>
                    </td>

                    <td className="py-3 pr-3 align-top">
                      {a.eligible ? (
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
                      ) : (
                        <span className="text-xs italic text-gray-600">accès par path /apps/{a.slug}/</span>
                      )}
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
                            <span className="inline-flex w-fit items-center gap-1 text-[11px] text-amber-700 dark:text-amber-400" title="Le port ou l'identifiant Homeroute ne correspond plus — re-synchronisez.">
                              <AlertTriangle className="h-3 w-3" /> désynchronisé
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
    </div>
  );
}
