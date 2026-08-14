/**
 * Intent + Present — discrete commands with optional UX choreography.
 *
 * Agents emit Intents via tools; the shell IntentExecutor runs Present steps
 * (goto, fill, pulse) so the operator sees what happened.
 *
 * Domain mutations for coding stay Agent→Server; product-visible ops prefer
 * Present-first storytelling (see ADR_FOCUS_INTENT_PRESENT.md).
 */
import { goto } from '$app/navigation';
import { get } from 'svelte/store';
import { patchFocus } from './focus';

// ─── Types ───────────────────────────────────────────────────────────────────

export type PresentStep =
	| { kind: 'goto'; path: string; ms?: number; project?: string }
	| {
			kind: 'fill';
			formId?: string;
			fields: Record<string, unknown>;
			mode?: 'snap' | 'type';
			ms?: number;
	  }
	| {
			kind: 'pulse';
			/** CSS selector, or role key: submit | create-form | field:{id} */
			target?: string;
			selector?: string;
			ms?: number;
	  }
	| { kind: 'wait'; ms: number }
	| { kind: 'announce'; message: string }
	| {
			kind: 'commit';
			formId?: string;
			method?: string;
			path: string;
			body?: Record<string, unknown>;
			/** e.g. /projects/{slug}/ide — fills from commit response */
			resultPathTemplate?: string;
			ms?: number;
	  }
	| {
			/** Modal: pick open PR or create new (coding target resolve) */
			kind: 'choose';
			title?: string;
			message?: string;
			options: Array<{
				id: string;
				label: string;
				detail?: string;
				source_branch?: string | null;
			}>;
			ms?: number;
	  };

export type Present = {
	steps: PresentStep[];
	announce?: string;
};

export type IntentDomain = {
	/** server = domain already applied; ux = FE should commit; none = present-only */
	mode: 'server' | 'ux' | 'none';
	done?: boolean;
};

export type Intent = {
	type: string;
	id: string;
	actor: 'agent' | 'human' | 'system';
	payload?: Record<string, unknown>;
	present?: Present;
	domain?: IntentDomain;
	/** Legacy SPA nav (fallback when present is empty) */
	navigation?: {
		action: 'goto' | 'open-ide' | 'switch-project' | 'open-panel';
		path?: string;
		project?: string;
	};
};

export type IntentStatus = {
	intentId: string;
	ok: boolean;
	step?: number;
	error?: string;
	/** Domain result from commit step (slug, path, …) */
	result?: Record<string, unknown>;
};

// ─── Intent log (agent + human) ──────────────────────────────────────────────

export type IntentLogEntry = {
	type: string;
	actor: 'agent' | 'human' | 'system' | 'ux';
	summary?: string;
	payload?: Record<string, unknown>;
	ts: number;
};

const intentLog: IntentLogEntry[] = [];
const MAX_LOG = 40;

const INTENT_LOG_KEY = 'veil.intent.log';

function persistIntentLogLocal() {
	if (typeof sessionStorage === 'undefined') return;
	try {
		sessionStorage.setItem(INTENT_LOG_KEY, JSON.stringify(intentLog.slice(-30)));
	} catch {
		/* ignore */
	}
}

/** Restore intent log after reload (sessionStorage). */
export function restoreIntentLog() {
	if (typeof sessionStorage === 'undefined') return;
	try {
		const raw = sessionStorage.getItem(INTENT_LOG_KEY);
		if (!raw) return;
		const arr = JSON.parse(raw) as IntentLogEntry[];
		if (Array.isArray(arr)) {
			intentLog.length = 0;
			intentLog.push(...arr.slice(-MAX_LOG));
		}
	} catch {
		/* ignore */
	}
}

/**
 * Merge durable session META intent_log into the local ring without re-POSTing.
 * Prefer newer local entries when keys collide (type+summary+ts).
 */
export function mergeIntentLogFromServer(entries: unknown[]) {
	if (!Array.isArray(entries) || !entries.length) return;
	const key = (e: IntentLogEntry) =>
		`${e.ts}|${e.actor}|${e.type}|${e.summary ?? ''}`;
	const seen = new Set(intentLog.map(key));
	const incoming: IntentLogEntry[] = [];
	for (const raw of entries) {
		if (!raw || typeof raw !== 'object') continue;
		const o = raw as Record<string, unknown>;
		const type = typeof o.type === 'string' ? o.type : null;
		if (!type) continue;
		const entry: IntentLogEntry = {
			type,
			actor: (typeof o.actor === 'string' ? o.actor : 'system') as IntentLogEntry['actor'],
			summary: typeof o.summary === 'string' ? o.summary : undefined,
			payload:
				o.payload && typeof o.payload === 'object'
					? (o.payload as Record<string, unknown>)
					: undefined,
			ts: typeof o.ts === 'number' ? o.ts : Number(o.ts) || Date.now()
		};
		if (seen.has(key(entry))) continue;
		seen.add(key(entry));
		incoming.push(entry);
	}
	if (!incoming.length) return;
	intentLog.push(...incoming);
	intentLog.sort((a, b) => a.ts - b.ts);
	if (intentLog.length > MAX_LOG) intentLog.splice(0, intentLog.length - MAX_LOG);
	persistIntentLogLocal();
}

