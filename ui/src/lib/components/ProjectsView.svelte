<script lang="ts">

  import CollectionView from './CollectionView.svelte';
  import ContextMenu from './ContextMenu.svelte';
  import EntityIdentity from './EntityIdentity.svelte';
  import ConfirmDialog from './ConfirmDialog.svelte';
  import StatusPill from './StatusPill.svelte';
  import { onMount } from 'svelte';
  import { reviewProjects, refreshReview, reviewForSlug } from '$lib/review/store';

  let repos: Record<string, unknown>[] = $state([]);
  let loading: boolean = $state(false);
  let error: string = $state('');
  let project_href_tpl: string = $state("/projects/{id}");
  let delete_open: boolean = $state(false);
  let deleting_id: string = $state('');
  let delete_name: string = $state('');
  let delete_busy: boolean = $state(false);

  onMount(() => {
    void refreshReview();
  });

   $effect(() => { // load_on_mount
  void (async () => {
        loading = true;
        error = "";
        repos = await (async () => { const __u = new URL("/api/repos", typeof window !== 'undefined' ? window.location.origin : 'http://localhost'); const __p = {} as Record<string, unknown>; for (const [k, v] of Object.entries(__p)) { if (v != null && v !== '') __u.searchParams.set(k, String(v)); } const __r = await fetch(__u.toString()); if (!__r.ok) throw new Error(await __r.text()); return await __r.json(); })();
        void refreshReview();
        loading = false;
  })();
    });
   $effect(() => { // do_delete
  void (async () => {
        if (deleting_id !== "") {
          if (delete_busy === true) {
            error = "";
            await (async () => { const __r = await fetch(`/api/repos/${encodeURIComponent(deleting_id)}`, { method: 'DELETE' }); if (!__r.ok) throw new Error(await __r.text()); const __t = await __r.text(); return __t ? JSON.parse(__t) : null; })();
            repos = await (async () => { const __u = new URL("/api/repos", typeof window !== 'undefined' ? window.location.origin : 'http://localhost'); const __p = {} as Record<string, unknown>; for (const [k, v] of Object.entries(__p)) { if (v != null && v !== '') __u.searchParams.set(k, String(v)); } const __r = await fetch(__u.toString()); if (!__r.ok) throw new Error(await __r.text()); return await __r.json(); })();
            deleting_id = "";
            delete_name = "";
            delete_busy = false;
            delete_open = false;
          };
        };
  })();
    });
</script>

