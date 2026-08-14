<script lang="ts">

  interface Props {
    title: string;
    subtitle?: string;
    back_href?: string;
    back_behavior?: string;
    loading?: boolean;
    loading_label?: string;
    error?: string;
    children?: Snippet | null;
    header_actions?: Snippet | null;
    summary?: Snippet | null;
    sidebar?: Snippet | null;
    footer?: Snippet | null;
    agent?: Record<string, unknown>;
  }
  let { title, subtitle = "", back_href = "..", back_behavior = "href", loading = false, loading_label = "Loading…", error = "", children, header_actions, summary, sidebar, footer, agent = {  } }: Props = $props();

  let veil_agent = $derived({ version: 1, role: "detail-view", product: agent, runtime: { loading, error, title, back_href, back_behavior } });
</script>

<div
  class="dk-page-shell dk-detail-shell dk-create-shell"
  data-veil-role="detail-view"
  data-veil-agent={JSON.stringify(veil_agent)}
>
  <header class="dk-page-shell__header dk-create-shell__header dk-detail-shell__header">
    <div class="dk-page-shell__left dk-create-shell__left">
      <a
        href={back_href}
        class="btn-ghost dk-page-shell__back dk-create-shell__back"
        aria-label="Back"
        data-veil-action="back"
        data-back-behavior={back_behavior}
        onclick={(e) => {
          if (back_behavior !== 'history') return;
          if (typeof window === 'undefined') return;
          if (window.history.length > 1) {
            e.preventDefault();
            window.history.back();
          }
        }}
      >
        <svg width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" aria-hidden="true">
          <path d="M19 12H5M12 19l-7-7 7-7" />
        </svg>
      </a>
      <div>
        <h1 class="dk-page-shell__title dk-create-shell__title">{title}</h1>
        {#if subtitle}
          <p class="dk-page-shell__subtitle dk-create-shell__subtitle">{subtitle}</p>
        {/if}
      </div>
    </div>
    {#if header_actions}
      <div class="dk-page-shell__header-actions" data-veil-slot="header_actions">
        {@render header_actions()}
      </div>
    {/if}
  </header>
  <div class="dk-page-shell__body dk-create-shell__body dk-detail-shell__body">
    {#if loading}
      <div class="card dk-loading">
        <div class="dk-spinner" aria-hidden="true"></div>
        <span>{loading_label}</span>
      </div>
    {:else}
      {#if error}
        <p class="dk-error">{error}</p>
      {/if}
      {#if summary}
        <div class="dk-detail-shell__summary" data-veil-slot="summary">
          {@render summary()}
        </div>
      {/if}
      {#if sidebar}
        <div class="dk-detail-shell__layout">
          <div class="dk-detail-shell__content" data-veil-slot="children">
            {#if children}
              {@render children()}
            {/if}
          </div>
          <aside class="dk-detail-shell__sidebar" data-veil-slot="sidebar">
            {@render sidebar()}
          </aside>
        </div>
      {:else if children}
        <div class="dk-detail-shell__children" data-veil-slot="children">
          {@render children()}
        </div>
      {/if}
      {#if footer}
        <div class="dk-page-shell__footer" data-veil-slot="footer">
          {@render footer()}
        </div>
      {/if}
    {/if}
  </div>
</div>


<style>
  /* TODO: Add component styles */
</style>
