import { useState, useEffect, useRef, useCallback, useMemo } from 'react';
import { ScrollText, Pause, Play, Search, ChevronDown, ChevronRight, X } from 'lucide-react';
import useWebSocket from '../hooks/useWebSocket';
import { getLogs } from '../api/client';

const LEVELS = [
  { key: 'ERROR', label: 'ERROR', color: 'text-red-400', bg: 'bg-red-400/10', rowBg: 'bg-red-400/5' },
  { key: 'WARN', label: 'WARN', color: 'text-yellow-400', bg: 'bg-yellow-400/10', rowBg: 'bg-yellow-400/5' },
  { key: 'INFO', label: 'INFO', color: 'text-blue-400', bg: 'bg-blue-400/10', rowBg: '' },
  { key: 'DEBUG', label: 'DEBUG', color: 'text-gray-500', bg: 'bg-gray-600/10', rowBg: '' },
];

const SERVICES = ['homeroute', 'edge', 'orchestrator', 'netcore'];

const TIME_RANGES = [
  { key: 'live', label: 'Live' },
  { key: '5m', label: '5m' },
  { key: '15m', label: '15m' },
  { key: '1h', label: '1h' },
  { key: '6h', label: '6h' },
  { key: '24h', label: '24h' },
];

const MAX_LOGS = 2000;

function formatTimestamp(ts) {
  if (!ts) return '';
  const d = new Date(ts);
  const h = String(d.getHours()).padStart(2, '0');
  const m = String(d.getMinutes()).padStart(2, '0');
  const s = String(d.getSeconds()).padStart(2, '0');
  const ms = String(d.getMilliseconds()).padStart(3, '0');
  return `${h}:${m}:${s}.${ms}`;
}

function timeRangeToMs(key) {
  const map = { '5m': 5 * 60000, '15m': 15 * 60000, '1h': 3600000, '6h': 6 * 3600000, '24h': 24 * 3600000 };
  return map[key] || 0;
}

