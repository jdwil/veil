<script lang="ts">
  /**
   * RuleBuilder — visual boolean expression builder for Decision nodes.
   *
   * Data model: Groups[] joined by OR, each group has Predicates[] joined by AND.
   * Serialized as JSON: [{ predicates: [{ path, op, value }] }]
   */

  export interface RuleField {
    path: string;
    type: string;
    label?: string;
  }

  interface Predicate {
    path: string;
    op: string;
    value: string;
  }

  interface Group {
    predicates: Predicate[];
  }

  // ─── Operator definitions ────────────────────────────────────────────────
  interface OperatorDef {
    value: string;
    label: string;
    types: string[];  // applicable type categories
    needsValue: boolean;
    needsSecondValue?: boolean;  // for Between
  }

  const ALL_TYPES = ['string', 'number', 'boolean', 'date', 'any'];

  const OPERATORS: OperatorDef[] = [
    { value: 'Eq',          label: 'equals',                 types: ALL_TYPES,                          needsValue: true },
    { value: 'Neq',         label: 'not equals',             types: ALL_TYPES,                          needsValue: true },
    { value: 'Gt',          label: 'greater than',           types: ['number', 'date'],                 needsValue: true },
    { value: 'Gte',         label: '≥',                      types: ['number', 'date'],                 needsValue: true },
    { value: 'Lt',          label: 'less than',              types: ['number', 'date'],                 needsValue: true },
    { value: 'Lte',         label: '≤',                      types: ['number', 'date'],                 needsValue: true },
    { value: 'Contains',    label: 'contains',               types: ['string', 'list'],                 needsValue: true },
    { value: 'NotContains', label: 'does not contain',       types: ['string', 'list'],                 needsValue: true },
    { value: 'StartsWith',  label: 'starts with',            types: ['string'],                         needsValue: true },
    { value: 'EndsWith',    label: 'ends with',              types: ['string'],                         needsValue: true },
    { value: 'In',          label: 'is in',                  types: ALL_TYPES,                          needsValue: true },
    { value: 'NotIn',       label: 'is not in',              types: ALL_TYPES,                          needsValue: true },
    { value: 'Exists',      label: 'exists (not null)',      types: ALL_TYPES,                          needsValue: false },
    { value: 'NotExists',   label: 'does not exist (null)',  types: ALL_TYPES,                          needsValue: false },
    { value: 'Matches',     label: 'matches regex',          types: ['string'],                         needsValue: true },
    { value: 'Between',     label: 'between',                types: ['number', 'date'],                 needsValue: true, needsSecondValue: true },
  ];

  // ─── Props ───────────────────────────────────────────────────────────────
  let { fields, value, onChange, compact = false }: {
    fields: RuleField[];
    value: string;
    onChange: (json: string) => void;
    compact?: boolean;
  } = $props();

  // ─── State ───────────────────────────────────────────────────────────────
  let groups: Group[] = $state([]);

  // Re-parse when external value changes
  $effect(() => {
    const parsed = parseGroups(value);
    // Only update if different (avoid loops)
    if (JSON.stringify(parsed) !== JSON.stringify(groups)) {
      groups = parsed;
    }
  });

  // ─── Helpers ─────────────────────────────────────────────────────────────
  function parseGroups(json: string): Group[] {
    try {
      const g = JSON.parse(json);
      if (Array.isArray(g) && g.length > 0) return g;
    } catch { /* empty */ }
    return [{ predicates: [defaultPredicate()] }];
  }

  function defaultPredicate(): Predicate {
    const firstField = fields.length > 0 ? fields[0].path : '';
    return { path: firstField, op: 'Exists', value: '' };
  }

  function emit() {
    onChange(JSON.stringify(groups));
  }

  /** Map VEIL IR types (Str, Int, F64, Bool, Dt, etc.) to operator category. */
  function normalizeType(t: string): string {
    const lower = t.toLowerCase();
    if (lower === 'str' || lower === 'string' || lower === 'email') return 'string';
    if (lower === 'int' || lower === 'f64' || lower === 'number' || lower === 'i64' || lower === 'u64') return 'number';
    if (lower === 'bool' || lower === 'boolean') return 'boolean';
    if (lower === 'dt' || lower === 'date' || lower === 'datetime') return 'date';
    if (lower.startsWith('list') || lower.startsWith('set') || lower.startsWith('vec')) return 'list';
    return 'any';
  }

  function operatorsForField(path: string): OperatorDef[] {
    const field = fields.find(f => f.path === path);
    if (!field) return OPERATORS; // unknown field → show all
    const norm = normalizeType(field.type);
    if (norm === 'any') return OPERATORS;
    return OPERATORS.filter(op => op.types.includes(norm) || op.types.includes('any'));
  }

  function fieldType(path: string): string {
    return fields.find(f => f.path === path)?.type ?? '';
  }

  // ─── Mutations ───────────────────────────────────────────────────────────
  function addPredicateToGroup(gi: number) {
    groups[gi].predicates = [...groups[gi].predicates, defaultPredicate()];
    groups = [...groups];
    emit();
  }

  function removePredicateFromGroup(gi: number, pi: number) {
    if (groups[gi].predicates.length <= 1) return;
    groups[gi].predicates = groups[gi].predicates.filter((_, i) => i !== pi);
    groups = [...groups];
    emit();
  }

  function updatePredicate(gi: number, pi: number, field: keyof Predicate, val: string) {
    groups[gi].predicates[pi] = { ...groups[gi].predicates[pi], [field]: val };
    // Reset value when operator changes to one that doesn't need a value
    if (field === 'op') {
      const opDef = OPERATORS.find(o => o.value === val);
      if (opDef && !opDef.needsValue) {
        groups[gi].predicates[pi].value = '';
      }
    }
    // Reset operator when field changes if current operator not valid for new type
    if (field === 'path') {
      const validOps = operatorsForField(val);
      const currentOp = groups[gi].predicates[pi].op;
      if (!validOps.find(o => o.value === currentOp)) {
        groups[gi].predicates[pi].op = validOps[0]?.value ?? 'Eq';
      }
    }
    groups = [...groups];
    emit();
  }

  function addGroup() {
    groups = [...groups, { predicates: [defaultPredicate()] }];
    emit();
  }

  function removeGroup(gi: number) {
    if (groups.length <= 1) return;
    groups = groups.filter((_, i) => i !== gi);
    emit();
  }
