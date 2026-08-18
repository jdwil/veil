<script lang="ts">

  import { onMount } from 'svelte';
  import PageHeader from './PageHeader.svelte';
  import StatCard from './StatCard.svelte';
  import CollectionView from './CollectionView.svelte';

  let repos: Record<string, unknown>[] = $state([]);
  let loading: boolean = $state(false);
  let error: string = $state('');
  let project_href_tpl: string = $state("/projects/{id}");

  onMount(() => {
    void (async () => {
      loading = true;
      error = '';
      try {
        const r = await fetch('/api/repos', { signal: AbortSignal.timeout(20000) });
        if (!r.ok) throw new Error((await r.text()) || `HTTP ${r.status}`);
        const data = await r.json();
        repos = Array.isArray(data) ? data : data.repos || data.items || [];
      } catch (e) {
        error = e instanceof Error ? e.message : String(e);
      } finally {
        loading = false;
      }
    })();
  });
</script>

<div class="dashboard">
  <PageHeader title="Dashboard" description="Runtime overview" />
  <div class="stats">
    {#each [{v: String(repos.length), l: "Projects"}, {v: "—", l: "Deploys"}, {v: "—", l: "Layers"}, {v: "—", l: "Artifacts"}] as s}
      <StatCard value={s.v} label={s.l} />
    {/each}
  </div>
  <CollectionView
    title="Recent projects"
    description="Jump into a repository managed by this runtime."
    items={repos}
    loading={loading}
    error={error}
    view_mode="both"
    default_layout="tiles"
    layout_storage_key="veil_runtime.dashboard.projects.layout"
    item_href_template={project_href_tpl}
    empty_title="No projects yet"
    empty_description="Create a project from the Projects page."
    empty_action_href="/projects/new"
    empty_action_label="Create project"
    primary_href="/projects/new"
    primary_label="Create"
    columns={[
      { key: 'name', label: 'Name', cell: 'identity', showAvatar: true, subtitleKey: 'default_branch' },
      { key: 'default_branch', label: 'Branch' },
      { key: 'updated_at', label: 'Updated' },
      { key: 'slug', label: 'Slug' },
    ]}
    agent={{
      intent: 'dashboard-projects',
      entity: 'Repo',
      entityLabel: 'Project',
      actions: [
        { id: 'list', label: 'All projects', href: '/projects', method: 'navigate' },
        { id: 'create', label: 'Create', href: '/projects/new', method: 'navigate' },
        { id: 'view', label: 'Open', hrefTemplate: '/projects/{id}', via: 'item-click', method: 'navigate' },
      ],
      api: { list: 'GET /api/repos', get: 'GET /api/repos/{id}' },
    }}
  />
</div>


<style>
.dashboard { max-width: 1120px; animation: dk-fade-in var(--dk-dur-slow, 420ms) var(--dk-ease-out, ease) both; }
.stats { display: grid; grid-template-columns: repeat(4, 1fr); gap: 1rem; margin-bottom: 2rem; }
@media (max-width: 900px) { .stats { grid-template-columns: repeat(2, 1fr); } }

</style>
