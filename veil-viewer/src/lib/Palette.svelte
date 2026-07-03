<script lang="ts">
  import { NODE_STYLES, type NodeKind } from '$lib/types';

  interface PaletteItem {
    kind: NodeKind;
    label: string;
    icon: string;
    category: string;
  }

  let { contextKind = 'Solution', activeGroup = null }: { contextKind?: NodeKind | null; activeGroup?: string | null } = $props();

  // All available construct types grouped by category
  const ALL_PALETTE_ITEMS: PaletteItem[] = [
    // Core structural
    { kind: 'Module', label: 'Module', icon: '📦', category: 'Structure' },
    { kind: 'TypeDef', label: 'Type', icon: '📋', category: 'Structure' },
    { kind: 'Interface', label: 'Interface', icon: '🔌', category: 'Structure' },
    { kind: 'Implementation', label: 'Implementation', icon: '🔗', category: 'Structure' },
    // DDD Domain
    { kind: 'TypeDef', label: 'Value Object', icon: '💎', category: 'Domain' },
    { kind: 'TypeDef', label: 'Entity', icon: '🔑', category: 'Domain' },
    { kind: 'TypeDef', label: 'Aggregate', icon: '🧩', category: 'Domain' },
    { kind: 'Interface', label: 'Port', icon: '🔌', category: 'Domain' },
    { kind: 'Flow', label: 'Domain Service', icon: '🖥️', category: 'Domain' },
    // DDD Infrastructure
    { kind: 'Implementation', label: 'Adapter', icon: '🔗', category: 'Infrastructure' },
    // DDD Aggregate children
    { kind: 'TypeDef', label: 'Event', icon: '⚡', category: 'Aggregate' },
    { kind: 'TypeDef', label: 'Command', icon: '📨', category: 'Aggregate' },
    // Flow
    { kind: 'Flow', label: 'Flow', icon: '🌊', category: 'Flow' },
    { kind: 'Saga', label: 'Saga', icon: '🔄', category: 'Flow' },
    { kind: 'Step', label: 'Step', icon: '▶️', category: 'Flow' },
    { kind: 'ParallelGateway', label: 'Parallel', icon: '⑃', category: 'Flow' },
    { kind: 'ErrorBoundary', label: 'Error Boundary', icon: '🛡️', category: 'Flow' },
  ];

  // Context-aware filtering based on parent kind AND active group
  const ALLOWED_ITEMS: Record<string, string[]> = {
    // Top level
    'Solution': ['Structure', 'Flow'],
    // Inside a module/context with domain group active
    'Module:domain': ['Domain'],
    // Inside a module/context with infrastructure group active
    'Module:infrastructure': ['Infrastructure'],
    // Inside a module with no group
    'Module': ['Domain', 'Infrastructure'],
    // Inside an aggregate
    'TypeDef': ['Aggregate'],
    // Inside a flow
    'Flow': ['Flow'],
    'Saga': ['Flow'],
  };

  let items = $derived.by(() => {
    // Try specific key first (kind:group), then just kind
    const specificKey = activeGroup ? `${contextKind}:${activeGroup}` : null;
    const allowedCategories = (specificKey && ALLOWED_ITEMS[specificKey])
      ?? ALLOWED_ITEMS[contextKind ?? 'Solution']
      ?? [];
    if (allowedCategories.length === 0) return [];
    return ALL_PALETTE_ITEMS.filter(item => allowedCategories.includes(item.category));
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
