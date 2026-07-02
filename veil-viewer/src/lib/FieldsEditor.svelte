<script lang="ts">
  import { ALL_TYPES, formatType } from '$lib/typeDisplay';

  interface FieldDef {
    name: string;
    type: string;
  }

  let { fields = [], label = 'Fields', onChange }: {
    fields: FieldDef[];
    label?: string;
    onChange: (fields: FieldDef[]) => void;
  } = $props();

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
</script>

<div class="fields-editor">
  <div class="section-header">
    <span class="label-text">{label}</span>
    <button class="add-btn" onclick={addField}>+ Add</button>
  </div>

  {#each fields as field, i}
    <div class="field-row">
      <input
        type="text"
        class="field-name"
        value={field.name}
        placeholder="field_name"
        oninput={(e) => updateField(i, 'name', e.currentTarget.value)}
      />
      <select
        class="field-type"
        value={field.type}
        onchange={(e) => updateField(i, 'type', e.currentTarget.value)}
      >
        {#each ALL_TYPES as t}
          <option value={t}>{formatType(t)}</option>
        {/each}
      </select>
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
  .label-text { font-size: 10px; text-transform: uppercase; letter-spacing: 0.5px; color: #64748b; font-weight: 600; }

  .field-row { display: flex; gap: 4px; align-items: center; }
  .field-name {
    flex: 1;
    background: rgba(0,0,0,0.3); border: 1px solid #2d2d44; border-radius: 4px;
    padding: 5px 8px; font-size: 11px; color: #e2e8f0; outline: none; font-family: monospace;
  }
  .field-name:focus { border-color: #6366f1; }
  .field-type {
    width: 90px;
    background: rgba(0,0,0,0.3); border: 1px solid #2d2d44; border-radius: 4px;
    padding: 5px 4px; font-size: 10px; color: #94a3b8; outline: none; cursor: pointer;
  }

  .add-btn {
    font-size: 10px; padding: 3px 8px; border-radius: 4px;
    background: rgba(99,102,241,0.1); border: 1px solid rgba(99,102,241,0.3);
    color: #a5b4fc; cursor: pointer;
  }
  .add-btn:hover { background: rgba(99,102,241,0.2); }
  .remove-btn {
    background: none; border: none; color: #64748b; font-size: 11px;
    cursor: pointer; padding: 2px 4px; border-radius: 3px;
  }
  .remove-btn:hover { color: #f87171; background: rgba(248,113,113,0.1); }
  .empty-hint { font-size: 10px; color: #475569; font-style: italic; padding: 4px 0; }
</style>
