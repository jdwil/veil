    <script lang="ts">
      import '../app.css';
      import type { Snippet } from 'svelte';
      import Sidebar from '$lib/components/Sidebar.svelte';
  import AgentSurface from '$lib/components/AgentSurface.svelte';
  import StatusBar from '$lib/components/StatusBar.svelte';
      let { children }: { children: Snippet } = $props();
    
    
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