<script lang="ts">
	/**
	 * AgentDock — persistent slide-out panel for the runtime agent.
	 * Lives in root layout, persists across all route navigation.
	 * Resizable via drag handle on the left edge.
	 */
	import { MessageList, ChatInput, ToolCallBlock } from '@aether-ui/core';
	import {
		agentMessages,
		agentIsStreaming,
		agentIsThinking,
		agentError,
		agentStatusLine,
		agentComposerKey,
		agentPendingSeed,
		agentPanelOpen,
		agentUnreadCount,
		agentSend,
		agentAbort,
		agentClear,
		agentInsertToken
	} from '$lib/agent/runtimeAgentSession';

	let panelWidth = $state(420);
	let isResizing = $state(false);
	const MIN_WIDTH = 320;
	const MAX_WIDTH = 700;

	// Provider status polling
	interface ProviderStatus {
		provider: string;
		acp_tunnel: { connected: boolean; agents: Array<{ agent_name: string }> };
	}
	let providerStatus: ProviderStatus | null = $state(null);

	async function fetchProviderStatus() {
		try {
			const res = await fetch('/api/agent/status');
			if (res.ok) {
				providerStatus = await res.json();
			}
		} catch { /* ignore — poll will retry */ }
	}

	$effect(() => {
		fetchProviderStatus();
		const interval = setInterval(fetchProviderStatus, 8000);
		return () => clearInterval(interval);
	});

	function providerLabel(): string {
		if (!providerStatus) return 'Aether · Runtime';
		if (providerStatus.acp_tunnel.connected && providerStatus.acp_tunnel.agents.length > 0) {
			return `Connected (${providerStatus.acp_tunnel.agents[0].agent_name})`;
		}
		return providerStatus.provider;
	}

	function isAcpConnected(): boolean {
		return providerStatus?.acp_tunnel?.connected ?? false;
	}

	function startResize(e: MouseEvent) {
		e.preventDefault();
		isResizing = true;
		const startX = e.clientX;
		const startWidth = panelWidth;

		function onMove(ev: MouseEvent) {
			const delta = startX - ev.clientX;
			panelWidth = Math.max(MIN_WIDTH, Math.min(MAX_WIDTH, startWidth + delta));
		}
		function onUp() {
			isResizing = false;
			document.removeEventListener('mousemove', onMove);
			document.removeEventListener('mouseup', onUp);
		}
		document.addEventListener('mousemove', onMove);
		document.addEventListener('mouseup', onUp);
	}

	function togglePanel() {
		agentPanelOpen.update((v) => !v);
		if (!$agentPanelOpen) {
			// Opening — clear unread
			agentUnreadCount.set(0);
		}
	}

	function handleSend(content: string, attachments?: File[]) {
		agentSend(content, attachments);
	}
</script>

