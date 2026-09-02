<script lang="ts">
	import type { ReviewAction } from '$lib/history/history';

	let { actions }: { actions: ReviewAction[] } = $props();

	const ICON: Record<string, string> = {
		approve: '✓',
		reject: '✕',
		request_changes: '↩',
		merge: '⇉',
		deploy: '☁',
		override_two_person: '⚠',
	};

	function actionClass(a: ReviewAction): string {
		if (a.result === 'blocked') return 'act-blocked';
		if (a.result === 'failure') return 'act-fail';
		if (a.action === 'reject' || a.action === 'override_two_person') return 'act-warn';
		return 'act-ok';
	}

	let ordered = $derived([...actions].sort((a, b) => b.at.localeCompare(a.at)));
</script>

<div class="timeline">
	{#if ordered.length === 0}
		<p class="empty">No review actions recorded yet.</p>
	{/if}
	{#each ordered as a (a.id)}
		<div class="row {actionClass(a)}">
			<span class="ic">{ICON[a.action] ?? '•'}</span>
			<div class="body">
				<div class="line1">
					<span class="action">{a.action.replace(/_/g, ' ')}</span>
					<span class="result">{a.result}</span>
					{#if a.environment}<span class="env">{a.environment}</span>{/if}
				</div>
				<div class="line2">
					<span class="actor">{a.actor}</span>
					<span class="kind">({a.actor_kind})</span>
					<span class="at">{a.at}</span>
				</div>
				{#if a.slugs?.length}
					<div class="meta">projects: {a.slugs.join(', ')}</div>
				{/if}
				{#if a.git_shas?.length}
					<div class="meta">sha: {a.git_shas.join(', ')}</div>
				{/if}
				{#if a.pr_ids?.length}
					<div class="meta">pr: {a.pr_ids.join(', ')}</div>
				{/if}
				{#if a.note}
					<div class="note">{a.note}</div>
				{/if}
			</div>
		</div>
	{/each}
</div>

<style>
	.timeline {
		display: flex;
		flex-direction: column;
		gap: 4px;
	}
	.empty {
		color: var(--dk-text-muted, #a3a3a3);
		font-size: 0.9rem;
	}
	.row {
		display: flex;
		gap: 12px;
		padding: 10px 12px;
		border-left: 3px solid var(--dk-border-soft, #555);
		border-radius: 0 8px 8px 0;
		background: var(--dk-surface, rgba(255, 255, 255, 0.015));
	}
	.act-ok {
		border-left-color: #4ade80;
	}
	.act-warn {
		border-left-color: #fbbf24;
	}
	.act-fail {
		border-left-color: #f87171;
	}
	.act-blocked {
		border-left-color: #a3a3a3;
		opacity: 0.85;
	}
	.ic {
		font-size: 1.1rem;
		width: 1.4rem;
		text-align: center;
	}
	.body {
		flex: 1;
		min-width: 0;
	}
	.line1 {
		display: flex;
		align-items: center;
		gap: 8px;
	}
	.action {
		font-weight: 700;
		text-transform: capitalize;
	}
	.result {
		font-size: 0.7rem;
		padding: 1px 8px;
		border-radius: 999px;
		border: 1px solid var(--dk-border-soft, #555);
		text-transform: uppercase;
	}
	.env {
		font-size: 0.7rem;
		padding: 1px 8px;
		border-radius: 999px;
		background: color-mix(in srgb, var(--accent, #737373) 20%, transparent);
	}
	.line2 {
		display: flex;
		gap: 8px;
		font-size: 0.75rem;
		color: var(--dk-text-muted, #a3a3a3);
		margin-top: 2px;
	}
	.meta {
		font-size: 0.72rem;
		color: var(--dk-text-muted, #a3a3a3);
		font-family: var(--dk-mono, ui-monospace, monospace);
		margin-top: 2px;
	}
	.note {
		font-size: 0.8rem;
		margin-top: 4px;
		padding: 5px 8px;
		background: var(--dk-bg, #0f0f0f);
		border-radius: 6px;
		white-space: pre-wrap;
		word-break: break-word;
	}
</style>
