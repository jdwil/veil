/**
 * Expression tree types for the visual editor.
 * Mirrors the VEIL AST Expr enum but as TypeScript types for the UI.
 */

export type Expr =
  | { kind: 'ident'; name: string }
  | { kind: 'int'; value: number }
  | { kind: 'float'; value: number }
  | { kind: 'string'; value: string }
  | { kind: 'bool'; value: boolean }
  | { kind: 'field_access'; base: Expr; field: string }
  | { kind: 'call'; target: string; method: string; args: Expr[]; sugar?: string }
  | { kind: 'binary_op'; left: Expr; op: BinOp; right: Expr }
  | { kind: 'unary_op'; op: UnaryOp; expr: Expr }
  | { kind: 'assign'; name: string; value: Expr }
  | { kind: 'mut_assign'; name: string; value: Expr }
  | { kind: 'if'; condition: Expr; then_body: Expr[]; else_body?: Expr[] }
  | { kind: 'if_let'; pattern: string; expr: Expr; then_body: Expr[]; else_body?: Expr[] }
  | { kind: 'match'; scrutinee: Expr; arms: MatchArm[] }
  | { kind: 'for'; binding: string; index?: string; iterable: Expr; body: Expr[] }
  | { kind: 'while'; condition: Expr; body: Expr[] }
  | { kind: 'while_let'; pattern: string; expr: Expr; body: Expr[] }
  | { kind: 'loop'; body: Expr[] }
  | { kind: 'break' }
  | { kind: 'continue' }
  | { kind: 'return'; value?: Expr }
  | { kind: 'closure'; params: string[]; body: Expr[] }
  | { kind: 'tuple'; items: Expr[] }
  | { kind: 'array'; items: Expr[] }
  | { kind: 'index'; base: Expr; index: Expr }
  | { kind: 'range'; start?: Expr; end?: Expr; inclusive: boolean }
  | { kind: 'cast'; expr: Expr; type_name: string }
  | { kind: 'try'; expr: Expr }
  | { kind: 'await'; expr: Expr }
  | { kind: 'struct_lit'; name: string; fields: [string, Expr][] }
  | { kind: 'struct_update'; name: string; fields: [string, Expr][]; base: Expr }
  | { kind: 'string_interp'; parts: StringPart[] }
  | { kind: 'action'; keyword: string; target: string; method: string; args: Expr[]; named_args: [string, Expr][] };

export type MatchArm = {
  pattern: string;
  body: Expr[];
};

export type StringPart =
  | { kind: 'literal'; value: string }
  | { kind: 'expr'; value: Expr };

export type BinOp = '+' | '-' | '*' | '/' | '%' | '==' | '!=' | '<' | '>' | '<=' | '>=' | '&&' | '||';
export type UnaryOp = '!' | '-';

export type TypeExpr =
  | { kind: 'named'; name: string }
  | { kind: 'generic'; name: string; args: TypeExpr[] }
  | { kind: 'result'; inner?: TypeExpr }
  | { kind: 'optional'; inner: TypeExpr }
  | { kind: 'list'; inner: TypeExpr }
  | { kind: 'map'; key: TypeExpr; value: TypeExpr }
  | { kind: 'set'; inner: TypeExpr }
  | { kind: 'tuple'; items: TypeExpr[] }
  | { kind: 'array'; inner: TypeExpr; size: number }
  | { kind: 'ref'; inner: TypeExpr; mutable: boolean }
  | { kind: 'dyn'; inner: TypeExpr }
  | { kind: 'impl_trait'; inner: TypeExpr }
  | { kind: 'fn_ptr'; params: TypeExpr[]; ret?: TypeExpr };

