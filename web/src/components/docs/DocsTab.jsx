import { useEffect, useState, useCallback } from 'react';
import { Loader2, AlertCircle, BookOpen, RefreshCw } from 'lucide-react';
import { getDocsOverview, getDocsEntry, getDocsDiagram } from '../../api/client';
import DocsSidebar from './DocsSidebar';
import DocsSearch from './DocsSearch';
import DocsOverview from './DocsOverview';
import DocsEntryView from './DocsEntryView';

export default function DocsTab({ slug }) {
  const [overview, setOverview] = useState(null);
  const [overviewDiagram, setOverviewDiagram] = useState(null);
  const [overviewError, setOverviewError] = useState(null);
  const [overviewLoading, setOverviewLoading] = useState(true);

  // Selected entry: { type, name } | null (null = overview)
  const [selected, setSelected] = useState({ type: 'overview', name: 'overview' });
  const [entry, setEntry] = useState(null);
  const [entryLoading, setEntryLoading] = useState(false);
  const [entryError, setEntryError] = useState(null);

  const loadOverview = useCallback(async () => {
    if (!slug) return;
    setOverviewLoading(true);
    setOverviewError(null);
    try {
      const res = await getDocsOverview(slug);
      const data = res.data?.data || null;
      setOverview(data);
      // If the overview has a diagram attached, fetch it.
      if (data?.overview?.frontmatter?.diagram) {
        try {
          const diagRes = await getDocsDiagram(slug, 'overview', 'overview');
          setOverviewDiagram(diagRes.data?.mermaid || null);
        } catch {
          setOverviewDiagram(null);
        }
      } else {
        setOverviewDiagram(null);
      }
    } catch (e) {
      const status = e?.response?.status;
      if (status === 404) {
        setOverview(null);
        setOverviewError('not-found');
      } else {
        setOverviewError(e?.message || 'Erreur de chargement');
      }
    } finally {
      setOverviewLoading(false);
    }
  }, [slug]);

  useEffect(() => {
    loadOverview();
  }, [loadOverview]);

  // Reset selection when slug changes.
  useEffect(() => {
    setSelected({ type: 'overview', name: 'overview' });
  }, [slug]);

  // Load the selected entry (other than overview, which is included in `overview`).
  useEffect(() => {
    if (!slug || !selected) return;
    if (selected.type === 'overview') {
      // The overview is already loaded in `overview`.
      setEntry(null);
      return;
    }
    let cancelled = false;
    setEntryLoading(true);
    setEntryError(null);
    (async () => {
      try {
        const res = await getDocsEntry(slug, selected.type, selected.name);
        if (cancelled) return;
        setEntry(res.data?.data || null);
      } catch (e) {
        if (cancelled) return;
        setEntryError(
          e?.response?.status === 404
            ? `Entrée introuvable : ${selected.type}/${selected.name}`
            : (e?.message || 'Erreur de chargement')
        );
        setEntry(null);
      } finally {
        if (!cancelled) setEntryLoading(false);
      }
    })();
    return () => { cancelled = true; };
  }, [slug, selected]);

  const handleSelect = useCallback((sel) => {
    setSelected(sel);
  }, []);

  // ── Render ────────────────────────────────────────────────────────

  if (!slug) {
    return (
      <div className="h-full flex items-center justify-center text-gray-400 p-6">
        Sélectionne une app pour voir sa documentation.
      </div>
    );
  }

  if (overviewLoading) {
    return (
      <div className="h-full flex items-center justify-center">
        <Loader2 className="w-6 h-6 animate-spin text-blue-400" />
      </div>
    );
  }

  if (overviewError === 'not-found' || !overview) {
    return (
      <div className="h-full flex flex-col items-center justify-center text-center p-8 gap-3">
        <BookOpen className="w-12 h-12 text-gray-600 opacity-40" />
        <div className="text-gray-300 font-medium">
          Pas encore de documentation pour cette app.
        </div>
        <div className="text-sm text-gray-500 max-w-md">
          La documentation est créée et maintenue par l'agent. Demande-lui de l'initialiser
          ou attends la prochaine itération sur l'app.
        </div>
        <button
          onClick={loadOverview}
          className="mt-2 inline-flex items-center gap-1.5 text-sm text-blue-400 hover:text-blue-300"
        >
          <RefreshCw className="w-3.5 h-3.5" /> Actualiser
        </button>
      </div>
    );
  }

  if (overviewError) {
    return (
      <div className="h-full flex items-center justify-center p-6">
        <div className="max-w-md p-4 bg-red-500/10 border border-red-500/30 rounded text-red-300 flex items-start gap-2">
          <AlertCircle className="w-5 h-5 flex-shrink-0 mt-0.5" />
          <div>
            <div className="font-medium">Erreur</div>
            <div className="text-sm mt-1">{overviewError}</div>
            <button
              onClick={loadOverview}
              className="mt-2 inline-flex items-center gap-1 text-sm text-blue-400 hover:text-blue-300"
            >
              <RefreshCw className="w-3 h-3" /> Réessayer
            </button>
          </div>
        </div>
      </div>
    );
  }

  return (
    <div className="h-full flex flex-col bg-gray-950">
      {/* Header with search */}
      <div className="border-b border-gray-800 bg-gray-900/60 p-3">
        <DocsSearch appId={slug} onPick={handleSelect} />
      </div>

      <div className="flex-1 overflow-hidden flex">
        <DocsSidebar
          overview={overview}
          selected={selected}
          onSelect={handleSelect}
        />

        <div className="flex-1 overflow-y-auto p-6">
          {selected.type === 'overview' ? (
            <DocsOverview overview={overview} diagram={overviewDiagram} />
          ) : entryLoading ? (
            <div className="flex items-center justify-center py-10">
              <Loader2 className="w-5 h-5 animate-spin text-blue-400" />
            </div>
          ) : entryError ? (
            <div className="p-4 bg-red-500/10 border border-red-500/30 rounded text-red-300">
              {entryError}
            </div>
          ) : entry ? (
            <DocsEntryView entry={entry} onPickLink={handleSelect} />
          ) : null}
        </div>
      </div>
    </div>
  );
}
