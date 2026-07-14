<script lang="ts">
  /**
   * Bottom review dock — VEIL source + Agent.
   * Resizable height, single-panel tabs, or side-by-side split.
   */
  import VeilSourcePanel from './VeilSourcePanel.svelte';
  import AetherAgentPanel from './AetherAgentPanel.svelte';
  import { selectedNodeId, irGraph } from '$lib/store';

  type DockTab = 'source' | 'agent' | 'split';

  const HEIGHT_KEY = 'veil.reviewDock.height';
  const MODE_KEY = 'veil.reviewDock.mode';
  const SPLIT_KEY = 'veil.reviewDock.splitRatio';
  const MIN_H = 140;
  const MAX_H_RATIO = 0.75;
  const DEFAULT_H = 280;

  function loadNum(key: string, fallback: number, min: number, max: number): number {
    if (typeof localStorage === 'undefined') return fallback;
    const raw = localStorage.getItem(key);
    if (!raw) return fallback;
    const n = Number(raw);
    if (!Number.isFinite(n)) return fallback;
    return Math.min(max, Math.max(min, n));
  }

  function loadMode(): DockTab {
    if (typeof localStorage === 'undefined') return 'source';
    const m = localStorage.getItem(MODE_KEY);
    if (m === 'source' || m === 'agent' || m === 'split') return m;
    return 'source';
  }

  let tab = $state<DockTab>(loadMode());
  let expanded = $state(true);
  let agentInsert = $state('');
  let heightPx = $state(loadNum(HEIGHT_KEY, DEFAULT_H, MIN_H, 900));
  /** Source pane share in split mode (0.2–0.8). */
  let splitRatio = $state(loadNum(SPLIT_KEY, 0.48, 0.2, 0.8));

  let resizing = $state(false);
  let splitResizing = $state(false);
  let dockEl: HTMLDivElement | null = $state(null);

  /** Selection chip for agent "insert context". */
  let selectionLabel = $derived.by(() => {
    const sid = $selectedNodeId;
    const g = $irGraph;
    if (!sid || !g) return null;
    const n = g.nodes.find((x) => String(x.id) === sid);
    if (!n) return null;
    const kind = n.metadata.subkind || n.kind;
    return `${kind} ${n.name}`;
  });

  function setTab(next: DockTab) {
    expanded = true;
    tab = next;
    try {
      localStorage.setItem(MODE_KEY, next);
    } catch {
      /* ignore */
    }
  }

  function insertSelection() {
    if (!selectionLabel) return;
    agentInsert = selectionLabel;
    if (tab === 'source') setTab('agent');
    queueMicrotask(() => {
      agentInsert = '';
    });
  }

  function maxHeight(): number {
    return Math.floor(window.innerHeight * MAX_H_RATIO);
  }

  function onResizePointerDown(e: PointerEvent) {
    if (!expanded) {
      expanded = true;
    }
    e.preventDefault();
    resizing = true;
    const startY = e.clientY;
    const startH = heightPx;
    const target = e.currentTarget as HTMLElement;
    try {
      target.setPointerCapture(e.pointerId);
    } catch {
      /* synthetic / non-capturable pointers still use window listeners */
    }

    function onMove(ev: PointerEvent) {
      // Dragging the top edge upward increases height.
      const next = startH + (startY - ev.clientY);
      heightPx = Math.min(maxHeight(), Math.max(MIN_H, next));
    }
    function onUp(ev: PointerEvent) {
      resizing = false;
      try {
        target.releasePointerCapture(ev.pointerId);
      } catch {
        /* ignore */
      }
      window.removeEventListener('pointermove', onMove);
      window.removeEventListener('pointerup', onUp);
      try {
        localStorage.setItem(HEIGHT_KEY, String(Math.round(heightPx)));
      } catch {
        /* ignore */
      }
    }
    window.addEventListener('pointermove', onMove);
    window.addEventListener('pointerup', onUp);
  }

  function onSplitPointerDown(e: PointerEvent) {
    e.preventDefault();
    e.stopPropagation();
    if (!dockEl) return;
    splitResizing = true;
    const target = e.currentTarget as HTMLElement;
    try {
      target.setPointerCapture(e.pointerId);
    } catch {
      /* ignore */
    }

    function onMove(ev: PointerEvent) {
      if (!dockEl) return;
      const rect = dockEl.getBoundingClientRect();
      if (rect.width <= 0) return;
      const x = ev.clientX - rect.left;
      splitRatio = Math.min(0.8, Math.max(0.2, x / rect.width));
    }
    function onUp(ev: PointerEvent) {
      splitResizing = false;
      try {
        target.releasePointerCapture(ev.pointerId);
      } catch {
        /* ignore */
      }
      window.removeEventListener('pointermove', onMove);
      window.removeEventListener('pointerup', onUp);
      try {
        localStorage.setItem(SPLIT_KEY, String(splitRatio));
      } catch {
        /* ignore */
      }
    }
    window.addEventListener('pointermove', onMove);
    window.addEventListener('pointerup', onUp);
  }

  function onHeightKey(e: KeyboardEvent) {
    if (e.key === 'ArrowUp') {
      e.preventDefault();
      heightPx = Math.min(maxHeight(), heightPx + 24);
      localStorage.setItem(HEIGHT_KEY, String(heightPx));
    } else if (e.key === 'ArrowDown') {
      e.preventDefault();
      heightPx = Math.max(MIN_H, heightPx - 24);
      localStorage.setItem(HEIGHT_KEY, String(heightPx));
    }
  }
