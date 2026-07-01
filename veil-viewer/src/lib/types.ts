// VEIL IR Types — mirrors the Rust IR graph model

export interface IrGraph {
  nodes: IrNode[];
  edges: IrEdge[];
  next_id: number;
}

export interface IrNode {
  id: number;
  kind: NodeKind;
  name: string;
  span: { start: number; end: number };
  metadata: NodeMetadata;
}

export interface NodeMetadata {
  parent: number | null;
  annotations: string[];
  properties: [string, string][];
}

export type NodeKind =
  | 'Solution'
  | 'Context'
  | 'Aggregate'
  | 'Entity'
  | 'ValueObject'
  | 'Event'
  | 'Command'
  | 'Query'
  | 'Port'
  | 'PortMethod'
  | 'Adapter'
  | 'Flow'
  | 'Saga'
  | 'Step'
  | 'ParallelGateway'
  | 'ErrorBoundary'
  | 'Service'
  | 'CallAction'
  | 'EmitAction'
  | 'AssignAction'
  | 'MatchDecision'
  | 'MatchArm'
  | 'DispatchAction'
  | 'InvokeAction'
  | 'RequestAction'
  | 'GuardAction';

export interface IrEdge {
  from: number;
  to: number;
  kind: EdgeKind;
}

export type EdgeKind =
  | 'Contains'
  | 'SequenceFlow'
  | 'Calls'
  | 'Emits'
  | 'Implements'
  | 'References';

// Visual config per node kind
export const NODE_STYLES: Record<NodeKind, { color: string; icon: string; label: string }> = {
  Solution: { color: '#6366f1', icon: '🏗️', label: 'Solution' },
  Context: { color: '#8b5cf6', icon: '📦', label: 'Context' },
  Aggregate: { color: '#ec4899', icon: '🧩', label: 'Aggregate' },
  Entity: { color: '#f43f5e', icon: '🔑', label: 'Entity' },
  ValueObject: { color: '#14b8a6', icon: '💎', label: 'Value Object' },
  Event: { color: '#f59e0b', icon: '⚡', label: 'Event' },
  Command: { color: '#3b82f6', icon: '📨', label: 'Command' },
  Query: { color: '#06b6d4', icon: '🔍', label: 'Query' },
  Port: { color: '#10b981', icon: '🔌', label: 'Port' },
  PortMethod: { color: '#34d399', icon: '⚙️', label: 'Method' },
  Adapter: { color: '#a855f7', icon: '🔗', label: 'Adapter' },
  Flow: { color: '#f97316', icon: '🌊', label: 'Flow' },
  Saga: { color: '#dc2626', icon: '🔄', label: 'Saga' },
  Step: { color: '#64748b', icon: '▶️', label: 'Step' },
  ParallelGateway: { color: '#eab308', icon: '⑃', label: 'Parallel' },
  ErrorBoundary: { color: '#ef4444', icon: '🛡️', label: 'Error Boundary' },
  Service: { color: '#0ea5e9', icon: '🖥️', label: 'Service' },
  CallAction: { color: '#10b981', icon: '📞', label: 'Call' },
  EmitAction: { color: '#f59e0b', icon: '⚡', label: 'Emit' },
  AssignAction: { color: '#6366f1', icon: '←', label: 'Assign' },
  MatchDecision: { color: '#8b5cf6', icon: '◆', label: 'Match' },
  MatchArm: { color: '#64748b', icon: '→', label: 'Arm' },
  DispatchAction: { color: '#f59e0b', icon: '📡', label: 'Dispatch' },
  InvokeAction: { color: '#3b82f6', icon: '⚙️', label: 'Invoke' },
  RequestAction: { color: '#10b981', icon: '🔌', label: 'Request' },
  GuardAction: { color: '#ef4444', icon: '🛡️', label: 'Guard' },
};
