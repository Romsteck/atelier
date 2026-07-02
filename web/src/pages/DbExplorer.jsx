import { useEffect, useState, useCallback, useRef } from 'react';
import { useSearchParams } from 'react-router-dom';
import {
  listApps,
  getAppDbTables,
  getAppDbTable,
  queryAppDbRows,
  insertAppDbRow,
  updateAppDbRow,
  deleteAppDbRow,
  unwrapApi as unwrap,
} from '../api/client';
import { TableSidebar } from '../components/db/TableSidebar';
import { DataGrid } from '../components/db/DataGrid';
import { Pagination } from '../components/db/Pagination';
import { RowFormModal } from '../components/db/RowFormModal';
import { DeleteConfirmModal } from '../components/db/DeleteConfirmModal';
import { AppPicker } from '../components/db/AppPicker';
import { Download, Plus, Trash2, RefreshCw, Database } from 'lucide-react';
import PageHeader from '../components/PageHeader';
import { useToast, Toast } from '../hooks/useToast';

export default function DbExplorer({ appSlug: propAppSlug, embedded }) {
  const [searchParams, setSearchParams] = useSearchParams();

  const selectedAppSlug = propAppSlug || searchParams.get('app') || null;
  const selectedTable = searchParams.get('table') || null;

  // Data
  const [apps, setApps] = useState([]);            // apps avec has_db (objets)
  const [pickerCounts, setPickerCounts] = useState({}); // slug -> nb tables (picker standalone)
  const [appTables, setAppTables] = useState([]);  // tables de l'app sélectionnée (+ row_count)
  const [schema, setSchema] = useState(null);
  const [result, setResult] = useState(null);

  // UI
  const [appsLoading, setAppsLoading] = useState(true);
  const [tablesLoading, setTablesLoading] = useState(false);
  const [tableLoading, setTableLoading] = useState(false);
  const [error, setError] = useState(null);

  // Pagination
  const [pageSize, setPageSize] = useState(50);
  const [currentPage, setCurrentPage] = useState(0);

  // Sort
  const [sortColumn, setSortColumn] = useState(null);
  const [sortDesc, setSortDesc] = useState(false);

  // Filters
  const [filters, setFilters] = useState([]);
  const [searchQuery, setSearchQuery] = useState('');
  const searchTimeout = useRef(null);

  // Génération de chargement : invalide les réponses en vol quand on change de
  // table (sinon une requête lente de la table précédente résout APRÈS et écrase
  // les données de la nouvelle table — l'ancienne « reste en mémoire »).
  const loadSeq = useRef(0);

  // Selection
  const [selectedRows, setSelectedRows] = useState(new Set());

  // Modals
  const [showAddRow, setShowAddRow] = useState(false);
  const [editingRow, setEditingRow] = useState(null);
  const [showDeleteConfirm, setShowDeleteConfirm] = useState(false);

  // Tiroir « Tables » (mobile <lg) : la sidebar 224px déborderait à 375px.
  const [tablesOpen, setTablesOpen] = useState(false);

  const { toast, showToast } = useToast(3000);

  // ── Load apps (résolution des noms + picker standalone) ──
  useEffect(() => {
    let cancelled = false;
    setAppsLoading(true);
    setError(null);

    (async () => {
      try {
        const res = await listApps();
        const all = unwrap(res)?.apps || unwrap(res) || [];
        const dbApps = (Array.isArray(all) ? all : []).filter(a => a.has_db);
        if (cancelled) return;
        setApps(dbApps);

        // Le picker (standalone) affiche le nombre de tables par app. On ne charge
        // ces décomptes que là — l'embarqué Studio est déjà scopé à une app.
        if (!propAppSlug) {
          const counts = {};
          await Promise.all(dbApps.map(async (app) => {
            try {
              const r = await getAppDbTables(app.slug);
              const raw = unwrap(r);
              const tables = raw?.tables || (Array.isArray(raw) ? raw : []);
              counts[app.slug] = tables.length;
            } catch {
              counts[app.slug] = null;
            }
          }));
          if (!cancelled) setPickerCounts(counts);
        }
      } catch (e) {
        if (!cancelled) setError(e.message);
      } finally {
        if (!cancelled) setAppsLoading(false);
      }
    })();

    return () => { cancelled = true; };
  }, [propAppSlug]);

  // ── Load les tables de l'app sélectionnée (avec row_count) pour le sidebar ──
  useEffect(() => {
    if (!selectedAppSlug) { setAppTables([]); return; }
    let cancelled = false;
    setAppTables([]); // évite d'afficher les tables de l'app précédente
    setTablesLoading(true);

    (async () => {
      try {
        const r = await getAppDbTables(selectedAppSlug, { counts: 1 });
        const raw = unwrap(r);
        const tables = raw?.tables || (Array.isArray(raw) ? raw : []);
        if (cancelled) return;
        setAppTables(tables);

        // Embarqué : auto-sélection de la 1ère table si aucune n'est choisie.
        if (propAppSlug && !selectedTable && tables.length > 0) {
          const name = typeof tables[0] === 'string' ? tables[0] : tables[0].name;
          setSearchParams({ app: propAppSlug, table: name }, { replace: true });
        }
      } catch (e) {
        if (!cancelled) setError(e.message);
      } finally {
        if (!cancelled) setTablesLoading(false);
      }
    })();

    return () => { cancelled = true; };
  }, [selectedAppSlug]); // eslint-disable-line

  // ── Load table data ──
  const loadTableData = useCallback(async () => {
    const appSlug = selectedAppSlug;
    if (!appSlug || !selectedTable) {
      setSchema(null);
      setResult(null);
      return;
    }

    // Ce chargement « possède » cette génération ; toute réponse d'un chargement
    // antérieur (table précédente) est ignorée si une nouvelle a démarré entre-temps.
    const seq = ++loadSeq.current;
    const isCurrent = () => seq === loadSeq.current;

    setTableLoading(true);
    setError(null);

    try {
      // Fetch schema first to know relations for expand
      const schemaRes = await getAppDbTable(appSlug, selectedTable);
      if (!isCurrent()) return;
      const schemaData = unwrap(schemaRes);
      setSchema(schemaData);

      // Build expand list from Lookup relations
      const expand = (schemaData?.relations || []).map(r => r.from_column);

      // Build structured filters from UI filters
      const apiFilters = filters.map(f => {
        const opMap = { eq: 'eq', neq: 'ne', gt: 'gt', gte: 'gte', lt: 'lt', lte: 'lte', like: 'like', is_null: 'is_null', not_null: 'is_not_null' };
        return {
          column: f.column,
          op: opMap[f.op] || 'eq',
          value: f.value,
        };
      });

      // Le endpoint structuré renvoie déjà `total` (count filtré, hors soft-delete) :
      // pas de 2ᵉ requête count séparée — un aller-retour de moins par chargement.
      const queryRes = await queryAppDbRows(appSlug, selectedTable, {
        filters: apiFilters,
        limit: pageSize,
        offset: currentPage * pageSize,
        order_by: sortColumn || undefined,
        order_desc: sortDesc,
        expand,
      });
      if (!isCurrent()) return;
      const queryData = unwrap(queryRes);

      setResult({
        columns: queryData?.columns || [],
        rows: queryData?.rows || [],
        total_count: queryData?.total || 0,
      });
    } catch (e) {
      if (isCurrent()) setError(e.message);
    } finally {
      if (isCurrent()) setTableLoading(false);
    }
  }, [selectedAppSlug, selectedTable, pageSize, currentPage, sortColumn, sortDesc, filters]);

  useEffect(() => { loadTableData(); }, [loadTableData]);

  // ── Search (debounced) ──
  function handleSearchChange(value) {
    setSearchQuery(value);
    if (searchTimeout.current) clearTimeout(searchTimeout.current);
    searchTimeout.current = setTimeout(() => {
      if (value.trim() && schema?.columns) {
        const textCol = schema.columns.find(c => !c.primary_key && isTextType(c.field_type));
        if (textCol) {
          setFilters(prev => {
            const without = prev.filter(f => f.op !== 'like' || !f.value?.startsWith?.('%'));
            return [...without, { column: textCol.name, op: 'like', value: `%${value}%` }];
          });
        }
      } else {
        setFilters(prev => prev.filter(f => f.op !== 'like' || !f.value?.startsWith?.('%')));
      }
      setCurrentPage(0);
    }, 400);
  }

  // ── Sort ──
  function handleSort(column) {
    if (sortColumn === column) {
      if (sortDesc) { setSortColumn(null); setSortDesc(false); }
      else setSortDesc(true);
    } else {
      setSortColumn(column);
      setSortDesc(false);
    }
    setCurrentPage(0);
  }

  // ── Filter ──
  function handleFilterChange(column, filter) {
    setFilters(prev => {
      const without = prev.filter(f => f.column !== column);
      return filter ? [...without, filter] : without;
    });
    setCurrentPage(0);
  }

  // ── Selection ──
  function handleSelectRow(idx, checked) {
    setSelectedRows(prev => { const n = new Set(prev); checked ? n.add(idx) : n.delete(idx); return n; });
  }
  function handleSelectAll(checked) {
    setSelectedRows(checked && result ? new Set(result.rows.map((_, i) => i)) : new Set());
  }

  // ── Édition en formulaire ── (un clic sur la ligne ouvre le formulaire pré-rempli)
  function handleRowClick(rowIdx) {
    const row = result?.rows?.[rowIdx];
    if (row) setEditingRow(row);
  }

  async function handleUpdateRow(id, patch) {
    if (!selectedAppSlug || !selectedTable || id == null) return;
    await updateAppDbRow(selectedAppSlug, selectedTable, id, patch);
    await loadTableData();
    showToast('Ligne mise à jour');
  }

  // ── Insert row ── (rowData déjà typé par RowFormModal/coerceValue)
  async function handleInsertRow(rowData) {
    if (!selectedAppSlug || !selectedTable) return;
    await insertAppDbRow(selectedAppSlug, selectedTable, rowData);
    await loadTableData();
    showToast('Ligne ajoutée');
  }

  // ── Delete rows ──
  async function handleDeleteSelected() {
    if (!selectedAppSlug || !selectedTable || !result) return;
    const ids = Array.from(selectedRows)
      .map(idx => result.rows[idx]?.id)
      .filter(v => v != null);
    for (const id of ids) {
      await deleteAppDbRow(selectedAppSlug, selectedTable, id);
    }
    setSelectedRows(new Set());
    await loadTableData();
    showToast(`${ids.length} ligne(s) supprimée(s)`);
  }

  // ── Export CSV ──
  function handleExportCSV() {
    if (!result || result.rows.length === 0) return;
    const visibleCols = result.columns.filter(c => !c.endsWith('_display'));
    const headers = visibleCols.join(',');
    const rows = result.rows.map(row =>
      visibleCols.map(col => {
        const val = row[col];
        if (val == null) return '';
        const str = String(val);
        return str.includes(',') || str.includes('"') || str.includes('\n') ? `"${str.replace(/"/g, '""')}"` : str;
      }).join(',')
    );
    const csv = [headers, ...rows].join('\n');
    const blob = new Blob([csv], { type: 'text/csv' });
    const url = URL.createObjectURL(blob);
    const a = document.createElement('a');
    a.href = url;
    a.download = `${selectedTable || 'export'}.csv`;
    a.click();
    URL.revokeObjectURL(url);
  }

  // ── Select table ──
  function handleSelectTable(appSlug, tableName) {
    // On invalide tout chargement en vol AVANT de re-render : une réponse en
    // retard de la table précédente ne pourra plus écraser la nouvelle.
    loadSeq.current++;
    // On vide schema/result tout de suite : pas de flash de l'ancienne table sous
    // le nouveau nom, et la grille passe en skeleton stable (cf. DataGrid).
    setResult(null);
    setSchema(null);
    setSearchParams({ app: appSlug, table: tableName });
    setCurrentPage(0);
    setSortColumn(null);
    setSortDesc(false);
    setFilters([]);
    setSearchQuery('');
    setSelectedRows(new Set());
  }

  // ── Sélection d'app (picker standalone) ──
  function handleSelectApp(slug) {
    setSearchParams({ app: slug });
  }

  // ── Retour au picker (changer d'app) ──
  function handleChangeApp() {
    loadSeq.current++;
    setResult(null);
    setSchema(null);
    setFilters([]);
    setSearchQuery('');
    setSelectedRows(new Set());
    setSearchParams({});
  }

  const totalCount = result?.total_count || 0;

  // App sélectionnée (objet) pour l'en-tête du sidebar.
  const selectedApp = apps.find(a => a.slug === selectedAppSlug)
    || (selectedAppSlug ? { slug: selectedAppSlug, name: selectedAppSlug } : null);
  const sidebarAppsWithTables = selectedAppSlug && selectedApp
    ? [{ app: selectedApp, tables: appTables }]
    : [];

  // Vue picker : standalone et aucune app choisie.
  const showPicker = !embedded && !selectedAppSlug;

  return (
    <>
      {!embedded && <PageHeader title="Bases de données" icon={Database} />}
      <div className={`flex h-full overflow-hidden relative ${embedded ? '' : 'rounded-sm border border-gray-700'}`}>
        {showPicker ? (
          <AppPicker
            apps={apps.map(a => ({ app: a, tableCount: pickerCounts[a.slug] ?? null }))}
            onSelect={handleSelectApp}
            loading={appsLoading}
          />
        ) : (
          <>
            {/* Overlay tactile du tiroir Tables (<lg) */}
            {tablesOpen && (
              <div className="absolute inset-0 bg-black/50 z-30 lg:hidden" onClick={() => setTablesOpen(false)} />
            )}

            {/* Sidebar (scopé à l'app sélectionnée) */}
            <TableSidebar
              appsWithTables={sidebarAppsWithTables}
              selectedAppSlug={selectedAppSlug}
              selectedTable={selectedTable}
              onSelectTable={(slug, name) => { setTablesOpen(false); handleSelectTable(slug, name); }}
              onChangeApp={!embedded ? handleChangeApp : undefined}
              loading={tablesLoading && appTables.length === 0}
              open={tablesOpen}
            />

            {/* Main */}
            <div className="flex flex-col flex-1 min-w-0">
              {/* Toolbar */}
              <div className="flex items-center gap-2 px-4 py-2 border-b border-gray-700 shrink-0 bg-gray-800/50">
                <button
                  onClick={() => setTablesOpen(true)}
                  className="lg:hidden p-1.5 text-gray-400 hover:text-gray-50 hover:bg-gray-700 rounded-sm shrink-0"
                  title="Tables"
                >
                  <Database className="w-4 h-4" />
                </button>
                <div className="flex items-center gap-2 flex-1">
                  {selectedTable ? (
                    <>
                      <Database className="w-4 h-4 text-blue-400" />
                      <span className="text-sm font-medium text-gray-50">
                        {selectedAppSlug && <span className="text-gray-500">{selectedAppSlug}.</span>}
                        {selectedTable}
                      </span>
                      {totalCount > 0 && <span className="text-xs text-gray-500">({totalCount.toLocaleString()} lignes)</span>}
                    </>
                  ) : (
                    <span className="text-sm text-gray-500">Selectionnez une table</span>
                  )}
                </div>

                {selectedAppSlug && (
                  <span
                    className="text-[10px] uppercase tracking-wide px-1.5 py-0.5 rounded-sm bg-emerald-500/15 text-emerald-700 dark:text-emerald-300 border border-emerald-500/30"
                    title="Postgres géré via atelier-dataverse (passerelle REST, plus d'accès direct)"
                  >
                    dataverse
                  </span>
                )}

                {selectedTable && (
                  <div className="flex items-center gap-1">
                    <input
                      type="text"
                      value={searchQuery}
                      onChange={e => handleSearchChange(e.target.value)}
                      placeholder="Rechercher..."
                      className="bg-gray-900 text-gray-50 text-xs rounded-sm px-2 py-1 border border-gray-600 w-40 outline-hidden"
                    />
                    <button
                      onClick={() => setShowAddRow(true)}
                      className="p-1.5 text-gray-400 hover:text-green-400 hover:bg-gray-700 rounded-sm border-none bg-transparent cursor-pointer"
                      title="Ajouter"
                    >
                      <Plus className="w-3.5 h-3.5" />
                    </button>
                    {selectedRows.size > 0 && (
                      <button
                        onClick={() => setShowDeleteConfirm(true)}
                        className="p-1.5 text-gray-400 hover:text-red-400 hover:bg-gray-700 rounded-sm border-none bg-transparent cursor-pointer"
                        title="Supprimer"
                      >
                        <Trash2 className="w-3.5 h-3.5" />
                      </button>
                    )}
                    <button onClick={handleExportCSV} disabled={!result?.rows?.length} className="p-1.5 text-gray-400 hover:text-gray-50 hover:bg-gray-700 rounded-sm border-none bg-transparent cursor-pointer disabled:opacity-30" title="Exporter CSV">
                      <Download className="w-3.5 h-3.5" />
                    </button>
                    <button onClick={loadTableData} className="p-1.5 text-gray-400 hover:text-gray-50 hover:bg-gray-700 rounded-sm border-none bg-transparent cursor-pointer" title="Actualiser">
                      <RefreshCw className="w-3.5 h-3.5" />
                    </button>
                  </div>
                )}
              </div>

              {/* Error */}
              {error && (
                <div className="px-4 py-2 text-xs bg-red-500/10 text-red-400 border-b border-red-500/20 shrink-0">
                  {error}
                </div>
              )}

              {/* Grid */}
              <div className="flex-1 overflow-hidden">
                {selectedTable ? (
                  <DataGrid
                    columns={result?.columns || []}
                    rows={result?.rows || []}
                    schema={schema}
                    sortColumn={sortColumn}
                    sortDesc={sortDesc}
                    onSort={handleSort}
                    filters={filters}
                    onFilterChange={handleFilterChange}
                    selectedRows={selectedRows}
                    onSelectRow={handleSelectRow}
                    onSelectAll={handleSelectAll}
                    onRowClick={handleRowClick}
                    loading={tableLoading}
                  />
                ) : (
                  <div className="flex flex-col items-center justify-center h-full text-gray-500">
                    <Database className="w-12 h-12 mb-3 opacity-20" />
                    <p className="text-sm">Selectionnez une table{!propAppSlug ? ' dans la barre laterale' : ''}</p>
                  </div>
                )}
              </div>

              {/* Pagination */}
              {selectedTable && totalCount > 0 && (
                <Pagination
                  currentPage={currentPage}
                  pageSize={pageSize}
                  totalCount={totalCount}
                  onPageChange={(p) => { setCurrentPage(p); setSelectedRows(new Set()); }}
                  onPageSizeChange={(s) => { setPageSize(s); setCurrentPage(0); setSelectedRows(new Set()); }}
                />
              )}
            </div>
          </>
        )}

        <Toast toast={toast} />

        {/* Modals */}
        {showAddRow && schema && (
          <RowFormModal
            mode="add"
            columns={schema.columns || []}
            relations={schema.relations || []}
            appSlug={selectedAppSlug}
            onSubmit={handleInsertRow}
            onClose={() => setShowAddRow(false)}
          />
        )}
        {editingRow && schema && (
          <RowFormModal
            mode="edit"
            columns={schema.columns || []}
            relations={schema.relations || []}
            appSlug={selectedAppSlug}
            initialRow={editingRow}
            onSubmit={handleUpdateRow}
            onClose={() => setEditingRow(null)}
          />
        )}
        {showDeleteConfirm && (
          <DeleteConfirmModal count={selectedRows.size} onConfirm={handleDeleteSelected} onClose={() => setShowDeleteConfirm(false)} />
        )}
      </div>
    </>
  );
}

function isTextType(type) {
  if (!type) return false;
  const t = type.toLowerCase();
  return ['text', 'varchar', 'char', 'string', 'email', 'url', 'phone'].some(k => t.includes(k));
}
