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
			setMessages((prev) =>
				prev.map((m) => {
					if (m.id !== id) return m;
					const blocks = [...m.content];
					blocks.push({
						type: 'tool_call',
						toolCall: {
							id: event.data.callId,
							name: event.data.name,
							arguments: '',
							status: 'executing' as const
						}
					});
					return { ...m, content: blocks };
				})
			);
			break;
		}
		case 'tool_result': {
			const id = event.data.messageId;
			setMessages((prev) =>
				prev.map((m) => {
					if (m.id !== id) return m;
					const blocks = [...m.content];
					blocks.push({
						type: 'tool_result',
						toolResult: {
							callId: event.data.callId,
							name: event.data.name,
							output: event.data.output,
							isError: event.data.isError
						}
					});
					return { ...m, content: blocks };
				})
			);
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

	const request: ChatRequest = {
		messages: history,
		systemPrompt: buildSystemPrompt(ctx)
	};

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
		'You are the VEIL Runtime agent. You have full control over the veil platform:',
		'- Edit code in any project via the IDE',
		'- Manage the SDLC: create/review/merge changes',
		'- Deploy projects to environments',
		'- Navigate the UI to show the user what you\'re doing',
		'- Inspect the bus, registry, and configuration',
		'- Remember knowledge in the wiki',
		'',
		'When the user asks you to do something:',
		'1. If it requires navigating to a different page, do so (they see the transition)',
		'2. If it requires editing code, open the IDE for that project and make changes',
		'3. If it spans multiple projects, switch between them seamlessly',
		'4. Show your work — use navigation so the user sees what\'s happening',
		'',
		`Current context:`,
		`- Page: ${ctx.page}`,
		`- Project: ${ctx.project || '(none — home/dashboard)'}`,
	];
	if (ctx.surfaces.length > 0) {
		parts.push(`- Available surfaces: ${JSON.stringify(ctx.surfaces.slice(0, 5))}`);
	}
	return parts.join('\n');
}

// ─── Session Persistence (sessionStorage) ───────────────────────────────────

const SESSION_KEY = 'veil.agent.session';

function persistSession() {
	if (typeof sessionStorage === 'undefined') return;
	try {
		const data = {
			messages: get(agentMessages),
			status: get(agentStatusLine),
			error: get(agentError),
			at: Date.now()
		};
		sessionStorage.setItem(SESSION_KEY, JSON.stringify(data));
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
		if (!raw) return;
		const data = JSON.parse(raw) as {
			messages?: Message[];
			status?: string;
			error?: string | null;
			at?: number;
		};
		// Ignore stale sessions (> 2 hours)
		if (data.at && Date.now() - data.at > 7_200_000) {
			clearPersistedSession();
			return;
		}
		if (Array.isArray(data.messages) && data.messages.length) {
			agentMessages.set(data.messages);
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
		| 'agent:session-state';
	payload?: unknown;
}

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
	}
}

// Forward session changes to IDE iframe in real-time
agentMessages.subscribe((msgs) => {
	if (ideFrame) {
		sendToIde({ type: 'agent:session-state', payload: { messages: msgs, isStreaming: get(agentIsStreaming) } });
	}
});
