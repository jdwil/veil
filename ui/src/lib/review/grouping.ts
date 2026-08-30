/**
 * Review grouping + presentation — pure functions over diff items.
 *
 * Builds review steps/groups from a StructDiff, parses agent rationales out of
 * PR text, scores risk, and resolves the layer-declared review presentation.
 * No network / store dependencies — see `pr-api.ts` for those.
 */
import type {
  ConstructPeek,
  DiffItem,
  ItemDecision,
  LayerReviewPolicy,
  PathSegment,
  ReviewComment,
  ReviewPresentation,
  ReviewStrategy,
  RiskLevel,
  StructDiff,
  WizardGroup,
  WizardItemState,
} from './types';

export function pathOf(item: DiffItem): string {
  if (item.path && item.name) {
    return item.path.includes(item.name) ? item.path : `${item.path}/${item.name}`;
  }
  return item.path || item.name || item.to_name || '(root)';
}

export function itemDisplayName(item: DiffItem): string {
  return item.name || item.to_name || item.from_name || pathOf(item);
}

export function itemKindLabel(kind: string): string {
  switch (kind) {
    case 'added':
      return 'Added';
    case 'removed':
      return 'Removed';
    case 'renamed':
      return 'Renamed';
    case 'signature_changed':
      return 'Signature';
    case 'body_changed':
      return 'Body';
    case 'annotations_changed':
      return 'Annotations';
    default:
      return kind.replaceAll('_', ' ');
  }
}

export function itemKindClass(kind: string): string {
  if (kind === 'added') return 'add';
  if (kind === 'removed') return 'rem';
  return 'chg';
}

export function containerLabel(item: DiffItem): string {
  const cp = item.container_path;
  if (cp && cp.length > 0) {
    return cp.map((s: PathSegment) => (s.subkind ? `${s.subkind} ${s.name}` : s.name)).join(' → ');
  }
  return item.path || '(root)';
}

/**
 * Extract per-construct rationales from agent PR description / commit messages.
 * Looks for markdown-ish sections: `### Name`, `- **Name**: why`, `path: why`.
 */