</script>

<div
  class="review-dock"
  class:collapsed={!expanded}
  class:resizing
  class:split-resizing={splitResizing}
  style:height={expanded ? `${heightPx}px` : undefined}
  bind:this={dockEl}
>
  <div
    class="resize-handle"
    role="separator"
    aria-orientation="horizontal"
    aria-label="Resize review panel"
    aria-valuenow={Math.round(heightPx)}
    aria-valuemin={MIN_H}
    tabindex="0"
    onpointerdown={onResizePointerDown}
    onkeydown={onHeightKey}
    title="Drag to resize"
  ></div>

  <div class="dock-chrome">
    <div class="dock-tabs" role="tablist" aria-label="Review panels">
      <button
        type="button"
        role="tab"
        class="dock-tab"
        class:active={tab === 'source'}
        aria-selected={tab === 'source'}
        onclick={() => setTab('source')}
      >
        VEIL source
      </button>
      <button
        type="button"
        role="tab"
        class="dock-tab"
        class:active={tab === 'agent'}
        aria-selected={tab === 'agent'}
        onclick={() => setTab('agent')}
      >
        Agent
      </button>
      <button
        type="button"
        role="tab"
        class="dock-tab"
        class:active={tab === 'split'}
        aria-selected={tab === 'split'}
        title="Source and agent side by side"
        onclick={() => setTab('split')}
      >
        Split
      </button>
    </div>
    <div class="dock-actions">
      {#if selectionLabel}
        <button
          type="button"
          class="insert-btn"
          title="Insert selected construct into agent prompt"
          onclick={insertSelection}
        >
          + Insert “{selectionLabel}”
        </button>
      {/if}
      <span class="height-hint" title="Panel height">{Math.round(heightPx)}px</span>
      <button
        type="button"
        class="collapse-btn"
        title={expanded ? 'Collapse dock' : 'Expand dock'}
        onclick={() => (expanded = !expanded)}
      >
        {expanded ? '▾' : '▴'}
      </button>
    </div>
  </div>
  {#if expanded}
    <!--
      Keep a single Source + Agent instance mounted for all tabs.
      {#if tab} branches remounted panels and wiped agent conversation mid-stream.
    -->
    <div
      class="dock-body"
      class:split={tab === 'split'}
      class:source-only={tab === 'source'}
      class:agent-only={tab === 'agent'}
    >
      <div
        class="split-pane source-pane"
        class:pane-hidden={tab === 'agent'}
        hidden={tab === 'agent'}
        aria-hidden={tab === 'agent'}
        style:flex={tab === 'split' ? `0 0 ${splitRatio * 100}%` : undefined}
      >
        {#if tab === 'split'}
          <div class="pane-label">Source</div>
        {/if}
        <VeilSourcePanel embedded />
      </div>
      {#if tab === 'split'}
        <div
          class="split-handle"
          role="separator"
          aria-orientation="vertical"
          aria-label="Resize source / agent split"
          tabindex="0"
          onpointerdown={onSplitPointerDown}
          title="Drag to resize panes"
        ></div>
      {/if}
      <div
        class="split-pane agent-pane"
        class:pane-hidden={tab === 'source'}
        hidden={tab === 'source'}
        aria-hidden={tab === 'source'}
      >
        {#if tab === 'split'}
          <div class="pane-label">Agent</div>
        {/if}
        <AetherAgentPanel embedded insertToken={agentInsert} />
      </div>
    </div>
  {/if}
</div>

<style>
  .review-dock {
    position: relative;
    flex-shrink: 0;
    display: flex;
    flex-direction: column;
    border-top: 1px solid var(--veil-border);
    background: var(--veil-surface);
    min-height: 0;
    /* Above CodePreview (z=10) so Agent Send / resize stay clickable */
    z-index: 20;
  }
  .review-dock.collapsed {
    height: auto !important;
  }
  .review-dock.resizing,
  .review-dock.split-resizing {
    user-select: none;
  }
  .resize-handle {
    position: absolute;
    top: -3px;
    left: 0;
    right: 0;
    height: 7px;
    cursor: ns-resize;
    z-index: 6;
    touch-action: none;
  }
  .resize-handle::after {
    content: '';
    position: absolute;
    left: 50%;
    top: 2px;
    transform: translateX(-50%);
    width: 36px;
    height: 3px;
    border-radius: 2px;
    background: var(--veil-border);
    opacity: 0.7;
  }
  .resize-handle:hover::after,
  .review-dock.resizing .resize-handle::after {
    background: var(--veil-accent, #60a5fa);
    opacity: 1;
  }
  .dock-chrome {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 8px;
    padding: 0 8px;
    border-bottom: 1px solid var(--veil-border);
    flex-shrink: 0;
    background: var(--veil-surface-alt);
  }
  .dock-tabs {
    display: flex;
    gap: 2px;
  }
  .dock-tab {
    background: none;
    border: none;
    border-bottom: 2px solid transparent;
    color: var(--veil-text-dim);
    font-size: 11px;
    font-weight: 700;
    letter-spacing: 0.03em;
    padding: 8px 12px;
    cursor: pointer;
  }
  .dock-tab:hover {
    color: var(--veil-text);
  }
  .dock-tab.active {
    color: var(--veil-text);
    border-bottom-color: var(--veil-accent, #60a5fa);
  }
  .dock-actions {
    display: flex;
    align-items: center;
    gap: 8px;
  }
  .height-hint {
    font-size: 9px;
    font-family: 'JetBrains Mono', monospace;
    color: var(--veil-text-faint);
  }
  .insert-btn {
    font-size: 10px;
    padding: 3px 8px;
    border-radius: 4px;
    border: 1px solid var(--veil-border);
    background: var(--veil-accent-subtle);
    color: var(--veil-text-secondary);
    cursor: pointer;
    max-width: 280px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .insert-btn:hover {
    color: var(--veil-text);
    border-color: var(--veil-accent, #60a5fa);
  }
  .collapse-btn {
    background: none;
    border: none;
    color: var(--veil-text-dim);
    cursor: pointer;
    font-size: 12px;
    padding: 4px 6px;
  }
  .dock-body {
    flex: 1;
    min-height: 0;
    display: flex;
    align-items: stretch;
    overflow: hidden;
  }
  /* Default single-tab modes stack as a column of one visible pane */
  .dock-body.source-only,
  .dock-body.agent-only {
    flex-direction: column;
  }
  .dock-body.split {
    flex-direction: row;
  }
  .dock-body.source-only .source-pane,
  .dock-body.agent-only .agent-pane {
    flex: 1 1 auto;
    width: 100%;
    height: 100%;
  }
  .split-pane {
    min-width: 0;
    min-height: 0;
    display: flex;
    flex-direction: column;
    overflow: hidden;
  }
  /*
   * Must beat `.split-pane { display: flex }` — HTML [hidden] alone loses to
   * author display rules, so the inactive pane stayed visible on Agent tab.
   */
  .split-pane.pane-hidden,
  .split-pane[hidden] {
    display: none !important;
  }
  .agent-pane {
    flex: 1 1 auto;
  }
  .pane-label {
    flex-shrink: 0;
    font-size: 9px;
    font-weight: 700;
    letter-spacing: 0.06em;
    text-transform: uppercase;
    color: var(--veil-text-faint);
    padding: 3px 10px;
    border-bottom: 1px solid var(--veil-border);
    background: var(--veil-surface-alt);
  }
  .split-handle {
    flex: 0 0 5px;
    cursor: col-resize;
    background: var(--veil-border);
    position: relative;
    touch-action: none;
  }
  .split-handle:hover,
  .review-dock.split-resizing .split-handle {
    background: var(--veil-accent, #60a5fa);
  }
</style>
