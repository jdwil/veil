<script lang="ts">
  import { Handle, Position } from '@xyflow/svelte';
  import { NODE_STYLES, getNodeStyle, type NodeKind } from '$lib/types';
  import { changedNodeIds } from '$lib/store';

  let { id, data } = $props();

  const kind: NodeKind = $derived(data.kind);
  const subkind: string | null = $derived(data.subkind ?? null);
  const style = $derived(getNodeStyle(kind, subkind));
  const hasChildren = $derived(data.hasChildren ?? false);
  const annotations: string[] = $derived(data.annotations ?? []);
  const refs: string[] = $derived(data.refs ?? []);
  const isGhost = $derived(data.isGhost ?? false);
  const properties: [string, string][] = $derived(data.properties ?? []);
  const inlineChildren: { name: string; kind: string; properties: [string, string][] }[] = $derived(data.inlineChildren ?? []);
  let detailsOpen = $state(false);
  let childrenOpen = $state(true);

  // Flash highlight when the agent just modified this node.
  let isFlashing = $state(false);
  $effect(() => {
    const ids = $changedNodeIds;
    const numericId = Number(id);
    if (ids.size > 0 && ids.has(numericId)) {
      isFlashing = true;
      const timer = setTimeout(() => { isFlashing = false; }, 1200);
      return () => clearTimeout(timer);
    } else {
      isFlashing = false;
    }
  });

  // Reference-line badge (e.g. `ctx Identity`, `contexts A, B`) — the builder
  // prefixes ref keys with `ref:` so this stays layer-agnostic.
  const refProp = $derived(properties.find(([k]) => k.startsWith('ref:')));
  const refKeyword = $derived(refProp ? refProp[0].slice(4) : null);
  const refValue = $derived(refProp ? refProp[1] : null);
  const hasCompensate = $derived(annotations.includes('has_compensate'));
  const isGroup = $derived(data.isGroup ?? false);
  const isAbstract = $derived(properties.some(([k, v]) => k === 'abstract' && v === 'true'));
  const isCritical = $derived(data.critical ?? false);
  const layerProvided = $derived(data.layerProvided ?? false);
  const bodyPreview: { text: string; keyword: string | null }[] = $derived(data.bodyPreview ?? []);
  const bodyEmpty: boolean = $derived(data.bodyEmpty ?? false);
  const bodyMore: number = $derived(data.bodyMore ?? 0);
  const routingTargets: string[] = $derived(data.routingTargets ?? []);
  const isStep = $derived(kind === 'Step');
  const stepKind: string = $derived(subkind || '');
  const isDecision = $derived(stepKind === 'decision');
  const isBranch = $derived(stepKind === 'branch');
  const isLoop = $derived(stepKind === 'loop');
  const isQuery = $derived(stepKind === 'query');
  const isAssign = $derived(stepKind === 'assign');
</script>

<div
  class="veil-node"
  class:has-children={hasChildren}
  class:ghost={isGhost}
  class:is-flow={kind === 'Flow'}
  class:is-error={kind === 'ErrorBoundary'}
  class:is-group={isGroup}
  class:is-critical={isCritical}
  class:layer-provided={layerProvided}
  class:agent-flash={isFlashing}
  class:step-decision={isDecision}
  class:step-branch={isBranch}
  class:step-loop={isLoop}
  class:step-query={isQuery}
  class:step-assign={isAssign}
  style="--node-color: {style.color}"
