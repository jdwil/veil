<script lang="ts">
	import { onMount } from 'svelte';
	import PageHeader from './PageHeader.svelte';
	import FormSection from './FormSection.svelte';
	import FormField from './FormField.svelte';

	let projects_dir = $state('');
	let layers_dir = $state('');
	let show_core_layers = $state(false);
	let config_path = $state('');
	let veil_home = $state('');
	let version = $state('');
	let loading = $state(true);
	let saving = $state(false);
	let error = $state('');
	let saved = $state(false);
	let git_connected = $state(false);
	let git_login = $state('');
	let git_owner = $state('');
	let git_default = $state('s3');
	let git_hint = $state('');
	let git_error = $state('');
	let reference_dirs_text = $state('');
	let reference_roots: { id?: string; path?: string; usable?: boolean; skip_reason?: string | null }[] =
		$state([]);

	async function load() {
		loading = true;
		error = '';
		try {
			const r = await fetch('/api/config');
			if (!r.ok) throw new Error((await r.text()) || `HTTP ${r.status}`);
			const data = await r.json();
			projects_dir = String(data.projects_dir ?? '');
			layers_dir = String(data.layers_dir ?? '');
			show_core_layers = Boolean(data.show_core_layers);
			const refs = Array.isArray(data.reference_dirs)
				? data.reference_dirs.map((x: unknown) => String(x)).filter((s: string) => s.trim())
				: [];
			reference_dirs_text = refs.join('\n');
			const rr = data.reference_roots;
			if (rr && typeof rr === 'object' && Array.isArray((rr as { roots?: unknown }).roots)) {
				reference_roots = (rr as { roots: typeof reference_roots }).roots;
			} else {
				reference_roots = [];
			}
			config_path = String(data.config_path ?? '');
			veil_home = String(data.veil_home ?? '');
			version = String(data.version ?? '');
			try {
				const g = await fetch('/api/git/status');
				if (g.ok) {
					const gs = await g.json();
					git_connected = Boolean(gs.connected);
					git_login = String(gs.login ?? '');
					git_owner = String(gs.owner ?? '');
					git_default = String(gs.default_origin ?? 's3');
					git_hint = String(gs.hint ?? '');
					git_error = String(gs.error ?? '');
				}
			} catch {
				/* optional */
			}
		} catch (e: unknown) {
			error = e instanceof Error ? e.message : 'Failed to load config';
		} finally {
			loading = false;
		}
	}

	async function save() {
		saving = true;
		error = '';
		saved = false;
		try {
			const r = await fetch('/api/config', {
				method: 'PATCH',
				headers: { 'Content-Type': 'application/json' },
				body: JSON.stringify({
					projects_dir: projects_dir.trim(),
					layers_dir: layers_dir.trim(),
					show_core_layers,
					reference_dirs: reference_dirs_text
						.split('\n')
						.map((s) => s.trim())
						.filter((s) => s.length > 0 && !s.startsWith('#')),
				}),
			});
			if (!r.ok) throw new Error((await r.text()) || `HTTP ${r.status}`);
			const data = await r.json();
			projects_dir = String(data.projects_dir ?? projects_dir);
			layers_dir = String(data.layers_dir ?? layers_dir);
			show_core_layers = Boolean(data.show_core_layers);
			if (Array.isArray(data.reference_dirs)) {
				reference_dirs_text = data.reference_dirs.map((x: unknown) => String(x)).join('\n');
			}
			const rr = data.reference_roots;
			if (rr && typeof rr === 'object' && Array.isArray((rr as { roots?: unknown }).roots)) {
				reference_roots = (rr as { roots: typeof reference_roots }).roots;
			}
			config_path = String(data.config_path ?? config_path);
			saved = true;
		} catch (e: unknown) {
			error = e instanceof Error ? e.message : 'Failed to save config';
		} finally {
			saving = false;
		}
	}

	onMount(() => {
		void load();
	});
</script>

