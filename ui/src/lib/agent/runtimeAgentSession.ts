/**
 * Shared agent session for the entire veil-runtime.
 *
 * This is the ONE session — it lives above all routes and survives navigation.
 * The IDE viewer (embedded iframe) connects to this same session via postMessage,
 * so there is no separate "IDE agent" vs "runtime agent". One agent, everywhere.
 *
 * Port of veil-viewer/src/lib/agentSession.ts, elevated to runtime scope with:
 * - sessionStorage persistence (survive page reload)
 * - Context injection (page, project, surfaces)
 * - Navigation event handling (agent can navigate the UI)
 * - IDE bridge (bidirectional postMessage with embedded viewer)
 */
import { writable, get } from 'svelte/store';
import {
	StreamService,
	type Message,
	type StreamEvent,
	type TextContent,
	type ChatRequest
} from '@aether-ui/core';
import {
	formatFocusForAgent,
	focusPayload,
	getFocus,
	patchFocus,
	type SessionFocus
} from './focus';
import {
	executeIntent,
	intentFromToolResult,
	STAGED_PRESENT_TOOLS,
	formatIntentLogForAgent,
	mergeIntentLogFromServer,
	type Intent
} from './intent';
import {
	mergeFiles,
	prepareAttachments,
	type WireAttachment
} from './attachments';
import { getCodingSessionId, setCodingSessionId } from '$lib/session/codingSession';

export { getCodingSessionId, setCodingSessionId };

// ─── State Stores ───────────────────────────────────────────────────────────

export const agentMessages = writable<Message[]>([]);
export const agentIsStreaming = writable(false);
export const agentIsThinking = writable(false);
export const agentError = writable<string | null>(null);
export const agentStatusLine = writable('');
export const agentComposerKey = writable(0);
export const agentPendingSeed = writable('');
export const agentPanelOpen = writable(false);
/** Collapsed to a thin strip (session stays warm; Cmd+K still toggles open). */
export const agentPanelMinimized = writable(false);
export const agentUnreadCount = writable(0);
/** Files dropped on the dock (outside ChatInput) waiting for Send. */
export const agentPendingAttachments = writable<File[]>([]);

const previewObjectUrls: string[] = [];

export function agentAddAttachments(files: File[]) {
	if (!files.length) return;
	agentPendingAttachments.update((prev) => mergeFiles(prev, files));
	agentPanelMinimized.set(false);
	agentPanelOpen.set(true);
}

export function agentRemovePendingAttachment(index: number) {
	agentPendingAttachments.update((prev) => prev.filter((_, i) => i !== index));
}

export function agentClearPendingAttachments() {
	agentPendingAttachments.set([]);
}

/** Open the agent dock expanded (un-minimize). Used when agent work needs attention. */
export function openAgentPanel() {
	agentPanelOpen.set(true);
	agentPanelMinimized.set(false);
	agentUnreadCount.set(0);
}

// ─── Context (injected per turn) — thin adapter over SessionFocus ───────────

export interface AgentContext {
	page: string;
	project: string | null;
	surfaces: unknown[];
}

/** @deprecated Prefer patchFocus / getFocus — kept for call sites. */
export function setAgentContext(ctx: Partial<AgentContext>) {
	const partial: Partial<SessionFocus> = {};
	if (ctx.page != null) partial.route = ctx.page;
	if (ctx.project !== undefined) partial.project = ctx.project;
	if (ctx.surfaces) partial.surfaces = ctx.surfaces;
	patchFocus(partial);
}

export function getAgentContext(): AgentContext {
	const f = getFocus();
	return {
		page: f.route,
		project: f.project,
		surfaces: f.surfaces ?? []
	};
}

// ─── Navigation Events ──────────────────────────────────────────────────────

export type NavigationAction = {
	action: 'goto' | 'open-ide' | 'switch-project' | 'open-panel';
	path?: string;
	project?: string;
};

/** Listeners for agent-initiated navigation (handled by root layout) */
const navigationListeners: Array<(nav: NavigationAction) => void> = [];

export function onAgentNavigation(listener: (nav: NavigationAction) => void): () => void {
	navigationListeners.push(listener);
	return () => {
		const idx = navigationListeners.indexOf(listener);
		if (idx >= 0) navigationListeners.splice(idx, 1);
	};
}

function emitNavigation(nav: NavigationAction) {
	for (const listener of navigationListeners) {
		listener(nav);
	}
}

/** Fallback nav when a tool result has no Intent+Present. Prefer present over this map. */
const TOOL_NAV: Record<string, NavigationAction> = {
	list_prs: { action: 'goto', path: '/review' },
	open_prs: { action: 'goto', path: '/review' },
	create_pr: { action: 'goto', path: '/review' },
	open_create_pr: { action: 'goto', path: '/review' },
	list_projects: { action: 'goto', path: '/projects' },
	open_projects: { action: 'goto', path: '/projects' },
	// create_project: prefer structured navigation from tool output (project/ide path)
	create_project: { action: 'goto', path: '/projects' },
	create_repo: { action: 'goto', path: '/projects' },
	rename_project: { action: 'goto', path: '/projects' },
	update_project: { action: 'goto', path: '/projects' },
	delete_project: { action: 'goto', path: '/projects' },
	open_deploy: { action: 'goto', path: '/deploy' },
	list_deploy_environments: { action: 'goto', path: '/deploy' },
	deploy_status: { action: 'goto', path: '/deploy' },
	plan_provision: { action: 'goto', path: '/deploy' },
	provision_project: { action: 'goto', path: '/deploy' },
	open_registry: { action: 'goto', path: '/registry' },
	search_registry: { action: 'goto', path: '/registry' },
	list_registry_layers: { action: 'goto', path: '/registry' },
	list_registry_stubs: { action: 'goto', path: '/registry' },
	open_dashboard: { action: 'goto', path: '/dashboard' },
	open_config: { action: 'goto', path: '/config' },
	get_config: { action: 'goto', path: '/config' },
	navigate_to: { action: 'goto', path: '/dashboard' }
};

