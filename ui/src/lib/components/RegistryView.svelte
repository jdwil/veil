<script lang="ts">

  import PageHeader from './PageHeader.svelte';
  import CollectionView from './CollectionView.svelte';

  let layers: Record<string, unknown>[] = $state([]);
  let stubs: Record<string, unknown>[] = $state([]);
  let loading: boolean = $state(false);
  let error: string = $state('');

   $effect(() => { // load_on_mount
  void (async () => {
        loading = true;
        error = "";
        layers = await (async () => { const __u = new URL("/api/registry/layers", typeof window !== 'undefined' ? window.location.origin : 'http://localhost'); const __p = {} as Record<string, unknown>; for (const [k, v] of Object.entries(__p)) { if (v != null && v !== '') __u.searchParams.set(k, String(v)); } const __r = await fetch(__u.toString()); if (!__r.ok) throw new Error(await __r.text()); return await __r.json(); })();
        stubs = await (async () => { const __u = new URL("/api/registry/stubs", typeof window !== 'undefined' ? window.location.origin : 'http://localhost'); const __p = {} as Record<string, unknown>; for (const [k, v] of Object.entries(__p)) { if (v != null && v !== '') __u.searchParams.set(k, String(v)); } const __r = await fetch(__u.toString()); if (!__r.ok) throw new Error(await __r.text()); return await __r.json(); })();
        loading = false;
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
