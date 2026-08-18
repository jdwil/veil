<script lang="ts">
	/**
	 * AgentDock — persistent slide-out panel for the runtime agent.
	 * Lives in root layout, persists across all route navigation.
	 * Resizable via drag handle on the left edge.
	 */
	import { MessageList, ChatInput, ToolCallBlock } from '@aether-ui/core';
	import { reviewOutstandingCount, refreshReview } from '$lib/review/store';
	import { presentFastForward, setPresentFastForward } from '$lib/agent/intent';
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
		agentPendingAttachments,
		agentAddAttachments,
		agentRemovePendingAttachment,
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
	/** Sidebar (~240) + a usable IDE column. Dock may take the rest. */
	const SHELL_RESERVED = 720;

	/** Cap so the IDE keeps a usable column; no 42vw/560 hard ceiling. */
	function maxWidth(): number {
		if (typeof window === 'undefined') return 720;
		return Math.max(MIN_WIDTH, window.innerWidth - SHELL_RESERVED);
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
		// Primary button only (ignore right-click / extra buttons).
		if (e.pointerType === 'mouse' && e.button !== 0) return;
		e.preventDefault();
		e.stopPropagation();
		const handle = e.currentTarget as HTMLElement;
		const pointerId = e.pointerId;
		try {
			handle.setPointerCapture(pointerId);
		} catch {
			/* optional — window capture-phase listeners are the source of truth */
		}
		isResizing = true;
		document.body.classList.add('agent-dock-resizing');
		const startX = e.clientX;
		const startWidth = panelWidth;
		let latest = startWidth;
		let finished = false;

		// Window + capture:true so the native IDE (same document, not an iframe)
		// cannot eat pointermove once the cursor leaves the 12px handle.
		const onMove = (ev: PointerEvent) => {
			if (ev.pointerId !== pointerId) return;
			ev.preventDefault();
			ev.stopPropagation();
			const cap = maxWidth();
			const delta = startX - ev.clientX; // drag left → wider dock
			latest = Math.max(MIN_WIDTH, Math.min(cap, startWidth + delta));
			panelWidth = latest;
		};
		const finish = (ev: Event) => {
			if (ev instanceof PointerEvent && ev.pointerId !== pointerId) return;
			if (finished) return;
			finished = true;
			isResizing = false;
			document.body.classList.remove('agent-dock-resizing');
			try {
				handle.releasePointerCapture(pointerId);
			} catch {
				/* ignore */
			}
			window.removeEventListener('pointermove', onMove, true);
			window.removeEventListener('pointerup', finish, true);
			window.removeEventListener('pointercancel', finish, true);
			panelWidth = latest;
			saveWidth(latest);
		};
		window.addEventListener('pointermove', onMove, { capture: true });
		window.addEventListener('pointerup', finish, { capture: true });
		window.addEventListener('pointercancel', finish, { capture: true });
	}

	let skipAnim = $state(
		typeof localStorage !== 'undefined' && presentFastForward()
	);

	function toggleSkipAnim() {
		skipAnim = !skipAnim;
		setPresentFastForward(skipAnim);
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
		void agentSend(content, attachments);
	}

	/** Whole-pane drop (not just the ChatInput strip). Capture so ChatInput does not double-add. */
	let dockEl: HTMLElement | null = $state(null);
	let isFileDrag = $state(false);

	function hasFileDrag(e: DragEvent): boolean {
		const types = e.dataTransfer?.types;
		if (!types) return false;
		return Array.from(types).includes('Files');
	}

	$effect(() => {
		const el = dockEl;
		if (!el) return;
		let hideTimer: ReturnType<typeof setTimeout> | null = null;
		const armHide = () => {
			if (hideTimer) clearTimeout(hideTimer);
			hideTimer = setTimeout(() => {
				isFileDrag = false;
			}, 80);
		};
		const onEnter = (e: DragEvent) => {
			if (!hasFileDrag(e)) return;
			e.preventDefault();
			e.stopPropagation();
			if (hideTimer) clearTimeout(hideTimer);
			isFileDrag = true;
			if (e.dataTransfer) e.dataTransfer.dropEffect = 'copy';
		};
		const onOver = (e: DragEvent) => {
			if (!hasFileDrag(e)) return;
			e.preventDefault();
			e.stopPropagation();
			if (e.dataTransfer) e.dataTransfer.dropEffect = 'copy';
			if (hideTimer) clearTimeout(hideTimer);
			isFileDrag = true;
		};
		const onLeave = (e: DragEvent) => {
			if (!hasFileDrag(e)) return;
			e.preventDefault();
			armHide();
		};
		const onDrop = (e: DragEvent) => {
			if (!hasFileDrag(e)) return;
			e.preventDefault();
			e.stopPropagation();
			if (hideTimer) clearTimeout(hideTimer);
			isFileDrag = false;
			const files = Array.from(e.dataTransfer?.files ?? []);
			if (files.length) agentAddAttachments(files);
		};
		const onPaste = (e: ClipboardEvent) => {
			const files = Array.from(e.clipboardData?.files ?? []);
			if (!files.length) return;
			e.preventDefault();
			agentAddAttachments(files);
		};
		el.addEventListener('dragenter', onEnter, true);
		el.addEventListener('dragover', onOver, true);
		el.addEventListener('dragleave', onLeave, true);
		el.addEventListener('drop', onDrop, true);
		el.addEventListener('paste', onPaste);
		return () => {
			if (hideTimer) clearTimeout(hideTimer);
			el.removeEventListener('dragenter', onEnter, true);
			el.removeEventListener('dragover', onOver, true);
			el.removeEventListener('dragleave', onLeave, true);
			el.removeEventListener('drop', onDrop, true);
			el.removeEventListener('paste', onPaste);
		};
	});

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
		class:file-drag={isFileDrag}
		style="width: {panelWidth}px"
		class:resizing={isResizing}
		role="complementary"
		aria-label="AI Agent"
		bind:this={dockEl}
	>
		{#if isFileDrag}
			<div class="drop-overlay" aria-hidden="true">
				<span>Drop documents to attach</span>
			</div>
		{/if}
		<!-- Resize handle: window capture + shield so the native IDE cannot steal moves. -->
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
					class:ff-on={skipAnim}
					title={skipAnim ? 'Play Present at human speed' : 'Skip Present animation (power user)'}
					onclick={toggleSkipAnim}
					aria-label="Toggle Present fast-forward"
					aria-pressed={skipAnim}
				>
					{skipAnim ? '»' : '▸'}
				</button>
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

		{#if $reviewOutstandingCount > 0}
			<a class="outstanding-strip" href="/review">
				{$reviewOutstandingCount}
				{#if $reviewOutstandingCount === 1}change{:else}changes{/if}
				 to review
			</a>
		{/if}

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
						Ask me to edit code, manage changes, deploy projects, or navigate — I can control the entire veil platform. Drop diagrams, ERDs, or docs onto this pane (or paste) and send them with your prompt. <kbd>Cmd+K</kbd> to toggle.
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
										'Show open pull requests — use list_prs so the UI navigates to /pulls'
									)}
							>
								Open pull requests
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
			{#if $agentPendingAttachments.length}
				<div class="pending-atts" aria-label="Pending attachments">
					{#each $agentPendingAttachments as file, i (file.name + file.size + i)}
						<span class="att-chip" title={`${file.name} (${file.size} bytes)`}>
							<span class="att-name">{file.name}</span>
							<button
								type="button"
								class="att-remove"
								aria-label="Remove {file.name}"
								onclick={() => agentRemovePendingAttachment(i)}
							>
								×
							</button>
						</span>
					{/each}
				</div>
			{/if}
			{#key $agentComposerKey}
				<ChatInput
					onSend={handleSend}
					onAbort={agentAbort}
					isStreaming={$agentIsStreaming}
					placeholder="Ask the agent… or drop files"
					initialText={$agentPendingSeed}
				/>
			{/key}
		</div>
	</aside>
	{/if}
{/if}
{#if isResizing}
	<div class="resize-shield" aria-hidden="true"></div>
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
		/* visible so the seam handle (left: -6px) stays hittable; panes clip themselves */
		overflow: visible;
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

	.agent-dock.file-drag {
		outline: 2px dashed var(--dk-brand-light, #a3a3a3);
		outline-offset: -4px;
	}

	.drop-overlay {
		position: absolute;
		inset: 0;
		z-index: 30;
		display: flex;
		align-items: center;
		justify-content: center;
		pointer-events: none;
		background: color-mix(in srgb, var(--dk-brand, #737373) 18%, rgba(0, 0, 0, 0.45));
		color: var(--dk-text, #e5e5e5);
		font-size: 0.95rem;
		font-weight: 700;
		letter-spacing: 0.02em;
	}

	.pending-atts {
		display: flex;
		flex-wrap: wrap;
		gap: 0.4rem;
		margin-bottom: 0.45rem;
	}

	.att-chip {
		display: inline-flex;
		align-items: center;
		gap: 0.35rem;
		max-width: 100%;
		padding: 0.2rem 0.45rem 0.2rem 0.55rem;
		border-radius: 999px;
		background: var(--dk-surface-2, #242424);
		border: 1px solid var(--dk-border-soft, rgba(46, 46, 46, 0.65));
		color: var(--dk-text, #e5e5e5);
		font-size: 0.7rem;
	}

	.att-name {
		max-width: 14rem;
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}

	.att-remove {
		border: none;
		background: transparent;
		color: var(--dk-text-muted, #9ca3af);
		cursor: pointer;
		font-size: 0.9rem;
		line-height: 1;
		padding: 0 0.1rem;
	}
	.att-remove:hover {
		color: #fca5a5;
	}

	/* While resizing, kill hit-testing on the native IDE (not an iframe). */
	:global(body.agent-dock-resizing) {
		cursor: col-resize !important;
		user-select: none !important;
	}
	:global(body.agent-dock-resizing iframe),
	:global(body.agent-dock-resizing .native-ide),
	:global(body.agent-dock-resizing .viewer-container),
	:global(body.agent-dock-resizing .content) {
		pointer-events: none !important;
	}
	:global(body.agent-dock-resizing *) {
		cursor: col-resize !important;
	}

	/* Full-viewport interceptor: sits above the IDE so moves never hit the editor. */
	.resize-shield {
		position: fixed;
		inset: 0;
		z-index: 2147483646;
		cursor: col-resize;
		touch-action: none;
	}

	/* No slide-in translate — that painted over the IDE and felt like a cover */

	.resize-handle {
		position: absolute;
		left: -6px;
		top: 0;
		bottom: 0;
		width: 12px;
		cursor: col-resize;
		z-index: 40;
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

	.btn-icon.ff-on {
		color: var(--dk-accent, #818cf8);
	}
	.outstanding-strip {
		display: block;
		padding: 0.4rem 0.85rem;
		font-size: 0.78rem;
		font-weight: 600;
		color: var(--dk-text, #f4f4f5);
		background: color-mix(in srgb, var(--dk-amber, #f59e0b) 18%, transparent);
		border-bottom: 1px solid color-mix(in srgb, var(--dk-amber, #f59e0b) 40%, transparent);
		text-decoration: none;
	}
	.outstanding-strip:hover {
		filter: brightness(1.08);
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

	/* MessageList is Tailwind-only — keep scroll if @source misses a rebuild */
	.msg-list :global([role='log']) {
		flex: 1 1 auto;
		min-height: 0;
		overflow-y: auto;
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