export function recordIntent(entry: Omit<IntentLogEntry, 'ts'> & { ts?: number }) {
	const full: IntentLogEntry = {
		...entry,
		ts: entry.ts ?? Date.now()
	};
	intentLog.push(full);
	if (intentLog.length > MAX_LOG) intentLog.splice(0, intentLog.length - 30);
	persistIntentLogLocal();
	// Best-effort server mirror
	if (typeof fetch !== 'undefined') {
		void fetch('/api/ux/intent_log', {
			method: 'POST',
			headers: { 'Content-Type': 'application/json' },
			body: JSON.stringify(full)
		}).catch(() => {});
	}
}

export function recentIntents(limit = 12): IntentLogEntry[] {
	return intentLog.slice(-limit).reverse();
}

export function formatIntentLogForAgent(limit = 8): string {
	const items = recentIntents(limit);
	if (!items.length) return '';
	const lines = ['## Recent intents (agent + human)'];
	for (const it of items) {
		lines.push(`- [${it.actor}] ${it.type}${it.summary ? ` ${it.summary}` : ''}`);
	}
	return lines.join('\n');
}

/** Capture human product mutations (form POSTs) into the intent log. */
export function installHumanIntentCapture(): () => void {
	if (typeof window === 'undefined') return () => {};
	const w = window as unknown as { __veilHumanIntentCapture?: boolean };
	if (w.__veilHumanIntentCapture) return () => {};
	w.__veilHumanIntentCapture = true;
	const orig = window.fetch.bind(window);
	window.fetch = async (input: RequestInfo | URL, init?: RequestInit) => {
		const res = await orig(input, init);
		try {
			const method = (init?.method || 'GET').toUpperCase();
			if (method !== 'POST' && method !== 'PUT' && method !== 'DELETE') return res;
			const url =
				typeof input === 'string'
					? input
					: input instanceof URL
						? input.href
						: (input as Request).url;
			const path = url.replace(/^https?:\/\/[^/]+/, '');
			if (path.includes('/api/ux/')) return res; // agent present commit — already logged
			if (!res.ok) return res;
			if (path.includes('/api/repos') && method === 'POST' && !path.includes('/pulls')) {
				let name = '';
				try {
					const body = init?.body ? JSON.parse(String(init.body)) : {};
					name = body.name || body.slug || '';
				} catch {
					/* ignore */
				}
				recordIntent({
					type: 'CreateProject',
					actor: 'human',
					summary: name || 'project',
					payload: { path }
				});
			} else if (path.includes('/api/pull_requests') && method === 'POST') {
				let title = '';
				try {
					const body = init?.body ? JSON.parse(String(init.body)) : {};
					title = body.title || '';
				} catch {
					/* ignore */
				}
				recordIntent({
					type: 'CreateChange',
					actor: 'human',
					summary: title || 'change',
					payload: { path }
				});
			}
		} catch {
			/* ignore */
		}
		return res;
	};
	return () => {
		window.fetch = orig;
		w.__veilHumanIntentCapture = false;
	};
}

// ─── Executor state ──────────────────────────────────────────────────────────

let executing = false;
const queue: Intent[] = [];
const statusListeners: Array<(s: IntentStatus) => void> = [];
/** Dedupe intent + tool_result both carrying the same Present. */
const recentIntentIds = new Set<string>();
const RECENT_TTL_MS = 8_000;

export function onIntentStatus(listener: (s: IntentStatus) => void): () => void {
	statusListeners.push(listener);
	return () => {
		const i = statusListeners.indexOf(listener);
		if (i >= 0) statusListeners.splice(i, 1);
	};
}

function emitStatus(s: IntentStatus) {
	for (const l of statusListeners) {
		try {
			l(s);
		} catch {
			/* ignore */
		}
	}
}

function sleep(ms: number): Promise<void> {
	return new Promise((r) => setTimeout(r, Math.max(0, ms)));
}

function easeOutCubic(t: number): number {
	return 1 - Math.pow(1 - t, 3);
}

