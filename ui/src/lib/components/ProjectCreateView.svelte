<script lang="ts">

  import { goto } from '$app/navigation';
  import CreateFormShell from './CreateFormShell.svelte';
  import FormSection from './FormSection.svelte';
  import FormField from './FormField.svelte';

  import { onMount } from 'svelte';

  let name: string = $state('');
  let description: string = $state('');
  let submitting: boolean = $state(false);
  let error: string = $state('');
  let git_provider: string = $state('github');
  let git_owner: string = $state('');
  let git_repo: string = $state('');
  let git_mode: string = $state('create');
  let git_private: string = $state('private');
  let git_repo_touched: boolean = $state(false);
  let git_owners: string[] = $state([]);
  let git_label: string = $state('Git origin per project');
  let bb_ready: boolean = $state(false);
  // Origin type: s3 bundle (default, own history) | repo root (own repo) |
  // repo subpath (a subdir of a shared repo — many projects per repo).
  let origin_type: string = $state('repo-root');
  let git_subpath: string = $state('');
  let git_subpath_touched: boolean = $state(false);

  function slugify(s: string): string {
    return s
      .trim()
      .toLowerCase()
      .replace(/[^a-z0-9]+/g, '-')
      .replace(/^-|-$/g, '');
  }

  $effect(() => {
    if (!git_repo_touched) {
      git_repo = slugify(name);
    }
  });

  $effect(() => {
    // Default the subpath to the project slug for repo-subpath origins.
    if (origin_type === 'repo-subpath' && !git_subpath_touched) {
      git_subpath = slugify(name);
    }
  });

  onMount(() => {
    void (async () => {
      try {
        const r = await fetch('/api/git/status');
        if (!r.ok) return;
        const g = await r.json();
        const login = String(g.login ?? '');
        const owner = String(g.owner ?? '');
        const orgs: string[] = Array.isArray(g.orgs) ? g.orgs.map(String) : [];
        const owners = [owner, login, ...orgs].filter((x, i, a) => x && a.indexOf(x) === i);
        git_owners = owners;
        if (!git_owner && owners[0]) git_owner = owners[0];
        bb_ready = Boolean(g.bitbucket?.token_present);
        if (g.bitbucket?.owner && git_provider === 'bitbucket' && !git_owner) {
          git_owner = String(g.bitbucket.owner);
        }
        git_label = owners.length
          ? `Each project picks a host (GitHub/Bitbucket) and owner (${owners.slice(0, 4).join(', ')})`
          : 'Each project picks its git host and owner/name';
      } catch {
        /* keep default */
      }
    })();
  });

  async function submit() {
    if (submitting) return;
    submitting = true;
    error = '';
    try {
      const slug = slugify(git_repo || name);
      // Build the origin by chosen type:
      //  - s3: no git binding (own S3 bundle history — the default legacy path)
      //  - repo-root: a git repo the project owns (subpath omitted)
      //  - repo-subpath: a subdir of a (possibly shared) repo
      let origin: Record<string, unknown> | undefined;
      if (origin_type === 's3') {
        origin = { kind: 's3' };
      } else {
        origin = {
          kind: 'git',
          provider: git_provider,
          owner: git_owner.trim(),
          name: slug,
          create: git_mode !== 'bind',
          private: git_private !== 'public',
        };
        if (origin_type === 'repo-subpath') {
          const sub = slugify(git_subpath || name);
          if (sub) origin.subpath = sub;
        }
      }
      const r = await fetch('/api/ux/create_project', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({
          name: name.trim(),
          description: description.trim() || undefined,
          origin,
          git_provider,
          git_owner: git_owner.trim(),
          git_repo: slug,
          git_mode,
          open: true,
          open_ide: true,
        }),
      });
      const text = await r.text();
      const data = text ? JSON.parse(text) : {};
      if (!r.ok) throw new Error(data.error || data.summary || text || `HTTP ${r.status}`);
      const out_slug = String(data.slug || data.id || slug);
      const path = String(data.path || `/projects/${encodeURIComponent(out_slug)}/ide`);
      await goto(path);
    } catch (e: unknown) {
      error = e instanceof Error ? e.message : String(e);
    } finally {
      submitting = false;
    }
  }
