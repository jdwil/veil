/**
 * Outstanding change sets — review state, not git status.
 * Polls GET /api/review/* and exposes badges for Projects / IDE / sign-off.
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

export const reviewItems = writable<OutstandingItem[]>([]);
export const reviewProjects = writable<ProjectReview[]>([]);
export const reviewOutstandingCount = derived(reviewItems, (items) =>
	items.filter((i) => i.status === 'outstanding').length
);

let pollTimer: ReturnType<typeof setInterval> | null = null;

export async function refreshReview(slug?: string): Promise<void> {
	try {
		const u = new URL(
			'/api/review/outstanding',
			typeof window !== 'undefined' ? window.location.origin : 'http://localhost'
		);
		if (slug) u.searchParams.set('slug', slug);
		u.searchParams.set('status', 'outstanding');
		const r = await fetch(u.toString());
		if (!r.ok) return;
		const data = await r.json();
		const items = Array.isArray(data.items) ? (data.items as OutstandingItem[]) : [];
		reviewItems.set(items);
		const by = Array.isArray(data.by_project) ? (data.by_project as ProjectReview[]) : [];
		reviewProjects.set(by);
	} catch {
		/* host may be booting */
	}
}

export function startReviewPoll(ms = 4000): () => void {
	void refreshReview();
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

export async function submitSignOff(opts: {
	ids?: string[];
	slug?: string;
	all?: boolean;
	decision?: 'approve' | 'reject';
	note?: string;
	actor?: string;
}): Promise<{ ok: boolean; error?: string; signed?: number }> {
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
				actor: opts.actor ?? 'human'
			})
		});
		const data = await r.json().catch(() => ({}));
		if (!r.ok) {
			return { ok: false, error: String(data.error || r.statusText) };
		}
		await refreshReview();
		return { ok: true, signed: Number(data.signed || 0) };
	} catch (e) {
		return { ok: false, error: e instanceof Error ? e.message : String(e) };
	}
}
