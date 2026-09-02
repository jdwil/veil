<script lang="ts">
	import { fetchBundleDetail, type BundleDetail } from '$lib/history/history';
	import ActionTimeline from './history/ActionTimeline.svelte';
	import ConversationView from './history/ConversationView.svelte';

	let { bundleId }: { bundleId: string } = $props();

	let detail = $state<BundleDetail | null>(null);
	let loading = $state(true);
	let err = $state<string | null>(null);

	async function load() {
		loading = true;
		err = null;
		try {
			detail = await fetchBundleDetail(bundleId);
		} catch (e) {
			err = e instanceof Error ? e.message : String(e);
		} finally {
			loading = false;
		}
	}

	$effect(() => {
		if (bundleId) load();
	});
</script>

<div class="detail">
	<a class="back" href="/history">← History</a>
	<h1>Review bundle</h1>
	<p class="bid">{bundleId}</p>

	{#if loading}
		<p class="status">Loading…</p>
	{:else if err}
		<p class="status err">{err}</p>
	{:else if detail}
		<section>
			<h2>Action timeline</h2>
			<ActionTimeline actions={detail.actions} />
		</section>

		<section>
			<h2>Agent conversations that produced this change</h2>
			{#if detail.sessions.length === 0}
				<p class="status">No linked agent sessions found for this bundle.</p>
			{/if}
			{#each detail.sessions as s (s.session_id)}
				<div class="sess">
					<div class="sess-head">
						<span class="sess-slug">{s.slug}</span>
						<span class="mono">{s.session_id}</span>
						<span class="count">{s.turn_count} turns</span>
					</div>
					<ConversationView
						turns={s.turns}
						title={`Bundle ${bundleId} — ${s.slug}`}
						sessionId={s.session_id}
						slug={s.slug}
					/>
				</div>
			{/each}
		</section>
	{/if}
</div>

<style>
	.detail {
		padding: 24px 28px;
		max-width: 1000px;
		margin: 0 auto;
	}
	.back {
		font-size: 0.85rem;
		color: var(--dk-text-muted, #a3a3a3);
		text-decoration: none;
	}
	.back:hover {
		color: var(--accent, #737373);
	}
	h1 {
		margin: 8px 0 0;
		font-size: 1.5rem;
	}
	.bid {
		font-family: var(--dk-mono, ui-monospace, monospace);
		color: var(--dk-text-muted, #a3a3a3);
		font-size: 0.8rem;
		margin: 2px 0 20px;
	}
	h2 {
		font-size: 1.05rem;
		margin: 24px 0 10px;
	}
	.status {
		color: var(--dk-text-muted, #a3a3a3);
	}
	.status.err {
		color: #f87171;
	}
	.sess {
		margin-bottom: 22px;
	}
	.sess-head {
		display: flex;
		gap: 10px;
		align-items: center;
		margin-bottom: 8px;
	}
	.sess-slug {
		font-weight: 700;
	}
	.mono {
		font-family: var(--dk-mono, ui-monospace, monospace);
		font-size: 0.72rem;
		color: var(--dk-text-muted, #a3a3a3);
	}
	.count {
		font-size: 0.72rem;
		color: var(--dk-text-muted, #a3a3a3);
	}
</style>