</script>

<CreateFormShell
  title="Create project"
  subtitle={git_label}
  back_href="/projects"
  mode="create"
  form_id="create-project"
  submit_label="Create"
  saving_label="Creating…"
  saving={submitting}
  required_values={origin_type === 's3' ? [name] : (origin_type === 'repo-subpath' ? [name, git_owner, git_subpath] : [name, git_owner])}
  on_submit={() => { void submit(); }}
  error={error}
  agent={{
    intent: 'create-project',
    entity: 'Repo',
    entityLabel: 'Project',
    notes: [
      'Required: name and git owner. Git host + owner/name are per project.',
      'On submit: POST /api/ux/create_project then open the IDE.',
      'Agent Present fills these fields and clicks Create.',
    ],
    api: { create: 'POST /api/ux/create_project' },
  }}
>
  <FormSection title="Basics" columns={1}>
    <FormField id="name" label="Name" bind:value={name} required={true} placeholder="My project" />
    <FormField id="description" label="Description" bind:value={description} input_type="textarea" placeholder="Optional" />
  </FormSection>
  <FormSection title="Origin" columns={1}>
    <FormField
      id="origin_type"
      label="Where history lives"
      input_type="select"
      bind:value={origin_type}
      options={[
        { value: 's3', label: 'S3 bundle (own history, no remote)' },
        { value: 'repo-root', label: 'Git repository (own repo)' },
        { value: 'repo-subpath', label: 'Subpath of a shared repo (many projects per repo)' },
      ]}
      hint={origin_type === 'repo-subpath'
        ? 'This project is a subdirectory of a shared repo. The IDE + agent are jailed to the subpath; review is scoped to it.'
        : ''}
    />
  </FormSection>
  {#if origin_type !== 's3'}
  <FormSection title="Git origin" columns={2}>
    <FormField
      id="git_provider"
      label="Host"
      input_type="select"
      bind:value={git_provider}
      options={[
        { value: 'github', label: 'GitHub' },
        { value: 'bitbucket', label: bb_ready ? 'Bitbucket' : 'Bitbucket (set VEIL_BITBUCKET_TOKEN)' },
      ]}
    />
    <FormField
      id="git_mode"
      label="Remote"
      input_type="select"
      bind:value={git_mode}
      options={origin_type === 'repo-subpath'
        ? [
            { value: 'bind', label: 'Use existing shared repository' },
            { value: 'create', label: 'Create shared repository' },
          ]
        : [
            { value: 'create', label: 'Create new repository' },
            { value: 'bind', label: 'Bind existing repository' },
          ]}
    />
    <FormField
      id="git_owner"
      label={git_provider === 'bitbucket' ? 'Workspace' : 'Owner / org'}
      bind:value={git_owner}
      required={true}
      placeholder={git_owners[0] || 'jdwil'}
      hint={git_owners.length ? `Suggestions: ${git_owners.join(', ')}` : ''}
    />
    <FormField
      id="git_repo"
      label={origin_type === 'repo-subpath' ? 'Shared repository name' : 'Repository name'}
      bind:value={git_repo}
      placeholder={origin_type === 'repo-subpath' ? 'dlx-shared-libs' : 'my-project'}
      oninput={() => { git_repo_touched = true; }}
    />
    {#if origin_type === 'repo-subpath'}
    <FormField
      id="git_subpath"
      label="Subpath"
      bind:value={git_subpath}
      required={true}
      placeholder="dlx-auth"
      hint="The subdirectory in the shared repo this project owns."
      oninput={() => { git_subpath_touched = true; }}
    />
    {/if}
    <FormField
      id="git_private"
      label="Visibility"
      input_type="select"
      bind:value={git_private}
      options={[
        { value: 'private', label: 'Private' },
        { value: 'public', label: 'Public' },
      ]}
    />
  </FormSection>
  {/if}
</CreateFormShell>


<style>
  /* TODO: Add component styles */
</style>
