<script lang="ts">
  /**
   * ConstructEditor — unified editor for any construct shape.
   * Shows relevant sub-editors based on the construct's shape:
   * - struct: fields, named blocks, fns
   * - enum: variants, transitions
   * - trait: methods, associated types
   * - impl: target, method impls
   * - fn: inputs, steps/body
   * - mod/group: children (handled by main graph)
   */

  import FieldsEditor from '$lib/FieldsEditor.svelte';
  import MethodEditor from '$lib/MethodEditor.svelte';
  import { BlockEditor } from '$lib/editors';
  import EnumEditor from './EnumEditor.svelte';
  import type { Expr } from './expr-types';

  let { shape, data, onChange }: {
    shape: string; // 'Struct' | 'Enum' | 'Trait' | 'Impl' | 'Fn' | 'Mod' | 'Group'
    data: {
      name: string;
      typeParams?: string[];
      fields?: { name: string; type: string }[];
      methods?: { name: string; params: { name: string; type: string }[]; returnType: string }[];
      variants?: string[];
      transitions?: { from: string; to: string }[];
      target?: string;
      inputs?: { name: string; type: string }[];
      body?: Expr[];
      visibility?: string;
      whereClause?: string[];
    };
    onChange: (data: any) => void;
  } = $props();

  function update(partial: Record<string, any>) {
    onChange({ ...data, ...partial });
    console.log('[VEIL Edit] Construct changed:', { shape, name: data.name, ...partial });
  }
</script>

<div class="construct-editor">
  <!-- Name + Type Params -->
  <div class="ce-row">
    <label class="ce-label">Name</label>
    <input class="ce-input" value={data.name}
      oninput={(e) => update({ name: (e.target as HTMLInputElement).value })} />
    {#if data.typeParams !== undefined}
      <span class="ce-angle">&lt;</span>
      <input class="ce-input sm" value={(data.typeParams ?? []).join(', ')} placeholder="T, U"
        oninput={(e) => update({ typeParams: (e.target as HTMLInputElement).value.split(',').map(s => s.trim()).filter(Boolean) })} />
      <span class="ce-angle">&gt;</span>
    {/if}
  </div>

  <!-- Visibility -->
  {#if data.visibility !== undefined}
    <div class="ce-row">
      <label class="ce-label">Visibility</label>
      <select class="ce-select" value={data.visibility}
        onchange={(e) => update({ visibility: (e.target as HTMLSelectElement).value })}>
        <option value="">private</option>
        <option value="pub">pub</option>
        <option value="pub(crate)">pub(crate)</option>
        <option value="pub(super)">pub(super)</option>
      </select>
    </div>
  {/if}

  <!-- Shape-specific editors -->
  {#if shape === 'Struct' || shape === 'TypeDef'}
    {#if data.fields}
      <FieldsEditor fields={data.fields} onChange={(f) => update({ fields: f })} />
    {/if}

  {:else if shape === 'Enum'}
    <EnumEditor
      variants={data.variants ?? []}
      transitions={data.transitions}
      onChange={(v) => update({ variants: v })}
      onTransitionsChange={(t) => update({ transitions: t })}
    />

  {:else if shape === 'Trait' || shape === 'Interface'}
    {#if data.methods}
      <MethodEditor methods={data.methods} onChange={(m) => update({ methods: m })} />
    {/if}

  {:else if shape === 'Impl' || shape === 'Implementation'}
    <div class="ce-row">
      <label class="ce-label">Implements</label>
      <input class="ce-input" value={data.target ?? ''} placeholder="TraitName"
        oninput={(e) => update({ target: (e.target as HTMLInputElement).value })} />
    </div>

  {:else if shape === 'Fn' || shape === 'Flow'}
    {#if data.inputs}
      <FieldsEditor fields={data.inputs} label="Inputs" onChange={(f) => update({ inputs: f })} />
    {/if}
    {#if data.body}
      <div class="ce-section">
        <span class="ce-section-label">Body</span>
        <BlockEditor exprs={data.body} onChange={(b) => update({ body: b })} depth={0} />
      </div>
    {/if}
  {/if}

  <!-- Where clause -->
  {#if data.whereClause !== undefined && (data.whereClause ?? []).length > 0}
    <div class="ce-row">
      <label class="ce-label">Where</label>
      <input class="ce-input" value={(data.whereClause ?? []).join(', ')} placeholder="T: Send + Sync"
        oninput={(e) => update({ whereClause: (e.target as HTMLInputElement).value.split(',').map(s => s.trim()).filter(Boolean) })} />
    </div>
  {/if}
</div>

<style>
  .construct-editor { display: flex; flex-direction: column; gap: 8px; }
  .ce-row { display: flex; align-items: center; gap: 6px; }
  .ce-label { font-size: 10px; color: #64748b; text-transform: uppercase; min-width: 55px; }
  .ce-input {
    flex: 1; background: #0f172a; border: 1px solid #334155; border-radius: 4px;
    padding: 5px 8px; font-size: 12px; color: #e2e8f0; outline: none;
    font-family: 'JetBrains Mono', monospace;
  }
  .ce-input.sm { max-width: 120px; }
  .ce-input:focus { border-color: #6366f1; }
  .ce-select {
    background: #0f172a; border: 1px solid #334155; border-radius: 4px;
    padding: 5px 8px; font-size: 11px; color: #94a3b8; outline: none;
  }
  .ce-angle { color: #64748b; font-size: 12px; }
  .ce-section { display: flex; flex-direction: column; gap: 4px; margin-top: 4px; }
  .ce-section-label { font-size: 10px; color: #64748b; text-transform: uppercase; font-weight: 600; }
</style>
