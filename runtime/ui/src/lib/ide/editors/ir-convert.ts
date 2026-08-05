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
  const exprProp = (props['expr'] ?? '').trim();

  switch (subkind) {
    case 'assign':
    case 'mut_assign': {
      // name is "varname = Target.method(args)" or similar
      const src = exprProp || node.name;
      const eqIdx = src.indexOf(' = ');
      if (eqIdx >= 0) {
        const varName = src.slice(0, eqIdx).replace(/^mut\s+/, '');
        const rhs = src.slice(eqIdx + 3);
        return {
          kind: subkind === 'mut_assign' ? 'mut_assign' : 'assign',
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

    case 'return': {
      // Label is the value only; expr prop is full "ret …" when present.
      const raw = (exprProp || node.name)
        .replace(/^\s*ret\s+/i, '')
        .trim();
      return raw
        ? { kind: 'return', value: parseInlineExpr(raw) }
        : { kind: 'return' };
    }

    case 'for': {
      // "for ep in self.api_endpoints" or "for i, ep in …"
      const raw = (exprProp || node.name).trim();
      const m = raw.match(/^for\s+(\w+)(?:\s*,\s*(\w+))?\s+in\s+(.+)$/s);
      if (m) {
        return {
          kind: 'for',
          binding: m[1],
          index: m[2],
          iterable: parseInlineExpr(m[3].trim()),
          body: [], // nested stmts are sibling Actions in flat IR (view order)
        };
      }
      return { kind: 'ident', name: raw };
    }

    case 'if': {
      // Name/expr is condition only (not "if …") to avoid "if if cond" UI doubling
      const cond = (exprProp.replace(/^\s*if\s+/, '') || node.name).trim();
      return {
        kind: 'if',
        condition: parseInlineExpr(cond),
        then_body: [],
        else_body: undefined,
      };
    }

    case 'else':
      return { kind: 'ident', name: 'else' };

    case 'while': {
      const cond = (exprProp.replace(/^\s*while\s+/, '') || node.name).trim();
      return {
        kind: 'while',
        condition: parseInlineExpr(cond),
        body: [],
      };
    }

    case 'loop':
      return { kind: 'loop', body: [] };

    case 'break':
      return { kind: 'break' };

    case 'continue':
      return { kind: 'continue' };

    case 'guard': {
      // Guard maps to 'if' shape — it's a precondition check.
      const guardText = node.name.replace(new RegExp(`^${subkind}\\s+`), '');
      return {
        kind: 'action',
        keyword: subkind,
        target: guardText,
        method: '',
        args: [],
        named_args: [],
      };
    }

    default: {
      // Layer-defined statement (dispatch, invoke, …) — keyword + target.
      // Do not treat control-flow subkinds above as layer actions.
      if (subkind && subkind !== 'assign' && subkind !== 'call' && subkind !== 'expr') {
        const text = (exprProp || node.name).replace(
          new RegExp(`^${subkind}\\s+`),
          ''
        );
        return {
          kind: 'action',
          keyword: subkind,
          target: text.split('{')[0].trim(),
          method: '',
          args: [],
          named_args: parseNamedArgs(text),
        };
      }
      return parseInlineExpr(exprProp || node.name);
    }
  }
}

/**
 * Convert a list of IR child nodes (the body of a step) into Expr[].
 * Only direct Action children — use {@link irGraphBodyToExprs} for Flow/svc
 * hosts whose bodies are nested under Step nodes.
 */
export function irChildrenToExprs(children: IrNode[]): Expr[] {
  const actions = children
    .filter((n) => n.kind === 'Action')
    .sort(sortBySeqThenSpan);
  // Rebuild nesting when Actions carry a `depth` property (method/fn bodies).
  if (actions.some((a) => prop(a, 'depth') != null)) {
    return nestActionsByDepth(actions);
  }
  return actions.map(irNodeToExpr);
}

function actionDepth(n: IrNode): number {
  const d = Number(prop(n, 'depth'));
  return Number.isFinite(d) ? d : 0;
}

/**
 * Rebuild nested for/if/while trees from a flat Action list ordered by `seq`
 * and annotated with `depth` (0 = top-level). Enables indented view-mode source.
 */
function nestActionsByDepth(actions: IrNode[]): Expr[] {
  let i = 0;

  function parseBlock(minDepth: number): Expr[] {
    const out: Expr[] = [];
    while (i < actions.length) {
      const n = actions[i];
      const d = actionDepth(n);
      if (d < minDepth) break;
      if (d > minDepth) {
        // Orphan deeper nodes — attach as flat exprs (should not happen)
        out.push(irNodeToExpr(n));
        i++;
        continue;
      }

      const sk = n.metadata.subkind ?? '';
      if (sk === 'for' || sk === 'while' || sk === 'loop' || sk === 'do') {
        i++;
        const body = parseBlock(minDepth + 1);
        const head = irNodeToExpr(n);
        if (head.kind === 'for') out.push({ ...head, body });
        else if (head.kind === 'while') out.push({ ...head, body });
        else if (head.kind === 'loop') out.push({ ...head, body });
        else out.push(head);
        continue;
      }

      if (sk === 'if' || sk === 'if_let') {
        i++;
        const then_body = parseBlock(minDepth + 1);
        let else_body: Expr[] | undefined;
        if (i < actions.length) {
          const next = actions[i];
          if (actionDepth(next) === minDepth && (next.metadata.subkind ?? '') === 'else') {
            i++; // consume else marker
            else_body = parseBlock(minDepth + 1);
          }
        }
        const head = irNodeToExpr(n);
        if (head.kind === 'if') {
          out.push({ ...head, then_body, else_body });
        } else if (head.kind === 'if_let') {
          out.push({ ...head, then_body, else_body });
        } else {
          out.push(head);
        }
        continue;
      }

      if (sk === 'else') {
        // else without matching if at this depth — stop so outer if can claim it
        break;
      }

      out.push(irNodeToExpr(n));
      i++;
    }
    return out;
  }

  return parseBlock(0);
}

function sortBySpan(a: IrNode, b: IrNode): number {
  return a.span.start - b.span.start || a.id - b.id || a.name.localeCompare(b.name);
}

function prop(node: IrNode, key: string): string | undefined {
  return node.metadata.properties.find(([k]) => k === key)?.[1];
}

/** Prefer explicit `seq` property (method body emission order), then span. */
function sortBySeqThenSpan(a: IrNode, b: IrNode): number {
  const sa = Number(prop(a, 'seq'));
  const sb = Number(prop(b, 'seq'));
  const aHas = Number.isFinite(sa);
  const bHas = Number.isFinite(sb);
  if (aHas && bHas && sa !== sb) return sa - sb;
  if (aHas !== bHas) return aHas ? -1 : 1;
  return sortBySpan(a, b);
}

/**
 * Resolve the executable body of any IR host as Expr[] for BlockEditor.
 *
 * Domain services / flows lower to:
 *   Flow → Step (query/load/…) → Action*
 *          Return
 * Selecting the Flow previously showed an empty body because Actions are
 * grandchildren. This walks Steps, flattens their Actions, and maps Return.
 */
/** Order host body children so Steps run before Returns (Return spans often match the host). */
function bodyChildOrder(n: IrNode): number {
  switch (n.kind) {
    case 'Inputs':
      return 0;
    case 'Step':
      return 1;
    case 'Action':
      return 2;
    case 'MatchDecision':
    case 'ParallelGateway':
    case 'ErrorBoundary':
      return 3;
    case 'Return':
      return 9;
    default:
      return 5;
  }
}

function sortBodyChildren(a: IrNode, b: IrNode): number {
  return bodyChildOrder(a) - bodyChildOrder(b) || sortBySeqThenSpan(a, b);
}

export function irGraphBodyToExprs(
  allNodes: IrNode[],
  hostId: number
): Expr[] {
  const children = allNodes
    .filter((n) => n.metadata.parent === hostId)
    .sort(sortBodyChildren);

  const directActions = children
    .filter((c) => c.kind === 'Action')
    .sort(sortBySeqThenSpan);
  // Plain step/fn body: Actions (and maybe Return) directly under this host.
  // Do not take this path when Steps are present (DomainService structure).
  const hasSteps = children.some((c) => c.kind === 'Step');
  if (directActions.length > 0 && !hasSteps) {
    const exprs = irChildrenToExprs(directActions);
    for (const c of children) {
      if (c.kind === 'Return') {
        const v = prop(c, 'expr');
        exprs.push(
          v
            ? { kind: 'return', value: parseInlineExpr(v) }
            : { kind: 'return' }
        );
      }
    }
    return exprs;
  }

  const exprs: Expr[] = [];
  for (const c of children) {
    if (c.kind === 'Step') {
      const stepKids = allNodes
        .filter((n) => n.metadata.parent === c.id)
        .sort(sortBodyChildren);
      // Prefer Action children; also recurse if a step nested further structure
      const stepActions = stepKids.filter((k) => k.kind === 'Action');
      if (stepActions.length > 0) {
        exprs.push(...irChildrenToExprs(stepActions));
      } else {
        exprs.push(...irGraphBodyToExprs(allNodes, c.id));
      }
    } else if (c.kind === 'Return') {
      const v = prop(c, 'expr');
      exprs.push(
        v ? { kind: 'return', value: parseInlineExpr(v) } : { kind: 'return' }
      );
    } else if (c.kind === 'Action') {
      exprs.push(irNodeToExpr(c));
    } else if (c.kind === 'MatchDecision') {
      // Match structure is not fully rehydrated here; surface as ident for review.
      exprs.push({ kind: 'ident', name: c.name || 'match' });
    }
  }
  return exprs;
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