// ─── DOM helpers ─────────────────────────────────────────────────────────────

function ensurePulseStyles() {
	if (typeof document === 'undefined') return;
	if (document.getElementById('veil-intent-present-css')) return;
	const style = document.createElement('style');
	style.id = 'veil-intent-present-css';
	style.textContent = `
@keyframes veil-intent-pulse {
  0% { box-shadow: 0 0 0 0 color-mix(in srgb, var(--dk-accent, #818cf8) 55%, transparent); transform: scale(1); }
  40% { box-shadow: 0 0 0 10px color-mix(in srgb, var(--dk-accent, #818cf8) 0%, transparent); transform: scale(1.03); }
  100% { box-shadow: 0 0 0 0 transparent; transform: scale(1); }
}
@keyframes veil-intent-field-flash {
  0% { background-color: color-mix(in srgb, var(--dk-accent, #818cf8) 28%, transparent); }
  100% { background-color: transparent; }
}
.veil-intent-pulse {
  animation: veil-intent-pulse 0.55s cubic-bezier(0.22, 1, 0.36, 1) 2;
  outline: 2px solid color-mix(in srgb, var(--dk-accent, #818cf8) 70%, transparent) !important;
  outline-offset: 2px;
  border-radius: 8px;
  transition: outline 0.2s ease, transform 0.2s ease;
}
.veil-intent-field-flash {
  animation: veil-intent-field-flash 0.65s ease-out;
}
.veil-intent-announce {
  position: fixed;
  bottom: 4.5rem;
  left: 50%;
  transform: translateX(-50%);
  z-index: 9999;
  padding: 0.5rem 1rem;
  border-radius: 999px;
  font-size: 0.85rem;
  color: var(--dk-text, #f4f4f5);
  background: color-mix(in srgb, var(--dk-surface, #18181b) 92%, var(--dk-accent, #818cf8));
  border: 1px solid color-mix(in srgb, var(--dk-accent, #818cf8) 40%, transparent);
  box-shadow: 0 8px 24px rgba(0,0,0,0.35);
  pointer-events: none;
  opacity: 0;
  transition: opacity 0.25s ease, transform 0.25s ease;
}
.veil-intent-announce.veil-intent-announce--show {
  opacity: 1;
  transform: translateX(-50%) translateY(-4px);
}
`;
	document.head.appendChild(style);
}

function resolvePulseEl(step: Extract<PresentStep, { kind: 'pulse' }>): HTMLElement | null {
	if (typeof document === 'undefined') return null;
	if (step.selector) {
		const el = document.querySelector(step.selector) as HTMLElement | null;
		if (el) return el;
	}
	const t = step.target || 'submit';
	if (t === 'submit' || t === 'create-form') {
		return (
			(document.querySelector(
				'[data-veil-role="create-form"] .btn-primary, .dk-create-shell .btn-primary, .dk-form-progress .btn-primary'
			) as HTMLElement | null) ||
			(document.querySelector('.btn-primary') as HTMLElement | null)
		);
	}
	if (t.startsWith('field:')) {
		const id = t.slice(6);
		return document.querySelector(
			`[data-veil-field="${id}"] input, [data-veil-field="${id}"] textarea, #${CSS.escape(id)}`
		) as HTMLElement | null;
	}
	// text:Approve — match visible button/link label (change detail, deploy, …)
	if (t.startsWith('text:')) {
		const needle = t.slice(5).trim().toLowerCase();
		const candidates = document.querySelectorAll(
			'button, a.btn-primary, a.btn, [role="button"], .btn-primary, .btn-ghost'
		);
		for (const el of candidates) {
			const label = (el.textContent || '').replace(/\s+/g, ' ').trim().toLowerCase();
			if (label === needle || label.includes(needle)) {
				return el as HTMLElement;
			}
		}
	}
	// CSS selector fallback
	try {
		const bySel = document.querySelector(t) as HTMLElement | null;
		if (bySel) return bySel;
	} catch {
		/* invalid selector */
	}
	return document.querySelector('.btn-primary') as HTMLElement | null;
}

function setNativeValue(el: HTMLInputElement | HTMLTextAreaElement, value: string) {
	const proto =
		el instanceof HTMLTextAreaElement
			? window.HTMLTextAreaElement.prototype
			: window.HTMLInputElement.prototype;
	const desc = Object.getOwnPropertyDescriptor(proto, 'value');
	if (desc?.set) {
		desc.set.call(el, value);
	} else {
		el.value = value;
	}
	el.dispatchEvent(new Event('input', { bubbles: true }));
	el.dispatchEvent(new Event('change', { bubbles: true }));
}