/** All expression kinds for the picker dropdown */
export const EXPR_KINDS: { kind: Expr['kind']; label: string; icon: string; category: string }[] = [
  // Basics
  { kind: 'ident', label: 'Variable', icon: '📝', category: 'Basic' },
  { kind: 'int', label: 'Integer', icon: '#️⃣', category: 'Basic' },
  { kind: 'float', label: 'Float', icon: '🔢', category: 'Basic' },
  { kind: 'string', label: 'String', icon: '📄', category: 'Basic' },
  { kind: 'bool', label: 'Boolean', icon: '✅', category: 'Basic' },
  { kind: 'array', label: 'Array', icon: '📦', category: 'Basic' },
  { kind: 'tuple', label: 'Tuple', icon: '🎯', category: 'Basic' },
  // Operations
  { kind: 'call', label: 'Call', icon: '📞', category: 'Operations' },
  { kind: 'field_access', label: 'Field Access', icon: '🔗', category: 'Operations' },
  { kind: 'binary_op', label: 'Binary Op', icon: '➕', category: 'Operations' },
  { kind: 'unary_op', label: 'Unary Op', icon: '❗', category: 'Operations' },
  { kind: 'index', label: 'Index', icon: '🔍', category: 'Operations' },
  { kind: 'cast', label: 'Cast', icon: '🔄', category: 'Operations' },
  { kind: 'try', label: 'Try (?)', icon: '❓', category: 'Operations' },
  { kind: 'await', label: 'Await', icon: '⏳', category: 'Operations' },
  { kind: 'range', label: 'Range', icon: '↔️', category: 'Operations' },
  // Control Flow
  { kind: 'if', label: 'If / Else', icon: '🔀', category: 'Control' },
  { kind: 'if_let', label: 'If Let', icon: '🔀', category: 'Control' },
  { kind: 'match', label: 'Match', icon: '🎰', category: 'Control' },
  { kind: 'for', label: 'For Loop', icon: '🔁', category: 'Control' },
  { kind: 'while', label: 'While Loop', icon: '🔁', category: 'Control' },
  { kind: 'while_let', label: 'While Let', icon: '🔁', category: 'Control' },
  { kind: 'loop', label: 'Loop', icon: '♾️', category: 'Control' },
  { kind: 'break', label: 'Break', icon: '🛑', category: 'Control' },
  { kind: 'continue', label: 'Continue', icon: '⏭️', category: 'Control' },
  { kind: 'return', label: 'Return', icon: '↩️', category: 'Control' },
  // Assignments
  { kind: 'assign', label: 'Let', icon: '=', category: 'Assign' },
  { kind: 'mut_assign', label: 'Let Mut', icon: '=', category: 'Assign' },
  // Constructors
  { kind: 'struct_lit', label: 'Struct Literal', icon: '🏗️', category: 'Construct' },
  { kind: 'struct_update', label: 'Struct Update', icon: '🏗️', category: 'Construct' },
  { kind: 'closure', label: 'Closure', icon: 'λ', category: 'Construct' },
  { kind: 'string_interp', label: 'Format String', icon: 'f"', category: 'Construct' },
  // Layer
  { kind: 'action', label: 'Statement', icon: '⚡', category: 'Layer' },
];

/** Create a default/empty expression for a given kind */
export function defaultExpr(kind: Expr['kind']): Expr {
  switch (kind) {
    case 'ident': return { kind: 'ident', name: '' };
    case 'int': return { kind: 'int', value: 0 };
    case 'float': return { kind: 'float', value: 0.0 };
    case 'string': return { kind: 'string', value: '' };
    case 'bool': return { kind: 'bool', value: true };
    case 'field_access': return { kind: 'field_access', base: { kind: 'ident', name: '' }, field: '' };
    case 'call': return { kind: 'call', target: '', method: '', args: [] };
    case 'binary_op': return { kind: 'binary_op', left: { kind: 'ident', name: '' }, op: '+', right: { kind: 'ident', name: '' } };
    case 'unary_op': return { kind: 'unary_op', op: '!', expr: { kind: 'ident', name: '' } };
    case 'assign': return { kind: 'assign', name: '', value: { kind: 'ident', name: '' } };
    case 'mut_assign': return { kind: 'mut_assign', name: '', value: { kind: 'ident', name: '' } };
    case 'if': return { kind: 'if', condition: { kind: 'bool', value: true }, then_body: [], else_body: undefined };
    case 'if_let': return { kind: 'if_let', pattern: 'Some(x)', expr: { kind: 'ident', name: '' }, then_body: [], else_body: undefined };
    case 'match': return { kind: 'match', scrutinee: { kind: 'ident', name: '' }, arms: [] };
    case 'for': return { kind: 'for', binding: 'item', iterable: { kind: 'ident', name: '' }, body: [] };
    case 'while': return { kind: 'while', condition: { kind: 'bool', value: true }, body: [] };
    case 'while_let': return { kind: 'while_let', pattern: 'Some(x)', expr: { kind: 'ident', name: '' }, body: [] };
    case 'loop': return { kind: 'loop', body: [] };
    case 'break': return { kind: 'break' };
    case 'continue': return { kind: 'continue' };
    case 'return': return { kind: 'return' };
    case 'closure': return { kind: 'closure', params: [], body: [] };
    case 'tuple': return { kind: 'tuple', items: [] };
    case 'array': return { kind: 'array', items: [] };
    case 'index': return { kind: 'index', base: { kind: 'ident', name: '' }, index: { kind: 'int', value: 0 } };
    case 'range': return { kind: 'range', inclusive: false };
    case 'cast': return { kind: 'cast', expr: { kind: 'ident', name: '' }, type_name: 'Int' };
    case 'try': return { kind: 'try', expr: { kind: 'ident', name: '' } };
    case 'await': return { kind: 'await', expr: { kind: 'ident', name: '' } };
    case 'struct_lit': return { kind: 'struct_lit', name: '', fields: [] };
    case 'struct_update': return { kind: 'struct_update', name: '', fields: [], base: { kind: 'ident', name: '' } };
    case 'string_interp': return { kind: 'string_interp', parts: [{ kind: 'literal', value: '' }] };
    case 'action': return { kind: 'action', keyword: '', target: '', method: '', args: [], named_args: [] };
  }
}

