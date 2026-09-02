<script lang="ts">
	import { onMount } from 'svelte';
	import PageHeader from './PageHeader.svelte';
	import StatusPill from './StatusPill.svelte';
	import FileDiffBlock from './FileDiffBlock.svelte';
	import {
		refreshReview,
		reconcileReview,
		reviewItems,
		reviewChangeSets,
		reviewBundles,
		reviewReady,
		reviewLoadError,
		submitSignOff,
		exportAuditPack,
		changeSetForSlug,
		fetchDeployGate,
		approveBundle,
		shipBundle,
		mergeBundle,
		type OutstandingItem,
		type ChangeSet,
		type ReviewBundle,
		type BundleActionResult
	} from '$lib/review/store';
	import { fetchHubSnapshot, hubSnapshot } from '$lib/ide/store';
	import {
		fetchOpenPullRequests,
		prBelongsToProject,
		isSmokeOrFixturePr,
		loadWizardDiff,
		sendFeedbackToAgent
	} from '$lib/review/pr-api';
	import { rationalesFromPrTexts } from '$lib/review/grouping';
	import type { DiffItem, FileDiff, PullRequest, StructDiff } from '$lib/review/types';
	import { filterDiffsByNames, parseUnifiedPatch } from '$lib/review/diff';
	import {
		buildCeremonyIdeas,
		buildReviewTree,
		ceremonyItemsFromDiff,
		cleanPrStory,
		declsFromHunks,
		findNode,
		flattenTree,
		hashItems,
		ideReviewHref,
		mergeLabel,
		overviewBeats,
		requestChangesApi,
		storyLead,
		type CeremonyIdea,
		type HierNode
	} from '$lib/review/ceremony';

	interface Props {
		slug?: string;
	}
	let { slug = '' }: Props = $props();

	let note = $state('');
	let busy = $state(false);
	let shipping = $state(false);
	let error = $state('');
	let message = $state('');
	let catalogReady = $state(false);
	let prs = $state<PullRequest[]>([]);
	let repoIds = $state<Record<string, string>>({});
	let walks = $state<
		Record<
			string,
			{
				items: DiffItem[];
				fileDiffs: FileDiff[];
				hash: string;
				source: string;
				note?: string;
				diff: StructDiff | null;
			}
		>
	>({});
	let walksLoading = $state(false);
	let walkGen = 0;
	let selectedKey = $state('overview');
	let showFiles = $state(false);
	let seen = $state<Record<string, boolean>>({});
	/** Per-project dev deploy gate: true when gate=none (one-action allowed). */
	let devOneAction = $state<Record<string, boolean>>({});

	const liveNames = $derived.by(() => {
		const set = new Set<string>();
		for (const p of $hubSnapshot?.projects ?? []) {
			if (p.name) set.add(p.name);
		}
		for (const name of Object.keys(repoIds)) set.add(name);
		return set;
	});

	function projectExists(name: string): boolean {
		if (!catalogReady) return false;
		return liveNames.has(name);
	}

	const items = $derived(
		$reviewItems.filter((i) => {
			if (i.status !== 'outstanding') return false;
			if (!catalogReady) return false;
			if (!projectExists(i.slug) && !(i.repo_id && liveNames.has(i.repo_id))) {
				return false;
			}
			if (!slug) return true;
			return i.slug === slug || i.repo_id === slug;
		})
	);

	const grouped = $derived.by(() => {
		const map = new Map<string, OutstandingItem[]>();
		for (const it of items) {
			const k = it.slug || 'unknown';
			const arr = map.get(k) ?? [];
			arr.push(it);
			map.set(k, arr);
		}
		return [...map.entries()];
	});

	// ── ReviewBundles (Part B/C): a review = ONE task's per-project changes. ──
	// Filter bundles to live projects; when a route slug is set, focus the bundle
	// that touches it. This is the operator-facing unit — one headline + per-
	// project sections + one decision surface.
	const bundles = $derived(
		$reviewBundles.filter((b) => {
			if (!catalogReady) return false;
			// Keep a bundle if at least one of its projects still exists.
			return b.project_slugs.some(
				(s) => projectExists(s) || liveNames.has(s)
			);
		})
	);
	const activeBundle = $derived.by((): ReviewBundle | null => {
		if (!bundles.length) return null;
		if (slug) {
			return (
				bundles.find((b) =>
					b.project_slugs.some((s) => s === slug)
				) ?? null
			);
		}
		return bundles.length === 1 ? bundles[0] : null;
	});
	/** Two-person prod gate surfaced after a blocked merge/ship. */
	let twoPersonBlock = $state<BundleActionResult['gate'] | null>(null);
	let twoPersonAction = $state<'merge' | 'ship' | null>(null);

	function repoIdFor(name: string): string {
		return setFor(name)?.repo_id || repoIds[name] || name;
	}

	function setFor(name: string): ChangeSet | null {
		return changeSetForSlug(name, $reviewChangeSets);
	}

	function prFor(name: string): PullRequest | null {
		const live = prs.filter((p) => prBelongsToProject(p, name) && !isSmokeOrFixturePr(p));
		const open = live.find((p) =>
			['ReadyForReview', 'Approved', 'ChangesRequested', 'Draft'].includes(p.status || '')
		);
		return open || live[0] || null;
	}

	function prIdFor(name: string): string | null {
		const pr = prFor(name);
		if (pr?.id) return pr.id;
		const cs = setFor(name);
		if (cs?.pr_id) return cs.pr_id;
		const hit = items.find((i) => (i.slug === name || i.repo_id === name) && i.pr_id);
		return hit?.pr_id || null;
	}

	async function fetchJson(url: string, ms = 20000): Promise<unknown | null> {
		const ctrl = new AbortController();
		const t = setTimeout(() => ctrl.abort(), ms);
		try {
			const r = await fetch(url, { signal: ctrl.signal });
			if (!r.ok) return null;
			return await r.json();
		} catch {
			return null;
		} finally {
			clearTimeout(t);
		}
	}

	async function loadWalks(names: string[]) {
		const gen = ++walkGen;
		walksLoading = true;
		const next: Record<
			string,
			{
				items: DiffItem[];
				fileDiffs: FileDiff[];
				hash: string;
				source: string;
				note?: string;
				diff: StructDiff | null;
			}
		> = {};
		await Promise.all(
			names.map(async (name) => {
				if (!projectExists(name)) return;
				const prId = prIdFor(name);
				const cs = setFor(name);
				try {
					let fileDiffs: FileDiff[] = [];
					let items: DiffItem[] = [];
					let source = 'none';
					let note: string | undefined;
					let diff: StructDiff | null = null;
					if (prId) {
						try {
							const loaded = await Promise.race([
								loadWizardDiff({
									prId,
									slug: name,
									allowWorkingTreeFallback: false
								}),
								new Promise<never>((_, rej) =>
									setTimeout(() => rej(new Error('diff timeout')), 18000)
								)
							]);
							diff = loaded.diff;
							items = Array.isArray(loaded.diff.items) ? loaded.diff.items : [];
							fileDiffs = Array.isArray(loaded.diff.file_diffs) ? loaded.diff.file_diffs : [];
							source = loaded.source;
							note = loaded.note;
						} catch {
							/* fall through */
						}
					}
					if (!fileDiffs.length && !items.length) {
						try {
							const loaded = await Promise.race([
								loadWizardDiff({
									prId: null,
									slug: name,
									allowWorkingTreeFallback: true
								}),
								new Promise<never>((_, rej) =>
									setTimeout(() => rej(new Error('diff timeout')), 12000)
								)
							]);
							if (!diff) diff = loaded.diff;
							if (!items.length) items = Array.isArray(loaded.diff.items) ? loaded.diff.items : [];
							if (!fileDiffs.length && Array.isArray(loaded.diff.file_diffs)) {
								fileDiffs = loaded.diff.file_diffs;
							}
							if (source === 'none' || source === 'pr-empty') source = loaded.source;
							if (!note) note = loaded.note;
						} catch {
							/* fall through */
						}
					}
					if (!fileDiffs.length) {
						const extra = (await fetchJson(`/api/p/${encodeURIComponent(name)}/diff`)) as {
							items?: DiffItem[];
							file_diffs?: FileDiff[];
						} | null;
						if (extra) {
							if (!items.length) items = Array.isArray(extra.items) ? extra.items : [];
							if (Array.isArray(extra.file_diffs) && extra.file_diffs.length) {
								fileDiffs = extra.file_diffs;
							}
							if (source === 'none') source = 'working-tree';
						}
					}
					if (!fileDiffs.length && cs?.session_id) {
						const data = (await fetchJson(
							`/api/sessions/${encodeURIComponent(cs.session_id)}/diff`
						)) as { patch?: string; git_patch?: string } | null;
						if (data) {
							const patch = String(data.patch || data.git_patch || '');
							fileDiffs = parseUnifiedPatch(patch);
							if (fileDiffs.length && source === 'none') source = 'git';
						}
					}
					next[name] = {
						items,
						fileDiffs,
						hash: hashItems(items),
						source,
						note,
						diff
					};
				} catch (e) {
					next[name] = {
						items: [],
						fileDiffs: [],
						hash: '',
						source: 'none',
						note: e instanceof Error ? e.message : String(e),
						diff: null
					};
				}
			})
		);
		if (gen !== walkGen) {
			return false;
		}
		walks = next;
		walksLoading = false;
		return true;
	}

	let lastWalkKey = '';

	const waiting = $derived(!$reviewReady || !catalogReady);
	// The project whose diff-walk is shown. Within a bundle, default to the
	// route slug or the bundle's first (most-outstanding) project.
	const ceremonySlug = $derived(
		slug || (activeBundle ? activeBundle.projects[0]?.slug ?? '' : grouped.length === 1 ? grouped[0][0] : '')
	);

	const ceremony = $derived.by(() => {
		const name = ceremonySlug;
		if (!name) {
			return {
				name: '',
				ideas: [] as CeremonyIdea[],
				walk: null as (typeof walks)[string] | null,
				cs: null as ChangeSet | null,
				pr: null as PullRequest | null
			};
		}
		const walk = walks[name] ?? null;
		const cs = setFor(name);
		const pr = prFor(name);
		const rats = rationalesFromPrTexts(pr?.description || '');
		const wiz = ceremonyItemsFromDiff(walk?.diff ?? null, rats);
		const ideas = buildCeremonyIdeas(
			wiz.length ? wiz : ceremonyItemsFromDiff(
				walk
					? {
							base_label: '',
							head_label: '',
							items: walk.items,
							added: 0,
							removed: 0,
							changed: 0,
							file_diffs: walk.fileDiffs
						}
					: null,
				rats
			),
			walk?.fileDiffs ?? []
		);
		return { name, ideas, walk, cs, pr };
	});

	const decls = $derived(declsFromHunks(ceremony.walk?.fileDiffs ?? []));
	const tree = $derived.by(() => {
		const fileRats = items
			.filter((i) => i.slug === ceremony.name && i.kind === 'file_edit' && i.rationale && i.path)
			.map((i) => ({ path: i.path as string, rationale: i.rationale as string }));
		return buildReviewTree({
			description: ceremony.pr?.description || '',
			decls,
			fileRationales: fileRats
		});
	});
	const flat = $derived(flattenTree(tree));
	const navKeys = $derived(['overview', ...flat.map((f) => f.node.key)]);
	const beats = $derived(overviewBeats(tree));
	const merge = $derived(
		mergeLabel(ceremony.pr?.source_branch, ceremony.pr?.target_branch || 'main')
	);
	const headline = $derived.by(() => {
		if (merge) return merge;
		const pr = ceremony.pr;
		if (pr?.title && !isSmokeOrFixturePr(pr)) return pr.title;
		return ceremony.name || 'Review';
	});
	const why = $derived.by(() => {
		const lead = ceremony.pr?.description ? storyLead(ceremony.pr.description) : '';
		if (lead) return lead;
		const extras = items
			.filter((i) => i.slug === ceremony.name && i.rationale)
			.map((i) => i.rationale as string);
		return extras[0] || ceremony.pr?.title || '';
	});
	const current = $derived(
		selectedKey === 'overview' ? null : findNode(tree, selectedKey)
	);
	const isOverview = $derived(selectedKey === 'overview' || !current);
	const drillFiles = $derived.by((): FileDiff[] => {
		const all = ceremony.walk?.fileDiffs ?? [];
		if (isOverview) return all;
		return filterDiffsByNames(all, current?.names || [current?.title || '']);
	});

	function selectKey(key: string) {
		selectedKey = key;
		showFiles = false;
		seen = { ...seen, [`${ceremony.name}:${key}`]: true };
	}

	function goRel(dir: number) {
		const i = navKeys.indexOf(selectedKey);
		const next = navKeys[Math.max(0, Math.min(navKeys.length - 1, (i < 0 ? 0 : i) + dir))];
		if (next) selectKey(next);
	}

	$effect(() => {
		if (!ceremony.name) return;
		if (!seen[`${ceremony.name}:overview`]) {
			seen = { ...seen, [`${ceremony.name}:overview`]: true };
		}
	});

	onMount(() => {
		void refreshReview();
		void fetchHubSnapshot()
			.then(async (snap) => {
				const live = (snap.projects || []).map((p) => p.name).filter(Boolean);
				if (live.length) await reconcileReview(live);
			})
			.finally(() => {
				catalogReady = true;
			});
		void fetchOpenPullRequests()
			.then((list) => {
				prs = list;
			})
			.catch(() => {
				prs = [];
			});
		void fetch('/api/repos')
			.then((r) => (r.ok ? r.json() : []))
			.then((list) => {
				const map: Record<string, string> = {};
				for (const row of Array.isArray(list) ? list : []) {
					const rec = row as Record<string, unknown>;
					const name = String(rec.name || rec.slug || '');
					const id =
						rec.id != null && typeof rec.id === 'object' && rec.id && 'value' in (rec.id as object)
							? String((rec.id as { value?: string }).value || '')
							: String(rec.id || rec.slug || '');
					if (name && id) map[name] = id;
					if (rec.slug && id) map[String(rec.slug)] = id;
				}
				repoIds = map;
			})
			.catch(() => {});
	});

	$effect(() => {
		const names = grouped.map(([n]) => n);
		const key = names
			.map((n) => `${n}:${prIdFor(n) || ''}`)
			.sort()
			.join('|');
		if (!catalogReady || !names.length) return;
		if (key === lastWalkKey) return;
		lastWalkKey = key;
		void loadWalks(names).then((ok) => {
			if (!ok) lastWalkKey = '';
		});
	});

	// Resolve the dev deploy gate for the project(s) in view so the UI can
	// offer one-action "Approve & Deploy" only when dev gate=none.
	$effect(() => {
		const name = ceremonySlug;
		if (!name || !catalogReady) return;
		if (name in devOneAction) return;
		void fetchDeployGate(name, 'dev').then((g) => {
			devOneAction = { ...devOneAction, [name]: g.one_action_ship };
		});
	});

	async function approvePrIfAny(prId?: string | null) {
		if (!prId) return;
		try {
			await fetch(`/api/pull_requests/${encodeURIComponent(prId)}/approve`, {
				method: 'POST',
				headers: { 'Content-Type': 'application/json' },
				body: JSON.stringify({ reviewer: 'operator', comment: note.trim() || 'Approved' })
			});
		} catch {
			/* merge still gated on the audit */
		}
	}

	async function act(decision: 'approve' | 'reject', proj: string) {
		busy = true;
		error = '';
		message = '';
		const cs = setFor(proj);
		const pr = prFor(proj);
		const walk = walks[proj];
		const res = await submitSignOff({
			slug: proj,
			decision,
			note: note.trim() || undefined,
			git_sha: cs?.git_sha || undefined,
			structural_diff_hash: walk?.hash,
			host_check: cs?.host_check,
			pr_id: pr?.id || cs?.pr_id || undefined
		});
		if (!res.ok) {
			busy = false;
			error = res.error || 'Could not record the decision';
			return;
		}
		const prId = pr?.id || res.approve_pr || cs?.pr_id;
		if (decision === 'approve') {
			await approvePrIfAny(prId);
		} else if (prId) {
			try {
				await requestChangesApi(prId, note.trim() || 'Changes requested');
			} catch {
				/* audit already recorded */
			}
			if (note.trim()) {
				sendFeedbackToAgent(
					[
						{
							index: 0,
							path: proj,
							name: proj,
							kind: 'review',
							text: note.trim()
						}
					],
					pr?.title
				);
			}
		}
		note = '';
		message =
			decision === 'approve'
				? `Approved ${proj}. It can be deployed.`
				: `Requested changes on ${proj}.`;
		busy = false;
		await refreshReview();
	}

	async function ship(name: string) {
		shipping = true;
		error = '';
		message = '';
		const pr = prFor(name);
		const cs = setFor(name);
		try {
			if (pr?.id && pr.status !== 'Merged') {
				const { mergeChangeApi } = await import('$lib/review/pr-api');
				await mergeChangeApi(pr.id, name);
			}
			const rid = repoIdFor(name);
			const r = await fetch('/api/provision-project', {
				method: 'POST',
				headers: { 'Content-Type': 'application/json' },
				body: JSON.stringify({
					project_slug: name,
					environment: 'dev',
					repo_id: rid,
					branch: 'main'
				})
			});
			const data = await r.json().catch(() => ({}));
			if (!r.ok || data.ok === false) {
				throw new Error(String(data.message || data.error || `HTTP ${r.status}`));
			}
			message = `Deploying ${name}${cs?.git_sha ? ` @ ${cs.git_sha.slice(0, 8)}` : ''}.`;
			window.location.href = `/projects/${encodeURIComponent(name)}`;
		} catch (e) {
			error = e instanceof Error ? e.message : String(e);
		} finally {
			shipping = false;
		}
	}

	/**
	 * One-action Approve & Deploy for dev-gated projects: record the human
	 * sign-off, then ship to the lower env — reusing the existing sign-off and
	 * deploy code paths behind a single confirm. Only offered when the dev gate
	 * is `none` (see devOneAction); prod stays behind the explicit ceremony.
	 */
	async function approveAndDeploy(proj: string) {
		if (busy || shipping) return;
		const ok = window.confirm(
			`Approve and deploy ${proj} to dev in one action?\n\nThis records your sign-off and ships to the dev environment.`
		);
		if (!ok) return;
		await act('approve', proj);
		if (error) return; // sign-off failed — do not ship
		await ship(proj);
	}

	/**
	 * Non-destructive decline: leave the change OUTSTANDING (no reject, no
	 * mutation) so the agent can keep iterating and the operator reviews again
	 * later. Just returns to the IDE.
	 */
	function keepWorking(proj: string) {
		message = `Left ${proj} open — keep working. It stays in review until you approve.`;
		error = '';
		note = '';
		if (projectExists(proj)) {
			window.location.href = ideReviewHref(proj, ideOpts(proj));
		}
	}

	// ── Bundle-level decisions (Part C): act on the WHOLE task ──────────────
	function bundleErr(res: BundleActionResult): boolean {
		if (res.error === 'two_person_required') {
			twoPersonBlock = res.gate ?? null;
			twoPersonAction = twoPersonAction; // keep the attempted action
			error = res.message || 'A second approver is required for production.';
			return true;
		}
		if (!res.ok) {
			error = res.message || 'Action failed';
			return true;
		}
		return false;
	}

	async function approveBundleAction() {
		if (!activeBundle || busy || shipping) return;
		busy = true;
		error = '';
		message = '';
		twoPersonBlock = null;
		const res = await approveBundle(activeBundle.id, note.trim() || undefined);
		busy = false;
		if (bundleErr(res)) return;
		note = '';
		message = `Approved the task (${activeBundle.project_slugs.join(', ')}). It can be merged and deployed.`;
	}

	async function shipBundleAction(environment = 'dev', override = false) {
		if (!activeBundle || busy || shipping) return;
		if (activeBundle.host_has_errors) {
			error = 'Fix compile errors in the task before deploying.';
			return;
		}
		const label = activeBundle.project_slugs.length > 1 ? `${activeBundle.project_slugs.length} projects` : activeBundle.project_slugs[0];
		if (!override) {
			const ok = window.confirm(
				`Approve, merge and deploy this task (${label}) to ${environment}?\n\nThis records your sign-off, merges each project to main, and deploys.`
			);
			if (!ok) return;
		}
		shipping = true;
		twoPersonAction = 'ship';
		error = '';
		message = '';
		twoPersonBlock = null;
		const res = await shipBundle(activeBundle.id, {
			environment,
			note: note.trim() || undefined,
			override_two_person: override
		});
		shipping = false;
		if (bundleErr(res)) return;
		note = '';
		twoPersonAction = null;
		message = `Shipping the task to ${environment} (${label}).`;
	}

	async function mergeBundleAction(environment = 'dev', override = false) {
		if (!activeBundle || busy || shipping) return;
		shipping = true;
		twoPersonAction = 'merge';
		error = '';
		message = '';
		twoPersonBlock = null;
		const res = await mergeBundle(activeBundle.id, {
			environment,
			note: note.trim() || undefined,
			override_two_person: override
		});
		shipping = false;
		if (bundleErr(res)) return;
		note = '';
		twoPersonAction = null;
		message = `Merged the task (${activeBundle.project_slugs.join(', ')}).`;
	}

	/** Confirm + re-run the blocked action with the two-person override. */
	async function overrideTwoPerson() {
		const ok = window.confirm(
			'OVERRIDE the two-person production rule?\n\nYou are merging to production WITHOUT a second distinct approver. This is recorded in the audit trail with your identity. Proceed only for a genuine exception.'
		);
		if (!ok) return;
		if (twoPersonAction === 'ship') await shipBundleAction('prod', true);
		else await mergeBundleAction('prod', true);
	}

	/**
	 * Request changes = talk to the agent (Part D). Instead of a reject, hand the
	 * operator's note to the agent as a revision request on THIS bundle so it
	 * revises the same PRs. Non-destructive: the bundle stays outstanding.
	 */
	function requestChangesToAgent() {
		if (!activeBundle) return;
		const text = note.trim();
		if (!text) {
			error = 'Type what you want changed, then Request changes.';
			return;
		}
		const projects = activeBundle.project_slugs.join(', ');
		const prompt =
			`Revise the open review (bundle ${activeBundle.id}, projects: ${projects}). ` +
			`Do NOT open a new PR — amend the SAME branches/PRs for these projects, ` +
			`refresh the rationales/summary, and re-alert when ready.\n\nRequested changes:\n${text}`;
		void import('$lib/agent/runtimeAgentSession')
			.then((m) => {
				const send = (m as { agentSend?: (t: string) => void }).agentSend;
				if (typeof send === 'function') send(prompt);
			})
			.catch(() => {});
		message = `Sent your changes to the agent for ${projects}. It will revise this review.`;
		note = '';
	}

	function sessionFor(name: string): string {
		return setFor(name)?.session_id || '';
	}

	function branchFor(name: string): string {
		const pr = prFor(name);
		const b = (pr?.source_branch || '').trim();
		if (b && b !== 'main' && b !== 'master') return b;
		return '';
	}

	function ideOpts(name: string, extra?: { file?: string | null; construct?: string | null }) {
		return {
			...extra,
			session: sessionFor(name),
			branch: branchFor(name)
		};
	}

	function openIdeHref(name: string, n?: HierNode | null): string {
		return ideReviewHref(
			name,
			ideOpts(name, {
				file: n?.names.find((x) => x.includes('.')) || null,
				construct: n && n.kind === 'construct' ? n.title : n?.names[0] || null
			})
		);
	}

	function onKey(e: KeyboardEvent) {
		if (busy) return;
		const tag = (e.target as HTMLElement)?.tagName;
		if (tag === 'INPUT' || tag === 'TEXTAREA' || tag === 'SELECT') return;
		if (e.key === 'ArrowDown' || e.key === 'j') {
			e.preventDefault();
			goRel(1);
		} else if (e.key === 'ArrowUp' || e.key === 'k') {
			e.preventDefault();
			goRel(-1);
		} else if (e.key === 'd' || e.key === 'D') {
			e.preventDefault();
			showFiles = !showFiles;
		}
	}
