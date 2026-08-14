/**
 * Shared agent session for the IDE dock.
 * Survives AetherAgentPanel remounts (Agent ↔ Split ↔ collapse) so conversation
 * and an in-flight stream are not wiped when the tab branch unmounts.
 */
import { writable, get } from 'svelte/store';
import {
	StreamService,
	type Message,
	type StreamEvent,
	type TextContent,
	type ChatRequest
} from '@aether-ui/core';
import { ideApiBase, refreshAfterEdit } from '$lib/ide/store';

export const agentMessages = writable<Message[]>([]);
export const agentIsStreaming = writable(false);
export const agentIsThinking = writable(false);
export const agentError = writable<string | null>(null);
export const agentStatusLine = writable('');
/** Bumps when host wants ChatInput remounted with a seed (e.g. + Insert). */
export const agentComposerKey = writable(0);
export const agentPendingSeed = writable('');

const stream = new StreamService();
let currentMessageId: string | null = null;

function chatWsUrl(): string {
	const base = ideApiBase().replace(/\/$/, '');
	const http = base.endsWith('/chat') ? base : `${base}/chat`;
	return http.replace(/^http/, 'ws');
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
						blocks[blocks.length - 1] = {
							type: 'text',
							text: last.text + event.data.delta
						};
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
					// Insert tool_call block BEFORE the trailing text block
					// (server emits tools after text, but they executed during)
					const blocks = [...m.content];
					const lastIdx = blocks.length - 1;
					const insertIdx =
						lastIdx >= 0 && blocks[lastIdx].type === 'text' ? lastIdx : blocks.length;
					blocks.splice(insertIdx, 0, {
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
					// Insert tool_result BEFORE the trailing text block
					const blocks = [...m.content];
					const lastIdx = blocks.length - 1;
					const insertIdx =
						lastIdx >= 0 && blocks[lastIdx].type === 'text' ? lastIdx : blocks.length;
					blocks.splice(insertIdx, 0, {
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
			const data = event.data as {
				sourceChanged?: boolean;
				contextWarning?: string | null;
				backend?: string;
			};
			if (data.contextWarning) {
				agentStatusLine.set(data.contextWarning);
			} else if (data.backend) {
				agentStatusLine.set(data.backend);
			}
			if (data.sourceChanged) {
				void refreshAfterEdit();
			}
			break;
		}
		default:
			break;
	}
}

/** Append construct token into the next composer seed (host + Insert). */
export function agentInsertToken(token: string) {
	const t = token.trim();
	if (!t) return;
	agentPendingSeed.update((prev) => (prev ? `${prev} ${t}` : t));
	agentComposerKey.update((k) => k + 1);
}

export async function agentSend(content: string, attachments?: File[]) {
	const text = content.trim();
	const files = attachments ?? [];
	if ((!text && files.length === 0) || get(agentIsStreaming)) return;

	agentError.set(null);
	agentStatusLine.set('');

	let wireText = text;
	let displayText = text;
	let preview: { name: string; url: string; mime: string }[] = [];
	let wireAtts: { name: string; mimeType: string; kind: string; dataBase64?: string }[] = [];
	if (files.length) {
		const { prepareAttachments } = await import('$lib/agent/attachments');
		try {
			const prepared = await prepareAttachments(text, files);
			wireText = prepared.wireText;
			displayText = prepared.displayText;
			preview = prepared.previewUrls;
			wireAtts = prepared.attachments;
			agentStatusLine.set(
				prepared.warnings.length
					? prepared.warnings.join(' ')
					: `Attached ${files.map((f) => f.name).join(', ')}`
			);
		} catch (e: unknown) {
			const msg = e instanceof Error ? e.message : String(e);
			agentError.set(`Could not read attachments: ${msg}`);
			return;
		}
	}
	if (!wireText.trim()) return;

	agentPendingSeed.set('');
	const blocks: Message['content'] = [{ type: 'text', text: displayText }];
	for (const p of preview) {
		blocks.push({ type: 'image', url: p.url, alt: p.name, mimeType: p.mime });
	}
	const userMessage: Message = {
		id: `u_${Date.now()}`,
		role: 'user',
		content: blocks,
		status: 'complete',
		createdAt: new Date().toISOString(),
		metadata: files.length ? { wireText } : undefined
	};
	setMessages((prev) => [...prev, userMessage]);
	agentIsStreaming.set(true);

	const history = get(agentMessages)
		.filter((m) => m.status === 'complete' || m.role === 'user')
		.map((m) => ({
			role: m.role,
			content: textOf(m)
		}));
	if (history.length) {
		history[history.length - 1] = { role: 'user', content: wireText };
	}

	const request = {
		messages: history,
		systemPrompt:
			'You are the VEIL IDE agent. Prefer wiki tools for platform knowledge when available. Edit packages via workspace tools.',
		attachments: wireAtts
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
	currentMessageId = null;
}

const HANDOFF_KEY = 'veil.agent.handoff';

/** Snapshot conversation for pop-out window handoff (localStorage so popups share it). */
export function agentSaveHandoff() {
	if (typeof localStorage === 'undefined') return;
	try {
		localStorage.setItem(
			HANDOFF_KEY,
			JSON.stringify({
				messages: get(agentMessages),
				status: get(agentStatusLine),
				error: get(agentError),
				at: Date.now()
			})
		);
	} catch {
		/* ignore */
	}
}

/** Load conversation if another window left a handoff snapshot. */
export function agentLoadHandoff() {
	if (typeof localStorage === 'undefined') return;
	try {
		const raw = localStorage.getItem(HANDOFF_KEY);
		if (!raw) return;
		const data = JSON.parse(raw) as {
			messages?: Message[];
			status?: string;
			error?: string | null;
			at?: number;
		};
		// Ignore stale handoffs (> 1 hour)
		if (data.at && Date.now() - data.at > 3_600_000) return;
		if (Array.isArray(data.messages) && data.messages.length) {
			agentMessages.set(data.messages);
		}
		if (data.status) agentStatusLine.set(data.status);
		if (data.error) agentError.set(data.error);
	} catch {
		/* ignore */
	}
}

// Expose for agentLayout / ReviewDock without circular imports
if (typeof window !== 'undefined') {
	const w = window as unknown as {
		__veilAgentSaveHandoff?: () => void;
		__veilAgentLoadHandoff?: () => void;
	};
	w.__veilAgentSaveHandoff = agentSaveHandoff;
	w.__veilAgentLoadHandoff = agentLoadHandoff;
}
