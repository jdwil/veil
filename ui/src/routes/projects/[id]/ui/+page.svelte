<script lang="ts">
	/**
	 * "Open UI" — dedicated window running the actual VEIL UI for a project via
	 * the static preview backend (crate::deploy::preview), with the developer
	 * overlay active. Selecting text / right-clicking an element raises a
	 * `veil:edit-intent` (postMessage from the previewed iframe) → we open the
	 * existing inner-agent pane pre-filled with a reference to the selected
	 * construct. Submitting flows through the operator-SDLC/review flow; on an
	 * accepted change the preview rebuilds and the iframe reloads.
	 *
	 * Vibe-code the front-end FROM the front-end.
	 */
	import { page } from '$app/stores';
	import { onMount, onDestroy } from 'svelte';
	import {
		openAgentPanel,
		agentPendingSeed,
		ensureCodingSession,
		setAgentContext,
		patchFocus,
	} from '$lib/agent';

	const projectId = $derived(($page.params.id ?? '').trim());

	type PreviewState = 'idle' | 'building' | 'ready' | 'failed';
	let state: PreviewState = $state('building');
	let errorMsg: string = $state('');
	let iframeSrc: string = $state('');
	let reloadNonce: number = $state(0);
	let lastRef: Record<string, unknown> | null = $state(null);

	let pollTimer: ReturnType<typeof setInterval> | null = null;

	function previewUrl(): string {
		// Public serving route (not under /api) — bust cache on reload.
		return `/preview/${encodeURIComponent(projectId)}/?v=${reloadNonce}`;
	}

	async function triggerBuild() {
		state = 'building';
		errorMsg = '';
		try {
			const r = await fetch(
				`/api/projects/${encodeURIComponent(projectId)}/preview/build`,
				{ method: 'POST' }
			);
			const body = await r.json().catch(() => ({}));
			applyStatus(body);
		} catch (e: unknown) {
			state = 'failed';
			errorMsg = e instanceof Error ? e.message : String(e);
		}
	}

	async function pollStatus() {
		try {
			const r = await fetch(
				`/api/projects/${encodeURIComponent(projectId)}/preview/status`
			);
			if (!r.ok) return;
			const body = await r.json();
			applyStatus(body);
		} catch {
			/* transient — keep polling */
		}
	}

	function applyStatus(body: any) {
		const st = String(body?.state ?? '');
		if (st === 'ready') {
			if (state !== 'ready') {
				state = 'ready';
				reloadNonce += 1;
				iframeSrc = previewUrl();
			}
		} else if (st === 'building') {
			state = 'building';
		} else if (st === 'failed') {
			state = 'failed';
			errorMsg = String(body?.error ?? 'Preview build failed');
		} else if (st === 'idle') {
			// No build yet — kick one off.
			void triggerBuild();
		}
	}

	/** Reload the preview after an accepted change (rebuild → reload iframe). */
	export function refreshPreview() {
		void (async () => {
			await triggerBuild();
			if (state === 'ready') {
				reloadNonce += 1;
				iframeSrc = previewUrl();
			}
		})();
	}

	/** Build the agent prompt seed for an edit intent, referencing the construct. */
	function seedForRef(ref: any): string {
		const construct = ref?.construct ?? 'this component';
		const project = ref?.project ?? projectId;
		const sel = (ref?.selection ?? '').toString().trim();
		const label = (ref?.label ?? '').toString().trim();
		const focus = sel
			? `the selected text “${sel}”`
			: label
				? `the “${label}” element`
				: 'the selected element';
		return `Re: ${construct} in ${project} (ui.veil), ${focus}: `;
	}

	function onEditIntent(ref: any) {
		if (!ref) return;
		lastRef = ref;
		// Seamlessly hand off to the EXISTING inner agent + session history.
		void ensureCodingSession(projectId);
		setAgentContext({ page: `/projects/${projectId}/ui`, project: projectId });
		patchFocus({
			route: `/projects/${projectId}/ui`,
			project: projectId,
			surfaces: [
				{
					kind: 'ui-construct',
					project: ref.project ?? projectId,
					construct: ref.construct ?? null,
					el: ref.el ?? null,
					selection: ref.selection ?? null,
					label: ref.label ?? null,
				},
			],
		});
		agentPendingSeed.set(seedForRef(ref));
		openAgentPanel();
	}

	function onMessage(ev: MessageEvent) {
		const data = ev?.data;
		if (data && typeof data === 'object' && data.type === 'veil:edit-intent') {
			onEditIntent(data.payload);
		}
	}

	onMount(() => {
		if (!projectId) return;
		window.addEventListener('message', onMessage);
		// Also catch the same-window CustomEvent (when not iframed).
		window.addEventListener('veil:edit-intent', (e: any) => onEditIntent(e?.detail));
		setAgentContext({ page: `/projects/${projectId}/ui`, project: projectId });
		void triggerBuild();
		pollTimer = setInterval(() => {
			if (state !== 'ready') void pollStatus();
		}, 2000);
	});

	onDestroy(() => {
		if (pollTimer) clearInterval(pollTimer);
		if (typeof window !== 'undefined') window.removeEventListener('message', onMessage);
	});
