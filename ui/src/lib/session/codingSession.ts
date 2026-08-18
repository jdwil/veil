/**
 * Durable coding-session ids, keyed by product slug.
 *
 * One global `veil.coding.sessionId` used to leak agent-core into every
 * `/projects/{other}/ide` request (HTTP 400 session slug mismatch).
 */

const GLOBAL_KEY = 'veil.coding.sessionId';
const BY_SLUG_KEY = 'veil.coding.sessionBySlug';

function readMap(): Record<string, string> {
	if (typeof localStorage === 'undefined') return {};
	try {
		const raw = localStorage.getItem(BY_SLUG_KEY);
		if (!raw) return {};
		const v = JSON.parse(raw) as unknown;
		if (!v || typeof v !== 'object' || Array.isArray(v)) return {};
		const out: Record<string, string> = {};
		for (const [k, val] of Object.entries(v as Record<string, unknown>)) {
			if (typeof val === 'string' && val) out[k] = val;
		}
		return out;
	} catch {
		return {};
	}
}

function writeMap(map: Record<string, string>) {
	if (typeof localStorage === 'undefined') return;
	try {
		localStorage.setItem(BY_SLUG_KEY, JSON.stringify(map));
	} catch {
		/* ignore */
	}
}

/**
 * Session id for `slug` (exact map hit only — no global fallback).
 * Omit `slug` to read the last-used global id (agent dock / compat).
 */
export function getCodingSessionId(slug?: string | null): string | null {
	if (typeof localStorage === 'undefined') return null;
	try {
		if (slug) return readMap()[slug] || null;
		return localStorage.getItem(GLOBAL_KEY);
	} catch {
		return null;
	}
}

/** Persist a session id. Pass `slug` so product switches do not clobber each other. */
export function setCodingSessionId(id: string | null, slug?: string | null) {
	if (typeof localStorage === 'undefined') return;
	try {
		if (slug) {
			const map = readMap();
			if (id) map[slug] = id;
			else delete map[slug];
			writeMap(map);
		}
		if (id) localStorage.setItem(GLOBAL_KEY, id);
		else if (!slug) localStorage.removeItem(GLOBAL_KEY);
	} catch {
		/* ignore */
	}
}

export function sessionIdBelongsToProject(
	sessionSlug: string | undefined | null,
	sessionRepoId: string | undefined | null,
	project: string
): boolean {
	if (!project) return false;
	return sessionSlug === project || sessionRepoId === project;
}