/** Render an expression as a one-line preview string */
export function exprPreview(expr: Expr): string {
  switch (expr.kind) {
    case 'ident': return expr.name || '_';
    case 'int': return String(expr.value);
    case 'float': return String(expr.value);
    case 'string': return `"${expr.value}"`;
    case 'bool': return String(expr.value);
    case 'field_access': return `${exprPreview(expr.base)}.${expr.field}`;
    case 'call': {
      const args = expr.args.map(exprPreview).join(', ');
      return expr.method ? `${expr.target}.${expr.method}(${args})` : `${expr.target}(${args})`;
    }
    case 'binary_op': return `${exprPreview(expr.left)} ${expr.op} ${exprPreview(expr.right)}`;
    case 'unary_op': return `${expr.op}${exprPreview(expr.expr)}`;
    case 'assign': return `${expr.name} = ${exprPreview(expr.value)}`;
    case 'mut_assign': return `mut ${expr.name} = ${exprPreview(expr.value)}`;
    case 'if': return `if ${exprPreview(expr.condition)} { ... }`;
    case 'if_let': return `if let ${expr.pattern} = ${exprPreview(expr.expr)} { ... }`;
    case 'match': return `match ${exprPreview(expr.scrutinee)} { ${expr.arms.length} arms }`;
    case 'for': return `for ${expr.binding} in ${exprPreview(expr.iterable)} { ... }`;
    case 'while': return `while ${exprPreview(expr.condition)} { ... }`;
    case 'while_let': return `while let ${expr.pattern} = ${exprPreview(expr.expr)} { ... }`;
    case 'loop': return `loop { ... }`;
    case 'break': return 'break';
    case 'continue': return 'continue';
    case 'return': return expr.value ? `ret ${exprPreview(expr.value)}` : 'ret';
    case 'closure': return `|${expr.params.join(', ')}| ...`;
    case 'tuple': return `(${expr.items.map(exprPreview).join(', ')})`;
    case 'array': return `[${expr.items.map(exprPreview).join(', ')}]`;
    case 'index': return `${exprPreview(expr.base)}[${exprPreview(expr.index)}]`;
    case 'range': {
      const s = expr.start ? exprPreview(expr.start) : '';
      const e = expr.end ? exprPreview(expr.end) : '';
      return `${s}${expr.inclusive ? '..=' : '..'}${e}`;
    }
    case 'cast': return `${exprPreview(expr.expr)} as ${expr.type_name}`;
    case 'try': return `${exprPreview(expr.expr)}?`;
    case 'await': return `await ${exprPreview(expr.expr)}`;
    case 'struct_lit': return `${expr.name} { ${expr.fields.length} fields }`;
    case 'struct_update': return `${expr.name} { ..${exprPreview(expr.base)} }`;
    case 'string_interp': return `f"..."`;
    case 'action': return `${expr.keyword} ${expr.target}${expr.method ? '.' + expr.method : ''}`;
  }
}
