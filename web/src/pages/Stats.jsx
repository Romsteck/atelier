import { useCallback, useEffect, useMemo, useState } from 'react';
import {
  BarChart3, Activity, Bot, Hammer, ShieldAlert, AlertTriangle,
  LayoutGrid, Archive, HardDrive, RefreshCw, Database, GitBranch,
  Cpu, FileText, MessageSquare, Network,
} from 'lucide-react';

import PageHeader from '../components/PageHeader';
import Button from '../components/Button';
import Section from '../components/Section';
import StatCard, { StatSkeleton } from '../components/StatCard';
import ScrollableTable from '../components/ScrollableTable';
import CommitHeatmap from '../components/git/CommitHeatmap';
import DailyBars from '../components/stats/DailyBars';
import { unwrapApi } from '../api/client';
import {
  getStatsOverview, getStatsApps, getStatsDataverse, getStatsDisk,
  getStatsGitActivity, getStatsPerf,
} from '../api/client';
import { timeAgo, formatBytes, freshnessClasses } from '../utils/formatters';

// ── Formatage compact (aucune lib) ──────────────────────────────────────────
function fmtNum(n) {
  if (n == null) return '—';
  const v = Number(n);
  if (v >= 1e9) return (v / 1e9).toFixed(1) + 'G';
  if (v >= 1e6) return (v / 1e6).toFixed(1) + 'M';
  if (v >= 1e3) return (v / 1e3).toFixed(1) + 'k';
  return String(v);
}
const fmtCost = (c) => (c == null ? '—' : '$' + Number(c).toFixed(2));
const fmtPct = (p) => (p == null ? '—' : p.toFixed(1) + '%');

// Classes littérales statiques (Tailwind v4 ne génère pas les classes dynamiques).
const STATE_CLS = {
  running: 'bg-emerald-500/10 text-emerald-700 dark:text-emerald-300 border-emerald-500/30',
  starting: 'bg-amber-500/10 text-amber-700 dark:text-amber-300 border-amber-500/30',
  stopping: 'bg-amber-500/10 text-amber-700 dark:text-amber-300 border-amber-500/30',
  crashed: 'bg-red-500/10 text-red-700 dark:text-red-300 border-red-500/30',
  stopped: 'bg-gray-500/10 text-gray-600 dark:text-gray-300 border-gray-500/30',
  unknown: 'bg-gray-500/10 text-gray-600 dark:text-gray-300 border-gray-500/30',
};
const BUILD_CLS = {
  success: 'text-emerald-600 dark:text-emerald-300',
  error: 'text-red-600 dark:text-red-300',
  interrupted: 'text-amber-600 dark:text-amber-300',
  running: 'text-blue-600 dark:text-blue-300',
};
const SEV_CLS = {
  critical: 'text-red-600 dark:text-red-300',
  high: 'text-orange-600 dark:text-orange-300',
  medium: 'text-amber-600 dark:text-amber-300',
  low: 'text-gray-500 dark:text-gray-400',
};

function StatePill({ state }) {
  const cls = STATE_CLS[state] || STATE_CLS.unknown;
  return (
    <span className={`inline-flex items-center rounded-full border px-2 py-0.5 text-[11px] font-medium ${cls}`}>
      {state}
    </span>
  );
}

