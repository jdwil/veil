    <script lang="ts">
      import '../app.css';
      import type { Snippet } from 'svelte';
      import { onMount } from 'svelte';
      import Sidebar from '$lib/components/Sidebar.svelte';
      import AgentSurface from '$lib/components/AgentSurface.svelte';
      import { AgentDock, agentPanelOpen } from '$lib/agent';
      import StatusBar from '$lib/components/StatusBar.svelte';
      let { children }: { children: Snippet } = $props();

      onMount(() => {
        // Open agent panel by default
        agentPanelOpen.set(true);
        // Cmd+K / Ctrl+K toggles agent panel
        const handler = (e: KeyboardEvent) => {
          if ((e.metaKey || e.ctrlKey) && e.key === 'k') {
            e.preventDefault();
            agentPanelOpen.update((v) => !v);
          }
        };
        window.addEventListener('keydown', handler);
        return () => window.removeEventListener('keydown', handler);
      });
    </script>

    <div class="shell">
      <Sidebar />
      <div class="content">
        <main class="shell-main">
          <AgentSurface />
          {@render children()}
        </main>
        <StatusBar status="connected" />
      </div>
      <AgentDock />
    </div>

    <style>
      .shell {
        display: flex;
        height: 100vh;
        width: 100%;
        overflow: hidden;
        background: transparent;
      }
      .content {
        flex: 1;
        min-width: 0;
        display: flex;
        flex-direction: column;
        overflow: hidden;
      }
      main.shell-main {
        flex: 1;
        overflow-y: auto;
        max-width: none;
        margin: 0;
        padding: 1.75rem 1.75rem 2rem;
        animation: none;
      }
    </style>
