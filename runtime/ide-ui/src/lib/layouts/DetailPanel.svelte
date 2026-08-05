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
  import {
    irGraph,
    saveEdits,
    saving,
    saveError,
    paletteConfig,
    selectedNodeId,
    diagnostics,
    focusDiagnostic,
    currentProjectParam,
    type EditOp,
    type Diagnostic,
  } from '$lib/store';
  import { askAgent, formatIssuePrompt } from '$lib/agentPrompt';
  import { formatType } from '$lib/typeDisplay';
  import { isCriticalNode } from '$lib/lenses';
  import { BodySourceBlock } from '$lib/editors';
  import { irGraphBodyToExprs } from '$lib/editors/ir-convert';
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

  // Diagnostics for this construct (API often has node_id null — match by name too)
  let nodeDiagnostics = $derived.by((): Diagnostic[] => {
    if (!selectedIrNode) return [];
    return $diagnostics.filter(
      (d) =>
        (d.node_id != null && d.node_id === selectedIrNode!.id) ||
        (d.node_name != null && d.node_name === selectedIrNode!.name)
    );
  });

  /** Issues section collapsed state (default open when there are issues). */
  let issuesOpen = $state(true);
  // Re-open when selection changes to a construct that has issues
  $effect(() => {
    const n = selectedIrNode?.id;
    const count = nodeDiagnostics.length;
    if (n != null && count > 0) issuesOpen = true;
  });

  function sendAllIssuesToAgent() {
    if (!selectedIrNode || nodeDiagnostics.length === 0) return;
    const prompt = formatIssuePrompt(nodeDiagnostics, {
      construct: selectedIrNode.name,
      project: currentProjectParam(),
      all: true,
    });
    askAgent(prompt, { autoSend: true });
  }

  function sendOneIssueToAgent(diag: Diagnostic) {
    const prompt = formatIssuePrompt([diag], {
      construct: selectedIrNode?.name ?? diag.node_name ?? undefined,
      project: currentProjectParam(),
      all: false,
    });
    askAgent(prompt, { autoSend: true });
  }

  // Doc (from layer)
  let doc = $derived(selectedIrNode?.metadata.doc ?? null);

  // Steps under a Flow / DomainService (svc with step query / load / …)
  let steps = $derived.by((): IrNode[] => {
    return children.filter((c) => c.kind === 'Step');
  });

  /** Returns attached at Flow level (codegen often places `ret` as Flow sibling of steps). */
  let flowReturns = $derived.by((): IrNode[] => {
    return children.filter((c) => c.kind === 'Return');
  });

  /** Parent construct when viewing a Step (for ← back control). */
  let parentHost = $derived.by((): IrNode | null => {
    if (!selectedIrNode || kind !== 'Step') return null;
    const pid = selectedIrNode.metadata.parent;
    if (pid == null) return null;
    return graph.nodes.find((n) => n.id === pid) ?? null;
  });

  // Multi-step Flow/svc: structured step sections (not a fake flattened body).
  let isStructuredService = $derived(
    kind === 'Flow' && steps.length > 0
  );

  // Simple body (Step host, free fn, method) — Actions directly under node.
  let showSimpleBody = $derived(
    !layerProvided &&
      (kind === 'Step' ||
        kind === 'InterfaceMethod' ||
        (kind === 'Flow' && steps.length === 0))
  );

  let bodyExprs = $derived.by((): Expr[] => {
    if (!selectedIrNode || !showSimpleBody) return [];
    return irGraphBodyToExprs(graph.nodes, selectedIrNode.id);
  });

  function stepBodyExprs(stepId: number): Expr[] {
    return irGraphBodyToExprs(graph.nodes, stepId);
  }

  function returnExprs(): Expr[] {
    return flowReturns.map((r) => {
      const v = r.metadata.properties.find(([k]) => k === 'expr')?.[1];
      if (v) {
        // Prefer full expression text from IR
        return { kind: 'return' as const, value: { kind: 'ident' as const, name: v } };
      }
      return { kind: 'return' as const };
    });
  }

  function goToParent() {
    if (parentHost) selectedNodeId.set(String(parentHost.id));
  }

  function handleStepBodyEdit(step: IrNode, newExprs: Expr[]) {
    const veilSource = newExprs.map((e) => exprToVeil(e));
    persist({ op: 'set_body', span_start: step.span.start, body: veilSource });
  }

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

    <!-- Parent trail when focused on a Step inside a service -->
    {#if parentHost}
      <div class="detail-parent-trail">
        <button type="button" class="detail-back" onclick={goToParent}>
          ← {parentHost.name}
        </button>
        <span class="detail-trail-sep">/</span>
        <span class="detail-trail-here">{selectedIrNode.name}</span>
      </div>
    {/if}

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

    <!-- Diagnostics / issues for this construct -->
    {#if nodeDiagnostics.length > 0}
      <section class="detail-section detail-issues">
        <div class="issues-header">
          <button
            type="button"
            class="issues-toggle"
            aria-expanded={issuesOpen}
            onclick={() => (issuesOpen = !issuesOpen)}
          >
            <span class="issues-chevron" class:collapsed={!issuesOpen}>▸</span>
            <h3 class="detail-section-title issues-title">
              Issues <span class="section-count">{nodeDiagnostics.length}</span>
            </h3>
          </button>
          <button
            type="button"
            class="issues-send-all"
            title="Send all issues on this construct to the agent"
            onclick={sendAllIssuesToAgent}
          >
            Agent: fix all
          </button>
        </div>
        {#if issuesOpen}
          <ul class="detail-diags">
            {#each nodeDiagnostics as diag}
              <li>
                <div
                  class="detail-diag"
                  class:error={diag.severity === 'Error' || diag.severity === 'error'}
                >
                  <button
                    type="button"
                    class="detail-diag-main"
                    onclick={() => focusDiagnostic(diag)}
                    title={diag.hint ?? diag.message}
                  >
                    <span class="detail-diag-sev"
                      >{diag.severity === 'Error' || diag.severity === 'error'
                        ? '⛔'
                        : '⚠️'}</span
                    >
                    <span class="detail-diag-msg">
                      {#if diag.code}<code class="detail-diag-code">[{diag.code}]</code>{/if}
                      {diag.message}
                    </span>
                    {#if diag.hint}
                      <span class="detail-diag-hint">{diag.hint}</span>
                    {/if}
                  </button>
                  <button
                    type="button"
                    class="detail-diag-send"
                    title="Ask agent to investigate and fix this issue"
                    onclick={() => sendOneIssueToAgent(diag)}
                  >
                    Agent
                  </button>
                </div>
              </li>
            {/each}
          </ul>
        {/if}
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

    <!--
      Structured service (DomainService / multi-step Flow):
      one section per step with its body — mirrors .veil `step name` blocks.
      Do not also show a flattened body (that dual view was confusing).
    -->
    {#if isStructuredService && !layerProvided}
      <section class="detail-section">
        <h3 class="detail-section-title">
          Implementation
          <span class="section-hint">{steps.length} step{steps.length === 1 ? '' : 's'}</span>
        </h3>
        <div class="detail-step-blocks">
          {#each steps as step}
            {@const stepExprs = stepBodyExprs(step.id)}
            <div class="detail-step-block">
              <header class="detail-step-head">
                <span class="step-kw">step</span>
                <span class="step-name">{step.name}</span>
              </header>
              <div class="detail-body-editor">
                <BodySourceBlock
                  exprs={stepExprs}
                  onChange={(exprs) => handleStepBodyEdit(step, exprs)}
                  emptyLabel="Empty step body"
                />
              </div>
            </div>
          {/each}
          {#if flowReturns.length > 0}
            <div class="detail-step-block detail-step-block--return">
              <header class="detail-step-head">
                <span class="step-kw">ret</span>
                <span class="step-name">result</span>
              </header>
              <div class="detail-body-editor">
                <BodySourceBlock
                  exprs={returnExprs()}
                  onChange={handleBodyEdit}
                  emptyLabel="No return expression"
                />
              </div>
            </div>
          {/if}
        </div>
        <p class="detail-service-hint">
          Expand the service in the outline and double-click a method/fn to open its flow graph.
        </p>
      </section>
    {/if}

    <!-- Simple body: single step host, free fn, or method -->
    {#if showSimpleBody}
      <section class="detail-section">
        <h3 class="detail-section-title">Body</h3>
        <div class="detail-body-editor">
          <BodySourceBlock
            exprs={bodyExprs}
            onChange={handleBodyEdit}
            emptyLabel="No body expressions."
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

  /* Parent trail (Step → service) */
  .detail-parent-trail {
    display: flex;
    align-items: center;
    gap: 0.4rem;
    padding: 0.35rem 1rem;
    border-bottom: 1px solid var(--veil-border, #2e2e2e);
    background: color-mix(in srgb, var(--veil-text-dim, #a3a3a3) 6%, transparent);
    font-size: 0.78rem;
  }
  .detail-back {
    border: none;
    background: none;
    color: var(--veil-text-secondary, #a3a3a3);
    font: inherit;
    font-weight: 600;
    cursor: pointer;
    padding: 0.15rem 0.25rem;
    border-radius: 4px;
  }
  .detail-back:hover {
    color: var(--veil-text, #e5e5e5);
    background: color-mix(in srgb, var(--veil-text-dim, #a3a3a3) 12%, transparent);
  }
  .detail-trail-sep {
    color: var(--veil-text-faint, #737373);
  }
  .detail-trail-here {
    color: var(--veil-text, #e5e5e5);
    font-weight: 600;
    font-family: var(--font-mono, monospace);
  }

  /* Structured service: step blocks */
  .detail-step-blocks {
    display: flex;
    flex-direction: column;
    gap: 0.75rem;
  }

  .detail-step-block {
    border: 1px solid var(--veil-border, #2e2e2e);
    border-radius: 8px;
    overflow: hidden;
    background: color-mix(in srgb, var(--veil-surface, #1a1a1a) 80%, #000);
  }

  .detail-step-block--return {
    border-style: dashed;
  }

  .detail-step-head {
    display: flex;
    align-items: center;
    gap: 0.45rem;
    padding: 0.4rem 0.65rem;
    border-bottom: 1px solid var(--veil-border, #2e2e2e);
    background: color-mix(in srgb, var(--veil-text-dim, #a3a3a3) 8%, transparent);
  }

  .step-kw {
    font-size: 0.65rem;
    font-weight: 700;
    text-transform: uppercase;
    letter-spacing: 0.04em;
    color: var(--veil-text-faint, #737373);
  }

  .step-name {
    font-weight: 650;
    font-family: var(--font-mono, monospace);
    font-size: 0.82rem;
    color: var(--veil-text, #e5e5e5);
  }

  .detail-service-hint {
    margin: 0.65rem 0 0;
    font-size: 0.7rem;
    color: var(--veil-text-faint, #737373);
    line-height: 1.4;
  }

  .section-hint {
    font-weight: 500;
    font-size: 0.65rem;
    text-transform: none;
    letter-spacing: 0;
    color: var(--veil-text-faint, #737373);
    margin-left: 0.35rem;
  }

  .issues-header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 0.5rem;
    margin-bottom: 0.35rem;
  }

  .issues-toggle {
    display: flex;
    align-items: center;
    gap: 0.35rem;
    border: none;
    background: transparent;
    color: inherit;
    font: inherit;
    cursor: pointer;
    padding: 0;
    min-width: 0;
  }

  .issues-title {
    margin: 0;
  }

  .issues-chevron {
    display: inline-block;
    font-size: 0.7rem;
    color: var(--veil-text-dim, #a3a3a3);
    transition: transform 0.12s ease;
    transform: rotate(90deg);
  }

  .issues-chevron.collapsed {
    transform: rotate(0deg);
  }

  .issues-send-all {
    flex-shrink: 0;
    border: 1px solid color-mix(in srgb, #f59e0b 45%, var(--veil-border, #2e2e2e));
    background: color-mix(in srgb, #f59e0b 12%, transparent);
    color: var(--veil-text, #e5e5e5);
    font: inherit;
    font-size: 0.68rem;
    font-weight: 650;
    padding: 0.28rem 0.55rem;
    border-radius: 6px;
    cursor: pointer;
    white-space: nowrap;
  }

  .issues-send-all:hover {
    background: color-mix(in srgb, #f59e0b 22%, transparent);
  }

  .detail-diags {
    list-style: none;
    margin: 0;
    padding: 0;
    display: flex;
    flex-direction: column;
    gap: 4px;
  }

  .detail-diag {
    display: flex;
    align-items: stretch;
    gap: 0;
    width: 100%;
    border: 1px solid var(--veil-border, #2e2e2e);
    border-radius: 6px;
    background: var(--veil-surface-alt, rgba(26, 26, 26, 0.6));
    overflow: hidden;
  }

  .detail-diag.error {
    border-color: color-mix(in srgb, #ef4444 45%, var(--veil-border, #2e2e2e));
  }

  .detail-diag-main {
    flex: 1;
    min-width: 0;
    display: flex;
    flex-direction: column;
    align-items: flex-start;
    gap: 2px;
    text-align: left;
    border: none;
    background: transparent;
    padding: 0.45rem 0.6rem;
    color: var(--veil-text, #e5e5e5);
    font: inherit;
    cursor: pointer;
  }

  .detail-diag-main:hover {
    background: var(--veil-accent-hover, rgba(115, 115, 115, 0.15));
  }

  .detail-diag-send {
    flex-shrink: 0;
    border: none;
    border-left: 1px solid var(--veil-border, #2e2e2e);
    background: transparent;
    color: var(--veil-text-dim, #a3a3a3);
    font: inherit;
    font-size: 0.68rem;
    font-weight: 650;
    padding: 0 0.65rem;
    cursor: pointer;
    white-space: nowrap;
  }

  .detail-diag-send:hover {
    color: var(--veil-text, #e5e5e5);
    background: color-mix(in srgb, #f59e0b 14%, transparent);
  }

  .detail-diag-sev {
    font-size: 0.75rem;
  }

  .detail-diag-msg {
    font-size: 0.78rem;
    line-height: 1.35;
  }

  .detail-diag-code {
    font-size: 0.7rem;
    color: var(--veil-text-dim, #a3a3a3);
    margin-right: 0.25rem;
  }

  .detail-diag-hint {
    font-size: 0.7rem;
    color: var(--veil-text-faint, #737373);
    line-height: 1.3;
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

  /* Body editor — chrome lives on BodySourceBlock (view/edit modes) */
  .detail-body-editor {
    min-height: 0;
  }

  .detail-step-block .detail-body-editor {
    padding: 0.35rem 0.5rem 0.65rem;
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
