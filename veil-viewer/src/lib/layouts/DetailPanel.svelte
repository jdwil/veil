<script lang="ts">
  /**
   * DetailPanel — main content area when a node is selected in tree/flat layouts.
   *
   * Displays editable properties (name, annotations, fields, methods, body) for
   * the selected construct. Replaces the floating PropertyEditor overlay when
   * using non-flow layouts.
   *
   * Follows ADR-viewer-editors.md: body review via BlockEditor only.
   * Zero domain knowledge — all presentation is layer-driven.
   */
  import { NODE_STYLES, getNodeStyle, getAnnotationDefs, type NodeKind, type IrGraph, type IrNode, type AnnotationSpec } from '$lib/types';
  import { irGraph, saveEdits, saving, saveError, paletteConfig, selectedNodeId, diagnostics, type EditOp } from '$lib/store';
  import { formatType } from '$lib/typeDisplay';
  import { isCriticalNode } from '$lib/lenses';
  import { BlockEditor } from '$lib/editors';
  import { irChildrenToExprs } from '$lib/editors/ir-convert';
  import { exprToVeil } from '$lib/editors/expr-serialize';
  import type { Expr } from '$lib/editors/expr-types';
  import type { PresentationModel } from '$lib/presentation';
  import { get } from 'svelte/store';

  let { graph, presentationModel }: {
    graph: IrGraph;
    presentationModel: PresentationModel | null;
  } = $props();

  // Resolve the selected node from the IR graph
  let selectedIrNode = $derived.by((): IrNode | null => {
    const id = $selectedNodeId;
    if (!id) return null;
    return graph.nodes.find(n => n.id === Number(id)) ?? null;
  });

  let kind = $derived<NodeKind | null>(selectedIrNode?.kind as NodeKind ?? null);
  let subkind = $derived<string | null>(selectedIrNode?.metadata.subkind ?? null);
  let style = $derived(kind ? getNodeStyle(kind, subkind) : null);
  let displayKind = $derived(subkind ?? kind ?? '');

  // Node identity for edits
  let spanStart = $derived<number | null>(selectedIrNode?.span.start ?? null);
  let layerProvided = $derived<boolean>(
    selectedIrNode?.metadata.annotations.includes('layer-provided') ?? false
  );

  // Editable name
  let name = $state('');
  $effect(() => {
    name = selectedIrNode?.name ?? '';
  });

  // Children of the selected node
  let children = $derived.by((): IrNode[] => {
    if (!selectedIrNode) return [];
    return graph.nodes
      .filter(n => n.metadata.parent === selectedIrNode!.id)
      .sort((a, b) => a.span.start - b.span.start);
  });

  // Methods (InterfaceMethod children)
  let methods = $derived.by(() => {
    const fromChildren = children
      .filter(c => c.kind === 'InterfaceMethod')
      .map(c => {
        const paramsRaw = c.metadata.properties.find(([k]) => k === 'params')?.[1] ?? '';
        const returnsRaw = c.metadata.properties.find(([k]) => k === 'returns')?.[1] ?? '';
        const sig = c.metadata.properties.find(([k]) => k === 'signature')?.[1] ?? '';
        const paramStr = paramsRaw.replace(/^\(|\)$/g, '');
        const params = paramStr ? paramStr.split(', ').map(p => {
          const [pName, pType] = p.split(': ');
          return { name: pName?.trim() ?? '', type: pType?.trim() ?? 'Str' };
        }) : [];
        const invariants = c.metadata.annotations
          .filter(a => a.includes('invariant') || a.startsWith('@invariant'))
          .map(a => a.startsWith('@') ? a : `@${a}`);
        return {
          id: c.id,
          name: c.name,
          params,
          returnType: returnsRaw,
          signature: sig || `${paramsRaw}${returnsRaw ? ` -> ${returnsRaw}` : ''}`,
          annotations: c.metadata.annotations,
          invariants,
          hasBody: c.metadata.properties.some(([k, v]) => k === 'has_body' && v === 'true'),
        };
      });
    if (fromChildren.length > 0) return fromChildren;

    // Fallback: parse from "methods" property
    const methodsStr = selectedIrNode?.metadata.properties.find(([k]) => k === 'methods')?.[1] ?? '';
    if (!methodsStr) return [];
    return methodsStr.split('; ').filter(Boolean).map(sig => {
      const parenIdx = sig.indexOf('(');
      if (parenIdx < 0) return { id: -1, name: sig.trim(), params: [], returnType: '', signature: sig, annotations: [], invariants: [], hasBody: false };
      let methodName = sig.slice(0, parenIdx).trim();
      let fallible = false;
      if (methodName.endsWith('!')) { fallible = true; methodName = methodName.slice(0, -1); }
      const closeIdx = sig.indexOf(')', parenIdx);
      const paramStr = sig.slice(parenIdx + 1, closeIdx);
      const params = paramStr ? paramStr.split(', ').map(p => {
        const [pn, pt] = p.split(': ');
        return { name: pn?.trim() ?? '', type: pt?.trim() ?? 'Str' };
      }) : [];
      let returnType = '';
      const arrowIdx = sig.indexOf('->', closeIdx);
      if (arrowIdx >= 0) returnType = sig.slice(arrowIdx + 2).trim();
      if (fallible && !returnType.startsWith('Res!')) returnType = returnType ? `Res!<${returnType}>` : 'Res!';
      return { id: -1, name: methodName, params, returnType, signature: sig, annotations: [], invariants: [], hasBody: false };
    });
  });

  // Fields
  let fieldsRaw = $derived(selectedIrNode?.metadata.properties.find(([k]) => k === 'fields')?.[1] ?? '');
  let fields = $derived.by(() => {
    if (!fieldsRaw) return [];
    return fieldsRaw.split(', ').filter(Boolean).map(f => {
      const [fname, ftype] = f.split(': ');
      return { name: fname?.trim() ?? '', type: ftype?.trim() ?? '' };
    });
  });

  // Annotations
  let availableAnnotations = $derived<AnnotationSpec[]>(getAnnotationDefs(subkind));
  let activeAnnotations = $state<Record<string, Record<string, string>>>({});

  $effect(() => {
    if (!selectedIrNode) { activeAnnotations = {}; return; }
    activeAnnotations = parseAnnotations(selectedIrNode.metadata.annotations);
  });

  function parseAnnotations(anns: string[]): Record<string, Record<string, string>> {
    const result: Record<string, Record<string, string>> = {};
    for (const ann of anns) {
      if (ann === 'layer-provided') continue;
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

  // Diagnostics for this node
  let nodeDiagnostics = $derived(
    $diagnostics.filter(d => d.node_id === selectedIrNode?.id)
  );

  // Doc (from layer)
  let doc = $derived(selectedIrNode?.metadata.doc ?? null);

  // Body editor: for Step / Flow / InterfaceMethod nodes
  let showBodyEditor = $derived(
    kind === 'Step' || kind === 'Flow' || kind === 'InterfaceMethod'
  );

  let bodyExprs = $derived.by((): Expr[] => {
    if (!selectedIrNode || !showBodyEditor) return [];
    const bodyChildren = children.filter(c =>
      c.kind === 'Step' || c.kind === 'Action' || c.kind === 'MatchDecision'
    );
    return irChildrenToExprs(bodyChildren);
  });

  // Properties (non-standard ones to display read-only)
  let properties = $derived.by(() => {
    if (!selectedIrNode) return [];
    const skip = new Set(['fields', 'methods', 'signature', 'params', 'returns', 'has_body']);
    return selectedIrNode.metadata.properties.filter(([k]) => !skip.has(k));
  });

  // Persist helpers
  async function persist(edit: EditOp) {
    await saveEdits([edit]);
  }

  function commitName() {
    if (spanStart === null || !selectedIrNode) return;
    if (name === selectedIrNode.name) return;
    persist({ op: 'rename', span_start: spanStart, name });
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

  function commitAnnotations() {
    if (spanStart === null) return;
    persist({ op: 'set_annotations', span_start: spanStart, annotations: serializeAnnotations() });
  }

  function handleBodyEdit(newExprs: Expr[]) {
    if (spanStart === null) return;
    const veilSource = newExprs.map(e => exprToVeil(e));
    persist({ op: 'set_body', span_start: spanStart, body: veilSource });
  }

  function selectMethod(methodId: number) {
    if (methodId > 0) {
      selectedNodeId.set(String(methodId));
    }
  }
</script>

{#if !selectedIrNode}
  <div class="detail-empty">
    <div class="detail-empty-content">
      <span class="detail-empty-icon">👈</span>
      <p>Select an item to view details</p>
    </div>
  </div>
{:else}
  <div class="detail-panel">
    <!-- Header -->
    <header class="detail-header">
      <div class="detail-header-main">
        {#if style}
          <span class="detail-icon">{style.icon}</span>
        {/if}
        <span class="detail-kind-badge">{displayKind}</span>
        {#if layerProvided}
          <span class="detail-badge layer-badge">layer-provided</span>
        {/if}
        {#if isCriticalNode(selectedIrNode, presentationModel, $diagnostics)}
          <span class="detail-badge critical-badge">critical</span>
        {/if}
      </div>
      {#if $saving}
        <span class="detail-save-status saving">Saving…</span>
      {/if}
      {#if $saveError}
        <span class="detail-save-status error">{$saveError}</span>
      {/if}
    </header>

    <!-- Name -->
    <section class="detail-section">
      <label class="detail-label" for="detail-name">Name</label>
      {#if layerProvided}
        <div class="detail-name-display">{name}</div>
      {:else}
        <input
          id="detail-name"
          class="detail-input"
          type="text"
          bind:value={name}
          onblur={commitName}
          onkeydown={(e) => { if (e.key === 'Enter') e.currentTarget.blur(); }}
        />
      {/if}
    </section>

    <!-- Doc -->
    {#if doc}
      <section class="detail-section">
        <div class="detail-doc">{doc}</div>
      </section>
    {/if}

    <!-- Diagnostics -->
    {#if nodeDiagnostics.length > 0}
      <section class="detail-section detail-diagnostics">
        <h3 class="detail-section-title">Diagnostics</h3>
        {#each nodeDiagnostics as diag}
          <div class="detail-diag" class:diag-error={diag.severity === 'error'} class:diag-warning={diag.severity === 'warning'}>
            <span class="diag-severity">{diag.severity === 'error' ? '✗' : '⚠'}</span>
            <span class="diag-message">{diag.message}</span>
            {#if diag.hint}
              <span class="diag-hint">{diag.hint}</span>
            {/if}
          </div>
        {/each}
      </section>
    {/if}

    <!-- Fields -->
    {#if fields.length > 0}
      <section class="detail-section">
        <h3 class="detail-section-title">Fields <span class="section-count">{fields.length}</span></h3>
        <div class="detail-fields">
          {#each fields as field}
            <div class="detail-field-row">
              <span class="field-name">{field.name}</span>
              <span class="field-type">{field.type}</span>
            </div>
          {/each}
        </div>
      </section>
    {/if}

    <!-- Methods -->
    {#if methods.length > 0}
      <section class="detail-section">
        <h3 class="detail-section-title">Methods <span class="section-count">{methods.length}</span></h3>
        <div class="detail-methods">
          {#each methods as method}
            <button
              class="detail-method-row"
              class:has-body={method.hasBody}
              onclick={() => selectMethod(method.id)}
            >
              <span class="method-name">{method.name}</span>
              <span class="method-sig">
                ({method.params.map(p => `${p.name}: ${p.type}`).join(', ')})
                {#if method.returnType}
                  → {method.returnType}
                {/if}
              </span>
              {#if method.invariants.length > 0}
                {#each method.invariants as inv}
                  <span class="method-invariant">{inv}</span>
                {/each}
              {/if}
            </button>
          {/each}
        </div>
      </section>
    {/if}

    <!-- Body Editor -->
    {#if showBodyEditor && !layerProvided}
      <section class="detail-section">
        <h3 class="detail-section-title">Body</h3>
        <div class="detail-body-editor">
          <BlockEditor
            exprs={bodyExprs}
            onChange={handleBodyEdit}
          />
        </div>
      </section>
    {/if}

    <!-- Annotations -->
    {#if availableAnnotations.length > 0 && !layerProvided}
      <section class="detail-section">
        <h3 class="detail-section-title">Annotations</h3>
        <div class="detail-annotations">
          {#each availableAnnotations as annDef}
            {@const active = annDef.name in activeAnnotations}
            <div class="annotation-item">
              <label class="annotation-toggle">
                <input
                  type="checkbox"
                  checked={active}
                  onchange={(e) => toggleAnnotation(annDef.name, e.currentTarget.checked)}
                />
                <span class="annotation-name">@{annDef.name}</span>
                {#if annDef.desc}
                  <span class="annotation-desc">{annDef.desc}</span>
                {/if}
              </label>
              {#if active && annDef.params.length > 0}
                <div class="annotation-params">
                  {#each annDef.params as param}
                    <div class="annotation-param-row">
                      <label class="annotation-param-label">{param}</label>
                      <input
                        class="detail-input annotation-param-input"
                        type="text"
                        value={activeAnnotations[annDef.name]?.[param] ?? ''}
                        onblur={(e) => updateAnnotationParam(annDef.name, param, e.currentTarget.value)}
                      />
                    </div>
                  {/each}
                </div>
              {/if}
            </div>
          {/each}
        </div>
      </section>
    {/if}

    <!-- Properties (read-only) -->
    {#if properties.length > 0}
      <section class="detail-section">
        <h3 class="detail-section-title">Properties</h3>
        <div class="detail-properties">
          {#each properties as [key, value]}
            <div class="detail-prop-row">
              <span class="prop-key">{key}</span>
              <span class="prop-value">{value}</span>
            </div>
          {/each}
        </div>
      </section>
    {/if}
  </div>
{/if}

<style>
  .detail-empty {
    display: flex;
    align-items: center;
    justify-content: center;
    height: 100%;
    min-height: 200px;
  }

  .detail-empty-content {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 8px;
    color: var(--veil-text-faint);
  }

  .detail-empty-icon {
    font-size: 24px;
    opacity: 0.5;
  }

  .detail-panel {
    padding: 16px 20px;
    overflow-y: auto;
    height: 100%;
    display: flex;
    flex-direction: column;
    gap: 16px;
  }

  .detail-header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 8px;
  }

  .detail-header-main {
    display: flex;
    align-items: center;
    gap: 8px;
  }

  .detail-icon {
    font-size: 20px;
  }

  .detail-kind-badge {
    font-size: 12px;
    padding: 2px 8px;
    border-radius: 4px;
    background: var(--veil-accent-subtle);
    color: var(--veil-text-secondary);
    font-weight: 500;
  }

  .detail-badge {
    font-size: 10px;
    padding: 2px 6px;
    border-radius: 3px;
    font-weight: 500;
  }

  .layer-badge {
    background: rgba(139, 92, 246, 0.15);
    color: #a78bfa;
  }

  .critical-badge {
    background: rgba(245, 158, 11, 0.15);
    color: #f59e0b;
  }

  .detail-save-status {
    font-size: 11px;
  }

  .detail-save-status.saving {
    color: var(--veil-text-dim);
  }

  .detail-save-status.error {
    color: #ef4444;
  }

  .detail-section {
    display: flex;
    flex-direction: column;
    gap: 6px;
  }

  .detail-section-title {
    font-size: 11px;
    font-weight: 600;
    color: var(--veil-text-dim);
    text-transform: uppercase;
    letter-spacing: 0.05em;
    margin: 0;
    display: flex;
    align-items: center;
    gap: 6px;
  }

  .section-count {
    font-size: 10px;
    padding: 1px 5px;
    border-radius: 8px;
    background: var(--veil-accent-subtle);
    color: var(--veil-text-faint);
    font-weight: normal;
  }

  .detail-label {
    font-size: 11px;
    font-weight: 600;
    color: var(--veil-text-dim);
    text-transform: uppercase;
    letter-spacing: 0.05em;
  }

  .detail-input {
    width: 100%;
    padding: 6px 10px;
    border: 1px solid var(--veil-border);
    border-radius: 4px;
    background: var(--veil-input-bg);
    color: var(--veil-text);
    font: inherit;
    font-size: 14px;
  }

  .detail-input:focus {
    outline: 1px solid var(--veil-accent);
    border-color: var(--veil-accent);
  }

  .detail-name-display {
    font-size: 14px;
    font-weight: 500;
    padding: 6px 0;
    color: var(--veil-text);
  }

  .detail-doc {
    font-size: 12px;
    color: var(--veil-text-secondary);
    line-height: 1.5;
    padding: 8px 10px;
    background: var(--veil-accent-subtle);
    border-radius: 4px;
  }

  /* Diagnostics */
  .detail-diagnostics {
    border: 1px solid var(--veil-border);
    border-radius: 6px;
    padding: 10px;
    background: var(--veil-input-bg);
  }

  .detail-diag {
    display: flex;
    align-items: flex-start;
    gap: 6px;
    padding: 4px 0;
    font-size: 12px;
  }

  .diag-error .diag-severity { color: #ef4444; }
  .diag-warning .diag-severity { color: #f59e0b; }

  .diag-message {
    flex: 1;
    color: var(--veil-text);
  }

  .diag-hint {
    color: var(--veil-text-dim);
    font-style: italic;
    font-size: 11px;
  }

  /* Fields */
  .detail-fields {
    display: flex;
    flex-direction: column;
    gap: 2px;
    font-family: var(--font-mono);
    font-size: 12px;
  }

  .detail-field-row {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 4px 8px;
    border-radius: 3px;
  }

  .detail-field-row:hover {
    background: var(--veil-accent-subtle);
  }

  .field-name {
    color: var(--veil-text);
    font-weight: 500;
  }

  .field-type {
    color: var(--veil-text-dim);
  }

  /* Methods */
  .detail-methods {
    display: flex;
    flex-direction: column;
    gap: 2px;
  }

  .detail-method-row {
    display: flex;
    align-items: center;
    gap: 6px;
    padding: 6px 8px;
    border: none;
    background: transparent;
    color: var(--veil-text);
    font: inherit;
    font-size: 12px;
    font-family: var(--font-mono);
    cursor: pointer;
    text-align: left;
    border-radius: 3px;
    transition: background 0.1s;
    flex-wrap: wrap;
  }

  .detail-method-row:hover {
    background: var(--veil-accent-hover);
  }

  .detail-method-row.has-body {
    border-left: 2px solid var(--veil-accent);
  }

  .method-name {
    font-weight: 600;
    color: var(--veil-text);
  }

  .method-sig {
    color: var(--veil-text-dim);
    font-size: 11px;
  }

  .method-invariant {
    font-size: 10px;
    padding: 1px 4px;
    border-radius: 3px;
    background: rgba(245, 158, 11, 0.1);
    color: #f59e0b;
  }

  /* Body editor */
  .detail-body-editor {
    border: 1px solid var(--veil-border);
    border-radius: 6px;
    padding: 8px;
    background: var(--veil-code-bg);
    min-height: 100px;
  }

  /* Annotations */
  .detail-annotations {
    display: flex;
    flex-direction: column;
    gap: 8px;
  }

  .annotation-item {
    display: flex;
    flex-direction: column;
    gap: 4px;
  }

  .annotation-toggle {
    display: flex;
    align-items: center;
    gap: 8px;
    cursor: pointer;
    font-size: 13px;
  }

  .annotation-toggle input[type="checkbox"] {
    width: 14px;
    height: 14px;
    accent-color: var(--veil-accent);
  }

  .annotation-name {
    font-family: var(--font-mono);
    font-size: 12px;
    color: var(--veil-text);
  }

  .annotation-desc {
    font-size: 11px;
    color: var(--veil-text-faint);
  }

  .annotation-params {
    padding-left: 22px;
    display: flex;
    flex-direction: column;
    gap: 4px;
  }

  .annotation-param-row {
    display: flex;
    align-items: center;
    gap: 8px;
  }

  .annotation-param-label {
    font-size: 11px;
    color: var(--veil-text-dim);
    min-width: 60px;
  }

  .annotation-param-input {
    flex: 1;
    font-size: 12px;
    padding: 4px 8px;
  }

  /* Properties */
  .detail-properties {
    display: flex;
    flex-direction: column;
    gap: 2px;
  }

  .detail-prop-row {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 3px 8px;
    font-size: 12px;
    border-radius: 3px;
  }

  .detail-prop-row:hover {
    background: var(--veil-accent-subtle);
  }

  .prop-key {
    color: var(--veil-text-dim);
    font-family: var(--font-mono);
    font-size: 11px;
    min-width: 80px;
  }

  .prop-value {
    color: var(--veil-text);
    font-family: var(--font-mono);
    font-size: 11px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
</style>
