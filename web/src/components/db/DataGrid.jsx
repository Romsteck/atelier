import { ArrowUp, ArrowDown, Loader2, FunctionSquare, Link2 } from 'lucide-react';
import { FilterDropdown } from './FilterDropdown';
import { getFieldConfig } from './fieldTypes';

function CellValue({ value, fieldType, displayValue }) {
  if (value == null) return <span className="italic text-gray-600">null</span>;

  const cfg = getFieldConfig(fieldType);

  // Formula badge
  if (cfg.isFormula) {
    return (
      <span className="flex items-center gap-1">
        <FunctionSquare className="w-3 h-3 text-purple-400 shrink-0" />
        <span className="text-purple-700 dark:text-purple-300">{String(value)}</span>
      </span>
    );
  }

  // Boolean badge
  if (fieldType === 'Boolean') {
    const isTrue = value === 1 || value === true || value === 'true';
    return (
      <span className={`px-1.5 py-0.5 rounded-sm text-[10px] font-medium ${
        isTrue ? 'bg-green-500/20 text-green-400' : 'bg-red-500/20 text-red-400'
      }`}>
        {isTrue ? 'Vrai' : 'Faux'}
      </span>
    );
  }

  // Currency
  if (cfg.formatCell) {
    const formatted = cfg.formatCell(value);
    if (formatted != null) return <span className="tabular-nums">{formatted}</span>;
  }

  // Link types
  if (cfg.isLink === 'mailto') {
    return <a href={`mailto:${value}`} className="text-blue-400 hover:underline" onClick={e => e.stopPropagation()}>{String(value)}</a>;
  }
  if (cfg.isLink === 'href') {
    return <a href={String(value)} target="_blank" rel="noopener noreferrer" className="text-blue-400 hover:underline" onClick={e => e.stopPropagation()}>{String(value)}</a>;
  }
  if (cfg.isLink === 'tel') {
    return <a href={`tel:${value}`} className="text-blue-400 hover:underline" onClick={e => e.stopPropagation()}>{String(value)}</a>;
  }

  // Lookup with display value
  if (fieldType === 'Lookup' && displayValue != null) {
    return (
      <span className="flex items-center gap-1">
        <Link2 className="w-3 h-3 text-gray-500 shrink-0" />
        <span>{String(displayValue)}</span>
        <span className="text-gray-600 text-[10px]">#{value}</span>
      </span>
    );
  }

  // Number alignment
  if (cfg.align === 'right') {
    return <span className="tabular-nums">{String(value)}</span>;
  }

  // Monospace
  if (cfg.mono) {
    return <span className="font-mono text-[11px]">{String(value)}</span>;
  }

  return String(value);
}

// Skeleton non-effondrant : occupe la zone à hauteur stable pendant le premier
// chargement d'une table (évite le spinner centré qui fait « disparaître » la grille).
function SkeletonGrid() {
  return (
    <div className="h-full overflow-hidden p-3">
      <div className="animate-pulse space-y-2">
        <div className="h-7 bg-gray-800 rounded-sm" />
        {Array.from({ length: 10 }).map((_, i) => (
          <div key={i} className="h-6 bg-gray-800/50 rounded-sm" />
        ))}
      </div>
    </div>
  );
}

