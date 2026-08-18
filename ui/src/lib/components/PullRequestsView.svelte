<script lang="ts">

  import { onMount } from 'svelte';
  import CollectionView from './CollectionView.svelte';
  import StatusPill from './StatusPill.svelte';

  let pull_requests: Record<string, unknown>[] = $state([]);
  let loading: boolean = $state(false);
  let error: string = $state('');
  let active_filter: string = $state('All');
  let pr_href_tpl: string = $state("/pulls/{id}");

  async function loadPulls(filter: string) {
    loading = true;
    error = '';
    try {
      const u = new URL('/api/pull_requests', window.location.origin);
      if (filter && filter !== 'All') u.searchParams.set('status', filter);
      const r = await fetch(u.toString(), { signal: AbortSignal.timeout(20000) });
      if (!r.ok) throw new Error((await r.text()) || `HTTP ${r.status}`);
      const data = await r.json();
      pull_requests = Array.isArray(data) ? data : data.pull_requests || data.items || [];
    } catch (e) {
      error = e instanceof Error ? e.message : String(e);
    } finally {
      loading = false;
    }
  }

  onMount(() => {
    void loadPulls(active_filter);
  });

  async function set_filter(filter: string) {
    active_filter = filter;
    await loadPulls(filter);
  }
</script>

<div class="changes">
  <CollectionView
    title="Changes"
    description="Pull requests — branch-based review workflow"
    items={pull_requests}
    loading={loading}
    error={error}
    view_mode="list"
    default_layout="list"
    layout_storage_key="veil_runtime.changes.layout"
    item_href_template={pr_href_tpl}
    show_avatar={false}
    primary_href="/pulls/new"
    primary_label="Create"
    empty_title="No pull requests"
    empty_description="Create a pull request to start a branch-based review workflow."
    empty_action_href="/pulls/new"
    empty_action_label="Create pull request"
    columns={[
      { key: 'title', label: 'Title', cell: 'identity', showAvatar: false, subtitleKey: 'source_branch' },
      { key: 'status', label: 'Status' },
      { key: 'author', label: 'Author' },
      { key: 'jira_ticket', label: 'Ticket' },
      { key: 'updated_at', label: 'Updated' },
    ]}
    agent={{
      intent: 'list-change-requests',
      entity: 'PullRequest',
      entityLabel: 'Pull Request',
      actions: [
        { id: 'create', label: 'Create', href: '/pulls/new', via: 'primary', method: 'navigate' },
        { id: 'view', label: 'View', hrefTemplate: '/pulls/{id}', via: 'item-click', method: 'navigate' },
      ],
      api: { list: 'GET /api/pull_requests', get: 'GET /api/pull_requests/{id}' },
    }}
  >
    {#snippet header_below()}
      <div class="pr-filters">
        {#each ['All', 'ReadyForReview', 'Approved', 'Draft', 'Merged'] as f}
          <button
            type="button"
            class="pr-filter-tab"
            class:cr-filter-tab--active={active_filter === f}
            onclick={() => { set_filter(f); }}
          >
            {f === 'ReadyForReview' ? 'Ready for Review' : f}
          </button>
        {/each}
      </div>
    {/snippet}
    {#snippet row(item)}
      {@const pr = /** @type {Record<string, unknown>} */ (item)}
      {@const crid = pr?.id != null && typeof pr.id === 'object' && pr.id.value != null ? String(pr.id.value) : String(pr?.id ?? '')}
      {@const status = String(pr?.status ?? 'Draft')}
      {@const statusVariant = status === 'Draft' ? 'neutral' : status === 'ReadyForReview' ? 'info' : status === 'Approved' ? 'success' : status === 'ChangesRequested' ? 'warning' : status === 'Merged' ? 'success' : status === 'Rejected' ? 'error' : 'neutral'}
      <td>
        <div class="pr-title-cell">
          <span class="pr-title">{pr?.title ? String(pr.title) : '—'}</span>
          <span class="pr-branch">{pr?.source_branch ? String(pr.source_branch) : ''}</span>
        </div>
      </td>
      <td>
        <StatusPill label={status === 'ReadyForReview' ? 'Ready for Review' : status} variant={statusVariant} />
      </td>
      <td><span class="dk-tile__meta">{pr?.author ? String(pr.author) : '—'}</span></td>
      <td><span class="dk-tile__meta">{pr?.jira_ticket ? String(pr.jira_ticket) : '—'}</span></td>
      <td><span class="dk-tile__meta">{pr?.updated_at ? String(pr.updated_at).slice(0, 10) : '—'}</span></td>
    {/snippet}
  </CollectionView>
</div>


<style>
.changes { max-width: 1120px; animation: dk-fade-in var(--dk-dur-slow, 420ms) var(--dk-ease-out, ease) both; }
.pr-filters {
  display: flex;
  gap: 0.25rem;
  padding: 0.5rem 0 0.75rem;
  border-bottom: 1px solid var(--dk-border-soft, var(--border));
  margin-bottom: 0.5rem;
  flex-wrap: wrap;
}
.pr-filter-tab {
  font-size: 0.8rem;
  font-weight: 550;
  padding: 0.35rem 0.75rem;
  border-radius: 999px;
  border: 1px solid transparent;
  background: transparent;
  color: var(--text-dim);
  cursor: pointer;
  transition:
    color var(--dk-dur-fast, 140ms) ease,
    background var(--dk-dur-fast, 140ms) ease,
    border-color var(--dk-dur-fast, 140ms) ease;
}
.pr-filter-tab:hover {
  color: var(--accent);
  background: color-mix(in srgb, var(--accent) 8%, transparent);
}
.pr-filter-tab--active {
  color: var(--accent);
  background: color-mix(in srgb, var(--accent) 12%, transparent);
  border-color: color-mix(in srgb, var(--accent) 30%, transparent);
}
.pr-title-cell { display: flex; flex-direction: column; gap: 0.15rem; }
.pr-title { font-weight: 550; font-size: 0.88rem; }
.pr-branch { font-size: 0.75rem; color: var(--dk-text-muted); font-family: ui-monospace, SFMono-Regular, Menlo, monospace; }

</style>
