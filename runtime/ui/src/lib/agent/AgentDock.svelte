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
		agentInsertToken,
		ideDiagnosticsSummary,
		getAgentContext,
		getCodingSessionId,
		ensureCodingSession
	} from '$lib/agent/runtimeAgentSession';

	// Chips seed the agent; navigation must come from agent tools, not hard-coded Svelte routes.

	function investigateAllIssuesPrompt(): string {
		const sum = $ideDiagnosticsSummary;
		const project = sum.project || getAgentContext().project || 'the open project';
		const lines = (sum.sample ?? []).map((d, i) => {
			const code = d.code ? ` [${d.code}]` : '';
			const where = d.node_name ? ` @ ${d.node_name}` : '';
			const hint = d.hint ? `\n   Hint: ${d.hint}` : '';
			return `${i + 1}. ${d.severity ?? 'Issue'}${code}${where}: ${d.message ?? ''}${hint}`;
		});
		const more =
			sum.count > lines.length
				? `\n…and ${sum.count - lines.length} more (use project diagnostics / IDE tools to list all).`
				: '';
		return [
			`Investigate and fix all open issues in project \`${project}\`.`,
			'',
			'Use IDE/project tools: read source, apply minimal correct edits, re-run checks.',
			'',
			'## Sample issues',
			lines.join('\n') || '(open the IDE diagnostics panel for the full list)',
			more
		].join('\n');
	}

	const WIDTH_KEY = 'veil.agent.dockWidth';
	const MIN_WIDTH = 320;
	const DEFAULT_WIDTH = 420;

	/** Allow the agent to claim most of the viewport so the IDE can shrink. */
	function maxWidth(): number {
		if (typeof window === 'undefined') return 900;
		return Math.max(MIN_WIDTH, Math.min(1100, Math.floor(window.innerWidth * 0.72)));
	}

	function clampWidth(n: number): number {
		return Math.min(maxWidth(), Math.max(MIN_WIDTH, Math.round(n)));
	}

	function loadWidth(): number {
		if (typeof localStorage === 'undefined') return DEFAULT_WIDTH;
		const n = Number(localStorage.getItem(WIDTH_KEY));
		if (!Number.isFinite(n) || n <= 0) return DEFAULT_WIDTH;
		return clampWidth(n);
	}

	function saveWidth(px: number) {
		try {
			localStorage.setItem(WIDTH_KEY, String(clampWidth(px)));
		} catch {
			/* ignore quota / private mode */
		}
	}

	// Restore saved width immediately on the client (avoid 420 flash then snap).
	let panelWidth = $state(
		typeof window !== 'undefined' ? loadWidth() : DEFAULT_WIDTH
	);
	let isResizing = $state(false);

	$effect(() => {
		// Re-clamp if the window shrinks below the saved width.
		const onResize = () => {
			panelWidth = clampWidth(panelWidth);
		};
		window.addEventListener('resize', onResize);
		// Ensure localStorage is applied after SSR/hydration.
		panelWidth = loadWidth();
		return () => window.removeEventListener('resize', onResize);
	});

	// Provider status polling
	interface ProviderStatus {
		provider: string;
		acp_tunnel: { connected: boolean; agents: Array<{ agent_name: string }> };
		sessions?: {
			enabled?: boolean;
			open?: Array<{ session_id: string; slug: string; revision: number }>;
			user_id?: string;
		};
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

	// Keep coding session warm when panel opens with a project context
	$effect(() => {
		if (!$agentPanelOpen) return;
		const proj = getAgentContext().project;
		if (proj) void ensureCodingSession(proj);
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

	function sessionHint(): string {
		const sid = getCodingSessionId();
		if (!sid) {
			return providerStatus?.sessions?.enabled ? 'No session' : '';
		}
		const open = providerStatus?.sessions?.open || [];
		const match = open.find((s) => s.session_id === sid);
		if (match) return `${match.slug} · r${match.revision}`;
		return `sess ${sid.slice(0, 8)}…`;
	}

	function startResize(e: PointerEvent) {
		// Pointer capture + body class so the IDE iframe cannot steal moves mid-drag.
		e.preventDefault();
		e.stopPropagation();
		const handle = e.currentTarget as HTMLElement;
		const pointerId = e.pointerId;
		try {
			handle.setPointerCapture(pointerId);
		} catch {
			/* ignore */
		}
		isResizing = true;
		document.body.classList.add('agent-dock-resizing');
		const startX = e.clientX;
		const startWidth = panelWidth;
		const cap = maxWidth();
		let latest = startWidth;

		function onMove(ev: PointerEvent) {
			if (ev.pointerId !== pointerId) return;
			const delta = startX - ev.clientX; // drag left → wider dock
			latest = Math.max(MIN_WIDTH, Math.min(cap, startWidth + delta));
			panelWidth = latest;
		}
		function onUp(ev: PointerEvent) {
			if (ev.pointerId !== pointerId) return;
			isResizing = false;
			document.body.classList.remove('agent-dock-resizing');
			try {
				handle.releasePointerCapture(pointerId);
			} catch {
				/* ignore */
			}
			handle.removeEventListener('pointermove', onMove);
			handle.removeEventListener('pointerup', onUp);
			handle.removeEventListener('pointercancel', onUp);
			// Persist the final drag width (use local latest — avoids stale $state in closure).
			panelWidth = latest;
			saveWidth(latest);
		}
		handle.addEventListener('pointermove', onMove);
		handle.addEventListener('pointerup', onUp);
		handle.addEventListener('pointercancel', onUp);
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
		<!-- Resize handle (pointer events — survives drag over IDE iframe) -->
		<div
			class="resize-handle"
			role="separator"
			aria-orientation="vertical"
			aria-valuenow={panelWidth}
			aria-valuemin={MIN_WIDTH}
			title="Drag to resize agent panel"
			onpointerdown={startResize}
		></div>

		<!-- Header -->
		<header class="dock-header">
			<div class="header-left">
				<span class="agent-icon" class:connected={isAcpConnected()}>◆</span>
				<span class="agent-title">Agent</span>
				<span class="agent-hint">{providerLabel()}</span>
				{#if sessionHint()}
					<span class="session-hint" title="Durable coding session">{sessionHint()}</span>
				{/if}
			</div>
			<div class="header-actions">
				{#if $agentStatusLine}
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
		{#if $agentError}
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
						{#if $ideDiagnosticsSummary.count > 0}
							<button
								class="example-chip example-chip--issues"
								onclick={() => agentSend(investigateAllIssuesPrompt())}
							>
								Investigate & fix all issues ({$ideDiagnosticsSummary.count})
							</button>
						{:else}
							<button
								class="example-chip"
								onclick={() =>
									agentSend(
										'Summarize the current project and any open diagnostics or TODOs. Use project/IDE tools as needed.'
									)}
							>
								Review current project
							</button>
							<button
								class="example-chip"
								onclick={() =>
									agentSend(
										'Show open change requests — use list_changes so the UI navigates to /changes'
									)}
							>
								Open change requests
							</button>
						{/if}
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
		/* Flex sibling of .content — must NOT overlay the IDE */
		position: relative;
		height: 100%;
		max-height: 100vh;
		display: flex;
		flex-direction: column;
		flex: 0 0 auto;
		flex-shrink: 0;
		min-width: 0;
		/* width set inline; grow/shrink only via resize handle */
		background: var(--dk-surface, #1a1a1a);
		border-left: 1px solid var(--dk-border-soft, rgba(46, 46, 46, 0.65));
		z-index: 2; /* above content chrome only, not a full-screen overlay */
		/* no heavy shadow — reads as a panel, not a modal over the IDE */
		box-shadow: none;
	}

	.agent-dock.resizing {
		user-select: none;
		transition: none;
	}

	/* While resizing, kill iframe hit-testing so the IDE cannot eat pointermoves. */
	:global(body.agent-dock-resizing) {
		cursor: col-resize !important;
		user-select: none !important;
	}
	:global(body.agent-dock-resizing iframe) {
		pointer-events: none !important;
	}
	:global(body.agent-dock-resizing *) {
		cursor: col-resize !important;
	}

	/* No slide-in translate — that painted over the IDE and felt like a cover */

	.resize-handle {
		position: absolute;
		left: -5px;
		top: 0;
		bottom: 0;
		width: 10px;
		cursor: col-resize;
		z-index: 20;
		touch-action: none;
		transition: background 140ms ease;
	}
	.resize-handle:hover,
	.agent-dock.resizing .resize-handle {
		background: color-mix(in srgb, var(--dk-brand, #737373) 55%, transparent);
	}

	.dock-header {
		display: flex;
		align-items: center;
		justify-content: space-between;
		padding: 0.6rem 0.85rem;
		border-bottom: 1px solid var(--dk-border-soft, rgba(46, 46, 46, 0.65));
		flex-shrink: 0;
		background: var(--dk-glass, rgba(26, 26, 26, 0.78));
		backdrop-filter: blur(12px);
	}

	.header-left {
		display: flex;
		align-items: center;
		gap: 0.5rem;
	}

	.agent-icon {
		color: var(--dk-brand-light, #a3a3a3);
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
		color: var(--dk-text, #e5e5e5);
	}

	.session-hint {
		font-size: 0.65rem;
		color: var(--dk-text-muted, #737373);
		background: color-mix(in srgb, var(--dk-brand, #525252) 20%, transparent);
		border: 1px solid var(--dk-border-soft, rgba(46, 46, 46, 0.65));
		border-radius: 999px;
		padding: 0.1rem 0.4rem;
		max-width: 9rem;
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
		font-variant-numeric: tabular-nums;
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
		color: var(--dk-text, #e5e5e5);
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
		color: var(--dk-text, #e5e5e5);
		margin: 0;
	}

	.empty-hint {
		color: var(--dk-text-muted, #9ca3af);
		font-size: 0.8rem;
		line-height: 1.5;
		margin: 0;
	}

	.empty-hint kbd {
		background: var(--dk-surface-3, #2e2e2e);
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
		background: var(--dk-surface-2, #242424);
		border: 1px solid var(--dk-border-soft, rgba(46, 46, 46, 0.65));
		color: var(--dk-text-muted, #9ca3af);
		font-size: 0.7rem;
		padding: 0.3rem 0.6rem;
		border-radius: 999px;
		cursor: pointer;
		transition: color 140ms ease, border-color 140ms ease, background 140ms ease;
		font-family: inherit;
	}
	.example-chip--issues {
		border-color: color-mix(in srgb, #f59e0b 50%, var(--veil-border, #404040));
		background: color-mix(in srgb, #f59e0b 12%, transparent);
	}

	.example-chip:hover {
		color: var(--dk-brand-light, #a3a3a3);
		border-color: var(--dk-brand, #737373);
		background: rgba(115, 115, 115, 0.08);
	}

	.input-area {
		flex-shrink: 0;
		border-top: 1px solid var(--dk-border-soft, rgba(46, 46, 46, 0.65));
		padding: 0.5rem;
		background: var(--dk-glass, rgba(26, 26, 26, 0.78));
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
		border: 1px solid var(--dk-border-soft, rgba(46, 46, 46, 0.65));
		border-radius: 8px;
		padding: 0.5rem 0.75rem;
		background: var(--dk-surface-2, #242424);
	}

	.input-area :global(textarea) {
		flex: 1;
		resize: none;
		border: none;
		background: transparent;
		color: var(--dk-text, #e5e5e5);
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
		background: var(--dk-brand, #737373);
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
		background: var(--dk-brand-light, #a3a3a3);
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
		color: var(--dk-text, #e5e5e5);
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
