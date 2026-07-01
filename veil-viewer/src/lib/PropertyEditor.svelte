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

  // Annotations/tags
  let annotations = $state<string[]>([...(node.data.annotations ?? [])]);
  let newAnnotation = $state('');

  // Fields (for domain types)
  let fields = $state<{ name: string; type: string }[]>(
    [...(node.data.fields ?? [])]
  );

  // Adapter-specific
  let targetPort = $state(node.data.targetPort ?? '');
  let envVars = $state<string[]>([...(node.data.envVars ?? [])]);
  let newEnvVar = $state('');

  // Flow-specific
  let inputs = $state<{ name: string; type: string }[]>(
    [...(node.data.inputs ?? [])]
  );

  // Reset state when node changes
  $effect(() => {
    name = node.data.label ?? '';
    annotations = [...(node.data.annotations ?? [])];
    fields = [...(node.data.fields ?? [])];
    targetPort = node.data.targetPort ?? '';
    envVars = [...(node.data.envVars ?? [])];
    inputs = [...(node.data.inputs ?? [])];
  });

  function save() {
    onUpdate(node.id, {
      ...node.data,
      label: name,
      annotations,
      fields: fields.length > 0 ? fields : undefined,
      targetPort: targetPort || undefined,
      envVars: envVars.length > 0 ? envVars : undefined,
      inputs: inputs.length > 0 ? inputs : undefined,
    });
  }

  function addAnnotation() {
    if (newAnnotation.trim()) {
      annotations = [...annotations, `@${newAnnotation.trim()}`];
      newAnnotation = '';
      save();
    }
  }

  function removeAnnotation(index: number) {
    annotations = annotations.filter((_, i) => i !== index);
    save();
  }

  function addField() {
    fields = [...fields, { name: '', type: 'Str' }];
  }

  function removeField(index: number) {
    fields = fields.filter((_, i) => i !== index);
    save();
  }

  function addInput() {
    inputs = [...inputs, { name: '', type: 'Str' }];
  }

  function removeInput(index: number) {
    inputs = inputs.filter((_, i) => i !== index);
    save();
  }

  function addEnvVar() {
    if (newEnvVar.trim()) {
      envVars = [...envVars, newEnvVar.trim()];
      newEnvVar = '';
      save();
    }
  }

  function removeEnvVar(index: number) {
    envVars = envVars.filter((_, i) => i !== index);
    save();
  }

  const TYPE_OPTIONS = ['Str', 'Int', 'F64', 'Bool', 'UUID', 'DateTime', 'Bytes', 'Email', 'Phone', 'Customer', 'Subscription'];

  const showFields = ['Aggregate', 'Entity', 'ValueObject', 'Event', 'Command', 'Port'].includes(kind);
  const showAnnotations = ['Aggregate', 'Flow', 'Step', 'Adapter', 'Context'].includes(kind);
  const showAdapter = kind === 'Adapter';
  const showFlowInputs = kind === 'Flow';
</script>

