/**
 * Official Review ceremony — hierarchy of ideas → constructs → file hunks.
 * Ship-gate lives on /review only (Approve / Request changes).
 */
import {
	buildWizardGroups,
	buildWizardItems,
	containerLabel,
	itemDisplayName,
	itemKindLabel,
	pathOf
} from '$lib/review/grouping';
import type {
	DiffItem,
	FileDiff,
	RiskLevel,
	StructDiff,
	WizardGroup,
	WizardItemState
} from '$lib/review/types';
import { pathsMatch, reviewRelPath } from '$lib/review/diff';

export type CeremonyIdea = {
	key: string;
	title: string;
	rationale: string | null;
	groups: WizardGroup[];
	risk: RiskLevel;
	fileDiffs: FileDiff[];
};

export function firstSentence(text: string, max = 88): string {
	const t = text.replace(/\s+/g, ' ').trim();
	if (!t) return '';
	const cut = t.split(/(?<=[.!?])\s/)[0] || t;
	return cut.length > max ? `${cut.slice(0, max - 1)}…` : cut;
}

export function hashItems(diffItems: DiffItem[]): string {
	const parts = diffItems.map((d) => `${d.kind}:${d.name || d.to_name || d.path || ''}`);
	let h = 2166136261;
	const s = parts.join('|');
	for (let i = 0; i < s.length; i++) {
		h ^= s.charCodeAt(i);
		h = Math.imul(h, 16777619);
	}
	return (h >>> 0).toString(16).padStart(8, '0');
}

export function filesForItem(item: DiffItem, files: FileDiff[]): FileDiff[] {
	if (!files.length) return [];
	const name = itemDisplayName(item).toLowerCase();
	const path = pathOf(item);
	const leaf = reviewRelPath(path).split('/').pop()?.toLowerCase() || '';
	const hits = files.filter((f) => {
		const p = f.path.toLowerCase();
		if (pathsMatch(f.path, path)) return true;
		if (name && (p.includes(name) || name.includes(p.split('/').pop() || ''))) return true;
		if (leaf && p.endsWith(leaf)) return true;
		return false;
	});
	return hits;
}

export function filesForGroups(groups: WizardGroup[], files: FileDiff[]): FileDiff[] {
	const seen = new Set<string>();
	const out: FileDiff[] = [];
	for (const g of groups) {
		for (const ch of g.children) {
			for (const f of filesForItem(ch.item, files)) {
				if (seen.has(f.path)) continue;
				seen.add(f.path);
				out.push(f);
			}
		}
	}
	return out;
}

export function leftoverFiles(ideas: CeremonyIdea[], files: FileDiff[]): FileDiff[] {
	const used = new Set(ideas.flatMap((i) => i.fileDiffs.map((f) => f.path)));
	return files.filter((f) => !used.has(f.path));
}

/** Group constructs by agent rationale (idea), then by construct. */
export function buildCeremonyIdeas(
	items: WizardItemState[],
	fileDiffs: FileDiff[]
): CeremonyIdea[] {
	const groups = buildWizardGroups(items);
	const map = new Map<string, WizardGroup[]>();
	for (const g of groups) {
		const r = (g.rationale || '').trim();
		const title = r ? firstSentence(r) : g.name;
		const key = r ? `idea:${title.toLowerCase()}` : `construct:${g.key}`;
		const list = map.get(key) || [];
		list.push(g);
		map.set(key, list);
	}
	const ideas: CeremonyIdea[] = [];
	for (const [key, gs] of map) {
		let risk: RiskLevel = 'low';
		let rationale: string | null = null;
		for (const g of gs) {
			if (g.risk === 'critical' || (g.risk === 'high' && risk !== 'critical')) risk = g.risk;
			else if (g.risk === 'normal' && risk === 'low') risk = 'normal';
			if (!rationale && g.rationale) rationale = g.rationale;
		}
		ideas.push({
			key,
			title: gs.length === 1 && !rationale ? gs[0].name : firstSentence(rationale || gs[0].name),
			rationale,
			groups: gs,
			risk,
			fileDiffs: filesForGroups(gs, fileDiffs)
		});
	}
	const leftover = leftoverFiles(ideas, fileDiffs);
	if (leftover.length) {
		ideas.push({
			key: 'files:other',
			title: leftover.length === 1 ? leftover[0].path.split('/').pop() || leftover[0].path : 'Files',
			rationale: null,
			groups: [],
			risk: leftover.length ? 'normal' : 'low',
			fileDiffs: leftover
		});
	}
	const rank: Record<RiskLevel, number> = { critical: 0, high: 1, normal: 2, low: 3 };
	ideas.sort((a, b) => rank[a.risk] - rank[b.risk] || a.title.localeCompare(b.title));
	return ideas;
}

