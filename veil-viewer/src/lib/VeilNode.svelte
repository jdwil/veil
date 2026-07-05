<script lang="ts">
  import { Handle, Position } from '@xyflow/svelte';
  import { NODE_STYLES, getNodeStyle, type NodeKind } from '$lib/types';

  let { data } = $props();

  const kind: NodeKind = data.kind;
  const subkind: string | null = data.subkind ?? null;
  const style = getNodeStyle(kind, subkind);
  const hasChildren = data.hasChildren ?? false;
  const annotations: string[] = data.annotations ?? [];
  const refs: string[] = data.refs ?? [];
  const isGhost = data.isGhost ?? false;
  const properties: [string, string][] = data.properties ?? [];
  const inlineChildren: { name: string; kind: string; properties: [string, string][] }[] = data.inlineChildren ?? [];
  let detailsOpen = $state(false);
  let childrenOpen = $state(true);

  // Reference-line badge (e.g. `ctx Identity`, `contexts A, B`) — the builder
  // prefixes ref keys with `ref:` so this stays layer-agnostic.
  const refProp = properties.find(([k]) => k.startsWith('ref:'));
  const refKeyword = refProp ? refProp[0].slice(4) : null;
  const refValue = refProp ? refProp[1] : null;
  const hasCompensate = annotations.includes('has_compensate');
  const isGroup = data.isGroup ?? false;
</script>

<div
  class="veil-node"
  class:has-children={hasChildren}
  class:ghost={isGhost}
  class:is-flow={kind === 'Flow'}
  class:is-error={kind === 'ErrorBoundary'}
  class:is-group={isGroup}
  style="--node-color: {style.color}"
