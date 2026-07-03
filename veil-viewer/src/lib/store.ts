import { writable } from 'svelte/store';
import type { IrGraph, IrNode } from './types';

export const irGraph = writable<IrGraph | null>(null);
export const veilSource = writable<string>('');
export const currentParent = writable<number | null>(null);
export const breadcrumbs = writable<{ id: number | null; name: string }[]>([]);
export const loading = writable(true);
export const error = writable<string | null>(null);
export const selectedNodeId = writable<string | null>(null);
export const paletteConfig = writable<any[]>([]);

const API_URL = 'http://localhost:3001/api/ir';
const SOURCE_URL = 'http://localhost:3001/api/source';
const PALETTE_URL = 'http://localhost:3001/api/palette';

export async function fetchIr() {
  loading.set(true);
  error.set(null);
  try {
    const [irRes, srcRes, palRes] = await Promise.all([
      fetch(API_URL),
      fetch(SOURCE_URL),
      fetch(PALETTE_URL),
    ]);
    if (!irRes.ok) throw new Error(`HTTP ${irRes.status}`);
    const data: IrGraph = await irRes.json();
    irGraph.set(data);

    if (srcRes.ok) {
      veilSource.set(await srcRes.text());
    }

    if (palRes.ok) {
      paletteConfig.set(await palRes.json());
    }

    // Find root and determine entry point
    const root = data.nodes.find(n => n.kind === 'Solution');
    if (root) {
      const rootChildren = data.nodes.filter(n => n.metadata.parent === root.id);
      const flows = rootChildren.filter(n => n.kind === 'Flow');
      const nonFlows = rootChildren.filter(n => n.kind !== 'Flow');

      if (flows.length === 1 && nonFlows.every(n => n.metadata.annotations.includes('📦 package'))) {
        currentParent.set(flows[0].id);
        breadcrumbs.set([{ id: flows[0].id, name: flows[0].name }]);
      } else {
        currentParent.set(root.id);
        breadcrumbs.set([{ id: root.id, name: root.name }]);
      }
    }
  } catch (e) {
    error.set(e instanceof Error ? e.message : 'Failed to fetch IR');
  } finally {
    loading.set(false);
  }
}

export function drillDown(node: IrNode) {
  currentParent.set(node.id);
  breadcrumbs.update(bc => [...bc, { id: node.id, name: node.name }]);
}

export function navigateTo(id: number | null) {
  currentParent.set(id);
  breadcrumbs.update(bc => {
    const idx = bc.findIndex(b => b.id === id);
    return idx >= 0 ? bc.slice(0, idx + 1) : bc;
  });
}

/** Get children of a given parent node */
export function getChildren(graph: IrGraph, parentId: number | null): IrNode[] {
  if (parentId === null) {
    return graph.nodes.filter(n => n.metadata.parent === null);
  }
  return graph.nodes.filter(n => n.metadata.parent === parentId);
}