/** Tools that mean "I'm working on product source" — operator should be in the IDE. */
const CODING_TOOLS = new Set([
	'write_source',
	'create_file',
	'select_file',
	'rename_construct',
	'ws_write',
	'ws_str_replace',
	'create_branch',
	'session_commit',
	'merge_branch',
	'veil_check',
	'veil_outline',
	'read_source',
	'list_files'
]);

/** Tools that mutate source (should refresh IDE + end on IDE). */
const CODE_EDIT_TOOLS = new Set([
	'write_source',
	'create_file',
	'rename_construct',
	'ws_write',
	'ws_str_replace',
	'session_commit',
	'merge_branch'
]);

/** Project slug we're coding on this turn (from create_project / tools / focus). */
let turnCodingProject: string | null = null;
/** True if this turn applied a source mutation. */
let turnDidCodeEdit = false;

function ideNav(project: string): NavigationAction {
	const p = project.trim();
	return {
		action: 'open-ide',
		path: `/projects/${encodeURIComponent(p)}/ide`,
		project: p
	};
}

function projectFromUnknown(output?: unknown, argsJson?: string): string | null {
	const tryObj = (o: Record<string, unknown>): string | null => {
		const direct = o.project ?? o.slug ?? o.name;
		if (typeof direct === 'string' && direct.trim() && !direct.includes(' ')) {
			// bare slug
			return direct.trim();
		}
		if (typeof direct === 'string' && direct.trim()) {
			// display name → leave as-is; routes accept slug mostly
			return direct.trim().toLowerCase().replace(/\s+/g, '-').replace(/_/g, '-');
		}
		const proj = o.project;
		if (proj && typeof proj === 'object') {
			const pr = proj as Record<string, unknown>;
			const s = pr.slug ?? pr.name;
			if (typeof s === 'string' && s.trim()) return String(s).trim();
		}
		const nav = o.navigation;
		if (nav && typeof nav === 'object') {
			const n = nav as Record<string, unknown>;
			if (typeof n.project === 'string' && n.project.trim()) return n.project.trim();
			if (typeof n.path === 'string') {
				const m = n.path.match(/\/projects\/([^/]+)/);
				if (m?.[1]) return decodeURIComponent(m[1]);
			}
		}
		return null;
	};
	if (output && typeof output === 'object') {
		const hit = tryObj(output as Record<string, unknown>);
		if (hit) return hit;
	}
	if (typeof output === 'string') {
		try {
			const parsed = JSON.parse(output) as Record<string, unknown>;
			const hit = tryObj(parsed);
			if (hit) return hit;
		} catch {
			/* ignore */
		}
	}
	if (argsJson) {
		try {
			const raw = JSON.parse(argsJson) as Record<string, unknown>;
			const detail = (raw.detail as Record<string, unknown>) || raw;
			const hit = tryObj(detail);
			if (hit) return hit;
		} catch {
			/* ignore */
		}
	}
	return getFocus().project ?? null;
}

function rememberCodingProject(project: string | null) {
	if (project?.trim()) turnCodingProject = project.trim();
}

/** Debounce IDE reloads so multi-write turns don't thrash IR/layout mid-stream. */
let ideRefreshTimer: ReturnType<typeof setTimeout> | null = null;
function refreshIdeAfterEdit() {
	if (typeof window === 'undefined') {
		sendToIde({ type: 'agent:refresh', payload: { reason: 'agent-write' } });
		return;
	}
	if (ideRefreshTimer != null) clearTimeout(ideRefreshTimer);
	ideRefreshTimer = setTimeout(() => {
		ideRefreshTimer = null;
		// iframe embed (legacy viewer)
		sendToIde({ type: 'agent:refresh', payload: { reason: 'agent-write' } });
		// same-window custom event for in-shell IdeApp
		window.dispatchEvent(
			new CustomEvent('veil:agent-refresh', { detail: { reason: 'agent-write' } })
		);
	}, 500);
}

function navigationFromTool(name: string, output?: unknown, argsJson?: string): NavigationAction | null {
	// Structured navigation in tool output (preferred)
	if (output && typeof output === 'object') {
		const o = output as Record<string, unknown>;
		if (o.navigation && typeof o.navigation === 'object') {
			return o.navigation as NavigationAction;
		}
		// Output may be a JSON string nested under detail
		if (typeof o.detail === 'string') {
			try {
				const parsed = JSON.parse(o.detail) as Record<string, unknown>;
				if (parsed.navigation && typeof parsed.navigation === 'object') {
					return parsed.navigation as NavigationAction;
				}
			} catch {
				/* ignore */
			}
		}
	}
	if (typeof output === 'string') {
		try {
			const parsed = JSON.parse(output) as Record<string, unknown>;
			if (parsed.navigation && typeof parsed.navigation === 'object') {
				return parsed.navigation as NavigationAction;
			}
		} catch {
			/* ignore */
		}
	}
	// navigate_to / open_project need args
	let args: Record<string, unknown> = {};
	if (argsJson) {
		try {
			const raw = JSON.parse(argsJson) as Record<string, unknown>;
			args = (raw.detail as Record<string, unknown>) || raw;
		} catch {
			/* ignore */
		}
	}
	if (name === 'navigate_to') {
		const path = String(args.path || args.detail || '');
		if (path) {
			return { action: 'goto', path: path.startsWith('/') ? path : `/${path}` };
		}
	}
	if (name === 'open_project' || name === 'open_ide' || name === 'switch_project') {
		const project = String(args.project || args.slug || args.id || '');
		if (project) {
			const isIde = name === 'open_ide';
			return {
				action: isIde ? 'open-ide' : name === 'switch_project' ? 'switch-project' : 'goto',
				// IDE stays inside the shell at /projects/{id}/ide (iframe embed, no full redirect).
				path: isIde ? `/projects/${project}/ide` : `/projects/${project}`,
				project
			};
		}
		return { action: 'goto', path: '/projects' };
	}
	// Source/IDE work → open product IDE (not leave operator on registry/dashboard)
	if (CODING_TOOLS.has(name) || name === 'create_project' || name === 'create_repo') {
		const project =
			projectFromUnknown(output, argsJson) || turnCodingProject || getFocus().project;
		if (project) {
			return ideNav(project);
		}
	}
	return TOOL_NAV[name] ?? null;
}

