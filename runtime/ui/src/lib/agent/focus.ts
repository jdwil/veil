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

/** One visible IDE / shell pane for multi-pane deictic focus. */
export type FocusPane = {
	id: string;
	label: string;
	/** What the human sees in this pane right now. */
	summary: string;
	/** Last-interacted / primary for "this" when set. */
	primary?: boolean;
	/** Structured extras (wizard step, signature, tab, …). */
	details?: Record<string, string | number | boolean | null | undefined>;
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
	/**
	 * All major panes currently visible (outline, canvas, PR wizard, dock, …).
	 * Agent should use these for "this", "the method", "in the wizard", etc.
	 */
	panes?: FocusPane[];
	/** Id of the primary pane (matches panes[].id). */
	primaryPane?: string | null;
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
		if ('changeId' in partial && partial.changeId == null) {
			next.changeId = null;
		}
		if ('panel' in partial && partial.panel == null) {
			next.panel = null;
		}
		if ('primaryPane' in partial && partial.primaryPane == null) {
			next.primaryPane = null;
		}
		if ('panes' in partial && (partial.panes == null || partial.panes.length === 0)) {
			next.panes = partial.panes ?? [];
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
	if (f.panes && f.panes.length) {
		out.panes = f.panes.slice(0, 16);
	}
	if (f.primaryPane) out.primaryPane = f.primaryPane;
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
			'  When the user says "this component", "this construct", "this method", or "this node", they mean the above (or the primary pane below).'
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
	if (f.primaryPane) lines.push(`- Primary pane: ${f.primaryPane}`);
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

	// Multi-pane IDE / shell — full picture for casual conversation
	if (f.panes && f.panes.length > 0) {
		lines.push('');
		lines.push('### Visible panes (what the operator is looking at)');
		lines.push(
			'Resolve deictic language against these panes. Prefer the pane marked ★ primary.'
		);
		for (const p of f.panes) {
			const star = p.primary ? '★ ' : '  ';
			lines.push(`${star}**${p.label}** (\`${p.id}\`): ${p.summary}`);
			if (p.details) {
				const bits: string[] = [];
				for (const [k, val] of Object.entries(p.details)) {
					if (val == null || val === '') continue;
					// Keep prompt compact — skip long nulls
					const s = String(val);
					if (s.length > 220) bits.push(`${k}=${s.slice(0, 200)}…`);
					else bits.push(`${k}=${s}`);
				}
				if (bits.length) lines.push(`     ${bits.join(' · ')}`);
			}
		}
		// Explicit PR Wizard deictic help
		const wiz = f.panes.find((p) => p.id === 'pr-wizard' && p.primary);
		if (wiz?.details?.itemName) {
			lines.push('');
			lines.push(
				`Operator is reviewing **\`${wiz.details.itemName}\`** in the PR Wizard` +
					(wiz.details.itemKind ? ` (${wiz.details.itemKind})` : '') +
					'. Questions about "this", "why", "the signature", or "this change" refer to that wizard step unless they name something else.'
			);
			if (wiz.details.rationale) {
				lines.push(`Agent rationale on this step: ${wiz.details.rationale}`);
			}
			if (wiz.details.signature) {
				lines.push(`Signature: ${wiz.details.signature}`);
			}
		}
	}

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