{#if $agentPanelOpen}
	<aside
		class="agent-dock"
		style="width: {panelWidth}px"
		class:resizing={isResizing}
		role="complementary"
		aria-label="AI Agent"
	>
		<!-- Resize handle -->
		<div
			class="resize-handle"
			role="separator"
			aria-orientation="vertical"
			onmousedown={startResize}
		></div>

		<!-- Header -->
		<header class="dock-header">
			<div class="header-left">
				<span class="agent-icon" class:connected={isAcpConnected()}>◆</span>
				<span class="agent-title">Agent</span>
				<span class="agent-hint">{providerLabel()}</span>
			</div>
			<div class="header-actions">
				{#if $agentStatusLine && $agentStatusLine !== 'WebSocket connection error'}
					<span class="status-line" title={$agentStatusLine}>{$agentStatusLine}</span>
				{/if}
				<button
					class="btn-icon"
					title="Clear conversation"
					onclick={agentClear}
					aria-label="Clear conversation"
				>
					⟲
				</button>
				<button
					class="btn-icon"
					title="Close (Cmd+K)"
					onclick={togglePanel}
					aria-label="Close agent panel"
				>
					✕
				</button>
			</div>
		</header>

		<!-- Error bar -->
		{#if $agentError && $agentError !== 'WebSocket connection error'}
			<div class="error-bar" role="alert">{$agentError}</div>
		{/if}

		<!-- Messages -->
		<div class="message-area">
			{#if $agentMessages.length === 0}
				<div class="empty-state">
					<p class="empty-title">VEIL Runtime Agent</p>
					<p class="empty-hint">
						Ask me to edit code, manage changes, deploy projects, or navigate — I can control the entire veil platform. <kbd>Cmd+K</kbd> to toggle.
					</p>
					<div class="empty-examples">
						<button class="example-chip" onclick={() => agentSend('Open the relay project in the IDE')}>Open relay in IDE</button>
						<button class="example-chip" onclick={() => agentSend('Show me open change requests')}>Open changes</button>
						<button class="example-chip" onclick={() => agentSend('Deploy wear-test to staging')}>Deploy to staging</button>
					</div>
				</div>
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

		<!-- Input -->
		<div class="input-area">
			{#key $agentComposerKey}
				<ChatInput
					onSend={handleSend}
					onAbort={agentAbort}
					isStreaming={$agentIsStreaming}
					placeholder="Ask the agent…"
					initialText={$agentPendingSeed}
				/>
			{/key}
		</div>
	</aside>
{/if}

<style>
	.agent-dock {
		height: 100vh;
		display: flex;
		flex-direction: column;
		flex-shrink: 0;
		background: var(--dk-surface, #12121a);
		border-left: 1px solid var(--dk-border-soft, rgba(42, 42, 56, 0.55));
		z-index: 100;
		box-shadow: -8px 0 32px rgba(0, 0, 0, 0.4);
		animation: slide-in 200ms var(--dk-ease-out, cubic-bezier(0.16, 1, 0.3, 1)) both;
	}

	.agent-dock.resizing {
		user-select: none;
		transition: none;
	}

	@keyframes slide-in {
		from {
			transform: translateX(100%);
			opacity: 0;
		}
		to {
			transform: translateX(0);
			opacity: 1;
		}
	}

	.resize-handle {
		position: absolute;
		left: -3px;
		top: 0;
		bottom: 0;
		width: 6px;
		cursor: col-resize;
		z-index: 10;
		transition: background 140ms ease;
	}
	.resize-handle:hover,
	.agent-dock.resizing .resize-handle {
		background: var(--dk-brand, #148770);
	}

	.dock-header {
		display: flex;
		align-items: center;
		justify-content: space-between;
		padding: 0.6rem 0.85rem;
		border-bottom: 1px solid var(--dk-border-soft, rgba(42, 42, 56, 0.55));
		flex-shrink: 0;
		background: var(--dk-glass, rgba(18, 18, 26, 0.72));
		backdrop-filter: blur(12px);
	}

	.header-left {
		display: flex;
		align-items: center;
		gap: 0.5rem;
	}

	.agent-icon {
		color: var(--dk-brand-light, #1fa88a);
		font-size: 0.9rem;
	}

	.agent-icon.connected {
		animation: pulse-icon 2s ease infinite;
	}

	@keyframes pulse-icon {
		0%, 100% { opacity: 1; }
		50% { opacity: 0.6; }
	}

	.agent-title {
		font-weight: 700;
		font-size: 0.8rem;
		color: var(--dk-text, #f4f5f7);
	}

	.agent-hint {
		color: var(--dk-text-muted, #9ca3af);
		font-size: 0.65rem;
	}

	.header-actions {
		display: flex;
		align-items: center;
		gap: 0.4rem;
	}

	.status-line {
		max-width: 140px;
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
		color: var(--dk-text-muted, #9ca3af);
		font-size: 0.6rem;
	}

	.btn-icon {
		background: none;
		border: none;
		color: var(--dk-text-muted, #9ca3af);
		cursor: pointer;
		padding: 0.25rem 0.4rem;
		border-radius: 4px;
		font-size: 0.8rem;
		transition: color 140ms ease, background 140ms ease;
	}
	.btn-icon:hover {
		color: var(--dk-text, #f4f5f7);
		background: rgba(255, 255, 255, 0.06);
	}

	.error-bar {
		padding: 0.4rem 0.85rem;
		background: rgba(239, 68, 68, 0.12);
		border-bottom: 1px solid rgba(239, 68, 68, 0.4);
		color: #fecaca;
		font-size: 0.75rem;
		flex-shrink: 0;
	}

	.message-area {
		flex: 1;
		min-height: 0;
		overflow-y: auto;
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

	.empty-state {
		padding: 1.5rem 1.25rem;
		display: flex;
		flex-direction: column;
		gap: 0.75rem;
	}

	.empty-title {
		font-weight: 700;
		font-size: 0.95rem;
		color: var(--dk-text, #f4f5f7);
		margin: 0;
	}

	.empty-hint {
		color: var(--dk-text-muted, #9ca3af);
		font-size: 0.8rem;
		line-height: 1.5;
		margin: 0;
	}

	.empty-hint kbd {
		background: var(--dk-surface-3, #22222f);
		border: 1px solid var(--dk-border, #2a2a38);
		padding: 0.1rem 0.35rem;
		border-radius: 4px;
		font-family: var(--dk-font, inherit);
		font-size: 0.7rem;
	}

	.empty-examples {
		display: flex;
		flex-wrap: wrap;
		gap: 0.4rem;
		margin-top: 0.25rem;
	}

	.example-chip {
		background: var(--dk-surface-2, #1a1a26);
		border: 1px solid var(--dk-border-soft, rgba(42, 42, 56, 0.55));
		color: var(--dk-text-muted, #9ca3af);
		font-size: 0.7rem;
		padding: 0.3rem 0.6rem;
		border-radius: 999px;
		cursor: pointer;
		transition: color 140ms ease, border-color 140ms ease, background 140ms ease;
		font-family: inherit;
	}
	.example-chip:hover {
		color: var(--dk-brand-light, #1fa88a);
		border-color: var(--dk-brand, #148770);
		background: rgba(20, 135, 112, 0.08);
	}

	.input-area {
		flex-shrink: 0;
		border-top: 1px solid var(--dk-border-soft, rgba(42, 42, 56, 0.55));
		padding: 0.5rem;
		background: var(--dk-glass, rgba(18, 18, 26, 0.72));
	}

	/* Fix ChatInput from @aether-ui/core which uses Tailwind classes we don't have */
	.input-area :global([role="form"]) {
		padding: 0.6rem 0.75rem;
		border-top: none;
		background: transparent;
	}

	.input-area :global([data-no-inner-focus]) {
		display: flex;
		align-items: center;
		gap: 0.5rem;
		border: 1px solid var(--dk-border-soft, rgba(42, 42, 56, 0.55));
		border-radius: 8px;
		padding: 0.5rem 0.75rem;
		background: var(--dk-surface-2, #1a1a26);
	}

	.input-area :global(textarea) {
		flex: 1;
		resize: none;
		border: none;
		background: transparent;
		color: var(--dk-text, #f4f5f7);
		font-size: 0.85rem;
		line-height: 1.4;
		outline: none;
		padding: 0;
		min-height: 1.4em;
		font-family: inherit;
	}

	.input-area :global(textarea::placeholder) {
		color: var(--dk-text-muted, #9ca3af);
	}

	.input-area :global(button[aria-label="Send message"]),
	.input-area :global(button[aria-label="Stop generating"]),
	.input-area :global(button[aria-label="Interject message"]) {
		background: var(--dk-brand, #148770);
		color: #fff;
		border: none;
		border-radius: 6px;
		padding: 0.35rem 0.75rem;
		font-size: 0.75rem;
		font-weight: 600;
		cursor: pointer;
		white-space: nowrap;
		transition: background 140ms ease;
	}

	.input-area :global(button[aria-label="Send message"]:hover) {
		background: var(--dk-brand-light, #1fa88a);
	}

	.input-area :global(button[aria-label="Send message"]:disabled) {
		opacity: 0.4;
		cursor: not-allowed;
	}

	.input-area :global(button[aria-label="Stop generating"]) {
		background: rgba(239, 68, 68, 0.2);
		color: #fca5a5;
	}

	.input-area :global(button[aria-label="Interject message"]) {
		background: rgba(251, 146, 60, 0.2);
		color: #fdba74;
	}

	.input-area :global(label) {
		cursor: pointer;
		color: var(--dk-text-muted, #9ca3af);
		font-size: 1rem;
		line-height: 1;
		transition: color 140ms ease;
	}

	.input-area :global(label:hover) {
		color: var(--dk-text, #f4f5f7);
	}

	.input-area :global(.sr-only) {
		position: absolute;
		width: 1px;
		height: 1px;
		padding: 0;
		margin: -1px;
		overflow: hidden;
		clip: rect(0, 0, 0, 0);
		border: 0;
	}

	.input-area :global(input[type="file"]) {
		position: absolute;
		width: 1px;
		height: 1px;
		overflow: hidden;
		clip: rect(0, 0, 0, 0);
	}

	/* Attachment preview chips */
	.input-area :global([role="form"] > div:first-child:not([data-no-inner-focus])) {
		display: flex;
		flex-wrap: wrap;
		gap: 0.4rem;
		margin-bottom: 0.4rem;
	}

	/* Hint text below input */
	.input-area :global(p) {
		text-align: center;
		font-size: 0.65rem;
		color: var(--dk-text-muted, #9ca3af);
		margin: 0.4rem 0 0;
		opacity: 0.7;
	}
</style>
