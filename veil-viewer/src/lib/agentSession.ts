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
import { ideApiBase, refreshAfterEdit } from '$lib/store';

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
					return {
						...m,
						content: [
							...m.content,
							{
								type: 'tool_call',
								toolCall: {
									id: event.data.callId,
									name: event.data.name,
									arguments: '',
									status: 'executing' as const
								}
							}
						]
					};
				})
			);
			break;
		}
		case 'tool_result': {
			const id = event.data.messageId;
			setMessages((prev) =>
				prev.map((m) => {
					if (m.id !== id) return m;
					return {
						...m,
						content: [
							...m.content,
							{
								type: 'tool_result',
								toolResult: {
									callId: event.data.callId,
									name: event.data.name,
									output: event.data.output,
									isError: event.data.isError
								}
							}
						]
					};
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
		.map((m) => ({
			role: m.role,
			content: textOf(m)
		}));

	const request: ChatRequest = {
		messages: history,
		systemPrompt:
			'You are the VEIL IDE agent. Prefer wiki tools for platform knowledge when available. Edit packages via workspace tools.'
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
