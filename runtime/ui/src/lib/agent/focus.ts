/**
 * SessionFocus — continuous "what are we looking at?" state.
 *
 * UX is source of truth for viewport/focus. Published to the agent every turn
 * and optionally persisted server-side by session id.
 *
 * See runtime/docs/ADR_FOCUS_INTENT_PRESENT.md
 */
import { writable, get } from 'svelte/store';

export type FocusSelection = {
	kind: string;
	id: string;
	label?: string;
};

export type FocusForm = {
	id: string;
	values?: Record<string, unknown>;
	dirty?: boolean;
};

export type FocusDiagnostics = {
	count: number;
	sample?: Array<{
		severity?: string;
		message?: string;
		node_name?: string | null;
		code?: string;
		hint?: string | null;
	}>;
};

/** Continuous snapshot of human attention in the shell + IDE. */
export type SessionFocus = {
	route: string;
	project: string | null;
	changeId?: string | null;
	file?: string | null;
	construct?: string | null;
	constructKind?: string | null;
	selection?: FocusSelection | null;
	panel?: string | null;
	form?: FocusForm | null;
	diagnostics?: FocusDiagnostics | null;
	revision?: number | null;
	/** AgentSurface contracts on the current page (optional). */
	surfaces?: unknown[];
	updatedAt: number;
};

const emptyFocus = (): SessionFocus => ({
	route: '/',
	project: null,
	updatedAt: Date.now()
});

export const sessionFocus = writable<SessionFocus>(emptyFocus());

/** Listeners notified after every patch (e.g. layout → server sync). */
type FocusListener = (focus: SessionFocus) => void;
const listeners: FocusListener[] = [];

export function onFocusChange(listener: FocusListener): () => void {
	listeners.push(listener);
	return () => {
		const i = listeners.indexOf(listener);
		if (i >= 0) listeners.splice(i, 1);
	};
}

function notify(focus: SessionFocus) {
	for (const l of listeners) {
		try {
			l(focus);
		} catch {
			/* ignore */
		}
	}
}

/** Merge a partial focus patch. Sets updatedAt. */
export function patchFocus(partial: Partial<SessionFocus>): SessionFocus {
	let next: SessionFocus = emptyFocus();
	sessionFocus.update((prev) => {
		next = {
			...prev,
			...partial,
			updatedAt: Date.now()
		};
		// Explicit nulls clear optional fields when provided
		if ('construct' in partial && partial.construct == null) {
			next.construct = null;
			next.constructKind = partial.constructKind ?? null;
		}
		if ('selection' in partial && partial.selection == null) {
			next.selection = null;
		}
		if ('file' in partial && partial.file == null) {
			next.file = null;
		}
		return next;
	});
	notify(next);
	return next;
}

export function getFocus(): SessionFocus {
	return get(sessionFocus);
}

/** JSON-safe payload for ChatRequest.focus / server store. */
export function focusPayload(focus?: SessionFocus): Record<string, unknown> {
	const f = focus ?? getFocus();
	const out: Record<string, unknown> = {
		route: f.route,
		project: f.project,
		updatedAt: f.updatedAt
	};
	if (f.changeId) out.changeId = f.changeId;
	if (f.file) out.file = f.file;
	if (f.construct) out.construct = f.construct;
	if (f.constructKind) out.constructKind = f.constructKind;
	if (f.selection) out.selection = f.selection;
	if (f.panel) out.panel = f.panel;
	if (f.form) out.form = f.form;
	if (f.diagnostics) out.diagnostics = f.diagnostics;
	if (f.revision != null) out.revision = f.revision;
	if (f.surfaces && f.surfaces.length) {
		out.surfaces = f.surfaces.slice(0, 12);
	}
	return out;
}

/** Human-readable block for system prompt / preamble. */
export function formatFocusForAgent(focus?: SessionFocus): string {
	const f = focus ?? getFocus();
	const lines = [
		'## Session focus (authoritative — use for "this" / "here")',
		`- Route: ${f.route}`,
		`- Project: ${f.project || '(none)'}`
	];
	if (f.file) lines.push(`- Active file: ${f.file}`);
	if (f.construct) {
		const k = f.constructKind ? ` (${f.constructKind})` : '';
		lines.push(`- Construct / component in view: \`${f.construct}\`${k}`);
		lines.push(
			'  When the user says "this component", "this construct", or "this node", they mean the above.'
		);
	}
	if (f.selection) {
		lines.push(
			`- Selection: ${f.selection.kind} id=${f.selection.id}` +
				(f.selection.label ? ` label=${f.selection.label}` : '')
		);
	}
	if (f.changeId) lines.push(`- Change request: ${f.changeId}`);
	if (f.panel) lines.push(`- Panel: ${f.panel}`);
	if (f.form?.id) {
		lines.push(`- Form: ${f.form.id}` + (f.form.dirty ? ' (dirty)' : ''));
	}
	if (f.diagnostics && f.diagnostics.count > 0) {
		lines.push(`- Open diagnostics: ${f.diagnostics.count}`);
		const sample = f.diagnostics.sample?.slice(0, 3) ?? [];
		for (const d of sample) {
			const name = d.node_name ? ` @ ${d.node_name}` : '';
			lines.push(`  - ${d.severity ?? 'Issue'}${name}: ${d.message ?? ''}`);
		}
	}
	if (f.revision != null) lines.push(`- Coding revision: ${f.revision}`);
	return lines.join('\n');
}

/**
 * Derive project slug from a SPA path when possible.
 * /projects/foo, /projects/foo/ide → foo
 */
export function projectFromRoute(route: string): string | null {
	const m = route.match(/^\/projects\/([^/]+)/);
	if (!m) return null;
	const id = decodeURIComponent(m[1]);
	if (id === 'new') return null;
	return id;
}

/**
 * Publish route-level focus (call from layout on navigation).
 * Preserves IDE construct/file when staying on the same project IDE route.
 */
export function publishRouteFocus(pathname: string): SessionFocus {
	const route = pathname || '/';
	const project = projectFromRoute(route);
	const prev = getFocus();
	const sameProjectIde =
		project &&
		prev.project === project &&
		/\/projects\/[^/]+\/ide\/?$/.test(route) &&
		/\/projects\/[^/]+\/ide\/?$/.test(prev.route);

	const partial: Partial<SessionFocus> = {
		route,
		project
	};

	if (!sameProjectIde) {
		// Leaving IDE (or switching project) clears construct focus
		if (!/\/ide\/?$/.test(route) || prev.project !== project) {
			partial.construct = null;
			partial.constructKind = null;
			partial.file = null;
			partial.selection = null;
		}
	}

	// Form focus on create project
	if (route === '/projects/new' || route.endsWith('/projectcreate')) {
		partial.form = { id: 'create-project' };
	} else if (prev.form?.id === 'create-project') {
		partial.form = null;
	}

	// Surfaces from designkit collector
	if (typeof window !== 'undefined') {
		const w = window as unknown as { __veilAgentSurface?: { surfaces?: unknown[] } };
		if (w.__veilAgentSurface?.surfaces) {
			partial.surfaces = w.__veilAgentSurface.surfaces;
		}
	}

	return patchFocus(partial);
}
