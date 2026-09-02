<script lang="ts">
	import { page } from '$app/stores';
	import HistoryBundleDetail from '$lib/components/HistoryBundleDetail.svelte';

	let id = $derived($page.params.id);
</script>

<HistoryBundleDetail bundleId={id} />
