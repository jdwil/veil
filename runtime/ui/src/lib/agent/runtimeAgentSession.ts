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

// ─── State Stores ───────────────────────────────────────────────────────────

export const agentMessages = writable<Message[]>([]);
export const agentIsStreaming = writable(false);
export const agentIsThinking = writable(false);
export const agentError = writable<string | null>(null);
export const agentStatusLine = writable('');
export const agentComposerKey = writable(0);
export const agentPendingSeed = writable('');
export const agentPanelOpen = writable(false);
export const agentUnreadCount = writable(0);

// ─── Context (injected per turn) ────────────────────────────────────────────

export interface AgentContext {
	page: string;
	project: string | null;
	surfaces: unknown[];
}

let currentContext: AgentContext = {
	page: '/',
	project: null,
	surfaces: []
};

export function setAgentContext(ctx: Partial<AgentContext>) {
	currentContext = { ...currentContext, ...ctx };
}

export function getAgentContext(): AgentContext {
	return currentContext;
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

/** Well-known MCP / agent tool names → SPA navigation (agent owns UX, not Svelte chips). */
const TOOL_NAV: Record<string, NavigationAction> = {
	list_changes: { action: 'goto', path: '/changes' },
	open_changes: { action: 'goto', path: '/changes' },
	create_change: { action: 'goto', path: '/changes/new' },
	open_create_change: { action: 'goto', path: '/changes/new' },
	list_projects: { action: 'goto', path: '/projects' },
	open_projects: { action: 'goto', path: '/projects' },
	open_deploy: { action: 'goto', path: '/deploy' },
	open_registry: { action: 'goto', path: '/registry' },
	open_dashboard: { action: 'goto', path: '/dashboard' },
	open_config: { action: 'goto', path: '/config' }
};

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
	return TOOL_NAV[name] ?? null;
}

// ─── Stream Service ─────────────────────────────────────────────────────────

const stream = new StreamService();
let currentMessageId: string | null = null;

function chatWsUrl(): string {
	if (typeof window === 'undefined') return 'ws://127.0.0.1:3000/api/agent/chat';
	const proto = window.location.protocol === 'https:' ? 'wss:' : 'ws:';
	return `${proto}//${window.location.host}/api/agent/chat`;
}