<div class="projects">
  <CollectionView
    title="Projects"
    description="Repositories managed by veil-runtime"
    items={repos}
    loading={loading}
    error={error}
    view_mode="both"
    default_layout="list"
    layout_storage_key="veil_runtime.projects.layout"
    show_avatar={true}
    primary_href="/projects/new"
    primary_label="Create"
    empty_title="No projects yet"
    empty_description="Create a project to get started with veil-runtime."
    empty_action_href="/projects/new"
    empty_action_label="Create your first project"
    item_href_template={project_href_tpl}
    columns={[
      { key: 'name', label: 'Name', cell: 'identity', showAvatar: true, subtitleKey: 'default_branch' },
      { key: 'default_branch', label: 'Branch' },
      { key: 'updated_at', label: 'Updated' },
      { key: 'slug', label: 'Slug' },
      { key: 'review', label: 'Review' },
      { key: '_actions', label: '' },
    ]}
    agent={{
      intent: 'list-projects',
      entity: 'Repo',
      entityLabel: 'Project',
      notes: [
        'Primary CTA and empty-state CTA open /projects/new.',
        'Click card or row → /projects/{id}.',
        'Row ⋮ menu: View or Delete (confirm).',
      ],
      actions: [
        { id: 'create', label: 'Create', href: '/projects/new', via: 'primary', method: 'navigate' },
        { id: 'view', label: 'View', hrefTemplate: '/projects/{id}', via: 'item-click', method: 'navigate' },
        { id: 'delete', label: 'Delete', via: 'context-menu', confirm: true, method: 'api' },
      ],
      api: {
        list: 'GET /api/repos',
        get: 'GET /api/repos/{id}',
        create: 'POST /api/repos',
        delete: 'DELETE /api/repos/{id}',
      },
    }}
  >
    {#snippet tile(item)}
      {@const repo = /** @type {Record<string, unknown>} */ (item)}
      {@const rid = repo?.id != null && typeof repo.id === 'object' && repo.id.value != null ? String(repo.id.value) : String(repo?.id ?? repo?.slug ?? '')}
      {@const rev = reviewForSlug(rid || String(repo?.slug ?? ''), $reviewProjects)}
      <div class="dk-tile__actions">
        <ContextMenu
          agent={{
            intent: 'project-row-actions',
            entity: 'Repo',
            entityLabel: 'Project',
            itemId: rid,
            actions: [
              { id: 'view', label: 'View', href: `/projects/${rid}` },
              { id: 'delete', label: 'Delete', confirm: true },
            ],
          }}
          items={[
            { label: 'View', href: `/projects/${rid}` },
            {
              label: 'Delete',
              danger: true,
              onSelect: () => {
                deleting_id = rid;
                delete_name = String(repo?.name ?? rid);
                delete_open = true;
              },
            },
          ]}
        />
      </div>
      <div class="dk-tile__row">
        <EntityIdentity
          name={String(repo?.name ?? '—')}
          subtitle={String(repo?.default_branch ?? 'main') + (repo?.slug ? ` · ${repo.slug}` : '')}
          show_avatar={true}
        />
      </div>
      {#if rev?.needs_sign_off}
        <div class="review-badges">
          <StatusPill label="Needs sign-off" variant="warning" />
          <span class="review-count">{rev.outstanding} unreviewed</span>
        </div>
      {:else if rev?.touched}
        <div class="review-badges">
          <StatusPill label="Touched" variant="info" />
        </div>
      {/if}
    {/snippet}
    {#snippet row(item)}
      {@const repo = /** @type {Record<string, unknown>} */ (item)}
      {@const rid = repo?.id != null && typeof repo.id === 'object' && repo.id.value != null ? String(repo.id.value) : String(repo?.id ?? repo?.slug ?? '')}
      {@const rev = reviewForSlug(rid || String(repo?.slug ?? ''), $reviewProjects)}
      <td>
        <EntityIdentity name={String(repo?.name ?? '—')} show_avatar={true} size="sm" />
      </td>
      <td><span class="dk-tile__meta">{repo?.default_branch ? String(repo.default_branch) : 'main'}</span></td>
      <td><span class="dk-tile__meta">{repo?.updated_at ? String(repo.updated_at).slice(0, 10) : '—'}</span></td>
      <td><span class="dk-tile__meta">{repo?.slug ? String(repo.slug) : '—'}</span></td>
      <td>
        {#if rev?.needs_sign_off}
          <StatusPill label={`Sign-off · ${rev.outstanding}`} variant="warning" />
        {:else if rev?.touched}
          <StatusPill label="Touched" variant="info" />
        {:else}
          <span class="dk-tile__meta">—</span>
        {/if}
      </td>
      <td class="dk-table__actions">
        <ContextMenu
          agent={{
            intent: 'project-row-actions',
            entity: 'Repo',
            entityLabel: 'Project',
            itemId: rid,
            actions: [
              { id: 'view', label: 'View', href: `/projects/${rid}` },
              { id: 'delete', label: 'Delete', confirm: true },
            ],
          }}
          items={[
            { label: 'View', href: `/projects/${rid}` },
            {
              label: 'Delete',
              danger: true,
              onSelect: () => {
                deleting_id = rid;
                delete_name = String(repo?.name ?? rid);
                delete_open = true;
              },
            },
          ]}
        />
      </td>
    {/snippet}
  </CollectionView>

  <ConfirmDialog
    bind:open={delete_open}
    title="Delete project"
    message={`Are you sure you want to delete “${delete_name}”? This cannot be undone.`}
    confirm_label="Delete"
    cancel_label="Cancel"
    variant="danger"
    on_confirm={() => { delete_busy = true; }}
  />
</div>


<style>
.projects { max-width: 1120px; }
.review-badges { display: flex; align-items: center; gap: 0.4rem; margin-top: 0.35rem; }
.review-count { font-size: 0.75rem; opacity: 0.7; }

</style>
