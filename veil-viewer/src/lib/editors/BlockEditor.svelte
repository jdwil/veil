<script lang="ts">
  import ExprEditor from './ExprEditor.svelte';
  import ExprPicker from './ExprPicker.svelte';
  import type { Expr } from './expr-types';

  let { exprs, onChange, depth = 0, label = '' }: {
    exprs: Expr[];
    onChange: (exprs: Expr[]) => void;
    depth?: number;
    label?: string;
  } = $props();

  function updateAt(index: number, expr: Expr) {
    const updated = [...exprs];
    updated[index] = expr;
    onChange(updated);
  }

  function removeAt(index: number) {
    onChange(exprs.filter((_, i) => i !== index));
  }

  function addExpr(expr: Expr) {
    onChange([...exprs, expr]);
  }

  function moveUp(index: number) {
    if (index === 0) return;
    const updated = [...exprs];
    [updated[index - 1], updated[index]] = [updated[index], updated[index - 1]];
    onChange(updated);
  }

  function moveDown(index: number) {
    if (index >= exprs.length - 1) return;
    const updated = [...exprs];
    [updated[index], updated[index + 1]] = [updated[index + 1], updated[index]];
    onChange(updated);
  }
</script>

<div class="block-editor">
  {#if label}
    <span class="block-label">{label}</span>
  {/if}
  <div class="block-body">
    {#each exprs as expr, i}
      <div class="block-line">
        <div class="line-actions">
          <button class="action-btn" onclick={() => moveUp(i)} disabled={i === 0}>↑</button>
          <button class="action-btn" onclick={() => moveDown(i)} disabled={i === exprs.length - 1}>↓</button>
          <button class="action-btn del" onclick={() => removeAt(i)}>×</button>
        </div>
        <div class="line-content">
          <ExprEditor {expr} onChange={(e) => updateAt(i, e)} depth={depth} />
        </div>
      </div>
    {/each}
    <div class="block-add">
      <ExprPicker onSelect={addExpr} />
    </div>
  </div>
</div>

<style>
  .block-editor {
    display: flex;
    flex-direction: column;
    gap: 2px;
    padding-left: 8px;
    border-left: 2px solid var(--veil-border);
    margin-left: 4px;
    min-width: 0;
  }

  .block-label {
    font-size: 10px;
    color: var(--veil-text-dim);
    text-transform: uppercase;
    font-weight: 600;
  }

  .block-body {
    display: flex;
    flex-direction: column;
    gap: 3px;
  }

  .block-line {
    display: flex;
    align-items: flex-start;
    gap: 4px;
    padding: 2px 0;
  }

  .line-actions {
    display: flex;
    flex-direction: column;
    gap: 1px;
    opacity: 0.3;
    transition: opacity 0.15s;
  }
  .block-line:hover .line-actions { opacity: 1; }

  .action-btn {
    background: none;
    border: none;
    color: var(--veil-text-dim);
    font-size: 9px;
    cursor: pointer;
    padding: 0 2px;
    line-height: 1;
  }
  .action-btn:hover { color: var(--veil-text); }
  .action-btn.del:hover { color: #ef4444; }
  .action-btn:disabled { opacity: 0.2; cursor: default; }

  .line-content {
    flex: 1;
    min-width: 0;
  }

  .block-add {
    padding-top: 4px;
  }
</style>
