// Annotation schema — defines available annotations per node kind,
// their parameters, and valid values.

import type { NodeKind } from './types';

export interface AnnotationParam {
  name: string;
  type: 'select' | 'text' | 'number';
  options?: string[];  // For select type
  placeholder?: string;
  required?: boolean;
}

export interface AnnotationDef {
  name: string;
  description: string;
  params: AnnotationParam[];
}

// Available annotations per node kind
export const ANNOTATION_SCHEMA: Partial<Record<NodeKind, AnnotationDef[]>> = {
  Flow: [
    {
      name: 'async',
      description: 'Execute as async function',
      params: [
        { name: 'runtime', type: 'select', options: ['tokio', 'async-std'], required: false },
      ],
    },
    {
      name: 'trace',
      description: 'Add distributed tracing',
      params: [
        { name: 'method', type: 'select', options: ['otel', 'xray', 'datadog', 'jaeger'], required: false },
      ],
    },
    {
      name: 'saga',
      description: 'Distributed transaction with compensation',
      params: [],
    },
    {
      name: 'timeout',
      description: 'Flow-level timeout',
      params: [
        { name: 'ms', type: 'number', placeholder: '30000', required: true },
      ],
    },
  ],
  Step: [
    {
      name: 'retry',
      description: 'Retry on failure',
      params: [
        { name: 'attempts', type: 'number', placeholder: '3', required: true },
        { name: 'backoff', type: 'select', options: ['fixed', 'exponential'], required: false },
      ],
    },
    {
      name: 'timeout',
      description: 'Step timeout',
      params: [
        { name: 'ms', type: 'number', placeholder: '5000', required: true },
      ],
    },
    {
      name: 'idempotent',
      description: 'Ensure at-most-once execution',
      params: [],
    },
    {
      name: 'no_compensate',
      description: 'Skip saga compensation (fire-and-forget)',
      params: [],
    },
  ],
  Aggregate: [
    {
      name: 'invariant',
      description: 'Domain constraint validation',
      params: [
        { name: 'expr', type: 'text', placeholder: 'field != nil', required: true },
      ],
    },
  ],
  ValueObject: [
    {
      name: 'invariant',
      description: 'Validation rule',
      params: [
        { name: 'expr', type: 'text', placeholder: 'valid_email(field)', required: true },
      ],
    },
  ],
  Adapter: [
    {
      name: 'env',
      description: 'Required environment variables',
      params: [
        { name: 'vars', type: 'text', placeholder: 'API_KEY, SECRET', required: true },
      ],
    },
  ],
  Port: [
    {
      name: 'async',
      description: 'All methods are async',
      params: [],
    },
  ],
  Command: [
    {
      name: 'idempotent',
      description: 'Command is idempotent',
      params: [],
    },
    {
      name: 'validate',
      description: 'Run validation before execution',
      params: [
        { name: 'expr', type: 'text', placeholder: 'amount > 0', required: true },
      ],
    },
  ],
  Saga: [
    {
      name: 'trace',
      description: 'Add distributed tracing',
      params: [
        { name: 'method', type: 'select', options: ['otel', 'xray', 'datadog', 'jaeger'], required: false },
      ],
    },
    {
      name: 'timeout',
      description: 'Overall saga timeout',
      params: [
        { name: 'ms', type: 'number', placeholder: '60000', required: true },
      ],
    },
  ],
  ErrorBoundary: [
    {
      name: 'retry',
      description: 'Retry failed operations',
      params: [
        { name: 'attempts', type: 'number', placeholder: '3', required: true },
        { name: 'backoff', type: 'select', options: ['fixed', 'exponential'], required: false },
      ],
    },
    {
      name: 'timeout',
      description: 'Timeout for the boundary',
      params: [
        { name: 'ms', type: 'number', placeholder: '30000', required: true },
      ],
    },
    {
      name: 'fallback',
      description: 'Fallback action on failure',
      params: [
        { name: 'action', type: 'text', placeholder: 'emit FailedEvent{}', required: true },
      ],
    },
  ],
};
