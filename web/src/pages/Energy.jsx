import { useState, useEffect, useRef, useCallback } from 'react';
import {
  Zap, Cpu, Thermometer, Activity,
  Leaf, Gauge, Rocket, CheckCircle, XCircle,
  Clock, Moon, Sun, RefreshCw
} from 'lucide-react';
import PageHeader from '../components/PageHeader';
import Section from '../components/Section';
import StatusBadge from '../components/StatusBadge';
import Button from '../components/Button';
import useWebSocket from '../hooks/useWebSocket';
import {
  getEnergyHosts, getCurrentEnergyMode, setEnergyMode,
  getEnergySchedule, saveEnergySchedule,
  setGovernorCore, setGovernorAll,
} from '../api/client';

const MODES = {
  economy: { label: 'Eco', icon: Leaf, color: 'green' },
  auto: { label: 'Auto', icon: Gauge, color: 'blue' },
  performance: { label: 'Perf', icon: Rocket, color: 'orange' },
};

const GRAPH_POINTS = 60;

export default function Energy() {
  const [hosts, setHosts] = useState([]);
  const [metrics, setMetrics] = useState({});
  const [modes, setModes] = useState({});
  const [switching, setSwitching] = useState(null);
  const [message, setMessage] = useState(null);
  const [schedule, setSchedule] = useState(null);
  const [scheduleLoading, setScheduleLoading] = useState(false);
  const [loading, setLoading] = useState(true);
  const historyRef = useRef({});
  const [graphTick, setGraphTick] = useState(0);

  const pushHistory = useCallback((hostId, key, value) => {
    if (!historyRef.current[hostId]) {
      historyRef.current[hostId] = {
        temp: new Array(GRAPH_POINTS).fill(null),
        cpu: new Array(GRAPH_POINTS).fill(null),
      };
    }
    const arr = historyRef.current[hostId][key];
    arr.push(value ?? null);
    if (arr.length > GRAPH_POINTS) arr.shift();
  }, []);

  useWebSocket({
    'energy:metrics': (data) => {
      setMetrics(prev => ({ ...prev, [data.hostId]: data }));
      if (data.online !== false) {
        pushHistory(data.hostId, 'temp', data.temperature);
        pushHistory(data.hostId, 'cpu', data.cpuPercent);
        setGraphTick(t => t + 1);
      }
      setModes(prev => {
        if (prev[data.hostId]?.mode !== data.mode) {
          return { ...prev, [data.hostId]: { mode: data.mode, governor: data.governor } };
        }
        return prev;
      });
    },
  });

  useEffect(() => {
    (async () => {
      try {
        const [hostsRes, schedRes] = await Promise.all([
          getEnergyHosts(),
          getEnergySchedule(),
        ]);
        const hostList = hostsRes.data?.hosts || [];
        setHosts(hostList);
        setSchedule(schedRes.data?.config || { enabled: false, nightStart: '23:00', nightEnd: '07:00' });

        const modeResults = await Promise.all(
          hostList.map(h => getCurrentEnergyMode(h.id).catch(() => null))
        );
        const modesMap = {};
        hostList.forEach((h, i) => {
          if (modeResults[i]?.data) modesMap[h.id] = modeResults[i].data;
        });
        setModes(modesMap);
      } catch (e) {
        console.error('Energy load failed:', e);
      } finally {
        setLoading(false);
      }
    })();
  }, []);

  useEffect(() => {
    if (message) {
      const t = setTimeout(() => setMessage(null), 4000);
      return () => clearTimeout(t);
    }
  }, [message]);

  const handleSetMode = async (hostId, m) => {
    setSwitching(`${hostId}:${m}`);
    try {
      const res = await setEnergyMode(m, hostId);
      if (res.data.success) {
        setMessage({ type: 'success', text: `Mode ${MODES[m].label} applique sur ${hostId}` });
        setModes(prev => ({ ...prev, [hostId]: { mode: m, governor: '' } }));
      } else {
        setMessage({ type: 'error', text: res.data.error || 'Echec' });
      }
    } catch (e) {
      setMessage({ type: 'error', text: e.response?.data?.error || e.message });
    } finally {
      setSwitching(null);
    }
  };

  const handleScheduleSave = async () => {
    setScheduleLoading(true);
    try {
      await saveEnergySchedule(schedule);
      setMessage({ type: 'success', text: 'Planification sauvegardee' });
    } catch (e) {
      setMessage({ type: 'error', text: 'Echec sauvegarde planification' });
    } finally {
      setScheduleLoading(false);
    }
  };

  if (loading) {
    return (
      <div>
        <PageHeader title="Energie" icon={Zap} />
        <div className="text-center py-8 text-gray-400 text-sm">Chargement...</div>
      </div>
    );
  }

  return (
    <div>
      <PageHeader title="Energie" icon={Zap} />

      {message && (
        <div className={`px-6 py-2 flex items-center gap-2 text-sm ${
          message.type === 'success' ? 'bg-green-900/50 text-green-400' : 'bg-red-900/50 text-red-400'
        }`}>
          {message.type === 'success' ? <CheckCircle className="w-4 h-4" /> : <XCircle className="w-4 h-4" />}
          {message.text}
        </div>
      )}

      {/* Two servers side by side */}
      <div className="grid grid-cols-1 lg:grid-cols-2">
        {hosts.map(host => (
          <ServerCard
            key={host.id}
            host={host}
            metrics={metrics[host.id]}
            mode={modes[host.id]}
            history={historyRef.current[host.id]}
            graphTick={graphTick}
            switching={switching}
            onSetMode={handleSetMode}
          />
        ))}
      </div>

      {/* Night schedule */}
      {schedule && (
        <Section title="Planification nocturne">
          <div className="space-y-3">
            <div className="flex items-center justify-between">
              <div className="flex items-center gap-3">
                <Moon className="w-5 h-5 text-indigo-400" />
                <div>
                  <p className="text-sm text-gray-300">Economie automatique la nuit</p>
                  <p className="text-xs text-gray-500">Restaure le mode precedent au reveil</p>
                </div>
              </div>
              <button
                onClick={() => setSchedule(s => ({ ...s, enabled: !s.enabled }))}
                className={`relative w-11 h-6 rounded-full transition-colors ${
                  schedule.enabled ? 'bg-indigo-600' : 'bg-gray-600'
                }`}
              >
                <span className={`absolute top-0.5 w-5 h-5 bg-white rounded-full transition-transform ${
                  schedule.enabled ? 'left-5' : 'left-0.5'
                }`} />
              </button>
            </div>

            {schedule.enabled && (
              <div className="flex items-center gap-4 flex-wrap">
                <div className="flex items-center gap-2">
                  <Moon className="w-4 h-4 text-gray-400" />
                  <input
                    type="time"
                    value={schedule.nightStart || '23:00'}
                    onChange={(e) => setSchedule(s => ({ ...s, nightStart: e.target.value }))}
                    className="bg-gray-800 border border-gray-600 text-white text-sm px-2 py-1"
                  />
                </div>
                <div className="flex items-center gap-2">
                  <Sun className="w-4 h-4 text-gray-400" />
                  <input
                    type="time"
                    value={schedule.nightEnd || '07:00'}
                    onChange={(e) => setSchedule(s => ({ ...s, nightEnd: e.target.value }))}
                    className="bg-gray-800 border border-gray-600 text-white text-sm px-2 py-1"
                  />
                </div>
                <Button variant="primary" onClick={handleScheduleSave} loading={scheduleLoading}>
                  <Clock className="w-4 h-4" /> Sauvegarder
                </Button>
              </div>
            )}

            {!schedule.enabled && (
              <Button variant="secondary" onClick={handleScheduleSave} loading={scheduleLoading}>
                Sauvegarder
              </Button>
            )}
          </div>
        </Section>
      )}
    </div>
  );
}

