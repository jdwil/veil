<script lang="ts">
  import { exprPreview, type Expr, type BinOp } from './expr-types';
  import ExprPicker from './ExprPicker.svelte';
  import BlockEditor from './BlockEditor.svelte';

  let { expr, onChange, depth = 0 }: {
    expr: Expr;
    onChange: (e: Expr) => void;
    depth?: number;
  } = $props();

  let collapsed = $state(depth > 2);

  function update(partial: Partial<Expr>) {
    onChange({ ...expr, ...partial } as Expr);
  }
</script>

<div class="expr-editor" class:nested={depth > 0} style="--depth: {depth}">
  {#if collapsed}
    <button class="collapsed-preview" onclick={() => collapsed = false}>
      <span class="preview-text">{exprPreview(expr)}</span>
      <span class="expand-icon">▶</span>
    </button>
  {:else}
    <div class="expr-content">
      {#if depth > 0}
        <button class="collapse-btn" onclick={() => collapsed = true}>▼</button>
      {/if}

      {#if expr.kind === 'ident'}
        <input class="inline-input" type="text" value={expr.name} placeholder="variable"
          oninput={(e) => update({ name: (e.target as HTMLInputElement).value })} />

      {:else if expr.kind === 'int'}
        <input class="inline-input num" type="number" value={expr.value}
          oninput={(e) => update({ value: parseInt((e.target as HTMLInputElement).value) || 0 })} />

      {:else if expr.kind === 'float'}
        <input class="inline-input num" type="number" step="0.1" value={expr.value}
          oninput={(e) => update({ value: parseFloat((e.target as HTMLInputElement).value) || 0 })} />

      {:else if expr.kind === 'string'}
        <span class="string-quote">"</span>
        <input class="inline-input str" type="text" value={expr.value}
          oninput={(e) => update({ value: (e.target as HTMLInputElement).value })} />
        <span class="string-quote">"</span>

      {:else if expr.kind === 'bool'}
        <select class="inline-select" value={String(expr.value)}
          onchange={(e) => update({ value: (e.target as HTMLSelectElement).value === 'true' })}>
          <option value="true">true</option>
          <option value="false">false</option>
        </select>

      {:else if expr.kind === 'binary_op'}
        <div class="binary-row">
          <svelte:self expr={expr.left} onChange={(e) => update({ left: e })} depth={depth + 1} />
          <select class="op-select" value={expr.op}
            onchange={(e) => update({ op: (e.target as HTMLSelectElement).value as BinOp })}>
            {#each ['+','-','*','/','%','==','!=','<','>','<=','>=','&&','||'] as op}
              <option value={op}>{op}</option>
            {/each}
          </select>
          <svelte:self expr={expr.right} onChange={(e) => update({ right: e })} depth={depth + 1} />
        </div>

      {:else if expr.kind === 'call'}
        <div class="call-row">
          <input class="inline-input" placeholder="target" value={expr.target}
            oninput={(e) => update({ target: (e.target as HTMLInputElement).value })} />
          {#if expr.method}
            <span class="dot">.</span>
            <input class="inline-input" placeholder="method" value={expr.method}
              oninput={(e) => update({ method: (e.target as HTMLInputElement).value })} />
          {/if}
          <span class="paren">(</span>
          {#each expr.args as arg, i}
            {#if i > 0}<span class="comma">,</span>{/if}
            <svelte:self expr={arg} onChange={(e) => {
              const args = [...expr.args]; args[i] = e; update({ args });
            }} depth={depth + 1} />
          {/each}
          <button class="tiny-btn" onclick={() => update({ args: [...expr.args, { kind: 'ident', name: '' }] })}>+</button>
          <span class="paren">)</span>
        </div>

      {:else if expr.kind === 'assign' || expr.kind === 'mut_assign'}
        <div class="assign-row">
          {#if expr.kind === 'mut_assign'}<span class="kw">mut</span>{/if}
          <input class="inline-input" placeholder="name" value={expr.name}
            oninput={(e) => update({ name: (e.target as HTMLInputElement).value })} />
          <span class="eq">=</span>
          <svelte:self expr={expr.value} onChange={(e) => update({ value: e })} depth={depth + 1} />
        </div>

      {:else if expr.kind === 'if'}
        <div class="block-expr">
          <div class="block-header">
            <span class="kw">if</span>
            <svelte:self expr={expr.condition} onChange={(e) => update({ condition: e })} depth={depth + 1} />
          </div>
          <BlockEditor exprs={expr.then_body} onChange={(body) => update({ then_body: body })} depth={depth + 1} label="then" />
          {#if expr.else_body}
            <BlockEditor exprs={expr.else_body} onChange={(body) => update({ else_body: body })} depth={depth + 1} label="else" />
          {:else}
            <button class="tiny-btn" onclick={() => update({ else_body: [] })}>+ else</button>
          {/if}
        </div>

      {:else if expr.kind === 'for'}
        <div class="block-expr">
          <div class="block-header">
            <span class="kw">for</span>
            <input class="inline-input sm" placeholder="item" value={expr.binding}
              oninput={(e) => update({ binding: (e.target as HTMLInputElement).value })} />
            <span class="kw">in</span>
            <svelte:self expr={expr.iterable} onChange={(e) => update({ iterable: e })} depth={depth + 1} />
          </div>
          <BlockEditor exprs={expr.body} onChange={(body) => update({ body })} depth={depth + 1} />
        </div>

      {:else if expr.kind === 'while'}
        <div class="block-expr">
          <div class="block-header">
            <span class="kw">while</span>
            <svelte:self expr={expr.condition} onChange={(e) => update({ condition: e })} depth={depth + 1} />
          </div>
          <BlockEditor exprs={expr.body} onChange={(body) => update({ body })} depth={depth + 1} />
        </div>

      {:else if expr.kind === 'match'}
        <div class="block-expr">
          <div class="block-header">
            <span class="kw">match</span>
            <svelte:self expr={expr.scrutinee} onChange={(e) => update({ scrutinee: e })} depth={depth + 1} />
          </div>
          {#each expr.arms as arm, i}
            <div class="match-arm">
              <input class="inline-input" placeholder="pattern" value={arm.pattern}
                oninput={(e) => { const arms = [...expr.arms]; arms[i] = { ...arm, pattern: (e.target as HTMLInputElement).value }; update({ arms }); }} />
              <span class="arrow">→</span>
              <BlockEditor exprs={arm.body} onChange={(body) => { const arms = [...expr.arms]; arms[i] = { ...arm, body }; update({ arms }); }} depth={depth + 1} />
            </div>
          {/each}
          <button class="tiny-btn" onclick={() => update({ arms: [...expr.arms, { pattern: '_', body: [] }] })}>+ arm</button>
        </div>

      {:else if expr.kind === 'return'}
        <span class="kw">ret</span>
        {#if expr.value}
          <svelte:self expr={expr.value} onChange={(e) => update({ value: e })} depth={depth + 1} />
        {/if}

      {:else if expr.kind === 'field_access'}
        <svelte:self expr={expr.base} onChange={(e) => update({ base: e })} depth={depth + 1} />
        <span class="dot">.</span>
        <input class="inline-input sm" value={expr.field}
          oninput={(e) => update({ field: (e.target as HTMLInputElement).value })} />

      {:else if expr.kind === 'index'}
        <svelte:self expr={expr.base} onChange={(e) => update({ base: e })} depth={depth + 1} />
        <span class="bracket">[</span>
        <svelte:self expr={expr.index} onChange={(e) => update({ index: e })} depth={depth + 1} />
        <span class="bracket">]</span>

      {:else if expr.kind === 'try'}
        <svelte:self expr={expr.expr} onChange={(e) => update({ expr: e })} depth={depth + 1} />
        <span class="postfix">?</span>

      {:else if expr.kind === 'await'}
        <span class="kw">await</span>
        <svelte:self expr={expr.expr} onChange={(e) => update({ expr: e })} depth={depth + 1} />

      {:else if expr.kind === 'cast'}
        <svelte:self expr={expr.expr} onChange={(e) => update({ expr: e })} depth={depth + 1} />
        <span class="kw">as</span>
        <input class="inline-input sm" value={expr.type_name}
          oninput={(e) => update({ type_name: (e.target as HTMLInputElement).value })} />

      {:else if expr.kind === 'closure'}
        <span class="pipe">|</span>
        <input class="inline-input" value={expr.params.join(', ')} placeholder="params"
          oninput={(e) => update({ params: (e.target as HTMLInputElement).value.split(',').map(s => s.trim()).filter(Boolean) })} />
        <span class="pipe">|</span>
        <BlockEditor exprs={expr.body} onChange={(body) => update({ body })} depth={depth + 1} />

      {:else if expr.kind === 'loop'}
        <div class="block-expr">
          <span class="kw">loop</span>
          <BlockEditor exprs={expr.body} onChange={(body) => update({ body })} depth={depth + 1} />
        </div>

      {:else if expr.kind === 'break'}
        <span class="kw">break</span>

      {:else if expr.kind === 'continue'}
        <span class="kw">continue</span>

      {:else if expr.kind === 'array'}
        <span class="bracket">[</span>
        {#each expr.items as item, i}
          {#if i > 0}<span class="comma">,</span>{/if}
          <svelte:self expr={item} onChange={(e) => {
            const items = [...expr.items]; items[i] = e; update({ items });
          }} depth={depth + 1} />
        {/each}
        <button class="tiny-btn" onclick={() => update({ items: [...expr.items, { kind: 'int', value: 0 }] })}>+</button>
        <span class="bracket">]</span>

      {:else}
        <span class="fallback">{exprPreview(expr)}</span>
      {/if}
    </div>
  {/if}
</div>

<style>
  .expr-editor { display: inline-flex; align-items: center; gap: 3px; flex-wrap: wrap; }
  .nested { padding: 2px 4px; border-left: 2px solid rgba(82, 82, 82, 0.3); margin-left: 2px; }
  .expr-content { display: inline-flex; align-items: center; gap: 3px; flex-wrap: wrap; }

  .collapsed-preview {
    background: #1e293b; border: 1px solid #334155; border-radius: 3px;
    padding: 2px 6px; color: var(--veil-text-secondary); font-size: 11px; cursor: pointer;
    font-family: 'JetBrains Mono', monospace; display: inline-flex; gap: 4px;
  }
  .collapsed-preview:hover { background: #334155; }
  .expand-icon { font-size: 8px; color: var(--veil-text-dim); }

  .collapse-btn {
    background: none; border: none; color: var(--veil-text-dim); font-size: 8px;
    cursor: pointer; padding: 0 2px;
  }

  .inline-input {
    background: #0f172a; border: 1px solid #334155; border-radius: 3px;
    color: var(--veil-text); padding: 2px 6px; font-size: 11px; min-width: 50px;
    font-family: 'JetBrains Mono', monospace; outline: none;
  }
  .inline-input:focus { border-color: var(--veil-text-dim); }
  .inline-input.sm { min-width: 35px; max-width: 80px; }
  .inline-input.num { max-width: 60px; }
  .inline-input.str { min-width: 80px; }

  .inline-select {
    background: #0f172a; border: 1px solid #334155; border-radius: 3px;
    color: var(--veil-text); padding: 2px 4px; font-size: 11px;
  }

  .op-select {
    background: #1e293b; border: 1px solid var(--veil-text-faint); border-radius: 3px;
    color: #fbbf24; padding: 2px 4px; font-size: 11px; font-weight: 600;
  }

  .kw { color: #c084fc; font-size: 11px; font-weight: 600; font-family: 'JetBrains Mono', monospace; }
  .dot, .comma, .eq, .arrow, .paren, .bracket, .pipe, .postfix {
    color: var(--veil-text-dim); font-family: 'JetBrains Mono', monospace; font-size: 11px;
  }
  .string-quote { color: #4ade80; font-family: 'JetBrains Mono', monospace; }

  .binary-row, .call-row, .assign-row { display: inline-flex; align-items: center; gap: 3px; flex-wrap: wrap; }

  .block-expr { display: flex; flex-direction: column; gap: 4px; width: 100%; }
  .block-header { display: flex; align-items: center; gap: 4px; flex-wrap: wrap; }

  .match-arm {
    display: flex; align-items: flex-start; gap: 4px; padding-left: 12px;
    border-left: 2px solid #334155;
  }

  .tiny-btn {
    background: #1e40af; color: white; border: none; border-radius: 3px;
    padding: 1px 5px; font-size: 10px; cursor: pointer;
  }
  .tiny-btn:hover { background: #2563eb; }

  .fallback { color: var(--veil-text-dim); font-style: italic; font-size: 11px; }
</style>
