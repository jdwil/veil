<script lang="ts">
	import type { Snippet } from 'svelte';

	interface MenuItem {
		label?: string;
		href?: string;
		danger?: boolean;
		disabled?: boolean;
		onSelect?: () => void;
	}

	interface Props {
		items?: MenuItem[];
		align?: string;
		aria_label?: string;
		children?: Snippet | null;
		agent?: Record<string, unknown>;
	}
	let { items = [], align = 'right', aria_label = 'Actions', children, agent = {} }: Props =
		$props();

	let open = $state(false);
	let root_el: HTMLElement | undefined = $state();
	let menu_el: HTMLElement | undefined = $state();

	let veil_agent = $derived({ version: 1, role: 'context-menu', product: agent, runtime: { open } });

	function close() {
		open = false;
	}

	function place_menu(menu: HTMLElement, root: HTMLElement) {
		const r = root.getBoundingClientRect();
		menu.style.position = 'fixed';
		menu.style.margin = '0';
		menu.style.inset = 'auto';
		menu.style.top = `${Math.round(r.bottom + 4)}px`;
		menu.style.zIndex = '10000';
		menu.style.pointerEvents = 'auto';
		menu.style.background =
			'color-mix(in srgb, var(--dk-surface-2, #242424) 94%, transparent)';
		if (align === 'right') {
			menu.style.right = `${Math.round(window.innerWidth - r.right)}px`;
			menu.style.left = 'auto';
		} else {
			menu.style.left = `${Math.round(r.left)}px`;
			menu.style.right = 'auto';
		}
	}

	$effect(() => {
		const menu = menu_el;
		const root = root_el;
		if (!open || !menu || !root) {
			if (menu && typeof menu.hidePopover === 'function') {
				try {
					menu.hidePopover();
				} catch {
					/* ignore */
				}
			}
			return;
		}
		place_menu(menu, root);
		let portaled = false;
		if (typeof menu.showPopover === 'function') {
			try {
				if (!menu.matches(':popover-open')) menu.showPopover();
			} catch {
				if (menu.parentElement !== document.body) {
					document.body.appendChild(menu);
					portaled = true;
				}
			}
		} else if (menu.parentElement !== document.body) {
			document.body.appendChild(menu);
			portaled = true;
		}
		place_menu(menu, root);
		return () => {
			if (typeof menu.hidePopover === 'function') {
				try {
					menu.hidePopover();
				} catch {
					/* ignore */
				}
			}
			if (portaled && root.isConnected && menu.parentElement === document.body) {
				root.appendChild(menu);
			}
		};
	});
</script>

<svelte:window
	onclick={(e) => {
		if (!open) return;
		const t = e.target;
		if (root_el?.contains?.(t as Node) || menu_el?.contains?.(t as Node)) return;
		close();
	}}
	onkeydown={(e) => {
		if (e.key === 'Escape' && open) close();
	}}
/>
<div
	class="dk-ctx"
	class:dk-ctx--open={open}
	bind:this={root_el}
	data-veil-role="context-menu"
	data-veil-agent={JSON.stringify(veil_agent)}
>
	<button
		type="button"
		class="dk-ctx__trigger"
		aria-label={aria_label}
		aria-haspopup="menu"
		aria-expanded={open}
		onclick={(e) => {
			e.stopPropagation();
			e.preventDefault();
			open = !open;
		}}
	>
		<span class="dk-ctx__dots" aria-hidden="true">⋮</span>
	</button>
	<div
		bind:this={menu_el}
		class="dk-ctx__menu"
		class:dk-ctx__menu--left={align === 'left'}
		class:dk-ctx__menu--right={align === 'right'}
		role="menu"
		popover="manual"
		onclick={(e) => e.stopPropagation()}
	>
		{#if children}
			{@render children()}
		{:else}
			{#each items || [] as item}
				{#if item.href && !item.disabled}
					<a
						href={item.href}
						class="dk-ctx__item"
						class:dk-ctx__item--danger={item.danger}
						role="menuitem"
						tabindex={open ? 0 : -1}
						onclick={(e) => {
							e.preventDefault();
							e.stopPropagation();
							close();
							if (item.onSelect) item.onSelect();
							else if (item.href) window.location.href = item.href;
						}}>{item.label}</a
					>
				{:else}
					<button
						type="button"
						class="dk-ctx__item"
						class:dk-ctx__item--danger={item.danger}
						role="menuitem"
						disabled={item.disabled}
						tabindex={open ? 0 : -1}
						onclick={(e) => {
							e.stopPropagation();
							if (!item.disabled) {
								close();
								item.onSelect?.();
							}
						}}>{item.label}</button
					>
				{/if}
			{/each}
		{/if}
	</div>
</div>

<style>
	.dk-ctx__menu[popover],
	.dk-ctx__menu:popover-open {
		margin: 0;
		inset: auto;
		border: 1px solid var(--dk-glass-border, rgba(255, 255, 255, 0.06));
		background: color-mix(in srgb, var(--dk-surface-2, #242424) 94%, transparent);
		color: var(--dk-text, #e5e5e5);
		padding: 0.35rem;
		min-width: 10rem;
		border-radius: var(--dk-radius-sm, 0.55rem);
		box-shadow: var(--dk-shadow-lg, 0 24px 64px rgba(0, 0, 0, 0.55));
		overflow: visible;
		pointer-events: auto;
	}
	.dk-ctx__item {
		display: flex;
		align-items: center;
		width: 100%;
		padding: 0.55rem 0.8rem;
		border: none;
		background: transparent;
		color: inherit;
		font: inherit;
		text-align: left;
		text-decoration: none;
		cursor: pointer;
		border-radius: 0.4rem;
	}
	.dk-ctx__item--danger {
		color: #f87171;
	}
	.dk-ctx__item:hover:not(:disabled) {
		background: rgba(255, 255, 255, 0.07);
	}
</style>
