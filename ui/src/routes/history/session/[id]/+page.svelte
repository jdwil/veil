<script lang="ts">
	import { page } from '$app/stores';
	import HistorySessionDetail from '$lib/components/HistorySessionDetail.svelte';

	let id = $derived($page.params.id);
</script>

<HistorySessionDetail sessionId={id} />