// ─── Stream Service ─────────────────────────────────────────────────────────

const stream = new StreamService();
let currentMessageId: string | null = null;

/**
 * Coalesce content_delta frames into one store update per animation frame.
 * ACP (and the old char-typewriter path) can emit dozens of deltas per second;
 * each update re-runs marked + MessageList and freezes the dock so text looks
 * like it “dumps” at the end. rAF batching keeps live paint without thrash.
 */
const pendingDeltas = new Map<string, string>();
let deltaRaf: number | null = null;

function flushPendingDeltas() {
	deltaRaf = null;
	if (pendingDeltas.size === 0) return;
	const batch = new Map(pendingDeltas);
	pendingDeltas.clear();
	setMessages((prev) =>
		prev.map((m) => {
			const delta = batch.get(m.id);
			if (!delta) return m;
			const blocks = [...m.content];
			const last = blocks[blocks.length - 1];
			if (last && last.type === 'text') {
				blocks[blocks.length - 1] = { type: 'text', text: last.text + delta };
			} else {
				blocks.push({ type: 'text', text: delta });
			}
			return { ...m, content: blocks, status: 'streaming' };
		})
	);
}

function queueContentDelta(messageId: string, delta: string) {
	if (!delta) return;
	pendingDeltas.set(messageId, (pendingDeltas.get(messageId) ?? '') + delta);
	if (typeof requestAnimationFrame === 'undefined') {
		flushPendingDeltas();
		return;
	}
	if (deltaRaf == null) {
		deltaRaf = requestAnimationFrame(flushPendingDeltas);
	}
}

/** Flush before tool/done so text ordering stays correct relative to tools. */
function flushDeltasNow() {
	if (deltaRaf != null && typeof cancelAnimationFrame !== 'undefined') {
		cancelAnimationFrame(deltaRaf);
		deltaRaf = null;
	}
	flushPendingDeltas();
}

function chatWsUrl(): string {
	if (typeof window === 'undefined') return 'ws://127.0.0.1:3000/api/agent/chat';
	const proto = window.location.protocol === 'https:' ? 'wss:' : 'ws:';
	return `${proto}//${window.location.host}/api/agent/chat`;
}

function textOf(m: Message): string {
	const wire = m.metadata?.wireText;
	if (typeof wire === 'string' && wire.trim()) return wire;
	return m.content
		.filter((b): b is TextContent => b.type === 'text')
		.map((b) => b.text)
		.join('\n');
}

function setMessages(updater: (prev: Message[]) => Message[]) {
	agentMessages.update(updater);
}

// ─── Event Handling ─────────────────────────────────────────────────────────

