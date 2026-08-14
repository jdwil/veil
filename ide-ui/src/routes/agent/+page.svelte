<script lang="ts">
	/**
	 * Agent pop-out window — full-height agent panel without the canvas.
	 * Opened via AgentPlacementControl → Window.
	 */
	import { onMount, onDestroy } from 'svelte';
	import AgentSideRail from '$lib/AgentSideRail.svelte';
	import {
		notifyPopoutReady,
		notifyPopoutClosed,
		setAgentPlacement,
		onAgentLayoutMessage
	} from '$lib/agentLayout';
	import { agentLoadHandoff, agentSaveHandoff } from '$lib/agentSession';

	let unsub = () => {};

	onMount(() => {
		document.documentElement.classList.add('dark');
		document.title = 'VEIL Agent';
		agentLoadHandoff();
		setAgentPlacement('window');
		notifyPopoutReady();
		unsub = onAgentLayoutMessage((msg) => {
			if (msg.type === 'focus-popout') {
				window.focus();
			}
		});
		const onBeforeUnload = () => {
			agentSaveHandoff();
			notifyPopoutClosed();
		};
		window.addEventListener('beforeunload', onBeforeUnload);
		return () => {
			window.removeEventListener('beforeunload', onBeforeUnload);
		};
	});

	onDestroy(() => {
		unsub();
		notifyPopoutClosed();
	});
</script>

<div class="popout-shell">
	<AgentSideRail popout side="right" />
</div>

<style>
	:global(html),
	:global(body) {
		margin: 0;
		height: 100%;
		overflow: hidden;
		background: #0f0f0f;
	}
	.popout-shell {
		width: 100vw;
		height: 100vh;
		display: flex;
		flex-direction: column;
	}
</style>