function findFieldInput(key: string): HTMLInputElement | HTMLTextAreaElement | null {
	if (typeof document === 'undefined') return null;
	const k = key.toLowerCase();

	// data-veil-field
	const byField = document.querySelector(
		`[data-veil-field="${key}"] input, [data-veil-field="${key}"] textarea`
	) as HTMLInputElement | HTMLTextAreaElement | null;
	if (byField) return byField;

	// id / name
	const byId = document.querySelector(`#${CSS.escape(key)}, [name="${key}"]`) as
		| HTMLInputElement
		| HTMLTextAreaElement
		| null;
	if (byId && (byId.tagName === 'INPUT' || byId.tagName === 'TEXTAREA')) return byId;

	// label text match inside create-form (prefer id=create-change / create-project shells)
	const form =
		document.querySelector(
			'#create-change, #create-project, [data-veil-role="create-form"], .dk-create-shell'
		) || document.body;
	const labels = form.querySelectorAll('label, .dk-field__label');
	for (const lab of labels) {
		const text = (lab.textContent || '').trim().toLowerCase().replace(/●/g, '').trim();
		if (text === k || text.startsWith(k) || k.startsWith(text)) {
			const field = lab.closest('.dk-field');
			const input = field?.querySelector('input, textarea') as
				| HTMLInputElement
				| HTMLTextAreaElement
				| null;
			if (input) return input;
			const forId = (lab as HTMLLabelElement).htmlFor;
			if (forId) {
				const el = document.getElementById(forId);
				if (el instanceof HTMLInputElement || el instanceof HTMLTextAreaElement) return el;
			}
		}
	}

	// placeholder heuristic
	const inputs = form.querySelectorAll('input, textarea');
	for (const el of inputs) {
		const ph = ((el as HTMLInputElement).placeholder || '').toLowerCase();
		if (ph.includes(k) || (k === 'name' && ph.includes('project'))) {
			return el as HTMLInputElement | HTMLTextAreaElement;
		}
		if (k === 'description' && (el.tagName === 'TEXTAREA' || ph.includes('optional'))) {
			return el as HTMLInputElement | HTMLTextAreaElement;
		}
	}

	// First text input for "name", first textarea for "description"
	if (k === 'name') {
		return form.querySelector('input[type="text"], input:not([type])') as HTMLInputElement | null;
	}
	if (k === 'description' || k === 'desc') {
		return form.querySelector('textarea') as HTMLTextAreaElement | null;
	}

	return null;
}

async function fillFields(
	fields: Record<string, unknown>,
	mode: 'snap' | 'type' = 'type'
): Promise<void> {
	ensurePulseStyles();
	for (const [key, raw] of Object.entries(fields)) {
		if (raw == null) continue;
		const value = String(raw);
		const el = findFieldInput(key);
		if (!el) continue;
		el.scrollIntoView({ block: 'nearest', behavior: 'smooth' });
		el.classList.add('veil-intent-field-flash');
		el.focus();
		if (mode === 'snap' || value.length > 48) {
			setNativeValue(el, value);
			await sleep(180);
		} else {
			setNativeValue(el, '');
			const duration = Math.min(420, 80 + value.length * 28);
			const start = performance.now();
			await new Promise<void>((resolve) => {
				const tick = (now: number) => {
					const t = Math.min(1, (now - start) / duration);
					const n = Math.floor(easeOutCubic(t) * value.length);
					setNativeValue(el, value.slice(0, n));
					if (t < 1) requestAnimationFrame(tick);
					else resolve();
				};
				requestAnimationFrame(tick);
			});
			await sleep(100);
		}
		el.classList.remove('veil-intent-field-flash');
	}
}

async function pulseEl(el: HTMLElement | null, ms = 550): Promise<void> {
	if (!el) {
		await sleep(ms);
		return;
	}
	ensurePulseStyles();
	el.scrollIntoView({ block: 'nearest', behavior: 'smooth' });
	el.classList.add('veil-intent-pulse');
	await sleep(ms);
	// keep class a bit longer for second pulse cycle
	await sleep(Math.min(400, ms));
	el.classList.remove('veil-intent-pulse');
}

function showAnnounce(message: string, ms = 1600) {
	if (typeof document === 'undefined' || !message) return;
	ensurePulseStyles();
	let el = document.querySelector('.veil-intent-announce') as HTMLElement | null;
	if (!el) {
		el = document.createElement('div');
		el.className = 'veil-intent-announce';
		document.body.appendChild(el);
	}
	el.textContent = message;
	requestAnimationFrame(() => el!.classList.add('veil-intent-announce--show'));
	setTimeout(() => {
		el?.classList.remove('veil-intent-announce--show');
	}, ms);
}

