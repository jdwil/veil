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
		patchFocus,
	} from '$lib/agent';
	import { startIdeFocusBridge, stopIdeFocusBridge } from '$lib/ide/focusBridge';
	import '$lib/ide/ide-app.css';

	const projectId = $derived(($page.params.id ?? '').trim());
	const focusFile = $derived($page.url.searchParams.get('file') || '');
	const focusConstruct = $derived($page.url.searchParams.get('construct') || '');
	const focusSession = $derived($page.url.searchParams.get('session') || '');
	const focusBranch = $derived($page.url.searchParams.get('branch') || '');

	$effect(() => {
		const p = projectId;
		if (!p) return;
		setAgentContext({
			page: `/projects/${p}/ide`,
			project: p,
		});
		patchFocus({
			route: `/projects/${p}/ide`,
			project: p,
		});
	});

	onMount(() => {
		const p = projectId;
		if (!p) return;
		const u = new URL(window.location.href);
		void ensureCodingSession(p, {
			sessionId: u.searchParams.get('session') || focusSession,
			branchName: u.searchParams.get('branch') || focusBranch
		});
		const stop = startIdeFocusBridge(p);
		return () => {
			stop();
			stopIdeFocusBridge();
		};
	});
</script>

{#if projectId}
	<div class="native-ide" data-veil-role="native-ide" data-project={projectId}>
		<IdeApp project={projectId} {focusFile} {focusConstruct} {focusSession} {focusBranch} />
	</div>
{:else}
	<p class="native-ide-missing">Missing project id.</p>
{/if}

<style>
	.native-ide {
		/* Share flex space with AgentDock — never position:fixed/absolute full viewport */
		flex: 1 1 auto;
		display: flex;
		flex-direction: column;
		min-height: 0;
		min-width: 0;
		width: 100%;
		height: 100%;
		overflow: hidden;
		background: var(--dk-surface, #0f0f0f);
	}
	.native-ide :global(.viewer-container) {
		flex: 1 1 auto;
		min-height: 0;
		min-width: 0;
		width: 100%;
		height: 100%;
	}
	.native-ide-missing {
		padding: 2rem;
		color: var(--dk-text-muted, #a3a3a3);
	}
</style>
