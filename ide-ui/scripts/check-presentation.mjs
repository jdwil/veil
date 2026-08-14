/**
 * Lightweight LAY-003 genericity check (no test runner required).
 * Run: node --experimental-strip-types src/lib/presentation.ts
 * or:  node scripts/check-presentation.mjs
 *
 * Duplicates the selfCheckProjection fixture in plain JS so CI can run
 * without vitest — still no DDD keywords.
 */

function constructName(n) {
  return n.metadata.subkind ?? n.kind;
}

function irChildren(graph, parentId) {
  return graph.nodes.filter((n) => n.metadata.parent === parentId);
}

function flattenGroups(graph, nodes) {
  const out = [];
  for (const n of nodes) {
    if (n.kind === 'Group') out.push(...flattenGroups(graph, irChildren(graph, n.id)));
    else out.push(n);
  }
  return out;
}

function ancestorWithName(graph, node, want) {
  let pid = node.metadata.parent;
  const byId = new Map(graph.nodes.map((n) => [n.id, n]));
  while (pid != null) {
    const p = byId.get(pid);
    if (!p) break;
    if (constructName(p) === want) return p;
    pid = p.metadata.parent;
  }
  return null;
}

const graph = {
  nodes: [
    { id: 1, kind: 'Module', name: 'H', metadata: { parent: null, subkind: 'Host' } },
    { id: 2, kind: 'Group', name: 'bucket', metadata: { parent: 1, subkind: 'Group' } },
    { id: 3, kind: 'TypeDef', name: 'RootA', metadata: { parent: 2, subkind: 'RootType' } },
    { id: 4, kind: 'TypeDef', name: 'ChildB', metadata: { parent: 3, subkind: 'ChildType' } },
    { id: 5, kind: 'TypeDef', name: 'OrphanC', metadata: { parent: 2, subkind: 'OtherType' } },
  ],
};

const direct = irChildren(graph, 1);
const candidates = flattenGroups(graph, direct);
const roots = candidates.filter((n) => constructName(n) === 'RootType');
const nested = candidates.filter(
  (n) => constructName(n) === 'ChildType' && ancestorWithName(graph, n, 'RootType')
);
const nestedIds = new Set(nested.map((n) => n.id));
const orphans = candidates.filter(
  (n) => constructName(n) !== 'RootType' && !nestedIds.has(n.id)
);
const display = [...roots, ...orphans].map((n) => n.name).sort();

const ok =
  display.includes('RootA') &&
  !display.includes('ChildB') &&
  display.includes('OrphanC');

if (!ok) {
  console.error('LAY-003 presentation self-check FAILED', display);
  process.exit(1);
}
console.log('LAY-003 presentation self-check OK', display);