async function runGoto(path: string, ms = 280, project?: string): Promise<void> {
	const p = path.startsWith('/') ? path : `/${path}`;
	await goto(p);
	patchFocus({
		route: p,
		project: project ?? undefined
	});
	// Allow Svelte to mount the target route (forms, IDE shell)
	await sleep(ms);
}

// ─── Public executor ─────────────────────────────────────────────────────────

export async function executeIntent(intent: Intent): Promise<IntentStatus> {
	if (intent.id && recentIntentIds.has(intent.id)) {
		return { intentId: intent.id, ok: true, step: -1 };
	}
	if (intent.id) {
		recentIntentIds.add(intent.id);
		setTimeout(() => recentIntentIds.delete(intent.id), RECENT_TTL_MS);
	}

	queue.push(intent);
	if (executing) {
		return { intentId: intent.id, ok: true, step: -1 };
	}
	executing = true;
	let last: IntentStatus = { intentId: intent.id, ok: true };

	try {
		while (queue.length) {
			const current = queue.shift()!;
			last = await runOne(current);
			emitStatus(last);
		}
	} finally {
		executing = false;
	}
	return last;
}

/** In-DOM modal for choose step (PR target). Resolves with selected option id. */
function runChoose(
	step: Extract<PresentStep, { kind: 'choose' }>,
	intentId: string
): Promise<Record<string, unknown>> {
	ensurePulseStyles();
	return new Promise((resolve) => {
		if (typeof document === 'undefined') {
			resolve({ choice: '__new__', ok: true });
			return;
		}
		const existing = document.getElementById('veil-choose-coding-target');
		if (existing) existing.remove();

		const root = document.createElement('div');
		root.id = 'veil-choose-coding-target';
		root.setAttribute('role', 'dialog');
		root.setAttribute('aria-modal', 'true');
		root.style.cssText =
			'position:fixed;inset:0;z-index:10050;display:flex;align-items:center;justify-content:center;' +
			'background:rgba(0,0,0,0.55);padding:1rem;';
		const panel = document.createElement('div');
		panel.style.cssText =
			'max-width:28rem;width:100%;border-radius:12px;padding:1.1rem 1.2rem;' +
			'background:var(--dk-surface,#18181b);color:var(--dk-text,#f4f4f5);' +
			'border:1px solid color-mix(in srgb, var(--dk-accent,#818cf8) 35%, transparent);' +
			'box-shadow:0 16px 48px rgba(0,0,0,0.45);';
		const h = document.createElement('h3');
		h.textContent = step.title || 'Which pull request?';
		h.style.cssText = 'margin:0 0 0.4rem;font-size:1.05rem;font-weight:600;';
		panel.appendChild(h);
		if (step.message) {
			const p = document.createElement('p');
			p.textContent = step.message;
			p.style.cssText = 'margin:0 0 0.85rem;font-size:0.85rem;opacity:0.85;line-height:1.4;';
			panel.appendChild(p);
		}
		const list = document.createElement('div');
		list.style.cssText = 'display:flex;flex-direction:column;gap:0.45rem;max-height:50vh;overflow:auto;';
		const finish = (choice: string, extra?: Record<string, unknown>) => {
			root.remove();
			const result = { ok: true, choice, intent_id: intentId, ...extra };
			void fetch('/api/ux/intent_ack', {
				method: 'POST',
				headers: { 'Content-Type': 'application/json' },
				body: JSON.stringify({ intent_id: intentId, result })
			}).catch(() => {});
			// Agent/orchestrator should re-call resolve_coding_target({ choice }) after ACK.
			resolve(result);
		};
		for (const opt of step.options || []) {
			const btn = document.createElement('button');
			btn.type = 'button';
			btn.style.cssText =
				'text-align:left;padding:0.65rem 0.75rem;border-radius:8px;cursor:pointer;' +
				'border:1px solid color-mix(in srgb, var(--dk-border,#3f3f46) 80%, transparent);' +
				'background:color-mix(in srgb, var(--dk-surface,#18181b) 90%, #fff 4%);color:inherit;font:inherit;';
			const label = document.createElement('div');
			label.textContent = opt.label || opt.id;
			label.style.cssText = 'font-weight:500;font-size:0.9rem;';
			btn.appendChild(label);
			if (opt.detail) {
				const d = document.createElement('div');
				d.textContent = opt.detail;
				d.style.cssText = 'font-size:0.75rem;opacity:0.7;margin-top:0.15rem;';
				btn.appendChild(d);
			}
			btn.onmouseenter = () => {
				btn.style.borderColor = 'var(--dk-accent,#818cf8)';
			};
			btn.onmouseleave = () => {
				btn.style.borderColor =
					'color-mix(in srgb, var(--dk-border,#3f3f46) 80%, transparent)';
			};
			btn.onclick = () =>
				finish(opt.id, {
					source_branch: opt.source_branch ?? null,
					label: opt.label
				});
			list.appendChild(btn);
		}
		panel.appendChild(list);
		const cancel = document.createElement('button');
		cancel.type = 'button';
		cancel.textContent = 'Cancel (new PR)';
		cancel.style.cssText =
			'margin-top:0.75rem;background:transparent;border:none;color:inherit;opacity:0.65;' +
			'cursor:pointer;font-size:0.8rem;text-decoration:underline;';
		cancel.onclick = () => finish('__new__');
		panel.appendChild(cancel);
		root.appendChild(panel);
		document.body.appendChild(root);

		const timeout = step.ms && step.ms > 0 ? step.ms : 120_000;
		window.setTimeout(() => {
			if (document.getElementById('veil-choose-coding-target')) {
				finish('__new__', { timed_out: true });
			}
		}, timeout);
	});
}

