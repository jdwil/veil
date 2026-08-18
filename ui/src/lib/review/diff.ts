import type { FileDiff, FileDiffHunk } from '$lib/ide/prWizard';

/** Project-relative path from a session workdir or repo path. */
export function reviewRelPath(path: string): string {
	const p = path.replace(/\\/g, '/').trim();
	if (!p) return '';
	const markers = ['/veil-ws/', '/veil-s3-ws/', '/veil-projects/'];
	for (const m of markers) {
		const i = p.indexOf(m);
		if (i >= 0) {
			const rest = p.slice(i + m.length);
			const parts = rest.split('/');
			// user/session/slug/file… or slug/file…
			const cut = parts.length > 3 ? parts.slice(3).join('/') : parts.slice(1).join('/');
			if (cut) return cut;
		}
	}
	return p.replace(/^\.\//, '');
}

export function pathsMatch(a: string, b: string): boolean {
	const x = reviewRelPath(a).toLowerCase();
	const y = reviewRelPath(b).toLowerCase();
	if (!x || !y) return false;
	return x === y || x.endsWith('/' + y) || y.endsWith('/' + x);
}

export function fileDiffsForPath(diffs: FileDiff[], path?: string | null): FileDiff[] {
	if (!path) return diffs;
	const hit = diffs.filter((d) => pathsMatch(d.path, path));
	return hit.length ? hit : [];
}

/** Parse `git diff` / `git_working_diff` text into FileDiffs. */
export function parseUnifiedPatch(patch: string): FileDiff[] {
	if (!patch.trim()) return [];
	const out: FileDiff[] = [];
	let current: FileDiff | null = null;
	let hunk: FileDiffHunk | null = null;
	for (const raw of patch.split('\n')) {
		if (raw.startsWith('diff --git ')) {
			if (current) {
				if (hunk) current.hunks = [...(current.hunks || []), hunk];
				out.push(current);
			}
			const m = raw.match(/b\/(.+)$/);
			current = {
				path: (m?.[1] || raw.slice(11)).trim(),
				status: 'modified',
				hunks: []
			};
			hunk = null;
			continue;
		}
		if (!current) continue;
		if (raw.startsWith('new file')) current.status = 'added';
		if (raw.startsWith('deleted file')) current.status = 'removed';
		if (raw.startsWith('@@')) {
			if (hunk) current.hunks = [...(current.hunks || []), hunk];
			hunk = { header: raw, lines: [] };
			continue;
		}
		if (hunk && (raw.startsWith('+') || raw.startsWith('-') || raw.startsWith(' ') || raw === '\\')) {
			hunk.lines = [...(hunk.lines || []), raw];
		}
	}
	if (current) {
		if (hunk) current.hunks = [...(current.hunks || []), hunk];
		out.push(current);
	}
	return out;
}

/** Keep only hunks that mention any of `names` (construct / keyword tokens). */
export function filterDiffsByNames(files: FileDiff[], names: string[]): FileDiff[] {
	const tokens = names
		.map((n) => n.trim())
		.filter((n) => n.length > 1);
	if (!files.length) return [];
	if (!tokens.length) return files;
	const lower = tokens.map((t) => t.toLowerCase());
	const hit = (text: string) => {
		const s = text.toLowerCase();
		return lower.some((t) => {
			const i = s.indexOf(t.toLowerCase());
			if (i < 0) return false;
			const before = i === 0 ? ' ' : s[i - 1];
			const after = s[i + t.length] || ' ';
			return !/[a-z0-9_]/i.test(before) && !/[a-z0-9_]/i.test(after);
		});
	};
	const out: FileDiff[] = [];
	for (const f of files) {
		if (hit(f.path)) {
			out.push(f);
			continue;
		}
		const hunks = (f.hunks || [])
			.map((h) => {
				const lines = h.lines || [];
				const idxs = lines
					.map((ln, i) => (hit(ln) ? i : -1))
					.filter((i) => i >= 0);
				if (!idxs.length && !hit(h.header || '')) return null;
				if (tokens.length === 1 && idxs.length && lines.length > 24) {
					const pad = 4;
					const start = Math.max(0, Math.min(...idxs) - pad);
					const end = Math.min(lines.length, Math.max(...idxs) + pad + 1);
					return { ...h, lines: lines.slice(start, end) };
				}
				return h;
			})
			.filter((h): h is NonNullable<typeof h> => !!h);
		if (hunks.length) {
			out.push({ ...f, hunks });
		}
	}
	return out;
}

export function hunkLineClass(line: string): string {
	if (line.startsWith('+') && !line.startsWith('+++')) return 'add';
	if (line.startsWith('-') && !line.startsWith('---')) return 'del';
	if (line.startsWith('@@') || line.startsWith('…')) return 'meta';
	return 'ctx';
}