</script>

<div class="rule-builder" class:compact>
  {#each groups as group, gi}
    {#if gi > 0}
      <div class="rb-or-sep"><span>OR</span></div>
    {/if}
    <div class="rb-group">
      <div class="rb-group-head">
        <span class="rb-group-label">Group {gi + 1} <span class="rb-logic-hint">AND</span></span>
        {#if groups.length > 1}
          <button type="button" class="rb-btn-ghost" onclick={() => removeGroup(gi)}>Remove</button>
        {/if}
      </div>
      {#each group.predicates as pred, pi}
        {#if pi > 0}
          <div class="rb-and-sep"><span>AND</span></div>
        {/if}
        <div class="rb-predicate">
          <!-- Field select -->
          <select
            class="rb-select rb-field-select"
            value={pred.path}
            onchange={(e) => updatePredicate(gi, pi, 'path', (e.target as HTMLSelectElement).value)}
          >
            <option value="">— field —</option>
            {#each fields as f}
              <option value={f.path}>{f.label || f.path}{f.type ? ` (${f.type})` : ''}</option>
            {/each}
          </select>

          <!-- Operator select (filtered by field type) -->
          <select
            class="rb-select rb-op-select"
            value={pred.op}
            onchange={(e) => updatePredicate(gi, pi, 'op', (e.target as HTMLSelectElement).value)}
          >
            {#each operatorsForField(pred.path) as op}
              <option value={op.value}>{op.label}</option>
            {/each}
          </select>

          <!-- Value input (hidden for Exists/NotExists) -->
          {#if OPERATORS.find(o => o.value === pred.op)?.needsValue}
            <input
              type="text"
              class="rb-input"
              value={pred.value}
              placeholder={fieldType(pred.path) || 'value'}
              oninput={(e) => updatePredicate(gi, pi, 'value', (e.target as HTMLInputElement).value)}
            />
          {/if}

          <!-- Remove predicate -->
          {#if group.predicates.length > 1}
            <button type="button" class="rb-btn-remove" onclick={() => removePredicateFromGroup(gi, pi)} title="Remove condition">✕</button>
          {/if}
        </div>
      {/each}
      <button type="button" class="rb-btn-add" onclick={() => addPredicateToGroup(gi)}>+ Add condition</button>
    </div>
  {/each}
  <button type="button" class="rb-btn-add-group" onclick={addGroup}>+ Add OR group</button>
</div>

<style>
  .rule-builder {
    display: flex;
    flex-direction: column;
    gap: 8px;
    font-size: 12px;
  }

  .rule-builder.compact {
    gap: 6px;
    font-size: 11px;
  }

  .rb-group {
    border: 1px solid var(--veil-border);
    border-radius: 8px;
    padding: 8px;
    background: var(--veil-input-bg);
    display: flex;
    flex-direction: column;
    gap: 6px;
  }

  .rb-group-head {
    display: flex;
    align-items: center;
    justify-content: space-between;
  }

  .rb-group-label {
    font-size: 10px;
    font-weight: 600;
    text-transform: uppercase;
    letter-spacing: 0.4px;
    color: var(--veil-text-dim);
  }

  .rb-logic-hint {
    font-size: 9px;
    color: var(--veil-text-faint);
    font-weight: 400;
    margin-left: 4px;
  }

  .rb-or-sep, .rb-and-sep {
    display: flex;
    align-items: center;
    justify-content: center;
    padding: 2px 0;
  }

  .rb-or-sep span {
    font-size: 10px;
    font-weight: 700;
    color: #f59e0b;
    text-transform: uppercase;
    letter-spacing: 1px;
  }

  .rb-and-sep span {
    font-size: 9px;
    color: var(--veil-text-faint);
    text-transform: uppercase;
    letter-spacing: 0.5px;
  }

  .rb-predicate {
    display: flex;
    align-items: center;
    gap: 4px;
    flex-wrap: wrap;
  }

  .rb-select {
    background: var(--veil-surface);
    border: 1px solid var(--veil-border);
    border-radius: 4px;
    padding: 4px 6px;
    font-size: inherit;
    color: var(--veil-text);
    cursor: pointer;
    outline: none;
    min-width: 0;
  }

  .rb-field-select {
    flex: 1;
    min-width: 80px;
  }

  .rb-op-select {
    min-width: 70px;
  }

  .rb-select:focus {
    border-color: var(--veil-text-dim);
  }

  .rb-input {
    flex: 1;
    min-width: 60px;
    background: var(--veil-surface);
    border: 1px solid var(--veil-border);
    border-radius: 4px;
    padding: 4px 6px;
    font-size: inherit;
    color: var(--veil-text);
    outline: none;
  }

  .rb-input:focus {
    border-color: var(--veil-text-dim);
  }

  .rb-input::placeholder {
    color: var(--veil-text-faint);
  }

  .rb-btn-remove {
    background: none;
    border: none;
    color: var(--veil-text-faint);
    cursor: pointer;
    font-size: 12px;
    padding: 2px 4px;
    border-radius: 3px;
    line-height: 1;
  }

  .rb-btn-remove:hover {
    color: #f87171;
    background: rgba(248, 113, 113, 0.1);
  }

  .rb-btn-ghost {
    background: none;
    border: none;
    color: var(--veil-text-faint);
    cursor: pointer;
    font-size: 10px;
    padding: 2px 6px;
    border-radius: 3px;
  }

  .rb-btn-ghost:hover {
    color: #f87171;
    background: rgba(248, 113, 113, 0.1);
  }

  .rb-btn-add, .rb-btn-add-group {
    background: none;
    border: 1px dashed var(--veil-border);
    border-radius: 4px;
    color: var(--veil-text-dim);
    cursor: pointer;
    font-size: 11px;
    padding: 4px 8px;
    text-align: center;
    transition: border-color 0.15s, color 0.15s;
  }

  .rb-btn-add:hover, .rb-btn-add-group:hover {
    border-color: var(--veil-text-dim);
    color: var(--veil-text);
  }

  .rb-btn-add-group {
    margin-top: 2px;
  }
</style>
