import { useState, useEffect, useCallback } from 'react';
import {
  Loader2, Workflow, AlertCircle, CheckCircle2, RefreshCw,
  Plug, Zap, GitBranch, Repeat, Layers, Equal, ListPlus, Plus, StopCircle,
  Filter, ListOrdered, Combine, Braces, ChevronRight, Hash, BarChart3, List as ListIcon,
} from 'lucide-react';
import FlowsStatsView from './FlowsStatsView';

async function api(path) {
  const res = await fetch(`/api${path}`, { credentials: 'include' });
  if (!res.ok) throw new Error(`${res.status} ${res.statusText}`);
  return res.json();
}

const KIND_ICON = {
  connector: Plug,
  action: Zap,
  if: GitBranch,
  switch: GitBranch,
  for_each: Repeat,
  while: Repeat,
  scope: Layers,
  terminate: StopCircle,
  set_var: Equal,
  append_to_var: ListPlus,
  increment_var: Plus,
  compose: Layers,
  select: Filter,
  filter: Filter,
  sort: ListOrdered,
  group_by: Combine,
  parse_json: Braces,
  length: Hash,
  take: Hash,
  dedupe: Filter,
  partition: Filter,
  join: Combine,
};

const KIND_COLOR = {
  connector: 'text-sky-400',
  action: 'text-amber-400',
  if: 'text-violet-400',
  switch: 'text-violet-400',
  for_each: 'text-emerald-400',
  while: 'text-emerald-400',
  scope: 'text-gray-400',
  terminate: 'text-rose-400',
  set_var: 'text-blue-400',
  append_to_var: 'text-blue-400',
  increment_var: 'text-blue-400',
  compose: 'text-fuchsia-400',
};

function statusPill(status) {
  if (status === 'success') return (
    <span className="inline-flex items-center gap-1 px-1.5 py-0.5 text-[10px] bg-green-500/15 text-green-400 rounded">
      <CheckCircle2 className="w-3 h-3" /> success
    </span>
  );
  if (status === 'failed') return (
    <span className="inline-flex items-center gap-1 px-1.5 py-0.5 text-[10px] bg-red-500/15 text-red-400 rounded">
      <AlertCircle className="w-3 h-3" /> failed
    </span>
  );
  return <span className="px-1.5 py-0.5 text-[10px] bg-gray-700 text-gray-300 rounded">{status}</span>;
}

function fmtDuration(ms) {
  if (ms == null) return '—';
  if (ms < 1) return '<1ms';
  if (ms < 1000) return `${ms}ms`;
  return `${(ms / 1000).toFixed(2)}s`;
}

function fmtTime(iso) {
  if (!iso) return '';
  try { return new Date(iso).toLocaleString(); } catch { return iso; }
}

function buildTree(steps) {
  const byId = new Map();
  steps.forEach(s => byId.set(s.record_id, { ...s, children: [] }));
  const roots = [];
  for (const s of steps) {
    const node = byId.get(s.record_id);
    if (s.parent_record_id && byId.has(s.parent_record_id)) {
      byId.get(s.parent_record_id).children.push(node);
    } else {
      roots.push(node);
    }
  }
  function sort(nodes) {
    nodes.sort((a, b) => (a.started_at || '').localeCompare(b.started_at || ''));
    nodes.forEach(n => sort(n.children));
  }
  sort(roots);
  return roots;
}

function DataBlock({ label, value, error }) {
  return (
    <div>
      <div className={`text-[10px] uppercase tracking-wider mb-1 ${error ? 'text-red-400' : 'text-gray-500'}`}>{label}</div>
      <pre className={`text-[11px] font-mono p-2.5 rounded max-h-48 overflow-auto whitespace-pre-wrap ${error ? 'bg-red-500/10 text-red-300 border border-red-500/30' : 'bg-gray-900 text-gray-300 border border-gray-700'}`}>
        {typeof value === 'string' ? value : JSON.stringify(value, null, 2)}
      </pre>
    </div>
  );
}

// ── Step card ──────────────────────────────────────────────────────────

