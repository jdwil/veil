import ELK from 'elkjs/lib/elk.bundled.js';
import type { Node, Edge } from '@xyflow/svelte';

const elk = new ELK();

// Fallback dimensions — used only when DOM measurement is unavailable
const FALLBACK_NODE_WIDTH = 260;
const FALLBACK_NODE_HEIGHT = 120;

// Minimum padding added around measured node sizes to prevent tight packing
const NODE_PADDING = 10;

/** Preferred left-to-right order for domain / infrastructure type columns. */
const TYPE_ORDER = [
  'Aggregate',
  'Entity',
  'ValueObject',
  'Event',
  'Command',
  'Query',
  'DomainService',
  'Repository',
  'Port',
  'Adapter',
  'Handler',
  'Service',
  'Orchestrator',
  'Group',
  'Other',
];

/**
 * Measures actual rendered node dimensions from the DOM.
 * Svelte Flow renders nodes with `data-id` attributes.
 */
export function measureNodesFromDOM(
  nodes: Node[],
  containerEl: HTMLElement | null
): Map<string, { width: number; height: number }> {
  const measurements = new Map<string, { width: number; height: number }>();

  for (const node of nodes) {
    let width = FALLBACK_NODE_WIDTH;
    let height = FALLBACK_NODE_HEIGHT;

    if (containerEl) {
      const nodeEl = containerEl.querySelector(
        `[data-id="${node.id}"]`
      ) as HTMLElement | null;

      if (nodeEl) {
        const rect = nodeEl.getBoundingClientRect();
        width = Math.ceil(rect.width) + NODE_PADDING;
        height = Math.ceil(rect.height) + NODE_PADDING;
      } else {
        width = (node as any).measured?.width ?? node.width ?? FALLBACK_NODE_WIDTH;
        height = (node as any).measured?.height ?? node.height ?? FALLBACK_NODE_HEIGHT;
      }
    }

    measurements.set(node.id, { width, height });
  }

  return measurements;
}

/**
 * Lay out nodes using ELK.js for flow-type views (services, steps, methods).
 */
export async function layoutNodes(
  nodes: Node[],
  edges: Edge[],
  direction: 'TB' | 'LR' = 'TB',
  containerEl: HTMLElement | null = null
): Promise<Node[]> {
  if (nodes.length === 0) return [];

  // No edges → column-by-type is clearer than ELK layered with empty graph
  if (edges.length === 0) {
    return layoutByType(nodes, containerEl);
  }

  const elkDirection = direction === 'LR' ? 'RIGHT' : 'DOWN';
  const measurements = measureNodesFromDOM(nodes, containerEl);

  const elkGraph = {
    id: 'root',
    layoutOptions: {
      'elk.algorithm': 'layered',
      'elk.direction': elkDirection,
      'elk.spacing.nodeNode': '60',
      'elk.layered.spacing.nodeNodeBetweenLayers': '120',
      'elk.layered.crossingMinimization.strategy': 'LAYER_SWEEP',
      'elk.layered.nodePlacement.strategy': 'NETWORK_SIMPLEX',
      'elk.layered.spacing.edgeNodeBetweenLayers': '50',
      'elk.layered.spacing.edgeEdgeBetweenLayers': '25',
      'elk.edgeRouting': 'ORTHOGONAL',
      'elk.layered.considerModelOrder.strategy': 'NODES_AND_EDGES',
    },
    children: nodes.map((node) => {
      const size = measurements.get(node.id)!;
      return {
        id: node.id,
        width: size.width,
        height: size.height,
      };
    }),
    edges: edges.map((edge, i) => ({
      id: edge.id ?? `e${i}`,
      sources: [edge.source],
      targets: [edge.target],
    })),
  };

  try {
    const layout = await elk.layout(elkGraph);

    const positionMap = new Map<string, { x: number; y: number }>();
    for (const child of layout.children ?? []) {
      positionMap.set(child.id, { x: child.x ?? 0, y: child.y ?? 0 });
    }

    return nodes.map((node) => ({
      ...node,
      position: positionMap.get(node.id) ?? node.position,
    }));
  } catch (err) {
    console.error('ELK layout failed, falling back to type columns:', err);
    return layoutByType(nodes, containerEl);
  }
}

/**
 * Column layout by construct type (Aggregate | Entity | …).
 * Deterministic, no ELK — avoids grid-of-death and overlapping type columns.
 */
export async function layoutByType(
  nodes: Node[],
  containerEl: HTMLElement | null = null
): Promise<Node[]> {
  if (nodes.length === 0) return [];

  const measurements = measureNodesFromDOM(nodes, containerEl);

  const groups: Record<string, Node[]> = {};
  for (const node of nodes) {
    const type = String(node.data.subkind ?? node.data.kind ?? 'Other');
    if (!groups[type]) groups[type] = [];
    groups[type].push(node);
  }

  // Stable column order: known DDD/infra types first, then alpha
  const types = Object.keys(groups).sort((a, b) => {
    const ai = TYPE_ORDER.indexOf(a);
    const bi = TYPE_ORDER.indexOf(b);
    const ao = ai === -1 ? 500 : ai;
    const bo = bi === -1 ? 500 : bi;
    if (ao !== bo) return ao - bo;
    return a.localeCompare(b);
  });

  // Sort within type by name for stability
  for (const t of types) {
    groups[t].sort((a, b) =>
      String(a.data.label ?? a.id).localeCompare(String(b.data.label ?? b.id))
    );
  }

  const GAP_X = 56;
  const GAP_Y = 36;
  const positions = new Map<string, { x: number; y: number }>();

  let x = 60;
  for (const type of types) {
    const col = groups[type];
    let y = 60;
    let colW = 0;
    for (const node of col) {
      const size = measurements.get(node.id)!;
      colW = Math.max(colW, size.width);
      positions.set(node.id, { x, y });
      y += size.height + GAP_Y;
    }
    x += colW + GAP_X;
  }

  return nodes.map((node) => ({
    ...node,
    position: positions.get(node.id) ?? node.position,
  }));
}
