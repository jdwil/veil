<script lang="ts">
  /**
   * Editor for enum variants — supports bare variants, tuple variants, and struct variants.
   * Examples:
   *   Pending                    (bare)
   *   Circle(F64)               (tuple variant)
   *   Point { x: F64, y: F64 } (struct variant)
   *   Pending -> Verified       (state transition)
   */

  interface VariantDef {
    name: string;
    data: string; // raw string: "(F64)" or "{ x: F64 }" or ""
  }

  interface TransitionDef {
    from: string;
    to: string;
  }

  let { variants = [], transitions = [], onChange, onTransitionsChange }: {
    variants: string[];
    transitions?: TransitionDef[];
    onChange: (variants: string[]) => void;
    onTransitionsChange?: (transitions: TransitionDef[]) => void;
  } = $props();

  // Parse variant strings into structured form
  let parsed = $derived<VariantDef[]>(variants.map(v => {
    const parenIdx = v.indexOf('(');
    const braceIdx = v.indexOf('{');
    if (parenIdx >= 0) {
      return { name: v.slice(0, parenIdx).trim(), data: v.slice(parenIdx) };
    }
    if (braceIdx >= 0) {
      return { name: v.slice(0, braceIdx).trim(), data: v.slice(braceIdx) };
    }
    return { name: v.trim(), data: '' };
  }));

  function addVariant() {
    onChange([...variants, '']);
  }

  function removeVariant(index: number) {
    onChange(variants.filter((_, i) => i !== index));
  }

  function updateVariant(index: number, name: string, data: string) {
    const updated = [...variants];
    updated[index] = data ? `${name}${data}` : name;
    onChange(updated);
  }

  function addTransition() {
    if (onTransitionsChange) {
      onTransitionsChange([...(transitions ?? []), { from: '', to: '' }]);
    }
  }

  function removeTransition(index: number) {
    if (onTransitionsChange) {
      onTransitionsChange((transitions ?? []).filter((_, i) => i !== index));
    }
  }

  function updateTransition(index: number, field: 'from' | 'to', value: string) {
    if (onTransitionsChange) {
      const updated = [...(transitions ?? [])];
      updated[index] = { ...updated[index], [field]: value };
      onTransitionsChange(updated);
    }
  }
</script>

<div class="enum-editor">
  <div class="section-header">
    <span class="label-text">Variants</span>
    <button class="add-btn" onclick={addVariant}>+ Variant</button>
  </div>

  {#each parsed as variant, i}
    <div class="variant-row">
      <input
        type="text"
        class="variant-name"
        value={variant.name}
        placeholder="VariantName"
        oninput={(e) => updateVariant(i, (e.target as HTMLInputElement).value, variant.data)}
      />
      <input
        type="text"
        class="variant-data"
        value={variant.data}
        placeholder="(Type) or &#123; field: Type &#125;"
        oninput={(e) => updateVariant(i, variant.name, (e.target as HTMLInputElement).value)}
      />
      <button class="remove-btn" onclick={() => removeVariant(i)}>✕</button>
    </div>
  {/each}

  {#if transitions && transitions.length > 0}
    <div class="transitions-section">
      <div class="section-header">
        <span class="label-text">Transitions</span>
        <button class="add-btn" onclick={addTransition}>+ Transition</button>
      </div>
      {#each transitions as trans, i}
        <div class="transition-row">
          <select class="trans-select" value={trans.from}
            onchange={(e) => updateTransition(i, 'from', (e.target as HTMLSelectElement).value)}>
            <option value="">From...</option>
            {#each parsed as v}
              <option value={v.name}>{v.name}</option>
            {/each}
          </select>
          <span class="trans-arrow">→</span>
          <select class="trans-select" value={trans.to}
            onchange={(e) => updateTransition(i, 'to', (e.target as HTMLSelectElement).value)}>
            <option value="">To...</option>
            {#each parsed as v}
              <option value={v.name}>{v.name}</option>
            {/each}
          </select>
          <button class="remove-btn" onclick={() => removeTransition(i)}>✕</button>
        </div>
      {/each}
    </div>
  {:else if onTransitionsChange}
    <button class="add-btn" onclick={addTransition}>+ Add State Transitions</button>
  {/if}
</div>

<style>
  .enum-editor { display: flex; flex-direction: column; gap: 6px; }
  .section-header { display: flex; align-items: center; justify-content: space-between; }
  .label-text { font-size: 10px; text-transform: uppercase; letter-spacing: 0.5px; color: #64748b; font-weight: 600; }

  .variant-row { display: flex; gap: 4px; align-items: center; }
  .variant-name {
    min-width: 80px; flex: 1;
    background: rgba(0,0,0,0.3); border: 1px solid #2d2d44; border-radius: 4px;
    padding: 5px 8px; font-size: 11px; color: #fbbf24; outline: none;
    font-family: 'JetBrains Mono', monospace; font-weight: 600;
  }
  .variant-name:focus { border-color: #f59e0b; }
  .variant-data {
    min-width: 80px; flex: 1;
    background: rgba(0,0,0,0.2); border: 1px solid #1e293b; border-radius: 4px;
    padding: 5px 6px; font-size: 10px; color: #94a3b8; outline: none;
    font-family: 'JetBrains Mono', monospace;
  }
  .variant-data:focus { border-color: #6366f1; }

  .transitions-section { margin-top: 8px; display: flex; flex-direction: column; gap: 4px; }
  .transition-row { display: flex; gap: 4px; align-items: center; }
  .trans-select {
    flex: 1;
    background: rgba(0,0,0,0.3); border: 1px solid #2d2d44; border-radius: 4px;
    padding: 4px 6px; font-size: 10px; color: #94a3b8; outline: none;
  }
  .trans-arrow { color: #64748b; font-size: 14px; }

  .add-btn {
    font-size: 10px; padding: 3px 8px; border-radius: 4px;
    background: rgba(99,102,241,0.1); border: 1px solid rgba(99,102,241,0.3);
    color: #a5b4fc; cursor: pointer;
  }
  .add-btn:hover { background: rgba(99,102,241,0.2); }
  .remove-btn {
    background: none; border: none; color: #64748b; font-size: 11px;
    cursor: pointer; padding: 2px 4px; border-radius: 3px;
  }
  .remove-btn:hover { color: #f87171; background: rgba(248,113,113,0.1); }
</style>
