<script lang="ts">
	/**
	 * Compact placement switcher for the Agent pane (bottom / left / right / window).
	 */
	import {
		agentPlacement,
		setAgentPlacement,
		openAgentPopout,
		PLACEMENT_OPTIONS,
		type AgentPlacement
	} from '$lib/agentLayout';

	interface Props {
		/** compact = icon row for dock chrome; full = labeled for side header */
		variant?: 'compact' | 'full';
	}

	let { variant = 'compact' }: Props = $props();

	function choose(id: AgentPlacement) {
		if (id === 'window') {
			const w = openAgentPopout();
			if (!w) {
				// Popup blocked — fall back to right rail
				setAgentPlacement('right');
			}
			return;
		}
		setAgentPlacement(id);
	}
</script>

<div
	class="placement-ctrl"
	class:full={variant === 'full'}
	role="group"
	aria-label="Agent pane placement"
>
	{#if variant === 'full'}
		<span class="lbl">Place</span>
	{/if}
	{#each PLACEMENT_OPTIONS as opt}
		<button
			type="button"
			class="place-btn"
			class:active={$agentPlacement === opt.id}
			title={opt.title}
			aria-pressed={$agentPlacement === opt.id}
			onclick={() => choose(opt.id)}
		>
			{opt.label}
		</button>
	{/each}
</div>

<style>
	.placement-ctrl {
		display: flex;
		align-items: center;
		gap: 2px;
		flex-shrink: 0;
	}
	.placement-ctrl.full {
		gap: 4px;
	}
	.lbl {
		font-size: 9px;
		font-weight: 700;
		letter-spacing: 0.05em;
		text-transform: uppercase;
		color: var(--veil-text-faint, #737373);
		margin-right: 2px;
	}
	.place-btn {
		background: transparent;
		border: 1px solid transparent;
		color: var(--veil-text-dim, #a3a3a3);
		font-size: 10px;
		font-weight: 600;
		padding: 3px 7px;
		border-radius: 4px;
		cursor: pointer;
		line-height: 1.2;
	}
	.place-btn:hover {
		color: var(--veil-text, #e5e5e5);
		background: var(--veil-accent-subtle, rgba(96, 165, 250, 0.1));
	}
	.place-btn.active {
		color: var(--veil-text, #e5e5e5);
		border-color: var(--veil-accent, #60a5fa);
		background: var(--veil-accent-subtle, rgba(96, 165, 250, 0.12));
	}
</style>