</script>

<svelte:window onkeydown={onKey} />

<div
	class="review"
	class:ceremony-mode={!!ceremonySlug && items.length > 0}
	data-veil-role="sign-off"
	data-veil-agent={JSON.stringify({
		intent: 'review',
		entity: 'ReviewBundle',
		notes: [
			'A review = ONE task (ReviewBundle) spanning all projects it touched. One headline + per-project sections + ONE decision surface.',
			'The human decides for the WHOLE task from the rolled-up summary — no need to read every diff or the transcript.',
			'Non-prod: Approve + Merge + Deploy is one action (approve → merge each project → deploy each).',
			'Prod (gate=sign_off) needs a second distinct approver (two-person, deferred seam); an audited override exists.',
			'"Request changes" hands the note to the AGENT to revise the SAME PRs — it is not a reject.',
			'"Not yet — keep working" leaves the task outstanding (non-destructive).',
			'The agent must not press Approve.',
		],
		actions: [
			{ id: 'approve', label: 'Approve', method: 'api' },
			{ id: 'approve-and-deploy', label: 'Approve + Merge + Deploy', method: 'api' },
			{ id: 'merge', label: 'Merge', method: 'api' },
			{ id: 'reject', label: 'Request changes (talk to agent)', method: 'ui' },
			{ id: 'keep-working', label: 'Not yet — keep working', method: 'ui' },
			{ id: 'override-two-person', label: 'Override two-person (audited)', method: 'api' },
		],
		api: {
			list: 'GET /api/review/bundles',
			approve: 'POST /api/review/bundles/{id}/approve',
			merge: 'POST /api/review/bundles/{id}/merge',
			ship: 'POST /api/review/bundles/{id}/ship',
			signOff: 'POST /api/review/sign_off',
			export: 'GET /api/review/export',
		},
	})}
