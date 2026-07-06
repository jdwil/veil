<script lang="ts">
  /**
   * AnnotationEditor — edits @annotations on constructs.
   * Annotations with expression arguments (like @invariant(status == Pending))
   * use the ExprEditor for the argument.
   */

  import ExprEditor from './ExprEditor.svelte';
  import type { Expr } from './expr-types';

  interface Annotation {
    name: string;
    args: string[];
  }

  interface AnnotationSpec {
    name: string;
    description: string;
    params: string[]; // param names/types
  }

  let { annotations = [], available = [], onChange }: {
    annotations: Annotation[];
    available: AnnotationSpec[];
    onChange: (annotations: Annotation[]) => void;
  } = $props();

  function toggle(name: string, enabled: boolean) {
    if (enabled) {
      onChange([...annotations, { name, args: [] }]);
    } else {
      onChange(annotations.filter(a => a.name !== name));
    }
    console.log('[VEIL Edit] Annotation toggled:', { name, enabled });
  }

  function updateArgs(index: number, args: string[]) {
    const updated = [...annotations];
    updated[index] = { ...updated[index], args };
    onChange(updated);
    console.log('[VEIL Edit] Annotation args changed:', { name: updated[index].name, args });
  }

  function isActive(name: string): boolean {
    return annotations.some(a => a.name === name);
  }

  function getAnnotation(name: string): Annotation | undefined {
    return annotations.find(a => a.name === name);
  }
</script>

<div class="annotation-editor">
  <span class="label-text">Annotations</span>

  {#each available as spec}
    {@const active = isActive(spec.name)}
    {@const ann = getAnnotation(spec.name)}
    <div class="ann-row" class:active>
      <label class="ann-toggle">
        <input type="checkbox" checked={active}
          onchange={(e) => toggle(spec.name, (e.target as HTMLInputElement).checked)} />
        <span class="ann-name">@{spec.name}</span>
      </label>
      {#if active && spec.params.length > 0 && ann}
        <div class="ann-args">
          {#each spec.params as param, i}
            <input class="ann-arg-input"
              placeholder={param}
              value={ann.args[i] ?? ''}
              oninput={(e) => {
                const args = [...(ann?.args ?? [])];
                args[i] = (e.target as HTMLInputElement).value;
                updateArgs(annotations.indexOf(ann!), args);
              }}
            />
          {/each}
        </div>
      {/if}
      {#if spec.description}
        <span class="ann-desc">{spec.description}</span>
      {/if}
    </div>
  {/each}

  {#if available.length === 0}
    <span class="empty-hint">No annotations available for this construct</span>
  {/if}
</div>

<style>
  .annotation-editor { display: flex; flex-direction: column; gap: 4px; }
  .label-text { font-size: 10px; text-transform: uppercase; color: #64748b; font-weight: 600; }

  .ann-row {
    display: flex; flex-direction: column; gap: 3px;
    padding: 4px 6px; border-radius: 4px;
    border: 1px solid transparent;
  }
  .ann-row.active {
    background: rgba(99, 102, 241, 0.05);
    border-color: rgba(99, 102, 241, 0.2);
  }

  .ann-toggle { display: flex; align-items: center; gap: 6px; cursor: pointer; }
  .ann-toggle input { width: 14px; height: 14px; accent-color: #6366f1; }
  .ann-name {
    font-family: 'JetBrains Mono', monospace; font-size: 11px;
    color: #c084fc; font-weight: 500;
  }

  .ann-args { display: flex; gap: 4px; padding-left: 20px; flex-wrap: wrap; }
  .ann-arg-input {
    background: #0f172a; border: 1px solid #334155; border-radius: 3px;
    padding: 3px 6px; font-size: 11px; color: #e2e8f0; outline: none;
    font-family: 'JetBrains Mono', monospace; min-width: 80px;
  }
  .ann-arg-input:focus { border-color: #6366f1; }

  .ann-desc { font-size: 10px; color: #475569; padding-left: 20px; }
  .empty-hint { font-size: 10px; color: #475569; font-style: italic; }
</style>