async function runCommit(
	step: Extract<PresentStep, { kind: 'commit' }>,
	intent: Intent
): Promise<Record<string, unknown>> {
	const method = (step.method || 'POST').toUpperCase();
	const body = {
		...(intent.payload || {}),
		...(step.body || {})
	};
	const res = await fetch(step.path, {
		method,
		headers: { 'Content-Type': 'application/json' },
		body: method === 'GET' || method === 'HEAD' ? undefined : JSON.stringify(body)
	});
	const text = await res.text();
	let data: Record<string, unknown> = {};
	try {
		data = text ? (JSON.parse(text) as Record<string, unknown>) : {};
	} catch {
		data = { raw: text };
	}
	if (!res.ok) {
		const err =
			(data.error as string) ||
			(data.summary as string) ||
			text ||
			`HTTP ${res.status}`;
		throw new Error(String(err));
	}
	// Navigate from template or response path
	const slug =
		(data.slug as string) ||
		(data.id as string) ||
		(typeof data.project === 'object' && data.project
			? String((data.project as { slug?: string; name?: string }).slug ||
					(data.project as { name?: string }).name ||
					'')
			: '') ||
		String(intent.payload?.name || '');
	const id = String(data.id || slug);
	let nextPath =
		(data.path as string) ||
		(data.navigation && typeof data.navigation === 'object'
			? String((data.navigation as { path?: string }).path || '')
			: '');
	if (!nextPath && step.resultPathTemplate) {
		nextPath = step.resultPathTemplate
			.replace(/\{slug\}/g, encodeURIComponent(slug))
			.replace(/\{name\}/g, encodeURIComponent(slug))
			.replace(/\{id\}/g, encodeURIComponent(id))
			.replace(/\{project\}/g, encodeURIComponent(slug));
	}
	if (nextPath) {
		await runGoto(nextPath, step.ms ?? 320, slug || undefined);
	}
	recordIntent({
		type: intent.type,
		actor: 'ux',
		summary: slug || id || intent.type,
		payload: { path: nextPath, domain: data }
	});
	return data;
}

async function runOne(intent: Intent): Promise<IntentStatus> {
	const steps = intent.present?.steps ?? [];
	if (intent.present?.announce) {
		showAnnounce(intent.present.announce);
	}

	recordIntent({
		type: intent.type,
		actor: intent.actor,
		summary: String(
			intent.payload?.name || intent.payload?.title || intent.present?.announce || intent.type
		),
		payload: intent.payload
	});

	if (steps.length === 0 && intent.navigation?.path) {
		try {
			const action = intent.navigation.action;
			const path = intent.navigation.path;
			if (action === 'open-ide' && intent.navigation.project) {
				await runGoto(
					`/projects/${encodeURIComponent(intent.navigation.project)}/ide`,
					300,
					intent.navigation.project
				);
			} else if (path) {
				await runGoto(path, 280, intent.navigation.project);
			}
			return { intentId: intent.id, ok: true };
		} catch (e) {
			return {
				intentId: intent.id,
				ok: false,
				error: e instanceof Error ? e.message : String(e)
			};
		}
	}

	let lastResult: Record<string, unknown> | undefined;
	for (let i = 0; i < steps.length; i++) {
		const step = steps[i];
		try {
			switch (step.kind) {
				case 'goto':
					await runGoto(step.path, step.ms ?? 300, step.project);
					break;
				case 'fill':
					await fillFields(step.fields, step.mode ?? 'type');
					if (step.ms) await sleep(step.ms);
					if (step.formId) {
						patchFocus({
							form: {
								id: step.formId,
								values: step.fields,
								dirty: true
							}
						});
					}
					break;
				case 'pulse':
					await pulseEl(resolvePulseEl(step), step.ms ?? 550);
					break;
				case 'wait':
					await sleep(step.ms);
					break;
				case 'announce':
					showAnnounce(step.message);
					await sleep(400);
					break;
				case 'commit':
					lastResult = await runCommit(step, intent);
					break;
				case 'choose':
					lastResult = await runChoose(step, intent.id);
					break;
				default:
					break;
			}
		} catch (e) {
			return {
				intentId: intent.id,
				ok: false,
				step: i,
				error: e instanceof Error ? e.message : String(e),
				result: lastResult
			};
		}
	}

	return { intentId: intent.id, ok: true, step: steps.length, result: lastResult };
}

