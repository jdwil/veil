import ELK from 'elkjs/lib/elk.bundled.js';
import type { Node, Edge } from '@xyflow/svelte';

const elk = new ELK();

const DEFAULT_NODE_WIDTH = 260;
const DEFAULT_NODE_HEIGHT = 80;

/**
 * Lay out nodes using ELK.js for flow-type views (services, steps, methods).
 * Replaces dagre for directed graph layouts.
 */
export async function layoutNodes(
  nodes: Node[],
  edges: Edge[],
  direction: 'TB' | 'LR' = 'TB'
): Promise<Node[]> {
  if (nodes.length === 0) return [];

  const elkDirection = direction === 'LR' ? 'RIGHT' : 'DOWN';

  const elkGraph = {
    id: 'root',
    layoutOptions: {
      'elk.algorithm': 'layered',
      'elk.direction': elkDirection,
      'elk.spacing.nodeNode': '40',
      'elk.layered.spacing.nodeNodeBetweenLayers': '100',
    },
    children: nodes.map((node) => ({
      id: node.id,
      width: DEFAULT_NODE_WIDTH,
      height: DEFAULT_NODE_HEIGHT,
    })),
    edges: edges.map((edge, i) => ({
      id: `e${i}`,
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
 */
export async function layoutByType(nodes: Node[]): Promise<Node[]> {
  if (nodes.length === 0) return [];

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
      partitionedChildren.push({
        id: node.id,
        width: DEFAULT_NODE_WIDTH,
        height: DEFAULT_NODE_HEIGHT,
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
    position: { x: (i % cols) * 300, y: Math.floor(i / cols) * 120 },
  }));
}
