<script lang="ts">

  interface Props {
    status: string;
  }
  let { status }: Props = $props();
</script>

<footer class="status-bar">
  <div class="left"><span class="dot"></span><span>{status}</span></div>
  <span class="version">veil-runtime v0.1.0</span>
</footer>


<style>
.status-bar {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 16px;
  padding: 8px 16px;
  border-top: 1px solid var(--dk-border-soft, var(--border));
  background: var(--dk-glass, var(--surface));
  backdrop-filter: blur(12px);
  font-size: 11px;
  color: var(--text-dim);
  flex-shrink: 0;
}
.left { display: flex; align-items: center; gap: 8px; }
.dot {
  width: 8px;
  height: 8px;
  border-radius: 50%;
  background: var(--dk-green, #22c55e);
  box-shadow: 0 0 8px color-mix(in srgb, var(--dk-green, #22c55e) 55%, transparent);
  flex-shrink: 0;
}
.version { color: var(--text-dim); }

</style>
