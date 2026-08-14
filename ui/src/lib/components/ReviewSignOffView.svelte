<script lang="ts">
	import { onMount } from 'svelte';
	import PageHeader from './PageHeader.svelte';
	import StatusPill from './StatusPill.svelte';
	import {
		refreshReview,
		reviewItems,
		submitSignOff,
		type OutstandingItem
	} from '$lib/review/store';

	interface Props {
		slug?: string;
	}
	let { slug = '' }: Props = $props();

	let note = $state('');
	let busy = $state(false);
	let error = $state('');
	let selected = $state<Record<string, boolean>>({});
	let message = $state('');

	const items = $derived(
		$reviewItems.filter((i) => {
			if (i.status !== 'outstanding') return false;
			if (!slug) return true;
			return i.slug === slug || i.repo_id === slug;
		})
	);

	const grouped = $derived.by(() => {
		const map = new Map<string, OutstandingItem[]>();
		for (const it of items) {
			const k = it.slug || 'unknown';
			const arr = map.get(k) ?? [];
			arr.push(it);
			map.set(k, arr);
		}
		return [...map.entries()];
	});

	onMount(() => {
		void refreshReview(slug || undefined);
	});

	function selectedIds(): string[] {
		return Object.entries(selected)
			.filter(([, v]) => v)
			.map(([id]) => id);
	}

	async function act(decision: 'approve' | 'reject') {
		busy = true;
		error = '';
		message = '';
		const ids = selectedIds();
		const res = await submitSignOff({
			ids,
			slug: ids.length ? undefined : slug || undefined,
			all: !ids.length && !slug,
			decision,
			note: note.trim() || undefined,
			actor: 'human'
		});
		busy = false;
		if (!res.ok) {
			error = res.error || 'Sign-off failed';
			return;
		}
		selected = {};
		note = '';
		message =
			decision === 'approve'
				? `Signed off ${res.signed ?? 0} change(s).`
				: `Rejected ${res.signed ?? 0} change(s). Remaining items stay outstanding.`;
	}
</script>

<div class="review" data-veil-role="sign-off" data-veil-agent={JSON.stringify({
	intent: 'sign-off',
	entity: 'OutstandingChangeSet',
	notes: [
		'This is review state, not git history.',
		'Approve / reject writes a SOC 2 audit record.',
		'Partial: check items, then Sign off or Reject.',
	],
	actions: [
		{ id: 'approve', label: 'Sign off', method: 'api' },
		{ id: 'reject', label: 'Reject', method: 'api' },
	],
	api: {
		list: 'GET /api/review/outstanding',
		signOff: 'POST /api/review/sign_off',
	},
})}>
	<PageHeader
		title={slug ? `Sign off · ${slug}` : 'Sign off'}
		description="Exactly what the agent did and why. Git remains history — this is the human half of the dual loop."
	>
		{#snippet actions()}
			<a class="btn-outline" href="/projects">Projects</a>
		{/snippet}
	</PageHeader>

	{#if error}
		<p class="dk-error" role="alert">{error}</p>
	{/if}
	{#if message}
		<p class="ok">{message}</p>
	{/if}

	{#if items.length === 0}
		<div class="card empty">
			<p>No outstanding changes{slug ? ` for ${slug}` : ''}.</p>
			<p class="hint">When the agent edits, creates, or commits, items appear here until you sign off.</p>
		</div>
	{:else}
		<p class="lede">
			Here is exactly what the agent did. Sign off the set (or selected items) before proceed / merge / deploy.
		</p>
		{#each grouped as [proj, rows]}
			<section class="card group">
				<header class="group-h">
					<a href={`/projects/${encodeURIComponent(proj)}`}>{proj}</a>
					<StatusPill label={`${rows.length} outstanding`} variant="warning" />
					<a class="btn-ghost" href={`/projects/${encodeURIComponent(proj)}/ide`}>Open IDE</a>
				</header>
				<ul>
					{#each rows as it}
						<li>
							<label>
								<input type="checkbox" bind:checked={selected[it.id]} />
								<span class="kind">{it.kind.replace('_', ' ')}</span>
								<span class="sum">{it.summary}</span>
								{#if it.path}
									<code>{it.path}</code>
								{/if}
							</label>
							{#if it.rationale}
								<p class="why">{it.rationale}</p>
							{/if}
							{#if it.git_sha}
								<p class="sha">git {it.git_sha.slice(0, 8)}</p>
							{/if}
						</li>
					{/each}
				</ul>
			</section>
		{/each}

		<div class="card actions" data-veil-role="create-form">
			<label class="note">
				<span>Note (optional)</span>
				<textarea class="input" bind:value={note} rows="2" placeholder="Why this set is acceptable (or not)"></textarea>
			</label>
			<div class="btns">
				<button
					type="button"
					class="btn-primary"
					data-veil-action="sign-off"
					disabled={busy}
					onclick={() => act('approve')}
				>
					{busy ? 'Recording…' : 'Sign off'}
				</button>
				<button
					type="button"
					class="btn-outline"
					data-veil-action="reject-sign-off"
					disabled={busy}
					onclick={() => act('reject')}
				>
					Reject
				</button>
			</div>
		</div>
	{/if}
</div>

<style>
	.review { max-width: 800px; }
	.lede { opacity: 0.85; margin: 0 0 1rem; }
	.ok { color: var(--dk-ok, #34d399); }
	.empty { padding: 1.25rem; }
	.hint { opacity: 0.7; font-size: 0.9rem; }
	.group { padding: 0.9rem 1rem 0.4rem; margin-bottom: 0.85rem; }
	.group-h {
		display: flex; align-items: center; gap: 0.6rem; margin-bottom: 0.5rem;
	}
	.group-h a { font-weight: 600; }
	ul { list-style: none; padding: 0; margin: 0; }
	li { padding: 0.55rem 0; border-top: 1px solid var(--dk-border-soft, #27272a); }
	label { display: flex; flex-wrap: wrap; align-items: baseline; gap: 0.45rem; cursor: pointer; }
	.kind { font-size: 0.72rem; text-transform: uppercase; letter-spacing: 0.04em; opacity: 0.65; }
	.sum { font-weight: 500; }
	code { font-size: 0.8rem; opacity: 0.75; }
	.why { margin: 0.25rem 0 0 1.4rem; font-size: 0.88rem; opacity: 0.8; }
	.sha { margin: 0.15rem 0 0 1.4rem; font-size: 0.75rem; font-family: ui-monospace, monospace; opacity: 0.55; }
	.actions { padding: 1rem; display: flex; flex-direction: column; gap: 0.75rem; }
	.note { display: flex; flex-direction: column; gap: 0.3rem; font-size: 0.85rem; }
	.btns { display: flex; gap: 0.5rem; }
	.btn-ghost { font-size: 0.8rem; opacity: 0.8; }
</style>