export function ceremonyItemsFromDiff(
	diff: StructDiff | null,
	rationales: Map<string, string>
): WizardItemState[] {
	if (!diff?.items?.length) return [];
	return buildWizardItems(diff.items, rationales, diff);
}

export function ideReviewHref(
	slug: string,
	opts?: {
		file?: string | null;
		construct?: string | null;
		session?: string | null;
		branch?: string | null;
	}
): string {
	const base = `/projects/${encodeURIComponent(slug)}/ide`;
	const q = new URLSearchParams();
	const file = opts?.file ? reviewRelPath(opts.file) : '';
	if (file) q.set('file', file);
	const construct = (opts?.construct || '').trim();
	if (construct) q.set('construct', construct);
	const session = (opts?.session || '').trim();
	if (session) q.set('session', session);
	const branch = (opts?.branch || '').trim();
	if (branch) q.set('branch', branch);
	const qs = q.toString();
	return qs ? `${base}?${qs}` : base;
}

export function constructNameOf(group: WizardGroup | null | undefined): string {
	if (!group) return '';
	return group.name || itemDisplayName(group.children[0]?.item) || '';
}

export function primaryFileOf(idea: CeremonyIdea, group?: WizardGroup | null): string {
	if (group) {
		const fromItem = group.children[0]?.item;
		if (fromItem?.path) return reviewRelPath(fromItem.path);
	}
	return idea.fileDiffs[0]?.path ? reviewRelPath(idea.fileDiffs[0].path) : '';
}

/** A construct declaration harvested from a unified hunk (layer keywords as written). */
export type SourceDecl = {
	verb: 'added' | 'removed';
	keyword: string;
	name: string;
};

export function declsFromHunks(files: FileDiff[]): SourceDecl[] {
	const out: SourceDecl[] = [];
	const seen = new Set<string>();
	const decl = /^\s*([a-z][a-z0-9_]*)\s+([A-Z][A-Za-z0-9_]*)\b/;
	const fnDecl = /^\s*fn\s+([A-Za-z_][A-Za-z0-9_]*)/;
	for (const f of files) {
		for (const h of f.hunks || []) {
			for (const line of h.lines || []) {
				const verb: SourceDecl['verb'] | null =
					line.startsWith('+') && !line.startsWith('+++')
						? 'added'
						: line.startsWith('-') && !line.startsWith('---')
							? 'removed'
							: null;
				if (!verb) continue;
				const body = line.slice(1);
				if (body.trim().startsWith('#')) continue;
				const named = body.match(decl);
				const fn = body.match(fnDecl);
				const keyword = named?.[1] || (fn ? 'fn' : '');
				const name = named?.[2] || fn?.[1] || '';
				if (!keyword || !name) continue;
				if (keyword === 'pkg' || keyword === 'use' || keyword === 'link') continue;
				const key = `${verb}:${keyword}:${name}`;
				if (seen.has(key)) continue;
				seen.add(key);
				out.push({ verb, keyword, name });
			}
		}
	}
	return out;
}

/** Drop machine prefixes from a PR description so the page can speak in product language. */
export function cleanPrStory(description: string): string {
	return description
		.split('\n')
		.filter((l) => !/^\s*project:\s+/i.test(l) && !/^\s*slug:\s+/i.test(l))
		.join('\n')
		.replace(/\n{3,}/g, '\n\n')
		.trim();
}

export type GuidedStep = {
	key: string;
	title: string;
	keyword: string;
	verb: string;
	rationale: string | null;
	container: string;
	files: FileDiff[];
	group: WizardGroup | null;
	peekLines: string[];
	fields: string[];
	methods: string[];
};

function peekLinesOf(group: WizardGroup | null): string[] {
	const it = group?.children[0];
	if (!it) return [];
	const peek = it.peek?.body_preview;
	if (peek?.length) return peek;
	const after = it.item.after_preview || it.item.after;
	if (Array.isArray(after) && after.length) return after.map(String);
	if (typeof after === 'string' && after.trim()) return after.split('\n');
	return [];
}

