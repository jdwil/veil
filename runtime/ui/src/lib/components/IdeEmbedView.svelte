<script lang="ts">
	/**
	 * @deprecated Product IDE is native at `/projects/[id]/ide` (no iframe).
	 * This component re-exports the native IdeApp for any residual imports.
	 */
	import IdeApp from '$lib/ide/IdeApp.svelte';
	import { setAgentContext, ensureCodingSession } from '$lib/agent';
	import { onMount } from 'svelte';
	import '$lib/ide/ide-app.css';

	interface Props {
		project: string;
	}
	let { project }: Props = $props();

	$effect(() => {
		const p = project.trim();
		if (p) {
			setAgentContext({ page: `/projects/${p}/ide`, project: p });
		}
	});

	onMount(() => {
		if (project.trim()) void ensureCodingSession(project.trim());
	});
</script>

<div class="native-ide-fallback" data-veil-role="native-ide">
	{#if project.trim()}
		<IdeApp project={project.trim()} />
	{/if}
</div>

<style>
	.native-ide-fallback {
		position: absolute;
		inset: 0;
		display: flex;
		flex-direction: column;
		min-height: 0;
		overflow: hidden;
	}
	.native-ide-fallback :global(.viewer-container) {
		flex: 1;
		min-height: 0;
		height: 100%;
	}
</style>
