<script lang="ts">

  interface Props {
    poll_ms?: number;
  }
  let { poll_ms = 0 }: Props = $props();
</script>

{#if true}
{@const publish = () => {
  if (typeof document === 'undefined' || typeof window === 'undefined') return;
  const surfaces = [];
  document.querySelectorAll('[data-veil-agent]').forEach((el) => {
    const raw = el.getAttribute('data-veil-agent');
    if (!raw) return;
    try {
      const parsed = JSON.parse(raw);
      surfaces.push({
        ...parsed,
        role: el.getAttribute('data-veil-role') || parsed.role,
        _dom: { tag: el.tagName.toLowerCase(), id: el.id || undefined },
      });
    } catch { /* ignore */ }
  });
  const payload = {
    version: 1,
    collectedAt: new Date().toISOString(),
    path: window.location.pathname,
    surfaces,
  };
  window.__veilAgentSurface = payload;
  window.dispatchEvent(new CustomEvent('veil:agent-surface', { detail: payload }));
}}
{@const _ = (() => {
  if (typeof document === 'undefined') return;
  publish();
  const mo = new MutationObserver(() => publish());
  mo.observe(document.body, { subtree: true, childList: true, attributes: true, attributeFilter: ['data-veil-agent', 'data-veil-role'] });
  return true;
})()}
<div
  class="dk-agent-surface-host"
  data-veil-role="agent-surface-host"
  data-veil-agent={JSON.stringify({
    version: 1,
    role: 'agent-surface-host',
    stock: {
      component: 'AgentSurface',
      purpose: 'Collects page agent contracts into window.__veilAgentSurface',
      howTo: ['Read window.__veilAgentSurface after load or on veil:agent-surface'],
      config: {},
    },
  })}
  hidden
  aria-hidden="true"
></div>
{/if}


<style>
  /* TODO: Add component styles */
</style>
