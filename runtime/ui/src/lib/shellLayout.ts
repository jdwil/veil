/**
 * Shell chrome preferences (sidebar collapse, global theme).
 * Theme applies to the entire product UX (sidebar, agent, native IDE).
 */
import { writable, get } from 'svelte/store';

const SIDEBAR_KEY = 'veil.shell.sidebarCollapsed';
const THEME_KEY = 'veil-theme';

function loadCollapsed(): boolean {
	if (typeof localStorage === 'undefined') return false;
	return localStorage.getItem(SIDEBAR_KEY) === '1';
}

export const sidebarCollapsed = writable(loadCollapsed());

export function setSidebarCollapsed(collapsed: boolean) {
	sidebarCollapsed.set(collapsed);
	try {
		localStorage.setItem(SIDEBAR_KEY, collapsed ? '1' : '0');
	} catch {
		/* ignore */
	}
}

export function toggleSidebarCollapsed() {
	setSidebarCollapsed(!get(sidebarCollapsed));
}

export type ShellTheme = 'dark' | 'light';

function readTheme(): ShellTheme {
	if (typeof localStorage === 'undefined') return 'dark';
	const t = localStorage.getItem(THEME_KEY);
	return t === 'light' ? 'light' : 'dark';
}

export const shellTheme = writable<ShellTheme>(readTheme());

/** Apply theme to document (shell + native IDE inherit). */
export function applyShellTheme(theme: ShellTheme) {
	if (typeof document === 'undefined') return;
	document.documentElement.setAttribute('data-theme', theme);
	document.documentElement.classList.toggle('dark', theme === 'dark');
	try {
		localStorage.setItem(THEME_KEY, theme);
	} catch {
		/* ignore */
	}
	shellTheme.set(theme);
}

export function toggleShellTheme() {
	const next: ShellTheme = get(shellTheme) === 'dark' ? 'light' : 'dark';
	applyShellTheme(next);
}

export function initShellTheme() {
	applyShellTheme(readTheme());
}
