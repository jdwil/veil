<script lang="ts">
  import type { TypeExpr } from './expr-types';

  let { type: ty, onChange }: {
    type: TypeExpr;
    onChange: (t: TypeExpr) => void;
  } = $props();

  const BUILTIN_TYPES = ['Str', 'Int', 'F64', 'Bool', 'UUID', 'DateTime', 'Bytes', 'Json'];

  function update(partial: Partial<TypeExpr>) {
    onChange({ ...ty, ...partial } as TypeExpr);
  }
</script>

<div class="type-editor">
  {#if ty.kind === 'named'}
    <input class="type-input" value={ty.name} list="builtin-types"
      oninput={(e) => onChange({ kind: 'named', name: (e.target as HTMLInputElement).value })} />
    <datalist id="builtin-types">
      {#each BUILTIN_TYPES as t}
        <option value={t} />
      {/each}
    </datalist>

  {:else if ty.kind === 'result'}
    <span class="type-kw">Res!</span>
    {#if ty.inner}
      <span class="type-angle">&lt;</span>
      <svelte:self type={ty.inner} onChange={(t) => onChange({ kind: 'result', inner: t })} />
      <span class="type-angle">&gt;</span>
    {/if}

  {:else if ty.kind === 'optional'}
    <span class="type-kw">Opt</span>
    <span class="type-angle">&lt;</span>
    <svelte:self type={ty.inner} onChange={(t) => onChange({ kind: 'optional', inner: t })} />
    <span class="type-angle">&gt;</span>

  {:else if ty.kind === 'list'}
    <span class="type-kw">List</span>
    <span class="type-angle">&lt;</span>
    <svelte:self type={ty.inner} onChange={(t) => onChange({ kind: 'list', inner: t })} />
    <span class="type-angle">&gt;</span>

  {:else if ty.kind === 'ref'}
    <span class="type-kw">&{ty.mutable ? 'mut ' : ''}</span>
    <svelte:self type={ty.inner} onChange={(t) => onChange({ kind: 'ref', inner: t, mutable: ty.mutable })} />

  {:else if ty.kind === 'tuple'}
    <span class="type-paren">(</span>
    {#each ty.items as item, i}
      {#if i > 0}<span class="type-comma">,</span>{/if}
      <svelte:self type={item} onChange={(t) => {
        const items = [...ty.items]; items[i] = t; onChange({ kind: 'tuple', items });
      }} />
    {/each}
    <span class="type-paren">)</span>

  {:else if ty.kind === 'fn_ptr'}
    <span class="type-kw">fn</span>
    <span class="type-paren">(</span>
    {#each ty.params as param, i}
      {#if i > 0}<span class="type-comma">,</span>{/if}
      <svelte:self type={param} onChange={(t) => {
        const params = [...ty.params]; params[i] = t; onChange({ ...ty, params } as TypeExpr);
      }} />
    {/each}
    <span class="type-paren">)</span>
    {#if ty.ret}
      <span class="type-arrow">→</span>
      <svelte:self type={ty.ret} onChange={(t) => onChange({ ...ty, ret: t } as TypeExpr)} />
    {/if}

  {:else}
    <span class="type-fallback">{JSON.stringify(ty)}</span>
  {/if}
</div>

<style>
  .type-editor { display: inline-flex; align-items: center; gap: 2px; }
  .type-input {
    background: #0f172a; border: 1px solid #334155; border-radius: 3px;
    color: #67e8f9; padding: 2px 6px; font-size: 11px; min-width: 40px; max-width: 100px;
    font-family: 'JetBrains Mono', monospace; outline: none;
  }
  .type-input:focus { border-color: #06b6d4; }
  .type-kw { color: #c084fc; font-size: 11px; font-family: 'JetBrains Mono', monospace; }
  .type-angle, .type-paren, .type-comma, .type-arrow {
    color: #64748b; font-size: 11px; font-family: 'JetBrains Mono', monospace;
  }
  .type-fallback { color: #64748b; font-size: 10px; }
</style>