function handleEvent(event: StreamEvent) {
	switch (event.event) {
		case 'message_start': {
			flushDeltasNow();
			pendingDeltas.clear();
			currentMessageId = event.data.messageId;
			const sid = (event.data as { sessionId?: string }).sessionId;
			if (sid) setCodingSessionId(sid);
			// Reset per-turn coding UX state (where to land after research detours)
			turnCodingProject = getFocus().project;
			turnDidCodeEdit = false;
			const msg: Message = {
				id: event.data.messageId,
				role: 'assistant',
				content: [],
				status: 'streaming',
				createdAt: new Date().toISOString(),
				model: event.data.model,
				provider: event.data.provider
			};
			setMessages((prev) => [...prev, msg]);
			// Increment unread if panel is closed or minimized
			if (!get(agentPanelOpen) || get(agentPanelMinimized)) {
				agentUnreadCount.update((n) => n + 1);
			}
			agentStatusLine.set('agent running…');
			break;
		}
		case 'status': {
			// Heartbeats during long write_source smoke / ACP tool rounds
			const data = event.data as { message?: string; heartbeat?: boolean };
			if (data.message) {
				agentStatusLine.set(data.message);
			}
			break;
		}
		case 'content_delta': {
			queueContentDelta(event.data.messageId, event.data.delta ?? '');
			break;
		}
		case 'tool_call_start': {
			flushDeltasNow();
			const id = event.data.messageId;
			const toolName = event.data.name as string;
			setMessages((prev) =>
				prev.map((m) => {
					if (m.id !== id) return m;
					const blocks = [...m.content];
					blocks.push({
						type: 'tool_call',
						toolCall: {
							id: event.data.callId,
							name: toolName,
							arguments: '',
							status: 'executing' as const
						}
					});
					return { ...m, content: blocks };
				})
			);
			// Early nav only for simple tools — staged Present tools wait for intent/result
			if (!STAGED_PRESENT_TOOLS.has(toolName)) {
				const navStart = navigationFromTool(toolName);
				if (navStart) emitNavigation(navStart);
			} else {
				agentStatusLine.set(`${toolName}…`);
			}
			break;
		}
		case 'tool_call_stop': {
			// Arguments often arrive here (path for navigate_to, project for open_ide)
			const data = event.data as {
				name?: string;
				arguments?: string;
				messageId?: string;
			};
			const toolName = data.name || '';
			if (!STAGED_PRESENT_TOOLS.has(toolName)) {
				const nav = navigationFromTool(toolName, undefined, data.arguments);
				if (nav) emitNavigation(nav);
			}
			break;
		}
		case 'tool_result': {
			flushDeltasNow();
			const id = event.data.messageId;
			const toolName = event.data.name as string;
			const output = event.data.output;
			setMessages((prev) =>
				prev.map((m) => {
					if (m.id !== id) return m;
					const blocks = [...m.content];
					blocks.push({
						type: 'tool_result',
						toolResult: {
							callId: event.data.callId,
							name: toolName,
							output,
							isError: event.data.isError
						}
					});
					return { ...m, content: blocks };
				})
			);
			// Git-shaped tools may switch the active coding session (branch/main)
			maybeApplyCodingSessionSwitch(toolName, output);
			if (
				!event.data.isError &&
				(CODE_EDIT_TOOLS.has(toolName) ||
					toolName === 'create_project' ||
					toolName === 'create_repo' ||
					toolName === 'create_pr' ||
					toolName === 'rename_project' ||
					toolName === 'sign_off' ||
					toolName === 'request_sign_off')
			) {
				void import('$lib/review/store').then((m) => m.refreshReview());
			}
			// Track coding project for end-of-turn landing
			const proj = projectFromUnknown(output);
			if (toolName === 'create_project' || toolName === 'create_repo' || CODING_TOOLS.has(toolName)) {
				rememberCodingProject(proj);
			}
			if (CODE_EDIT_TOOLS.has(toolName) && !event.data.isError) {
				turnDidCodeEdit = true;
				void import('$lib/review/store').then((m) => m.refreshReview());
				// Ensure IDE is open *before* refresh so SSE/reload is visible
				const p = turnCodingProject || proj || getFocus().project;
				if (p) {
					emitNavigation(ideNav(p));
					// Allow SPA route + iframe to settle, then force IR reload
					setTimeout(() => refreshIdeAfterEdit(), 350);
				} else {
					refreshIdeAfterEdit();
				}
				agentStatusLine.set(`${toolName} — source updated`);
			}
			// Intent + Present preferred over coarse TOOL_NAV
			const intent = intentFromToolResult(toolName, output);
			if (intent?.present?.steps?.length) {
				void runIntentPresent(intent);
			} else if (!CODE_EDIT_TOOLS.has(toolName)) {
				// Edit tools already navigated to IDE above
				const nav = navigationFromTool(toolName, output);
				if (nav) emitNavigation(nav);
			}
			break;
		}
		case 'error': {
			flushDeltasNow();
			agentError.set(event.data.message);
			agentStatusLine.set(event.data.message);
			break;
		}
		case 'content_stop': {
			flushDeltasNow();
			break;
		}
		case 'done': {
			flushDeltasNow();
			const id = event.data.messageId;
			setMessages((prev) =>
				prev.map((m) => (m.id === id ? { ...m, status: 'complete' as const } : m))
			);
			const data = event.data as Record<string, unknown>;
			if (data.contextWarning && typeof data.contextWarning === 'string') {
				agentStatusLine.set(data.contextWarning);
			} else if (data.backend && typeof data.backend === 'string') {
				agentStatusLine.set(data.backend);
			}
			const sourceChanged = data.sourceChanged === true || turnDidCodeEdit;
			// End-of-turn landing: if we edited (or server says source changed), do NOT leave
			// the operator on a research detour (/registry, etc.) — open the product IDE.
			if (sourceChanged) {
				const project =
					turnCodingProject ||
					projectFromUnknown(data) ||
					getFocus().project ||
					null;
				if (project) {
					emitNavigation(ideNav(project));
					setTimeout(() => refreshIdeAfterEdit(), 400);
					agentStatusLine.set(`IDE · ${project}`);
				} else if (data.navigation && typeof data.navigation === 'object') {
					emitNavigation(data.navigation as NavigationAction);
				}
			} else if (data.navigation && typeof data.navigation === 'object') {
				// Coarse navigation in done payload when no code edits
				emitNavigation(data.navigation as NavigationAction);
			}
			// Turn-completion → review prompt: surface a lightweight banner when
			// the turn left unreviewed work for the project. Non-blocking.
			if (data.needsReview === true && typeof data.reviewSlug === 'string') {
				const count =
					typeof data.reviewCount === 'number' ? (data.reviewCount as number) : 1;
				void import('$lib/review/store').then(({ setReviewPrompt }) => {
					setReviewPrompt(data.reviewSlug as string, count);
				});
			}
			// Persist after each complete turn
			persistSession();
			break;
		}
		default: {
			// Custom events from ProductHost aether bridge
			const raw = event as { event: string; data: unknown };
			if (raw.event === 'intent' && raw.data && typeof raw.data === 'object') {
				const data = raw.data as { intent?: Intent; name?: string };
				const intent =
					data.intent ||
					intentFromToolResult(data.name || '', data);
				if (intent?.present?.steps?.length) {
					void runIntentPresent(intent);
				} else if (intent) {
					void runIntentPresent(intent);
				}
			} else if (raw.event === 'navigation' && raw.data && typeof raw.data === 'object') {
				// Coarse SPA navigation (skipped server-side when intent.present exists)
				emitNavigation(raw.data as NavigationAction);
			}
			break;
		}
	}
}

/** Run Present choreography; ACK server when done (UX commit coordination). */
async function runIntentPresent(intent: Intent) {
	if (intent.present?.announce) {
		agentStatusLine.set(intent.present.announce);
	}
	const status = await executeIntent(intent);
	if (!status.ok && status.error) {
		agentStatusLine.set(`Present failed: ${status.error}`);
	} else if (status.ok && intent.present?.announce) {
		agentStatusLine.set(intent.present.announce);
	}
	// Notify server so get_current_context / follow-on tools see completion
	if (intent.id) {
		try {
			await fetch('/api/ux/intent_ack', {
				method: 'POST',
				headers: { 'Content-Type': 'application/json' },
				body: JSON.stringify({
					intent_id: intent.id,
					ok: status.ok,
					result: status.result ?? null,
					error: status.error ?? null
				})
			});
		} catch {
			/* ignore */
		}
	}
}

