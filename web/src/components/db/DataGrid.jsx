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
        <span className="text-purple-300">{String(value)}</span>
      </span>
    );
  }

  // Boolean badge
  if (fieldType === 'Boolean') {
    const isTrue = value === 1 || value === true || value === 'true';
    return (
      <span className={`px-1.5 py-0.5 rounded text-[10px] font-medium ${
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
  editingCell,
  editValue,
  savingCell,
  onStartEdit,
  onEditValueChange,
  onCommitEdit,
  onCancelEdit,
  loading,
}) {
  if (loading) {
    return (
      <div className="flex items-center justify-center h-full text-gray-500 text-sm">
        <Loader2 className="w-5 h-5 animate-spin mr-2" /> Chargement...
      </div>
    );
  }

  if (!columns || columns.length === 0) {
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
    <div className="overflow-auto h-full">
      <table className="w-full text-sm border-collapse">
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
                      className="flex items-center gap-1 border-none bg-transparent text-gray-400 hover:text-white cursor-pointer text-xs font-semibold"
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
              <tr key={rowIdx} className={`${isSelected ? 'bg-blue-500/10' : rowIdx % 2 ? 'bg-gray-800/30' : ''} hover:bg-gray-700/30`}>
                <td className="px-3 py-1.5 border-b border-gray-700/50">
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
                  const isPk = colSchema?.primary_key;
                  const fieldType = colSchema?.field_type || 'Text';
                  const cfg = getFieldConfig(fieldType);
                  const isEditing = editingCell?.row === rowIdx && editingCell?.col === col;
                  const isSaving = savingCell?.row === rowIdx && savingCell?.col === col;
                  const isRO = isPk || cfg.readOnly;

                  if (isSaving) {
                    return (
                      <td key={col} className="px-3 py-1.5 border-b border-gray-700/50">
                        <Loader2 className="w-3 h-3 animate-spin text-blue-400" />
                      </td>
                    );
                  }

                  if (isEditing) {
                    const inputType = cfg.inputType === 'checkbox' ? 'text' : (cfg.inputType || 'text');
                    return (
                      <td key={col} className="px-1 py-0.5 border-b border-gray-700/50">
                        <input
                          type={inputType}
                          step={cfg.step}
                          value={editValue}
                          onChange={e => onEditValueChange(e.target.value)}
                          onKeyDown={e => {
                            if (e.key === 'Enter') onCommitEdit();
                            if (e.key === 'Escape') onCancelEdit();
                          }}
                          onBlur={onCommitEdit}
                          className="w-full bg-gray-900 text-white text-xs rounded px-2 py-1 border border-blue-500 outline-none"
                          autoFocus
                        />
                      </td>
                    );
                  }

                  const displayValue = row[col + '_display'];

                  return (
                    <td
                      key={col}
                      className={`px-3 py-1.5 border-b border-gray-700/50 text-xs max-w-[300px] truncate ${
                        cfg.align === 'right' ? 'text-right' : ''
                      } ${cfg.mono ? 'font-mono' : ''} ${
                        isRO ? 'text-gray-500 cursor-default' : 'text-gray-300 cursor-pointer hover:bg-gray-700/50'
                      }`}
                      title={value == null ? 'null' : String(value)}
                      onDoubleClick={() => !isRO && onStartEdit(rowIdx, col, value)}
                    >
                      <CellValue value={value} fieldType={fieldType} displayValue={displayValue} />
                    </td>
                  );
                })}
              </tr>
            );
          })}
          {rows.length === 0 && (
            <tr>
              <td colSpan={visibleColumns.length + 1} className="text-center py-8 text-gray-500 text-xs">
                Aucune ligne
              </td>
            </tr>
          )}
        </tbody>
      </table>
    </div>
  );
}
