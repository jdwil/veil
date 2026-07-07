<script lang="ts">
  import { ALL_TYPES, formatType } from '$lib/typeDisplay';
  import { TypeEditor } from '$lib/editors';
  import type { TypeExpr } from '$lib/editors/expr-types';

  interface FieldDef {
    name: string;
    type: string;
  }

  let { fields = [], label = 'Fields', onChange }: {
    fields: FieldDef[];
    label?: string;
    onChange: (fields: FieldDef[]) => void;
  } = $props();

  let useVisualTypes = $state(false);

  function addField() {
    onChange([...fields, { name: '', type: 'Str' }]);
  }

  function removeField(index: number) {
    onChange(fields.filter((_, i) => i !== index));
  }

  function updateField(index: number, field: string, value: string) {
    const updated = [...fields];
    (updated[index] as any)[field] = value;
    onChange(updated);
  }

  function moveUp(index: number) {
    if (index === 0) return;
    const updated = [...fields];
    [updated[index - 1], updated[index]] = [updated[index], updated[index - 1]];
    onChange(updated);
  }

  function moveDown(index: number) {
    if (index >= fields.length - 1) return;
    const updated = [...fields];
    [updated[index], updated[index + 1]] = [updated[index + 1], updated[index]];
    onChange(updated);
  }

  /** Convert a type string to a TypeExpr for the visual editor */
  function stringToTypeExpr(s: string): TypeExpr {
    s = s.trim();
    if (s.startsWith('Res!<')) return { kind: 'result', inner: stringToTypeExpr(s.slice(5, -1)) };
    if (s === 'Res!') return { kind: 'result' };
    if (s.startsWith('Opt<')) return { kind: 'optional', inner: stringToTypeExpr(s.slice(4, -1)) };
    if (s.startsWith('List<')) return { kind: 'list', inner: stringToTypeExpr(s.slice(5, -1)) };
    if (s.startsWith('&mut ')) return { kind: 'ref', inner: stringToTypeExpr(s.slice(5)), mutable: true };
    if (s.startsWith('&')) return { kind: 'ref', inner: stringToTypeExpr(s.slice(1)), mutable: false };
    return { kind: 'named', name: s };
  }

  /** Convert a TypeExpr back to a type string */
  function typeExprToString(ty: TypeExpr): string {
    switch (ty.kind) {
      case 'named': return ty.name;
      case 'result': return ty.inner ? `Res!<${typeExprToString(ty.inner)}>` : 'Res!';
      case 'optional': return `Opt<${typeExprToString(ty.inner)}>`;
      case 'list': return `List<${typeExprToString(ty.inner)}>`;
      case 'ref': return ty.mutable ? `&mut ${typeExprToString(ty.inner)}` : `&${typeExprToString(ty.inner)}`;
      default: return JSON.stringify(ty);
    }
  }
</script>

<div class="fields-editor">
  <div class="section-header">
    <span class="label-text">{label}</span>
    <div class="header-actions">
      <button class="toggle-btn" onclick={() => useVisualTypes = !useVisualTypes} title="Toggle visual type editor">
        {useVisualTypes ? '📝' : '⚡'}
      </button>
      <button class="add-btn" onclick={addField}>+ Add</button>
    </div>
  </div>

  {#each fields as field, i}
    <div class="field-row">
      <div class="field-actions">
        <button class="action-btn" onclick={() => moveUp(i)} disabled={i === 0}>↑</button>
        <button class="action-btn" onclick={() => moveDown(i)} disabled={i === fields.length - 1}>↓</button>
      </div>
      <input
        type="text"
        class="field-name"
        value={field.name}
        placeholder="field_name"
        oninput={(e) => updateField(i, 'name', (e.target as HTMLInputElement).value)}
      />
      <span class="field-colon">:</span>
      {#if useVisualTypes}
        <TypeEditor
          type={stringToTypeExpr(field.type)}
          onChange={(ty) => updateField(i, 'type', typeExprToString(ty))}
        />
      {:else}
        <select
          class="field-type"
          value={field.type}
          onchange={(e) => updateField(i, 'type', (e.target as HTMLSelectElement).value)}
        >
          {#each ALL_TYPES as t}
            <option value={t}>{formatType(t)}</option>
          {/each}
        </select>
      {/if}
      <button class="remove-btn" onclick={() => removeField(i)}>✕</button>
    </div>
  {/each}

  {#if fields.length === 0}
    <div class="empty-hint">No {label.toLowerCase()} defined</div>
  {/if}
</div>

<style>
  .fields-editor { display: flex; flex-direction: column; gap: 4px; }
  .section-header { display: flex; align-items: center; justify-content: space-between; }
  .header-actions { display: flex; gap: 4px; align-items: center; }
  .label-text { font-size: 10px; text-transform: uppercase; letter-spacing: 0.5px; color: var(--veil-text-dim); font-weight: 600; }

  .field-row { display: flex; gap: 4px; align-items: center; }
  .field-actions {
    display: flex; flex-direction: column; gap: 1px; opacity: 0.3;
  }
  .field-row:hover .field-actions { opacity: 1; }
  .action-btn {
    background: none; border: none; color: var(--veil-text-dim); font-size: 9px;
    cursor: pointer; padding: 0 2px; line-height: 1;
  }
  .action-btn:hover { color: var(--veil-text); }
  .action-btn:disabled { opacity: 0.2; }

  .field-name {
    flex: 1; min-width: 60px;
    background: var(--veil-input-bg); border: 1px solid var(--veil-border); border-radius: 4px;
    padding: 5px 8px; font-size: 11px; color: var(--veil-text); outline: none;
    font-family: 'JetBrains Mono', monospace;
  }
  .field-name:focus { border-color: var(--veil-text-dim); }
  .field-colon { color: var(--veil-text-dim); font-size: 11px; }
  .field-type {
    width: 90px;
    background: var(--veil-input-bg); border: 1px solid var(--veil-border); border-radius: 4px;
    padding: 5px 4px; font-size: 10px; color: var(--veil-text); outline: none; cursor: pointer;
  }

  .add-btn {
    font-size: 10px; padding: 3px 8px; border-radius: 4px;
    background: rgba(99,102,241,0.1); border: 1px solid rgba(99,102,241,0.3);
    color: var(--veil-text); cursor: pointer;
  }
  .add-btn:hover { background: rgba(99,102,241,0.2); }
  .toggle-btn {
    font-size: 12px; padding: 2px 4px; border-radius: 3px;
    background: none; border: 1px solid var(--veil-border); cursor: pointer;
  }
  .toggle-btn:hover { background: var(--veil-border); }
  .remove-btn {
    background: none; border: none; color: var(--veil-text-dim); font-size: 11px;
    cursor: pointer; padding: 2px 4px; border-radius: 3px;
  }
  .remove-btn:hover { color: #f87171; background: rgba(248,113,113,0.1); }
  .empty-hint { font-size: 10px; color: var(--veil-text-faint); font-style: italic; padding: 4px 0; }
</style>
