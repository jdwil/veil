<script lang="ts">
	import { page } from '$app/stores';
	import {
		sidebarCollapsed,
		toggleSidebarCollapsed,
		shellTheme,
		toggleShellTheme,
	} from '$lib/shellLayout';

	let open_cr_count: number = $state(0);

	$effect(() => {
		void (async () => {
			open_cr_count = 0;
			try {
				const __u = new URL(
					'/api/pull_requests?status=ReadyForReview',
					typeof window !== 'undefined' ? window.location.origin : 'http://localhost'
				);
				const __r = await fetch(__u.toString());
				if (!__r.ok) return;
				const resp = await __r.json();
				open_cr_count = Array.isArray(resp) ? resp.length : 0;
			} catch {
				/* optional */
			}
		})();
	});

	const links = [
		{ href: '/dashboard', label: 'Dashboard', icon: '⌂' },
		{ href: '/projects', label: 'Projects', icon: '▣' },
		{ href: '/pulls', label: 'Changes', icon: '⇄', badge: true },
		{ href: '/deploy', label: 'Deploy', icon: '☁' },
		{ href: '/registry', label: 'Registry', icon: '⧉' },
		{ href: '/bus', label: 'Bus', icon: '⚡' },
		{ href: '/agents', label: 'Agents', icon: '◆' },
		{ href: '/config', label: 'Config', icon: '⚙' },
	] as const;

	function isActive(href: string): boolean {
		const path = $page.url.pathname;
		if (href === '/dashboard') return path === '/' || path.startsWith('/dashboard');
		if (href === '/projects') return path.startsWith('/projects');
		if (href === '/pulls') return path.startsWith('/pulls') || path.startsWith('/changecreate') || path.startsWith('/changedetail');
		return path === href || path.startsWith(href + '/');
	}
</script>

<aside
	class="sidebar"
	class:sidebar--collapsed={$sidebarCollapsed}
	aria-label="Main navigation"
	data-collapsed={$sidebarCollapsed ? 'true' : 'false'}
