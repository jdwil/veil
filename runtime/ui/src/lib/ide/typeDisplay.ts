// Human-readable type display for the UI
// Converts VEIL type syntax to readable form

export function formatType(raw: string): string {
  if (!raw) return '';
  
  // Res! → Result (can fail)
  if (raw === 'Res!') return '→ Result';
  
  // Res!<T> → Result<T>
  if (raw.startsWith('Res!<') && raw.endsWith('>')) {
    const inner = raw.slice(5, -1);
    return `→ Result<${formatType(inner)}>`;
  }
  
  // Opt<T> → Optional<T>
  if (raw.startsWith('Opt<') && raw.endsWith('>')) {
    const inner = raw.slice(4, -1);
    return `${formatType(inner)}?`;
  }
  
  // List<T> → T[]
  if (raw.startsWith('List<') && raw.endsWith('>')) {
    const inner = raw.slice(5, -1);
    return `${formatType(inner)}[]`;
  }
  
  // Common type aliases for display
  const aliases: Record<string, string> = {
    'Str': 'String',
    'Int': 'Integer',
    'F64': 'Float',
    'Bool': 'Boolean',
    'UUID': 'UUID',
    'DateTime': 'DateTime',
    'Bytes': 'Bytes',
  };
  
  return aliases[raw] ?? raw;
}

// Format a method signature for display
export function formatMethodSignature(name: string, params: string, returns: string): string {
  const paramsPart = params || '()';
  const returnsPart = returns ? ` ${formatType(returns)}` : '';
  return `${name}${paramsPart}${returnsPart}`;
}

// Available types for dropdowns
export const TYPE_OPTIONS = [
  'Str', 'Int', 'F64', 'Bool', 'UUID', 'DateTime', 'Bytes',
  'Res!', 'Opt', 'List',
];

// All types including domain types (for broader selection)
export const ALL_TYPES = [
  ...TYPE_OPTIONS,
  'Email', 'Phone', 'Customer', 'Subscription', 'Plan',
  'CustomerStatus', 'SubStatus',
];
