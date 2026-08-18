<script lang="ts">
	import { onMount } from 'svelte';
	import PageHeader from './PageHeader.svelte';
	import FormSection from './FormSection.svelte';
	import FormField from './FormField.svelte';
	import StatusPill from './StatusPill.svelte';
	import {
		refreshReview,
		reviewChangeSets,
		reviewAudits,
		reviewReady,
		reviewLoadError,
		type ChangeSet,
		type SignOffAudit
	} from '$lib/review/store';

	let artifactId: string = $state('');
	let targetType: string = $state('lambda');
	let deploying: boolean = $state(false);
	let error: string = $state('');
	let message: string = $state('');
	let advanced = $state(false);
	let repoIds = $state<Record<string, string>>({});

	const ready: SignOffAudit[] = $derived(
		$reviewAudits.filter((a) => {
			if (a.decision !== 'approve' || !a.slug) return false;
			if (!Object.keys(repoIds).length) return true;
			return Boolean(repoIds[a.slug]);
		})
	);

	const blocked: ChangeSet[] = $derived(
		$reviewChangeSets.filter((c) => c.outstanding > 0 && (!Object.keys(repoIds).length || repoIds[c.slug]))
	);

	onMount(() => {
		void refreshReview();
		void fetch('/api/repos')
			.then((r) => (r.ok ? r.json() : []))
			.then((list) => {
				const map: Record<string, string> = {};
				for (const row of Array.isArray(list) ? list : []) {
					const rec = row as Record<string, unknown>;
					const name = String(rec.name || rec.slug || '');
					const id =
						rec.id != null && typeof rec.id === 'object' && rec.id && 'value' in (rec.id as object)
							? String((rec.id as { value?: string }).value || '')
							: String(rec.id || rec.slug || '');
					if (name && id) map[name] = id;
					if (rec.slug && id) map[String(rec.slug)] = id;
				}
				repoIds = map;
			})
			.catch(() => {});
	});

	async function ship(slug: string, sha?: string | null) {
		deploying = true;
		error = '';
		message = '';
		try {
			const r = await fetch('/api/provision-project', {
				method: 'POST',
				headers: { 'Content-Type': 'application/json' },
				body: JSON.stringify({
					project_slug: slug,
					environment: 'dev',
					repo_id: repoIds[slug] || slug,
					branch: 'main'
				})
			});
			const data = await r.json().catch(() => ({}));
			if (!r.ok || data.ok === false) {
				throw new Error(String(data.message || data.error || `HTTP ${r.status}`));
			}
			message = `Shipping ${slug}${sha ? ` @ ${sha.slice(0, 8)}` : ''}.`;
		} catch (e) {
			error = e instanceof Error ? e.message : String(e);
		} finally {
			deploying = false;
		}
	}

	async function triggerDeploy() {
		deploying = true;
		error = '';
		try {
			const __r = await fetch('/api/deploy', {
				method: 'POST',
				headers: { 'Content-Type': 'application/json' },
				body: JSON.stringify({ artifact_id: artifactId, target: targetType })
			});
			if (!__r.ok) throw new Error(await __r.text());
		} catch (e) {
			error = e instanceof Error ? e.message : String(e);
		} finally {
			deploying = false;
		}
	}
</script>

<div class="deploy">
	<PageHeader
		title="Deploy"
		description="Deploy a version that has been approved."
	>
		{#snippet actions()}
			<a class="btn-outline" href="/review">Review</a>
		{/snippet}
	</PageHeader>

	{#if error || $reviewLoadError}
		<p class="dk-error" role="alert">{error || $reviewLoadError}</p>
	{/if}
	{#if message}
		<p class="ok">{message}</p>
	{/if}

	{#if !$reviewReady && blocked.length === 0 && ready.length === 0}
		<div class="card dk-loading" role="status" aria-live="polite" aria-busy="true">
			<div class="dk-spinner" aria-hidden="true"></div>
			<span>Loading ship status…</span>
		</div>
	{:else}
	{#if blocked.length}
		<section class="card">
			<h2>Needs review</h2>
			<ul>
				{#each blocked as cs}
					<li>
						<a href={`/review/${encodeURIComponent(cs.slug)}`}>{cs.slug}</a>
						<StatusPill label={`${cs.outstanding} outstanding`} variant="warning" />
					</li>
				{/each}
			</ul>
		</section>
	{/if}

	<section class="card">
		<h2>Ready to deploy</h2>
		{#if ready.length === 0}
			<p class="hint">Nothing approved yet.</p>
		{:else}
			<ul>
				{#each ready as a}
					<li>
						<strong>{a.slug}</strong>
						{#if a.git_sha}
							<code>{a.git_sha.slice(0, 8)}</code>
						{/if}
						<span class="hint">{a.at} · {a.actor}</span>
						<button
							type="button"
							class="btn-primary"
							disabled={deploying}
							onclick={() => ship(a.slug || '', a.git_sha)}
						>
							{deploying ? 'Deploying…' : 'Deploy'}
						</button>
					</li>
				{/each}
			</ul>
		{/if}
	</section>
	{/if}

	<p class="advanced-toggle">
		<button type="button" class="btn-ghost" onclick={() => (advanced = !advanced)}>
			{advanced ? 'Hide' : 'Advanced'}: artifact id
		</button>
	</p>
	{#if advanced}
		<form
			class="dk-form card"
			onsubmit={(e) => {
				e.preventDefault();
				triggerDeploy();
			}}
		>
			<FormSection title="Artifact (legacy)" columns={1}>
				<FormField label="Artifact ID" bind:value={artifactId} required={true} placeholder="art_…" />
				<div class="dk-field">
					<label class="dk-field__label" for="deploy-target">Target</label>
					<div class="dk-field__control">
						<select id="deploy-target" bind:value={targetType}>
							<option value="lambda">Lambda</option>
							<option value="container">Container</option>
						</select>
					</div>
				</div>
			</FormSection>
			<div class="actions">
				<button type="submit" class="btn-outline" disabled={deploying || !artifactId}>
					{deploying ? 'Deploying…' : 'Deploy artifact'}
				</button>
			</div>
		</form>
	{/if}
</div>

<style>
	.deploy { max-width: 42rem; animation: dk-fade-in var(--dk-dur-slow, 420ms) var(--dk-ease-out, ease) both; }
	.dk-form { padding: 1.25rem 1.35rem 1.5rem; display: flex; flex-direction: column; gap: 1.5rem; }
	.actions { display: flex; justify-content: flex-end; }
	.card { padding: 1rem 1.15rem; margin-bottom: 1rem; }
	h2 { font-size: 0.95rem; margin: 0 0 0.6rem; }
	ul { list-style: none; padding: 0; margin: 0; }
	li { display: flex; flex-wrap: wrap; align-items: center; gap: 0.5rem; padding: 0.45rem 0; border-top: 1px solid var(--dk-border-soft, #27272a); }
	.hint { opacity: 0.7; font-size: 0.88rem; }
	.ok { color: var(--dk-ok, #34d399); }
	.env-banner {
		padding: 0.65rem 0.85rem;
		border-radius: 8px;
		background: color-mix(in oklab, #f59e0b 16%, transparent);
		margin: 0 0 1rem;
		font-size: 0.88rem;
	}
	.advanced-toggle { margin: 0.5rem 0; }
	.btn-ghost { font-size: 0.82rem; opacity: 0.75; }
</style>
