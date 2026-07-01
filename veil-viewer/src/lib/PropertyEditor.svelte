<script lang="ts">
  import { NODE_STYLES, type NodeKind } from '$lib/types';

  let { node, onUpdate, onClose }: {
    node: { id: string; data: any };
    onUpdate: (id: string, data: any) => void;
    onClose: () => void;
  } = $props();

  let name = $state(node.data.label ?? '');
  let kind: NodeKind = node.data.kind;
  let style = NODE_STYLES[kind];

  // Field editing for domain constructs
  let fields = $state<{ name: string; type: string }[]>(
    (node.data.fields ?? []).length > 0
      ? node.data.fields
      : []
  );

  function addField() {
    fields = [...fields, { name: '', type: 'Str' }];
  }

  function removeField(index: number) {
    fields = fields.filter((_, i) => i !== index);
  }

  function save() {
    onUpdate(node.id, {
      ...node.data,
      label: name,
      fields: fields.length > 0 ? fields : undefined,
    });
  }

  // Auto-save on changes
  $effect(() => {
    // Trigger save whenever name or fields change
    const _ = name;
    const __ = JSON.stringify(fields);
    save();
  });

  const TYPE_OPTIONS = ['Str', 'Int', 'F64', 'Bool', 'UUID', 'DateTime', 'Bytes', 'Email', 'Phone'];

  const showFields = ['Aggregate', 'Entity', 'ValueObject', 'Event', 'Command', 'Port'].includes(kind);
</script>

<div class="property-editor">
  <div class="pe-header">
    <div class="pe-title-row">
      <span class="pe-icon">{style.icon}</span>
      <span class="pe-kind" style="color: {style.color}">{style.label}</span>
    </div>
    <button class="pe-close" onclick={onClose}>✕</button>
  </div>

  <div class="pe-body">
    <label class="pe-label">
      <span class="label-text">Name</span>
      <input
        type="text"
        class="pe-input"
        bind:value={name}
        placeholder="Enter name..."
      />
    </label>

    {#if showFields}
      <div class="pe-section">
        <div class="section-header">
          <span class="label-text">Fields</span>
          <button class="add-btn" onclick={addField}>+ Add</button>
        </div>
        <div class="fields-list">
          {#each fields as field, i}
            <div class="field-row">
              <input
                type="text"
                class="pe-input field-name"
                bind:value={field.name}
                placeholder="field_name"
              />
              <select class="pe-select" bind:value={field.type}>
                {#each TYPE_OPTIONS as t}
                  <option value={t}>{t}</option>
                {/each}
              </select>
              <button class="remove-btn" onclick={() => removeField(i)}>✕</button>
            </div>
          {/each}
        </div>
      </div>
    {/if}
  </div>
</div>

<style>
  .property-editor {
    position: absolute;
    top: 60px;
    right: 16px;
    width: 280px;
    background: rgba(20, 20, 35, 0.98);
    border: 1px solid #2d2d44;
    border-radius: 12px;
    backdrop-filter: blur(16px);
    box-shadow: 0 12px 40px rgba(0, 0, 0, 0.6);
    z-index: 50;
    overflow: hidden;
  }

  .pe-header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 12px 16px;
    border-bottom: 1px solid #2d2d44;
  }

  .pe-title-row {
    display: flex;
    align-items: center;
    gap: 8px;
  }

  .pe-icon {
    font-size: 16px;
  }

  .pe-kind {
    font-size: 11px;
    font-weight: 700;
    text-transform: uppercase;
    letter-spacing: 0.5px;
  }

  .pe-close {
    background: none;
    border: none;
    color: #64748b;
    font-size: 14px;
    cursor: pointer;
    padding: 4px;
    border-radius: 4px;
  }

  .pe-close:hover {
    color: #e2e8f0;
    background: rgba(255, 255, 255, 0.05);
  }

  .pe-body {
    padding: 12px 16px;
    display: flex;
    flex-direction: column;
    gap: 12px;
  }

  .pe-label {
    display: flex;
    flex-direction: column;
    gap: 4px;
  }

  .label-text {
    font-size: 10px;
    text-transform: uppercase;
    letter-spacing: 0.5px;
    color: #64748b;
    font-weight: 600;
  }

  .pe-input {
    background: rgba(0, 0, 0, 0.3);
    border: 1px solid #2d2d44;
    border-radius: 6px;
    padding: 8px 10px;
    font-size: 13px;
    color: #e2e8f0;
    outline: none;
    transition: border-color 0.15s;
  }

  .pe-input:focus {
    border-color: #6366f1;
  }

  .pe-input::placeholder {
    color: #475569;
  }

  .pe-select {
    background: rgba(0, 0, 0, 0.3);
    border: 1px solid #2d2d44;
    border-radius: 6px;
    padding: 6px 8px;
    font-size: 11px;
    color: #e2e8f0;
    outline: none;
    cursor: pointer;
  }

  .pe-select:focus {
    border-color: #6366f1;
  }

  .pe-section {
    display: flex;
    flex-direction: column;
    gap: 6px;
  }

  .section-header {
    display: flex;
    align-items: center;
    justify-content: space-between;
  }

  .add-btn {
    font-size: 10px;
    padding: 3px 8px;
    border-radius: 4px;
    background: rgba(99, 102, 241, 0.1);
    border: 1px solid rgba(99, 102, 241, 0.3);
    color: #a5b4fc;
    cursor: pointer;
  }

  .add-btn:hover {
    background: rgba(99, 102, 241, 0.2);
  }

  .fields-list {
    display: flex;
    flex-direction: column;
    gap: 4px;
  }

  .field-row {
    display: flex;
    gap: 4px;
    align-items: center;
  }

  .field-name {
    flex: 1;
    font-size: 11px;
    padding: 6px 8px;
  }

  .remove-btn {
    background: none;
    border: none;
    color: #64748b;
    font-size: 11px;
    cursor: pointer;
    padding: 4px;
    border-radius: 4px;
  }

  .remove-btn:hover {
    color: #f87171;
    background: rgba(248, 113, 113, 0.1);
  }
</style>
