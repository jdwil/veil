<script lang="ts">
	/**
	 * Vertical agent rail for left/right of canvas (and pop-out shell).
	 */
	import AetherAgentPanel from './AetherAgentPanel.svelte';
	import AgentPlacementControl from './AgentPlacementControl.svelte';
	import {
		agentSideWidth,
		setAgentSideWidth,
		SIDE_MIN,
		SIDE_MAX,
		setAgentPlacement
	} from '$lib/agentLayout';
	import { selectedNodeId, irGraph } from '$lib/store';
	import { agentInsertToken } from '$lib/agentSession';

	interface Props {
		/** Which edge this rail is on (affects resize handle side). */
		side?: 'left' | 'right';
		/** Pop-out window: full viewport, no width drag. */
		popout?: boolean;
		insertToken?: string;
	}

	let { side = 'right', popout = false, insertToken = '' }: Props = $props();

	let resizing = $state(false);

	let selectionLabel = $derived.by(() => {
		const sid = $selectedNodeId;
		const g = $irGraph;
		if (!sid || !g) return null;
		const n = g.nodes.find((x) => String(x.id) === sid);
		if (!n) return null;
		const kind = n.metadata.subkind || n.kind;
		return `${kind} ${n.name}`;
	});

	function onResizeDown(e: PointerEvent) {
		e.preventDefault();
		resizing = true;
		const startX = e.clientX;
		const startW = $agentSideWidth;
		const target = e.currentTarget as HTMLElement;
		try {
			target.setPointerCapture(e.pointerId);
		} catch {
			/* ignore */
		}

		function onMove(ev: PointerEvent) {
			// Left rail: drag right increases width; right rail: drag left increases width
			const delta = side === 'left' ? ev.clientX - startX : startX - ev.clientX;
			setAgentSideWidth(startW + delta);
		}
		function onUp(ev: PointerEvent) {
			resizing = false;
			try {
				target.releasePointerCapture(ev.pointerId);
			} catch {
				/* ignore */
			}
			window.removeEventListener('pointermove', onMove);
			window.removeEventListener('pointerup', onUp);
		}
		window.addEventListener('pointermove', onMove);
		window.addEventListener('pointerup', onUp);
	}

</script>

<aside
	class="agent-rail"
	class:left={side === 'left'}
	class:right={side === 'right'}
	class:popout
	class:resizing
	style:width={popout ? '100%' : `${$agentSideWidth}px`}
	aria-label="Agent panel"
>
	{#if !popout && side === 'right'}
		<div
			class="v-resize"
			role="separator"
			aria-orientation="vertical"
			aria-label="Resize agent panel"
			aria-valuenow={$agentSideWidth}
			aria-valuemin={SIDE_MIN}
			aria-valuemax={SIDE_MAX}
			tabindex="0"
			onpointerdown={onResizeDown}
			title="Drag to resize"
		></div>
	{/if}

	<div class="rail-chrome">
		<span class="rail-title">Agent</span>
		{#if selectionLabel && !popout}
			<button
				type="button"
				class="insert-btn"
				title="Insert selected construct into agent prompt"
				onclick={() => agentInsertToken(selectionLabel!)}
			>
				+ Insert
			</button>
		{/if}
		<div class="rail-actions">
			{#if !popout}
				<AgentPlacementControl variant="compact" />
			{:else}
				<button
					type="button"
					class="dock-btn"
					title="Dock agent back into the IDE"
					onclick={() => {
						// Tell opener if present, else set placement for next open
						if (window.opener && !window.opener.closed) {
							try {
								window.opener.focus();
							} catch {
								/* ignore */
							}
						}
						setAgentPlacement('right');
						window.close();
					}}
				>
					Dock
				</button>
			{/if}
		</div>
	</div>

	<div class="rail-body">
		<AetherAgentPanel embedded insertToken={insertToken} />
	</div>

	{#if !popout && side === 'left'}
		<div
			class="v-resize"
			role="separator"
			aria-orientation="vertical"
			aria-label="Resize agent panel"
			aria-valuenow={$agentSideWidth}
			aria-valuemin={SIDE_MIN}
			aria-valuemax={SIDE_MAX}
			tabindex="0"
			onpointerdown={onResizeDown}
			title="Drag to resize"
		></div>
	{/if}
</aside>

<style>
	.agent-rail {
		position: relative;
		flex-shrink: 0;
		display: flex;
		flex-direction: column;
		min-width: 0;
		min-height: 0;
		background: var(--veil-surface, #141414);
		border-color: var(--veil-border, #2e2e2e);
		z-index: 15;
	}
	.agent-rail.right {
		border-left: 1px solid var(--veil-border, #2e2e2e);
	}
	.agent-rail.left {
		border-right: 1px solid var(--veil-border, #2e2e2e);
	}
	.agent-rail.popout {
		width: 100% !important;
		height: 100%;
		border: none;
	}
	.agent-rail.resizing {
		user-select: none;
	}
	.v-resize {
		position: absolute;
		top: 0;
		bottom: 0;
		width: 5px;
		cursor: col-resize;
		z-index: 6;
		touch-action: none;
	}
	.agent-rail.right .v-resize {
		left: -2px;
	}
	.agent-rail.left .v-resize {
		right: -2px;
	}
	.v-resize:hover,
	.agent-rail.resizing .v-resize {
		background: var(--veil-accent, #60a5fa);
	}
	.rail-chrome {
		display: flex;
		align-items: center;
		gap: 8px;
		padding: 4px 8px;
		border-bottom: 1px solid var(--veil-border, #2e2e2e);
		background: var(--veil-surface-alt, #1a1a1a);
		flex-shrink: 0;
	}
	.rail-title {
		font-size: 11px;
		font-weight: 700;
		letter-spacing: 0.03em;
		color: var(--veil-text, #e5e5e5);
	}
	.rail-actions {
		margin-left: auto;
		display: flex;
		align-items: center;
		gap: 6px;
	}
	.insert-btn,
	.dock-btn {
		font-size: 10px;
		padding: 3px 8px;
		border-radius: 4px;
		border: 1px solid var(--veil-border, #2e2e2e);
		background: var(--veil-accent-subtle, rgba(96, 165, 250, 0.1));
		color: var(--veil-text-secondary, #a3a3a3);
		cursor: pointer;
	}
	.insert-btn:hover,
	.dock-btn:hover {
		color: var(--veil-text, #e5e5e5);
		border-color: var(--veil-accent, #60a5fa);
	}
	.rail-body {
		flex: 1;
		min-height: 0;
		display: flex;
		flex-direction: column;
		overflow: hidden;
	}
</style>