<div class="property-editor" onclick={(e) => e.stopPropagation()} onpointerdown={(e) => e.stopPropagation()}>
  <div class="pe-header">
    <div class="pe-title-row">
      <span class="pe-icon">{style.icon}</span>
      <span class="pe-kind" style="color: {style.color}">{style.label}</span>
    </div>
    <button class="pe-close" onclick={(e) => { e.stopPropagation(); onClose(); }}>✕</button>
  </div>

  <div class="pe-body">
    <!-- Name -->
    <label class="pe-label">
      <span class="label-text">Name</span>
      <input type="text" class="pe-input" bind:value={name} oninput={save} placeholder="Enter name..." />
    </label>

    <!-- Adapter: target port -->
    {#if showAdapter}
      <label class="pe-label">
        <span class="label-text">Implements Port</span>
        <input type="text" class="pe-input" bind:value={targetPort} oninput={save} placeholder="PortName" />
      </label>

      <div class="pe-section">
        <div class="section-header">
          <span class="label-text">Env Vars</span>
        </div>
        <div class="inline-add">
          <input type="text" class="pe-input small" bind:value={newEnvVar} placeholder="VAR_NAME" onkeydown={(e) => e.key === 'Enter' && addEnvVar()} />
          <button class="add-btn" onclick={addEnvVar}>+</button>
        </div>
        <div class="tag-list">
          {#each envVars as v, i}
            <span class="tag">{v} <button class="tag-remove" onclick={() => removeEnvVar(i)}>✕</button></span>
          {/each}
        </div>
      </div>
    {/if}

    <!-- Flow: inputs -->
    {#if showFlowInputs}
      <div class="pe-section">
        <div class="section-header">
          <span class="label-text">Inputs</span>
          <button class="add-btn" onclick={addInput}>+ Add</button>
        </div>
        <div class="fields-list">
          {#each inputs as field, i}
            <div class="field-row">
              <input type="text" class="pe-input field-name" bind:value={field.name} oninput={save} placeholder="param" />
              <select class="pe-select" bind:value={field.type} onchange={save}>
                {#each TYPE_OPTIONS as t}<option value={t}>{t}</option>{/each}
              </select>
              <button class="remove-btn" onclick={() => removeInput(i)}>✕</button>
            </div>
          {/each}
        </div>
      </div>
    {/if}

    <!-- Annotations/tags -->
    {#if showAnnotations}
      <div class="pe-section">
        <div class="section-header">
          <span class="label-text">Annotations</span>
        </div>
        <div class="inline-add">
          <input type="text" class="pe-input small" bind:value={newAnnotation} placeholder="async" onkeydown={(e) => e.key === 'Enter' && addAnnotation()} />
          <button class="add-btn" onclick={addAnnotation}>+</button>
        </div>
        <div class="tag-list">
          {#each annotations as ann, i}
            <span class="tag">{ann} <button class="tag-remove" onclick={() => removeAnnotation(i)}>✕</button></span>
          {/each}
        </div>
      </div>
    {/if}

    <!-- Fields -->
    {#if showFields}
      <div class="pe-section">
        <div class="section-header">
          <span class="label-text">Fields</span>
          <button class="add-btn" onclick={addField}>+ Add</button>
        </div>
        <div class="fields-list">
          {#each fields as field, i}
            <div class="field-row">
              <input type="text" class="pe-input field-name" bind:value={field.name} oninput={save} placeholder="field_name" />
              <select class="pe-select" bind:value={field.type} onchange={save}>
                {#each TYPE_OPTIONS as t}<option value={t}>{t}</option>{/each}
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
    top: 12px;
    right: 12px;
    width: 290px;
    max-height: calc(100vh - 100px);
    overflow-y: auto;
    background: rgba(20, 20, 35, 0.98);
    border: 1px solid #2d2d44;
    border-radius: 12px;
    backdrop-filter: blur(16px);
    box-shadow: 0 12px 40px rgba(0, 0, 0, 0.6);
    z-index: 50;
  }

  .pe-header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 12px 16px;
    border-bottom: 1px solid #2d2d44;
    position: sticky;
    top: 0;
    background: rgba(20, 20, 35, 0.98);
    border-radius: 12px 12px 0 0;
  }

  .pe-title-row { display: flex; align-items: center; gap: 8px; }
  .pe-icon { font-size: 16px; }
  .pe-kind { font-size: 11px; font-weight: 700; text-transform: uppercase; letter-spacing: 0.5px; }

  .pe-close {
    background: none; border: none; color: #64748b; font-size: 14px;
    cursor: pointer; padding: 4px 8px; border-radius: 4px;
  }
  .pe-close:hover { color: #e2e8f0; background: rgba(255,255,255,0.05); }

  .pe-body { padding: 12px 16px; display: flex; flex-direction: column; gap: 14px; }
  .pe-label { display: flex; flex-direction: column; gap: 4px; }
  .label-text { font-size: 10px; text-transform: uppercase; letter-spacing: 0.5px; color: #64748b; font-weight: 600; }

  .pe-input {
    background: rgba(0,0,0,0.3); border: 1px solid #2d2d44; border-radius: 6px;
    padding: 8px 10px; font-size: 13px; color: #e2e8f0; outline: none; transition: border-color 0.15s;
    width: 100%; box-sizing: border-box;
  }
  .pe-input:focus { border-color: #6366f1; }
  .pe-input::placeholder { color: #475569; }
  .pe-input.small { padding: 6px 8px; font-size: 11px; flex: 1; }

  .pe-select {
    background: rgba(0,0,0,0.3); border: 1px solid #2d2d44; border-radius: 6px;
    padding: 6px 8px; font-size: 11px; color: #e2e8f0; outline: none; cursor: pointer; min-width: 70px;
  }
  .pe-select:focus { border-color: #6366f1; }

  .pe-section { display: flex; flex-direction: column; gap: 6px; }
  .section-header { display: flex; align-items: center; justify-content: space-between; }

  .add-btn {
    font-size: 10px; padding: 3px 8px; border-radius: 4px;
    background: rgba(99,102,241,0.1); border: 1px solid rgba(99,102,241,0.3);
    color: #a5b4fc; cursor: pointer;
  }
  .add-btn:hover { background: rgba(99,102,241,0.2); }

  .inline-add { display: flex; gap: 4px; align-items: center; }

  .fields-list { display: flex; flex-direction: column; gap: 4px; }
  .field-row { display: flex; gap: 4px; align-items: center; }
  .field-name { flex: 1; font-size: 11px; padding: 6px 8px; }

  .remove-btn {
    background: none; border: none; color: #64748b; font-size: 11px;
    cursor: pointer; padding: 4px; border-radius: 4px;
  }
  .remove-btn:hover { color: #f87171; background: rgba(248,113,113,0.1); }

  .tag-list { display: flex; flex-wrap: wrap; gap: 4px; }
  .tag {
    display: flex; align-items: center; gap: 4px;
    font-size: 10px; padding: 3px 8px; border-radius: 6px;
    background: rgba(99,102,241,0.12); color: #a5b4fc;
    border: 1px solid rgba(99,102,241,0.25);
  }
  .tag-remove {
    background: none; border: none; color: #64748b; font-size: 9px;
    cursor: pointer; padding: 0 2px;
  }
  .tag-remove:hover { color: #f87171; }
</style>
