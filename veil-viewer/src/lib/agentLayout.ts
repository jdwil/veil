/**
 * Agent pane placement: bottom dock, left/right of canvas, or separate window.
 * Preference is persisted in localStorage.
 */
import { writable, get } from 'svelte/store';

export type AgentPlacement = 'bottom' | 'left' | 'right' | 'window';

const PLACEMENT_KEY = 'veil.agent.placement';
const WIDTH_KEY = 'veil.agent.sideWidth';
const CHANNEL = 'veil-agent-layout';

export const SIDE_MIN = 280;
export const SIDE_MAX = 720;
export const SIDE_DEFAULT = 380;

function loadPlacement(): AgentPlacement {
	if (typeof localStorage === 'undefined') return 'bottom';
	const v = localStorage.getItem(PLACEMENT_KEY);
	if (v === 'bottom' || v === 'left' || v === 'right' || v === 'window') return v;
	return 'bottom';
}

function loadWidth(): number {
	if (typeof localStorage === 'undefined') return SIDE_DEFAULT;
	const n = Number(localStorage.getItem(WIDTH_KEY));
	if (!Number.isFinite(n)) return SIDE_DEFAULT;
	return Math.min(SIDE_MAX, Math.max(SIDE_MIN, n));
}

/** Where the agent pane lives relative to the canvas. */
export const agentPlacement = writable<AgentPlacement>(loadPlacement());

/** Width of left/right side agent rail (px). */
export const agentSideWidth = writable<number>(loadWidth());

/** Live reference to a pop-out window opened from this page (main only). */
export const agentPopoutRef = writable<Window | null>(null);

/** True when this document is the agent pop-out page. */
export function isAgentPopoutPage(): boolean {
	if (typeof window === 'undefined') return false;
	return (
		window.location.pathname.endsWith('/agent') ||
		window.location.search.includes('agent_popout=1')
	);
}

export function setAgentPlacement(next: AgentPlacement) {
	agentPlacement.set(next);
	try {
		localStorage.setItem(PLACEMENT_KEY, next);
	} catch {
		/* ignore */
	}
	broadcast({ type: 'placement', placement: next });
}

export function setAgentSideWidth(px: number) {
	const w = Math.min(SIDE_MAX, Math.max(SIDE_MIN, Math.round(px)));
	agentSideWidth.set(w);
	try {
		localStorage.setItem(WIDTH_KEY, String(w));
	} catch {
		/* ignore */
	}
}

type LayoutMsg =
	| { type: 'placement'; placement: AgentPlacement }
	| { type: 'popout-ready' }
	| { type: 'popout-closed' }
	| { type: 'focus-main' }
	| { type: 'focus-popout' }
	| { type: 'session-sync'; payload: string };

let channel: BroadcastChannel | null = null;

function getChannel(): BroadcastChannel | null {
	if (typeof BroadcastChannel === 'undefined') return null;
	if (!channel) channel = new BroadcastChannel(CHANNEL);
	return channel;
}

function broadcast(msg: LayoutMsg) {
	try {
		getChannel()?.postMessage(msg);
	} catch {
		/* ignore */
	}
}

/** Subscribe to cross-window layout events (returns unsubscribe). */
export function onAgentLayoutMessage(handler: (msg: LayoutMsg) => void): () => void {
	const ch = getChannel();
	if (!ch) return () => {};
	const fn = (ev: MessageEvent) => {
		const data = ev.data as LayoutMsg;
		if (data && typeof data === 'object' && 'type' in data) handler(data);
	};
	ch.addEventListener('message', fn);
	return () => ch.removeEventListener('message', fn);
}

export function notifyPopoutReady() {
	broadcast({ type: 'popout-ready' });
}

export function notifyPopoutClosed() {
	broadcast({ type: 'popout-closed' });
}

export function requestFocusMain() {
	broadcast({ type: 'focus-main' });
}

/**
 * Open (or focus) the agent in a separate browser window.
 * Returns the Window handle when opened from the main IDE.
 */
export function openAgentPopout(): Window | null {
	if (typeof window === 'undefined') return null;
	const existing = get(agentPopoutRef);
	if (existing && !existing.closed) {
		existing.focus();
		return existing;
	}
	// Handoff chat snapshot so the new window starts mid-conversation
	try {
		// Dynamic import avoided — call via window-level side channel set in agentSession
		const save = (window as unknown as { __veilAgentSaveHandoff?: () => void })
			.__veilAgentSaveHandoff;
		save?.();
	} catch {
		/* ignore */
	}
	const w = Math.min(520, Math.floor(screen.availWidth * 0.35));
	const h = Math.min(800, Math.floor(screen.availHeight * 0.85));
	const left = Math.max(0, screen.availWidth - w - 40);
	const top = 40;
	const features = `popup=yes,width=${w},height=${h},left=${left},top=${top}`;
	const pop = window.open('/agent', 'veil-agent-popout', features);
	if (!pop) {
		// Popup blocked — stay docked
		return null;
	}
	agentPopoutRef.set(pop);
	setAgentPlacement('window');
	return pop;
}

/** Close popout if open and restore a docked placement. */
export function closeAgentPopout(restore: AgentPlacement = 'right') {
	const w = get(agentPopoutRef);
	if (w && !w.closed) {
		try {
			w.close();
		} catch {
			/* ignore */
		}
	}
	agentPopoutRef.set(null);
	if (get(agentPlacement) === 'window') {
		setAgentPlacement(restore === 'window' ? 'right' : restore);
	}
}

/** Labels for UI. */
export const PLACEMENT_OPTIONS: { id: AgentPlacement; label: string; title: string }[] = [
	{ id: 'bottom', label: 'Bottom', title: 'Agent in the bottom review dock (current)' },
	{ id: 'right', label: 'Right', title: 'Agent to the right of the canvas' },
	{ id: 'left', label: 'Left', title: 'Agent to the left of the canvas' },
	{ id: 'window', label: 'Window', title: 'Open agent in a separate browser window' }
];