export function parseRationales(text: string): Map<string, string> {
  const map = new Map<string, string>();
  if (!text) return map;
  const lines = text.split('\n');
  let current: string | null = null;
  let buf: string[] = [];
  const flush = () => {
    if (current && buf.length) {
      map.set(current.toLowerCase(), buf.join('\n').trim());
    }
    current = null;
    buf = [];
  };
  for (const raw of lines) {
    const line = raw.trim();
    const h = line.match(/^#{1,4}\s+(.+)/);
    if (h) {
      flush();
      current = h[1].replace(/`/g, '').trim();
      continue;
    }
    const bullet = line.match(/^[-*]\s+\*?\*?([A-Za-z0-9_./:-]+)\*?\*?\s*[:—-]\s*(.+)$/);
    if (bullet) {
      flush();
      map.set(bullet[1].toLowerCase(), bullet[2].trim());
      continue;
    }
    if (current) buf.push(raw);
  }
  flush();
  // Whole description as package rationale
  if (map.size === 0 && text.trim().length > 20) {
    map.set('*', text.trim().slice(0, 2000));
  }
  return map;
}

export function matchRationale(item: DiffItem, rationales: Map<string, string>): string | null {
  const candidates = [
    item.name,
    item.to_name,
    item.from_name,
    pathOf(item),
    item.path,
  ].filter(Boolean) as string[];
  for (const c of candidates) {
    const hit = rationales.get(c.toLowerCase());
    if (hit) return hit;
    // partial: name contained in key or key in name
    for (const [k, v] of rationales) {
      if (k === '*') continue;
      if (c.toLowerCase().includes(k) || k.includes(c.toLowerCase())) return v;
    }
  }
  return rationales.get('*') ?? null;
}

/** Merge multiple rationale sources (later maps win on key conflict). */
export function mergeRationaleMaps(...maps: Map<string, string>[]): Map<string, string> {
  const out = new Map<string, string>();
  for (const m of maps) {
    for (const [k, v] of m) {
      if (k && v) out.set(k, v);
    }
  }
  return out;
}

/**
 * Parse rationales from PR description + review comments (agent replies often
 * list `- **Construct**: why` after a rationale pass).
 */
export function rationalesFromPrTexts(
  description: string,
  comments?: ReviewComment[] | null
): Map<string, string> {
  const maps: Map<string, string>[] = [parseRationales(description || '')];
  if (comments?.length) {
    for (const c of comments) {
      const body = c.body || '';
      // Prefer agent replies and freeform bodies that look like rationale lists
      if (
        c.author === 'agent' ||
        body.includes('[pr-wizard:agent_reply]') ||
        body.includes('## Rationales') ||
        /[-*]\s+\*?\*?[A-Za-z]/.test(body)
      ) {
        maps.push(parseRationales(body));
      }
    }
  }
  return mergeRationaleMaps(...maps);
}

/**
 * Re-apply intents onto existing review steps **without** resetting decisions.
 * Prefer annotation/peek intents from a fresh diff; fall back to text maps.
 */
export function mergeRationalesIntoItems(
  items: WizardItemState[],
  rationales: Map<string, string>,
  nextDiff?: StructDiff | null
): WizardItemState[] {
  if (!items.length) return items;

  // Index next-diff intents by construct name for stable match even if order shifts
  const byName = new Map<string, { intent?: string | null; peek?: ConstructPeek | null }>();
  if (nextDiff?.items?.length) {
    for (let i = 0; i < nextDiff.items.length; i++) {
      const di = nextDiff.items[i];
      const name = (di.name || di.to_name || '').toLowerCase();
      if (!name) continue;
      const ann = nextDiff.item_annotations?.[i]?.intent ?? null;
      const peek = nextDiff.item_peeks?.[i] ?? null;
      const intent = ann || peek?.intent || null;
      if (intent || peek) {
        byName.set(name, { intent, peek });
      }
    }
  }

  let changed = false;
  const out = items.map((it, index) => {
    let rationale = it.rationale;
    let peek = it.peek;
    let peekBase = it.peekBase;

    // Same-index annotation (working-tree refresh)
    if (nextDiff) {
      const ann = nextDiff.item_annotations?.[index]?.intent ?? null;
      const p = nextDiff.item_peeks?.[index] ?? null;
      const pb = nextDiff.item_peeks_base?.[index] ?? null;
      const sameName =
        !nextDiff.items?.[index] ||
        (nextDiff.items[index].name || nextDiff.items[index].to_name) ===
          (it.item.name || it.item.to_name);
      if (sameName) {
        if (!rationale && ann) rationale = ann;
        if (!rationale && p?.intent) rationale = p.intent;
        if (p && (!peek || !peek.intent)) peek = { ...p, intent: p.intent || peek?.intent };
        if (pb) peekBase = pb;
      }
    }

    // Name-keyed match across the full next diff
    if (!rationale) {
      const name = (it.item.name || it.item.to_name || '').toLowerCase();
      const hit = name ? byName.get(name) : undefined;
      if (hit?.intent) rationale = hit.intent;
      if (hit?.peek && !peek?.intent) peek = hit.peek;
    }

    if (!rationale) {
      rationale = matchRationale(it.item, rationales);
    }

    if (rationale !== it.rationale || peek !== it.peek || peekBase !== it.peekBase) {
      changed = true;
      return { ...it, rationale, peek, peekBase };
    }
    return it;
  });
  return changed ? out : items;
}

export function riskFromCriticality(c?: string | null): RiskLevel {
  const s = (c || '').toLowerCase();
  if (s === 'critical' || s === '3') return 'critical';
  if (s === 'high' || s === '2') return 'high';
  if (s === 'low' || s === '0') return 'low';
  return 'normal';
}

export function riskFromKind(kind: string): RiskLevel {
  const k = kind.toLowerCase();
  if (k === 'removed' || k.includes('delete')) return 'high';
  if (k === 'signature_changed' || k === 'renamed') return 'high';
  if (k === 'body_changed') return 'normal';
  if (k === 'annotations_changed') return 'low';
  if (k === 'added') return 'normal';
  return 'normal';
}

export function maxRisk(a: RiskLevel, b: RiskLevel): RiskLevel {
  const rank = { critical: 3, high: 2, normal: 1, low: 0 };
  return rank[a] >= rank[b] ? a : b;
}

export function riskLabel(r: RiskLevel): string {
  return r === 'critical' ? 'Critical' : r === 'high' ? 'High' : r === 'low' ? 'Low' : 'Normal';
}

export function buildWizardItems(
  items: DiffItem[],
  rationales: Map<string, string>,
  diff?: StructDiff | null
): WizardItemState[] {
  const allNames = items.map((it) => itemDisplayName(it)).filter(Boolean);
  return items.map((item, index) => {
    const peek = diff?.item_peeks?.[index] ?? null;
    const peekBase = diff?.item_peeks_base?.[index] ?? null;
    const ann = diff?.item_annotations?.[index] ?? null;
    const annIntent = ann?.intent ?? null;
    const fromPeek = peek?.intent ?? null;
    const matched = matchRationale(item, rationales);
    const rationale = annIntent || fromPeek || matched;
    const criticality = ann?.criticality
      ? riskFromCriticality(String(ann.criticality))
      : riskFromKind(item.kind);
    const impactFromApi = diff?.item_impact?.[index] ?? null;
    const impact =
      impactFromApi && impactFromApi.length
        ? impactFromApi
        : inferImpact(item, allNames, index);
    return {
      index,
      item,
      decision: null,
      feedback: '',
      teachingNote: '',
      rationale: rationale ?? null,
      peek,
      peekBase,
      sentToAgent: false,
      criticality,
      annotation: ann,
      impact,
    };
  });
}

/** Group DiffItems by construct identity (path + name); sort groups by max risk. */
export function buildWizardGroups(items: WizardItemState[]): WizardGroup[] {
  const map = new Map<string, WizardItemState[]>();
  for (const it of items) {
    const name = itemDisplayName(it.item) || `item-${it.index}`;
    const path = pathOf(it.item) || '';
    const key = `${path}::${name}`;
    const list = map.get(key) || [];
    list.push(it);
    map.set(key, list);
  }
  const groups: WizardGroup[] = [];
  for (const [key, children] of map) {
    let risk: RiskLevel = 'low';
    let rationale: string | null = null;
    const impact = new Set<string>();
    for (const c of children) {
      risk = maxRisk(risk, c.criticality);
      if (!rationale && c.rationale) rationale = c.rationale;
      for (const i of c.impact) impact.add(i);
    }
    const first = children[0];
    groups.push({
      key,
      name: itemDisplayName(first.item) || key,
      path: pathOf(first.item),
      risk,
      children,
      expanded: children.length > 1,
      decision: aggregateDecision(children),
      rationale,
      impact: [...impact],
    });
  }
  const rank = { critical: 0, high: 1, normal: 2, low: 3 };
  groups.sort((a, b) => rank[a.risk] - rank[b.risk] || a.name.localeCompare(b.name));
  return groups;
}

function aggregateDecision(children: WizardItemState[]): ItemDecision {
  if (!children.length) return null;
  if (children.every((c) => c.decision === 'approve')) return 'approve';
  if (children.some((c) => c.decision === 'feedback')) return 'feedback';
  if (children.every((c) => c.decision === 'skip')) return 'skip';
  return null;
}

export function syncGroupDecision(g: WizardGroup): WizardGroup {
  return { ...g, decision: aggregateDecision(g.children) };
}

function inferImpact(item: DiffItem, allNames: string[], selfIndex: number): string[] {
  // Client fallback only — prefer server item_impact (IR graph edges).
  const name = itemDisplayName(item);
  const container = (item.container_path || []).map((s) => s.name).join('/');
  const out: string[] = [];
  if (container) out.push(`in ${container}`);
  for (let i = 0; i < allNames.length; i++) {
    if (i === selfIndex) continue;
    const n = allNames[i];
    if (!n || n === name) continue;
    if (n.includes(name) || (name.length > 3 && name.includes(n))) out.push(n);
  }
  return out.slice(0, 8);
}

function asStrategy(s?: string | null): ReviewStrategy | null {
  const v = (s || '').toLowerCase().trim();
  if (v === 'structural' || v === 'component_sandbox' || v === 'file_diff') return v;
  return null;
}

/** Resolve review presentation from layer `review` policies + construct subkind. */
export function resolveReviewPresentation(opts: {
  layers?: string[];
  subkind?: string | null;
  nodeKind?: string | null;
  policies?: Record<string, LayerReviewPolicy> | null;
}): ReviewPresentation {
  const layers = (opts.layers || []).map((l) => l.toLowerCase());
  const policies = opts.policies || {};
  // Prefer the most specific layer policy (svelte* over base/ui).
  const order = [...layers].sort((a, b) => {
    const score = (n: string) =>
      n.includes('svelte') ? 3 : n === 'ui' || n.includes('react') ? 2 : n === 'base' ? 0 : 1;
    return score(b) - score(a);
  });
  for (const name of order) {
    const pol =
      policies[name] ||
      policies[Object.keys(policies).find((k) => k.toLowerCase() === name) || ''];
    if (!pol?.strategy) continue;
    const strategy = asStrategy(pol.strategy) || 'structural';
    const fallback = asStrategy(pol.fallback) || 'structural';
    const secondary = (pol.secondary || [])
      .map((s) => asStrategy(s))
      .filter((s): s is ReviewStrategy => !!s);
    return {
      strategy,
      target: pol.target || undefined,
      fallback,
      secondary: secondary.length ? secondary : ['file_diff'],
      impact: pol.impact?.length ? pol.impact : ['dependents'],
      fromLayer: name,
    };
  }
  // Heuristic when no policy map (working-tree without used_layers).
  const sk = (opts.subkind || '').toLowerCase();
  const nk = (opts.nodeKind || '').toLowerCase();
  const isUi =
    layers.some((l) => l.includes('svelte') || l.includes('react') || l === 'ui') ||
    sk.includes('page') ||
    sk.includes('component') ||
    nk.includes('view');
  if (isUi && (layers.some((l) => l.includes('svelte')) || sk.includes('component'))) {
    return {
      strategy: 'component_sandbox',
      target: 'svelte5',
      fallback: 'structural',
      secondary: ['file_diff'],
      impact: ['dependents'],
    };
  }
  return {
    strategy: 'structural',
    fallback: 'structural',
    secondary: ['file_diff'],
    impact: ['dependents'],
  };
}
