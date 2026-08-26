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
			config_path = String(data.config_path ?? '');
			veil_home = String(data.veil_home ?? '');
			version = String(data.version ?? '');
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
				}),
			});
			if (!r.ok) throw new Error((await r.text()) || `HTTP ${r.status}`);
			const data = await r.json();
			projects_dir = String(data.projects_dir ?? projects_dir);
			layers_dir = String(data.layers_dir ?? layers_dir);
			show_core_layers = Boolean(data.show_core_layers);
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
	<PageHeader title="Config" description="Local host settings (projects tree, layer catalog)." />
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
	.ok {
		margin: 0;
		font-size: 0.85rem;
		color: #86efac;
	}
</style>