// ─── Public API ─────────────────────────────────────────────────────────────

/** Append construct token into the next composer seed (host + Insert). */
export function agentInsertToken(token: string) {
	const t = token.trim();
	if (!t) return;
	agentPendingSeed.update((prev) => (prev ? `${prev} ${t}` : t));
	agentComposerKey.update((k) => k + 1);
}

export async function agentSend(content: string, attachments?: File[]) {
	const pending = get(agentPendingAttachments);
	const files = mergeFiles(pending, attachments ?? []);
	agentClearPendingAttachments();

	const text = content.trim();
	if ((!text && files.length === 0) || get(agentIsStreaming)) return;

	agentError.set(null);
	agentStatusLine.set('');

	let prepared: Awaited<ReturnType<typeof prepareAttachments>> | null = null;
	if (files.length) {
		agentStatusLine.set(`Reading ${files.map((f) => f.name).join(', ')}…`);
		try {
			prepared = await prepareAttachments(text, files);
		} catch (e: unknown) {
			const msg = e instanceof Error ? e.message : String(e);
			agentError.set(`Could not read attachments: ${msg}`);
			agentStatusLine.set(msg);
			return;
		}
		if (prepared.warnings.length) {
			agentStatusLine.set(prepared.warnings.join(' '));
		} else {
			agentStatusLine.set(`Attached ${files.map((f) => f.name).join(', ')}`);
		}
		for (const p of prepared.previewUrls) previewObjectUrls.push(p.url);
	}

	const wireText = prepared?.wireText ?? text;
	const displayText = prepared?.displayText ?? text;
	if (!wireText.trim()) return;

	agentPendingSeed.set('');
	const contentBlocks: Message['content'] = [{ type: 'text', text: displayText }];
	if (prepared) {
		for (const p of prepared.previewUrls) {
			contentBlocks.push({ type: 'image', url: p.url, alt: p.name, mimeType: p.mime });
		}
	}
	const userMessage: Message = {
		id: `u_${Date.now()}`,
		role: 'user',
		content: contentBlocks,
		status: 'complete',
		createdAt: new Date().toISOString(),
		metadata: prepared
			? {
					wireText,
					attached: prepared.attachments.map((a) => a.name)
				}
			: undefined
	};
	setMessages((prev) => [...prev, userMessage]);
	agentIsStreaming.set(true);

	const history = get(agentMessages)
		.filter((m) => m.status === 'complete' || m.role === 'user')
		.map((m) => ({ role: m.role, content: textOf(m) }));
	// This turn's user row is display-only in the bubble; force the wire body.
	if (history.length) {
		history[history.length - 1] = { role: 'user', content: wireText };
	}

	// Refresh surfaces + IDE multi-pane viewport into SessionFocus before the turn
	if (typeof window !== 'undefined') {
		const w = window as unknown as { __veilAgentSurface?: { surfaces?: unknown[] } };
		if (w.__veilAgentSurface?.surfaces) {
			patchFocus({ surfaces: w.__veilAgentSurface.surfaces });
		}
		// IDE panes (PR wizard step, outline, dock, …) — deictic "this"
		try {
			const { flushIdeViewportToFocus } = await import('../ide/ideViewport');
			flushIdeViewportToFocus();
		} catch {
			/* not on IDE route */
		}
	}
	const focus = getFocus();
	const ctx = getAgentContext();

	// Ensure durable coding session when project is known (slug form for server)
	const projectSlug = slugifyClientProject(focus.project || ctx.project);
	if (projectSlug) {
		await ensureCodingSession(projectSlug);
		// Keep focus.project as slug so server resolve_chat_project never drops it
		if (focus.project && focus.project !== projectSlug) {
			patchFocus({ project: projectSlug });
		}
	}

	const focusForSend = getFocus();
	const focusPayloadObj = focusPayload(focusForSend);
	// Force slug on wire
	if (projectSlug && focusPayloadObj && typeof focusPayloadObj === 'object') {
		(focusPayloadObj as Record<string, unknown>).project = projectSlug;
	}

	// `project` + `focus` + `attachments` are VEIL hub extensions.
	const request = {
		messages: history,
		systemPrompt: buildSystemPrompt(getAgentContext()),
		project: projectSlug || undefined,
		sessionId: getCodingSessionId() || undefined,
		session_id: getCodingSessionId() || undefined,
		focus: focusPayloadObj,
		attachments: (prepared?.attachments ?? []) as WireAttachment[]
	} as ChatRequest;

	try {
		await stream.connect(
			chatWsUrl(),
			request,
			(ev) => handleEvent(ev),
			() => {
				agentIsStreaming.set(false);
				agentIsThinking.set(false);
				if (currentMessageId) {
					const id = currentMessageId;
					setMessages((prev) =>
						prev.map((m) =>
							m.id === id && m.status === 'streaming'
								? { ...m, status: 'complete' as const }
								: m
						)
					);
					currentMessageId = null;
				}
				persistSession();
			},
			(err) => {
				agentIsStreaming.set(false);
				agentError.set(err);
				agentStatusLine.set(err);
			}
		);
	} catch (e: unknown) {
		agentIsStreaming.set(false);
		const msg = e instanceof Error ? e.message : String(e);
		agentError.set(msg);
		agentStatusLine.set(msg);
	}
}

export function agentAbort() {
	stream.abort();
	agentIsStreaming.set(false);
}

export function agentClear() {
	agentAbort();
	agentMessages.set([]);
	agentError.set(null);
	agentStatusLine.set('');
	agentPendingSeed.set('');
	agentUnreadCount.set(0);
	agentClearPendingAttachments();
	for (const u of previewObjectUrls) {
		try {
			URL.revokeObjectURL(u);
		} catch {
			/* ignore */
		}
	}
	previewObjectUrls.length = 0;
	currentMessageId = null;
	clearPersistedSession();
}