>
	<div class="sidebar__top">
		{#if !$sidebarCollapsed}
			<div class="logo"><span>◆</span> veil-runtime</div>
		{:else}
			<div class="logo logo--mark" title="veil-runtime"><span>◆</span></div>
		{/if}
		<button
			type="button"
			class="collapse-btn"
			onclick={() => toggleSidebarCollapsed()}
			title={$sidebarCollapsed ? 'Expand menu (⌘B)' : 'Collapse menu (⌘B)'}
			aria-label={$sidebarCollapsed ? 'Expand main menu' : 'Collapse main menu'}
			aria-expanded={!$sidebarCollapsed}
		>
			{$sidebarCollapsed ? '›' : '‹'}
		</button>
	</div>

	<nav>
		{#each links as link}
			<a
				href={link.href}
				class:nav-badge-link={link.badge}
				class:active={isActive(link.href)}
				aria-current={isActive(link.href) ? 'page' : undefined}
				title={link.label}
			>
				<span class="nav-icon" aria-hidden="true">{link.icon}</span>
				{#if !$sidebarCollapsed}
					<span class="nav-label">{link.label}</span>
					{#if link.badge && open_cr_count > 0}
						<span class="nav-badge">{open_cr_count}</span>
					{/if}
				{:else if link.badge && open_cr_count > 0}
					<span class="nav-badge nav-badge--dot" aria-label="{open_cr_count} open changes"></span>
				{/if}
			</a>
		{/each}
	</nav>

	<div class="sidebar__footer">
		<button
			type="button"
			class="theme-btn"
			onclick={() => toggleShellTheme()}
			title={$shellTheme === 'dark' ? 'Switch to light mode' : 'Switch to dark mode'}
			aria-label="Toggle color theme"
		>
			<span class="theme-btn__icon" aria-hidden="true">{$shellTheme === 'dark' ? '☀' : '☾'}</span>
			{#if !$sidebarCollapsed}
				<span class="theme-btn__label">{$shellTheme === 'dark' ? 'Light mode' : 'Dark mode'}</span>
			{/if}
		</button>
	</div>
</aside>

<style>
	.sidebar {
		width: 240px;
		flex-shrink: 0;
		height: 100%;
		background: var(--dk-glass, var(--surface));
		backdrop-filter: blur(16px) saturate(1.15);
		-webkit-backdrop-filter: blur(16px) saturate(1.15);
		border-right: 1px solid var(--dk-border-soft, var(--border));
		display: flex;
		flex-direction: column;
		padding: 12px 0 12px;
		overflow-x: hidden;
		overflow-y: auto;
		transition: width 200ms var(--dk-ease-out, cubic-bezier(0.16, 1, 0.3, 1));
	}

	.sidebar nav {
		flex: 1 1 auto;
		min-height: 0;
	}

	.sidebar__footer {
		flex-shrink: 0;
		padding: 10px 10px 4px;
		border-top: 1px solid var(--dk-border-soft, rgba(46, 46, 46, 0.65));
		margin-top: 8px;
	}

	.theme-btn {
		width: 100%;
		display: flex;
		align-items: center;
		gap: 0.5rem;
		padding: 0.45rem 0.6rem;
		border: 1px solid var(--dk-border-soft, rgba(46, 46, 46, 0.65));
		border-radius: 8px;
		background: transparent;
		color: var(--dk-text-muted, #a3a3a3);
		font-size: 0.8rem;
		cursor: pointer;
		transition: background 120ms ease, color 120ms ease, border-color 120ms ease;
	}
	.theme-btn:hover {
		background: var(--dk-surface-2, #242424);
		color: var(--dk-text, #e5e5e5);
		border-color: var(--dk-brand, #737373);
	}
	.theme-btn__icon {
		flex-shrink: 0;
		width: 1.25rem;
		text-align: center;
	}
	.theme-btn__label {
		white-space: nowrap;
		overflow: hidden;
		text-overflow: ellipsis;
	}
	.sidebar--collapsed .theme-btn {
		justify-content: center;
		padding: 0.45rem;
	}

	.sidebar--collapsed {
		width: 52px;
	}

	.sidebar__top {
		display: flex;
		align-items: center;
		justify-content: space-between;
		gap: 0.35rem;
		padding: 0 10px 16px;
		min-height: 2.25rem;
	}

	.sidebar--collapsed .sidebar__top {
		flex-direction: column;
		padding: 0 6px 12px;
		gap: 0.5rem;
	}

	.logo {
		display: flex;
		align-items: center;
		gap: 10px;
		padding: 4px 8px;
		font-weight: 700;
		font-size: 1.05rem;
		color: var(--text);
		letter-spacing: -0.02em;
		min-width: 0;
		white-space: nowrap;
		overflow: hidden;
	}
	.logo span {
		color: var(--accent);
		flex-shrink: 0;
	}
	.logo--mark {
		justify-content: center;
		padding: 4px;
		font-size: 1rem;
	}

	.collapse-btn {
		flex-shrink: 0;
		width: 1.75rem;
		height: 1.75rem;
		display: inline-flex;
		align-items: center;
		justify-content: center;
		border: 1px solid var(--dk-border-soft, rgba(46, 46, 46, 0.65));
		border-radius: 6px;
		background: transparent;
		color: var(--text-dim, #a3a3a3);
		cursor: pointer;
		font-size: 0.95rem;
		line-height: 1;
		padding: 0;
		transition:
			color 140ms ease,
			background 140ms ease,
			border-color 140ms ease;
	}
	.collapse-btn:hover {
		color: var(--text, #e5e5e5);
		background: rgba(255, 255, 255, 0.06);
		border-color: var(--accent, #a3a3a3);
	}

	nav {
		display: flex;
		flex-direction: column;
		gap: 2px;
	}

	nav a {
		display: flex;
		align-items: center;
		gap: 0.65rem;
		padding: 10px 18px;
		text-decoration: none;
		color: var(--text-dim);
		font-size: 14px;
		border-left: 2px solid transparent;
		position: relative;
		transition:
			color var(--dk-dur-fast, 140ms) var(--dk-ease, ease),
			background var(--dk-dur-fast, 140ms) var(--dk-ease, ease),
			border-color var(--dk-dur-fast, 140ms) ease;
	}

	.sidebar--collapsed nav a {
		justify-content: center;
		padding: 12px 0;
		border-left-width: 0;
		border-right: 2px solid transparent;
	}

	nav a:hover {
		color: var(--accent);
		background: color-mix(in srgb, var(--accent) 10%, transparent);
	}

	nav a.active,
	nav a:global([aria-current='page']) {
		color: var(--accent);
		background: color-mix(in srgb, var(--accent) 12%, transparent);
		border-left-color: var(--accent);
		font-weight: 600;
	}

	.sidebar--collapsed nav a.active,
	.sidebar--collapsed nav a:global([aria-current='page']) {
		border-left-color: transparent;
		border-right-color: var(--accent);
	}

	.nav-icon {
		flex-shrink: 0;
		width: 1.15rem;
		text-align: center;
		font-size: 0.95rem;
		opacity: 0.9;
	}

	.nav-label {
		flex: 1;
		min-width: 0;
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}

	.nav-badge-link {
		/* flex already on a */
	}

	.nav-badge {
		font-size: 0.65rem;
		font-weight: 700;
		min-width: 1.15rem;
		height: 1.15rem;
		display: inline-flex;
		align-items: center;
		justify-content: center;
		border-radius: 999px;
		background: var(--accent);
		color: #0f0f0f;
		padding: 0 0.3rem;
		line-height: 1;
	}

	.nav-badge--dot {
		position: absolute;
		top: 8px;
		right: 8px;
		min-width: 7px;
		width: 7px;
		height: 7px;
		padding: 0;
		background: var(--dk-text, #e5e5e5);
		border: 1px solid var(--dk-bg, #0f0f0f);
	}
</style>
