/**
 * Outstanding change sets — review state, not git status.
 * Polls GET /api/review/* and exposes badges for Projects / IDE / Review.
 */
import { writable, derived } from 'svelte/store';

export type OutstandingItem = {
	id: string;
	repo_id?: string | null;
	slug: string;
	project_name?: string | null;
	kind: string;
	path?: string | null;
	summary: string;
	rationale?: string | null;
	git_sha?: string | null;
	session_id?: string | null;
	pr_id?: string | null;
	created_at: string;
	status: 'outstanding' | 'approved' | 'rejected';
};

export type ProjectReview = {
	slug: string;
	repo_id?: string | null;
	outstanding: number;
	needs_sign_off: boolean;
	touched: boolean;
	last_touched_at?: string | null;
	last_kind?: string | null;
};

export type ChangeSummary = {
	headline: string;
	files: string[];
	why: string[];
	file_changes: number;
	error_count: number;
	warning_count: number;
	check_status: string;
};

export type ChangeSet = {
	id: string;
	slug: string;
	repo_id?: string | null;
	session_id?: string | null;
	pr_id?: string | null;
	git_sha?: string | null;
	item_ids: string[];
	outstanding: number;
	summary: string;
	change_summary?: ChangeSummary | null;
	host_check?: {
		severity?: string;
		error_count?: number;
		warning_count?: number;
		summary?: string;
	} | null;
	host_has_errors: boolean;
};

export type SignOffAudit = {
	id: string;
	at: string;
	actor: string;
	decision: string;
	item_ids: string[];
	note?: string | null;
	slug?: string | null;
	git_sha?: string | null;
	structural_diff_hash?: string | null;
	pr_id?: string | null;
	changeset_id?: string | null;
	actor_kind?: string;
};

export type AuditEnv = {
	veil_dev?: boolean;
	ci_auto_pass?: boolean;
	audit_environment?: boolean;
	note?: string;
};

export const reviewItems = writable<OutstandingItem[]>([]);
export const reviewProjects = writable<ProjectReview[]>([]);
export const reviewChangeSets = writable<ChangeSet[]>([]);
export const reviewAudits = writable<SignOffAudit[]>([]);
export const reviewAuditEnv = writable<AuditEnv | null>(null);
export const reviewReady = writable(false);
export const reviewLoading = writable(false);
export const reviewLoadError = writable('');
export const reviewOutstandingCount = derived(reviewItems, (items) =>
	items.filter((i) => i.status === 'outstanding').length
);

/**
 * Turn-completion review prompt. Set by the agent session when a turn finishes
 * with unreviewed work (from the 'done' event's needsReview/reviewSlug), consumed
 * by a global banner. `null` means no pending prompt.
 */
export const reviewPrompt = writable<{ slug: string; count: number } | null>(null);

export function setReviewPrompt(slug: string, count: number): void {
	if (!slug) return;
	reviewPrompt.set({ slug, count: Math.max(1, count || 1) });
}

export function clearReviewPrompt(): void {
	reviewPrompt.set(null);
}

let pollTimer: ReturnType<typeof setInterval> | null = null;
let inFlight = 0;

export async function refreshReview(slug?: string): Promise<void> {
	inFlight += 1;
	reviewLoading.set(true);
	try {
		const u = new URL(
			'/api/review/outstanding',
			typeof window !== 'undefined' ? window.location.origin : 'http://localhost'
		);
		if (slug) u.searchParams.set('slug', slug);
		u.searchParams.set('status', 'outstanding');
		const r = await fetch(u.toString());
		if (!r.ok) {
			reviewLoadError.set(`Could not load review (${r.status})`);
			return;
		}
		const data = await r.json();
		const items = Array.isArray(data.items) ? (data.items as OutstandingItem[]) : [];
		reviewItems.set(items);
		const by = Array.isArray(data.by_project) ? (data.by_project as ProjectReview[]) : [];
		reviewProjects.set(by);
		const sets = Array.isArray(data.change_sets) ? (data.change_sets as ChangeSet[]) : [];
		reviewChangeSets.set(sets);
		const audits = Array.isArray(data.audits) ? (data.audits as SignOffAudit[]) : [];
		reviewAudits.set(audits);
		if (data.audit_env && typeof data.audit_env === 'object') {
			reviewAuditEnv.set(data.audit_env as AuditEnv);
		}
		reviewLoadError.set('');
	} catch {
		reviewLoadError.set('Could not reach the host for review.');
	} finally {
		inFlight = Math.max(0, inFlight - 1);
		if (inFlight === 0) {
			reviewLoading.set(false);
			reviewReady.set(true);
		}
	}
}

export function startReviewPoll(ms = 4000): () => void {
	void refreshReview().then(() => reconcileReviewWithCatalog());
	if (pollTimer) clearInterval(pollTimer);
	pollTimer = setInterval(() => {
		void refreshReview();
	}, ms);
	return () => {
		if (pollTimer) {
			clearInterval(pollTimer);
			pollTimer = null;
		}
	};
}

export function reviewForSlug(slug: string, projects: ProjectReview[]): ProjectReview | null {
	const s = slug.trim().toLowerCase();
	return (
		projects.find(
			(p) =>
				p.slug.toLowerCase() === s ||
				(p.repo_id && p.repo_id.toLowerCase() === s)
		) ?? null
	);
}