>
  <Handle type="target" position={Position.Top} />
  <Handle type="target" position={Position.Left} />

  <!-- Glow pulse layer for events/flows -->
  {#if kind === 'Flow' && !isGhost}
    <div class="glow-pulse"></div>
  {/if}

  <div class="node-inner">
    <div class="node-header">
      <span class="node-icon">{style.icon}</span>
      <span class="node-kind">{style.label}</span>
      {#if hasCompensate}
        <span class="compensate-badge" title="Has compensation (rollback)">↩</span>
      {/if}
      {#if hasChildren && !isGhost}
        <span class="expand-indicator">⤵</span>
      {/if}
    </div>

    {#if refValue}
      <div class="context-badge">
        <span class="ctx-icon">{getNodeStyle('Module', refKeyword).icon}</span>
        <span class="ctx-name">{refValue}</span>
      </div>
    {/if}

    <div class="node-name" class:code-name={kind === 'Action'}>{data.label}</div>

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

  <Handle type="source" position={Position.Bottom} />
  <Handle type="source" position={Position.Right} />
</div>

<style>
  .veil-node {
    position: relative;
    background: linear-gradient(145deg, rgba(26, 26, 46, 0.95), rgba(20, 20, 35, 0.98));
    border: 1.5px solid var(--node-color);
    border-radius: 14px;
    padding: 0;
    min-width: 190px;
    max-width: 360px;
    backdrop-filter: blur(12px);
    box-shadow:
      0 0 15px color-mix(in srgb, var(--node-color) 25%, transparent),
      0 8px 32px rgba(0, 0, 0, 0.5),
      inset 0 1px 0 rgba(255, 255, 255, 0.05);
    transition: all 0.3s cubic-bezier(0.4, 0, 0.2, 1);
    transform: perspective(800px) rotateX(2deg);
    transform-style: preserve-3d;
  }

  .veil-node:hover {
    transform: perspective(800px) rotateX(0deg) translateY(-4px) scale(1.02);
    box-shadow:
      0 0 30px color-mix(in srgb, var(--node-color) 45%, transparent),
      0 16px 48px rgba(0, 0, 0, 0.6),
      inset 0 1px 0 rgba(255, 255, 255, 0.08);
    border-color: color-mix(in srgb, var(--node-color) 80%, white);
  }

  .veil-node.has-children {
    cursor: pointer;
  }

  .veil-node.has-children:hover {
    transform: perspective(800px) rotateX(-1deg) translateY(-6px) scale(1.03);
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
    background: linear-gradient(145deg, rgba(30, 20, 10, 0.95), rgba(25, 15, 5, 0.98));
  }

  .veil-node.is-error {
    border-width: 2px;
    background: linear-gradient(145deg, rgba(40, 15, 15, 0.95), rgba(30, 10, 10, 0.98));
  }

  .veil-node.is-group {
    background: rgba(139, 92, 246, 0.04);
    border: 2px dashed rgba(139, 92, 246, 0.4);
    border-radius: 16px;
    box-shadow: none;
    transform: none;
    min-width: unset;
    max-width: unset;
  }

  .veil-node.is-group:hover {
    transform: none;
    box-shadow: 0 0 20px rgba(139, 92, 246, 0.15);
    border-color: rgba(139, 92, 246, 0.6);
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
    color: #475569;
    animation: bobble 2s ease-in-out infinite;
  }

  @keyframes bobble {
    0%, 100% { transform: translateY(0); }
    50% { transform: translateY(2px); }
  }

  .node-name {
    font-size: 15px;
    font-weight: 800;
    color: #f1f5f9;
    word-break: break-word;
    text-shadow: 0 1px 2px rgba(0, 0, 0, 0.3);
  }

  .node-name.code-name {
    font-family: 'JetBrains Mono', 'Fira Code', monospace;
    font-size: 12px;
    font-weight: 500;
    background: rgba(0, 0, 0, 0.2);
    padding: 4px 8px;
    border-radius: 4px;
    color: #a5f3fc;
  }

  .context-badge {
    display: flex;
    align-items: center;
    gap: 4px;
    padding: 2px 8px;
    margin-bottom: 4px;
    border-radius: 4px;
    background: rgba(139, 92, 246, 0.12);
    border: 1px solid rgba(139, 92, 246, 0.3);
    width: fit-content;
  }

  .ctx-icon {
    font-size: 10px;
  }

  .ctx-name {
    font-size: 10px;
    font-weight: 600;
    color: #c4b5fd;
  }

  .compensate-badge {
    font-size: 12px;
    color: #10b981;
    margin-left: auto;
    title: "Has compensation";
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
    background: rgba(0, 0, 0, 0.3);
    border-radius: 6px;
    border: 1px solid rgba(255, 255, 255, 0.05);
  }

  .details-toggle {
    display: flex;
    align-items: center;
    gap: 4px;
    margin-top: 6px;
    padding: 2px 6px;
    background: rgba(99, 102, 241, 0.08);
    border: 1px solid rgba(99, 102, 241, 0.2);
    border-radius: 4px;
    cursor: pointer;
    color: #64748b;
    font-size: 9px;
    transition: all 0.15s;
  }

  .details-toggle:hover {
    background: rgba(99, 102, 241, 0.15);
    color: #94a3b8;
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
    color: #94a3b8;
    word-break: break-all;
  }

  .detail-key {
    color: #64748b;
  }

  .detail-value {
    color: #cbd5e1;
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
    background: rgba(99, 102, 241, 0.06);
    border: 1px solid rgba(99, 102, 241, 0.15);
    border-radius: 6px;
    font-size: 11px;
  }

  .child-icon {
    font-size: 11px;
  }

  .child-name {
    color: #e2e8f0;
    font-weight: 600;
  }

  .annotation-badge {
    font-size: 9px;
    padding: 2px 7px;
    border-radius: 6px;
    background: rgba(99, 102, 241, 0.15);
    color: #a5b4fc;
    border: 1px solid rgba(99, 102, 241, 0.25);
    font-weight: 500;
    width: fit-content;
  }

  .ref-badge {
    font-size: 9px;
    padding: 2px 7px;
    border-radius: 6px;
    background: rgba(168, 85, 247, 0.12);
    color: #c4b5fd;
    border: 1px solid rgba(168, 85, 247, 0.25);
    font-weight: 500;
  }
</style>
