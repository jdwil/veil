<script lang="ts">
	/**
	 * Compact + menu for tree/flat layouts (replaces the left Constructs palette).
	 * Lists constructs valid for the current host and creates via click.
	 */
	import { onMount } from 'svelte';
	import { NODE_STYLES, type NodeKind } from '$lib/ide/types';
	import { paletteConfig, isFlowComposerMode } from '$lib/ide/store';

	export interface CreateItem {
		kind: NodeKind;
		label: string;
		icon: string;
		color?: string;
		category: string;
		name?: string;
		keyword?: string;
		group?: string;
		dg?: string;
		description?: string;
		is_step?: boolean;
	}

	interface Props {
		contextKind?: string;
		contextKindCore?: string;
		activeGroup?: string | null;
		disabled?: boolean;
		/** Called with a palette-shaped item when the user picks one. */
		onCreate: (item: CreateItem) => void | Promise<void>;
	}

	let {
		contextKind = 'Solution',
		contextKindCore = 'Solution',
		activeGroup = null,
		disabled = false,
		onCreate,
	}: Props = $props();

	let open = $state(false);
	let rootEl: HTMLDivElement | null = $state(null);
	let busy = $state(false);

	let items = $derived.by((): CreateItem[] => {
		const config = $paletteConfig;
		if (!config?.length) return [];
		const ck = contextKind || 'Solution';
		const results: CreateItem[] = [];
		const flow = isFlowComposerMode();

		for (const c of config) {
			if ((c.entry_type || 'construct') !== 'construct') continue;
			if (flow && (c.name === 'ReactionPackage' || c.name === 'Edge')) continue;

			let show = false;
			if (flow) {
				show = true;
			} else if (ck === 'Solution' && (c.allowed_in === 'top' || c.allowed_in === 'any')) {
				show = true;
			} else if (
				c.allowed_in === ck ||
				c.allowed_in === contextKindCore ||
				c.allowed_in.split(',').map((s) => s.trim()).includes(ck) ||
				c.allowed_in.split(',').map((s) => s.trim()).includes(contextKindCore)
			) {
				if (c.group && activeGroup) show = c.group === activeGroup;
				else show = true;
			} else if (c.allowed_in === 'any' && ck !== 'Solution') {
				if (c.kind === 'Group' && activeGroup) show = false;
				else show = true;
			}

			if (show) {
				results.push({
					kind: c.kind as NodeKind,
					label: c.label || c.name,
					icon: c.icon || '◇',
					color: c.color,
					category: c.group || 'General',
					name: c.name,
					keyword: c.keyword,
					group: c.group || undefined,
					dg: c.dg || undefined,
					description: c.description || undefined,
					is_step: !!(c as { is_step?: boolean }).is_step,
				});
			}
		}
		return results;
	});

	let categories = $derived([...new Set(items.map((i) => i.category))]);

	function toggle() {
		if (disabled) return;
		open = !open;
	}

	async function pick(item: CreateItem) {
		if (busy) return;
		busy = true;
		try {
			await onCreate(item);
			open = false;
		} finally {
			busy = false;
		}
	}

	onMount(() => {
		const onDoc = (e: MouseEvent) => {
			if (!open || !rootEl) return;
			if (!rootEl.contains(e.target as Node)) open = false;
		};
		const onKey = (e: KeyboardEvent) => {
			if (e.key === 'Escape') open = false;
		};
		document.addEventListener('mousedown', onDoc);
		document.addEventListener('keydown', onKey);
		return () => {
			document.removeEventListener('mousedown', onDoc);
			document.removeEventListener('keydown', onKey);
		};
	});
</script>

