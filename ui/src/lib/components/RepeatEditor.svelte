<script lang="ts">

  interface Props {
    items?: Record<string, unknown>[];
    label?: string;
    add_label?: string;
    empty_label?: string;
    collapsible?: boolean;
    summary_key?: string;
    max_items?: number;
    item_template?: Snippet | null;
    on_add?: () => void | null;
    on_remove?: (arg0: number) => void | null;
    agent?: Record<string, unknown>;
  }
  let { items = $bindable([]), label = "Items", add_label = "Add item", empty_label = "No items yet", collapsible = true, summary_key = "name", max_items = 0, item_template, on_add, on_remove, agent = {  } }: Props = $props();

  let collapsed: boolean[] = $state([]);

  let can_add = $derived(max_items === 0 || items.length < max_items);
  let veil_agent = $derived({ version: 1, role: "repeat-editor", product: agent, runtime: { label, count: items.length, max_items } });

  function add_item() {
    if (on_add !== null) {
      on_add();
    } else {
      items = items.concat([{  }]);
    };
    collapsed = collapsed.concat([false]);
  }
  function remove_item(index: number) {
    if (on_remove !== null) {
      on_remove(index);
    } else {
      items = items.toSpliced(index, 1);
    };
    collapsed = collapsed.toSpliced(index, 1);
  }
  function toggle_collapse(index: number) {
    collapsed = collapsed.with(index, !collapsed[index]);
  }
</script>

{#if true}
{@const list = Array.isArray(items) ? items : []}
<div
  class="dk-repeat-editor"
  data-veil-role="repeat-editor"
  data-veil-agent={JSON.stringify(veil_agent)}
>
  <div class="dk-repeat-editor__header">
    <span class="dk-repeat-editor__label">{label} ({list.length}{max_items > 0 ? `/${max_items}` : ''})</span>
    {#if can_add}
      <button type="button" class="btn-ghost dk-repeat-editor__add" onclick={add_item} data-veil-action="add">
        <span aria-hidden="true">+</span> {add_label}
      </button>
    {/if}
  </div>
  {#if list.length === 0}
    <p class="dk-repeat-editor__empty">{empty_label}</p>
  {:else}
    <div class="dk-repeat-editor__list">
      {#each list as item, i (i)}
        {@const isCollapsed = collapsible && collapsed[i]}
        {@const summaryText = item && summary_key && item[summary_key] ? String(item[summary_key]) : `Item ${i + 1}`}
        <div class="dk-repeat-editor__item" class:dk-repeat-editor__item--collapsed={isCollapsed}>
          <div class="dk-repeat-editor__item-header">
            {#if collapsible}
              <button type="button" class="btn-ghost dk-repeat-editor__toggle" onclick={() => toggle_collapse(i)} aria-label={isCollapsed ? 'Expand' : 'Collapse'}>
                <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" style="transform: rotate({isCollapsed ? '0' : '90'}deg); transition: transform 0.15s;">
                  <path d="M9 18l6-6-6-6" />
                </svg>
              </button>
            {/if}
            <span class="dk-repeat-editor__summary">{summaryText}</span>
            <button type="button" class="btn-ghost dk-repeat-editor__remove" onclick={() => remove_item(i)} aria-label="Remove item" data-veil-action="remove">
              <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" aria-hidden="true">
                <path d="M18 6 6 18M6 6l12 12" />
              </svg>
            </button>
          </div>
          {#if !isCollapsed}
            <div class="dk-repeat-editor__item-body">
              {#if item_template}
                {@render item_template(item, i)}
              {:else}
                <pre class="dk-fallback">{JSON.stringify(item, null, 2)}</pre>
              {/if}
            </div>
          {/if}
        </div>
      {/each}
    </div>
  {/if}
</div>
{/if}


<style>
  /* TODO: Add component styles */
</style>