// ─── Server Card ──────────────────────────────────────────────────────────────

function ServerCard({ host, metrics: m, mode, history, graphTick, switching, onSetMode }) {
  const online = m ? m.online !== false : null; // null = no data yet
  const hostMode = mode?.mode || 'unknown';
  const governor = m?.governor || mode?.governor || '';

  return (
    <div className="border-b border-r border-gray-700 bg-gray-900">
      {/* Header: name + status */}
      <div className="px-4 py-2 sm:px-5 sm:py-3 border-b border-gray-700/50 flex items-center justify-between">
        <div className="flex items-center gap-2">
          <h2 className="text-sm font-semibold text-gray-300 uppercase tracking-wider">{host.name}</h2>
          {online === true && <StatusBadge status="up">En ligne</StatusBadge>}
          {online === false && <StatusBadge status="down">Hors ligne</StatusBadge>}
          {online === null && <StatusBadge status="unknown">...</StatusBadge>}
        </div>
        {governor && online && (
          <span className="text-xs font-mono text-gray-500">{governor}</span>
        )}
      </div>

      <div className="px-4 py-3 sm:px-5 space-y-3">
        {/* Offline state */}
        {online === false && (
          <div className="text-center py-4 text-gray-500 text-sm">
            Serveur injoignable
          </div>
        )}

        {/* Metrics row */}
        {online !== false && (
          <>
            <div className="grid grid-cols-3 gap-2">
              <MetricCell
                icon={Thermometer}
                value={m?.temperature != null ? `${m.temperature.toFixed(0)}°` : '—'}
                color={m?.temperature > 80 ? 'text-red-400' : m?.temperature > 60 ? 'text-yellow-400' : 'text-green-400'}
              />
              <MetricCell
                icon={Activity}
                value={m?.cpuPercent != null ? `${m.cpuPercent.toFixed(0)}%` : '—'}
                color={m?.cpuPercent > 80 ? 'text-red-400' : m?.cpuPercent > 50 ? 'text-yellow-400' : 'text-green-400'}
              />
              <MetricCell
                icon={Cpu}
                value={m ? `${m.frequencyGhz.toFixed(2)}` : '—'}
                sub="GHz"
              />
            </div>

            {m?.model && (
              <p className="text-[10px] text-gray-600 font-mono truncate">{m.model} — {m.cores || '?'} coeurs</p>
            )}

            {/* Graphs */}
            {history && (
              <div className="grid grid-cols-2 gap-2">
                <MiniGraph data={history.temp} max={100} color="#f59e0b" label="Temp" unit="°C" tick={graphTick} />
                <MiniGraph data={history.cpu} max={100} color="#3b82f6" label="CPU" unit="%" tick={graphTick} />
              </div>
            )}
          </>
        )}

        {/* Mode selector */}
        <div className="flex gap-2">
          {Object.entries(MODES).map(([key, modeInfo]) => {
            const Icon = modeInfo.icon;
            const active = hostMode === key;
            const isSwitching = switching === `${host.id}:${key}`;
            const colorMap = {
              green: { border: 'border-green-500', bg: 'bg-green-900/30', text: 'text-green-400' },
              blue: { border: 'border-blue-500', bg: 'bg-blue-900/30', text: 'text-blue-400' },
              orange: { border: 'border-orange-500', bg: 'bg-orange-900/30', text: 'text-orange-400' },
            };
            const c = colorMap[modeInfo.color];

            return (
              <button
                key={key}
                onClick={() => !active && onSetMode(host.id, key)}
                disabled={isSwitching}
                className={`flex-1 py-2 px-2 border text-center text-xs font-medium transition-all ${
                  active
                    ? `${c.border} ${c.bg} border-2 ${c.text}`
                    : 'border-gray-700 bg-gray-800 text-gray-400 hover:bg-gray-750 hover:border-gray-600'
                } ${isSwitching ? 'opacity-60' : ''}`}
              >
                <div className="flex items-center justify-center gap-1">
                  {isSwitching ? (
                    <RefreshCw className="w-3.5 h-3.5 animate-spin" />
                  ) : (
                    <Icon className="w-3.5 h-3.5" />
                  )}
                  {modeInfo.label}
                </div>
              </button>
            );
          })}
        </div>

        {/* Per-core governor grid */}
        {online !== false && m?.perCore && m.perCore.length > 0 && (
          <PerCoreGrid cores={m.perCore} hostId={host.id} />
        )}
      </div>
    </div>
  );
}

