/**
 * Shell chrome preferences (sidebar collapse, etc.).
 * Persisted so IDE embed and reloads keep the user's layout.
 */
import { writable, get } from 'svelte/store';

const SIDEBAR_KEY = 'veil.shell.sidebarCollapsed';

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
