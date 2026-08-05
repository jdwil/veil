/**
 * Serialize an Expr tree back to VEIL source text.
 * Used for preview display and eventual persistence.
 */

import type { Expr, TypeExpr } from './expr-types';

export function exprToVeil(expr: Expr, indent = 0): string {
  const pad = '  '.repeat(indent);
  const pad1 = '  '.repeat(indent + 1);

  switch (expr.kind) {
    case 'ident': return expr.name;
    case 'int': return String(expr.value);
    case 'float': return String(expr.value);
    case 'string': return `"${expr.value}"`;
    case 'bool': return String(expr.value);

    case 'field_access':
      return `${exprToVeil(expr.base)}.${expr.field}`;

    case 'call': {
      const args = expr.args.map(a => exprToVeil(a)).join(', ');
      if (expr.sugar) {
        // Desugared statement — emit with original keyword
        if (expr.args.length === 1 && expr.args[0].kind === 'struct_lit') {
          const sl = expr.args[0];
          const fields = sl.fields.map(([k, v]) => {
            const vs = exprToVeil(v);
            return k === vs ? k : `${k}: ${vs}`;
          }).join(', ');
          return `${expr.sugar} ${sl.name}{${fields}}`;
        }
        return `${expr.sugar} ${expr.target}.${expr.method}(${args})`;
      }
      // Canonical form: bare calls (never `call` keyword — SER-003/005).
      if (expr.method) return `${expr.target}.${expr.method}(${args})`;
      return `${expr.target}(${args})`;
    }

    case 'binary_op':
      return `${exprToVeil(expr.left)} ${expr.op} ${exprToVeil(expr.right)}`;

    case 'unary_op':
      return `${expr.op}${exprToVeil(expr.expr)}`;

    case 'assign':
      return `${expr.name} = ${exprToVeil(expr.value)}`;

    case 'mut_assign':
      return `mut ${expr.name} = ${exprToVeil(expr.value)}`;

    case 'if': {
      let s = `if ${exprToVeil(expr.condition)}\n`;
      s += expr.then_body.map(e => `${pad1}${exprToVeil(e, indent + 1)}`).join('\n');
      if (expr.else_body && expr.else_body.length > 0) {
        s += `\n${pad}else\n`;
        s += expr.else_body.map(e => `${pad1}${exprToVeil(e, indent + 1)}`).join('\n');
      }
      return s;
    }

    case 'if_let': {
      let s = `if let ${expr.pattern} = ${exprToVeil(expr.expr)}\n`;
      s += expr.then_body.map(e => `${pad1}${exprToVeil(e, indent + 1)}`).join('\n');
      if (expr.else_body && expr.else_body.length > 0) {
        s += `\n${pad}else\n`;
        s += expr.else_body.map(e => `${pad1}${exprToVeil(e, indent + 1)}`).join('\n');
      }
      return s;
    }

    case 'match': {
      let s = `match ${exprToVeil(expr.scrutinee)}\n`;
      for (const arm of expr.arms) {
        const body = arm.body.map(e => exprToVeil(e, indent + 2)).join('; ');
        s += `${pad1}${arm.pattern} -> ${body}\n`;
      }
      return s.trimEnd();
    }

    case 'for': {
      const idx = expr.index ? `${expr.index}, ` : '';
      let s = `for ${idx}${expr.binding} in ${exprToVeil(expr.iterable)}\n`;
      s += expr.body.map(e => `${pad1}${exprToVeil(e, indent + 1)}`).join('\n');
      return s;
    }

    case 'while': {
      let s = `while ${exprToVeil(expr.condition)}\n`;
      s += expr.body.map(e => `${pad1}${exprToVeil(e, indent + 1)}`).join('\n');
      return s;
    }

    case 'while_let': {
      let s = `while let ${expr.pattern} = ${exprToVeil(expr.expr)}\n`;
      s += expr.body.map(e => `${pad1}${exprToVeil(e, indent + 1)}`).join('\n');
      return s;
    }

    case 'loop': {
      let s = `loop\n`;
      s += expr.body.map(e => `${pad1}${exprToVeil(e, indent + 1)}`).join('\n');
      return s;
    }

    case 'break': return 'break';
    case 'continue': return 'continue';

    case 'return':
      return expr.value ? `ret ${exprToVeil(expr.value)}` : 'ret';

    case 'closure': {
      const params = expr.params.join(', ');
      if (expr.body.length === 1) {
        return `|${params}| ${exprToVeil(expr.body[0])}`;
      }
      let s = `|${params}|\n`;
      s += expr.body.map(e => `${pad1}${exprToVeil(e, indent + 1)}`).join('\n');
      return s;
    }

    case 'tuple':
      return `(${expr.items.map(e => exprToVeil(e)).join(', ')})`;

    case 'array':
      return `[${expr.items.map(e => exprToVeil(e)).join(', ')}]`;

    case 'index':
      return `${exprToVeil(expr.base)}[${exprToVeil(expr.index)}]`;

    case 'range': {
      const s = expr.start ? exprToVeil(expr.start) : '';
      const e = expr.end ? exprToVeil(expr.end) : '';
      return `${s}${expr.inclusive ? '..=' : '..'}${e}`;
    }

    case 'cast':
      return `${exprToVeil(expr.expr)} as ${expr.type_name}`;

    case 'try':
      return `${exprToVeil(expr.expr)}?`;

    case 'await':
      return `await ${exprToVeil(expr.expr)}`;

    case 'struct_lit': {
      const fields = expr.fields.map(([k, v]) => {
        const vs = exprToVeil(v);
        return k === vs ? k : `${k}: ${vs}`;
      }).join(', ');
      return `${expr.name}{${fields}}`;
    }

    case 'struct_update': {
      const fields = expr.fields.map(([k, v]) => {
        const vs = exprToVeil(v);
        return k === vs ? k : `${k}: ${vs}`;
      }).join(', ');
      return `${expr.name}{${fields}, ..${exprToVeil(expr.base)}}`;
    }

    case 'string_interp': {
      const inner = expr.parts.map(p =>
        p.kind === 'literal' ? p.value : `{${exprToVeil(p.value)}}`
      ).join('');
      return `f"${inner}"`;
    }

    case 'action': {
      if (expr.named_args.length > 0) {
        const fields = expr.named_args.map(([k, v]) => {
          const vs = exprToVeil(v);
          return k === vs ? k : `${k}: ${vs}`;
        }).join(', ');
        return `${expr.keyword} ${expr.target}{${fields}}`;
      }
      const args = expr.args.map(a => exprToVeil(a)).join(', ');
      if (expr.method) return `${expr.keyword} ${expr.target}.${expr.method}(${args})`;
      return `${expr.keyword} ${expr.target}(${args})`;
    }
  }
}

