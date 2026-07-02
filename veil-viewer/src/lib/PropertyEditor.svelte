<script lang="ts">
  import { NODE_STYLES, getNodeStyle, type NodeKind, type IrGraph, type IrNode } from '$lib/types';
  import { ANNOTATION_SCHEMA, type AnnotationDef } from '$lib/annotations';
  import { irGraph } from '$lib/store';

  let { node, onUpdate, onClose }: {
    node: { id: string; data: any };
    onUpdate: (id: string, data: any) => void;
    onClose: () => void;
  } = $props();

  let name = $state(node.data.label ?? '');
  let kind = $derived<NodeKind>(node.data.kind);
  let subkind = $derived<string | null>(node.data.subkind ?? null);
  let style = $derived(getNodeStyle(kind, subkind));
  let displayKind = $derived(subkind ?? kind);

  // Get children of this node from the graph
  let children = $state<IrNode[]>([]);
  $effect(() => {
    const g = $irGraph;
    if (!g) { children = []; return; }
    const nodeId = Number(node.id);
    if (isNaN(nodeId)) { children = []; return; }
    children = g.nodes.filter((n: IrNode) => n.metadata.parent === nodeId);
  });

  // Annotations
  let activeAnnotations = $state<Record<string, Record<string, string>>>(
    parseAnnotations(node.data.annotations ?? [])
  );

  // Reset state when node changes
  $effect(() => {
    name = node.data.label ?? '';
    activeAnnotations = parseAnnotations(node.data.annotations ?? []);
  });

  // Available annotations for this node kind
  let availableAnnotations = $derived<AnnotationDef[]>(
    ANNOTATION_SCHEMA[subkind ?? kind] ?? ANNOTATION_SCHEMA[kind] ?? []
  );

  function parseAnnotations(anns: string[]): Record<string, Record<string, string>> {
    const result: Record<string, Record<string, string>> = {};
    for (const ann of anns) {
      const clean = ann.startsWith('@') ? ann.slice(1) : ann;
      const parenIdx = clean.indexOf('(');
      if (parenIdx >= 0) {
        const annName = clean.slice(0, parenIdx);
        const argsStr = clean.slice(parenIdx + 1, -1);
        const args: Record<string, string> = {};
        for (const part of argsStr.split(',')) {
          const trimmed = part.trim();
          const eqIdx = trimmed.indexOf('=');
          if (eqIdx >= 0) {
            args[trimmed.slice(0, eqIdx).trim()] = trimmed.slice(eqIdx + 1).trim().replace(/"/g, '');
          } else if (trimmed) {
            const def = availableAnnotations.find(a => a.name === annName);
            const paramName = def?.params[0]?.name ?? 'value';
            args[paramName] = trimmed;
          }
        }
        result[annName] = args;
      } else {
        result[clean] = {};
      }
    }
    return result;
  }

  function serializeAnnotations(): string[] {
    const result: string[] = [];
    for (const [annName, args] of Object.entries(activeAnnotations)) {
      const entries = Object.entries(args).filter(([_, v]) => v !== '');
      if (entries.length === 0) {
        result.push(`@${annName}`);
      } else if (entries.length === 1) {
        const def = availableAnnotations.find(a => a.name === annName);
        if (def?.params.length === 1) {
          result.push(`@${annName}(${entries[0][1]})`);
        } else {
          result.push(`@${annName}(${entries[0][0]}="${entries[0][1]}")`);
        }
      } else {
        const parts = entries.map(([k, v]) => `${k}="${v}"`).join(', ');
        result.push(`@${annName}(${parts})`);
      }
    }
    return result;
  }

  function toggleAnnotation(annName: string, checked: boolean) {
    if (checked) {
      activeAnnotations[annName] = {};
    } else {
      delete activeAnnotations[annName];
    }
    activeAnnotations = { ...activeAnnotations };
    save();
  }

  function updateAnnotationParam(annName: string, paramName: string, value: string) {
    if (activeAnnotations[annName]) {
      activeAnnotations[annName][paramName] = value;
      activeAnnotations = { ...activeAnnotations };
      save();
    }
  }

  function save() {
    onUpdate(node.id, {
      ...node.data,
      label: name,
      annotations: serializeAnnotations(),
    });
  }
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

    <!-- Children: show methods, fields, events, etc. based on what this node contains -->
    {#if children.length > 0}
      <div class="pe-section">
        <span class="label-text">Contains</span>
        <div class="children-list">
          {#each children as child}
            <div class="child-item">
              <span class="child-icon">{getNodeStyle(child.kind, child.metadata.subkind)?.icon ?? '•'}</span>
              <div class="child-info">
                <span class="child-name">{child.name}</span>
                {#if child.metadata.properties.length > 0}
                  <span class="child-sig">
                    {#each child.metadata.properties as [key, value]}
                      <span class="sig-part">{value}</span>
                    {/each}
                  </span>
                {/if}
              </div>
            </div>
          {/each}
        </div>
      </div>
    {/if}

    <!-- Properties from IR -->
    {#if node.data.properties && node.data.properties.length > 0}
      <div class="pe-section">
        <span class="label-text">Details</span>
        <div class="props-list">
          {#each node.data.properties as [key, value]}
            <div class="prop-item">
              <span class="prop-key">{key}:</span>
              <span class="prop-value">{value}</span>
            </div>
          {/each}
        </div>
      </div>
    {/if}

    <!-- Annotations as checkboxes -->
    {#if availableAnnotations.length > 0}
      <div class="pe-section">
        <span class="label-text">Annotations</span>
        <div class="annotations-list">
          {#each availableAnnotations as annDef}
            {@const isActive = annDef.name in activeAnnotations}
            <div class="annotation-item">
              <label class="annotation-checkbox">
                <input
                  type="checkbox"
                  checked={isActive}
                  onchange={(e) => toggleAnnotation(annDef.name, e.currentTarget.checked)}
                />
                <span class="ann-name">@{annDef.name}</span>
                <span class="ann-desc">{annDef.description}</span>
              </label>
              {#if isActive && annDef.params.length > 0}
                <div class="annotation-params">
                  {#each annDef.params as param}
                    <div class="param-row">
                      <span class="param-label">{param.name}:</span>
                      {#if param.type === 'select' && param.options}
                        <select
                          class="pe-select"
                          value={activeAnnotations[annDef.name]?.[param.name] ?? ''}
                          onchange={(e) => updateAnnotationParam(annDef.name, param.name, e.currentTarget.value)}
                        >
                          <option value="">—</option>
                          {#each param.options as opt}
                            <option value={opt}>{opt}</option>
                          {/each}
                        </select>
                      {:else}
                        <input
                          type={param.type === 'number' ? 'number' : 'text'}
                          class="pe-input small"
                          value={activeAnnotations[annDef.name]?.[param.name] ?? ''}
                          placeholder={param.placeholder ?? ''}
                          oninput={(e) => updateAnnotationParam(annDef.name, param.name, e.currentTarget.value)}
                        />
                      {/if}
                    </div>
                  {/each}
                </div>
              {/if}
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
    width: 300px;
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

  .children-list { display: flex; flex-direction: column; gap: 3px; }
  .child-item {
    display: flex; align-items: center; gap: 6px;
    padding: 6px 8px; border-radius: 6px;
    background: rgba(0,0,0,0.2); border: 1px solid rgba(255,255,255,0.05);
    font-size: 11px;
  }
  .child-icon { font-size: 12px; }
  .child-info { display: flex; flex-direction: column; gap: 1px; flex: 1; min-width: 0; }
  .child-name { color: #e2e8f0; font-weight: 500; }
  .child-sig { display: flex; flex-direction: column; gap: 1px; }
  .sig-part { color: #64748b; font-size: 10px; font-family: monospace; word-break: break-all; }

  .props-list { display: flex; flex-direction: column; gap: 3px; }
  .prop-item {
    display: flex; gap: 6px; align-items: baseline;
    padding: 4px 8px; border-radius: 4px;
    background: rgba(0,0,0,0.15);
    font-size: 11px;
  }
  .prop-key { color: #64748b; font-family: monospace; }
  .prop-value { color: #cbd5e1; font-family: monospace; word-break: break-all; }

  .annotations-list { display: flex; flex-direction: column; gap: 2px; }
  .annotation-item { border-radius: 6px; overflow: hidden; }
  .annotation-checkbox {
    display: flex; align-items: center; gap: 8px;
    padding: 6px 8px; border-radius: 6px; cursor: pointer; transition: background 0.15s;
  }
  .annotation-checkbox:hover { background: rgba(99,102,241,0.06); }
  .annotation-checkbox input[type="checkbox"] { accent-color: #6366f1; width: 14px; height: 14px; cursor: pointer; }
  .ann-name { font-size: 11px; font-weight: 600; color: #a5b4fc; font-family: monospace; }
  .ann-desc { font-size: 9px; color: #475569; margin-left: auto; }
  .annotation-params {
    padding: 6px 8px 8px 30px; display: flex; flex-direction: column; gap: 5px;
    animation: slideDown 0.15s ease-out;
  }
  @keyframes slideDown { from { opacity: 0; transform: translateY(-4px); } to { opacity: 1; transform: translateY(0); } }
  .param-row { display: flex; align-items: center; gap: 6px; }
  .param-label { font-size: 10px; color: #64748b; min-width: 50px; }
</style>