// ─── Parse tool results → Intent ─────────────────────────────────────────────

function parseJsonish(output: unknown): Record<string, unknown> | null {
	if (output && typeof output === 'object' && !Array.isArray(output)) {
		return output as Record<string, unknown>;
	}
	if (typeof output === 'string') {
		try {
			const v = JSON.parse(output);
			if (v && typeof v === 'object') return v as Record<string, unknown>;
		} catch {
			return null;
		}
	}
	return null;
}

/** Build or extract Intent from a platform tool result. */
export function intentFromToolResult(toolName: string, output: unknown): Intent | null {
	const obj = parseJsonish(output);
	if (!obj) return null;

	// Nested under detail (ACP sometimes wraps)
	const root =
		obj.intent && typeof obj.intent === 'object'
			? obj
			: obj.detail && typeof obj.detail === 'object'
				? (obj.detail as Record<string, unknown>)
				: obj;

	if (root.intent && typeof root.intent === 'object') {
		const raw = root.intent as Record<string, unknown>;
		return normalizeIntent(raw, toolName, root);
	}

	// Synthesize from navigation + known tools when server omitted intent envelope
	return synthesizeIntent(toolName, root);
}

function normalizeIntent(
	raw: Record<string, unknown>,
	toolName: string,
	root: Record<string, unknown>
): Intent {
	const present = raw.present as Present | undefined;
	const navigation =
		(raw.navigation as Intent['navigation']) ||
		(root.navigation as Intent['navigation']) ||
		undefined;
	return {
		type: String(raw.type || toolName),
		id: String(raw.id || `intent_${toolName}_${Date.now()}`),
		actor: (raw.actor as Intent['actor']) || 'agent',
		payload: (raw.payload as Record<string, unknown>) || undefined,
		present,
		domain: (raw.domain as IntentDomain) || undefined,
		navigation
	};
}

