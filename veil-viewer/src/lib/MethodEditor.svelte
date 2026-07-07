<script lang="ts">
  import { ALL_TYPES, formatType } from '$lib/typeDisplay';

  interface MethodParam {
    name: string;
    type: string;
  }

  interface MethodDef {
    name: string;
    params: MethodParam[];
    returnType: string;
  }

  let { methods = [], onChange }: {
    methods: MethodDef[];
    onChange: (methods: MethodDef[]) => void;
  } = $props();

  function addMethod() {
    onChange([...methods, { name: '', params: [], returnType: '' }]);
  }

  function removeMethod(index: number) {
    onChange(methods.filter((_, i) => i !== index));
  }

  function updateMethod(index: number, field: string, value: any) {
    const updated = [...methods];
    (updated[index] as any)[field] = value;
    onChange(updated);
  }

  function addParam(methodIdx: number) {
    const updated = [...methods];
    updated[methodIdx].params = [...updated[methodIdx].params, { name: '', type: 'Str' }];
    onChange(updated);
  }

  function removeParam(methodIdx: number, paramIdx: number) {
    const updated = [...methods];
    updated[methodIdx].params = updated[methodIdx].params.filter((_, i) => i !== paramIdx);
    onChange(updated);
  }

  function updateParam(methodIdx: number, paramIdx: number, field: string, value: string) {
    const updated = [...methods];
    (updated[methodIdx].params[paramIdx] as any)[field] = value;
    onChange(updated);
  }
</script>

<div class="methods-editor">
  <div class="section-header">
    <span class="label-text">Methods</span>
    <button class="add-btn" onclick={addMethod}>+ Add</button>
  </div>

  {#each methods as method, mi}
    <div class="method-card">
      <div class="method-header">
        <input
          type="text"
          class="method-name-input"
          value={method.name}
          placeholder="method_name"
          oninput={(e) => updateMethod(mi, 'name', e.currentTarget.value)}
        />
        <button class="remove-btn" onclick={() => removeMethod(mi)}>✕</button>
      </div>

      <!-- Params -->
      <div class="params-section">
        <div class="params-header">
          <span class="sub-label">Params</span>
          <button class="add-btn-small" onclick={() => addParam(mi)}>+</button>
        </div>
        {#each method.params as param, pi}
          <div class="param-row">
            <input
              type="text"
              class="param-name"
              value={param.name}
              placeholder="name"
              oninput={(e) => updateParam(mi, pi, 'name', e.currentTarget.value)}
            />
            <select
              class="param-type"
              value={param.type}
              onchange={(e) => updateParam(mi, pi, 'type', e.currentTarget.value)}
            >
              {#each ALL_TYPES as t}
                <option value={t}>{formatType(t)}</option>
              {/each}
            </select>
            <button class="remove-btn-small" onclick={() => removeParam(mi, pi)}>✕</button>
          </div>
        {/each}
      </div>

      <!-- Return type -->
      <div class="return-section">
        <span class="sub-label">Returns</span>
        <select
          class="return-type"
          value={method.returnType}
          onchange={(e) => updateMethod(mi, 'returnType', e.currentTarget.value)}
        >
          <option value="">void</option>
          <option value="Res!">Result (can fail)</option>
          {#each ALL_TYPES as t}
            <option value={`Res!<${t}>`}>Result({formatType(t)})</option>
          {/each}
        </select>
      </div>
    </div>
  {/each}

  {#if methods.length === 0}
    <div class="empty-hint">No methods defined</div>
  {/if}
</div>

<style>
  .methods-editor { display: flex; flex-direction: column; gap: 8px; }
  .section-header { display: flex; align-items: center; justify-content: space-between; }
  .label-text { font-size: 10px; text-transform: uppercase; letter-spacing: 0.5px; color: var(--veil-text-dim); font-weight: 600; }

  .method-card {
    background: var(--veil-input-bg);
    border: 1px solid var(--veil-border);
    border-radius: 8px;
    padding: 8px;
    display: flex;
    flex-direction: column;
    gap: 6px;
  }

  .method-header { display: flex; align-items: center; gap: 4px; }
  .method-name-input {
    flex: 1;
    background: var(--veil-input-bg); border: 1px solid var(--veil-border); border-radius: 4px;
    padding: 5px 8px; font-size: 12px; color: var(--veil-text); font-weight: 600;
    outline: none; font-family: monospace;
  }
  .method-name-input:focus { border-color: var(--veil-text-dim); }

  .params-section { display: flex; flex-direction: column; gap: 3px; padding-left: 8px; }
  .params-header { display: flex; align-items: center; gap: 4px; }
  .sub-label { font-size: 9px; color: var(--veil-text-faint); text-transform: uppercase; }

  .param-row { display: flex; gap: 3px; align-items: center; }
  .param-name {
    flex: 1;
    background: var(--veil-input-bg); border: 1px solid var(--veil-border); border-radius: 4px;
    padding: 3px 6px; font-size: 11px; color: var(--veil-text); outline: none; font-family: monospace;
  }
  .param-name:focus { border-color: var(--veil-text-dim); }
  .param-type {
    width: 80px;
    background: var(--veil-input-bg); border: 1px solid var(--veil-border); border-radius: 4px;
    padding: 3px 4px; font-size: 10px; color: var(--veil-text-secondary); outline: none; cursor: pointer;
  }

  .return-section { display: flex; align-items: center; gap: 6px; padding-left: 8px; }
  .return-type {
    flex: 1;
    background: var(--veil-input-bg); border: 1px solid var(--veil-border); border-radius: 4px;
    padding: 3px 6px; font-size: 10px; color: var(--veil-text-secondary); outline: none; cursor: pointer;
  }

  .add-btn {
    font-size: 10px; padding: 3px 8px; border-radius: 4px;
    background: var(--veil-accent-subtle); border: 1px solid var(--veil-border);
    color: var(--veil-text); cursor: pointer;
  }
  .add-btn:hover { background: var(--veil-accent-hover); }
  .add-btn-small {
    font-size: 10px; padding: 1px 6px; border-radius: 3px;
    background: var(--veil-accent-subtle); border: 1px solid var(--veil-accent-hover);
    color: var(--veil-text); cursor: pointer;
  }

  .remove-btn {
    background: none; border: none; color: var(--veil-text-dim); font-size: 12px;
    cursor: pointer; padding: 2px 4px; border-radius: 3px;
  }
  .remove-btn:hover { color: #f87171; background: rgba(248,113,113,0.1); }
  .remove-btn-small {
    background: none; border: none; color: var(--veil-text-faint); font-size: 10px;
    cursor: pointer; padding: 1px 3px;
  }
  .remove-btn-small:hover { color: #f87171; }

  .empty-hint { font-size: 10px; color: var(--veil-text-faint); font-style: italic; padding: 4px 8px; }
</style>
