<script lang="ts">
	import { page } from '$app/stores';
	import IdeEmbedView from '$lib/components/IdeEmbedView.svelte';

	const projectId = $derived($page.params.id ?? '');
</script>

<IdeEmbedView project={projectId} />
