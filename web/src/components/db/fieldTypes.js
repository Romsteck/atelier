/**
 * Field type configuration for Dataverse schema-aware rendering.
 * Maps FieldType strings from the backend to input/render config.
 */

const FIELD_TYPES = {
  Boolean: {
    inputType: 'checkbox',
    align: 'center',
    format: (v) => v === 1 || v === true,
    toSql: (v) => v ? 1 : 0,
  },
  Number: { inputType: 'number', align: 'right', step: '1' },
  AutoIncrement: { inputType: 'number', align: 'right', readOnly: true },
  Decimal: { inputType: 'number', align: 'right', step: '0.01' },
  Currency: {
    inputType: 'number',
    align: 'right',
    step: '0.01',
    formatCell: (v) => v != null ? new Intl.NumberFormat('fr-FR', { style: 'currency', currency: 'EUR' }).format(v) : null,
  },
  Percent: {
    inputType: 'number',
    align: 'right',
    step: '0.1',
    formatCell: (v) => v != null ? `${v}%` : null,
  },
  DateTime: { inputType: 'datetime-local' },
  Date: { inputType: 'date' },
  Time: { inputType: 'time' },
  Email: { inputType: 'email', isLink: 'mailto' },
  Url: { inputType: 'url', isLink: 'href' },
  Phone: { inputType: 'tel', isLink: 'tel' },
  Duration: { inputType: 'text' },
  Json: { inputType: 'textarea' },
  Uuid: { inputType: 'text', mono: true },
  Choice: { inputType: 'select' },
  MultiChoice: { inputType: 'multiselect' },
  Lookup: { inputType: 'lookup' },
  Formula: { readOnly: true, isFormula: true },
  Text: { inputType: 'text' },
};

export function getFieldConfig(fieldType) {
  return FIELD_TYPES[fieldType] || FIELD_TYPES.Text;
}

export function isReadOnly(fieldType) {
  const cfg = getFieldConfig(fieldType);
  return cfg.readOnly || false;
}

export function isTextLike(fieldType) {
  return ['Text', 'Email', 'Url', 'Phone', 'Uuid', 'Duration'].includes(fieldType);
}