export function DataGrid({
  columns,
  rows,
  schema,
  sortColumn,
  sortDesc,
  onSort,
  filters,
  onFilterChange,
  selectedRows,
  onSelectRow,
  onSelectAll,
  onRowClick,
  loading,
}) {
  const hasData = columns && columns.length > 0;

  // Table neuve en cours de chargement (pas encore de colonnes) → skeleton stable.
  if (loading && !hasData) {
    return <SkeletonGrid />;
  }

  if (!hasData) {
    return <div className="flex items-center justify-center h-full text-gray-500 text-sm">Aucune donnee</div>;
  }

  const schemaMap = {};
  if (schema?.columns) {
    schema.columns.forEach(c => { schemaMap[c.name] = c; });
  }

  const allSelected = rows.length > 0 && selectedRows.size === rows.length;

  // Hide _display columns from the grid (they're used by Lookup rendering)
  const visibleColumns = columns.filter(col => !col.endsWith('_display'));

  return (
    <div className="relative h-full">
      {/* Rechargement même table : overlay discret, on garde les lignes (stale-while-revalidate). */}
      {loading && (
        <div className="absolute top-2 left-1/2 -translate-x-1/2 z-20 flex items-center gap-1.5 px-2.5 py-1 rounded-full bg-gray-900/90 border border-gray-700 text-[11px] text-gray-300 shadow-lg">
          <Loader2 className="w-3 h-3 animate-spin text-blue-400" /> Chargement...
        </div>
      )}
      <div className={`overflow-auto h-full transition-opacity ${loading ? 'opacity-50 pointer-events-none' : ''}`}>
        <table className="w-full min-w-max text-sm border-collapse">
          <thead className="sticky top-0 z-10">
            <tr className="bg-gray-800">
              <th className="w-10 px-3 py-2 border-b border-gray-700">
                <input
                  type="checkbox"
                  checked={allSelected}
                  onChange={e => onSelectAll(e.target.checked)}
                  className="cursor-pointer"
                />
              </th>
              {visibleColumns.map(col => {
                const currentFilter = filters.find(f => f.column === col);
                const isSorted = sortColumn === col;
                const colSchema = schemaMap[col];
                const fieldType = colSchema?.field_type;
                return (
                  <th key={col} className="px-3 py-2 text-left text-xs font-semibold text-gray-400 border-b border-gray-700 whitespace-nowrap">
                    <div className="flex items-center gap-1">
                      <button
                        onClick={() => onSort(col)}
                        className="flex items-center gap-1 border-none bg-transparent text-gray-400 hover:text-gray-50 cursor-pointer text-xs font-semibold"
                      >
                        {col}
                        {isSorted && (sortDesc ? <ArrowDown className="w-3 h-3 text-blue-400" /> : <ArrowUp className="w-3 h-3 text-blue-400" />)}
                      </button>
                      {colSchema?.primary_key && <span className="text-[9px] text-yellow-400 font-bold">PK</span>}
                      {fieldType === 'Formula' && <FunctionSquare className="w-3 h-3 text-purple-400" />}
                      {fieldType === 'Lookup' && <Link2 className="w-3 h-3 text-gray-500" />}
                      {colSchema?.required && !colSchema?.primary_key && <span className="text-red-400 text-[9px]">*</span>}
                      <FilterDropdown column={col} fieldType={fieldType} choices={colSchema?.choices} currentFilter={currentFilter} onFilterChange={onFilterChange} />
                    </div>
                  </th>
                );
              })}
            </tr>
          </thead>
          <tbody>
            {rows.map((row, rowIdx) => {
              const isSelected = selectedRows.has(rowIdx);
              return (
                <tr
                  key={rowIdx}
                  onClick={() => onRowClick?.(rowIdx)}
                  className={`cursor-pointer ${isSelected ? 'bg-blue-500/10' : rowIdx % 2 ? 'bg-gray-800/30' : ''} hover:bg-gray-700/30`}
                >
                  <td className="px-3 py-1.5 border-b border-gray-700/50" onClick={e => e.stopPropagation()}>
                    <input
                      type="checkbox"
                      checked={isSelected}
                      onChange={e => onSelectRow(rowIdx, e.target.checked)}
                      className="cursor-pointer"
                    />
                  </td>
                  {visibleColumns.map(col => {
                    const value = row[col];
                    const colSchema = schemaMap[col];
                    const fieldType = colSchema?.field_type || 'Text';
                    const cfg = getFieldConfig(fieldType);
                    const displayValue = row[col + '_display'];

                    return (
                      <td
                        key={col}
                        className={`px-3 py-1.5 border-b border-gray-700/50 text-xs max-w-[300px] truncate text-gray-300 ${
                          cfg.align === 'right' ? 'text-right' : ''
                        } ${cfg.mono ? 'font-mono' : ''}`}
                        title={value == null ? 'null' : String(value)}
                      >
                        <CellValue value={value} fieldType={fieldType} displayValue={displayValue} />
                      </td>
                    );
                  })}
                </tr>
              );
            })}
            {rows.length === 0 && !loading && (
              <tr>
                <td colSpan={visibleColumns.length + 1} className="text-center py-8 text-gray-500 text-xs">
                  Aucune ligne
                </td>
              </tr>
            )}
          </tbody>
        </table>
      </div>
    </div>
  );
}
