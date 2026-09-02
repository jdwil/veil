<script lang="ts">
	import type { Turn } from '$lib/history/history';
	import {
		conversationToMarkdown,
		turnToMarkdown,
		copyToClipboard,
	} from '$lib/history/history';
	import ToolCallBlock from './ToolCallBlock.svelte';

	let {
		turns,
		title,
		sessionId,
		slug,
	}: { turns: Turn[]; title?: string; sessionId?: string; slug?: string } = $props();

	let copied = $state<string | null>(null);

	let ordered = $derived([...turns].sort((a, b) => a.turn_id.localeCompare(b.turn_id)));

	async function copyConversation() {
		const md = conversationToMarkdown(turns, { title, sessionId, slug });
		const ok = await copyToClipboard(md);
		flash(ok ? 'Conversation copied' : 'Copy failed');
	}

	async function copyTurn(t: Turn) {
		const ok = await copyToClipboard(turnToMarkdown(t));
		flash(ok ? 'Turn copied' : 'Copy failed');
	}

	function flash(msg: string) {
		copied = msg;
		setTimeout(() => (copied = null), 1800);
	}

	function sortedTools(t: Turn) {
		return [...(t.tool_calls ?? [])].sort((a, b) => (a.order ?? 0) - (b.order ?? 0));
	}
</script>

<div class="conv">
	<div class="conv-toolbar">
		<button class="copy-btn" onclick={copyConversation} title="Copy the full conversation as paste-ready markdown">
			⧉ Copy conversation
		</button>
		{#if copied}<span class="copied">{copied}</span>{/if}
	</div>

	{#if ordered.length === 0}
		<p class="empty">No turns captured for this conversation.</p>
	{/if}

	{#each ordered as turn (turn.turn_id)}
		<div class="turn turn--{turn.role}">
			<div class="turn-head">
				<span class="turn-role">{turn.role}</span>
				<span class="turn-ts">{turn.ts}</span>
				{#if turn.backend}<span class="turn-backend">{turn.backend}</span>{/if}
				<button class="copy-turn" onclick={() => copyTurn(turn)} title="Copy this turn">⧉ Copy turn</button>
			</div>
			{#if turn.content?.trim()}
				<div class="turn-text">{turn.content}</div>
			{/if}
			{#if sortedTools(turn).length}
				<div class="turn-tools">
					{#each sortedTools(turn) as tc}
						<ToolCallBlock {tc} />
					{/each}
				</div>
			{/if}
		</div>
	{/each}
</div>

<style>
	.conv {
		display: flex;
		flex-direction: column;
		gap: 10px;
	}
	.conv-toolbar {
		display: flex;
		align-items: center;
		gap: 10px;
		position: sticky;
		top: 0;
		z-index: 2;
		padding: 4px 0;
		background: var(--dk-bg, #0f0f0f);
	}
	.copy-btn,
	.copy-turn {
		font-size: 0.78rem;
		padding: 5px 12px;
		border: 1px solid var(--dk-border-soft, #555);
		border-radius: 6px;
		background: transparent;
		color: var(--dk-text, #e5e5e5);
		cursor: pointer;
	}
	.copy-btn:hover,
	.copy-turn:hover {
		background: color-mix(in srgb, var(--accent, #737373) 14%, transparent);
		border-color: var(--accent, #737373);
	}
	.copied {
		font-size: 0.75rem;
		color: #4ade80;
	}
	.empty {
		color: var(--dk-text-muted, #a3a3a3);
		font-size: 0.9rem;
	}
	.turn {
		border: 1px solid var(--dk-border-soft, rgba(120, 120, 120, 0.25));
		border-radius: 10px;
		padding: 10px 14px;
		background: var(--dk-surface, rgba(255, 255, 255, 0.015));
	}
	.turn--user {
		border-left: 3px solid var(--accent, #737373);
	}
	.turn--assistant {
		border-left: 3px solid #60a5fa;
	}
	.turn-head {
		display: flex;
		align-items: center;
		gap: 10px;
		margin-bottom: 6px;
	}
	.turn-role {
		font-weight: 700;
		text-transform: capitalize;
		font-size: 0.85rem;
	}
	.turn-ts,
	.turn-backend {
		font-size: 0.7rem;
		color: var(--dk-text-muted, #a3a3a3);
	}
	.turn-backend {
		border: 1px solid var(--dk-border-soft, #555);
		border-radius: 4px;
		padding: 0 6px;
	}
	.copy-turn {
		margin-left: auto;
		padding: 2px 8px;
		font-size: 0.7rem;
	}
	.turn-text {
		white-space: pre-wrap;
		word-break: break-word;
		font-size: 0.9rem;
		line-height: 1.5;
		color: var(--dk-text, #e5e5e5);
	}
	.turn-tools {
		margin-top: 8px;
	}
</style>