<div class="create-menu" bind:this={rootEl}>
	<button
		type="button"
		class="create-btn"
		class:open
		disabled={disabled || items.length === 0}
		onclick={toggle}
		title="Add construct"
		aria-haspopup="menu"
		aria-expanded={open}
	>
		<span class="create-plus" aria-hidden="true">+</span>
		<span class="create-label">Add</span>
	</button>

	{#if open}
		<div class="create-popover" role="menu" aria-label="Add construct">
			{#if items.length === 0}
				<p class="create-empty">No constructs available at this level</p>
			{:else}
				{#each categories as category}
					<div class="create-cat">
						<span class="create-cat-label">{category}</span>
						{#each items.filter((i) => i.category === category) as item}
							<button
								type="button"
								class="create-item"
								role="menuitem"
								style="--tile-color: {item.color || NODE_STYLES[item.kind]?.color || 'var(--veil-text-dim)'}"
								title={item.description || item.label}
								disabled={busy}
								onclick={() => void pick(item)}
							>
								<span class="create-item-icon">{item.icon}</span>
								<span class="create-item-text">
									<span class="create-item-label">{item.label}</span>
									{#if item.description}
										<span class="create-item-desc">{item.description}</span>
									{/if}
								</span>
							</button>
						{/each}
					</div>
				{/each}
			{/if}
		</div>
	{/if}
</div>

<style>
	.create-menu {
		position: relative;
		flex-shrink: 0;
	}

	.create-btn {
		display: inline-flex;
		align-items: center;
		gap: 0.3rem;
		padding: 0.28rem 0.55rem;
		border-radius: 6px;
		border: 1px solid var(--veil-border, #2e2e2e);
		background: var(--veil-surface, #1a1a1a);
		color: var(--veil-text, #e5e5e5);
		font-size: 0.72rem;
		font-weight: 600;
		cursor: pointer;
		font-family: inherit;
		transition:
			background 140ms ease,
			border-color 140ms ease;
	}
	.create-btn:hover:not(:disabled) {
		border-color: var(--veil-text-dim, #a3a3a3);
		background: color-mix(in srgb, var(--veil-text-dim, #a3a3a3) 12%, transparent);
	}
	.create-btn:disabled {
		opacity: 0.45;
		cursor: not-allowed;
	}
	.create-btn.open {
		border-color: var(--veil-text-secondary, #a3a3a3);
	}

	.create-plus {
		font-size: 0.95rem;
		line-height: 1;
		font-weight: 700;
	}

	.create-popover {
		position: absolute;
		top: calc(100% + 6px);
		left: 0;
		z-index: 40;
		min-width: 220px;
		max-width: 280px;
		max-height: min(420px, 60vh);
		overflow-y: auto;
		padding: 0.4rem;
		border-radius: 8px;
		border: 1px solid var(--veil-border, #2e2e2e);
		background: var(--veil-surface, #1a1a1a);
		box-shadow: 0 12px 40px rgba(0, 0, 0, 0.55);
	}

	.create-empty {
		margin: 0;
		padding: 0.75rem;
		font-size: 0.75rem;
		color: var(--veil-text-dim, #a3a3a3);
	}

	.create-cat {
		display: flex;
		flex-direction: column;
		gap: 2px;
		margin-bottom: 0.45rem;
	}
	.create-cat:last-child {
		margin-bottom: 0;
	}

	.create-cat-label {
		font-size: 0.62rem;
		text-transform: uppercase;
		letter-spacing: 0.06em;
		color: var(--veil-text-faint, #737373);
		padding: 0.2rem 0.4rem;
	}

	.create-item {
		display: flex;
		align-items: flex-start;
		gap: 0.5rem;
		width: 100%;
		text-align: left;
		padding: 0.4rem 0.5rem;
		border: none;
		border-radius: 6px;
		background: transparent;
		color: var(--veil-text, #e5e5e5);
		cursor: pointer;
		font-family: inherit;
		border-left: 2px solid transparent;
		transition: background 120ms ease;
	}
	.create-item:hover:not(:disabled) {
		background: color-mix(in srgb, var(--tile-color, #737373) 14%, transparent);
		border-left-color: var(--tile-color, #737373);
	}
	.create-item:disabled {
		opacity: 0.5;
		cursor: wait;
	}

	.create-item-icon {
		flex-shrink: 0;
		font-size: 0.95rem;
		line-height: 1.2;
	}

	.create-item-text {
		display: flex;
		flex-direction: column;
		gap: 1px;
		min-width: 0;
	}

	.create-item-label {
		font-size: 0.78rem;
		font-weight: 600;
	}

	.create-item-desc {
		font-size: 0.65rem;
		color: var(--veil-text-dim, #a3a3a3);
		line-height: 1.3;
		display: -webkit-box;
		-webkit-line-clamp: 2;
		-webkit-box-orient: vertical;
		overflow: hidden;
	}
</style>
