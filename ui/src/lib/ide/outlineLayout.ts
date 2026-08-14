/**
 * Outline pane layout prefs (tree sidebar width + expand/collapse).
 * localStorage only — no remote sync.
 *
 * Collapse keys are path strings (e.g. `RelayContext:Relay/g:domain`), scoped
 * per project so we never race empty-file → `main.veil` and wipe state.
 */
import { currentProjectParam } from './store';

export const OUTLINE_WIDTH_KEY = 'veil.outline.sidebarWidth';
export const OUTLINE_MIN = 200;
export const OUTLINE_MAX = 720;
export const OUTLINE_DEFAULT = 320;

const COLLAPSE_PREFIX = 'veil.outline.collapsed:';

export function clampOutlineWidth(px: number): number {
	if (!Number.isFinite(px)) return OUTLINE_DEFAULT;
	return Math.min(OUTLINE_MAX, Math.max(OUTLINE_MIN, Math.round(px)));
}

export function loadOutlineWidth(): number {
	if (typeof localStorage === 'undefined') return OUTLINE_DEFAULT;
	try {
		const n = Number(localStorage.getItem(OUTLINE_WIDTH_KEY));
		if (!Number.isFinite(n) || n <= 0) return OUTLINE_DEFAULT;
		return clampOutlineWidth(n);
	} catch {
		return OUTLINE_DEFAULT;
	}
}

export function saveOutlineWidth(px: number): void {
	if (typeof localStorage === 'undefined') return;
	try {
		localStorage.setItem(OUTLINE_WIDTH_KEY, String(clampOutlineWidth(px)));
	} catch {
		/* quota / private mode */
	}
}

/**
 * Scope collapse state per project (stable for the whole IDE session).
 * Do not include file name — active file starts empty then becomes `main.veil`,
 * which previously wiped just-saved collapse keys.
 */
export function outlineCollapseScope(_fileName?: string | null): string {
	const project = currentProjectParam() ?? 'default';
	return `${COLLAPSE_PREFIX}${project}`;
}

export function loadCollapsedKeys(scope: string): Set<string> {
	if (typeof localStorage === 'undefined') return new Set();
	try {
		const raw = localStorage.getItem(scope);
		if (!raw) return new Set();
		const arr = JSON.parse(raw) as unknown;
		if (!Array.isArray(arr)) return new Set();
		return new Set(arr.filter((x): x is string => typeof x === 'string' && x.length > 0));
	} catch {
		return new Set();
	}
}

export function saveCollapsedKeys(scope: string, keys: Set<string>): void {
	if (typeof localStorage === 'undefined') return;
	try {
		// Always write (including empty array) so "expand all" is sticky.
		localStorage.setItem(scope, JSON.stringify([...keys].sort()));
	} catch {
		/* ignore */
	}
}