/** Guided steps are constructs / ideas — never a file dump. */
export function buildGuidedSteps(ideas: CeremonyIdea[], decls: SourceDecl[], allFiles: FileDiff[]): GuidedStep[] {
	const steps: GuidedStep[] = [];
	for (const idea of ideas) {
		if (idea.key === 'files:other') continue;
		if (idea.groups.length) {
			for (const g of idea.groups) {
				const it = g.children[0]?.item;
				const peek = g.children[0]?.peek;
				steps.push({
					key: g.key,
					title: g.name,
					keyword: peek?.subkind || it?.subkind || it?.node_kind || '',
					verb: itemKindLabel(it?.kind || 'changed'),
					rationale: g.rationale || idea.rationale,
					container: it ? containerLabel(it) : '',
					files: filesForGroups([g], idea.fileDiffs.length ? idea.fileDiffs : allFiles),
					group: g,
					peekLines: peekLinesOf(g),
					fields: peek?.fields || [],
					methods: peek?.methods || []
				});
			}
			continue;
		}
	}
	if (!steps.length && decls.length) {
		for (const d of decls) {
			steps.push({
				key: `${d.verb}:${d.keyword}:${d.name}`,
				title: d.name,
				keyword: d.keyword,
				verb: d.verb === 'added' ? 'Added' : 'Removed',
				rationale: null,
				container: '',
				files: [],
				group: null,
				peekLines: [],
				fields: [],
				methods: []
			});
		}
	}
	return steps;
}

export function storyBeats(decls: SourceDecl[], steps: GuidedStep[]): string[] {
	if (decls.length) {
		return decls.map((d) =>
			d.verb === 'added' ? `Added \`${d.keyword} ${d.name}\`` : `Removed \`${d.keyword} ${d.name}\``
		);
	}
	return steps.map((s) => {
		const kw = s.keyword ? `${s.keyword} ` : '';
		return `${s.verb} ${kw}${s.title}`.trim();
	});
}

export type HierKind = 'overview' | 'idea' | 'folder' | 'construct';

export type HierNode = {
	key: string;
	title: string;
	keyword: string;
	verb: string;
	kind: HierKind;
	rationale: string | null;
	names: string[];
	children: HierNode[];
	peekLines: string[];
	fields: string[];
	methods: string[];
};

export function mergeLabel(source?: string | null, target?: string | null): string {
	const s = (source || '').trim();
	const t = (target || 'main').trim() || 'main';
	if (!s) return '';
	return `${s} → ${t}`;
}