>
	<PageHeader
		title={ceremonySlug && merge ? merge : ceremonySlug ? `Review · ${ceremonySlug}` : 'Review'}
		description={ceremonySlug ? ceremonySlug : 'Approve to unlock deploy.'}
	>
		{#snippet actions()}
			{#if slug}
				<a class="btn-outline" href="/review">All reviews</a>
			{:else}
				<a class="btn-outline" href="/projects">Projects</a>
			{/if}
			{#if ceremonySlug && projectExists(ceremonySlug)}
				<a class="btn-outline" href={ideReviewHref(ceremonySlug, ideOpts(ceremonySlug))}>Open in IDE</a>
			{/if}
			<button type="button" class="btn-ghost" onclick={() => void exportAuditPack()}>Export record</button>
		{/snippet}
	</PageHeader>

	{#if error || $reviewLoadError}
		<p class="dk-error" role="alert">{error || $reviewLoadError}</p>
	{/if}
	{#if message}
		<p class="ok">{message}</p>
	{/if}

	{#if waiting}
		<div class="card dk-loading" role="status" aria-live="polite" aria-busy="true">
			<div class="dk-spinner" aria-hidden="true"></div>
			<span>Loading…</span>
		</div>
		<div class="skel" aria-hidden="true">
			<div class="skel-card"></div>
			<div class="skel-card"></div>
			<div class="skel-card short"></div>
		</div>
	{:else if items.length === 0}
		<div class="card empty appear">
			<p>Nothing to review{slug ? ` for ${slug}` : ''}.</p>
			<p class="hint">When a project has unapproved work, it shows up here.</p>
			{#if slug && projectExists(slug)}
				<button type="button" class="btn-primary" disabled={shipping} onclick={() => ship(slug)}>
					{shipping ? 'Deploying…' : 'Deploy'}
				</button>
			{/if}
		</div>
	{:else if !ceremonySlug}
		<div class="queue appear">
			<p class="hint queue-lead">Select a task to review.</p>
			{#if bundles.length}
				{#each bundles as b}
					<a class="proj-card card" href={`/review/${encodeURIComponent(b.projects[0]?.slug ?? '')}`}>
						<div class="proj-h">
							<strong>{b.title}</strong>
							{#each b.project_slugs as ps}
								<StatusPill label={ps} variant="warning" />
							{/each}
							{#if b.host_has_errors}
								<span class="err-pill">errors</span>
							{/if}
						</div>
						<p class="sum">{b.summary}</p>
					</a>
				{/each}
			{:else}
				{#each grouped as [proj, rows]}
					{@const cs = setFor(proj)}
					{@const pr = prFor(proj)}
					<a class="proj-card card" href={`/review/${encodeURIComponent(proj)}`}>
						<div class="proj-h">
							<strong>{pr?.title || proj}</strong>
							<StatusPill label={proj} variant="warning" />
							{#if cs?.git_sha}
								<code class="sha-pill">{cs.git_sha.slice(0, 8)}</code>
							{/if}
						</div>
						<p class="sum">{cleanPrStory(pr?.description || '') || rows.find((r) => r.rationale)?.rationale || cs?.summary || 'Outstanding work'}</p>
					</a>
				{/each}
			{/if}
		</div>
	{:else}
		{@const proj = ceremony.name}
		{@const cs = ceremony.cs}
		{@const walk = ceremony.walk}
		<div class="ceremony appear">
			<aside class="hier" aria-label="Changes">
				{#if walksLoading && !walk}
					<div class="walk-loading" role="status">
						<span class="dk-spinner" aria-hidden="true"></span>
						<span>Loading…</span>
					</div>
				{/if}
				<nav>
					<button
						type="button"
						class="idea-btn"
						class:on={isOverview}
						data-key="overview"
						onclick={() => selectKey('overview')}
					>
						<span class="idea-title">Overview</span>
					</button>
					{#each flat as { node: n, depth }}
						<button
							type="button"
							class="idea-btn depth-{Math.min(depth, 3)}"
							class:on={selectedKey === n.key}
							class:folder={n.kind === 'idea' || n.kind === 'folder'}
							data-key={n.key}
							onclick={() => selectKey(n.key)}
							style="padding-left: {0.5 + depth * 0.75}rem"
						>
							{#if n.verb}
								<span class="kind {n.verb.toLowerCase().startsWith('add') ? 'add' : n.verb.toLowerCase().startsWith('rem') ? 'rem' : 'chg'}">{n.verb}</span>
							{/if}
							{#if n.keyword}
								<code class="kw">{n.keyword}</code>
							{/if}
							<span class="idea-title">{n.title}</span>
							{#if n.children.length}
								<span class="dim">{n.children.length}</span>
							{/if}
						</button>
					{/each}
				</nav>
			</aside>

			<section class="drill">
				{#if cs?.host_has_errors}
					<p class="check-banner" role="status">
						This project still has compile errors{cs.host_check?.error_count
							? ` (${cs.host_check.error_count})`
							: ''}.
						{#if cs.host_check?.summary}
							<span class="hint">{cs.host_check.summary}</span>
						{/if}
					</p>
				{/if}

				{#if isOverview}
					<article class="story">
						{#if activeBundle && activeBundle.project_slugs.length > 1}
							<section class="bundle-banner" data-veil-role="bundle-summary">
								<span class="bundle-tag">One task · {activeBundle.project_slugs.length} projects</span>
								<h2 class="story-h">{activeBundle.title}</h2>
								<p class="bundle-sum">{activeBundle.summary}</p>
								<div class="bundle-projects">
									{#each activeBundle.projects as bp}
										<a
											class="bundle-proj"
											class:on={bp.slug === ceremonySlug}
											href={`/review/${encodeURIComponent(bp.slug)}`}
										>
											<span class="bp-slug">{bp.slug}</span>
											<span class="bp-head">{bp.change_summary?.headline || `${bp.outstanding} change(s)`}</span>
											<span
												class="bp-check"
												class:err={bp.host_has_errors}
											>{bp.change_summary?.check_status || (bp.host_has_errors ? 'errors' : 'clean')}</span>
										</a>
									{/each}
								</div>
								<p class="condensed-hint">
									Reviewing <strong>{ceremonySlug}</strong> below. Approve / merge / deploy
									decisions apply to the whole task.
								</p>
							</section>
						{/if}
						{#if cs?.change_summary}
							{@const sum = cs.change_summary}
							<section class="condensed" data-veil-role="change-summary">
								<h2 class="story-h">{sum.headline || headline}</h2>
								<div class="condensed-meta">
									<span
										class="check-chip"
										class:check-chip--clean={sum.error_count === 0}
										class:check-chip--err={sum.error_count > 0}
										class:check-chip--warn={sum.error_count === 0 && sum.warning_count > 0}
									>
										{sum.check_status || 'checks clean'}
									</span>
									{#if sum.files.length}
										<span class="files-chip" title={sum.files.join('\n')}>
											{sum.files.length}
											{sum.files.length === 1 ? 'file' : 'files'} touched
										</span>
									{/if}
								</div>
								{#if sum.files.length}
									<ul class="files-list">
										{#each sum.files.slice(0, 8) as f}
											<li><code>{f}</code></li>
										{/each}
										{#if sum.files.length > 8}
											<li class="dim">+{sum.files.length - 8} more</li>
										{/if}
									</ul>
								{/if}
								{#if sum.why.length > 1}
									<ul class="why-list">
										{#each sum.why.slice(0, 5) as w}
											<li>{w}</li>
										{/each}
									</ul>
								{/if}
								<p class="condensed-hint">
									Decide from this summary — expand the diff below only if you need the exact
									changes.
								</p>
							</section>
						{:else}
							<h2 class="story-h">{headline}</h2>
						{/if}
						{#if ceremony.pr?.title}
							<p class="pr-title">{ceremony.pr.title}</p>
						{/if}
						{#if why}
							<p class="story-why">{why}</p>
						{/if}
						{#if beats.length}
							<ul class="beats">
								{#each beats as b}
									<li>{b}</li>
								{/each}
							</ul>
						{/if}
					</article>
				{:else if current}
					<div class="item-card {current.verb.toLowerCase().startsWith('add') ? 'add' : current.verb.toLowerCase().startsWith('rem') ? 'rem' : 'chg'}">
						<div class="drill-h">
							<div>
								{#if current.verb}
									<span class="kind">{current.verb}</span>
								{/if}
								{#if current.keyword}
									<code class="kw">{current.keyword}</code>
								{/if}
								<h3>{current.title}</h3>
							</div>
							<a class="btn-ghost" href={openIdeHref(proj, current)}>Open in IDE</a>
						</div>

						{#if current.rationale}
							<section class="rationale">
								<h4>Why</h4>
								<p>{current.rationale}</p>
							</section>
						{/if}

						{#if current.children.length}
							<ul class="parts">
								{#each current.children as ch}
									<li>
										<button type="button" class="linkish" onclick={() => selectKey(ch.key)}>
											{#if ch.keyword}<code class="kw">{ch.keyword}</code>{/if}
											{ch.title}
										</button>
									</li>
								{/each}
							</ul>
						{/if}

						{#if current.fields.length || current.methods.length}
							<section class="shape">
								{#if current.fields.length}
									<p><span class="dim">Fields</span> {current.fields.join(' · ')}</p>
								{/if}
								{#if current.methods.length}
									<p><span class="dim">Methods</span> {current.methods.join(' · ')}</p>
								{/if}
							</section>
						{/if}

						{#if current.peekLines.length}
							<section class="critical">
								<h4>Critical body</h4>
								<pre class="after">{current.peekLines.join('\n')}</pre>
							</section>
						{/if}

						<div class="walk-nav">
							<button
								type="button"
								class="btn-outline"
								onclick={() => (showFiles = !showFiles)}
							>
								{showFiles ? 'Hide' : 'Show'} exact changes
							</button>
						</div>

						{#if showFiles}
							{#if drillFiles.length}
								{#each drillFiles as fd}
									<FileDiffBlock
										diff={fd}
										ideHref={ideReviewHref(
											proj,
											ideOpts(proj, { file: fd.path, construct: current.title })
										)}
									/>
								{/each}
							{:else}
								<p class="hint">No isolated hunk for this item. Open it in the IDE.</p>
							{/if}
						{/if}
					</div>
				{:else if walksLoading}
					<p class="hint">Loading…</p>
				{:else if walk?.note}
					<p class="hint">{walk.note}</p>
				{/if}

				<div class="card actions" data-veil-role="create-form">
					{#if activeBundle && activeBundle.project_slugs.length > 1}
						<p class="bundle-scope">
							This decision applies to the whole task —
							<strong>{activeBundle.project_slugs.length} projects</strong>:
							{activeBundle.project_slugs.join(', ')}.
						</p>
					{/if}
					<label class="note">
						<span>Note (optional)</span>
						<textarea class="input" bind:value={note} rows="2" placeholder="Why you approve — or, to request changes, what you want the agent to change"></textarea>
					</label>
					{#if twoPersonBlock && twoPersonBlock.active && !twoPersonBlock.satisfied}
						<div class="two-person" role="alert" data-veil-role="two-person-gate">
							<p class="two-person-h">Second approver required (production)</p>
							<p class="hint">
								{twoPersonBlock.blocked.join(', ')} need a second distinct approver
								(have {twoPersonBlock.approvals.map((a) => a[1]).join(', ')} of {twoPersonBlock.required}).
								Ask another operator to approve, or override with an audited reason.
							</p>
							<button
								type="button"
								class="btn-outline danger"
								data-veil-action="override-two-person"
								disabled={busy || shipping}
								onclick={() => overrideTwoPerson()}
							>
								Override (audited) — merge to prod without a second approver
							</button>
						</div>
					{/if}
					<div class="btns">
						{#if activeBundle && devOneAction[ceremonySlug]}
							<button
								type="button"
								class="btn-primary"
								data-veil-action="approve-and-deploy"
								disabled={busy || shipping || activeBundle.host_has_errors}
								title={activeBundle.host_has_errors
									? 'Fix compile errors before deploying'
									: 'Approve, merge and deploy the whole task to dev'}
								onclick={() => shipBundleAction('dev')}
							>
								{shipping ? 'Shipping…' : busy ? 'Approving…' : 'Approve + Merge + Deploy'}
							</button>
							<button
								type="button"
								class="btn-outline"
								data-veil-action="sign-off"
								disabled={busy || shipping}
								onclick={() => approveBundleAction()}
							>
								{busy ? 'Saving…' : 'Approve only'}
							</button>
							<button
								type="button"
								class="btn-outline"
								data-veil-action="merge"
								disabled={busy || shipping}
								title="Approve + merge to main without deploying"
								onclick={() => mergeBundleAction('dev')}
							>
								{shipping ? 'Merging…' : 'Merge'}
							</button>
						{:else if activeBundle}
							<button
								type="button"
								class="btn-primary"
								data-veil-action="sign-off"
								disabled={busy || shipping}
								onclick={() => approveBundleAction()}
							>
								{busy ? 'Saving…' : 'Approve'}
							</button>
							<button
								type="button"
								class="btn-outline"
								data-veil-action="merge"
								disabled={busy || shipping}
								title="Merge to main (production gates apply)"
								onclick={() => mergeBundleAction('dev')}
							>
								{shipping ? 'Merging…' : 'Merge'}
							</button>
						{:else}
							<button
								type="button"
								class="btn-primary"
								data-veil-action="sign-off"
								disabled={busy}
								onclick={() => act('approve', proj)}
							>
								{busy ? 'Saving…' : 'Approve'}
							</button>
						{/if}
						<button
							type="button"
							class="btn-outline"
							data-veil-action="reject-sign-off"
							disabled={busy || shipping}
							title="Hand your note to the agent to revise this same review"
							onclick={() => (activeBundle ? requestChangesToAgent() : act('reject', proj))}
						>
							Request changes
						</button>
						<button
							type="button"
							class="btn-ghost"
							data-veil-action="keep-working"
							disabled={busy || shipping}
							onclick={() => keepWorking(proj)}
							title="Leave this change open so the agent can keep iterating"
						>
							Not yet — keep working
						</button>
					</div>
				</div>
			</section>
		</div>
	{/if}
</div>

<style>
	.review {
		max-width: 1120px;
		flex: 1 1 auto;
		min-height: 0;
		display: flex;
		flex-direction: column;
	}
	.review.ceremony-mode { max-width: none; }
	.ok { color: var(--dk-ok, #34d399); }
	.empty { padding: 1.25rem; }
	.appear { animation: dk-fade-up 0.4s var(--dk-ease-out) both; }
	.skel {
		display: flex;
		flex-direction: column;
		gap: 0.75rem;
		margin-top: 0.85rem;
	}
	.skel-card {
		height: 5.5rem;
		border-radius: 10px;
		background: linear-gradient(
			90deg,
			color-mix(in oklab, var(--dk-border-soft, #27272a) 70%, transparent) 0%,
			color-mix(in oklab, #52525b 28%, transparent) 50%,
			color-mix(in oklab, var(--dk-border-soft, #27272a) 70%, transparent) 100%
		);
		background-size: 200% 100%;
		animation: signoff-shimmer 1.2s ease-in-out infinite;
	}
	.skel-card.short { height: 3.25rem; width: 70%; }
	@keyframes signoff-shimmer {
		0% { background-position: 100% 0; }
		100% { background-position: -100% 0; }
	}
	.hint { opacity: 0.7; font-size: 0.9rem; }
	.queue { display: flex; flex-direction: column; gap: 0.75rem; }
	.queue-lead { margin: 0 0 0.2rem; }
	.proj-card {
		display: block;
		padding: 0.9rem 1rem;
		text-decoration: none;
		color: inherit;
	}
	.proj-card:hover { border-color: var(--dk-brand-light, #a3a3a3); }
	.proj-h { display: flex; align-items: center; gap: 0.55rem; flex-wrap: wrap; }
	.proj-card .sum { margin: 0.4rem 0 0; opacity: 0.75; font-size: 0.88rem; }
	.sha-pill {
		font-size: 0.75rem;
		opacity: 0.65;
		font-family: ui-monospace, monospace;
	}
	.check-banner {
		padding: 0.65rem 0.85rem;
		border-radius: 8px;
		background: color-mix(in oklab, #f59e0b 16%, transparent);
		margin: 0 0 1rem;
		font-size: 0.88rem;
	}
	.ceremony {
		display: grid;
		grid-template-columns: minmax(220px, 280px) minmax(0, 1fr);
		gap: 1rem;
		align-items: stretch;
		flex: 1 1 auto;
		min-height: 0;
	}
	.hier {
		border: 1px solid var(--dk-border-soft, #27272a);
		border-radius: 10px;
		padding: 0.65rem 0.45rem 0.8rem;
		background: color-mix(in oklab, var(--dk-surface, #1a1a1a) 88%, transparent);
		min-height: 0;
		overflow: auto;
	}
	.hier-h {
		display: flex;
		justify-content: space-between;
		padding: 0.15rem 0.5rem 0.55rem;
		font-size: 0.72rem;
		text-transform: uppercase;
		letter-spacing: 0.04em;
		opacity: 0.65;
	}
	.idea { margin-bottom: 0.25rem; }
	.idea-btn, .g-btn {
		width: 100%;
		display: flex;
		align-items: baseline;
		gap: 0.4rem;
		background: none;
		border: 0;
		color: inherit;
		text-align: left;
		padding: 0.35rem 0.5rem;
		border-radius: 6px;
		cursor: pointer;
	}
	.idea-btn.on, .g-btn.on {
		background: color-mix(in oklab, var(--dk-brand, #737373) 22%, transparent);
	}
	.idea-title { font-weight: 600; font-size: 0.86rem; }
	.idea-btn.folder .idea-title { font-weight: 650; }
	.pr-title { margin: 0 0 0.45rem; opacity: 0.75; font-size: 0.92rem; }
	.linkish {
		background: none;
		border: 0;
		color: inherit;
		padding: 0;
		cursor: pointer;
		text-align: left;
		font: inherit;
	}
	.linkish:hover { text-decoration: underline; }
	.g-btn { padding-left: 1.1rem; font-size: 0.82rem; }
	.idea ul { list-style: none; margin: 0; padding: 0; }
	.risk {
		font-size: 0.62rem;
		text-transform: uppercase;
		letter-spacing: 0.04em;
		opacity: 0.7;
	}
	.risk-critical { color: #f87171; }
	.risk-high { color: #fbbf24; }
	.dim { opacity: 0.55; font-size: 0.75rem; }
	.seen { color: #34d399; }
	.walk-loading {
		display: flex;
		align-items: center;
		gap: 0.55rem;
		padding: 0.45rem 0.5rem;
		font-size: 0.82rem;
		opacity: 0.75;
	}
	.walk-loading .dk-spinner { width: 1rem; height: 1rem; }
	.drill { min-width: 0; min-height: 0; overflow: auto; }
	.story { margin-bottom: 1rem; }
	.story-h { margin: 0 0 0.4rem; font-size: 1.25rem; }
	.condensed {
		border: 1px solid var(--dk-border-soft, #27272a);
		border-left-width: 3px;
		border-left-color: var(--dk-accent, #6366f1);
		border-radius: 10px;
		padding: 0.85rem 1rem;
		margin-bottom: 1rem;
		background: color-mix(in oklab, var(--dk-surface-2, #242424) 55%, transparent);
	}
	.condensed .story-h { margin-bottom: 0.5rem; line-height: 1.35; }
	.condensed-meta {
		display: flex;
		flex-wrap: wrap;
		gap: 0.5rem;
		align-items: center;
		margin-bottom: 0.5rem;
	}
	.check-chip, .files-chip {
		font-size: 0.72rem;
		padding: 0.15rem 0.5rem;
		border-radius: 999px;
		letter-spacing: 0.02em;
		font-weight: 600;
	}
	.check-chip--clean { background: color-mix(in oklab, #34d399 20%, transparent); color: #6ee7b7; }
	.check-chip--warn { background: color-mix(in oklab, #f59e0b 20%, transparent); color: #fbbf24; }
	.check-chip--err { background: color-mix(in oklab, #f87171 22%, transparent); color: #fca5a5; }
	.files-chip {
		background: color-mix(in oklab, var(--dk-surface, #1a1a1a) 80%, transparent);
		color: var(--dk-text-muted, #a1a1aa);
		cursor: default;
	}
	.files-list {
		list-style: none;
		margin: 0 0 0.5rem;
		padding: 0;
		display: flex;
		flex-wrap: wrap;
		gap: 0.3rem 0.75rem;
	}
	.files-list li { font-size: 0.8rem; }
	.files-list code { font-size: 0.78rem; opacity: 0.85; }
	.why-list { margin: 0 0 0.5rem; padding-left: 1.15rem; }
	.why-list li { margin: 0.15rem 0; font-size: 0.88rem; }
	.condensed-hint { margin: 0.25rem 0 0; font-size: 0.78rem; opacity: 0.6; }
	.story-why { margin: 0 0 0.65rem; line-height: 1.5; white-space: pre-wrap; }
	.beats { margin: 0; padding-left: 1.15rem; }
	.beats li { margin: 0.2rem 0; }
	.item-card {
		border: 1px solid var(--dk-border-soft, #27272a);
		border-radius: 10px;
		padding: 0.85rem 1rem;
		margin-bottom: 0.85rem;
		border-left-width: 3px;
	}
	.item-card.add { border-left-color: #34d399; }
	.item-card.rem { border-left-color: #f87171; }
	.item-card.chg { border-left-color: #60a5fa; }
	.kw {
		font-size: 0.78rem;
		opacity: 0.8;
	}
	.shape { font-size: 0.88rem; margin: 0.5rem 0; }
	.critical { margin: 0.65rem 0; }
	.critical h4, .rationale h4 {
		margin: 0 0 0.3rem;
		font-size: 0.72rem;
		text-transform: uppercase;
		letter-spacing: 0.04em;
		opacity: 0.65;
	}
	.critical pre {
		margin: 0;
		padding: 0.5rem;
		font-size: 0.75rem;
		white-space: pre-wrap;
		max-height: 12rem;
		overflow: auto;
		background: #0a0a0a;
		border-radius: 6px;
	}
	.walk-nav {
		display: flex;
		flex-wrap: wrap;
		gap: 0.45rem;
		align-items: center;
		margin: 0.75rem 0 0.4rem;
	}
	.drill-h {
		display: flex;
		align-items: flex-start;
		justify-content: space-between;
		gap: 0.75rem;
		margin-bottom: 0.75rem;
	}
	.drill-h h2 { margin: 0.15rem 0 0; font-size: 1.15rem; }
	.path { margin: 0.2rem 0 0; }
	.path code { font-size: 0.78rem; opacity: 0.7; }
	.rationale {
		padding: 0.7rem 0.85rem;
		border-radius: 8px;
		background: color-mix(in oklab, var(--dk-surface-2, #242424) 80%, transparent);
		margin-bottom: 0.85rem;
	}
	.rationale h3, .walk-h {
		margin: 0 0 0.35rem;
		font-size: 0.72rem;
		text-transform: uppercase;
		letter-spacing: 0.04em;
		opacity: 0.65;
	}
	.rationale p { margin: 0; line-height: 1.45; }
	.parts { list-style: none; padding: 0; margin: 0 0 0.75rem; }
	.parts li { padding: 0.25rem 0; font-size: 0.85rem; }
	.peek-split {
		display: grid;
		grid-template-columns: 1fr 1fr;
		gap: 0.5rem;
		margin-bottom: 0.85rem;
	}
	.peek-split pre {
		margin: 0;
		padding: 0.5rem;
		font-size: 0.72rem;
		white-space: pre-wrap;
		max-height: 14rem;
		overflow: auto;
		background: #0a0a0a;
		border-radius: 6px;
	}
	.before { color: #fca5a5; }
	.after { color: #6ee7b7; }
	.kind { font-size: 0.72rem; text-transform: uppercase; letter-spacing: 0.04em; opacity: 0.65; }
	.add { color: #34d399; }
	.rem { color: #f87171; }
	.chg { color: #60a5fa; }
	.actions { padding: 0.85rem 0 0.2rem; display: flex; flex-direction: column; gap: 0.75rem; box-shadow: none; margin-top: 1rem; }
	.note { display: flex; flex-direction: column; gap: 0.3rem; font-size: 0.85rem; }
	.btns { display: flex; gap: 0.5rem; flex-wrap: wrap; }
	.btn-ghost { font-size: 0.8rem; opacity: 0.85; }
	.err-pill {
		font-size: 0.7rem;
		padding: 0.12rem 0.45rem;
		border-radius: 999px;
		background: color-mix(in oklab, #f87171 22%, transparent);
		color: #fca5a5;
		font-weight: 600;
	}
	.bundle-scope {
		margin: 0 0 0.35rem;
		font-size: 0.85rem;
		opacity: 0.8;
	}
	.bundle-banner {
		border: 1px solid var(--dk-border-soft, #27272a);
		border-left-width: 3px;
		border-left-color: #a78bfa;
		border-radius: 10px;
		padding: 0.85rem 1rem;
		margin-bottom: 1rem;
		background: color-mix(in oklab, var(--dk-surface-2, #242424) 45%, transparent);
	}
	.bundle-tag {
		font-size: 0.68rem;
		text-transform: uppercase;
		letter-spacing: 0.05em;
		color: #c4b5fd;
		font-weight: 700;
	}
	.bundle-banner .story-h { margin: 0.3rem 0 0.35rem; }
	.bundle-sum { margin: 0 0 0.65rem; opacity: 0.8; font-size: 0.9rem; }
	.bundle-projects {
		display: flex;
		flex-direction: column;
		gap: 0.4rem;
		margin-bottom: 0.5rem;
	}
	.bundle-proj {
		display: grid;
		grid-template-columns: minmax(90px, auto) 1fr auto;
		gap: 0.6rem;
		align-items: baseline;
		padding: 0.4rem 0.55rem;
		border-radius: 7px;
		border: 1px solid var(--dk-border-soft, #27272a);
		text-decoration: none;
		color: inherit;
		font-size: 0.84rem;
	}
	.bundle-proj:hover { border-color: #a78bfa; }
	.bundle-proj.on { background: color-mix(in oklab, #a78bfa 16%, transparent); }
	.bp-slug { font-weight: 650; }
	.bp-head { opacity: 0.82; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
	.bp-check { font-size: 0.72rem; opacity: 0.7; }
	.bp-check.err { color: #fca5a5; opacity: 1; }
	.two-person {
		border: 1px solid color-mix(in oklab, #f59e0b 40%, transparent);
		border-radius: 9px;
		padding: 0.7rem 0.85rem;
		background: color-mix(in oklab, #f59e0b 12%, transparent);
		display: flex;
		flex-direction: column;
		gap: 0.4rem;
	}
	.two-person-h { margin: 0; font-weight: 700; font-size: 0.9rem; color: #fbbf24; }
	.btn-outline.danger { border-color: #f87171; color: #fca5a5; }
	.btn-outline.danger:hover { background: color-mix(in oklab, #f87171 15%, transparent); }
	@media (max-width: 860px) {
		.ceremony { grid-template-columns: 1fr; }
		.hier { position: static; max-height: 16rem; }
		.peek-split { grid-template-columns: 1fr; }
	}
</style>