/** Send tool approval (human-in-the-loop for destructive actions) */
export function agentApproveToolCall(callId: string, approved: boolean) {
	stream.sendToolApproval(callId, approved);
}

/** When create_branch / switch_main returns a new session, stick it for next turns + IDE. */
function maybeApplyCodingSessionSwitch(toolName: string, output: unknown) {
	const gitTools = new Set([
		'create_branch',
		'switch_main',
		'session_commit',
		'merge_branch',
		'session_status'
	]);
	if (!gitTools.has(toolName)) return;
	let obj: Record<string, unknown> | null = null;
	if (typeof output === 'string') {
		try {
			obj = JSON.parse(output) as Record<string, unknown>;
		} catch {
			return;
		}
	} else if (output && typeof output === 'object') {
		obj = output as Record<string, unknown>;
	}
	if (!obj) return;
	const sid =
		(typeof obj.codingSessionId === 'string' && obj.codingSessionId) ||
		(typeof obj.session_id === 'string' && obj.session_id) ||
		(obj.session &&
			typeof obj.session === 'object' &&
			typeof (obj.session as { session_id?: string }).session_id === 'string' &&
			(obj.session as { session_id: string }).session_id) ||
		null;
	if (sid && (obj.switched === true || toolName === 'create_branch' || toolName === 'switch_main')) {
		const slug =
			(obj.session &&
				typeof obj.session === 'object' &&
				typeof (obj.session as { slug?: string }).slug === 'string' &&
				(obj.session as { slug: string }).slug) ||
			(typeof obj.slug === 'string' && obj.slug) ||
			undefined;
		setCodingSessionId(sid, slug);
		// Best-effort refresh IDE meta (dynamic import avoids circular deps)
		void import('$lib/ide/store').then((store) => {
			store.setCodingSessionId(sid, slug);
			if (obj.session && typeof obj.session === 'object') {
				const s = obj.session as {
					session_id?: string;
					slug?: string;
					revision?: number;
					draft_mode?: boolean;
					branch_name?: string;
					base_branch?: string;
					head_commit?: string | null;
					committed_revision?: number | null;
					uncommitted?: boolean;
					branch?: string;
				};
				store.codingSessionMeta.set({
					session_id: s.session_id || sid,
					slug: s.slug || '',
					revision: s.revision ?? 0,
					draft_mode: s.draft_mode,
					branch_name: s.branch_name,
					base_branch: s.base_branch,
					head_commit: s.head_commit ?? null,
					committed_revision: s.committed_revision ?? null,
					uncommitted: s.uncommitted,
					branch: s.branch
				});
				if (typeof s.revision === 'number') store.codingSessionRevision.set(s.revision);
			}
		});
	}
}

/** Display name or path segment → URL/product slug. */
function slugifyClientProject(raw: string | null | undefined): string | null {
	if (!raw) return null;
	const s = raw.trim();
	if (!s || s.startsWith('(none')) return null;
	const slug = s
		.toLowerCase()
		.replace(/[^a-z0-9]+/g, '-')
		.replace(/^-+|-+$/g, '');
	return slug.length >= 2 ? slug : null;
}

// ─── System Prompt Builder ──────────────────────────────────────────────────

function buildSystemPrompt(ctx: AgentContext): string {
	const focus = getFocus();
	const parts = [
		'You are the VEIL Runtime agent. You control the entire veil platform UX via MCP tools.',
		'Coordination law: Focus (what user sees) + Intent/Present (visible product ops) + domain tools (coding/host).',
		'The user watches the dashboard: EVERY product action MUST be an MCP tool call. create_project returns intent.present — UX will animate the form; do NOT re-create.',
		'',
		'FORBIDDEN (silent / invisible — never do these):',
		'- shell curl/wget/fetch/httpie to /api/repos, /api/projects, /api/pull_requests, or any ProductHost API',
		'- mkdir / writing under VEIL_PROJECTS_DIR, monorepo, or ~/dev/veil-projects',
		'- describing navigation without calling navigate_to / open_* / create_project',
		'- inventing "project is empty" without list_files + read_source on the bound slug',
		'- create_project when the product already exists (tool returns existing:true — reuse it)',
		'',
		'REQUIRED platform MCP tools:',
		'- create_project({name, description?, origin_provider?, origin_owner?, origin_repo?, origin_create?, via?}) — CREATE once. Per-project git origin (jdwil/…, veil/…, Bitbucket). Then write layers/*.layer + MISSION.md + main.veil.',
		'- get_git_status / get_origin / bind_origin — inspect or change THIS project\'s git host+owner. Never curl /api/repos.',
		'- File root keyword is pkg only. Never write sol (removed).',
		'- Product annotations (@on, @command, @request, …) are authored in layers/*.layer via `ann`. Missing from ddd.layer is NOT a VEIL platform gap — do not stop to wiki-search it.',
		'- rename_project({name, project?, new_slug?}) / update_project — RENAME display name. NEVER curl/PATCH /api/repos or Bitbucket.',
		'- After via=ux create: wait_intent_ack({intent_id}) before write_source (intent_id from tool result).',
		'- REMOTE: each project has its own git origin. create_project / bind_origin set it. Session is a host checkout. write_source → session_commit (push that origin). NEVER grep the monorepo or /tmp.',
		'- Local conversion source (read-only): reference_roots / reference_list / reference_read / reference_grep on operator-listed dirs (VEIL_REFERENCE_DIRS / Config). Never write those trees. Author VEIL with write_source.',
		'- list_projects / get_project / open_project / open_ide / navigate_to / get_current_context / wait_intent_ack',
		'- list_prs / create_pr / get_pr / submit_pr / approve_pr / merge_pr / …',
		'- deploy / registry / config tools as needed',
		'',
		'IDE dual-loop tools (when a project is open):',
		'- list_files, select_file, veil_outline, read_source, veil_check, write_source, rename_construct',
		'- session_status / create_branch / session_commit / merge_branch / switch_main',
		'- wiki_*, http_request, dev_*',
		'',
		'Coding loop (non-negotiable for edits/refactors):',
		'1. Ensure scope: open_ide({project: slug}) OR use Focus/live_project — tool result includes file_count.',
		'2. list_files → if any .veil files, the project is NOT empty.',
		'3. read_source (or select_file + read_source) before redesigning domain models.',
		'4. write_source with full intended content → veil_check → fix any NEW diags same turn → session_commit (real git commit + push branch).',
		'5. When task done: create_pr + submit_pr (PR for human review). NEVER merge_branch/merge_pr unless operator says "merge".',
		'6. Short requests ("use ddd.layer", "refactor to X") mean edit the EXISTING package, not invent a new one.',
		'7. After create_project succeeds, write files. wiki_search is for platform contracts (bang/harness), not a tour before building.',
		'',
		'Deictic references ("this component", "this method", "this change", "here", "this change in /review"):',
		'- Use Session Focus + Visible panes below. Do not ask the user to restate what is selected.',
		'- Prefer the pane marked ★ primary. If the operator is on /review, "this" is the selected change (name, rationale, hunks).',
		'- If Focus.construct is set, operate on that construct unless panes point at a more specific review item.',
		'',
		'When the user asks to open/show/list something in the UI:',
		'1. Call the matching tool FIRST',
		'2. Explain briefly what they are looking at',
		'',
		formatFocusForAgent(focus),
		'',
		formatIntentLogForAgent(8),
		'',
		`Legacy page line: ${ctx.page} / project=${ctx.project || '(none)'}`
	];
	if ((focus.surfaces?.length || ctx.surfaces.length) > 0) {
		const surfaces = focus.surfaces?.length ? focus.surfaces : ctx.surfaces;
		parts.push(`- Available surfaces: ${JSON.stringify(surfaces.slice(0, 5))}`);
	}
	return parts.join('\n');
}

