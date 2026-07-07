<script lang="ts">
  import { NODE_STYLES, getNodeStyle, getAnnotationDefs, type NodeKind, type IrGraph, type IrNode, type AnnotationSpec } from '$lib/types';
  import { irGraph, saveEdits, saving, saveError, paletteConfig, type EditOp } from '$lib/store';
  import { formatType } from '$lib/typeDisplay';
  import MethodEditor from '$lib/MethodEditor.svelte';
  import FieldsEditor from '$lib/FieldsEditor.svelte';
  import { BlockEditor, AnnotationEditor, EnumEditor } from '$lib/editors';
  import { irChildrenToExprs } from '$lib/editors/ir-convert';
  import { exprToVeil } from '$lib/editors/expr-serialize';
  import type { Expr } from '$lib/editors/expr-types';

  let { node, onUpdate, onClose, onImplement }: {
    node: { id: string; data: any };
    onUpdate: (id: string, data: any) => void;
    onClose: () => void;
    onImplement?: (implEntry: any, targetNodeName: string) => void;
  } = $props();

  let name = $state(node.data.label ?? '');
  let kind = $derived<NodeKind>(node.data.kind);
  let subkind = $derived<string | null>(node.data.subkind ?? null);
  let style = $derived(getNodeStyle(kind, subkind));
  let displayKind = $derived(subkind ?? kind);

  // Check if any impl-shaped construct in the palette targets this node's subkind.
  // If so, we can offer a "Create [impl label]" button.
  // tgt may be a comma-separated list (e.g. "Port, Repository").
  let implConstruct = $derived.by(() => {
    if (!subkind) return null;
    const config = $paletteConfig;
    if (!config || config.length === 0) return null;
    return config.find((c: any) => {
      if (!c.tgt) return false;
      const targets = c.tgt.split(',').map((t: string) => t.trim());
      return targets.includes(subkind);
    }) ?? null;
  });

  // Get children of this node from the graph
  let children = $state<IrNode[]>([]);
  $effect(() => {
    const g = $irGraph;
    if (!g) { children = []; return; }
    const nodeId = Number(node.id);
    if (isNaN(nodeId)) { children = []; return; }
    children = g.nodes.filter((n: IrNode) => n.metadata.parent === nodeId);
  });

  // Determine what kind of editor to show — driven by the core shape
  // (node kind), never by layer-specific subkind names.
  let editorType = $derived.by(() => {
    if (kind === 'Interface') return 'methods';
    if (kind === 'TypeDef') return 'fields';
    if (kind === 'Implementation') return 'adapter';
    if (kind === 'Flow') return 'flow';
    return 'generic';
  });

  // Parse children into editable method structures (for Port/Interface)
  let methods = $derived.by(() => {
    // First try: parse from InterfaceMethod child nodes
    const fromChildren = children
      .filter(c => c.kind === 'InterfaceMethod')
      .map(c => {
        const paramsRaw = c.metadata.properties.find(([k]) => k === 'params')?.[1] ?? '';
        const returnsRaw = c.metadata.properties.find(([k]) => k === 'returns')?.[1] ?? '';
        // Parse "(name: Type, name: Type)" into array
        const paramStr = paramsRaw.replace(/^\(|\)$/g, '');
        const params = paramStr ? paramStr.split(', ').map(p => {
          const [name, type] = p.split(': ');
          return { name: name?.trim() ?? '', type: type?.trim() ?? 'Str' };
        }) : [];
        return { name: c.name, params, returnType: returnsRaw };
      });
    if (fromChildren.length > 0) return fromChildren;

    // Fallback: parse from properties "methods" string (semicolon-separated signatures)
    const methodsStr = (node.data.properties ?? []).find(([k]: [string, string]) => k === 'methods')?.[1] ?? '';
    if (!methodsStr) return localMethods;
    return methodsStr.split('; ').filter(Boolean).map((sig: string) => {
      // Parse "name(params) -> RetType" or "name!(params) -> RetType"
      const parenIdx = sig.indexOf('(');
      if (parenIdx < 0) return { name: sig.trim(), params: [], returnType: '' };
      let methodName = sig.slice(0, parenIdx).trim();
      // Handle "!" suffix (fallible methods)
      let fallible = false;
      if (methodName.endsWith('!')) {
        fallible = true;
        methodName = methodName.slice(0, -1);
      }
      const closeIdx = sig.indexOf(')', parenIdx);
      const paramStr = sig.slice(parenIdx + 1, closeIdx);
      const params = paramStr ? paramStr.split(', ').map(p => {
        const [name, type] = p.split(': ');
        return { name: name?.trim() ?? '', type: type?.trim() ?? 'Str' };
      }) : [];
      let returnType = '';
      const arrowIdx = sig.indexOf('->', closeIdx);
      if (arrowIdx >= 0) {
        returnType = sig.slice(arrowIdx + 2).trim();
        if (fallible && !returnType.startsWith('Res!')) {
          returnType = returnType ? `Res!<${returnType}>` : 'Res!';
        }
      } else if (fallible) {
        returnType = 'Res!';
      }
      return { name: methodName, params, returnType };
    });
  });

  // Local methods state for unsaved nodes (no spanStart yet)
  let localMethods = $state<any[]>([]);

  // Method bodies for impl-shaped nodes (one Expr[] per inherited method)
  let implMethodBodies = $state<import('$lib/editors/expr-types').Expr[][]>([]);

  function handleImplMethodBodyChange(methodIndex: number, newExprs: import('$lib/editors/expr-types').Expr[]) {
    const updated = [...implMethodBodies];
    while (updated.length <= methodIndex) updated.push([]);
    updated[methodIndex] = newExprs;
    implMethodBodies = updated;
    // Log for now — will persist when backend supports it
    console.log('[VEIL Edit] Impl method body changed:', {
      nodeId: node.id,
      nodeName: name,
      methodIndex,
      exprCount: newExprs.length,
    });
  }

  // Parse children into editable field structures (for types)
  let fields = $derived.by(() => {
    return children
      .filter(c => c.kind === 'TypeDef')
      .map(c => ({ name: c.name, type: c.metadata.subkind ?? c.kind }));
  });

  // The AST span start identifies the construct on the server side. Edits are
  // no-ops (locally only) when it's missing (e.g. an unsaved dropped node).
  let spanStart = $derived<number | null>(node.data.spanStart ?? null);
  let layerProvided = $derived<boolean>(node.data.layerProvided ?? false);


  function handleBodyEdit(newExprs: Expr[]) {
    // Convert exprs back to VEIL source for logging/eventual persistence
    const veilSource = newExprs.map(e => exprToVeil(e)).join('\n');
    console.log('[VEIL Edit] Step body changed:', {
      nodeId: node.id,
      nodeName: name,
      exprCount: newExprs.length,
      veilSource,
    });
    // TODO: When backend edit API is connected, send the edit operation here.
    // For now, the edit is logged to console and visible in the code preview.
  }

  function handleMethodsChange(newMethods: any[]) {
    if (spanStart === null) {
      // Node not yet saved — update local state so the UI responds
      localMethods = newMethods;
      return;
    }
    persist({
      op: 'set_methods',
      span_start: spanStart,
      methods: newMethods.map(m => ({
        name: m.name,
        params: (m.params ?? []).map((p: any) => ({ name: p.name, type: p.type })),
        return_type: m.returnType ?? '',
      })),
    });
  }

  function handleFieldsChange(newFields: any[]) {
    if (spanStart === null) return;
    persist({
      op: 'set_fields',
      span_start: spanStart,
      fields: newFields.map(f => ({ name: f.name, type: f.type })),
    });
  }

  // Persist an edit to the server; the store updates irGraph/source/generated
  // on success so all panels reflect the change live.
  async function persist(edit: EditOp) {
    await saveEdits([edit]);
  }

  // Annotations
  let activeAnnotations = $state<Record<string, Record<string, string>>>(
    parseAnnotations(node.data.annotations ?? [])
  );

  // Reset state when node changes
  $effect(() => {
    name = node.data.label ?? '';
    activeAnnotations = parseAnnotations(node.data.annotations ?? []);
  });

  // Available annotations come from the layer (via /api/palette), keyed by the
  // construct subkind — zero hardcoded domain vocabulary in the viewer.
  let availableAnnotations = $derived<AnnotationSpec[]>(getAnnotationDefs(subkind));

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
            const paramName = def?.params[0] ?? 'value';
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
    commitAnnotations();
  }

  function updateAnnotationParam(annName: string, paramName: string, value: string) {
    if (activeAnnotations[annName]) {
      activeAnnotations[annName][paramName] = value;
      activeAnnotations = { ...activeAnnotations };
      commitAnnotations();
    }
  }

  // Local echo — updates the on-canvas node immediately for responsiveness.
  // Persistence to the server happens on commit (blur / annotation toggle).
  function save() {
    onUpdate(node.id, {
      ...node.data,
      label: name,
      annotations: serializeAnnotations(),
    });
  }

  // Persist a rename when the name field loses focus (avoids a round-trip per
  // keystroke). No-op if the name is unchanged or the node isn't yet saved.
  function commitName() {
    save();
    if (spanStart === null) return;
    if (name === (node.data.label ?? '')) return;
    persist({ op: 'rename', span_start: spanStart, name });
  }

  // Persist the current annotation set to the server.
  function commitAnnotations() {
    save();
    if (spanStart === null) return;
    persist({ op: 'set_annotations', span_start: spanStart, annotations: serializeAnnotations() });
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
      <input type="text" class="pe-input" bind:value={name} oninput={save} onblur={commitName} placeholder="Enter name..." />
    </label>

    {#if layerProvided}
      <div class="pe-note pe-note-info">Layer-provided (read-only infrastructure)</div>
    {/if}
    {#if $saving}
      <div class="pe-note">Saving…</div>
    {:else if $saveError}
      <div class="pe-note pe-note-error">Save failed: {$saveError}</div>
    {/if}

    <!-- Implement button: shown on trait-shaped nodes that have a paired impl construct -->
    {#if implConstruct && onImplement}
      <button
        class="pe-implement-btn"
        onclick={() => onImplement(implConstruct, name)}
        title="Create a {implConstruct.label} that implements this {displayKind}"
      >
        <span class="impl-icon">{implConstruct.icon}</span>
        Create {implConstruct.label}
      </button>
    {/if}

    <!-- Type-specific editor -->
    {#if editorType === 'methods'}
      <MethodEditor methods={methods} onChange={handleMethodsChange} />
      <!-- Method bodies (concrete default implementations) -->
      {#if methods.length > 0}
        <div class="pe-section">
          <span class="label-text">Method Bodies</span>
          <div class="impl-methods">
            {#each methods as method, mi}
              <div class="impl-method-card">
                <div class="impl-method-sig">
                  <span class="impl-method-icon">⚡</span>
                  <code class="impl-method-name">{method.name || '(unnamed)'}({method.params?.map((p: any) => `${p.name}: ${p.type}`).join(', ') ?? ''}){method.returnType ? ` -> ${method.returnType}` : ''}</code>
                </div>
                <BlockEditor
                  exprs={implMethodBodies[mi] ?? []}
                  onChange={(newExprs) => handleImplMethodBodyChange(mi, newExprs)}
                  depth={0}
                  label="body"
                />
              </div>
            {/each}
          </div>
        </div>
      {/if}
    {:else if editorType === 'adapter'}
      <!-- Impl-shaped: show inherited methods with editable bodies -->
      {@const implementsName = node.data.properties?.find(([k]: [string, string]) => k === 'implements')?.[1] ?? ''}
      {@const targetNode = implementsName && $irGraph ? $irGraph.nodes.find((n: any) => n.name === implementsName) : null}
      {@const targetMethods = targetNode?.metadata?.properties?.find(([k]: [string, string]) => k === 'methods')?.[1] ?? ''}
      {@const inheritedMethods = targetMethods ? targetMethods.split('; ').filter(Boolean).map((sig: string) => {
        const parenIdx = sig.indexOf('(');
        if (parenIdx < 0) return { name: sig.trim(), signature: sig.trim() };
        let methodName = sig.slice(0, parenIdx).trim();
        if (methodName.endsWith('!')) methodName = methodName.slice(0, -1);
        return { name: methodName, signature: sig.trim() };
      }) : []}

      {#if implementsName}
        <div class="pe-section">
          <span class="label-text">Implements</span>
          <div class="pe-note pe-note-info">{implementsName}</div>
        </div>
      {/if}

      <div class="pe-section">
        <span class="label-text">Methods</span>
        {#if inheritedMethods.length > 0}
          <div class="impl-methods">
            {#each inheritedMethods as method, mi}
              <div class="impl-method-card">
                <div class="impl-method-sig">
                  <span class="impl-method-icon">⚡</span>
                  <code class="impl-method-name">{method.signature}</code>
                </div>
                <BlockEditor
                  exprs={implMethodBodies[mi] ?? []}
                  onChange={(newExprs) => handleImplMethodBodyChange(mi, newExprs)}
                  depth={0}
                  label="body"
                />
              </div>
            {/each}
          </div>
        {:else}
          <div class="pe-empty">No methods inherited. Add methods to the target interface first.</div>
        {/if}
      </div>

      <!-- Additional methods (not from target) -->
      <MethodEditor methods={localMethods} onChange={handleMethodsChange} />
    {:else if editorType === 'fields'}
      <!-- Fields from properties -->
      {@const fieldsRaw = node.data.properties?.find(([k]: [string, string]) => k === 'fields')?.[1] ?? ''}
      {@const parsedFields = fieldsRaw ? fieldsRaw.split(', ').map((f: string) => {
        const [name, type] = f.split(': ');
        return { name: name?.trim() ?? '', type: type?.trim() ?? 'Str' };
      }) : []}
      <FieldsEditor fields={parsedFields} label="Fields" onChange={handleFieldsChange} />

      <!-- Business logic methods (fn:* properties) -->
      {@const fnProps = (node.data.properties ?? []).filter(([k]: [string, string]) => k.startsWith('fn:'))}
      {#if fnProps.length > 0}
        <div class="pe-section">
          <span class="label-text">Methods</span>
          <div class="methods-list">
            {#each fnProps as [key, sig]}
              <div class="method-item">
                <span class="method-icon">⚡</span>
                <code class="method-sig">{key.slice(3)}{sig}</code>
              </div>
            {/each}
          </div>
        </div>
      {/if}

      <!-- Show children (events, commands) if any -->
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
                        <span class="sig-part">{key}: {formatType(value)}</span>
                      {/each}
                    </span>
                  {/if}
                </div>
              </div>
            {/each}
          </div>
        </div>
      {/if}
    {:else if children.length > 0}
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

    <!-- Expression Editor for flow/step bodies -->
    {#if kind === 'Step' || kind === 'Action' || kind === 'Flow'}
      <div class="pe-section">
        <span class="label-text">
          {kind === 'Step' ? 'Step Body' : kind === 'Action' ? 'Expression' : 'Flow Body'}
        </span>
        <div class="expr-editor-container">
          {#if children.length > 0}
            {@const bodyExprs = irChildrenToExprs(children)}
            <BlockEditor
              exprs={bodyExprs}
              onChange={(newExprs) => handleBodyEdit(newExprs)}
              depth={0}
            />
          {:else}
            <p class="pe-empty">No expressions. Drill into this node to add them.</p>
          {/if}
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
                <span class="ann-desc">{annDef.desc}</span>
              </label>
              {#if isActive && annDef.params.length > 0}
                <div class="annotation-params">
                  {#each annDef.params as param}
                    <div class="param-row">
                      <span class="param-label">{param}:</span>
                      <input
                        type="text"
                        class="pe-input small"
                        value={activeAnnotations[annDef.name]?.[param] ?? ''}
                        oninput={(e) => updateAnnotationParam(annDef.name, param, e.currentTarget.value)}
                      />
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

  .methods-list {
    display: flex;
    flex-direction: column;
    gap: 4px;
    margin-top: 4px;
  }

  .method-item {
    display: flex;
    align-items: center;
    gap: 6px;
    padding: 4px 8px;
    background: rgba(82, 82, 82, 0.1);
    border-radius: 4px;
    border-left: 2px solid #737373;
  }

  .method-icon {
    font-size: 12px;
  }

  .method-sig {
    font-family: 'JetBrains Mono', monospace;
    font-size: 11px;
    color: #a3a3a3;
  }
    position: absolute;
    top: 12px;
    right: 12px;
    width: 300px;
    max-height: calc(100vh - 100px);
    overflow-y: auto;
    background: rgba(20, 20, 35, 0.98);
    border: 1px solid #2e2e2e;
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
    border-bottom: 1px solid #2e2e2e;
    position: sticky;
    top: 0;
    background: rgba(20, 20, 35, 0.98);
    border-radius: 12px 12px 0 0;
  }

  .pe-title-row { display: flex; align-items: center; gap: 8px; }
  .pe-icon { font-size: 16px; }
  .pe-kind { font-size: 11px; font-weight: 700; text-transform: uppercase; letter-spacing: 0.5px; }

  .pe-close {
    background: none; border: none; color: #737373; font-size: 14px;
    cursor: pointer; padding: 4px 8px; border-radius: 4px;
  }
  .pe-close:hover { color: #e5e5e5; background: rgba(255,255,255,0.05); }

  .pe-body { padding: 12px 16px; display: flex; flex-direction: column; gap: 14px; }
  .pe-label { display: flex; flex-direction: column; gap: 4px; }
  .label-text { font-size: 10px; text-transform: uppercase; letter-spacing: 0.5px; color: #737373; font-weight: 600; }

  .pe-note { font-size: 11px; color: #a3a3a3; padding: 4px 2px; }
  .pe-note-error { color: #f87171; }
  .pe-note-info { color: #d4d4d4; }

  .pe-input {
    background: rgba(0,0,0,0.3); border: 1px solid #2e2e2e; border-radius: 6px;
    padding: 8px 10px; font-size: 13px; color: #e5e5e5; outline: none; transition: border-color 0.15s;
    width: 100%; box-sizing: border-box;
  }
  .pe-input:focus { border-color: #737373; }
  .pe-input::placeholder { color: #525252; }
  .pe-input.small { padding: 6px 8px; font-size: 11px; flex: 1; }

  .pe-select {
    background: rgba(0,0,0,0.3); border: 1px solid #2e2e2e; border-radius: 6px;
    padding: 6px 8px; font-size: 11px; color: #e5e5e5; outline: none; cursor: pointer; min-width: 70px;
  }
  .pe-select:focus { border-color: #737373; }

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
  .child-name { color: #e5e5e5; font-weight: 500; }
  .child-sig { display: flex; flex-direction: column; gap: 1px; }
  .sig-part { color: #737373; font-size: 10px; font-family: monospace; word-break: break-all; }

  .props-list { display: flex; flex-direction: column; gap: 3px; }
  .prop-item {
    display: flex; gap: 6px; align-items: baseline;
    padding: 4px 8px; border-radius: 4px;
    background: rgba(0,0,0,0.15);
    font-size: 11px;
  }
  .prop-key { color: #737373; font-family: monospace; }
  .prop-value { color: #cbd5e1; font-family: monospace; word-break: break-all; }

  .annotations-list { display: flex; flex-direction: column; gap: 2px; }
  .annotation-item { border-radius: 6px; overflow: hidden; }
  .annotation-checkbox {
    display: flex; align-items: center; gap: 8px;
    padding: 6px 8px; border-radius: 6px; cursor: pointer; transition: background 0.15s;
  }
  .annotation-checkbox:hover { background: rgba(99,102,241,0.06); }
  .annotation-checkbox input[type="checkbox"] { accent-color: #737373; width: 14px; height: 14px; cursor: pointer; }
  .ann-name { font-size: 11px; font-weight: 600; color: #d4d4d4; font-family: monospace; }
  .ann-desc { font-size: 9px; color: #525252; margin-left: auto; }
  .annotation-params {
    padding: 6px 8px 8px 30px; display: flex; flex-direction: column; gap: 5px;
    animation: slideDown 0.15s ease-out;
  }
  @keyframes slideDown { from { opacity: 0; transform: translateY(-4px); } to { opacity: 1; transform: translateY(0); } }
  .param-row { display: flex; align-items: center; gap: 6px; }
  .param-label { font-size: 10px; color: #737373; min-width: 50px; }
  .expr-editor-container {
    padding: 4px 0;
  }

  .pe-hint {
    font-size: 11px;
    color: #737373;
    margin: 0 0 8px 0;
  }

  .pe-empty {
    font-size: 11px;
    color: #525252;
    font-style: italic;
    margin: 0;
  }

  .impl-methods { display: flex; flex-direction: column; gap: 6px; }
  .impl-method-card {
    background: rgba(0,0,0,0.2);
    border: 1px solid #2e2e2e;
    border-radius: 8px;
    padding: 8px;
    display: flex;
    flex-direction: column;
    gap: 6px;
  }
  .impl-method-sig {
    display: flex; align-items: center; gap: 6px;
    padding-bottom: 4px;
    border-bottom: 1px solid rgba(255,255,255,0.05);
  }
  .impl-method-icon { font-size: 12px; }
  .impl-method-name {
    font-family: 'JetBrains Mono', monospace;
    font-size: 11px; color: #d4d4d4; word-break: break-all;
  }
  .impl-method-body {
    padding: 4px 8px;
    background: rgba(0,0,0,0.15);
    border-radius: 4px;
    border-left: 2px solid #334155;
    min-height: 32px;
  }

  .pe-implement-btn {
    display: flex;
    align-items: center;
    gap: 8px;
    width: 100%;
    padding: 10px 12px;
    border-radius: 8px;
    background: rgba(168, 85, 247, 0.1);
    border: 1px solid rgba(168, 85, 247, 0.3);
    color: #a3a3a3;
    font-size: 12px;
    font-weight: 600;
    cursor: pointer;
    transition: all 0.15s;
  }
  .pe-implement-btn:hover {
    background: rgba(168, 85, 247, 0.2);
    border-color: rgba(168, 85, 247, 0.5);
    transform: translateY(-1px);
  }
  .impl-icon { font-size: 14px; }

  .step-body-preview {
    display: flex;
    flex-direction: column;
    gap: 3px;
  }

  .body-line {
    display: flex;
    align-items: center;
    gap: 6px;
    padding: 3px 6px;
    background: rgba(0, 0, 0, 0.15);
    border-radius: 3px;
    border-left: 2px solid #334155;
  }

  .body-icon { font-size: 12px; }

  .body-code {
    font-family: 'JetBrains Mono', monospace;
    font-size: 11px;
    color: #a5f3fc;
  }
</style>