function textOf(m: Message): string {
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
			currentMessageId = event.data.messageId;
			const sid = (event.data as { sessionId?: string }).sessionId;
			if (sid) setCodingSessionId(sid);
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
			// Increment unread if panel is closed
			if (!get(agentPanelOpen)) {
				agentUnreadCount.update((n) => n + 1);
			}
			break;
		}
		case 'content_delta': {
			const id = event.data.messageId;
			setMessages((prev) =>
				prev.map((m) => {
					if (m.id !== id) return m;
					const blocks = [...m.content];
					const last = blocks[blocks.length - 1];
					if (last && last.type === 'text') {
						blocks[blocks.length - 1] = { type: 'text', text: last.text + event.data.delta };
					} else {
						blocks.push({ type: 'text', text: event.data.delta });
					}
					return { ...m, content: blocks, status: 'streaming' };
				})
			);
			break;
		}
		case 'tool_call_start': {
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
			// Agent-driven UX: navigate as soon as a known platform tool starts
			const navStart = navigationFromTool(toolName);
			if (navStart) emitNavigation(navStart);
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
			const nav = navigationFromTool(toolName, undefined, data.arguments);
			if (nav) emitNavigation(nav);
			break;
		}
		case 'tool_result': {
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
			const nav = navigationFromTool(toolName, output);
			if (nav) emitNavigation(nav);
			break;
		}
		case 'error': {
			agentError.set(event.data.message);
			agentStatusLine.set(event.data.message);
			break;
		}
		case 'done': {
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
			// Handle navigation events in done payload
			if (data.navigation && typeof data.navigation === 'object') {
				emitNavigation(data.navigation as NavigationAction);
			}
			// Persist after each complete turn
			persistSession();
			break;
		}
		default: {
			// Handle custom navigation events from the backend
			const raw = event as { event: string; data: unknown };
			if (raw.event === 'navigation' && raw.data && typeof raw.data === 'object') {
				emitNavigation(raw.data as NavigationAction);
			}
			break;
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
	const text = content.trim();
	if ((!text && !(attachments && attachments.length)) || get(agentIsStreaming)) return;

	agentError.set(null);
	agentStatusLine.set('');
	if (attachments?.length) {
		agentStatusLine.set(
			`Attached ${attachments.map((f) => f.name).join(', ')} (text-only for now)`
		);
	}
	if (!text) return;

	agentPendingSeed.set('');
	const userMessage: Message = {
		id: `u_${Date.now()}`,
		role: 'user',
		content: [{ type: 'text', text }],
		status: 'complete',
		createdAt: new Date().toISOString()
	};
	setMessages((prev) => [...prev, userMessage]);
	agentIsStreaming.set(true);

	const history = get(agentMessages)
		.filter((m) => m.status === 'complete' || m.role === 'user')
		.map((m) => ({ role: m.role, content: textOf(m) }));

	// Collect current context for injection
	const ctx = getAgentContext();
	if (typeof window !== 'undefined' && (window as any).__veilAgentSurface) {
		ctx.surfaces = (window as any).__veilAgentSurface.surfaces || [];
	}

	// Ensure durable coding session when project is known
	if (ctx.project) {
		await ensureCodingSession(ctx.project);
	}

	// `project` is a VEIL hub extension (not in aether ChatRequest type) so the
	// backend can scope dual-loop MCP tools to the open product.
	const request = {
		messages: history,
		systemPrompt: buildSystemPrompt(ctx),
		project: ctx.project || undefined,
		sessionId: getCodingSessionId() || undefined,
		session_id: getCodingSessionId() || undefined
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
	currentMessageId = null;
	clearPersistedSession();
}

/** Send tool approval (human-in-the-loop for destructive actions) */
export function agentApproveToolCall(callId: string, approved: boolean) {
	stream.sendToolApproval(callId, approved);
}

// ─── System Prompt Builder ──────────────────────────────────────────────────

function buildSystemPrompt(ctx: AgentContext): string {
	const parts = [
		'You are the VEIL Runtime agent. You control the entire veil platform UX via tools.',
		'The user must SEE you work: always call navigation tools so the dashboard changes pages.',
		'',
		'Platform UX tools (use these — do not only describe navigation):',
		'- navigate_to({path}) — any SPA path (/changes, /projects/{id}, /deploy, …)',
		'- list_changes / create_change — SDLC change requests',
		'- list_projects / open_project / open_ide — projects; open_ide embeds IDE in-shell (agent stays here)',
		'- open_deploy / open_registry / open_dashboard / open_config',
		'',
		'IDE dual-loop tools (when a project is open — they ARE connected via MCP):',
		'- list_files — packages/layers in the project',
		'- select_file({ name | index }) — switch active file',
		'- veil_outline — IR construct topology (prefer this for "show me construct X")',
		'- read_source — active .veil text (after select_file if needed)',
		'- veil_check / write_source / rename_construct — edit + validate',
		'- Also: wiki_*, http_request, dev_* for dual-loop / Mind Palace',
		'',
		'When the user asks about a construct/node (e.g. decrypt_integration_secrets):',
		'1. list_files (if needed) → select_file the package that owns it',
		'2. veil_outline to locate the construct',
		'3. read_source for full details — do NOT claim IDE tools are unavailable',
		'',
		'When the user asks to open/show/list something in the UI:',
		'1. Call the matching tool FIRST (e.g. list_changes for "open changes")',
		'2. Then explain briefly what they are looking at',
		'3. For code edits: open_ide then write_source / structured edit tools',
		'',
		`Current context:`,
		`- Page: ${ctx.page}`,
		`- Project: ${ctx.project || '(none — home/dashboard)'}`
	];
	if (ctx.surfaces.length > 0) {
		parts.push(`- Available surfaces: ${JSON.stringify(ctx.surfaces.slice(0, 5))}`);
	}
	return parts.join('\n');
}

// ─── Session Persistence (sessionStorage + durable coding session_id) ───────

const SESSION_KEY = 'veil.agent.session';
const CODING_SESSION_KEY = 'veil.coding.sessionId';

export function getCodingSessionId(): string | null {
	if (typeof localStorage === 'undefined') return null;
	try {
		return localStorage.getItem(CODING_SESSION_KEY);
	} catch {
		return null;
	}
}

export function setCodingSessionId(id: string | null) {
	if (typeof localStorage === 'undefined') return;
	try {
		if (id) localStorage.setItem(CODING_SESSION_KEY, id);
		else localStorage.removeItem(CODING_SESSION_KEY);
	} catch {
		/* ignore */
	}
}

/** Create or attach durable coding session for a project slug. */
export async function ensureCodingSession(slug: string | null): Promise<string | null> {
	if (!slug || typeof window === 'undefined') return getCodingSessionId();
	const existing = getCodingSessionId();
	if (existing) {
		try {
			const res = await fetch(`/api/sessions/${encodeURIComponent(existing)}`);
			if (res.ok) {
				const data = await res.json();
				if (data?.session?.slug === slug) return existing;
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
		if (!res.ok) return getCodingSessionId();
		const data = await res.json();
		const id = data?.session?.session_id as string | undefined;
		if (id) setCodingSessionId(id);
		return id ?? null;
	} catch {
		return getCodingSessionId();
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
			// User selected something in the IDE — add as context token
			const sel = msg.payload as { construct?: string; path?: string } | undefined;
			if (sel?.construct) {
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
			agentPanelOpen.set(true);
			agentUnreadCount.set(0);
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
			ideDiagnosticsSummary.set({
				count: typeof p?.count === 'number' ? p.count : 0,
				project: p?.project ?? null,
				sample: Array.isArray(p?.sample) ? p.sample : []
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
