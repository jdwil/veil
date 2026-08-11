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
		agentPanelMinimized,
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
		// Close fully (not minimize)
		agentPanelOpen.set(false);
		agentPanelMinimized.set(false);
	}

	function minimizePanel() {
		agentPanelMinimized.set(true);
	}

	function expandPanel() {
		agentPanelMinimized.set(false);
		agentUnreadCount.set(0);
	}

	function handleSend(content: string, attachments?: File[]) {
		agentSend(content, attachments);
	}

	/** Auto-scroll chat unless the user has scrolled up intentionally. */
	let messageAreaEl: HTMLDivElement | null = $state(null);
	let userPausedAutoScroll = $state(false);
	let showJumpLatest = $state(false);

	function onMessageAreaScroll() {
		const el = messageAreaEl;
		if (!el) return;
		// Prefer the MessageList scroller if present
		const scroller =
			(el.querySelector('[role="log"]') as HTMLElement | null) ?? el;
		const dist = scroller.scrollHeight - scroller.scrollTop - scroller.clientHeight;
		userPausedAutoScroll = dist > 40;
		showJumpLatest = dist > 120;
	}

	function scrollChatToBottom() {
		const el = messageAreaEl;
		if (!el) return;
		const scroller =
			(el.querySelector('[role="log"]') as HTMLElement | null) ?? el;
		scroller.scrollTop = scroller.scrollHeight;
	}

	function resumeChatAutoScroll() {
		userPausedAutoScroll = false;
		showJumpLatest = false;
		requestAnimationFrame(() => scrollChatToBottom());
	}

	// Follow stream/tool updates unless paused
	$effect(() => {
		const msgs = $agentMessages;
		const streaming = $agentIsStreaming;
		const thinking = $agentIsThinking;
		// Depend on nested content length for deltas
		let fp = `${msgs.length}:${streaming}:${thinking}`;
		for (const m of msgs) {
			fp += `|${m.id}:${m.content?.length ?? 0}`;
			for (const b of m.content ?? []) {
				if (b.type === 'text' && 'text' in b) fp += (b as { text: string }).text.length;
				if (b.type === 'tool_call' && 'toolCall' in b) {
					const tc = (b as { toolCall: { arguments?: string; status?: string } }).toolCall;
					fp += `${tc?.status ?? ''}:${(tc?.arguments ?? '').length}`;
				}
			}
		}
		void fp;
		if (userPausedAutoScroll) return;
		requestAnimationFrame(() => {
			requestAnimationFrame(() => {
				if (!userPausedAutoScroll) scrollChatToBottom();
			});
		});
	});
</script>