export default function Logs() {
  const [liveLogs, setLiveLogs] = useState([]);
  const [historyLogs, setHistoryLogs] = useState([]);
  const [historyTotal, setHistoryTotal] = useState(0);
  const [loadingHistory, setLoadingHistory] = useState(false);

  const [activeLevels, setActiveLevels] = useState(new Set(['ERROR', 'WARN', 'INFO']));
  const [activeServices, setActiveServices] = useState(new Set(SERVICES));
  const [searchText, setSearchText] = useState('');
  const [searchInput, setSearchInput] = useState('');
  const [timeRange, setTimeRange] = useState('live');

  const [paused, setPaused] = useState(false);
  const [newCount, setNewCount] = useState(0);
  const [expandedId, setExpandedId] = useState(null);

  const scrollRef = useRef(null);
  const autoScrollRef = useRef(true);
  const debounceRef = useRef(null);
  const wsConnected = useRef(false);

  const isLive = timeRange === 'live';

  // Debounce search
  useEffect(() => {
    clearTimeout(debounceRef.current);
    debounceRef.current = setTimeout(() => {
      setSearchText(searchInput);
    }, 300);
    return () => clearTimeout(debounceRef.current);
  }, [searchInput]);

  // Toggle level filter
  const toggleLevel = useCallback((level) => {
    setActiveLevels(prev => {
      const next = new Set(prev);
      if (next.has(level)) next.delete(level);
      else next.add(level);
      return next;
    });
  }, []);

  // Toggle service filter
  const toggleService = useCallback((svc) => {
    setActiveServices(prev => {
      const next = new Set(prev);
      if (next.has(svc)) next.delete(svc);
      else next.add(svc);
      return next;
    });
  }, []);

  // WebSocket for live logs
  useWebSocket(isLive ? {
    'log:entry': (data) => {
      if (!data) return;
      wsConnected.current = true;
      const entry = { ...data, _id: data.id || `${Date.now()}-${Math.random()}` };
      setLiveLogs(prev => {
        const next = [...prev, entry];
        if (next.length > MAX_LOGS) return next.slice(next.length - MAX_LOGS);
        return next;
      });
      if (paused) {
        setNewCount(n => n + 1);
      }
    }
  } : {});

  // Fetch history logs when not live
  useEffect(() => {
    if (isLive) return;
    setLoadingHistory(true);
    setHistoryLogs([]);
    setHistoryTotal(0);

    const since = new Date(Date.now() - timeRangeToMs(timeRange)).toISOString();
    const params = { since, limit: 100, offset: 0 };
    if (activeLevels.size < 4) params.level = [...activeLevels].map(l => l.toLowerCase()).join(',');
    if (activeServices.size < SERVICES.length) params.service = [...activeServices].join(',');
    if (searchText) params.q = searchText;

    getLogs(params)
      .then(res => {
        const data = res.data;
        if (data?.logs) setHistoryLogs(data.logs);
        if (data?.total != null) setHistoryTotal(data.total);
      })
      .catch(() => {})
      .finally(() => setLoadingHistory(false));
  }, [timeRange, activeLevels, activeServices, searchText, isLive]);

  // Load more history
  const loadMore = useCallback(() => {
    if (loadingHistory || isLive) return;
    setLoadingHistory(true);
    const since = new Date(Date.now() - timeRangeToMs(timeRange)).toISOString();
    const params = { since, limit: 100, offset: historyLogs.length };
    if (activeLevels.size < 4) params.level = [...activeLevels].map(l => l.toLowerCase()).join(',');
    if (activeServices.size < SERVICES.length) params.service = [...activeServices].join(',');
    if (searchText) params.q = searchText;

    getLogs(params)
      .then(res => {
        const data = res.data;
        if (data?.logs) setHistoryLogs(prev => [...prev, ...data.logs]);
      })
      .catch(() => {})
      .finally(() => setLoadingHistory(false));
  }, [loadingHistory, isLive, timeRange, historyLogs.length, activeLevels, activeServices, searchText]);

  // Auto-scroll in live mode
  useEffect(() => {
    if (!isLive || paused) return;
    const el = scrollRef.current;
    if (el) {
      el.scrollTop = el.scrollHeight;
    }
  }, [liveLogs, isLive, paused]);

  // Resume auto-scroll
  const resume = useCallback(() => {
    setPaused(false);
    setNewCount(0);
    const el = scrollRef.current;
    if (el) el.scrollTop = el.scrollHeight;
  }, []);

  // Filter live logs client-side
  const filteredLogs = useMemo(() => {
    const source = isLive ? liveLogs : historyLogs;
    return source.filter(log => {
      const level = (log.level || 'INFO').toUpperCase();
      if (!activeLevels.has(level)) return false;
      const svc = (log.service || '').toLowerCase();
      if (svc && !activeServices.has(svc)) return false;
      if (searchText) {
        const q = searchText.toLowerCase();
        const msg = (log.message || '').toLowerCase();
        const src = typeof log.source === 'object'
          ? [log.source.crate_name, log.source.module, log.source.function].filter(Boolean).join(' ').toLowerCase()
          : (log.source || '').toLowerCase();
        if (!msg.includes(q) && !src.includes(q)) return false;
      }
      return true;
    });
  }, [isLive, liveLogs, historyLogs, activeLevels, activeServices, searchText]);

  const levelCfg = (level) => LEVELS.find(l => l.key === (level || 'INFO').toUpperCase()) || LEVELS[2];

  return (
    <div className="flex flex-col h-full">
      {/* Header */}
      <div className="p-4 sm:p-6 border-b border-gray-700">
        <div className="flex items-center gap-3">
          <ScrollText className="w-6 h-6 text-blue-400" />
          <h1 className="text-2xl font-bold text-white">Logs</h1>
          {isLive && (
            <span className="flex items-center gap-1.5 ml-2">
              <span className="relative flex h-2.5 w-2.5">
                <span className="animate-ping absolute inline-flex h-full w-full rounded-full bg-green-400 opacity-75" />
                <span className="relative inline-flex rounded-full h-2.5 w-2.5 bg-green-400" />
              </span>
              <span className="text-xs text-green-400">Live</span>
            </span>
          )}
          <span className="text-sm text-gray-500 ml-auto">
            {isLive ? liveLogs.length : filteredLogs.length} entrees
          </span>
        </div>
      </div>

      {/* Filters bar */}
      <div className="px-4 sm:px-6 py-3 border-b border-gray-700 flex flex-wrap items-center gap-2">
        {/* Level toggles */}
        <div className="flex gap-1">
          {LEVELS.map(l => (
            <button
              key={l.key}
              onClick={() => toggleLevel(l.key)}
              className={`px-2.5 py-1 text-xs rounded-lg transition-colors border ${
                activeLevels.has(l.key)
                  ? `${l.bg} ${l.color} border-current/20`
                  : 'bg-gray-800 text-gray-500 border-gray-700 hover:bg-gray-700'
              }`}
            >
              {l.label}
            </button>
          ))}
        </div>

        <div className="w-px h-5 bg-gray-700" />

        {/* Service chips */}
        <div className="flex gap-1">
          {SERVICES.map(svc => (
            <button
              key={svc}
              onClick={() => toggleService(svc)}
              className={`px-2.5 py-1 text-xs rounded-lg transition-colors border ${
                activeServices.has(svc)
                  ? 'bg-gray-700 text-gray-200 border-gray-600'
                  : 'bg-gray-800 text-gray-500 border-gray-700 hover:bg-gray-700'
              }`}
            >
              {svc}
            </button>
          ))}
        </div>

        <div className="w-px h-5 bg-gray-700" />

        {/* Search */}
        <div className="relative flex-1 min-w-[180px] max-w-xs">
          <Search className="absolute left-2.5 top-1/2 -translate-y-1/2 w-3.5 h-3.5 text-gray-500" />
          <input
            type="text"
            value={searchInput}
            onChange={(e) => setSearchInput(e.target.value)}
            placeholder="Rechercher..."
            className="w-full pl-8 pr-8 py-1.5 text-xs bg-gray-700 border border-gray-600 text-white rounded-lg placeholder-gray-500 focus:outline-none focus:border-blue-500"
          />
          {searchInput && (
            <button onClick={() => { setSearchInput(''); setSearchText(''); }} className="absolute right-2 top-1/2 -translate-y-1/2 text-gray-500 hover:text-gray-300">
              <X className="w-3.5 h-3.5" />
            </button>
          )}
        </div>

        <div className="w-px h-5 bg-gray-700" />

        {/* Time range */}
        <div className="flex gap-1">
          {TIME_RANGES.map(tr => (
            <button
              key={tr.key}
              onClick={() => { setTimeRange(tr.key); setPaused(false); setNewCount(0); }}
              className={`px-2.5 py-1 text-xs rounded-lg transition-colors border ${
                timeRange === tr.key
                  ? 'bg-blue-500/20 text-blue-400 border-blue-500/30'
                  : 'bg-gray-800 text-gray-400 border-gray-700 hover:bg-gray-700'
              }`}
            >
              {tr.label}
            </button>
          ))}
        </div>

        {/* Pause/Resume (live only) */}
        {isLive && (
          <>
            <div className="w-px h-5 bg-gray-700" />
            <button
              onClick={() => paused ? resume() : setPaused(true)}
              className={`flex items-center gap-1.5 px-2.5 py-1 text-xs rounded-lg transition-colors border ${
                paused
                  ? 'bg-yellow-400/10 text-yellow-400 border-yellow-400/20'
                  : 'bg-gray-800 text-gray-400 border-gray-700 hover:bg-gray-700'
              }`}
            >
              {paused ? <Play className="w-3 h-3" /> : <Pause className="w-3 h-3" />}
              {paused ? 'Reprendre' : 'Pause'}
            </button>
          </>
        )}
      </div>

      {/* New entries badge */}
      {paused && newCount > 0 && (
        <div className="px-4 sm:px-6 py-1.5 bg-blue-500/10 border-b border-blue-500/20">
          <button onClick={resume} className="text-xs text-blue-400 hover:text-blue-300">
            {newCount} nouvelle{newCount > 1 ? 's' : ''} entree{newCount > 1 ? 's' : ''} — cliquer pour reprendre
          </button>
        </div>
      )}

      {/* Log entries */}
      <div
        ref={scrollRef}
        className="flex-1 overflow-y-auto font-mono text-xs"
      >
        {filteredLogs.length === 0 ? (
          <div className="p-8 text-center text-gray-500 text-sm">
            {isLive ? 'En attente de logs...' : loadingHistory ? 'Chargement...' : 'Aucun log trouve'}
          </div>
        ) : (
          <div className="divide-y divide-gray-800/50">
            {filteredLogs.map((log, i) => {
              const cfg = levelCfg(log.level);
              const id = log._id || log.id || i;
              const isExpanded = expandedId === id;

              return (
                <div key={id}>
                  <button
                    onClick={() => setExpandedId(isExpanded ? null : id)}
                    className={`w-full text-left px-4 py-1.5 flex items-start gap-2 hover:bg-gray-700/30 transition-colors ${cfg.rowBg}`}
                  >
                    <ChevronRight className={`w-3 h-3 mt-0.5 text-gray-600 flex-shrink-0 transition-transform ${isExpanded ? 'rotate-90' : ''}`} />
                    <span className="text-gray-500 flex-shrink-0 w-[85px]">{formatTimestamp(log.timestamp)}</span>
                    <span className={`inline-flex items-center px-1.5 py-0 rounded text-[10px] font-medium flex-shrink-0 w-[46px] justify-center ${cfg.bg} ${cfg.color}`}>
                      {(log.level || 'INFO').toUpperCase()}
                    </span>
                    <span className="inline-flex items-center px-1.5 py-0 rounded text-[10px] text-gray-400 bg-gray-700/50 flex-shrink-0">
                      {log.service || '-'}
                    </span>
                    {log.source && (
                      <span className="text-gray-500 flex-shrink-0 truncate max-w-[250px]">
                        {typeof log.source === 'object'
                          ? [log.source.crate_name, log.source.module?.split('::').slice(1).join('::'), log.source.function].filter(Boolean).join(' > ')
                          : log.source}
                      </span>
                    )}
                    <span className="text-gray-200 truncate">{log.message}</span>
                  </button>

                  {isExpanded && (
                    <div className="px-4 py-2 ml-8 bg-gray-800/50 border-l-2 border-gray-700 text-[11px] space-y-1">
                      {log.request_id && (
                        <div><span className="text-gray-500">request_id:</span> <span className="text-gray-300">{log.request_id}</span></div>
                      )}
                      {log.user_id && (
                        <div><span className="text-gray-500">user_id:</span> <span className="text-gray-300">{log.user_id}</span></div>
                      )}
                      {(log.source?.file || log.file) && (
                        <div><span className="text-gray-500">fichier:</span> <span className="text-gray-300">{log.source?.file || log.file}{(log.source?.line || log.line) ? `:${log.source?.line || log.line}` : ''}</span></div>
                      )}
                      {log.data && (
                        <div>
                          <span className="text-gray-500">data:</span>
                          <pre className="mt-1 text-gray-300 whitespace-pre-wrap break-all bg-gray-900/50 rounded p-2">
                            {typeof log.data === 'string' ? log.data : JSON.stringify(log.data, null, 2)}
                          </pre>
                        </div>
                      )}
                      {!log.request_id && !log.user_id && !log.file && !log.data && (
                        <div className="text-gray-500 italic">Pas de details supplementaires</div>
                      )}
                    </div>
                  )}
                </div>
              );
            })}
          </div>
        )}

        {/* Load more for history */}
        {!isLive && historyLogs.length > 0 && historyLogs.length < historyTotal && (
          <div className="p-4 text-center">
            <button
              onClick={loadMore}
              disabled={loadingHistory}
              className="px-4 py-2 text-xs bg-gray-800 text-gray-400 border border-gray-700 rounded-lg hover:bg-gray-700 disabled:opacity-40"
            >
              {loadingHistory ? 'Chargement...' : `Charger plus (${historyLogs.length}/${historyTotal})`}
            </button>
          </div>
        )}
      </div>
    </div>
  );
}
