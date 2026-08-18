<script lang="ts">
	import { hunkLineClass } from '$lib/review/diff';
	import type { FileDiff } from '$lib/ide/prWizard';

	interface Props {
		diff: FileDiff;
		ideHref?: string;
	}
	let { diff, ideHref = '' }: Props = $props();

	const lineCount = $derived(
		(diff.hunks || []).reduce((n, h) => n + (h.lines?.length || 0), 0)
	);
</script>

<details class="fd" open>
	<summary>
		<code>{diff.path}</code>
		<span class="st">{diff.status}</span>
		<span class="n">{lineCount} lines</span>
		{#if ideHref}
			<a class="ide" href={ideHref} onclick={(e) => e.stopPropagation()}>Open in IDE</a>
		{/if}
	</summary>
	{#if (diff.hunks || []).length === 0}
		<p class="empty">No hunks in this file.</p>
	{:else}
		{#each diff.hunks || [] as hunk}
			<pre class="hunk"><span class="meta">{hunk.header || ''}</span>
{#each hunk.lines || [] as line}<span class={hunkLineClass(line)}>{line}
</span>{/each}</pre>
		{/each}
	{/if}
</details>

<style>
	.fd {
		border: 1px solid var(--dk-border-soft, #27272a);
		border-radius: 8px;
		overflow: hidden;
		margin: 0.4rem 0;
		background: color-mix(in oklab, #000 35%, transparent);
	}
	summary {
		display: flex;
		align-items: center;
		gap: 0.5rem;
		padding: 0.45rem 0.65rem;
		cursor: pointer;
		list-style: none;
		font-size: 0.8rem;
	}
	summary::-webkit-details-marker { display: none; }
	summary code { font-size: 0.78rem; }
	.st {
		font-size: 0.68rem;
		text-transform: uppercase;
		letter-spacing: 0.04em;
		opacity: 0.6;
	}
	.n { margin-left: auto; opacity: 0.5; font-size: 0.72rem; }
	.ide {
		font-size: 0.72rem;
		opacity: 0.8;
		text-decoration: none;
		color: inherit;
	}
	.ide:hover { opacity: 1; text-decoration: underline; }
	.empty { margin: 0; padding: 0.5rem 0.65rem; font-size: 0.8rem; opacity: 0.65; }
	.hunk {
		margin: 0;
		padding: 0.45rem 0.65rem 0.65rem;
		font-size: 0.72rem;
		line-height: 1.4;
		overflow-x: auto;
		max-height: 28rem;
		background: #0a0a0a;
		color: #d4d4d4;
		white-space: pre;
	}
	.meta { color: #818cf8; }
	.add { color: #6ee7b7; display: block; }
	.del { color: #fca5a5; display: block; }
	.ctx { color: #a1a1aa; display: block; }
</style>
