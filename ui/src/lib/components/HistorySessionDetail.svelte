<script lang="ts">
	import { fetchSessionTurns, type Turn } from '$lib/history/history';
	import ConversationView from './history/ConversationView.svelte';

	let { sessionId }: { sessionId: string } = $props();

	let turns = $state<Turn[]>([]);
	let slug = $state<string | undefined>(undefined);
	let loading = $state(true);
	let err = $state<string | null>(null);

	async function load() {
		loading = true;
		err = null;
		try {
			const r = await fetchSessionTurns(sessionId);
			turns = r.turns ?? [];
			slug = turns.find((t) => t.project)?.project ?? undefined;
		} catch (e) {
			err = e instanceof Error ? e.message : String(e);
		} finally {
			loading = false;
		}
	}

	$effect(() => {
		if (sessionId) load();
	});
</script>

<div class="detail">
	<a class="back" href="/history">← History</a>
	<h1>Conversation</h1>
	<p class="sid">{sessionId}{#if slug} · {slug}{/if}</p>

	{#if loading}
		<p class="status">Loading…</p>
	{:else if err}
		<p class="status err">{err}</p>
	{:else}
		<ConversationView {turns} title={`Session ${sessionId}`} {sessionId} {slug} />
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
	.sid {
		font-family: var(--dk-mono, ui-monospace, monospace);
		color: var(--dk-text-muted, #a3a3a3);
		font-size: 0.8rem;
		margin: 2px 0 20px;
	}
	.status {
		color: var(--dk-text-muted, #a3a3a3);
	}
	.status.err {
		color: #f87171;
	}
</style>