/** First prose block of a PR description — the overview, not the outline. */
export function storyLead(description: string): string {
	const clean = cleanPrStory(description);
	if (!clean) return '';
	const parts = clean.split(/\n(?=##\s)/);
	for (const sec of parts) {
		const head = (sec.match(/^##\s+(.+)/) || [])[1] || '';
		if (/^mission\.md$/i.test(head) || /^rationales?$/i.test(head)) continue;
		const body = sec.replace(/^##\s+[^\n]+\n*/, '').trim();
		if (!body) continue;
		return body.split(/\n\n/)[0].trim();
	}
	return clean.split(/\n\n/)[0].trim();
}

function slugKey(s: string): string {
	return s.toLowerCase().replace(/[^a-z0-9]+/g, '-').replace(/^-|-$/g, '') || 'node';
}

function parseMarkdownOutline(description: string): HierNode[] {
	const lines = cleanPrStory(description).split('\n');
	const roots: HierNode[] = [];
	let idea: HierNode | null = null;
	let folder: HierNode | null = null;
	let skip = false;
	const bullet = /^\s*[-*]\s+(?:\*\*|`)?([A-Za-z][A-Za-z0-9_]*)(?:\*\*|`)?(?:\s*[—–\-:]\s*(.+))?/;
	const h2 = /^##\s+(.+)/;
	const h3 = /^###\s+(.+)/;
	for (const raw of lines) {
		const t = raw.trim();
		const m2 = t.match(h2);
		if (m2) {
			const title = m2[1].replace(/`/g, '').trim();
			if (/^mission\.md$/i.test(title) || /^rationales?$/i.test(title)) {
				skip = /^rationales?$/i.test(title);
				idea = null;
				folder = null;
				continue;
			}
			skip = false;
			idea = {
				key: `idea:${slugKey(title)}`,
				title,
				keyword: '',
				verb: '',
				kind: 'idea',
				rationale: null,
				names: [],
				children: [],
				peekLines: [],
				fields: [],
				methods: []
			};
			folder = null;
			roots.push(idea);
			continue;
		}
		if (skip) continue;
		const m3 = t.match(h3);
		if (m3 && idea) {
			const title = m3[1].replace(/`/g, '').trim();
			folder = {
				key: `folder:${slugKey(idea.title)}:${slugKey(title)}`,
				title,
				keyword: title.toLowerCase().startsWith('group ') ? 'group' : '',
				verb: '',
				kind: 'folder',
				rationale: null,
				names: [],
				children: [],
				peekLines: [],
				fields: [],
				methods: []
			};
			idea.children.push(folder);
			continue;
		}
		const ticks = [...t.matchAll(/`([A-Za-z][A-Za-z0-9_]*)`/g)].map((m) => m[1]);
		const b = t.match(bullet);
		const names = ticks.length ? ticks : b ? [b[1]] : [];
		if (names.length && (t.startsWith('-') || t.startsWith('*'))) {
			const why = (b?.[2] || '').trim() || null;
			for (const name of names) {
				const leaf: HierNode = {
					key: `c:${slugKey(name)}`,
					title: name,
					keyword: '',
					verb: 'Added',
					kind: 'construct',
					rationale: names.length === 1 ? why : null,
					names: [name],
					children: [],
					peekLines: [],
					fields: [],
					methods: []
				};
				const parent = folder || idea;
				if (parent) parent.children.push(leaf);
				else roots.push(leaf);
			}
			continue;
		}
		if (idea && t && !t.startsWith('#')) {
			if (folder && !folder.rationale) folder.rationale = t;
			else if (!idea.rationale) idea.rationale = t;
		}
	}
	return roots.filter((n) => n.children.length > 0 || n.kind === 'construct');
}

function collectNames(n: HierNode): string[] {
	const out = [...n.names];
	for (const c of n.children) out.push(...collectNames(c));
	return [...new Set(out.filter(Boolean))];
}

function attachNames(n: HierNode) {
	n.names = collectNames(n);
	for (const c of n.children) attachNames(c);
}

function familyOf(n: HierNode): string {
	if (/Handler$|Listener$/.test(n.title)) return 'handlers';
	if (n.keyword === 'fn') return 'fns';
	if (n.keyword && n.kind === 'construct') return n.keyword;
	return '';
}

function familyTitle(fam: string, verb: string): string {
	if (fam === 'handlers') return verb ? `${verb} handlers` : 'Handlers';
	if (fam === 'fns') return verb ? `${verb} functions` : 'Functions';
	if (fam) return verb ? `${verb} ${fam}` : fam;
	return 'Group';
}

function clusterSiblings(nodes: HierNode[]): HierNode[] {
	const mapped = nodes.map((n) => ({ ...n, children: clusterSiblings(n.children) }));
	const byFam = new Map<string, HierNode[]>();
	for (const n of mapped) {
		if (n.kind !== 'construct') continue;
		const fam = familyOf(n);
		if (!fam) continue;
		const list = byFam.get(fam) || [];
		list.push(n);
		byFam.set(fam, list);
	}
	const wrapped = new Set<string>();
	const out: HierNode[] = [];
	for (const n of mapped) {
		if (n.kind !== 'construct') {
			out.push(n);
			continue;
		}
		const fam = familyOf(n);
		const bucket = fam ? byFam.get(fam) : undefined;
		if (!fam || !bucket || bucket.length < 2) {
			out.push(n);
			continue;
		}
		if (wrapped.has(fam)) continue;
		wrapped.add(fam);
		const verb = bucket.every((k) => k.verb === bucket[0].verb) ? bucket[0].verb : '';
		out.push({
			key: `cluster:${fam}:${bucket.map((k) => k.title).join(',')}`,
			title: familyTitle(fam, verb),
			keyword: bucket[0].keyword,
			verb,
			kind: 'folder',
			rationale: null,
			names: bucket.flatMap((k) => k.names),
			children: bucket,
			peekLines: [],
			fields: [],
			methods: []
		});
	}
	return out;
}

function applyDeclKeywords(nodes: HierNode[], decls: SourceDecl[]) {
	const byName = new Map(decls.map((d) => [d.name.toLowerCase(), d]));
	const walk = (n: HierNode) => {
		const d = byName.get(n.title.toLowerCase());
		if (d) {
			if (!n.keyword) n.keyword = d.keyword;
			n.verb = d.verb === 'added' ? 'Added' : d.verb === 'removed' ? 'Removed' : n.verb;
		}
		for (const c of n.children) walk(c);
	};
	for (const n of nodes) walk(n);
}

function veilFoldersFromDecls(decls: SourceDecl[]): HierNode[] {
	// Fallback when the PR has no outline: cluster by keyword.
	const byKw = new Map<string, SourceDecl[]>();
	for (const d of decls) {
		const list = byKw.get(d.keyword) || [];
		list.push(d);
		byKw.set(d.keyword, list);
	}
	const out: HierNode[] = [];
	for (const [kw, list] of byKw) {
		const kids: HierNode[] = list.map((d) => ({
			key: `c:${slugKey(d.name)}`,
			title: d.name,
			keyword: d.keyword,
			verb: d.verb === 'added' ? 'Added' : 'Removed',
			kind: 'construct',
			rationale: null,
			names: [d.name],
			children: [],
			peekLines: [],
			fields: [],
			methods: []
		}));
		if (kids.length === 1) {
			out.push(kids[0]);
			continue;
		}
		out.push({
			key: `kw:${kw}`,
			title: familyTitle(kw === 'fn' ? 'fns' : /handler|listener/i.test(kw) ? 'handlers' : kw, 'Added'),
			keyword: kw,
			verb: 'Added',
			kind: 'folder',
			rationale: null,
			names: kids.map((k) => k.title),
			children: clusterSiblings(kids),
			peekLines: [],
			fields: [],
			methods: []
		});
	}
	return out;
}

export function buildReviewTree(opts: {
	description?: string;
	decls: SourceDecl[];
	fileRationales?: { path: string; rationale: string }[];
}): HierNode[] {
	const fromMd = parseMarkdownOutline(opts.description || '');
	applyDeclKeywords(fromMd, opts.decls);
	let ideas = fromMd.map((n) => ({ ...n, children: clusterSiblings(n.children) }));
	if (!ideas.length && opts.decls.length) {
		ideas = veilFoldersFromDecls(opts.decls);
	}
	if (opts.fileRationales?.length) {
		for (const fr of opts.fileRationales) {
			const leaf = fr.path.split('/').pop() || fr.path;
			const already = ideas.some(
				(n) => n.title.toLowerCase().includes(leaf.toLowerCase().replace(/\.[^.]+$/, ''))
			);
			if (already) continue;
			ideas.push({
				key: `file:${slugKey(leaf)}`,
				title: leaf,
				keyword: '',
				verb: 'Edited',
				kind: 'idea',
				rationale: fr.rationale,
				names: [leaf],
				children: [],
				peekLines: [],
				fields: [],
				methods: []
			});
		}
	}
	for (const n of ideas) attachNames(n);
	return ideas;
}

export function flattenTree(nodes: HierNode[], depth = 0): { node: HierNode; depth: number }[] {
	const out: { node: HierNode; depth: number }[] = [];
	for (const n of nodes) {
		out.push({ node: n, depth });
		out.push(...flattenTree(n.children, depth + 1));
	}
	return out;
}

export function findNode(nodes: HierNode[], key: string): HierNode | null {
	for (const n of nodes) {
		if (n.key === key) return n;
		const hit = findNode(n.children, key);
		if (hit) return hit;
	}
	return null;
}

export function overviewBeats(tree: HierNode[]): string[] {
	return tree.map((n) => {
		const count = n.children.length;
		if (count) return `${n.title} (${count})`;
		return n.title;
	});
}

export async function requestChangesApi(prId: string, comment: string): Promise<void> {
	const r = await fetch(`/api/pull_requests/${encodeURIComponent(prId)}/request-changes`, {
		method: 'POST',
		headers: { 'Content-Type': 'application/json' },
		body: JSON.stringify({ comment: comment.trim() || 'Changes requested' })
	});
	if (!r.ok) {
		const text = await r.text().catch(() => '');
		throw new Error(`request-changes HTTP ${r.status}: ${text}`);
	}
}
