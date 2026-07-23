import { createContext, useCallback, useContext, useEffect, useMemo, useRef, useState } from 'react';
import useWebSocket from '../hooks/useWebSocket';
import {
  cancelPilotNight, cancelPilotRun, createPilotItem, deletePilotItem, getPilotBacklog,
  getPilotNight, getPilotSchedule, getPilotState, movePilotItem, patchPilotItem,
  runPilotItem, setPilotSchedule, startPilotNight, unwrapApi,
} from '../api/client';
import { setBadgeSlice } from '../lib/notify';

const PilotContext = createContext(null);

export function PilotProvider({ children }) {
  const [items, setItems] = useState([]);
  const [state, setState] = useState(null);
  const [schedule, setSchedule] = useState(null);
  const [night, setNight] = useState(null);
  const [transcripts, setTranscripts] = useState({});
  // Mise à jour autonome d'Atelier en cours (worker détaché) : snapshot serveur
  // au fetch + événements live `platform:maintenance` — consommé par
  // MaintenanceOverlay (bandeau de phase + overlay d'indisponibilité).
  const [maintenance, setMaintenance] = useState(null);
  const [loading, setLoading] = useState(true);

  const refetch = useCallback(async () => {
    try {
      const [a, s, sc, n] = await Promise.all([getPilotBacklog(), getPilotState(), getPilotSchedule(), getPilotNight()]);
      const list = unwrapApi(a); if (Array.isArray(list)) setItems(list);
      const st = unwrapApi(s);
      setState(st); setSchedule(unwrapApi(sc)); setNight(unwrapApi(n));
      // Champ serveur nul hors maintenance : on purge un snapshot ACTIF périmé
      // (fin manquée pendant une coupure WS) mais on préserve un verdict
      // terminal (active:false + outcome) que l'overlay est en train d'afficher.
      setMaintenance((cur) => st?.maintenance ?? (cur?.active ? null : cur));
    } catch { /* keep the last snapshot */ }
    finally { setLoading(false); }
  }, []);
  useEffect(() => { refetch(); }, [refetch]);

  const { epoch, status: wsStatus } = useWebSocket({
    'platform:maintenance': (snap) => { if (snap) setMaintenance(snap); },
    'pilot:backlog': (ev) => {
      if (!ev) return;
      if (ev.action === 'deleted') setItems((v) => v.filter((x) => x.id !== ev.id));
      else if (ev.item) setItems((v) => v.some((x) => x.id === ev.item.id)
        ? v.map((x) => x.id === ev.item.id ? ev.item : x) : [...v, ev.item]);
    },
    'pilot:night': (snap) => setNight(snap),
    'pilot:transcript': (line) => {
      if (!line?.run_id) return;
      setTranscripts((v) => ({ ...v, [line.run_id]: [...(v[line.run_id] || []).slice(-499), line] }));
    },
    'resync': (m) => { if (String(m?.channel || '').startsWith('pilot:')) refetch(); },
  });
  const prevEpoch = useRef(0);
  useEffect(() => { if (epoch && epoch !== prevEpoch.current) { prevEpoch.current = epoch; refetch(); } }, [epoch, refetch]);

  // NOTE : la capture directe (quick-add) est retirée de l'UI — les items
  // naissent via le CP (MCP backlog_add). L'endpoint HTTP POST reste serveur.
  const capture = useCallback(async (body) => {
    const item = unwrapApi(await createPilotItem(body));
    setItems((v) => v.some((x) => x.id === item.id) ? v : [...v, item]);
    return item;
  }, []);
  const patch = useCallback(async (id, body) => {
    setItems((v) => v.map((x) => x.id === id ? { ...x, ...body } : x));
    try { const item = unwrapApi(await patchPilotItem(id, body)); setItems((v) => v.map((x) => x.id === id ? item : x)); return item; }
    catch (e) { refetch(); throw e; }
  }, [refetch]);
  const move = useCallback(async (id, lane, position) => {
    setItems((v) => v.map((x) => x.id === id ? { ...x, lane, ...(position != null ? { position } : {}) } : x));
    try { const item = unwrapApi(await movePilotItem(id, { lane, position })); setItems((v) => v.map((x) => x.id === id ? item : x)); return item; }
    catch (e) { refetch(); throw e; }
  }, [refetch]);
  const remove = useCallback(async (id) => { setItems((v) => v.filter((x) => x.id !== id)); try { await deletePilotItem(id); } catch (e) { refetch(); throw e; } }, [refetch]);
  const run = useCallback(async (id, confirm = false) => unwrapApi(await runPilotItem(id, confirm)), []);
  // Annulation d'un run live — l'état de l'item revient par le WS `pilot:backlog`
  // (settle côté service), pas de mutation optimiste ici.
  const cancelRun = useCallback(async (runId) => unwrapApi(await cancelPilotRun(runId)), []);
  const saveSchedule = useCallback(async (body) => { const v = unwrapApi(await setPilotSchedule(body)); setSchedule(v); return v; }, []);
  const launchNight = useCallback(async () => { const v = unwrapApi(await startPilotNight()); setNight(v); return v; }, []);
  const stopNight = useCallback(async () => { await cancelPilotNight(); }, []);

  const counts = useMemo(() => ({
    attention: items.filter((x) => x.lane === 'attention').length,
    blocked: items.filter((x) => x.exec_status === 'blocked').length,
    running: items.filter((x) => ['queued', 'running'].includes(x.exec_status)).length,
    ready: items.filter((x) => x.lane === 'ready' && x.exec_status === 'idle').length,
    done: items.filter((x) => x.lane === 'done').length,
  }), [items]);
  useEffect(() => { setBadgeSlice('pilot', counts.attention); }, [counts.attention]);
  useEffect(() => () => setBadgeSlice('pilot', 0), []);
  const value = useMemo(() => ({ items, state, schedule, night, transcripts, maintenance, wsStatus, loading, counts, capture, patch, move, remove, run, cancelRun, saveSchedule, launchNight, stopNight, refetch }), [items, state, schedule, night, transcripts, maintenance, wsStatus, loading, counts, capture, patch, move, remove, run, cancelRun, saveSchedule, launchNight, stopNight, refetch]);
  return <PilotContext.Provider value={value}>{children}</PilotContext.Provider>;
}

export function usePilot() {
  const value = useContext(PilotContext);
  if (!value) throw new Error('usePilot must be used inside PilotProvider');
  return value;
}