function StepCard({ slug, runId, node }) {
  const [open, setOpen] = useState(false);
  const [details, setDetails] = useState(null);
  const [loadingDetails, setLoadingDetails] = useState(false);
  const [detailsError, setDetailsError] = useState(null);

  const Icon = KIND_ICON[node.kind] || Workflow;
  const colorCls = KIND_COLOR[node.kind] || 'text-gray-400';
  // Sub-discriminator label: connector → "dataverse.list", action → name.
  const detailLabel = node.detail || node.kind;
  const isFailed = node.status === 'failed';
  const borderCls = isFailed
    ? 'border-red-500/40'
    : open
      ? 'border-blue-400/50'
      : 'border-gray-700';

  async function toggle() {
    if (!open && !details && (node.has_input || node.has_output || node.has_error)) {
      setLoadingDetails(true);
      setDetailsError(null);
      try {
        const r = await api(`/apps/${slug}/flows/_runs/${runId}/steps/${node.record_id}`);
        setDetails(r.step);
      } catch (e) {
        setDetailsError(e.message);
      } finally {
        setLoadingDetails(false);
      }
    }
    setOpen(!open);
  }

  return (
    <div className="w-full max-w-2xl">
      <button
        type="button"
        onClick={toggle}
        className={`w-full text-left rounded-lg border ${borderCls} bg-gray-800/70 hover:bg-gray-800 transition-colors`}
      >
        <div className="flex items-center gap-3 px-3 py-2.5">
          <div className={`w-8 h-8 rounded-md bg-gray-900/60 flex items-center justify-center ${colorCls} shrink-0`}>
            <Icon className="w-4 h-4" />
          </div>
          <div className="flex-1 min-w-0">
            <div className="flex items-baseline gap-2">
              <span className="font-mono text-[13px] text-white truncate">{node.step_id}</span>
              <span className={`text-[10px] font-mono ${node.detail ? colorCls : 'text-gray-500'}`}>{detailLabel}</span>
            </div>
          </div>
          <span className="text-[10px] text-gray-500 font-mono shrink-0">{fmtDuration(node.duration_ms)}</span>
          {statusPill(node.status)}
          <ChevronRight className={`w-3.5 h-3.5 text-gray-500 transition-transform ${open ? 'rotate-90' : ''}`} />
        </div>
        {open && (
          <div className="px-3 pb-3 pt-2 border-t border-gray-700/60 grid gap-2.5">
            {loadingDetails && (
              <div className="flex items-center gap-2 text-[11px] text-gray-500">
                <Loader2 className="w-3.5 h-3.5 animate-spin" /> Loading…
              </div>
            )}
            {detailsError && (
              <div className="text-[11px] text-red-400">Error: {detailsError}</div>
            )}
            {details && details.input != null && <DataBlock label="Input" value={details.input} />}
            {details && (isFailed && details.error
              ? <DataBlock label="Error" value={details.error} error />
              : details.output != null && <DataBlock label="Output" value={details.output} />)}
            {!loadingDetails && !detailsError && !node.has_input && !node.has_output && !node.has_error && (
              <div className="text-[11px] text-gray-500 italic">No payload</div>
            )}
          </div>
        )}
      </button>
    </div>
  );
}

// ── Connectors (visual lines/dots between sibling cards) ───────────────

function Connector({ color = 'gray' }) {
  const colorCls = color === 'gray' ? 'bg-gray-600/70' : color === 'emerald' ? 'bg-emerald-500/40' : 'bg-violet-500/40';
  return (
    <div className="flex flex-col items-center my-1.5" aria-hidden="true">
      <div className={`w-px h-3 ${colorCls}`} />
      <div className={`w-1.5 h-1.5 rounded-full ${color === 'gray' ? 'bg-gray-500' : color === 'emerald' ? 'bg-emerald-400' : 'bg-violet-400'}`} />
      <div className={`w-px h-3 ${colorCls}`} />
    </div>
  );
}

// ── Recursive rendering with kind-aware containers ─────────────────────