// ─── Session Persistence (sessionStorage + durable coding session_id) ───────

const SESSION_KEY = 'veil.agent.session';

/** Apply durable session META focus/intent_log into local Focus + intent log. */
function hydrateFocusFromSession(session: Record<string, unknown> | undefined | null) {
	if (!session) return;
	const lastFocus = session.last_focus as Record<string, unknown> | undefined;
	if (lastFocus && typeof lastFocus === 'object') {
		patchFocus({
			route: typeof lastFocus.route === 'string' ? lastFocus.route : undefined,
			project:
				lastFocus.project === null
					? null
					: typeof lastFocus.project === 'string'
						? lastFocus.project
						: undefined,
			file: typeof lastFocus.file === 'string' ? lastFocus.file : undefined,
			construct: typeof lastFocus.construct === 'string' ? lastFocus.construct : undefined,
			constructKind:
				typeof lastFocus.constructKind === 'string' ? lastFocus.constructKind : undefined
		});
	}
	const log = session.intent_log;
	if (Array.isArray(log) && log.length) {
		mergeIntentLogFromServer(log);
	}
}

/** Create or attach durable coding session for a project slug **or** repo UUID. */
export async function ensureCodingSession(slug: string | null): Promise<string | null> {
	if (!slug || typeof window === 'undefined') return getCodingSessionId();
	const existing = getCodingSessionId(slug);
	if (existing) {
		try {
			const res = await fetch(`/api/sessions/${encodeURIComponent(existing)}`);
			if (res.ok) {
				const data = await res.json();
				const s = data?.session as Record<string, unknown> | undefined;
				// Same product if slug matches **or** route used repo UUID (id).
				const sessionSlug = typeof s?.slug === 'string' ? s.slug : '';
				const sessionRepo = typeof s?.repo_id === 'string' ? s.repo_id : '';
				if (sessionSlug === slug || sessionRepo === slug) {
					hydrateFocusFromSession(s as Record<string, unknown>);
					return existing;
				}
			}
		} catch {
			/* recreate */
		}
	}
	try {
		const res = await fetch('/api/sessions', {
			method: 'POST',
			headers: { 'Content-Type': 'application/json' },
			body: JSON.stringify({ slug })
		});
		if (!res.ok) return null;
		const data = await res.json();
		hydrateFocusFromSession(data?.session as Record<string, unknown>);
		const id = data?.session?.session_id as string | undefined;
		if (id) setCodingSessionId(id, slug);
		return id ?? null;
	} catch {
		return null;
	}
}

/** Hydrate agent transcript from durable session turns (server). */
export async function hydrateFromServer(sessionId?: string | null): Promise<void> {
	const sid = sessionId || getCodingSessionId();
	if (!sid) return;
	try {
		const res = await fetch(`/api/sessions/${encodeURIComponent(sid)}/turns`);
		if (!res.ok) return;
		const data = await res.json();
		const turns = (data?.turns || []) as Array<{
			turn_id: string;
			role: string;
			content: string;
			ts?: string;
		}>;
		if (!turns.length) return;
		const msgs: Message[] = turns.map((t) => ({
			id: t.turn_id,
			role: t.role === 'user' ? 'user' : 'assistant',
			content: [{ type: 'text', text: t.content }],
			status: 'complete' as const,
			createdAt: t.ts ? new Date(Number(t.ts) * 1000).toISOString() : new Date().toISOString()
		}));
		agentMessages.set(msgs);
	} catch {
		/* ignore */
	}
}

function persistSession() {
	if (typeof sessionStorage === 'undefined') return;
	try {
		const data = {
			messages: get(agentMessages),
			status: get(agentStatusLine),
			error: get(agentError),
			codingSessionId: getCodingSessionId(),
			at: Date.now()
		};
		sessionStorage.setItem(SESSION_KEY, JSON.stringify(data));
		// Also mirror session id to localStorage for tab-close survival
		if (data.codingSessionId) setCodingSessionId(data.codingSessionId);
	} catch { /* quota exceeded — ignore */ }
}