function synthesizeIntent(toolName: string, root: Record<string, unknown>): Intent | null {
	// Only synthesize for platform tools that benefit from present
	if (toolName === 'rename_project' || toolName === 'update_project') {
		const name = String(root.name || '');
		const slug = String(
			root.slug ||
				(root.project as Record<string, unknown> | undefined)?.slug ||
				''
		);
		const nav = root.navigation as Intent['navigation'] | undefined;
		const path = nav?.path || (slug ? `/projects/${encodeURIComponent(slug)}` : '/projects');
		return {
			type: 'UpdateProject',
			id: `intent_rename_project_${Date.now()}`,
			actor: 'agent',
			payload: { name, slug },
			domain: { mode: 'server', done: root.ok !== false },
			present: {
				announce: name ? `Renamed project to ${name}` : 'Renamed project',
				steps: [{ kind: 'goto', path, ms: 280, project: slug || undefined }]
			},
			navigation: nav || { action: 'goto', path, project: slug || undefined }
		};
	}

	if (toolName === 'create_project' || toolName === 'create_repo') {
		const name = String(
			root.slug ||
				root.name ||
				(root.project as Record<string, unknown> | undefined)?.slug ||
				(root.project as Record<string, unknown> | undefined)?.name ||
				''
		);
		const desc =
			(root.payload as Record<string, unknown> | undefined)?.description ??
			root.description ??
			'';
		const nav = root.navigation as Intent['navigation'] | undefined;
		const finalPath =
			nav?.path ||
			(name ? `/projects/${encodeURIComponent(name)}/ide` : '/projects');
		const ux = root.pending_ux === true || root.execution && (root.execution as { domain?: string }).domain === 'ux';
		const steps: PresentStep[] = [
			{ kind: 'goto', path: '/projects/new', ms: 320 },
			{
				kind: 'fill',
				formId: 'create-project',
				fields: {
					name: name || 'project',
					...(desc ? { description: String(desc) } : {})
				},
				mode: 'type'
			},
			{ kind: 'wait', ms: 180 },
			{ kind: 'pulse', target: 'submit', ms: 600 },
			{ kind: 'wait', ms: 220 }
		];
		if (ux) {
			steps.push({
				kind: 'commit',
				formId: 'create-project',
				method: 'POST',
				path: '/api/ux/create_project',
				body: {
					name: name || 'project',
					description: desc || undefined,
					open: true,
					open_ide: true
				},
				resultPathTemplate: '/projects/{slug}/ide'
			});
		} else {
			steps.push({
				kind: 'goto',
				path: finalPath,
				ms: 320,
				project: name || undefined
			});
		}
		return {
			type: 'CreateProject',
			id: `intent_create_project_${Date.now()}`,
			actor: 'agent',
			payload: { name, description: desc },
			domain: { mode: ux ? 'ux' : 'server', done: !ux && root.ok !== false },
			present: {
				announce: name ? `Creating project ${name}` : 'Creating project',
				steps
			},
			navigation: nav
		};
	}

	if (toolName === 'resolve_coding_target') {
		// Prefer embedded intent (needs_choice Present modal)
		if (root.intent && typeof root.intent === 'object') {
			return root.intent as Intent;
		}
		return null;
	}

	if (toolName === 'create_pr' || toolName === 'open_create_pr') {
		const payload = (root.payload as Record<string, unknown> | undefined) || {};
		const title = String(root.title || payload.title || '');
		const description = String(
			root.description || payload.description || root.body || payload.body || ''
		);
		const project = String(
			root.slug || root.project || payload.slug || payload.project || ''
		);
		const fields: Record<string, string> = {};
		if (title) fields.title = title;
		if (description) fields.description = description;
		if (project) {
			fields.project = project;
			fields.slug = project;
		}
		const nav = root.navigation as Intent['navigation'] | undefined;
		const ux =
			root.pending_ux === true ||
			(root.execution as { domain?: string } | undefined)?.domain === 'ux';
		const fillSteps: PresentStep[] =
			Object.keys(fields).length > 0
				? [
						{
							kind: 'fill',
							formId: 'create-change',
							fields,
							mode: 'type'
						},
						{ kind: 'wait', ms: 160 },
						{ kind: 'pulse', target: 'submit', ms: 550 },
						{ kind: 'wait', ms: 180 }
					]
				: [];
		return {
			type: 'CreateChange',
			id: `intent_create_pr_${Date.now()}`,
			actor: 'agent',
			payload: { title, description, project: project || undefined },
			domain: { mode: ux ? 'ux' : 'server', done: !ux },
			present: {
				announce: title ? `Creating change: ${title}` : 'Open create change',
				steps: [
					{ kind: 'goto', path: '/pulls/new', ms: 360 },
					...fillSteps,
					...(nav?.path && !ux
						? ([{ kind: 'goto' as const, path: nav.path, ms: 280 }] as PresentStep[])
						: [])
				]
			},
			navigation: nav
		};
	}

	const nav = root.navigation as Intent['navigation'] | undefined;
	if (nav?.path || nav?.project) {
		return {
			type: toolName,
			id: `intent_${toolName}_${Date.now()}`,
			actor: 'agent',
			domain: { mode: 'none' },
			navigation: nav,
			present: nav.path
				? {
						steps: [
							{
								kind: 'goto',
								path:
									nav.action === 'open-ide' && nav.project
										? `/projects/${encodeURIComponent(nav.project)}/ide`
										: nav.path!,
								ms: 280,
								project: nav.project
							}
						]
					}
				: undefined
		};
	}

	return null;
}

/** Tools whose present must run instead of early TOOL_NAV. */
export const STAGED_PRESENT_TOOLS = new Set([
	'create_project',
	'create_repo',
	'create_pr',
	'open_create_pr',
	'resolve_coding_target'
]);

export function isIntentExecuting(): boolean {
	return executing || queue.length > 0;
}

/** Busy flag for UI (optional). */
export const intentBusy = {
	subscribe(fn: (v: boolean) => void) {
		fn(executing);
		const unsub = onIntentStatus(() => fn(executing || queue.length > 0));
		// poll lightly while queue may drain
		const id = setInterval(() => fn(executing || queue.length > 0), 200);
		return () => {
			unsub();
			clearInterval(id);
		};
	}
};
