<script lang="ts">
	/**
	 * Native product IDE — same Svelte app as the shell (no iframe, no /viewer hop).
	 * AgentDock lives in +layout; IdeApp is the dual-loop workspace.
	 */
	import { page } from '$app/stores';
	import { onMount } from 'svelte';
	import IdeApp from '$lib/ide/IdeApp.svelte';
	import {
		setAgentContext,
		ensureCodingSession,
	} from '$lib/agent';
	import '$lib/ide/ide-app.css';

	const projectId = $derived(($page.params.id ?? '').trim());

	$effect(() => {
		const p = projectId;
		if (!p) return;
		setAgentContext({
			page: `/projects/${p}/ide`,
			project: p,
		});
	});

	onMount(() => {
		const p = projectId;
		if (p) void ensureCodingSession(p);
	});
</script>

{#if projectId}
	<div class="native-ide" data-veil-role="native-ide" data-project={projectId}>
		<IdeApp project={projectId} />
	</div>
{:else}
	<p class="native-ide-missing">Missing project id.</p>
{/if}

<style>
	.native-ide {
		/* Fill shell main content (layout already full-bleed for /ide routes) */
		position: absolute;
		inset: 0;
		display: flex;
		flex-direction: column;
		min-height: 0;
		min-width: 0;
		overflow: hidden;
		background: var(--dk-surface, #0f0f0f);
	}
	.native-ide :global(.viewer-container) {
		flex: 1;
		min-height: 0;
		height: 100%;
	}
	.native-ide-missing {
		padding: 2rem;
		color: var(--dk-text-muted, #a3a3a3);
	}
</style>