<div class="config">
	<PageHeader title="Config" description="Local host settings, layer catalog, and GitHub origin." />
	{#if loading}
		<p class="hint">Loading…</p>
	{:else}
		<form
			class="dk-form card"
			onsubmit={(e) => {
				e.preventDefault();
				void save();
			}}
		>
			<FormSection title="Paths" columns={1}>
				<FormField
					id="projects_dir"
					label="Projects directory"
					bind:value={projects_dir}
					required={true}
					placeholder="~/veil-projects"
				/>
				<FormField
					id="layers_dir"
					label="Layers directory"
					bind:value={layers_dir}
					placeholder="optional override"
				/>
			</FormSection>
			<FormSection title="Reference trees" columns={1}>
				<FormField
					id="reference_dirs"
					label="Local code the agent may read"
					input_type="textarea"
					rows={4}
					bind:value={reference_dirs_text}
					placeholder={'~/src/legacy-app\nshop=~/code/shop'}
					hint="Read-only. One path per line (optional name=/path). The inner agent uses reference_read here when converting existing code to VEIL. It cannot write these trees. Env VEIL_REFERENCE_DIRS overlays this list."
				/>
				{#if reference_roots.length}
					<ul class="git-facts">
						{#each reference_roots as r}
							<li>
								<code>{r.id || 'root'}</code>
								{r.path || ''}
								{#if r.usable === false}
									— skipped{r.skip_reason ? `: ${r.skip_reason}` : ''}
								{/if}
							</li>
						{/each}
					</ul>
				{/if}
			</FormSection>
			<label class="check">
				<input type="checkbox" bind:checked={show_core_layers} />
				Show core layers in the IDE
			</label>
			{#if error}
				<p class="dk-error">{error}</p>
			{/if}
			{#if saved}
				<p class="ok">Saved.</p>
			{/if}
			<div class="actions">
				<button type="submit" class="btn-primary" disabled={saving || !projects_dir.trim()}>
					{saving ? 'Saving…' : 'Save'}
				</button>
			</div>
			{#if config_path || veil_home}
				<p class="hint">
					{#if version}version {version} · {/if}
					{#if config_path}{config_path}{/if}
					{#if veil_home}<br />home {veil_home}{/if}
				</p>
			{/if}
		</form>
		<section class="card git-card">
			<h2 class="git-title">GitHub</h2>
			<p class="hint">
				New projects store source on GitHub when a token and
				<code>VEIL_GITHUB_OWNER</code> are set. Restart ProductHost after changing env.
			</p>
			<ul class="git-facts">
				<li>Account: {git_connected ? git_login || 'connected' : 'not connected'}</li>
				<li>Owner for new repos: {git_owner || '—'}</li>
				<li>Default origin: {git_default}</li>
			</ul>
			{#if git_error}
				<p class="dk-error">{git_error}</p>
			{/if}
			{#if git_hint}
				<p class="hint">{git_hint}</p>
			{/if}
		</section>
	{/if}
</div>

<style>
	.config {
		max-width: 42rem;
		animation: dk-fade-in var(--dk-dur-slow, 420ms) var(--dk-ease-out, ease) both;
	}
	.dk-form {
		padding: 1.25rem 1.35rem 1.5rem;
		display: flex;
		flex-direction: column;
		gap: 1.5rem;
	}
	.actions {
		display: flex;
		justify-content: flex-end;
	}
	.check {
		display: flex;
		align-items: center;
		gap: 0.5rem;
		font-size: 0.9rem;
	}
	.hint {
		margin: 0;
		font-size: 0.8rem;
		opacity: 0.7;
	}
	.git-card {
		margin-top: 1.25rem;
		padding: 1.25rem 1.35rem;
	}
	.git-title {
		margin: 0 0 0.5rem;
		font-size: 1rem;
	}
	.git-facts {
		margin: 0.75rem 0;
		padding-left: 1.1rem;
		font-size: 0.9rem;
	}
	.ok {
		margin: 0;
		font-size: 0.85rem;
		color: #86efac;
	}
</style>