export function changeSetForSlug(slug: string, sets: ChangeSet[]): ChangeSet | null {
	const s = slug.trim().toLowerCase();
	return sets.find((c) => c.slug.toLowerCase() === s) ?? null;
}

export async function submitSignOff(opts: {
	ids?: string[];
	slug?: string;
	all?: boolean;
	decision?: 'approve' | 'reject';
	note?: string;
	actor?: string;
	git_sha?: string;
	structural_diff_hash?: string;
	host_check?: unknown;
	pr_id?: string;
}): Promise<{ ok: boolean; error?: string; signed?: number; audit?: SignOffAudit; approve_pr?: string }> {
	try {
		const r = await fetch('/api/review/sign_off', {
			method: 'POST',
			headers: { 'Content-Type': 'application/json' },
			body: JSON.stringify({
				ids: opts.ids ?? [],
				slug: opts.slug,
				all: opts.all ?? false,
				decision: opts.decision ?? 'approve',
				note: opts.note,
				actor: opts.actor,
				git_sha: opts.git_sha,
				structural_diff_hash: opts.structural_diff_hash,
				host_check: opts.host_check,
				pr_id: opts.pr_id,
				via: 'ui'
			})
		});
		const data = await r.json().catch(() => ({}));
		if (!r.ok) {
			return { ok: false, error: String(data.error || r.statusText) };
		}
		await refreshReview();
		return {
			ok: true,
			signed: Number(data.signed || 0),
			audit: data.audit as SignOffAudit | undefined,
			approve_pr: typeof data.approve_pr === 'string' ? data.approve_pr : undefined
		};
	} catch (e) {
		return { ok: false, error: e instanceof Error ? e.message : String(e) };
	}
}

export async function reconcileReview(liveSlugs: string[]): Promise<number> {
	const live = liveSlugs.map((s) => s.trim()).filter(Boolean);
	if (!live.length) return 0;
	try {
		const r = await fetch('/api/review/reconcile', {
			method: 'POST',
			headers: { 'Content-Type': 'application/json' },
			body: JSON.stringify({ live_slugs: live })
		});
		const data = await r.json().catch(() => ({}));
		const closed = Number(data.closed || 0);
		if (closed > 0) await refreshReview();
		return closed;
	} catch {
		return 0;
	}
}

let catalogReconcileStarted = false;

/** Once per session: drop review items for products that are gone. */
export async function reconcileReviewWithCatalog(): Promise<void> {
	if (catalogReconcileStarted) return;
	catalogReconcileStarted = true;
	try {
		const live = new Set<string>();
		const [projR, repoR] = await Promise.all([
			fetch('/api/projects'),
			fetch('/api/repos')
		]);
		if (projR.ok) {
			const data = await projR.json();
			for (const p of Array.isArray(data.projects) ? data.projects : []) {
				const rec = p as Record<string, unknown>;
				if (rec.name) live.add(String(rec.name));
				if (rec.slug) live.add(String(rec.slug));
				if (rec.id) live.add(String(rec.id));
			}
		}
		if (repoR.ok) {
			const list = await repoR.json();
			for (const row of Array.isArray(list) ? list : []) {
				const rec = row as Record<string, unknown>;
				if (rec.name) live.add(String(rec.name));
				if (rec.slug) live.add(String(rec.slug));
				const id = rec.id;
				if (id && typeof id === 'object' && id && 'value' in (id as object)) {
					live.add(String((id as { value?: string }).value || ''));
				} else if (id) {
					live.add(String(id));
				}
			}
		}
		await reconcileReview([...live].filter(Boolean));
	} catch {
		catalogReconcileStarted = false;
	}
}

/**
 * Fetch the approval gate policy for a project + environment.
 * `gate: 'none'` → dev-gated, one-action Approve & Deploy allowed.
 * `gate: 'sign_off'` → prod ceremony; combined action is NOT offered.
 * Any failure falls back to the permissive default so dev flows keep working.
 */
export async function fetchDeployGate(
	slug: string,
	environment = 'dev'
): Promise<{ gate: 'none' | 'sign_off'; one_action_ship: boolean }> {
	try {
		const u = new URL(
			`/api/projects/${encodeURIComponent(slug)}/deploy/gate`,
			typeof window !== 'undefined' ? window.location.origin : 'http://localhost'
		);
		u.searchParams.set('environment', environment);
		const r = await fetch(u.toString());
		if (!r.ok) return { gate: 'none', one_action_ship: true };
		const data = await r.json();
		const gate = data.gate === 'sign_off' ? 'sign_off' : 'none';
		return { gate, one_action_ship: gate === 'none' };
	} catch {
		return { gate: 'none', one_action_ship: true };
	}
}

export async function exportAuditPack(): Promise<void> {
	const r = await fetch('/api/review/export');
	if (!r.ok) throw new Error(`export HTTP ${r.status}`);
	const data = await r.json();
	const blob = new Blob([JSON.stringify(data, null, 2)], { type: 'application/json' });
	const url = URL.createObjectURL(blob);
	const a = document.createElement('a');
	a.href = url;
	a.download = `veil-signoff-export-${new Date().toISOString().slice(0, 10)}.json`;
	a.click();
	URL.revokeObjectURL(url);
}
