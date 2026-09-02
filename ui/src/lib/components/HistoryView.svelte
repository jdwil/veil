<script lang="ts">
	import { fetchRecent, type RecentFeed } from '$lib/history/history';
	import ActionTimeline from './history/ActionTimeline.svelte';

	let feed = $state<RecentFeed | null>(null);
	let loading = $state(true);
	let err = $state<string | null>(null);

	// Filters
	let slug = $state('');
	let actor = $state('');
	let action = $state('');
	let tab = $state<'activity' | 'conversations' | 'reviews'>('activity');

	async function load() {
		loading = true;
		err = null;
		try {
			feed = await fetchRecent({
				slug: slug || undefined,
				actor: actor || undefined,
				action: action || undefined,
				limit: 150,
			});
		} catch (e) {
			err = e instanceof Error ? e.message : String(e);
		} finally {
			loading = false;
		}
	}

	$effect(() => {
		load();
	});

	const ACTION_TYPES = [
		'',
		'approve',
		'reject',
		'request_changes',
		'merge',
		'deploy',
		'override_two_person',
	];
</script>

<div class="history">
	<header class="hist-head">
		<div>
			<h1>History</h1>
			<p class="sub">Agent conversations & review-action audit — browse, trace, and copy.</p>
		</div>
	</header>

	<div class="filters">
		<input
			placeholder="Filter by project slug…"
			bind:value={slug}
			onkeydown={(e) => e.key === 'Enter' && load()}
		/>
		<input
			placeholder="Actor…"
			bind:value={actor}
			onkeydown={(e) => e.key === 'Enter' && load()}
		/>
		<select bind:value={action} onchange={() => load()}>
			{#each ACTION_TYPES as at}
				<option value={at}>{at === '' ? 'Any action' : at.replace(/_/g, ' ')}</option>
			{/each}
		</select>
		<button class="apply" onclick={() => load()}>Apply</button>
	</div>

	<div class="tabs">
		<button class:active={tab === 'activity'} onclick={() => (tab = 'activity')}>Activity</button>
		<button class:active={tab === 'conversations'} onclick={() => (tab = 'conversations')}>
			Conversations
		</button>
		<button class:active={tab === 'reviews'} onclick={() => (tab = 'reviews')}>Reviews</button>
	</div>

	{#if loading}
		<p class="status">Loading…</p>
	{:else if err}
		<p class="status err">{err}</p>
	{:else if feed}
		{#if tab === 'activity'}
			<section>
				<h2>Recent review actions</h2>
				<ActionTimeline actions={feed.actions} />
			</section>
		{:else if tab === 'conversations'}
			<section>
				<h2>Coding sessions</h2>
				{#if feed.sessions.length === 0}
					<p class="status">No sessions found.</p>
				{/if}
				<div class="cards">
					{#each feed.sessions as s (s.session_id)}
						<a class="card" href={`/history/session/${encodeURIComponent(s.session_id)}`}>
							<div class="card-title">{s.slug || '(unknown project)'}</div>
							<div class="card-meta">
								{#if s.branch_name}<span class="chip">{s.branch_name}</span>{/if}
								<span class="mono">{s.session_id}</span>
							</div>
							<div class="card-ts">{s.at}</div>
						</a>
					{/each}
				</div>
			</section>
		{:else if tab === 'reviews'}
			<section>
				<h2>Review bundles</h2>
				{#if feed.bundles.length === 0}
					<p class="status">No review bundles found.</p>
				{/if}
				<div class="cards">
					{#each feed.bundles as b (b.id)}
						<a class="card" href={`/history/bundle/${encodeURIComponent(b.id)}`}>
							<div class="card-title">{b.title || b.id}</div>
							<div class="card-summary">{b.summary}</div>
							<div class="card-meta">
								{#each b.project_slugs as ps}<span class="chip">{ps}</span>{/each}
								{#if b.outstanding > 0}
									<span class="chip chip--warn">{b.outstanding} outstanding</span>
								{/if}
							</div>
							<div class="card-ts">{b.created_at}</div>
						</a>
					{/each}
				</div>
			</section>
		{/if}
	{/if}
</div>

<style>
	.history {
		padding: 24px 28px;
		max-width: 1100px;
		margin: 0 auto;
	}
	.hist-head h1 {
		margin: 0;
		font-size: 1.6rem;
	}
	.sub {
		color: var(--dk-text-muted, #a3a3a3);
		margin: 4px 0 16px;
		font-size: 0.9rem;
	}
	.filters {
		display: flex;
		gap: 8px;
		flex-wrap: wrap;
		margin-bottom: 14px;
	}
	.filters input,
	.filters select {
		padding: 7px 10px;
		border: 1px solid var(--dk-border-soft, #555);
		border-radius: 7px;
		background: var(--dk-surface, rgba(255, 255, 255, 0.02));
		color: var(--dk-text, #e5e5e5);
		font-size: 0.85rem;
	}
	.filters input {
		min-width: 200px;
	}
	.apply {
		padding: 7px 16px;
		border: 1px solid var(--accent, #737373);
		border-radius: 7px;
		background: color-mix(in srgb, var(--accent, #737373) 16%, transparent);
		color: var(--dk-text, #e5e5e5);
		cursor: pointer;
	}
	.tabs {
		display: flex;
		gap: 4px;
		border-bottom: 1px solid var(--dk-border-soft, #555);
		margin-bottom: 16px;
	}
	.tabs button {
		padding: 8px 16px;
		background: transparent;
		border: none;
		border-bottom: 2px solid transparent;
		color: var(--dk-text-muted, #a3a3a3);
		cursor: pointer;
		font-size: 0.9rem;
	}
	.tabs button.active {
		color: var(--dk-text, #e5e5e5);
		border-bottom-color: var(--accent, #737373);
		font-weight: 600;
	}
	h2 {
		font-size: 1rem;
		margin: 0 0 10px;
	}
	.status {
		color: var(--dk-text-muted, #a3a3a3);
	}
	.status.err {
		color: #f87171;
	}
	.cards {
		display: grid;
		grid-template-columns: repeat(auto-fill, minmax(300px, 1fr));
		gap: 10px;
	}
	.card {
		display: block;
		text-decoration: none;
		color: inherit;
		border: 1px solid var(--dk-border-soft, rgba(120, 120, 120, 0.3));
		border-radius: 10px;
		padding: 12px 14px;
		background: var(--dk-surface, rgba(255, 255, 255, 0.015));
		transition: border-color 120ms ease, background 120ms ease;
	}
	.card:hover {
		border-color: var(--accent, #737373);
		background: color-mix(in srgb, var(--accent, #737373) 7%, transparent);
	}
	.card-title {
		font-weight: 700;
		margin-bottom: 4px;
	}
	.card-summary {
		font-size: 0.82rem;
		color: var(--dk-text-muted, #cfcfcf);
		margin-bottom: 6px;
	}
	.card-meta {
		display: flex;
		flex-wrap: wrap;
		gap: 6px;
		align-items: center;
	}
	.chip {
		font-size: 0.68rem;
		padding: 1px 8px;
		border-radius: 999px;
		border: 1px solid var(--dk-border-soft, #555);
	}
	.chip--warn {
		color: #fbbf24;
		border-color: rgba(251, 191, 36, 0.5);
	}
	.mono {
		font-family: var(--dk-mono, ui-monospace, monospace);
		font-size: 0.7rem;
		color: var(--dk-text-muted, #a3a3a3);
	}
	.card-ts {
		font-size: 0.7rem;
		color: var(--dk-text-muted, #a3a3a3);
		margin-top: 6px;
	}
</style>
