<script lang="ts">
  import { NODE_STYLES, type NodeKind } from '$lib/types';
  import { paletteConfig, stubs } from '$lib/store';

  // Format a stub method signature for the hover tooltip.
  function stubSig(m: { name: string; params: [string, string][]; return_type: string | null }): string {
    const params = m.params.map(([n, t]) => `${n}: ${t}`).join(', ');
    const ret = m.return_type ? ` -> ${m.return_type}` : '';
    return `${m.name}(${params})${ret}`;
  }

  interface PaletteItem {
    kind: NodeKind;
    label: string;
    icon: string;
    color?: string;
    category: string;
    name?: string;
    keyword?: string;
    group?: string;
    dg?: string;
  }

  let { contextKind = "Solution", contextKindCore = "Solution", activeGroup = null }: { contextKind?: NodeKind | null; contextKindCore?: string; activeGroup?: string | null } = $props();

  // Build palette items from API config, falling back to hardcoded if not loaded
  let items = $derived.by(() => {
    const config = $paletteConfig;
    if (!config || config.length === 0) return fallbackItems();

    const ck = contextKind ?? 'Solution';
    const results: PaletteItem[] = [];

    for (const c of config) {
      // Check if this construct is allowed in the current context
      let show = false;
      if (ck === 'Solution' && c.allowed_in === 'top') show = true;
      else if (c.allowed_in === ck || c.allowed_in === contextKindCore || c.allowed_in.split(',').map(s => s.trim()).includes(ck) || c.allowed_in.split(',').map(s => s.trim()).includes(contextKindCore)) {
        // Check group match
        if (c.group && activeGroup) {
          show = c.group === activeGroup;
        } else if (!c.group) {
          show = true;
        } else if (!activeGroup) {
          show = true;
        }
      }
      else if (c.allowed_in === 'any' && ck !== 'Solution') {
        // Group is structural — only show it when NOT already inside a group.
        if (c.kind === 'Group' && activeGroup) {
          show = false;
        } else {
          show = true;
        }
      }

      if (show) {
        results.push({
          kind: c.kind as NodeKind,
          label: c.label,
          icon: c.icon,
          color: c.color,
          category: c.group || 'General',
          name: c.name,
          keyword: c.keyword,
          group: c.group || undefined,
          dg: c.dg || undefined,
        });
      }
    }

    return results;
  });

  function fallbackItems(): PaletteItem[] {
    // Core-shape fallback when the palette API isn't available.
    // Real vocabulary always comes from /api/palette (layer files).
    if (contextKind === 'Solution') {
      return [
        { kind: 'Module', label: 'Module', icon: '📦', category: 'General' },
        { kind: 'Flow', label: 'Flow', icon: '🌊', category: 'General' },
      ];
    }
    return [];
  }

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
                style="--tile-color: {item.color || NODE_STYLES[item.kind]?.color || 'var(--veil-text-dim)'}"
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
    background: var(--veil-surface-alt);
    border-right: 1px solid var(--veil-border);
    backdrop-filter: blur(12px);
    overflow-y: auto;
    z-index: 5;
  }

  .palette-header {
    padding: 12px 16px;
    border-bottom: 1px solid var(--veil-border);
  }

  .palette-title {
    font-size: 11px;
    font-weight: 700;
    text-transform: uppercase;
    letter-spacing: 0.8px;
    color: var(--veil-text-dim);
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
    color: var(--veil-text-faint);
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
    color: var(--veil-text-faint);
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
    background: var(--veil-code-bg);
    cursor: grab;
    transition: all 0.15s;
    user-select: none;
  }

  .palette-tile:hover {
    background: var(--veil-accent-subtle);
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
    color: var(--veil-text);
    font-weight: 500;
  }

  @media (max-width: 768px) {
    .palette {
      width: 100%;
      min-width: unset;
      max-height: 120px;
      flex-direction: row;
      border-right: none;
      border-bottom: 1px solid var(--veil-border);
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

  /* External stub crates (UX-006) */
  .stub-crate {
    margin-bottom: 8px;
  }
  .stub-name {
    display: block;
    font-size: 11px;
    font-weight: 700;
    color: var(--veil-text);
    padding: 2px 0;
  }
  .stub-version {
    font-weight: 400;
    color: var(--veil-text-dim);
    margin-left: 6px;
  }
  .stub-struct {
    display: flex;
    align-items: center;
    gap: 6px;
    padding: 3px 8px;
    border-radius: 4px;
    background: rgba(148, 163, 184, 0.06);
    border: 1px solid var(--veil-border);
    margin: 2px 0;
    cursor: help;
  }
  .stub-icon { font-size: 12px; }
  .stub-struct-name {
    font-size: 11px;
    color: var(--veil-text);
    font-family: 'JetBrains Mono', monospace;
  }
  .stub-method-count {
    font-size: 9px;
    color: var(--veil-text-dim);
    margin-left: auto;
  }
</style>
