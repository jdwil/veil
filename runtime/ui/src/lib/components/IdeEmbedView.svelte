<script lang="ts">
	/**
	 * Embeds dual-loop IDE UI inside the runtime shell.
	 * Agent chrome stays in the parent (AgentDock); IDE loads with showAgentRail=0.
	 */
	import { onDestroy, onMount } from 'svelte';
	import {
		registerIdeFrame,
		unregisterIdeFrame,
		setAgentContext,
	} from '$lib/agent';

	interface Props {
		project: string;
	}
	let { project }: Props = $props();

	let iframeEl: HTMLIFrameElement | null = $state(null);
	let loadError = $state<string | null>(null);
	let loaded = $state(false);

	/**
	 * Same-origin /viewer via ProductHost (or Vite proxy). Agent rail off — parent owns chat.
	 * `v` busts stale iframe caches when the embedded IDE SPA is rebuilt (hashed assets
	 * only update if index.html is re-fetched).
	 */
	const viewerSrc = $derived.by(() => {
		const p = project.trim();
		if (!p) return '';
		const q = new URLSearchParams({
			project: p,
			showAgentRail: '0',
			// bump when outline/layout behavior changes and embed must reload
			v: 'issues-agent-1',
		});
		return `/viewer/?${q.toString()}`;
	});

	const projectHref = $derived(
		project ? `/projects/${encodeURIComponent(project)}` : '/projects'
	);
	const standaloneHref = $derived(
		project ? `/viewer/?project=${encodeURIComponent(project)}` : '/viewer/'
	);

	function onFrameLoad() {
		loaded = true;
		loadError = null;
		if (iframeEl) {
			registerIdeFrame(iframeEl);
		}
	}

	function onFrameError() {
		loadError = 'Failed to load the IDE viewer. Is ProductHost serving /viewer?';
	}

	$effect(() => {
		const p = project;
		if (p) {
			setAgentContext({
				page: `/projects/${p}/ide`,
				project: p,
			});
		}
	});

	onMount(() => {
		if (iframeEl) {
			registerIdeFrame(iframeEl);
		}
	});

	onDestroy(() => {
		unregisterIdeFrame();
	});
</script>

<div
	class="ide-embed"
	data-veil-role="ide-embed"
	data-veil-agent={JSON.stringify({
		version: 1,
		role: 'ide-embed',
		product: { project, intent: 'edit-in-ide' },
	})}
>
	<header class="ide-embed__bar">
		<div class="ide-embed__left">
			<a class="ide-embed__back" href={projectHref} aria-label="Back to project">
				<svg
					width="18"
					height="18"
					viewBox="0 0 24 24"
					fill="none"
					stroke="currentColor"
					stroke-width="2"
					aria-hidden="true"
				>
					<path d="M19 12H5M12 19l-7-7 7-7" />
				</svg>
				<span>Project</span>
			</a>
			<span class="ide-embed__sep" aria-hidden="true">/</span>
			<strong class="ide-embed__title">{project || 'IDE'}</strong>
			<span class="ide-embed__badge">IDE</span>
		</div>
		<div class="ide-embed__right">
			<span class="ide-embed__hint">Menu ⌘B · Agent ⌘K</span>
			<a
				class="ide-embed__standalone"
				href={standaloneHref}
				target="_blank"
				rel="noopener noreferrer"
				title="Open full IDE with its own agent pane"
			>
				Standalone
			</a>
		</div>
	</header>

	{#if !project}
		<div class="ide-embed__empty" role="alert">
			<p>No project specified.</p>
			<a href="/projects">Back to projects</a>
		</div>
	{:else if loadError}
		<div class="ide-embed__empty" role="alert">
			<p>{loadError}</p>
			<a href={standaloneHref} target="_blank" rel="noopener noreferrer">Open standalone IDE</a>
		</div>
	{:else}
		<iframe
			bind:this={iframeEl}
			class="ide-embed__frame"
			class:ready={loaded}
			src={viewerSrc}
			title={`VEIL IDE — ${project}`}
			onload={onFrameLoad}
			onerror={onFrameError}
			allow="clipboard-read; clipboard-write"
		></iframe>
	{/if}
</div>

<style>
	.ide-embed {
		display: flex;
		flex-direction: column;
		height: 100%;
		min-height: 0;
		width: 100%;
		background: var(--dk-bg, #0c0e12);
		overflow: hidden;
	}

	.ide-embed__bar {
		flex-shrink: 0;
		display: flex;
		align-items: center;
		justify-content: space-between;
		gap: 1rem;
		padding: 0.45rem 0.85rem;
		border-bottom: 1px solid var(--dk-border-soft, rgba(255, 255, 255, 0.08));
		background: color-mix(in srgb, var(--dk-glass, #141820) 92%, transparent);
		backdrop-filter: blur(12px);
	}

	.ide-embed__left {
		display: flex;
		align-items: center;
		gap: 0.5rem;
		min-width: 0;
	}

	.ide-embed__back {
		display: inline-flex;
		align-items: center;
		gap: 0.35rem;
		color: var(--text-dim, #9ca3af);
		text-decoration: none;
		font-size: 0.8rem;
		padding: 0.25rem 0.4rem;
		border-radius: 6px;
		transition:
			color 140ms ease,
			background 140ms ease;
	}
	.ide-embed__back:hover {
		color: var(--accent, #a3a3a3);
		background: color-mix(in srgb, var(--accent, #a3a3a3) 12%, transparent);
	}

	.ide-embed__sep {
		color: var(--text-dim, #6b7280);
		opacity: 0.6;
	}

	.ide-embed__title {
		font-size: 0.9rem;
		font-weight: 600;
		color: var(--text, #e5e7eb);
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}

	.ide-embed__badge {
		font-size: 0.65rem;
		font-weight: 600;
		letter-spacing: 0.04em;
		text-transform: uppercase;
		padding: 0.15rem 0.45rem;
		border-radius: 999px;
		color: var(--accent, #a3a3a3);
		border: 1px solid color-mix(in srgb, var(--accent, #a3a3a3) 40%, transparent);
		background: color-mix(in srgb, var(--accent, #a3a3a3) 12%, transparent);
	}

	.ide-embed__right {
		display: flex;
		align-items: center;
		gap: 0.75rem;
		flex-shrink: 0;
	}

	.ide-embed__hint {
		font-size: 0.72rem;
		color: var(--text-dim, #6b7280);
	}

	.ide-embed__standalone {
		font-size: 0.75rem;
		color: var(--text-dim, #9ca3af);
		text-decoration: none;
		padding: 0.25rem 0.55rem;
		border-radius: 6px;
		border: 1px solid var(--dk-border-soft, rgba(255, 255, 255, 0.1));
	}
	.ide-embed__standalone:hover {
		color: var(--accent, #a3a3a3);
		border-color: color-mix(in srgb, var(--accent, #a3a3a3) 40%, transparent);
	}

	.ide-embed__frame {
		flex: 1;
		min-height: 0;
		width: 100%;
		border: 0;
		background: var(--dk-bg, #0c0e12);
		opacity: 0.85;
		transition: opacity 200ms ease;
	}
	.ide-embed__frame.ready {
		opacity: 1;
	}

	.ide-embed__empty {
		flex: 1;
		display: flex;
		flex-direction: column;
		align-items: center;
		justify-content: center;
		gap: 0.75rem;
		color: var(--text-dim, #9ca3af);
		padding: 2rem;
	}
	.ide-embed__empty a {
		color: var(--accent, #a3a3a3);
	}

	@media (max-width: 720px) {
		.ide-embed__hint {
			display: none;
		}
	}
</style>
