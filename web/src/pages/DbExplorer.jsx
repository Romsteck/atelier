import { useEffect, useState, useCallback, useRef } from 'react';
import { useSearchParams } from 'react-router-dom';
import {
  listApps,
  getAppDbTables,
  getAppDbTable,
  queryAppDbRows,
  executeAppDb,
} from '../api/client';
import { TableSidebar } from '../components/db/TableSidebar';
import { DataGrid } from '../components/db/DataGrid';
import { Pagination } from '../components/db/Pagination';
import { AddRowModal } from '../components/db/AddRowModal';
import { DeleteConfirmModal } from '../components/db/DeleteConfirmModal';
import { Download, Plus, Trash2, RefreshCw, Database, Loader2 } from 'lucide-react';
import PageHeader from '../components/PageHeader';

function unwrap(res) {
  const d = res.data;
  return (d && typeof d === 'object' && 'data' in d) ? d.data : d;
}

export default function DbExplorer({ appSlug: propAppSlug, embedded }) {
  const [searchParams, setSearchParams] = useSearchParams();

  const selectedAppSlug = propAppSlug || searchParams.get('app') || null;
  const selectedTable = searchParams.get('table') || null;

  // Data
  const [appsWithTables, setAppsWithTables] = useState([]);
  const [schema, setSchema] = useState(null);
  const [result, setResult] = useState(null);

  // UI
  const [sidebarLoading, setSidebarLoading] = useState(true);
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

  // Selection
  const [selectedRows, setSelectedRows] = useState(new Set());

  // Inline editing
  const [editingCell, setEditingCell] = useState(null);
  const [editValue, setEditValue] = useState('');
  const [savingCell, setSavingCell] = useState(null);

  // Modals
  const [showAddRow, setShowAddRow] = useState(false);
  const [showDeleteConfirm, setShowDeleteConfirm] = useState(false);

  // Toast
  const [toast, setToast] = useState(null);

  function showToast(msg, type = 'ok') {
    setToast({ msg, type });
    setTimeout(() => setToast(null), 3000);
  }

  // ── Load sidebar ──
  useEffect(() => {
    setSidebarLoading(true);
    setError(null);

    const loadApps = async () => {
      try {
        const res = await listApps();
        const apps = unwrap(res)?.apps || unwrap(res) || [];
        const dbApps = (Array.isArray(apps) ? apps : []).filter(a => a.has_db);

        const results = await Promise.all(
          dbApps.map(async (app) => {
            try {
              const tablesRes = await getAppDbTables(app.slug);
              const raw = unwrap(tablesRes);
              const tables = raw?.tables || (Array.isArray(raw) ? raw : []);
              return { app, tables };
            } catch {
              return { app, tables: [] };
            }
          })
        );
        setAppsWithTables(results);

        if (propAppSlug && !selectedTable) {
          const appData = results.find(r => r.app.slug === propAppSlug);
          if (appData && appData.tables.length > 0) {
            const name = typeof appData.tables[0] === 'string' ? appData.tables[0] : appData.tables[0].name;
            setSearchParams({ app: propAppSlug, table: name }, { replace: true });
          }
        }
      } catch (e) {
        setError(e.message);
      } finally {
        setSidebarLoading(false);
      }
    };
    loadApps();
  }, [propAppSlug]); // eslint-disable-line

  // ── Load table data ──
  const loadTableData = useCallback(async () => {
    const appSlug = selectedAppSlug;
    if (!appSlug || !selectedTable) {
      setSchema(null);
      setResult(null);
      return;
    }

    setTableLoading(true);
    setError(null);

    try {
      // Fetch schema first to know relations for expand
      const schemaRes = await getAppDbTable(appSlug, selectedTable);
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

      // Use the new structured query endpoint
      const queryRes = await queryAppDbRows(appSlug, selectedTable, {
        filters: apiFilters,
        limit: pageSize,
        offset: currentPage * pageSize,
        order_by: sortColumn || undefined,
        order_desc: sortDesc,
        expand,
      });
      const queryData = unwrap(queryRes);

      // Also get total count (without pagination)
      const countRes = await queryAppDbRows(appSlug, selectedTable, {
        filters: apiFilters,
        limit: 1,
        offset: 0,
      });
      const countTotal = unwrap(countRes)?.total || queryData?.total || 0;

      setResult({
        columns: queryData?.columns || [],
        rows: queryData?.rows || [],
        total_count: countTotal,
      });
    } catch (e) {
      setError(e.message);
    } finally {
      setTableLoading(false);
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

  // ── Inline edit ──
  function handleStartEdit(row, col, value) {
    if (isPgBackend) {
      showToast('Édition désactivée sur postgres-dataverse — utilise db.graphql', 'err');
      return;
    }
    const colSchema = schema?.columns?.find(c => c.name === col);
    if (colSchema?.primary_key) return;
    setEditingCell({ row, col });
    setEditValue(value == null ? '' : String(value));
  }

  async function handleCommitEdit() {
    if (!editingCell || !result || !selectedAppSlug || !selectedTable) return;

    const row = result.rows[editingCell.row];
    const original = row[editingCell.col];
    if (String(original ?? '') === editValue) { setEditingCell(null); return; }

    const pkCol = schema?.columns?.find(c => c.primary_key);
    if (!pkCol) { setEditingCell(null); return; }

    const pkValue = row[pkCol.name];
    setSavingCell(editingCell);
    setEditingCell(null);

    try {
      const newVal = editValue === '' ? 'NULL' : `'${editValue.replace(/'/g, "''")}'`;
      const sql = `UPDATE "${selectedTable}" SET "${editingCell.col}" = ${newVal} WHERE "${pkCol.name}" = '${String(pkValue).replace(/'/g, "''")}'`;
      await executeAppDb(selectedAppSlug, sql);
      await loadTableData();
      showToast('Cellule mise a jour');
    } catch (e) {
      setError(e.message);
    } finally {
      setSavingCell(null);
    }
  }

  // ── Insert row ──
  async function handleInsertRow(rowData) {
    if (!selectedAppSlug || !selectedTable) return;
    const cols = Object.keys(rowData);
    const vals = cols.map(c => {
      const v = rowData[c];
      return v == null ? 'NULL' : `'${String(v).replace(/'/g, "''")}'`;
    });
    const sql = `INSERT INTO "${selectedTable}" (${cols.map(c => `"${c}"`).join(', ')}) VALUES (${vals.join(', ')})`;
    await executeAppDb(selectedAppSlug, sql);
    await loadTableData();
    showToast('Ligne ajoutee');
  }

  // ── Delete rows ──
  async function handleDeleteSelected() {
    if (!selectedAppSlug || !selectedTable || !result || !schema) return;
    const pkCol = schema.columns?.find(c => c.primary_key);
    if (!pkCol) throw new Error('No PK');

    const pkValues = Array.from(selectedRows).map(idx => result.rows[idx][pkCol.name]);
    for (const pk of pkValues) {
      const sql = `DELETE FROM "${selectedTable}" WHERE "${pkCol.name}" = '${String(pk).replace(/'/g, "''")}'`;
      await executeAppDb(selectedAppSlug, sql);
    }
    setSelectedRows(new Set());
    await loadTableData();
    showToast(`${pkValues.length} ligne(s) supprimee(s)`);
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
    setSearchParams({ app: appSlug, table: tableName });
    setCurrentPage(0);
    setSortColumn(null);
    setSortDesc(false);
    setFilters([]);
    setSearchQuery('');
    setSelectedRows(new Set());
    setEditingCell(null);
  }

  const totalCount = result?.total_count || 0;

  // Backend awareness: block SQL-brut writes when the app is on the
  // postgres-dataverse backend. Reads still go through the IPC layer
  // which routes correctly per backend; writes need a GraphQL UI that
  // hasn't shipped yet (see Phase F.2 in the dataverse plan).
  const selectedApp = appsWithTables.find(a => a.app.slug === selectedAppSlug)?.app;
  const dbBackend = selectedApp?.db_backend || 'legacy-sqlite';
  const isPgBackend = dbBackend === 'postgres-dataverse';
  const writesDisabledReason = isPgBackend
    ? 'Désactivé sur postgres-dataverse. Utilise l\'endpoint GraphQL /api/apps/{slug}/db/graphql ou les tools MCP db.graphql / db.insert / db.update / db.delete.'
    : null;

  return (
    <>
      {!embedded && <PageHeader title="Bases de données" icon={Database} />}
      <div className={`flex h-full overflow-hidden ${embedded ? '' : 'rounded border border-gray-700'}`}>
      {/* Sidebar */}
      <TableSidebar
        appsWithTables={propAppSlug ? appsWithTables.filter(a => a.app.slug === propAppSlug) : appsWithTables}
        selectedAppSlug={selectedAppSlug}
        selectedTable={selectedTable}
        onSelectTable={handleSelectTable}
        loading={sidebarLoading}
      />

      {/* Main */}
      <div className="flex flex-col flex-1 min-w-0">
        {/* Toolbar */}
        <div className="flex items-center gap-2 px-4 py-2 border-b border-gray-700 shrink-0 bg-gray-800/50">
          <div className="flex items-center gap-2 flex-1">
            {selectedTable ? (
              <>
                <Database className="w-4 h-4 text-blue-400" />
                <span className="text-sm font-medium text-white">
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
              className={`text-[10px] uppercase tracking-wide px-1.5 py-0.5 rounded ${
                isPgBackend
                  ? 'bg-emerald-500/15 text-emerald-300 border border-emerald-500/30'
                  : 'bg-amber-500/10 text-amber-300 border border-amber-500/20'
              }`}
              title={
                isPgBackend
                  ? 'Postgres + GraphQL (managé via hr-dataverse)'
                  : 'SQLite legacy (hr-db) — migration vers postgres-dataverse à venir'
              }
            >
              {isPgBackend ? 'pg+graphql' : 'sqlite'}
            </span>
          )}

          {selectedTable && (
            <div className="flex items-center gap-1">
              <input
                type="text"
                value={searchQuery}
                onChange={e => handleSearchChange(e.target.value)}
                placeholder="Rechercher..."
                className="bg-gray-900 text-white text-xs rounded px-2 py-1 border border-gray-600 w-40 outline-none"
              />
              <button
                onClick={() => setShowAddRow(true)}
                disabled={isPgBackend}
                className="p-1.5 text-gray-400 hover:text-green-400 hover:bg-gray-700 rounded border-none bg-transparent cursor-pointer disabled:opacity-30 disabled:cursor-not-allowed"
                title={writesDisabledReason || 'Ajouter'}
              >
                <Plus className="w-3.5 h-3.5" />
              </button>
              {selectedRows.size > 0 && (
                <button
                  onClick={() => setShowDeleteConfirm(true)}
                  disabled={isPgBackend}
                  className="p-1.5 text-gray-400 hover:text-red-400 hover:bg-gray-700 rounded border-none bg-transparent cursor-pointer disabled:opacity-30 disabled:cursor-not-allowed"
                  title={writesDisabledReason || 'Supprimer'}
                >
                  <Trash2 className="w-3.5 h-3.5" />
                </button>
              )}
              <button onClick={handleExportCSV} disabled={!result?.rows?.length} className="p-1.5 text-gray-400 hover:text-white hover:bg-gray-700 rounded border-none bg-transparent cursor-pointer disabled:opacity-30" title="Exporter CSV">
                <Download className="w-3.5 h-3.5" />
              </button>
              <button onClick={loadTableData} className="p-1.5 text-gray-400 hover:text-white hover:bg-gray-700 rounded border-none bg-transparent cursor-pointer" title="Actualiser">
                <RefreshCw className="w-3.5 h-3.5" />
              </button>
            </div>
          )}
        </div>

        {/* Postgres-dataverse banner: explain that writes go through GraphQL */}
        {isPgBackend && selectedTable && (
          <div className="px-4 py-1.5 text-[11px] bg-emerald-500/5 text-emerald-200 border-b border-emerald-500/20 shrink-0">
            Cette app est sur le backend <strong>postgres-dataverse</strong>. Les
            écritures via SQL brut sont désactivées — utilise le tool MCP{' '}
            <code className="bg-black/40 px-1 rounded">db.graphql</code> ou
            l'endpoint <code className="bg-black/40 px-1 rounded">/api/apps/{selectedAppSlug}/db/graphql</code>.
          </div>
        )}

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
              editingCell={editingCell}
              editValue={editValue}
              savingCell={savingCell}
              onStartEdit={handleStartEdit}
              onEditValueChange={setEditValue}
              onCommitEdit={handleCommitEdit}
              onCancelEdit={() => setEditingCell(null)}
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
            onPageChange={(p) => { setCurrentPage(p); setSelectedRows(new Set()); setEditingCell(null); }}
            onPageSizeChange={(s) => { setPageSize(s); setCurrentPage(0); setSelectedRows(new Set()); }}
          />
        )}
      </div>

      {/* Toast */}
      {toast && (
        <div className={`fixed bottom-4 right-4 z-50 px-4 py-2 rounded-lg text-sm shadow-lg ${
          toast.type === 'ok' ? 'bg-green-500/90 text-white' : 'bg-red-500/90 text-white'
        }`}>
          {toast.msg}
        </div>
      )}

      {/* Modals */}
      {showAddRow && schema && (
        <AddRowModal
          columns={schema.columns || []}
          relations={schema.relations || []}
          appSlug={selectedAppSlug}
          onInsert={handleInsertRow}
          onClose={() => setShowAddRow(false)}
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