function clearPersistedSession() {
	if (typeof sessionStorage === 'undefined') return;
	sessionStorage.removeItem(SESSION_KEY);
}

export function restoreSession() {
	if (typeof sessionStorage === 'undefined') return;
	try {
		const raw = sessionStorage.getItem(SESSION_KEY);
		if (!raw) {
			// Fall back to durable server turns if we only have coding session id
			void hydrateFromServer();
			return;
		}
		const data = JSON.parse(raw) as {
			messages?: Message[];
			status?: string;
			error?: string | null;
			codingSessionId?: string;
			at?: number;
		};
		// Ignore stale sessions (> 2 hours) for browser cache; still try server hydrate
		if (data.at && Date.now() - data.at > 7_200_000) {
			clearPersistedSession();
			void hydrateFromServer(data.codingSessionId || getCodingSessionId());
			return;
		}
		if (data.codingSessionId) setCodingSessionId(data.codingSessionId);
		if (Array.isArray(data.messages) && data.messages.length) {
			agentMessages.set(data.messages);
		} else {
			void hydrateFromServer(data.codingSessionId || getCodingSessionId());
		}
		if (data.status) agentStatusLine.set(data.status);
		if (data.error) agentError.set(data.error);
	} catch { /* ignore parse errors */ }
}

// ─── IDE Bridge (postMessage) ───────────────────────────────────────────────

const IDE_ORIGIN = '*'; // Will be restricted in production

export interface IdeBridgeMessage {
	type:
		| 'agent:edit'
		| 'agent:navigate'
		| 'agent:refresh'
		| 'ide:selection'
		| 'ide:error'
		| 'ide:ready'
		| 'ide:agent-prompt'
		| 'ide:diagnostics-summary'
		| 'agent:session-state';
	payload?: unknown;
}

/** Latest IDE diagnostics summary (for agent empty-state chips). */
export const ideDiagnosticsSummary = writable<{
	count: number;
	project: string | null;
	sample: Array<{
		severity?: string;
		message?: string;
		node_name?: string | null;
		code?: string;
		hint?: string | null;
	}>;
}>({ count: 0, project: null, sample: [] });

let ideFrame: HTMLIFrameElement | null = null;

export function registerIdeFrame(frame: HTMLIFrameElement) {
	ideFrame = frame;
	// Send current session state so IDE can show conversation
	sendToIde({
		type: 'agent:session-state',
		payload: { messages: get(agentMessages), isStreaming: get(agentIsStreaming) }
	});
}

export function unregisterIdeFrame() {
	ideFrame = null;
}

export function sendToIde(msg: IdeBridgeMessage) {
	if (ideFrame?.contentWindow) {
		ideFrame.contentWindow.postMessage(msg, IDE_ORIGIN);
	}
}

/** Listen for messages from the IDE iframe */
export function initIdeBridge() {
	if (typeof window === 'undefined') return;
	window.addEventListener('message', handleIdeMessage);
}

export function destroyIdeBridge() {
	if (typeof window === 'undefined') return;
	window.removeEventListener('message', handleIdeMessage);
}

function handleIdeMessage(event: MessageEvent) {
	const msg = event.data as IdeBridgeMessage | undefined;
	if (!msg || typeof msg.type !== 'string') return;

	switch (msg.type) {
		case 'ide:selection': {
			// User selected something in the IDE — structured Focus (not just a chat token)
			const sel = msg.payload as {
				construct?: string;
				path?: string;
				kind?: string;
				id?: string;
			} | undefined;
			if (sel?.construct) {
				patchFocus({
					construct: sel.construct,
					constructKind: sel.kind ?? null,
					file: sel.path ?? undefined,
					selection: {
						kind: sel.kind || 'construct',
						id: sel.id || sel.construct,
						label: sel.construct
					}
				});
				agentInsertToken(`[IDE: ${sel.construct}]`);
			}
			break;
		}
		case 'ide:error': {
			const err = msg.payload as { message?: string } | undefined;
			if (err?.message) {
				agentStatusLine.set(`IDE: ${err.message}`);
			}
			break;
		}
		case 'ide:ready': {
			// IDE loaded — send current session state
			sendToIde({
				type: 'agent:session-state',
				payload: { messages: get(agentMessages), isStreaming: get(agentIsStreaming) }
			});
			break;
		}
		case 'ide:agent-prompt': {
			// User sent issues / a task from the IDE detail panel
			const p = msg.payload as { text?: string; autoSend?: boolean } | undefined;
			const text = p?.text?.trim();
			if (!text) break;
			openAgentPanel();
			if (p?.autoSend !== false) {
				void agentSend(text);
			} else {
				agentInsertToken(text);
			}
			break;
		}
		case 'ide:diagnostics-summary': {
			const p = msg.payload as {
				count?: number;
				project?: string | null;
				sample?: Array<{
					severity?: string;
					message?: string;
					node_name?: string | null;
					code?: string;
					hint?: string | null;
				}>;
			} | undefined;
			const count = typeof p?.count === 'number' ? p.count : 0;
			const sample = Array.isArray(p?.sample) ? p.sample : [];
			ideDiagnosticsSummary.set({
				count,
				project: p?.project ?? null,
				sample
			});
			patchFocus({
				diagnostics: { count, sample },
				project: p?.project ?? getFocus().project
			});
			break;
		}
	}
}

// Forward session changes to IDE iframe in real-time
agentMessages.subscribe((msgs) => {
	if (ideFrame) {
		sendToIde({ type: 'agent:session-state', payload: { messages: msgs, isStreaming: get(agentIsStreaming) } });
	}
});
