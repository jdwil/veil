<script lang="ts">

  import { onMount } from 'svelte';
  import PageHeader from './PageHeader.svelte';
  import CollectionView from './CollectionView.svelte';

  let layers: Record<string, unknown>[] = $state([]);
  let stubs: Record<string, unknown>[] = $state([]);
  let loading: boolean = $state(false);
  let error: string = $state('');

  onMount(() => {
    void (async () => {
      loading = true;
      error = '';
      try {
        const [lr, sr] = await Promise.all([
          fetch('/api/registry/layers', { signal: AbortSignal.timeout(20000) }),
          fetch('/api/registry/stubs', { signal: AbortSignal.timeout(20000) }),
        ]);
        if (!lr.ok) throw new Error((await lr.text()) || `layers HTTP ${lr.status}`);
        if (!sr.ok) throw new Error((await sr.text()) || `stubs HTTP ${sr.status}`);
        const ld = await lr.json();
        const sd = await sr.json();
        layers = Array.isArray(ld) ? ld : ld.layers || ld.items || [];
        stubs = Array.isArray(sd) ? sd : sd.stubs || sd.items || [];
      } catch (e) {
        error = e instanceof Error ? e.message : String(e);
      } finally {
        loading = false;
      }
    })();
  });
</script>

<div class="registry">
  <PageHeader title="Registry" description="Layers and stubs known to this runtime." />
  <CollectionView
    title="Layers"
    items={layers}
    loading={loading}
    error={error}
    view_mode="list"
    default_layout="list"
    show_avatar={false}
    empty_title="No layers registered"
    columns={[
      { key: 'name', label: 'Name', cell: 'identity', showAvatar: false },
      { key: 'version', label: 'Version' },
    ]}
    agent={{ intent: 'list-layers', entity: 'Layer', entityLabel: 'Layer' }}
  />
  <div class="gap"></div>
  <CollectionView
    title="Stubs"
    items={stubs}
    loading={loading}
    view_mode="list"
    default_layout="list"
    show_avatar={false}
    empty_title="No stubs registered"
    columns={[
      { key: 'crate_name', label: 'Crate', cell: 'identity', showAvatar: false },
      { key: 'version', label: 'Version' },
    ]}
    agent={{ intent: 'list-stubs', entity: 'Stub', entityLabel: 'Stub' }}
  />
</div>


<style>
.registry { max-width: 1120px; animation: dk-fade-in var(--dk-dur-slow, 420ms) var(--dk-ease-out, ease) both; }
.gap { height: 1.5rem; }

</style>