</script>

<svelte:head>
	<title>Open UI · {projectId}</title>
</svelte:head>

<div class="ui-window">
	<header class="ui-bar">
		<div class="ui-bar__left">
			<a class="back" href={`/projects/${encodeURIComponent(projectId)}`}>← Project</a>
			<span class="ui-title">Running UI · <code>{projectId}</code></span>
		</div>
		<div class="ui-bar__right">
			{#if state === 'ready'}
				<span class="pill pill--ok">live · edit mode</span>
			{:else if state === 'building'}
				<span class="pill pill--warn">building…</span>
			{:else if state === 'failed'}
				<span class="pill pill--err">build failed</span>
			{/if}
			<button class="btn" onclick={() => refreshPreview()} disabled={state === 'building'}>
				Rebuild
			</button>
		</div>
	</header>

	<div class="ui-hint">
		Select text or right-click any element to open the agent and vibe-code it.
	</div>

	<div class="ui-stage">
		{#if state === 'ready'}
			<iframe
				title="VEIL preview"
				src={iframeSrc}
				class="preview-frame"
			></iframe>
		{:else if state === 'failed'}
			<div class="stage-msg stage-msg--err">
				<h2>Preview build failed</h2>
				<pre>{errorMsg}</pre>
				<button class="btn" onclick={() => triggerBuild()}>Retry build</button>
			</div>
		{:else}
			<div class="stage-msg">
				<div class="spinner"></div>
				<p>Starting preview for <code>{projectId}</code>…</p>
				<p class="muted">First build installs dependencies; later rebuilds are fast.</p>
			</div>
		{/if}
	</div>
</div>

<style>
	.ui-window {
		display: flex;
		flex-direction: column;
		height: 100vh;
		background: var(--dk-surface, #0b0b0c);
		color: var(--dk-text, #e5e5e5);
	}
	.ui-bar {
		display: flex;
		align-items: center;
		justify-content: space-between;
		padding: 0.55rem 0.9rem;
		border-bottom: 1px solid var(--dk-border-soft, #26262b);
		flex-shrink: 0;
	}
	.ui-bar__left,
	.ui-bar__right {
		display: flex;
		align-items: center;
		gap: 0.65rem;
	}
	.back {
		color: inherit;
		text-decoration: none;
		font-size: 0.85rem;
		opacity: 0.85;
	}
	.back:hover {
		opacity: 1;
	}
	.ui-title {
		font-size: 0.9rem;
		font-weight: 600;
	}
	.ui-hint {
		font-size: 0.78rem;
		color: var(--dk-text-muted, #9a9aa2);
		padding: 0.4rem 0.9rem;
		border-bottom: 1px solid var(--dk-border-soft, #26262b);
		flex-shrink: 0;
	}
	.ui-stage {
		flex: 1;
		min-height: 0;
		position: relative;
		background: #fff;
	}
	.preview-frame {
		width: 100%;
		height: 100%;
		border: 0;
		display: block;
	}
	.stage-msg {
		position: absolute;
		inset: 0;
		display: flex;
		flex-direction: column;
		align-items: center;
		justify-content: center;
		gap: 0.5rem;
		background: var(--dk-surface, #0b0b0c);
		color: var(--dk-text, #e5e5e5);
		text-align: center;
		padding: 2rem;
	}
	.stage-msg pre {
		max-width: 90%;
		overflow: auto;
		text-align: left;
		background: rgba(0, 0, 0, 0.3);
		padding: 0.75rem 1rem;
		border-radius: 6px;
		font-size: 0.8rem;
	}
	.stage-msg--err h2 {
		color: var(--dk-red, #ef4444);
		margin: 0;
	}
	.muted {
		color: var(--dk-text-muted, #9a9aa2);
		font-size: 0.82rem;
	}
	.spinner {
		width: 30px;
		height: 30px;
		border: 3px solid rgba(255, 255, 255, 0.15);
		border-top-color: var(--dk-accent, #6ea8fe);
		border-radius: 50%;
		animation: spin 0.8s linear infinite;
	}
	@keyframes spin {
		to {
			transform: rotate(360deg);
		}
	}
	.pill {
		font-size: 0.72rem;
		font-weight: 600;
		border-radius: 999px;
		padding: 0.15rem 0.55rem;
		border: 1px solid var(--dk-border-soft, #333);
	}
	.pill--ok {
		color: #22c55e;
		border-color: color-mix(in srgb, #22c55e 45%, transparent);
	}
	.pill--warn {
		color: #f59e0b;
		border-color: color-mix(in srgb, #f59e0b 45%, transparent);
	}
	.pill--err {
		color: #ef4444;
		border-color: color-mix(in srgb, #ef4444 45%, transparent);
	}
	.btn {
		font: inherit;
		font-size: 0.82rem;
		border: 1px solid var(--dk-border-soft, #333);
		background: transparent;
		color: inherit;
		border-radius: 6px;
		padding: 0.3rem 0.7rem;
		cursor: pointer;
	}
	.btn:hover:not(:disabled) {
		background: rgba(255, 255, 255, 0.06);
	}
	.btn:disabled {
		opacity: 0.5;
		cursor: default;
	}
</style>