export default function Stats() {
  const [overview, setOverview] = useState(null);
  const [apps, setApps] = useState([]);
  const [dataverse, setDataverse] = useState(null);
  const [disk, setDisk] = useState(null);
  const [gitActivity, setGitActivity] = useState(null);
  const [perf, setPerf] = useState(null);

  const [loading, setLoading] = useState(true);
  const [heavyLoading, setHeavyLoading] = useState(true);
  const [refreshing, setRefreshing] = useState(false);

  // Fetch principal (rapide) : overview + tableau par app.
  const reload = useCallback(async () => {
    const [ov, ap] = await Promise.all([
      getStatsOverview().catch(() => null),
      getStatsApps().catch(() => null),
    ]);
    if (ov) setOverview(unwrapApi(ov));
    if (ap) setApps(unwrapApi(ap)?.apps || []);
    setLoading(false);
  }, []);

  // Fetch des sources lentes (fan-out dataverse, du disque, git) — en parallèle,
  // sans bloquer le rendu du fetch principal ; caches serveur côté endpoints.
  const reloadHeavy = useCallback(async (refresh = false) => {
    setHeavyLoading(true);
    const [dv, dk, ga, pf] = await Promise.all([
      getStatsDataverse(refresh).catch(() => null),
      getStatsDisk(refresh).catch(() => null),
      getStatsGitActivity().catch(() => null),
      getStatsPerf().catch(() => null),
    ]);
    if (dv) setDataverse(unwrapApi(dv));
    if (dk) setDisk(unwrapApi(dk));
    if (ga) setGitActivity(unwrapApi(ga));
    if (pf) setPerf(unwrapApi(pf));
    setHeavyLoading(false);
  }, []);

  useEffect(() => {
    reload();
    reloadHeavy(false);
  }, [reload, reloadHeavy]);

  const onRefresh = async () => {
    setRefreshing(true);
    await Promise.all([reload(), reloadHeavy(true)]);
    setRefreshing(false);
  };

  // ── Index par slug pour fusionner perf/dataverse/disque dans le tableau ────
  const perfBySlug = useMemo(() => {
    const m = {};
    (perf?.apps || []).forEach((p) => { m[p.slug] = p; });
    return m;
  }, [perf]);
  const dvBySlug = useMemo(() => {
    const m = {};
    (dataverse?.apps || []).forEach((d) => { m[d.slug] = d; });
    return m;
  }, [dataverse]);

  // ── KPI dérivés de l'overview ──────────────────────────────────────────────
  const kpi = useMemo(() => {
    if (!overview) return null;
    const builds = overview.builds_7d || [];
    const buildsTotal = builds.reduce((s, b) => s + b.count, 0);
    const buildsErr = builds.filter((b) => b.status === 'error' || b.status === 'interrupted')
      .reduce((s, b) => s + b.count, 0);
    const byLevel = overview.logs?.by_level || [];
    const logErr = byLevel.filter((l) => l.level === 'error' || l.level === 'warn')
      .reduce((s, l) => s + l.count, 0);
    const running = overview.apps?.by_state?.running || 0;
    const openSev = overview.surveillance?.findings_open || {};
    return {
      hitsToday: overview.traffic?.today?.hits ?? 0,
      err5xx: overview.traffic?.today?.errors_5xx ?? 0,
      cost30d: overview.agent?.cost_30d ?? 0,
      turns30d: overview.agent?.turns_30d ?? 0,
      tokens30d: (overview.agent?.tokens_in_30d ?? 0) + (overview.agent?.tokens_out_30d ?? 0),
      buildsTotal, buildsErr,
      findingsOpen: overview.surveillance?.findings_open_total ?? 0,
      findingsSev: ['critical', 'high', 'medium', 'low']
        .map((s) => (openSev[s] ? `${openSev[s]} ${s}` : null)).filter(Boolean).join(' · '),
      logErr, logTotal: overview.logs?.total ?? 0,
      running, appsTotal: overview.apps?.total ?? 0,
      backupLast: overview.backup?.last || null,
      diskTotal: disk ? (disk.databases_total + disk.workspaces_total + disk.git_total) : null,
    };
  }, [overview, disk]);

  const trafficBars = (overview?.traffic?.series_30d || []).map((d) => ({
    label: d.day, value: d.hits, danger: d.errors_5xx > 0,
  }));
  const agentBars = (overview?.agent?.series_30d || []).map((d) => ({
    label: d.day, value: d.cost,
  }));

  return (
    <div className="mx-auto flex max-w-7xl flex-col gap-6 p-4 sm:p-6">
      <PageHeader icon={BarChart3} title="Statistiques d'utilisation">
        <Button onClick={onRefresh} variant="secondary" size="sm" icon={RefreshCw} loading={refreshing}>
          Rafraîchir
        </Button>
      </PageHeader>

      {/* ── Tuiles KPI ────────────────────────────────────────────────────── */}
      <div className="grid grid-cols-2 md:grid-cols-4 gap-3">
        {loading || !kpi ? (
          Array.from({ length: 8 }).map((_, i) => <StatSkeleton key={i} />)
        ) : (
          <>
            <StatCard icon={Activity} color="text-blue-400" label="Requêtes aujourd'hui"
              value={fmtNum(kpi.hitsToday)} sub={kpi.err5xx > 0 ? `${kpi.err5xx} erreurs 5xx` : 'aucune erreur 5xx'} />
            <StatCard icon={Bot} color="text-purple-400" label="Coût agent · 30 j"
              value={fmtCost(kpi.cost30d)} sub={`${fmtNum(kpi.turns30d)} tours · ${fmtNum(kpi.tokens30d)} tokens`} />
            <StatCard icon={Hammer} color="text-amber-400" label="Builds · 7 j"
              value={fmtNum(kpi.buildsTotal)} sub={kpi.buildsErr > 0 ? `${kpi.buildsErr} en échec` : 'tous réussis'} />
            <StatCard icon={ShieldAlert} color="text-red-400" label="Findings ouverts"
              value={fmtNum(kpi.findingsOpen)} sub={kpi.findingsSev || '—'} to="/surveillance" />
            <StatCard icon={AlertTriangle} color="text-orange-400" label="Logs err/warn · 24 h"
              value={fmtNum(kpi.logErr)} sub={`${fmtNum(kpi.logTotal)} events au total`} />
            <StatCard icon={LayoutGrid} color="text-emerald-400" label="Apps actives"
              value={`${kpi.running}/${kpi.appsTotal}`} sub="en cours d'exécution" to="/" />
            <StatCard icon={Archive} color="text-cyan-400" label="Dernière sauvegarde"
              value={kpi.backupLast ? timeAgo(kpi.backupLast.finished_at) : '—'}
              sub={kpi.backupLast ? kpi.backupLast.status : 'aucune'} to="/backup" />
            <StatCard icon={HardDrive} color="text-indigo-400" label="Disque total"
              value={kpi.diskTotal != null ? formatBytes(kpi.diskTotal) : '…'}
              sub={heavyLoading && kpi.diskTotal == null ? 'calcul…' : 'bases + workspaces + git'} />
          </>
        )}
      </div>

      {/* ── Séries temporelles trafic + coût agent ────────────────────────── */}
      <div className="grid gap-4 lg:grid-cols-2">
        <Section title="Trafic HTTP · 30 jours">
          <div className="px-4 pb-4">
            <DailyBars data={trafficBars} color="bg-blue-500/70" format={(v) => `${v} req`} />
            <p className="mt-2 text-xs text-gray-500">
              Barres rouges = jour avec au moins une erreur 5xx. Compté au path-proxy.
            </p>
          </div>
        </Section>
        <Section title="Coût agent Studio · 30 jours">
          <div className="px-4 pb-4">
            <DailyBars data={agentBars} color="bg-purple-500/70" format={fmtCost} />
            <p className="mt-2 text-xs text-gray-500">
              Coût quotidien des tours d'agent (tokens facturés). Total 30 j : {fmtCost(kpi?.cost30d)}.
            </p>
          </div>
        </Section>
      </div>

      {/* ── Tableau par app ───────────────────────────────────────────────── */}
      <Section title="Par application">
        <ScrollableTable>
          <table className="w-full text-sm">
            <thead>
              <tr className="text-left text-xs uppercase tracking-wider text-gray-500 border-b border-gray-700/50">
                <th className="px-3 py-2">App</th>
                <th className="px-3 py-2 text-right">Req 24h / 7j</th>
                <th className="px-3 py-2 text-right">Latence</th>
                <th className="px-3 py-2 text-right">Tokens / Coût 30j</th>
                <th className="px-3 py-2 text-right">CPU / RAM</th>
                <th className="px-3 py-2 text-right">DB (lignes · taille)</th>
                <th className="px-3 py-2 text-right">Findings</th>
                <th className="px-3 py-2 text-right">Docs</th>
                <th className="px-3 py-2">Dernier build</th>
                <th className="px-3 py-2">Contexte</th>
              </tr>
            </thead>
            <tbody>
              {apps.map((a) => {
                const m = a.metrics || {};
                const enc = a.encapsulation || {};
                const pf = perfBySlug[a.slug];
                const dv = dvBySlug[a.slug];
                const lb = m.last_build;
                const lat = m.latency_ms_avg;
                return (
                  <tr key={a.slug} className="border-b border-gray-800/50 hover:bg-gray-800/30">
                    <td className="px-3 py-2">
                      <div className="flex items-center gap-2">
                        <span className="font-medium">{a.slug}</span>
                        <StatePill state={a.state} />
                      </div>
                      <div className="text-[11px] text-gray-500">{a.stack || '—'}</div>
                    </td>
                    <td className="px-3 py-2 text-right tabular-nums">
                      {fmtNum(m.hits_24h ?? 0)} <span className="text-gray-500">/ {fmtNum(m.hits_7d ?? 0)}</span>
                      {m.errors_7d > 0 && <div className="text-[11px] text-red-500">{m.errors_7d} err 7j</div>}
                    </td>
                    <td className="px-3 py-2 text-right tabular-nums text-gray-400">
                      {lat != null ? `${Math.round(lat)} ms` : '—'}
                    </td>
                    <td className="px-3 py-2 text-right tabular-nums">
                      {fmtNum(m.tokens_30d ?? 0)}
                      <div className="text-[11px] text-gray-500">{fmtCost(m.cost_30d ?? 0)}</div>
                    </td>
                    <td className="px-3 py-2 text-right tabular-nums">
                      {pf ? (
                        <>
                          <span className={pf.cpu_pct > 50 ? 'text-amber-500' : ''}>{fmtPct(pf.cpu_pct)}</span>
                          <div className="text-[11px] text-gray-500">{pf.memory_bytes != null ? formatBytes(pf.memory_bytes) : '—'}</div>
                        </>
                      ) : (
                        <span className="text-gray-600">{heavyLoading ? '…' : '—'}</span>
                      )}
                    </td>
                    <td className="px-3 py-2 text-right tabular-nums">
                      {a.has_db ? (
                        dv && !dv.error ? (
                          <>
                            {fmtNum(dv.rows_estimate ?? 0)}
                            <div className="text-[11px] text-gray-500">{dv.size_bytes != null ? formatBytes(dv.size_bytes) : '—'}</div>
                          </>
                        ) : (
                          <span className="text-gray-600">{heavyLoading ? '…' : '—'}</span>
                        )
                      ) : (
                        <span className="text-gray-600">sans DB</span>
                      )}
                    </td>
                    <td className="px-3 py-2 text-right tabular-nums">
                      {m.findings_open ? (
                        <span className="text-red-500">{m.findings_open}</span>
                      ) : (
                        <span className="text-emerald-600">0</span>
                      )}
                    </td>
                    <td className="px-3 py-2 text-right tabular-nums text-gray-400">
                      {m.docs ? `${m.docs.entries} · ${m.docs.with_diagram}◈` : '—'}
                    </td>
                    <td className="px-3 py-2 text-xs">
                      {lb ? (
                        <span className={BUILD_CLS[lb.status] || 'text-gray-400'}>
                          {lb.kind} · {lb.status}
                          {lb.finished_at && <span className="text-gray-500"> · {timeAgo(lb.finished_at)}</span>}
                        </span>
                      ) : <span className="text-gray-600">—</span>}
                    </td>
                    <td className="px-3 py-2 text-xs">
                      {enc.claude_md ? (
                        <span className={freshnessClasses(enc.claude_md_mtime)}>
                          {enc.claude_md_mtime ? timeAgo(enc.claude_md_mtime) : 'présent'}
                        </span>
                      ) : <span className="text-amber-500">pas de CLAUDE.md</span>}
                      <span className="text-gray-500"> · {enc.rules ?? 0}r/{enc.skills ?? 0}s</span>
                    </td>
                  </tr>
                );
              })}
              {apps.length === 0 && !loading && (
                <tr><td colSpan={10} className="px-3 py-6 text-center text-gray-500">Aucune application</td></tr>
              )}
            </tbody>
          </table>
        </ScrollableTable>
      </Section>

      {/* ── Sections domaine ──────────────────────────────────────────────── */}
      <div className="grid gap-4 lg:grid-cols-2">
        {/* Agent — par modèle */}
        <Section title="Agent Studio · par modèle (30 j)">
          <div className="px-4 pb-4">
            <BreakdownList
              icon={Bot}
              items={(overview?.agent?.by_model || []).map((x) => ({
                label: x.model, value: `${fmtNum(x.turns)} tours · ${fmtNum(x.tokens)} tk`,
              }))}
              empty="Aucun tour d'agent enregistré"
            />
          </div>
        </Section>

        {/* Surveillance */}
        <Section title="Surveillance IA · 30 jours">
          <div className="px-4 pb-4 space-y-2">
            <BreakdownList
              icon={ShieldAlert}
              items={Object.entries(overview?.surveillance?.runs_30d || {}).map(([k, v]) => ({
                label: k, value: fmtNum(v),
              }))}
              empty="Aucun run de surveillance"
            />
            <div className="text-xs text-gray-500">
              {fmtNum(overview?.surveillance?.tokens_30d ?? 0)} tokens consommés ·{' '}
              <a href="/surveillance" className="text-blue-400 hover:underline">voir la surveillance →</a>
            </div>
          </div>
        </Section>

        {/* Logs */}
        <Section title="Logs · 24 heures">
          <div className="px-4 pb-4">
            <BreakdownList
              icon={FileText}
              items={(overview?.logs?.by_level || []).map((l) => ({
                label: l.level, value: fmtNum(l.count),
                cls: l.level === 'error' ? SEV_CLS.critical : l.level === 'warn' ? SEV_CLS.medium : undefined,
              }))}
              empty="Aucun log sur 24 h"
            />
          </div>
        </Section>

        {/* Conversations */}
        <Section title="Conversations agent">
          <div className="px-4 pb-4 space-y-2">
            <div className="flex items-center gap-2 text-sm">
              <MessageSquare className="w-4 h-4 text-gray-500" />
              <span className="font-semibold">{fmtNum(overview?.conversations?.total ?? 0)}</span>
              <span className="text-gray-500">conversations · {overview?.conversations?.apps ?? 0} apps</span>
            </div>
            <BreakdownList
              items={(overview?.conversations?.by_model || []).map((x) => ({
                label: x.model, value: fmtNum(x.count),
              }))}
              empty="Aucune conversation"
            />
          </div>
        </Section>

        {/* Perfs live */}
        <Section title="Perfs live (CPU / RAM / réseau)">
          <div className="px-4 pb-4">
            {heavyLoading && !perf ? (
              <div className="text-xs text-gray-500">Échantillonnage…</div>
            ) : (perf?.apps || []).length === 0 ? (
              <div className="text-xs text-gray-500">Aucune app active</div>
            ) : (
              <div className="space-y-1.5">
                {(perf?.apps || []).map((p) => (
                  <div key={p.slug} className="flex items-center gap-2 text-sm">
                    <Cpu className="w-3.5 h-3.5 text-gray-500 shrink-0" />
                    <span className="w-24 truncate font-medium">{p.slug}</span>
                    <span className="tabular-nums text-gray-400 w-14 text-right">{fmtPct(p.cpu_pct)}</span>
                    <span className="tabular-nums text-gray-400 w-20 text-right">{p.memory_bytes != null ? formatBytes(p.memory_bytes) : '—'}</span>
                    <span className="flex items-center gap-1 tabular-nums text-gray-500 text-xs">
                      <Network className="w-3 h-3" />
                      {p.ip_ingress_bytes != null ? `↓${formatBytes(p.ip_ingress_bytes)} ↑${formatBytes(p.ip_egress_bytes)}` : 'n/d'}
                    </span>
                  </div>
                ))}
                <p className="text-[11px] text-gray-600 pt-1">
                  Réseau « n/d » = app lancée avant l'activation de la comptabilité IP (redémarrer l'app).
                </p>
              </div>
            )}
          </div>
        </Section>

        {/* Disque */}
        <Section title="Occupation disque">
          <div className="px-4 pb-4 space-y-2">
            {heavyLoading && !disk ? (
              <div className="text-xs text-gray-500">Calcul (du)…</div>
            ) : disk ? (
              <>
                <DiskRow icon={Database} label="Bases Postgres" bytes={disk.databases_total} n={disk.databases?.length} />
                <DiskRow icon={LayoutGrid} label="Workspaces (src/)" bytes={disk.workspaces_total} n={disk.workspaces?.length} />
                <DiskRow icon={GitBranch} label="Dépôts git" bytes={disk.git_total} n={disk.git_repos?.length} />
              </>
            ) : (
              <div className="text-xs text-gray-500">Indisponible</div>
            )}
          </div>
        </Section>
      </div>

      {/* ── Heatmap git globale ───────────────────────────────────────────── */}
      <Section title="Activité de développement (tous dépôts)">
        <CommitHeatmap data={gitActivity?.activity || []} loading={heavyLoading && !gitActivity} />
      </Section>
    </div>
  );
}

function BreakdownList({ icon: Icon, items, empty }) {
  if (!items || items.length === 0) {
    return <div className="text-xs text-gray-500">{empty}</div>;
  }
  return (
    <div className="space-y-1">
      {items.map((it, i) => (
        <div key={it.label ?? i} className="flex items-center justify-between text-sm">
          <span className="flex items-center gap-2 text-gray-400">
            {Icon && i === 0 && <Icon className="w-3.5 h-3.5 text-gray-500" />}
            <span className={Icon && i === 0 ? '' : 'pl-[22px]'}>{it.label}</span>
          </span>
          <span className={`tabular-nums ${it.cls || 'text-gray-300'}`}>{it.value}</span>
        </div>
      ))}
    </div>
  );
}

function DiskRow({ icon: Icon, label, bytes, n }) {
  return (
    <div className="flex items-center justify-between text-sm">
      <span className="flex items-center gap-2 text-gray-400">
        <Icon className="w-4 h-4 text-gray-500" />
        {label}
        {n != null && <span className="text-gray-600 text-xs">({n})</span>}
      </span>
      <span className="tabular-nums font-medium">{formatBytes(bytes || 0)}</span>
    </div>
  );
}