// ─── PerCoreGrid ──────────────────────────────────────────────────────────────

const GOVERNORS = ['powersave', 'schedutil', 'performance'];
const GOV_SHORT = { powersave: 'PS', schedutil: 'SU', performance: 'PF' };
const GOV_COLORS = {
  powersave: 'text-green-400',
  schedutil: 'text-blue-400',
  performance: 'text-orange-400',
};

function PerCoreGrid({ cores, hostId }) {
  const [busy, setBusy] = useState(null); // "all:gov" or "3:gov"

  const handleSetCore = async (coreId, gov) => {
    const key = `${coreId}:${gov}`;
    setBusy(key);
    try {
      await setGovernorCore(coreId, gov, hostId);
    } catch (e) {
      console.error('Governor set failed:', e);
    } finally {
      setBusy(null);
    }
  };

  const handleSetAll = async (gov) => {
    const key = `all:${gov}`;
    setBusy(key);
    try {
      await setGovernorAll(gov, hostId);
    } catch (e) {
      console.error('Governor set all failed:', e);
    } finally {
      setBusy(null);
    }
  };

  return (
    <div>
      {/* Apply-all row */}
      <div className="flex items-center gap-2 mb-1.5">
        <span className="text-[10px] text-gray-500 uppercase tracking-wider">Tous</span>
        <div className="flex gap-1 ml-auto">
          {GOVERNORS.map(gov => (
            <button
              key={gov}
              onClick={() => handleSetAll(gov)}
              disabled={busy != null}
              className={`px-2 py-0.5 text-[10px] font-mono border border-gray-700 bg-gray-800 hover:bg-gray-750 transition-colors ${GOV_COLORS[gov] || 'text-gray-400'} ${busy === `all:${gov}` ? 'opacity-50' : ''}`}
            >
              {busy === `all:${gov}` ? '...' : GOV_SHORT[gov]}
            </button>
          ))}
        </div>
      </div>

      {/* Cores grid */}
      <div className="grid gap-px" style={{ gridTemplateColumns: `repeat(${Math.min(cores.length, 8)}, 1fr)` }}>
        {cores.map(core => {
          const pct = core.maxFreqMhz > 0
            ? Math.round((core.frequencyMhz / core.maxFreqMhz) * 100)
            : 0;
          const govColor = GOV_COLORS[core.governor] || 'text-gray-400';

          return (
            <div key={core.coreId} className="bg-gray-800 border border-gray-700/50 p-1 text-center group relative">
              <div className="text-[9px] text-gray-600">C{core.coreId}</div>
              <div className="text-[11px] font-mono text-white tabular-nums">{core.frequencyMhz}</div>
              <div className={`text-[9px] font-mono ${govColor}`}>{GOV_SHORT[core.governor] || core.governor.slice(0, 2).toUpperCase()}</div>
              {/* Frequency bar */}
              <div className="mt-0.5 h-0.5 bg-gray-700 rounded-full overflow-hidden">
                <div
                  className={`h-full transition-all ${pct > 80 ? 'bg-orange-500' : pct > 40 ? 'bg-blue-500' : 'bg-green-500'}`}
                  style={{ width: `${pct}%` }}
                />
              </div>
              {/* Hover tooltip with governor buttons */}
              <div className="hidden group-hover:flex absolute z-10 bottom-full left-1/2 -translate-x-1/2 mb-1 bg-gray-900 border border-gray-600 rounded p-1 gap-0.5 shadow-lg">
                {GOVERNORS.map(gov => (
                  <button
                    key={gov}
                    onClick={() => handleSetCore(core.coreId, gov)}
                    disabled={busy != null}
                    className={`px-1.5 py-0.5 text-[9px] font-mono border transition-colors ${
                      core.governor === gov
                        ? 'border-blue-500 bg-blue-900/40 text-blue-300'
                        : 'border-gray-700 bg-gray-800 text-gray-400 hover:bg-gray-700'
                    }`}
                  >
                    {GOV_SHORT[gov]}
                  </button>
                ))}
              </div>
            </div>
          );
        })}
      </div>
    </div>
  );
}