function NodeWithChildren({ slug, runId, node }) {
  const childCount = node.children?.length || 0;

  // for_each → wrap children in an emerald container, group by iteration_index
  if (node.kind === 'for_each' && childCount > 0) {
    const byIter = new Map();
    for (const c of node.children) {
      const k = c.iteration_index ?? 0;
      if (!byIter.has(k)) byIter.set(k, []);
      byIter.get(k).push(c);
    }
    const sortedKeys = [...byIter.keys()].sort((a, b) => a - b);
    return (
      <>
        <StepCard slug={slug} runId={runId} node={node} />
        <Connector color="emerald" />
        <div className="w-full max-w-2xl border border-emerald-500/20 rounded-lg bg-emerald-950/20 p-3 space-y-3">
          <div className="text-[10px] font-semibold uppercase tracking-wider text-emerald-400 flex items-center gap-1.5">
            <Repeat className="w-3 h-3" />
            for_each · {sortedKeys.length} iteration{sortedKeys.length > 1 ? 's' : ''}
          </div>
          {sortedKeys.map((k, i) => (
            <div key={k} className="bg-gray-900/40 rounded border border-emerald-500/10 p-3">
              <div className="text-[10px] font-mono text-emerald-300/80 mb-2">iteration #{k}</div>
              <div className="flex flex-col items-center">
                {byIter.get(k).map((sub, j) => (
                  <div key={sub.record_id} className="w-full flex flex-col items-center">
                    {j > 0 && <Connector />}
                    <NodeWithChildren slug={slug} runId={runId} node={sub} />
                  </div>
                ))}
              </div>
            </div>
          ))}
        </div>
      </>
    );
  }

  // if / switch → wrap children in a violet container, group by branch
  if ((node.kind === 'if' || node.kind === 'switch') && childCount > 0) {
    const byBranch = new Map();
    for (const c of node.children) {
      const k = c.branch || 'then';
      if (!byBranch.has(k)) byBranch.set(k, []);
      byBranch.get(k).push(c);
    }
    return (
      <>
        <StepCard slug={slug} runId={runId} node={node} />
        <Connector color="violet" />
        <div className="w-full max-w-2xl border border-violet-500/20 rounded-lg bg-violet-950/20 p-3 space-y-3">
          <div className="text-[10px] font-semibold uppercase tracking-wider text-violet-400 flex items-center gap-1.5">
            <GitBranch className="w-3 h-3" />
            {node.kind} · branche{byBranch.size > 1 ? 's' : ''} {[...byBranch.keys()].join(', ')}
          </div>
          {[...byBranch.entries()].map(([branch, items]) => (
            <div key={branch} className="bg-gray-900/40 rounded border border-violet-500/10 p-3">
              <div className="text-[10px] font-mono text-violet-300/80 mb-2 uppercase">{branch}</div>
              <div className="flex flex-col items-center">
                {items.map((sub, j) => (
                  <div key={sub.record_id} className="w-full flex flex-col items-center">
                    {j > 0 && <Connector />}
                    <NodeWithChildren slug={slug} runId={runId} node={sub} />
                  </div>
                ))}
              </div>
            </div>
          ))}
        </div>
      </>
    );
  }

  // Default — just stack children below
  return (
    <>
      <StepCard slug={slug} runId={runId} node={node} />
      {childCount > 0 && (
        <>
          <Connector />
          <div className="pl-6 border-l border-gray-700/50 ml-3 w-full flex flex-col items-center">
            {node.children.map((sub, j) => (
              <div key={sub.record_id} className="w-full flex flex-col items-center">
                {j > 0 && <Connector />}
                <NodeWithChildren slug={slug} runId={runId} node={sub} />
              </div>
            ))}
          </div>
        </>
      )}
    </>
  );
}

function StepStream({ slug, runId, nodes }) {
  if (!nodes || nodes.length === 0) return null;
  return (
    <div className="flex flex-col items-center w-full">
      {nodes.map((n, i) => (
        <div key={n.record_id} className="w-full flex flex-col items-center">
          {i > 0 && <Connector />}
          <NodeWithChildren slug={slug} runId={runId} node={n} />
        </div>
      ))}
    </div>
  );
}

// ── Main tab ───────────────────────────────────────────────────────────