{#if $agentPanelOpen}
	{#if $agentPanelMinimized}
		<aside
			class="agent-dock agent-dock--minimized"
			role="complementary"
			aria-label="AI Agent (minimized)"
		>
			<button
				type="button"
				class="min-strip"
				title="Expand agent panel (Cmd+K)"
				onclick={expandPanel}
				aria-label="Expand agent panel"
			>
				<span class="agent-icon" class:connected={isAcpConnected()}>◆</span>
				<span class="min-label">Agent</span>
				{#if $agentUnreadCount > 0}
					<span class="min-badge" aria-label="{$agentUnreadCount} unread">{$agentUnreadCount}</span>
				{:else if $agentIsStreaming || $agentIsThinking}
					<span class="min-pulse" title="Agent running">●</span>
				{/if}
			</button>
			<button
				type="button"
				class="btn-icon min-close"
				title="Close agent panel"
				onclick={togglePanel}
				aria-label="Close agent panel"
			>
				✕
			</button>
		</aside>
	{:else}
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
					title="Minimize agent panel"
					onclick={minimizePanel}
					aria-label="Minimize agent panel"
				>
					—
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
		<div
			class="message-area"
			bind:this={messageAreaEl}
			onscroll={onMessageAreaScroll}
		>
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
				<div class="msg-list" onscroll={onMessageAreaScroll}>
					<MessageList
						messages={$agentMessages}
						isStreaming={$agentIsStreaming}
						isThinking={$agentIsThinking}
					/>
				</div>
				{#if showJumpLatest}
					<button type="button" class="jump-latest" onclick={resumeChatAutoScroll}>
						↓ Latest
					</button>
				{/if}
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

	.agent-dock--minimized {
		width: 44px;
		flex: 0 0 44px;
		align-items: center;
		padding: 0.5rem 0.2rem;
		gap: 0.35rem;
		background: var(--dk-glass, rgba(26, 26, 26, 0.92));
	}

	.min-strip {
		flex: 1;
		min-height: 0;
		width: 100%;
		display: flex;
		flex-direction: column;
		align-items: center;
		gap: 0.55rem;
		padding: 0.65rem 0.15rem;
		border: none;
		border-radius: 8px;
		background: transparent;
		color: var(--dk-text-muted, #9ca3af);
		cursor: pointer;
		transition: background 140ms ease, color 140ms ease;
	}
	.min-strip:hover {
		background: rgba(255, 255, 255, 0.06);
		color: var(--dk-text, #e5e5e5);
	}
	.min-label {
		writing-mode: vertical-rl;
		text-orientation: mixed;
		transform: rotate(180deg);
		font-size: 0.7rem;
		font-weight: 700;
		letter-spacing: 0.06em;
		text-transform: uppercase;
	}
	.min-badge {
		min-width: 1.15rem;
		height: 1.15rem;
		padding: 0 0.25rem;
		border-radius: 999px;
		background: var(--dk-brand, #3b82f6);
		color: #fff;
		font-size: 0.6rem;
		font-weight: 700;
		display: inline-flex;
		align-items: center;
		justify-content: center;
	}
	.min-pulse {
		color: #34d399;
		font-size: 0.65rem;
		animation: pulse-icon 1.2s ease infinite;
	}
	.min-close {
		flex-shrink: 0;
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
		/* MessageList owns scrolling — avoid nested overflow fighting auto-scroll */
		overflow: hidden;
		display: flex;
		flex-direction: column;
		position: relative;
	}

	.jump-latest {
		position: absolute;
		bottom: 0.65rem;
		left: 50%;
		transform: translateX(-50%);
		z-index: 5;
		padding: 0.3rem 0.75rem;
		border-radius: 999px;
		border: 1px solid var(--dk-border, #2e2e2e);
		background: var(--dk-surface-2, #242424);
		color: var(--dk-text, #e5e5e5);
		font-size: 0.7rem;
		font-weight: 600;
		cursor: pointer;
		box-shadow: 0 4px 12px rgba(0, 0, 0, 0.35);
	}
	.jump-latest:hover {
		border-color: var(--dk-brand, #737373);
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

	/* Breathing room between stacked tool-use cards */
	.msg-list :global(.tool-call-card) {
		margin-top: 0.2rem;
		margin-bottom: 0.35rem;
	}
	.msg-list :global(.tool-call-card + .tool-call-card) {
		margin-top: 0.45rem;
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
		padding: 0.65rem 0.75rem;
		background: var(--dk-glass, rgba(26, 26, 26, 0.78));
	}

	/* Fix ChatInput from @aether-ui/core which uses Tailwind classes we don't have */
	.input-area :global([role="form"]) {
		padding: 0;
		border-top: none;
		background: transparent;
	}

	.input-area :global([data-no-inner-focus]) {
		display: flex;
		align-items: center;
		gap: 0.6rem;
		border: 1px solid var(--dk-border-soft, rgba(46, 46, 46, 0.65));
		border-radius: 10px;
		/* Equal inset; slightly more horizontal so text isn't tight to the border */
		padding: 0.6rem 0.85rem 0.6rem 0.9rem;
		background: var(--dk-surface-2, #242424);
		min-height: 3rem;
		box-sizing: border-box;
	}

	.input-area :global(textarea) {
		flex: 1;
		resize: none;
		border: none;
		background: transparent;
		color: var(--dk-text, #e5e5e5);
		font-size: 0.9rem;
		line-height: 1.45;
		outline: none;
		/* Symmetric padding — left matches right so placeholder isn't cramped after 📎 */
		padding: 0.45rem 0.5rem;
		min-height: 2.5rem;
		max-height: 10rem;
		font-family: inherit;
		box-sizing: border-box;
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
		padding: 0.5rem 0.9rem;
		font-size: 0.8rem;
		font-weight: 600;
		cursor: pointer;
		white-space: nowrap;
		transition: background 140ms ease;
		margin: 0;
		align-self: center;
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
		font-size: 1.1rem;
		line-height: 1;
		transition: color 140ms ease;
		align-self: center;
		padding: 0;
		margin: 0;
		flex-shrink: 0;
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
