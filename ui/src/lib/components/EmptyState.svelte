<script lang="ts">

  interface Props {
    title: string;
    description?: string;
    action_href?: string;
    action_label?: string;
    agent?: Record<string, unknown>;
  }
  let { title, description = "", action_href = "", action_label = "", agent = {  } }: Props = $props();

  let has_action = $derived(action_href !== "");
  let veil_agent = $derived({ version: 1, role: "empty-state", product: agent, runtime: { has_action, action_href, action_label } });
</script>

<div class="dk-empty card" data-veil-role="empty-state" data-veil-agent={JSON.stringify(veil_agent)}>
  <div class="dk-empty__icon" aria-hidden="true">
    <svg width="32" height="32" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5">
      <path d="M10 13a5 5 0 0 0 7.54.54l3-3a5 5 0 0 0-7.07-7.07l-1.72 1.71" />
      <path d="M14 11a5 5 0 0 0-7.54-.54l-3 3a5 5 0 0 0 7.07 7.07l1.71-1.71" />
    </svg>
  </div>
  <h3 class="dk-empty__title">{title}</h3>
  {#if description}
    <p class="dk-empty__desc">{description}</p>
  {/if}
  {#if action_href && action_label}
    <a href={action_href} class="btn-primary" data-veil-action="empty-cta">{action_label}</a>
  {/if}
</div>


<style>
  /* TODO: Add component styles */
</style>
