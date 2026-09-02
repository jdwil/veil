/**
 * History / audit surface — client for the audit-logging query endpoints
 * (veil-server session_api.rs Part 3):
 *   GET /api/history/recent
 *   GET /api/history/actions
 *   GET /api/history/bundles/{id}
 *   GET /api/history/blob?ref=
 *   GET /api/sessions/{id}/turns   (full-fidelity agent turns)
 *
 * Also provides faithful, paste-ready text formatting of a conversation for the
 * "Copy conversation" / "Copy turn" convenience (paste into another agent).
 */

export type ToolCall = {
	name: string;
	tool_call_id?: string;
	kind?: string | null;
	status?: string | null;
	order?: number;
	started_at?: string;
	input?: unknown;
	output?: unknown;
	content?: string;
	content_ref?: string;
	content_preview?: string;
	content_bytes?: number;
	content_truncated?: boolean;
	fidelity?: 'full' | 'name_only';
	detail?: string;
};

export type Turn = {
	turn_id: string;
	role: string;
	content: string;
	tool_calls: ToolCall[];
	project?: string | null;
	active_file?: string | null;
	ts: string;
	backend?: string | null;
};

export type ReviewAction = {
	id: string;
	at: string;
	actor: string;
	actor_kind: string;
	action: string;
	bundle_id?: string | null;
	slugs: string[];
	environment?: string | null;
	git_shas: string[];
	pr_ids: string[];
	result: string;
	note?: string | null;
};

export type RecentFeed = {
	ok: boolean;
	user_id: string;
	sessions: Array<{
		kind: 'session';
		session_id: string;
		slug: string;
		branch_name?: string | null;
		at: string;
		user_id: string;
	}>;
	actions: Array<
		ReviewAction & { kind: 'review_action' }
	>;
	bundles: Array<{
		id: string;
		title: string;
		summary: string;
		project_slugs: string[];
		outstanding: number;
		created_at: string;
	}>;
};

export type BundleDetail = {
	ok: boolean;
	bundle_id: string;
	bundle: unknown;
	actions: ReviewAction[];
	sessions: Array<{
		session_id: string;
		slug: string;
		turn_count: number;
		turns: Turn[];
	}>;
};

async function getJson<T>(url: string): Promise<T> {
	const r = await fetch(url);
	if (!r.ok) throw new Error(`${r.status} ${await r.text()}`);
	return (await r.json()) as T;
}

export function fetchRecent(params: {
	slug?: string;
	actor?: string;
	action?: string;
	limit?: number;
} = {}): Promise<RecentFeed> {
	const q = new URLSearchParams();
	if (params.slug) q.set('slug', params.slug);
	if (params.actor) q.set('actor', params.actor);
	if (params.action) q.set('action', params.action);
	if (params.limit) q.set('limit', String(params.limit));
	const qs = q.toString();
	return getJson<RecentFeed>(`/api/history/recent${qs ? `?${qs}` : ''}`);
}

export function fetchActions(params: {
	bundle?: string;
	slug?: string;
	actor?: string;
	action?: string;
	limit?: number;
} = {}): Promise<{ ok: boolean; count: number; actions: ReviewAction[] }> {
	const q = new URLSearchParams();
	if (params.bundle) q.set('bundle', params.bundle);
	if (params.slug) q.set('slug', params.slug);
	if (params.actor) q.set('actor', params.actor);
	if (params.action) q.set('action', params.action);
	if (params.limit) q.set('limit', String(params.limit));
	const qs = q.toString();
	return getJson(`/api/history/actions${qs ? `?${qs}` : ''}`);
}

export function fetchBundleDetail(id: string): Promise<BundleDetail> {
	return getJson<BundleDetail>(`/api/history/bundles/${encodeURIComponent(id)}`);
}

export function fetchSessionTurns(
	sessionId: string,
): Promise<{ session_id: string; turns: Turn[] }> {
	return getJson(`/api/sessions/${encodeURIComponent(sessionId)}/turns`);
}