export function typeToVeil(ty: TypeExpr): string {
  switch (ty.kind) {
    case 'named': return ty.name;
    case 'generic': return `${ty.name}<${ty.args.map(typeToVeil).join(', ')}>`;
    case 'result': return ty.inner ? `Res!<${typeToVeil(ty.inner)}>` : 'Res!';
    case 'optional': return `Opt<${typeToVeil(ty.inner)}>`;
    case 'list': return `List<${typeToVeil(ty.inner)}>`;
    case 'map': return `Map<${typeToVeil(ty.key)}, ${typeToVeil(ty.value)}>`;
    case 'set': return `Set<${typeToVeil(ty.inner)}>`;
    case 'tuple': return `(${ty.items.map(typeToVeil).join(', ')})`;
    case 'array': return `[${typeToVeil(ty.inner)}; ${ty.size}]`;
    case 'ref': return ty.mutable ? `&mut ${typeToVeil(ty.inner)}` : `&${typeToVeil(ty.inner)}`;
    case 'dyn': return `dyn ${typeToVeil(ty.inner)}`;
    case 'impl_trait': return `impl ${typeToVeil(ty.inner)}`;
    case 'fn_ptr': {
      const params = ty.params.map(typeToVeil).join(', ');
      const ret = ty.ret ? ` -> ${typeToVeil(ty.ret)}` : '';
      return `fn(${params})${ret}`;
    }
  }
}