export default function FlowsTab({ slug }) {
  const [view, setView] = useState('list'); // 'list' | 'stats'
  const [flows, setFlows] = useState([]);
  const [runs, setRuns] = useState([]);
  const [loadingFlows, setLoadingFlows] = useState(true);
  const [loadingRuns, setLoadingRuns] = useState(true);
  const [selectedFlow, setSelectedFlow] = useState(null);
  const [selectedRun, setSelectedRun] = useState(null);
  const [runDoc, setRunDoc] = useState(null);
  const [loadingDoc, setLoadingDoc] = useState(false);
  const [error, setError] = useState(null);

  const loadFlows = useCallback(async () => {
    setLoadingFlows(true);
    try {
      const r = await api(`/apps/${slug}/flows`);
      setFlows(r.flows || []);
    } catch (e) {
      setError(`Cannot load flows: ${e.message}`);
    } finally { setLoadingFlows(false); }
  }, [slug]);

  const loadRuns = useCallback(async () => {
    setLoadingRuns(true);
    try {
      const qs = selectedFlow ? `?flow_name=${encodeURIComponent(selectedFlow)}` : '';
      const r = await api(`/apps/${slug}/flows/_runs${qs}`);
      setRuns(r.runs || []);
    } catch (e) {
      setError(`Cannot load runs: ${e.message}`);
    } finally { setLoadingRuns(false); }
  }, [slug, selectedFlow]);

  useEffect(() => { loadFlows(); }, [loadFlows]);
  useEffect(() => { loadRuns(); }, [loadRuns]);

  useEffect(() => {
    if (!selectedRun) { setRunDoc(null); return; }
    setLoadingDoc(true);
    api(`/apps/${slug}/flows/_runs/${encodeURIComponent(selectedRun)}`)
      .then(r => setRunDoc(r.run))
      .catch(e => setError(`Cannot load run: ${e.message}`))
      .finally(() => setLoadingDoc(false));
  }, [slug, selectedRun]);

  const tree = runDoc?.steps ? buildTree(runDoc.steps) : [];

  // ── Stats subview ───────────────────────────────────────────────
  function statsSelectFlow(_appSlug, flowName, runId) {
    // Coming back to the list view: preselect the flow + run on click
    setSelectedFlow(flowName || null);
    setSelectedRun(runId || null);
    setView('list');
  }

  return (
    <div className="flex flex-col h-full overflow-hidden">
      {/* Top bar: Liste / Stats toggle */}
      <div className="px-3 py-1.5 border-b border-gray-700 bg-gray-800/40 flex items-center gap-2 shrink-0">
        <button
          onClick={() => setView('list')}
          className={`flex items-center gap-1.5 px-2.5 py-1 rounded text-[11px] font-medium ${view === 'list' ? 'bg-blue-500 text-white' : 'text-gray-400 hover:text-white hover:bg-gray-700'}`}
        >
          <ListIcon className="w-3.5 h-3.5" /> Liste
        </button>
        <button
          onClick={() => setView('stats')}
          className={`flex items-center gap-1.5 px-2.5 py-1 rounded text-[11px] font-medium ${view === 'stats' ? 'bg-blue-500 text-white' : 'text-gray-400 hover:text-white hover:bg-gray-700'}`}
        >
          <BarChart3 className="w-3.5 h-3.5" /> Stats
        </button>
        <span className="ml-auto text-[10px] text-gray-500">{slug}</span>
      </div>

      {view === 'stats' ? (
        <div className="flex-1 overflow-hidden">
          <FlowsStatsView scope="app" slug={slug} onSelectFlow={statsSelectFlow} />
        </div>
      ) : (
      <div className="flex flex-1 overflow-hidden">
      {/* Sidebar */}
      <aside className="w-[280px] min-w-[280px] h-full bg-gray-800/50 border-r border-gray-700 flex flex-col">
        <div className="px-3 py-2 border-b border-gray-700 flex items-center justify-between">
          <span className="text-[10px] font-semibold uppercase tracking-wider text-gray-500">Flows</span>
          <button onClick={loadFlows} className="text-gray-400 hover:text-white" title="Refresh">
            <RefreshCw className={`w-3.5 h-3.5 ${loadingFlows ? 'animate-spin' : ''}`} />
          </button>
        </div>
        <div className="overflow-y-auto">
          <div
            onClick={() => setSelectedFlow(null)}
            className={`flex items-center gap-2 px-3 py-1.5 text-[12px] cursor-pointer ${selectedFlow == null ? 'bg-gray-700/50 text-white' : 'text-gray-400 hover:bg-gray-700/30'}`}
          >
            <Workflow className="w-3 h-3" />
            <span>Tous les flux</span>
            <span className="ml-auto text-[10px] text-gray-500">{flows.length}</span>
          </div>
          {flows.map(f => (
            <div
              key={f.name}
              onClick={() => setSelectedFlow(f.name)}
              className={`flex items-center gap-2 px-3 py-1.5 text-[12px] cursor-pointer ${selectedFlow === f.name ? 'bg-gray-700/50 text-white' : 'text-gray-300 hover:bg-gray-700/30'}`}
            >
              <Workflow className="w-3 h-3 text-gray-500" />
              <span className="flex-1 truncate font-mono">{f.name}</span>
              <span className="text-[10px] text-gray-500">{f.step_count}</span>
            </div>
          ))}
          {!loadingFlows && flows.length === 0 && (
            <div className="text-center py-8 text-gray-500 text-xs">Aucun flux défini</div>
          )}
        </div>

        <div className="px-3 py-2 border-t border-b border-gray-700 flex items-center justify-between">
          <span className="text-[10px] font-semibold uppercase tracking-wider text-gray-500">Runs récents</span>
          <button onClick={loadRuns} className="text-gray-400 hover:text-white" title="Refresh">
            <RefreshCw className={`w-3.5 h-3.5 ${loadingRuns ? 'animate-spin' : ''}`} />
          </button>
        </div>
        <div className="overflow-y-auto flex-1">
          {runs.map(r => (
            <div
              key={r.run_id}
              onClick={() => setSelectedRun(r.run_id)}
              className={`px-3 py-2 text-[11px] cursor-pointer border-l-2 ${selectedRun === r.run_id ? 'border-blue-400 bg-gray-700/50' : 'border-transparent hover:bg-gray-700/30'}`}
            >
              <div className="flex items-center gap-2 mb-0.5">
                {statusPill(r.status)}
                <span className="text-gray-500 ml-auto">{fmtDuration(r.duration_ms)}</span>
              </div>
              <div className="font-mono text-gray-300 truncate">{r.flow_name}</div>
              <div className="text-gray-500 text-[10px]">{fmtTime(r.started_at)}</div>
            </div>
          ))}
          {!loadingRuns && runs.length === 0 && (
            <div className="text-center py-8 text-gray-500 text-xs">Aucun run</div>
          )}
        </div>
      </aside>

      {/* Main panel */}
      <div className="flex-1 flex flex-col h-full overflow-hidden">
        {error && (
          <div className="px-4 py-2 bg-red-500/10 border-b border-red-500/30 text-red-400 text-xs flex items-center gap-2">
            <AlertCircle className="w-3.5 h-3.5" /> {error}
            <button onClick={() => setError(null)} className="ml-auto text-red-400 hover:text-white">×</button>
          </div>
        )}
        {!selectedRun ? (
          <div className="flex flex-col items-center justify-center h-full text-gray-500">
            <Workflow className="w-12 h-12 mb-3 opacity-20" />
            <p className="text-sm">Sélectionne un run pour voir son flux</p>
          </div>
        ) : loadingDoc || !runDoc ? (
          <div className="flex items-center justify-center h-full text-gray-500"><Loader2 className="w-5 h-5 animate-spin" /></div>
        ) : (
          <div className="flex flex-col h-full overflow-hidden">
            <div className="px-6 py-4 border-b border-gray-700 bg-gray-900/40">
              <div className="flex items-center gap-3 mb-1.5">
                <span className="font-mono text-[14px] text-white">{runDoc.flow_name}</span>
                {statusPill(runDoc.status)}
                <span className="text-[11px] text-gray-500 ml-auto">{fmtDuration(runDoc.duration_ms)}</span>
              </div>
              <div className="text-[11px] text-gray-500 font-mono">{runDoc.run_id}</div>
              <div className="text-[11px] text-gray-500">{fmtTime(runDoc.started_at)} · trigger: <span className="font-mono">{runDoc.trigger_kind}</span></div>
              {runDoc.error && (
                <div className="mt-3 px-3 py-2 bg-red-500/10 border border-red-500/30 rounded text-[11px] text-red-300 font-mono">
                  <div className="font-semibold mb-1 flex items-center gap-1.5"><AlertCircle className="w-3 h-3" /> Step <span className="bg-red-500/20 px-1 rounded">{runDoc.error.step_id}</span> failed</div>
                  <div>{runDoc.error.message}</div>
                </div>
              )}
            </div>

            <div className="flex-1 overflow-y-auto py-8 px-4">
              {tree.length === 0 ? (
                <div className="text-center py-8 text-gray-500 text-xs">Aucun step (le run a échoué avant exécution)</div>
              ) : (
                <div className="flex flex-col items-center">
                  <div className="w-1.5 h-1.5 rounded-full bg-gray-500 mb-1.5" />
                  <StepStream slug={slug} runId={selectedRun} nodes={tree} />
                  <div className="w-1.5 h-1.5 rounded-full bg-gray-500 mt-1.5" />
                </div>
              )}
            </div>
          </div>
        )}
      </div>
      </div>
      )}
    </div>
  );
}