export function fetchBlob(ref: string): Promise<{ ok: boolean; ref: string; content: string }> {
	return getJson(`/api/history/blob?ref=${encodeURIComponent(ref)}`);
}

// ─── Faithful text rendering for clipboard (Copy conversation / Copy turn) ──

function stringifyValue(v: unknown): string {
	if (v == null) return '';
	if (typeof v === 'string') return v;
	try {
		return JSON.stringify(v, null, 2);
	} catch {
		return String(v);
	}
}

/** Render ONE tool call as a clean markdown block (name + args + result). */
export function toolCallToMarkdown(tc: ToolCall): string {
	const lines: string[] = [];
	const status = tc.status ? ` (${tc.status})` : '';
	lines.push(`- **tool: ${tc.name}**${status}`);
	const input = stringifyValue(tc.input);
	if (input) {
		lines.push('  - args:');
		lines.push('    ```json');
		for (const l of input.split('\n')) lines.push(`    ${l}`);
		lines.push('    ```');
	}
	const result =
		tc.content ||
		(tc.content_ref
			? `[large result offloaded → ${tc.content_ref}${
					tc.content_preview ? `]\n${tc.content_preview}` : ']'
			  }`
			: stringifyValue(tc.output));
	if (result) {
		lines.push('  - result:');
		lines.push('    ```');
		for (const l of result.split('\n')) lines.push(`    ${l}`);
		lines.push('    ```');
	}
	return lines.join('\n');
}

/** Render ONE turn (role, text, ordered tool calls) as paste-ready markdown. */
export function turnToMarkdown(turn: Turn): string {
	const lines: string[] = [];
	const who = turn.role === 'user' ? 'User' : turn.role === 'assistant' ? 'Assistant' : turn.role;
	const stamp = turn.ts ? ` — ${turn.ts}` : '';
	lines.push(`### ${who}${stamp}`);
	if (turn.content?.trim()) {
		lines.push('');
		lines.push(turn.content.trim());
	}
	const tools = [...(turn.tool_calls ?? [])].sort(
		(a, b) => (a.order ?? 0) - (b.order ?? 0),
	);
	if (tools.length) {
		lines.push('');
		lines.push('**Tool calls:**');
		for (const tc of tools) lines.push(toolCallToMarkdown(tc));
	}
	return lines.join('\n');
}

/** Render a full conversation (ordered turns) as paste-ready markdown. */
export function conversationToMarkdown(
	turns: Turn[],
	header?: { title?: string; sessionId?: string; slug?: string },
): string {
	const lines: string[] = [];
	if (header?.title) lines.push(`# ${header.title}`);
	const meta: string[] = [];
	if (header?.slug) meta.push(`project: ${header.slug}`);
	if (header?.sessionId) meta.push(`session: ${header.sessionId}`);
	if (meta.length) lines.push(`_${meta.join(' · ')}_`);
	if (lines.length) lines.push('');
	const ordered = [...turns].sort((a, b) => a.turn_id.localeCompare(b.turn_id));
	for (const t of ordered) {
		lines.push(turnToMarkdown(t));
		lines.push('');
		lines.push('---');
		lines.push('');
	}
	return lines.join('\n').trim();
}

/** Copy text to the clipboard; returns true on success. */
export async function copyToClipboard(text: string): Promise<boolean> {
	try {
		await navigator.clipboard.writeText(text);
		return true;
	} catch {
		// Fallback: hidden textarea + execCommand for non-secure contexts.
		try {
			const ta = document.createElement('textarea');
			ta.value = text;
			ta.style.position = 'fixed';
			ta.style.opacity = '0';
			document.body.appendChild(ta);
			ta.select();
			const ok = document.execCommand('copy');
			document.body.removeChild(ta);
			return ok;
		} catch {
			return false;
		}
	}
}
