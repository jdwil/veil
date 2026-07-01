<script lang="ts">
  import { NODE_STYLES, type NodeKind } from '$lib/types';

  interface PaletteItem {
    kind: NodeKind;
    label: string;
    icon: string;
    category: string;
  }

  let { contextKind = 'Solution' }: { contextKind?: NodeKind | null } = $props();

  // All available construct types grouped by category
  const ALL_PALETTE_ITEMS: PaletteItem[] = [
    // Structural
    { kind: 'Module', label: 'Module', icon: '📦', category: 'Structure' },
    { kind: 'TypeDef', label: 'Type', icon: '📋', category: 'Structure' },
    { kind: 'Interface', label: 'Interface', icon: '🔌', category: 'Structure' },
    { kind: 'Implementation', label: 'Implementation', icon: '🔗', category: 'Structure' },
    // Flow
    { kind: 'Flow', label: 'Flow', icon: '🌊', category: 'Flow' },
    { kind: 'Saga', label: 'Saga', icon: '🔄', category: 'Flow' },
    { kind: 'Step', label: 'Step', icon: '▶️', category: 'Flow' },
    { kind: 'ParallelGateway', label: 'Parallel', icon: '⑃', category: 'Flow' },
    { kind: 'ErrorBoundary', label: 'Error Boundary', icon: '🛡️', category: 'Flow' },
  ];

  // Context-aware filtering: show only what makes sense at the current level
  const ALLOWED_CHILDREN: Record<string, NodeKind[]> = {
    Solution: ['Module', 'Flow', 'Saga', 'Implementation'],
    Module: ['TypeDef', 'Interface', 'Flow'],
    TypeDef: [],
    Interface: [],
    Flow: ['Step', 'ParallelGateway', 'ErrorBoundary'],
    Saga: ['Step'],
    ParallelGateway: ['Step'],
    Step: [],
    Implementation: [],
    ErrorBoundary: [],
  };

  $effect(() => {
    // Reactively update items when contextKind changes
  });

  let items = $derived.by(() => {
    const allowed = ALLOWED_CHILDREN[contextKind ?? 'Solution'] ?? [];
    if (allowed.length === 0) return [];
    return ALL_PALETTE_ITEMS.filter(item => allowed.includes(item.kind));
  });

  let categories = $derived([...new Set(items.map(i => i.category))]);

  function onDragStart(event: DragEvent, item: PaletteItem) {
    if (!event.dataTransfer) return;
    event.dataTransfer.setData('application/veil-node', JSON.stringify(item));
    event.dataTransfer.effectAllowed = 'move';
  }
</script>

<aside class="palette">
  <div class="palette-header">
    <span class="palette-title">Constructs</span>
  </div>

  <div class="palette-body">
    {#if items.length === 0}
      <div class="palette-empty">
        <span class="empty-text">No constructs available at this level</span>
      </div>
    {:else}
      {#each categories as category}
        <div class="palette-category">
          <span class="category-label">{category}</span>
          <div class="palette-items">
            {#each items.filter(i => i.category === category) as item}
              <div
                class="palette-tile"
                draggable="true"
                ondragstart={(e) => onDragStart(e, item)}
                style="--tile-color: {NODE_STYLES[item.kind].color}"
              >
                <span class="tile-icon">{item.icon}</span>
                <span class="tile-label">{item.label}</span>
              </div>
            {/each}
          </div>
        </div>
      {/each}
    {/if}
  </div>
</aside>

<style>
  .palette {
    width: 200px;
    min-width: 200px;
    display: flex;
    flex-direction: column;
    background: rgba(26, 26, 46, 0.95);
    border-right: 1px solid #2d2d44;
    backdrop-filter: blur(12px);
    overflow-y: auto;
    z-index: 5;
  }

  .palette-header {
    padding: 12px 16px;
    border-bottom: 1px solid #2d2d44;
  }

  .palette-title {
    font-size: 11px;
    font-weight: 700;
    text-transform: uppercase;
    letter-spacing: 0.8px;
    color: #64748b;
  }

  .palette-body {
    padding: 8px;
    display: flex;
    flex-direction: column;
    gap: 12px;
  }

  .palette-empty {
    padding: 16px 8px;
    text-align: center;
  }

  .empty-text {
    font-size: 10px;
    color: #475569;
    font-style: italic;
  }

  .palette-category {
    display: flex;
    flex-direction: column;
    gap: 4px;
  }

  .category-label {
    font-size: 9px;
    text-transform: uppercase;
    letter-spacing: 0.6px;
    color: #475569;
    padding: 0 4px;
    margin-bottom: 2px;
  }

  .palette-items {
    display: flex;
    flex-direction: column;
    gap: 3px;
  }

  .palette-tile {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 8px 10px;
    border-radius: 8px;
    border: 1px solid rgba(255, 255, 255, 0.05);
    background: rgba(0, 0, 0, 0.2);
    cursor: grab;
    transition: all 0.15s;
    user-select: none;
  }

  .palette-tile:hover {
    background: rgba(99, 102, 241, 0.08);
    border-color: var(--tile-color);
    transform: translateX(2px);
  }

  .palette-tile:active {
    cursor: grabbing;
    transform: scale(0.97);
  }

  .tile-icon {
    font-size: 14px;
  }

  .tile-label {
    font-size: 11px;
    color: #cbd5e1;
    font-weight: 500;
  }

  @media (max-width: 768px) {
    .palette {
      width: 100%;
      min-width: unset;
      max-height: 120px;
      flex-direction: row;
      border-right: none;
      border-bottom: 1px solid #2d2d44;
      overflow-x: auto;
      overflow-y: hidden;
    }

    .palette-body {
      flex-direction: row;
      gap: 8px;
    }

    .palette-items {
      flex-direction: row;
    }
  }
</style>
