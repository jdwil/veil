<script lang="ts">

  interface Props {
    items?: Record<string, unknown>[];
    align?: string;
    aria_label?: string;
    children?: Snippet | null;
    agent?: Record<string, unknown>;
  }
  let { items = [], align = "right", aria_label = "Actions", children, agent = {  } }: Props = $props();

  let open: boolean = $state(false);
  let root_el: Any = $state(undefined as any);
  let menu_el: Any = $state(undefined as any);

  let veil_agent = $derived({ version: 1, role: "context-menu", product: agent, runtime: { open } });

  function close() {
    open = false;
  }
  function open_menu() {
    open = true;
  }
  function toggle() {
    open = !open;
  }
</script>

<svelte:window
  onclick={(e) => {
    if (!open) return;
    const t = e.target;
    if (root_el?.contains?.(t) || menu_el?.contains?.(t)) return;
    open = false;
    const menu = menu_el;
    if (menu && typeof menu.hidePopover === 'function') {
      try { menu.hidePopover(); } catch (_) {}
    }
  }}
  onkeydown={(e) => {
    if (e.key === 'Escape' && open) {
      open = false;
      const menu = menu_el;
      if (menu && typeof menu.hidePopover === 'function') {
        try { menu.hidePopover(); } catch (_) {}
      }
    }
  }}
/>
<div
  class="dk-ctx"
  class:dk-ctx--open={open}
  bind:this={root_el}
  data-veil-role="context-menu"
  data-veil-agent={JSON.stringify(veil_agent)}
>
  <button
    type="button"
    class="dk-ctx__trigger"
    aria-label={aria_label}
    aria-haspopup="menu"
    aria-expanded={open}
    onclick={(e) => {
      e.stopPropagation();
      e.preventDefault();
      const willOpen = !open;
      open = willOpen;
      const menu = menu_el;
      const root = root_el;
      const al = align;
      // After Svelte applies hidden={!open}, place menu above stacking contexts.
      queueMicrotask(() => {
        if (!menu || !root) return;
        if (!willOpen) {
          if (typeof menu.hidePopover === 'function') {
            try { menu.hidePopover(); } catch (_) {}
          }
          return;
        }
        const r = root.getBoundingClientRect();
        menu.style.position = 'fixed';
        menu.style.margin = '0';
        menu.style.inset = 'auto';
        menu.style.top = `${Math.round(r.bottom + 4)}px`;
        menu.style.zIndex = '10000';
        menu.style.pointerEvents = 'auto';
        menu.style.background = 'color-mix(in srgb, var(--dk-surface-2, #242424) 94%, transparent)';
        if (al === 'right') {
          menu.style.right = `${Math.round(window.innerWidth - r.right)}px`;
          menu.style.left = 'auto';
        } else {
          menu.style.left = `${Math.round(r.left)}px`;
          menu.style.right = 'auto';
        }
        if (typeof menu.showPopover === 'function') {
          try {
            if (!menu.matches(':popover-open')) menu.showPopover();
            return;
          } catch (_) { /* fall through to body portal */ }
        }
        if (menu.parentElement !== document.body) document.body.appendChild(menu);
      });
    }}
  >
    <span class="dk-ctx__dots" aria-hidden="true">⋮</span>
  </button>
  <div
    bind:this={menu_el}
    class="dk-ctx__menu"
    class:dk-ctx__menu--left={align === 'left'}
    class:dk-ctx__menu--right={align === 'right'}
    role="menu"
    popover="manual"
  >
    {#if children}
      {@render children()}
    {:else}
      {#each (items || []) as item}
        {#if item.href && !item.disabled}
          <a
            href={item.href}
            class="dk-ctx__item"
            class:dk-ctx__item--danger={item.danger}
            role="menuitem"
            tabindex={open ? 0 : -1}
            onclick={(e) => {
              e.preventDefault();
              e.stopPropagation();
              open = false;
              const menu = menu_el;
              if (menu && typeof menu.hidePopover === 'function') {
                try { menu.hidePopover(); } catch (_) {}
              }
              if (item.onSelect) item.onSelect();
              else if (item.href) window.location.href = item.href;
            }}
          >{item.label}</a>
        {:else}
          <button
            type="button"
            class="dk-ctx__item"
            class:dk-ctx__item--danger={item.danger}
            role="menuitem"
            disabled={item.disabled}
            tabindex={open ? 0 : -1}
            onclick={(e) => {
              e.stopPropagation();
              if (!item.disabled) {
                open = false;
                const menu = menu_el;
                if (menu && typeof menu.hidePopover === 'function') {
                  try { menu.hidePopover(); } catch (_) {}
                }
                if (item.onSelect) item.onSelect();
              }
            }}
          >{item.label}</button>
        {/if}
      {/each}
    {/if}
  </div>
</div>


<style>
/* Popover top-layer + body portal: beat table/tile stacking without app.css edits */
.dk-ctx__menu[popover],
.dk-ctx__menu:popover-open {
  margin: 0;
  inset: auto;
  border: 1px solid var(--dk-glass-border, rgba(255, 255, 255, 0.06));
  background: color-mix(in srgb, var(--dk-surface-2, #242424) 94%, transparent);
  color: var(--dk-text, #e5e5e5);
  padding: 0.35rem;
  min-width: 10rem;
  border-radius: var(--dk-radius-sm, 0.55rem);
  box-shadow: var(--dk-shadow-lg, 0 24px 64px rgba(0, 0, 0, 0.55));
  overflow: visible;
  pointer-events: auto;
}
.dk-ctx__item {
  display: flex;
  align-items: center;
  width: 100%;
  padding: 0.55rem 0.8rem;
  border: none;
  background: transparent;
  color: inherit;
  font: inherit;
  text-align: left;
  text-decoration: none;
  cursor: pointer;
  border-radius: 0.4rem;
}
.dk-ctx__item--danger { color: #f87171; }
.dk-ctx__item:hover:not(:disabled) {
  background: rgba(255, 255, 255, 0.07);
}

</style>
