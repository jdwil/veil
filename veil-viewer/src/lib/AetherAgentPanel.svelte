<script lang="ts">
	/**
	 * Agent dock panel UI. Conversation + stream live in agentSession so tab
	 * switches (Agent ↔ Split) and panel remounts do not wipe the chat.
	 */
	import { MessageList, ChatInput } from '@aether-ui/core';
	import {
		agentMessages,
		agentIsStreaming,
		agentIsThinking,
		agentError,
		agentStatusLine,
		agentComposerKey,
		agentPendingSeed,
		agentSend,
		agentAbort,
		agentInsertToken
	} from '$lib/agentSession';

	interface Props {
		/** When set (e.g. from canvas selection), append into the next send. */
		insertToken?: string;
		embedded?: boolean;
	}

	let { insertToken = '', embedded = false }: Props = $props();

	$effect(() => {
		if (insertToken?.trim()) {
			agentInsertToken(insertToken);
		}
	});
</script>

<div class="aether-agent" class:embedded>
	<header class="head">
		<span class="title">Agent</span>
		<span class="hint">Aether · WebSocket · VEIL tools</span>
		{#if $agentStatusLine}
			<span class="status" title={$agentStatusLine}>{$agentStatusLine}</span>
		{/if}
	</header>

	{#if $agentError}
		<div class="err" role="alert">{$agentError}</div>
	{/if}

	<div class="thread">
		{#if $agentMessages.length === 0}
			<p class="empty">
				Ask about VEIL packages, layers, stubs, or dual-loop. Select a node and use
				<strong>+ Insert</strong>. Attach sketches when ready (UI now; backend next).
			</p>
		{:else}
			<div class="msg-list">
				<MessageList
					messages={$agentMessages}
					isStreaming={$agentIsStreaming}
					isThinking={$agentIsThinking}
				/>
			</div>
		{/if}
	</div>

	{#key $agentComposerKey}
		<ChatInput
			onSend={agentSend}
			onAbort={agentAbort}
			isStreaming={$agentIsStreaming}
			placeholder="Ask the agent…"
			initialText={$agentPendingSeed}
		/>
	{/key}
</div>

<style>
	.aether-agent {
		display: flex;
		flex-direction: column;
		height: 100%;
		min-height: 0;
		background: var(--color-surface-0, #0f0f0f);
		color: var(--color-text-primary, #e5e5e5);
		font-size: 0.875rem;
	}
	.aether-agent.embedded {
		border: none;
	}
	.head {
		display: flex;
		align-items: center;
		gap: 0.75rem;
		padding: 0.45rem 0.75rem;
		border-bottom: 1px solid var(--color-surface-border, #2e2e2e);
		flex-shrink: 0;
	}
	.title {
		font-weight: 700;
		font-size: 0.75rem;
	}
	.hint {
		color: var(--color-text-muted, #737373);
		font-size: 0.65rem;
	}
	.status {
		margin-left: auto;
		max-width: 45%;
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
		color: var(--color-text-secondary, #a3a3a3);
		font-size: 0.65rem;
	}
	.err {
		padding: 0.4rem 0.75rem;
		background: rgba(248, 113, 113, 0.12);
		border-bottom: 1px solid #f87171;
		color: #fecaca;
		font-size: 0.8rem;
	}
	.thread {
		flex: 1;
		min-height: 0;
		overflow: auto;
		display: flex;
		flex-direction: column;
	}
	.msg-list {
		flex: 1;
		min-height: 0;
		display: flex;
		flex-direction: column;
	}
	.msg-list :global(> *) {
		flex: 1;
		min-height: 0;
	}
	.empty {
		color: var(--color-text-muted, #737373);
		padding: 0.75rem 1rem;
		line-height: 1.5;
		font-size: 0.8rem;
	}
</style>
