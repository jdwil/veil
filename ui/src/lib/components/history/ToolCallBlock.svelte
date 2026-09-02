<script lang="ts">
	import type { ToolCall } from '$lib/history/history';
	import { fetchBlob } from '$lib/history/history';

	let { tc }: { tc: ToolCall } = $props();

	let open = $state(false);
	let expandedBlob = $state<string | null>(null);
	let loadingBlob = $state(false);
	let blobErr = $state<string | null>(null);

	function fmt(v: unknown): string {
		if (v == null) return '';
		if (typeof v === 'string') return v;
		try {
			return JSON.stringify(v, null, 2);
		} catch {
			return String(v);
		}
	}

	let argsText = $derived(fmt(tc.input));
	let resultText = $derived(tc.content ?? fmt(tc.output));

	async function loadBlob() {
		if (!tc.content_ref) return;
		loadingBlob = true;
		blobErr = null;
		try {
			const r = await fetchBlob(tc.content_ref);
			expandedBlob = r.content;
		} catch (e) {
			blobErr = e instanceof Error ? e.message : String(e);
		} finally {
			loadingBlob = false;
		}
	}

	function statusClass(s?: string | null): string {
		if (!s) return '';
		if (s === 'failed') return 'st-fail';
		if (s === 'completed') return 'st-ok';
		return 'st-run';
	}
</script>

<div class="tool-call" class:open>
	<button class="tc-head" onclick={() => (open = !open)} aria-expanded={open}>
		<span class="tc-caret">{open ? '▾' : '▸'}</span>
		<span class="tc-icon">🔧</span>
		<span class="tc-name">{tc.name}</span>
		{#if tc.status}<span class="tc-status {statusClass(tc.status)}">{tc.status}</span>{/if}
		{#if tc.fidelity === 'name_only'}<span class="tc-badge">name only</span>{/if}
	</button>
	{#if open}
		<div class="tc-body">
			{#if argsText}
				<div class="tc-section">
					<div class="tc-label">Arguments</div>
					<pre class="tc-pre">{argsText}</pre>
				</div>
			{/if}
			{#if resultText}
				<div class="tc-section">
					<div class="tc-label">Result</div>
					<pre class="tc-pre">{resultText}</pre>
				</div>
			{/if}
			{#if tc.content_ref}
				<div class="tc-section">
					<div class="tc-label">
						Large result — offloaded ({tc.content_bytes ?? '?'} bytes)
					</div>
					{#if tc.content_preview}
						<pre class="tc-pre tc-preview">{tc.content_preview}…</pre>
					{/if}
					{#if expandedBlob !== null}
						<pre class="tc-pre">{expandedBlob}</pre>
					{:else}
						<button class="tc-expand" onclick={loadBlob} disabled={loadingBlob}>
							{loadingBlob ? 'Loading…' : 'Load full result'}
						</button>
						{#if blobErr}<span class="tc-err">{blobErr}</span>{/if}
					{/if}
				</div>
			{/if}
			{#if tc.content_truncated}
				<div class="tc-note">Result truncated (S3 offload unavailable).</div>
			{/if}
		</div>
	{/if}
</div>

<style>
	.tool-call {
		border: 1px solid var(--dk-border-soft, rgba(120, 120, 120, 0.3));
		border-radius: 8px;
		margin: 6px 0;
		background: var(--dk-surface-2, rgba(255, 255, 255, 0.02));
		overflow: hidden;
	}
	.tc-head {
		display: flex;
		align-items: center;
		gap: 0.5rem;
		width: 100%;
		padding: 8px 10px;
		background: transparent;
		border: none;
		cursor: pointer;
		color: var(--dk-text, #e5e5e5);
		font-size: 0.85rem;
		text-align: left;
	}
	.tc-head:hover {
		background: color-mix(in srgb, var(--accent, #737373) 8%, transparent);
	}
	.tc-caret {
		width: 1rem;
		opacity: 0.7;
	}
	.tc-icon {
		opacity: 0.85;
	}
	.tc-name {
		font-family: var(--dk-mono, ui-monospace, monospace);
		font-weight: 600;
		flex: 1;
	}
	.tc-status {
		font-size: 0.7rem;
		padding: 1px 7px;
		border-radius: 999px;
		border: 1px solid transparent;
	}
	.st-ok {
		color: #4ade80;
		border-color: rgba(74, 222, 128, 0.4);
	}
	.st-fail {
		color: #f87171;
		border-color: rgba(248, 113, 113, 0.4);
	}
	.st-run {
		color: #fbbf24;
		border-color: rgba(251, 191, 36, 0.4);
	}
	.tc-badge {
		font-size: 0.65rem;
		opacity: 0.6;
		border: 1px dashed var(--dk-border-soft, #555);
		border-radius: 4px;
		padding: 0 5px;
	}
	.tc-body {
		padding: 4px 12px 12px;
		border-top: 1px solid var(--dk-border-soft, rgba(120, 120, 120, 0.2));
	}
	.tc-section {
		margin-top: 8px;
	}
	.tc-label {
		font-size: 0.7rem;
		text-transform: uppercase;
		letter-spacing: 0.04em;
		color: var(--dk-text-muted, #a3a3a3);
		margin-bottom: 3px;
	}
	.tc-pre {
		margin: 0;
		padding: 8px 10px;
		background: var(--dk-bg, #0f0f0f);
		border-radius: 6px;
		font-family: var(--dk-mono, ui-monospace, monospace);
		font-size: 0.78rem;
		line-height: 1.4;
		white-space: pre-wrap;
		word-break: break-word;
		max-height: 380px;
		overflow: auto;
		color: var(--dk-text, #d4d4d4);
	}
	.tc-preview {
		opacity: 0.75;
		max-height: 140px;
	}
	.tc-expand {
		font-size: 0.78rem;
		padding: 4px 10px;
		border: 1px solid var(--dk-border-soft, #555);
		border-radius: 6px;
		background: transparent;
		color: var(--dk-text, #e5e5e5);
		cursor: pointer;
	}
	.tc-expand:hover {
		background: color-mix(in srgb, var(--accent, #737373) 12%, transparent);
	}
	.tc-err {
		color: #f87171;
		font-size: 0.75rem;
		margin-left: 8px;
	}
	.tc-note {
		font-size: 0.72rem;
		color: var(--dk-text-muted, #a3a3a3);
		margin-top: 6px;
		font-style: italic;
	}
</style>
