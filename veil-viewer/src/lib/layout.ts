import ELK from 'elkjs/lib/elk.bundled.js';
import type { Node, Edge } from '@xyflow/svelte';

const elk = new ELK();

// Fallback dimensions — used only when DOM measurement is unavailable
const FALLBACK_NODE_WIDTH = 260;
const FALLBACK_NODE_HEIGHT = 120;

// Minimum padding added around measured node sizes to prevent tight packing
const NODE_PADDING = 10;

/**
 * Measures actual rendered node dimensions from the DOM.
 * Svelte Flow renders nodes with `data-id` attributes.
 * Returns a Map of nodeId → { width, height }.
 *
 * This is THE critical piece — without real measurements, ELK will
 * overlap nodes because it doesn't know their actual rendered size.
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
      // Svelte Flow renders each node in a wrapper with data-id
      const nodeEl = containerEl.querySelector(
        `[data-id="${node.id}"]`
      ) as HTMLElement | null;

      if (nodeEl) {
        const rect = nodeEl.getBoundingClientRect();
        // Use ceil to avoid sub-pixel overlap issues
        width = Math.ceil(rect.width) + NODE_PADDING;
        height = Math.ceil(rect.height) + NODE_PADDING;
      } else {
        // Node not yet rendered — use Svelte Flow's measured values if available
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
 * Supports optional DOM measurement for accurate sizing.
 *
 * @param nodes - The nodes to layout
 * @param edges - Edges connecting nodes
 * @param direction - 'TB' (top-bottom) or 'LR' (left-right)
 * @param containerEl - Optional DOM container for node measurement
 */
export async function layoutNodes(
  nodes: Node[],
  edges: Edge[],
  direction: 'TB' | 'LR' = 'TB',
  containerEl: HTMLElement | null = null
): Promise<Node[]> {
  if (nodes.length === 0) return [];

  const elkDirection = direction === 'LR' ? 'RIGHT' : 'DOWN';
  const measurements = measureNodesFromDOM(nodes, containerEl);

  const elkGraph = {
    id: 'root',
    layoutOptions: {
      'elk.algorithm': 'layered',
      'elk.direction': elkDirection,
      'elk.spacing.nodeNode': '50',
      'elk.layered.spacing.nodeNodeBetweenLayers': '100',
      // Crossing minimization for cleaner diagrams
      'elk.layered.crossingMinimization.strategy': 'LAYER_SWEEP',
      // Good node placement for variable-sized nodes
      'elk.layered.nodePlacement.strategy': 'BRANDES_KOEPF',
      // Prevent edge/node overlap
      'elk.layered.spacing.edgeNodeBetweenLayers': '40',
      'elk.layered.spacing.edgeEdgeBetweenLayers': '20',
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
    console.error('ELK layout failed, falling back to grid:', err);
    return fallbackGrid(nodes);
  }
}

/**
 * Lay out nodes using ELK.js for non-flow views (domain groups, bounded contexts).
 * Groups nodes by type and arranges them in columns with proper spacing.
 * Supports optional DOM measurement for accurate sizing.
 *
 * @param nodes - The nodes to layout
 * @param containerEl - Optional DOM container for node measurement
 */
export async function layoutByType(
  nodes: Node[],
  containerEl: HTMLElement | null = null
): Promise<Node[]> {
  if (nodes.length === 0) return [];

  const measurements = measureNodesFromDOM(nodes, containerEl);

  // Group nodes by their display type (subkind or kind) for partitioning
  const groups: Record<string, Node[]> = {};
  for (const node of nodes) {
    const type = String(node.data.subkind ?? node.data.kind ?? 'Other');
    if (!groups[type]) groups[type] = [];
    groups[type].push(node);
  }

  // Assign partition indices so ELK groups same-type nodes together
  let partitionIndex = 0;
  const partitionedChildren: any[] = [];
  for (const [_, groupNodes] of Object.entries(groups)) {
    for (const node of groupNodes) {
      const size = measurements.get(node.id)!;
      partitionedChildren.push({
        id: node.id,
        width: size.width,
        height: size.height,
        layoutOptions: {
          'elk.partitioning.partition': String(partitionIndex),
        },
      });
    }
    partitionIndex++;
  }

  const elkGraph = {
    id: 'root',
    layoutOptions: {
      'elk.algorithm': 'layered',
      'elk.direction': 'RIGHT',
      'elk.spacing.nodeNode': '40',
      'elk.layered.spacing.nodeNodeBetweenLayers': '80',
      'elk.partitioning.activate': 'true',
      // Better placement for variable-sized grouped nodes
      'elk.layered.nodePlacement.strategy': 'BRANDES_KOEPF',
      'elk.layered.crossingMinimization.strategy': 'LAYER_SWEEP',
    },
    children: partitionedChildren,
    edges: [] as any[],
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
    console.error('ELK partitioned layout failed, falling back to grid:', err);
    return fallbackGrid(nodes);
  }
}

/** Simple fallback grid layout if ELK fails */
function fallbackGrid(nodes: Node[]): Node[] {
  const cols = Math.ceil(Math.sqrt(nodes.length));
  return nodes.map((node, i) => ({
    ...node,
    position: { x: (i % cols) * 300, y: Math.floor(i / cols) * 150 },
  }));
}