>
  <Handle type="target" position={Position.Top} id="top" />
  <Handle type="target" position={Position.Left} id="left" />
  <Handle type="target" position={Position.Bottom} id="bottom" />
  <Handle type="target" position={Position.Right} id="right" />

  <!-- Glow pulse layer for events/flows -->
  {#if kind === 'Flow' && !isGhost}
    <div class="glow-pulse"></div>
  {/if}

  <div class="node-inner">
    <div class="node-header">
      <span class="node-icon">{style.icon}</span>
      <span class="node-kind">{style.label}</span>
      {#if layerProvided}
        <span class="infra-badge" title="Layer-provided infrastructure (not user source)">infra</span>
      {/if}
      {#if isCritical}
        <span class="critical-badge" title="Critical (layer lens or diagnostic)">!</span>
      {/if}
      {#if hasCompensate}
        <span class="compensate-badge" title="Has compensation (rollback)">↩</span>
      {/if}
      {#if hasChildren && !isGhost}
        <span class="expand-indicator" title="Double-click to enter">⤵</span>
      {/if}
    </div>

    {#if refValue}
      <div class="context-badge">
        <span class="ctx-icon">{getNodeStyle('Module', refKeyword).icon}</span>
        <span class="ctx-name">{refValue}</span>
      </div>
    {/if}

    <div
      class="node-name"
      class:code-name={kind === 'Action'}
      title={hasChildren && !isGhost ? 'Double-click to enter' : undefined}
    >{data.label}</div>
    {#if hasChildren && !isGhost}
      <div class="enter-hint">Double-click to enter</div>
    {/if}

    {#if isAbstract}
      <span class="abstract-badge">abstract</span>
    {/if}

    <!-- UX-024: step body summary on the card -->
    {#if isStep && !subkind && bodyEmpty}
      <div class="body-preview empty">empty step</div>
    {:else if isStep && !subkind && bodyPreview.length > 0}
      <div class="body-preview">
        {#each bodyPreview as line}
          <div
            class="body-line"
            class:is-guard={line.keyword === 'guard'}
            class:is-subblock={line.keyword === 'compensate' || data.annotations?.includes(`has_${line.keyword}`)}
            title={line.text}
          >
            {#if line.keyword && line.keyword !== 'call' && line.keyword !== 'assign'}
              <span class="body-kw">{line.keyword}</span>
            {/if}
            <span class="body-text">{line.text}</span>
          </div>
        {/each}
        {#if bodyMore > 0}
          <div class="body-more">+{bodyMore} more · select for full body</div>
        {/if}
      </div>
    {/if}

    {#if routingTargets.length > 0}
      <div class="routing-badges">
        {#each routingTargets.slice(0, 3) as t}
          <span class="routing-badge" title="Calls {t}">→ {t}</span>
        {/each}
      </div>
    {/if}

    {#if properties.length > 0}
      <button class="details-toggle" onclick={(e) => { e.stopPropagation(); detailsOpen = !detailsOpen; }}>
        <span class="toggle-icon">{detailsOpen ? '▾' : '▸'}</span>
        <span class="toggle-label">details</span>
      </button>
      {#if detailsOpen}
        <div class="node-details">
          {#each properties as [key, value]}
            <div class="detail-line">
              <span class="detail-key">{key}:</span>
              <span class="detail-value">{value}</span>
            </div>
          {/each}
        </div>
      {/if}
    {/if}

    {#if annotations.length > 0}
      <div class="node-badges">
        {#each annotations.slice(0, 3) as ann}
          <span class="annotation-badge">{ann}</span>
        {/each}
      </div>
    {/if}

    {#if refs.length > 0 && !isGhost}
      <div class="node-badges">
        {#each refs.slice(0, 2) as ref}
          <span class="ref-badge">{ref}</span>
        {/each}
      </div>
    {/if}

    {#if inlineChildren.length > 0}
      <button class="details-toggle" onclick={(e) => { e.stopPropagation(); childrenOpen = !childrenOpen; }}>
        <span class="toggle-icon">{childrenOpen ? '▾' : '▸'}</span>
        <span class="toggle-label">{inlineChildren.length} branches</span>
      </button>
      {#if childrenOpen}
        <div class="inline-children">
          {#each inlineChildren as child}
            <div class="inline-child">
              <span class="child-icon">{getNodeStyle(child.kind as NodeKind, null)?.icon ?? '▶️'}</span>
              <span class="child-name">{child.name}</span>
            </div>
          {/each}
        </div>
      {/if}
    {/if}
  </div>

  <Handle type="source" position={Position.Bottom} id="bottom" />
  <Handle type="source" position={Position.Right} id="right" />
  <Handle type="source" position={Position.Top} id="top" />
  <Handle type="source" position={Position.Left} id="left" />
</div>

<style>
  .veil-node {
    position: relative;
    background: linear-gradient(145deg, var(--veil-node-bg), var(--veil-node-bg-end));
    border: 1.5px solid var(--node-color);
    border-radius: 14px;
    padding: 0;
    min-width: 190px;
    max-width: 360px;
    backdrop-filter: blur(12px);
    box-shadow:
      0 0 15px color-mix(in srgb, var(--node-color) 25%, transparent),
      0 8px 32px var(--veil-shadow),
      inset 0 1px 0 var(--veil-highlight);
    transition: all 0.3s cubic-bezier(0.4, 0, 0.2, 1);
    transform: perspective(800px) rotateX(2deg);
    transform-style: preserve-3d;
  }

  .veil-node:hover {
    transform: perspective(800px) rotateX(0deg) translateY(-4px) scale(1.02);
    box-shadow:
      0 0 30px color-mix(in srgb, var(--node-color) 45%, transparent),
      0 16px 48px var(--veil-shadow-strong),
      inset 0 1px 0 var(--veil-highlight);
    border-color: color-mix(in srgb, var(--node-color) 80%, white);
  }

  .veil-node.has-children {
    cursor: pointer;
  }

  .veil-node.has-children:hover {
    transform: perspective(800px) rotateX(-1deg) translateY(-6px) scale(1.03);
  }

  /* UX-017: layer-provided infrastructure is distinct and dimmed when shown */
  .veil-node.layer-provided {
    opacity: 0.62;
    border-style: dashed;
    filter: grayscale(0.25);
  }
  .veil-node.layer-provided:hover {
    opacity: 0.8;
  }
  .infra-badge {
    font-size: 8px;
    text-transform: uppercase;
    letter-spacing: 0.4px;
    padding: 1px 5px;
    border-radius: 4px;
    background: rgba(148, 163, 184, 0.2);
    color: var(--veil-text-dim);
    margin-left: auto;
  }

  .body-preview {
    margin: 4px 10px 8px;
    padding: 6px 8px;
    border-radius: 6px;
    background: rgba(0, 0, 0, 0.18);
    border: 1px solid rgba(255, 255, 255, 0.04);
    display: flex;
    flex-direction: column;
    gap: 3px;
    max-width: 100%;
  }
  .body-preview.empty {
    font-size: 10px;
    font-style: italic;
    color: var(--veil-text-faint);
    text-align: center;
  }
  .body-line {
    display: flex;
    gap: 6px;
    font-size: 10px;
    font-family: 'JetBrains Mono', monospace;
    color: var(--veil-text-dim);
    line-height: 1.35;
    overflow: hidden;
  }
  .body-line.is-guard {
    color: #fbbf24;
  }
  .body-line.is-subblock .body-kw {
    color: #f87171;
  }
  .body-kw {
    flex-shrink: 0;
    font-weight: 700;
    font-size: 9px;
    text-transform: lowercase;
    color: var(--veil-accent, #60a5fa);
  }
  .body-text {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .body-more {
    font-size: 9px;
    color: var(--veil-text-faint);
    font-style: italic;
  }
  .routing-badges {
    display: flex;
    flex-wrap: wrap;
    gap: 4px;
    padding: 0 10px 8px;
  }
  .routing-badge {
    font-size: 9px;
    padding: 1px 6px;
    border-radius: 4px;
    background: rgba(96, 165, 250, 0.12);
    color: #93c5fd;
    font-family: 'JetBrains Mono', monospace;
  }

  .veil-node.ghost {
    opacity: 0.4;
    border-style: dashed;
    box-shadow: none;
    transform: perspective(800px) rotateX(2deg) scale(0.95);
  }

  .veil-node.ghost:hover {
    opacity: 0.55;
    transform: perspective(800px) rotateX(2deg) scale(0.95);
    box-shadow: none;
  }

  .veil-node.is-flow {
    border-width: 2px;
    background: linear-gradient(145deg, var(--veil-node-bg), var(--veil-node-bg-end));
  }

  .veil-node.is-error {
    border-width: 2px;
    background: linear-gradient(145deg, var(--veil-node-bg), var(--veil-node-bg-end));
  }

  .veil-node.is-group {
    background: var(--veil-accent-subtle);
    border: 2px dashed var(--veil-border);
    border-radius: 16px;
    box-shadow: none;
    transform: none;
    min-width: unset;
    max-width: unset;
  }

  .veil-node.is-group:hover {
    transform: none;
    box-shadow: 0 0 20px var(--veil-accent-hover);
    border-color: var(--veil-accent);
  }

  /* Glow pulse animation for events and flows */
  .glow-pulse {
    position: absolute;
    inset: -3px;
    border-radius: 16px;
    background: transparent;
    border: 2px solid var(--node-color);
    opacity: 0;
    animation: glowPulse 2.5s ease-in-out infinite;
    pointer-events: none;
  }

  @keyframes glowPulse {
    0%, 100% { opacity: 0; transform: scale(1); }
    50% { opacity: 0.6; transform: scale(1.02); }
  }

  .node-inner {
    padding: 14px 18px;
    position: relative;
    z-index: 1;
  }

  .node-header {
    display: flex;
    align-items: center;
    gap: 6px;
    margin-bottom: 6px;
  }

  .node-icon {
    font-size: 15px;
    filter: drop-shadow(0 0 4px var(--node-color));
  }

  .node-kind {
    font-size: 10px;
    text-transform: uppercase;
    letter-spacing: 0.8px;
    color: var(--node-color);
    font-weight: 700;
  }

  .expand-indicator {
    margin-left: auto;
    font-size: 12px;
    color: var(--veil-text-faint);
    animation: bobble 2s ease-in-out infinite;
  }

  @keyframes bobble {
    0%, 100% { transform: translateY(0); }
    50% { transform: translateY(2px); }
  }

  .node-name {
    font-size: 15px;
    font-weight: 800;
    color: var(--veil-text);
    word-break: break-word;
    text-shadow: 0 1px 2px var(--veil-input-bg);
  }

  .enter-hint {
    font-size: 10px;
    font-weight: 500;
    color: var(--veil-text-faint);
    margin-top: 2px;
    letter-spacing: 0.02em;
  }

  .node-name.code-name {
    font-family: 'JetBrains Mono', 'Fira Code', monospace;
    font-size: 12px;
    font-weight: 500;
    background: var(--veil-code-bg);
    padding: 4px 8px;
    border-radius: 4px;
    color: var(--veil-text);
  }

  .context-badge {
    display: flex;
    align-items: center;
    gap: 4px;
    padding: 2px 8px;
    margin-bottom: 4px;
    border-radius: 4px;
    background: var(--veil-accent-subtle);
    border: 1px solid var(--veil-border);
    width: fit-content;
  }

  .ctx-icon {
    font-size: 10px;
  }

  .ctx-name {
    font-size: 10px;
    font-weight: 600;
    color: var(--veil-text-secondary);
  }

  .compensate-badge {
    font-size: 12px;
    color: var(--veil-text-secondary);
    margin-left: auto;
  }

  .critical-badge {
    font-size: 10px;
    font-weight: 800;
    color: #fbbf24;
    background: rgba(251, 191, 36, 0.15);
    border: 1px solid rgba(251, 191, 36, 0.4);
    border-radius: 4px;
    padding: 0 4px;
    margin-left: auto;
  }

  .veil-node.is-critical {
    box-shadow: 0 0 0 1px rgba(251, 191, 36, 0.35), 0 0 12px rgba(251, 191, 36, 0.12);
  }

  .node-badges {
    display: flex;
    flex-direction: column;
    gap: 3px;
    margin-top: 8px;
  }

  .node-details {
    margin-top: 4px;
    padding: 6px 8px;
    background: var(--veil-input-bg);
    border-radius: 6px;
    border: 1px solid var(--veil-highlight);
  }

  .details-toggle {
    display: flex;
    align-items: center;
    gap: 4px;
    margin-top: 6px;
    padding: 2px 6px;
    background: var(--veil-accent-subtle);
    border: 1px solid var(--veil-accent-hover);
    border-radius: 4px;
    cursor: pointer;
    color: var(--veil-text-dim);
    font-size: 9px;
    transition: all 0.15s;
  }

  .details-toggle:hover {
    background: var(--veil-accent-hover);
    color: var(--veil-text-secondary);
  }

  .toggle-icon {
    font-size: 8px;
  }

  .toggle-label {
    font-size: 9px;
  }

  .detail-line {
    font-size: 10px;
    font-family: 'JetBrains Mono', 'Fira Code', monospace;
    line-height: 1.5;
    color: var(--veil-text-secondary);
    word-break: break-all;
  }

  .detail-key {
    color: var(--veil-text-dim);
  }

  .detail-value {
    color: var(--veil-text);
  }

  .inline-children {
    margin-top: 4px;
    display: flex;
    flex-direction: column;
    gap: 3px;
  }

  .inline-child {
    display: flex;
    align-items: center;
    gap: 6px;
    padding: 4px 8px;
    background: var(--veil-accent-subtle);
    border: 1px solid var(--veil-accent-hover);
    border-radius: 6px;
    font-size: 11px;
  }

  .child-icon {
    font-size: 11px;
  }

  .child-name {
    color: var(--veil-text);
    font-weight: 600;
  }

  .abstract-badge {
    font-size: 9px;
    padding: 2px 7px;
    border-radius: 6px;
    background: rgba(148, 163, 184, 0.12);
    color: var(--veil-text-secondary);
    border: 1px solid rgba(148, 163, 184, 0.25);
    font-weight: 500;
    font-style: italic;
    width: fit-content;
  }

  .annotation-badge {
    font-size: 9px;
    padding: 2px 7px;
    border-radius: 6px;
    background: var(--veil-accent-hover);
    color: var(--veil-text);
    border: 1px solid var(--veil-border);
    font-weight: 500;
    width: fit-content;
  }

  .ref-badge {
    font-size: 9px;
    padding: 2px 7px;
    border-radius: 6px;
    background: rgba(168, 85, 247, 0.12);
    color: var(--veil-text-secondary);
    border: 1px solid rgba(168, 85, 247, 0.25);
    font-weight: 500;
  }

  .agent-flash {
    animation: agent-highlight 1.2s ease-out;
  }

  @keyframes agent-highlight {
    0% {
      box-shadow: 0 0 0 3px rgba(96, 165, 250, 0.8), 0 0 12px rgba(96, 165, 250, 0.4);
      border-color: rgba(96, 165, 250, 0.8);
    }
    50% {
      box-shadow: 0 0 0 2px rgba(96, 165, 250, 0.5), 0 0 8px rgba(96, 165, 250, 0.2);
      border-color: rgba(96, 165, 250, 0.5);
    }
    100% {
      box-shadow: none;
      border-color: var(--node-color, var(--veil-border));
    }
  }
  /* ─── Step-type node shapes ─────────────────────────────────── */

  .step-decision,
  .step-branch {
    border-radius: 4px;
    clip-path: polygon(50% 0%, 100% 50%, 50% 100%, 0% 50%);
    min-width: 200px;
    min-height: 120px;
    display: flex;
    align-items: center;
    justify-content: center;
    padding: 24px 32px;
  }

  .step-decision .node-inner,
  .step-branch .node-inner {
    text-align: center;
  }

  .step-loop {
    border-radius: 24px;
    border-width: 2.5px;
    border-style: dashed;
  }

  .step-query {
    border-radius: 8px;
    border-left: 4px solid var(--node-color);
  }

  .step-assign {
    border-radius: 6px;
    border-style: dotted;
    border-width: 2px;
  }
</style>