// ─── MetricCell ───────────────────────────────────────────────────────────────

function MetricCell({ icon: Icon, value, sub, color }) {
  return (
    <div className="bg-gray-800 border border-gray-700 p-2 text-center">
      <Icon className="w-3.5 h-3.5 text-gray-500 mx-auto mb-1" />
      <div className={`text-base font-semibold tabular-nums ${color || 'text-white'}`}>
        {value}
        {sub && <span className="text-xs text-gray-500 ml-0.5">{sub}</span>}
      </div>
    </div>
  );
}

// ─── MiniGraph (canvas) ──────────────────────────────────────────────────────

function MiniGraph({ data, max, color, label, unit, tick }) {
  const canvasRef = useRef(null);

  useEffect(() => {
    const canvas = canvasRef.current;
    if (!canvas) return;
    const ctx = canvas.getContext('2d');
    const dpr = window.devicePixelRatio || 1;
    const w = canvas.clientWidth;
    const h = canvas.clientHeight;
    canvas.width = w * dpr;
    canvas.height = h * dpr;
    ctx.scale(dpr, dpr);

    ctx.fillStyle = '#111827';
    ctx.fillRect(0, 0, w, h);

    // Grid
    ctx.strokeStyle = '#1f2937';
    ctx.lineWidth = 0.5;
    for (let i = 0; i <= 4; i++) {
      const y = (h - 16) * i / 4 + 8;
      ctx.beginPath();
      ctx.moveTo(0, y);
      ctx.lineTo(w, y);
      ctx.stroke();
    }

    const validData = data || [];
    const points = [];
    const padTop = 8;
    const padBottom = 8;
    const graphH = h - padTop - padBottom;

    for (let i = 0; i < validData.length; i++) {
      if (validData[i] != null) {
        const x = (i / (GRAPH_POINTS - 1)) * w;
        const y = padTop + graphH - (validData[i] / max) * graphH;
        points.push({ x, y });
      }
    }

    if (points.length > 1) {
      ctx.beginPath();
      ctx.moveTo(points[0].x, padTop + graphH);
      points.forEach(p => ctx.lineTo(p.x, p.y));
      ctx.lineTo(points[points.length - 1].x, padTop + graphH);
      ctx.closePath();
      ctx.fillStyle = color + '15';
      ctx.fill();

      ctx.beginPath();
      ctx.moveTo(points[0].x, points[0].y);
      for (let i = 1; i < points.length; i++) {
        ctx.lineTo(points[i].x, points[i].y);
      }
      ctx.strokeStyle = color;
      ctx.lineWidth = 1.5;
      ctx.stroke();

      const last = points[points.length - 1];
      ctx.beginPath();
      ctx.arc(last.x, last.y, 2.5, 0, Math.PI * 2);
      ctx.fillStyle = color;
      ctx.fill();
    }

    // Label + value overlay
    const lastVal = validData.filter(v => v != null).pop();
    ctx.font = '10px monospace';
    ctx.fillStyle = '#6b7280';
    ctx.textAlign = 'left';
    ctx.fillText(label, 3, h - 2);
    if (lastVal != null) {
      ctx.fillStyle = color;
      ctx.font = 'bold 10px monospace';
      ctx.textAlign = 'right';
      ctx.fillText(`${lastVal.toFixed(0)}${unit}`, w - 3, h - 2);
    }
  }, [data, max, color, label, unit, tick]);

  return (
    <div className="border border-gray-700/50 overflow-hidden">
      <canvas ref={canvasRef} className="w-full" style={{ height: '70px' }} />
    </div>
  );
}
