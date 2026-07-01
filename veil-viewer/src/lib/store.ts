import { writable } from 'svelte/store';
import type { IrGraph, IrNode } from './types';

export const irGraph = writable<IrGraph | null>(null);
export const currentParent = writable<number | null>(null);
export const breadcrumbs = writable<{ id: number | null; name: string }[]>([]);
export const loading = writable(true);
export const error = writable<string | null>(null);

const API_URL = 'http://localhost:3001/api/ir';

export async function fetchIr() {
  loading.set(true);
  error.set(null);
  try {
    const res = await fetch(API_URL);
    if (!res.ok) throw new Error(`HTTP ${res.status}`);
    const data: IrGraph = await res.json();
    irGraph.set(data);

    // Find root and determine entry point
    const root = data.nodes.find(n => n.kind === 'Solution');
    if (root) {
      // Check if this is a composition (root children include a Flow but no user-visible structure)
      const rootChildren = data.nodes.filter(n => n.metadata.parent === root.id);
      const flows = rootChildren.filter(n => n.kind === 'Flow');
      const nonFlows = rootChildren.filter(n => n.kind !== 'Flow');

      // If there's exactly one flow and only package groups, auto-drill into the flow
      if (flows.length === 1 && nonFlows.every(n => n.metadata.annotations.includes('📦 package'))) {
        // Composer mode: enter directly into the flow
        currentParent.set(flows[0].id);
        breadcrumbs.set([{ id: flows[0].id, name: flows[0].name }]);
      } else {
        // Builder mode: start at root
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
