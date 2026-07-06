/**
 * Convert IR node data (as served by /api/ir) into editor Expr trees.
 *
 * The IR represents expressions as child Action nodes with metadata properties.
 * This module reconstructs the Expr tree structure from that flat representation.
 */

import type { Expr } from './expr-types';
import type { IrNode } from '../types';

/**
 * Convert an IR Action node into an Expr.
 * Action nodes have: name (display text), metadata.subkind, metadata.properties.
 */
export function irNodeToExpr(node: IrNode): Expr {
  const subkind = node.metadata.subkind ?? '';
  const props = Object.fromEntries(node.metadata.properties);

  switch (subkind) {
    case 'assign': {
      // name is "varname = Target.method(args)" or similar
      const eqIdx = node.name.indexOf(' = ');
      if (eqIdx >= 0) {
        const varName = node.name.slice(0, eqIdx);
        const rhs = node.name.slice(eqIdx + 3);
        return {
          kind: 'assign',
          name: varName,
          value: parseInlineExpr(rhs),
        };
      }
      return { kind: 'ident', name: node.name };
    }

    case 'call': {
      // name is "call Target.method" or "call Target"
      const callText = node.name.replace(/^call\s+/, '');
      const args = props['args'] ?? '';
      return parseCallExpr(callText, args);
    }

    case 'guard': {
      // name is "guard condition"
      const guardText = node.name.replace(/^guard\s+/, '');
      return {
        kind: 'action',
        keyword: 'guard',
        target: guardText,
        method: '',
        args: [],
        named_args: [],
      };
    }

    case 'dispatch':
    case 'invoke':
    case 'request':
    case 'notify':
    case 'emit': {
      const text = node.name.replace(new RegExp(`^${subkind}\\s+`), '');
      return {
        kind: 'action',
        keyword: subkind,
        target: text.split('{')[0].trim(),
        method: '',
        args: [],
        named_args: parseNamedArgs(text),
      };
    }

    default: {
      // Generic: try to parse the name as an expression
      return parseInlineExpr(node.name);
    }
  }
}

/**
 * Convert a list of IR child nodes (the body of a step) into Expr[].
 */
export function irChildrenToExprs(children: IrNode[]): Expr[] {
  return children
    .filter(n => n.kind === 'Action')
    .map(irNodeToExpr);
}

/** Parse a simple inline expression string into an Expr. */
function parseInlineExpr(text: string): Expr {
  text = text.trim();

  // Integer literal
  if (/^\d+$/.test(text)) return { kind: 'int', value: parseInt(text) };

  // Float literal
  if (/^\d+\.\d+$/.test(text)) return { kind: 'float', value: parseFloat(text) };

  // String literal
  if (text.startsWith('"') && text.endsWith('"')) {
    return { kind: 'string', value: text.slice(1, -1) };
  }

  // Boolean
  if (text === 'true') return { kind: 'bool', value: true };
  if (text === 'false') return { kind: 'bool', value: false };

  // Field access: a.b.c
  if (text.includes('.') && !text.includes('(')) {
    const parts = text.split('.');
    let expr: Expr = { kind: 'ident', name: parts[0] };
    for (let i = 1; i < parts.length; i++) {
      expr = { kind: 'field_access', base: expr, field: parts[i] };
    }
    return expr;
  }

  // Call: Target.method(args) or func(args)
  if (text.includes('(')) {
    const parenIdx = text.indexOf('(');
    const before = text.slice(0, parenIdx);
    const argsStr = text.slice(parenIdx + 1, -1);
    return parseCallExpr(before, argsStr);
  }

  // Plain identifier
  return { kind: 'ident', name: text };
}

/** Parse a call expression from target text and args string. */
function parseCallExpr(targetText: string, argsStr: string): Expr {
  const dotIdx = targetText.lastIndexOf('.');
  const target = dotIdx >= 0 ? targetText.slice(0, dotIdx) : targetText;
  const method = dotIdx >= 0 ? targetText.slice(dotIdx + 1) : '';

  const args: Expr[] = argsStr
    ? argsStr.split(',').map(a => parseInlineExpr(a.trim()))
    : [];

  return { kind: 'call', target, method, args };
}

/** Parse named args from `Target{field: val, ...}` text. */
function parseNamedArgs(text: string): [string, Expr][] {
  const braceIdx = text.indexOf('{');
  if (braceIdx < 0) return [];
  const inner = text.slice(braceIdx + 1, text.lastIndexOf('}'));
  if (!inner.trim()) return [];

  return inner.split(',').map(part => {
    const colonIdx = part.indexOf(':');
    if (colonIdx >= 0) {
      const key = part.slice(0, colonIdx).trim();
      const val = part.slice(colonIdx + 1).trim();
      return [key, parseInlineExpr(val)] as [string, Expr];
    }
    const trimmed = part.trim();
    return [trimmed, { kind: 'ident', name: trimmed }] as [string, Expr];
  });
}
