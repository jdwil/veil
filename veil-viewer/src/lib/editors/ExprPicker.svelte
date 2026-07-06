<script lang="ts">
  import { EXPR_KINDS, defaultExpr, type Expr } from './expr-types';

  let { onSelect }: { onSelect: (expr: Expr) => void } = $props();
  let open = $state(false);
  let filter = $state('');

  let filtered = $derived(
    filter
      ? EXPR_KINDS.filter(k => k.label.toLowerCase().includes(filter.toLowerCase()) || k.category.toLowerCase().includes(filter.toLowerCase()))
      : EXPR_KINDS
  );

  let grouped = $derived(() => {
    const groups: Record<string, typeof EXPR_KINDS> = {};
    for (const item of filtered) {
      (groups[item.category] ??= []).push(item);
    }
    return groups;
  });

  function select(kind: Expr['kind']) {
    onSelect(defaultExpr(kind));
    open = false;
    filter = '';
  }
</script>

<div class="expr-picker">
  <button class="add-btn" onclick={() => open = !open}>+ Add</button>
  {#if open}
    <div class="picker-dropdown">
      <input
        class="picker-search"
        type="text"
        placeholder="Search expressions..."
        bind:value={filter}
      />
      <div class="picker-list">
        {#each Object.entries(grouped()) as [category, items]}
          <div class="picker-category">{category}</div>
          {#each items as item}
            <button class="picker-item" onclick={() => select(item.kind)}>
              <span class="picker-icon">{item.icon}</span>
              <span class="picker-label">{item.label}</span>
            </button>
          {/each}
        {/each}
      </div>
    </div>
  {/if}
</div>

<style>
  .expr-picker {
    position: relative;
    display: inline-block;
  }

  .add-btn {
    background: #1e40af;
    color: white;
    border: none;
    border-radius: 4px;
    padding: 4px 10px;
    font-size: 12px;
    cursor: pointer;
  }
  .add-btn:hover { background: #2563eb; }

  .picker-dropdown {
    position: absolute;
    top: 100%;
    left: 0;
    z-index: 1000;
    background: #1e293b;
    border: 1px solid #334155;
    border-radius: 6px;
    width: 220px;
    max-height: 320px;
    overflow: hidden;
    display: flex;
    flex-direction: column;
    box-shadow: 0 10px 25px rgba(0,0,0,0.4);
  }

  .picker-search {
    padding: 8px;
    border: none;
    border-bottom: 1px solid #334155;
    background: #0f172a;
    color: #e2e8f0;
    font-size: 12px;
    outline: none;
  }

  .picker-list {
    overflow-y: auto;
    max-height: 260px;
  }

  .picker-category {
    padding: 4px 8px;
    font-size: 10px;
    text-transform: uppercase;
    color: #64748b;
    font-weight: 600;
    border-top: 1px solid #1e293b;
    background: #0f172a;
  }

  .picker-item {
    display: flex;
    align-items: center;
    gap: 6px;
    width: 100%;
    padding: 6px 10px;
    border: none;
    background: transparent;
    color: #e2e8f0;
    font-size: 12px;
    cursor: pointer;
    text-align: left;
  }
  .picker-item:hover { background: #334155; }

  .picker-icon { font-size: 14px; }
  .picker-label { flex: 1; }
</style>
