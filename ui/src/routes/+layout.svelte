    <script lang="ts">
      import '../app.css';
      import type { Snippet } from 'svelte';
      import { onMount } from 'svelte';
      import { goto } from '$app/navigation';
      import { page } from '$app/stores';
      import Sidebar from '$lib/components/Sidebar.svelte';
      import AgentSurface from '$lib/components/AgentSurface.svelte';
      import {
        AgentDock,
        agentPanelOpen,
        agentPanelMinimized,
        agentUnreadCount,
        openAgentPanel,
        onAgentNavigation,
        restoreSession,
        publishRouteFocus,
        installHumanIntentCapture,
        restoreIntentLog,
      } from '$lib/agent';
      import StatusBar from '$lib/components/StatusBar.svelte';
      import {
        toggleSidebarCollapsed,
        initShellTheme,
      } from '$lib/shellLayout';
      let { children }: { children: Snippet } = $props();

      /** Full-bleed main when IDE is embedded (no page padding/scroll chrome). */
      const ideEmbedMode = $derived(/\/projects\/[^/]+\/ide\/?$/.test($page.url.pathname));

      /** Keep SessionFocus.route in sync with SPA navigation. */
      $effect(() => {
        publishRouteFocus($page.url.pathname);
      });

      /** Resolve open_ide / legacy /viewer paths → shell embed route. */
      function ideEmbedPath(project: string): string {
        return `/projects/${encodeURIComponent(project)}/ide`;
      }

      function projectFromViewerPath(path: string): string | null {
        try {
          const u = new URL(path, typeof window !== 'undefined' ? window.location.origin : 'http://localhost');
          const q = u.searchParams.get('project');
          if (q) return q;
        } catch {
          /* ignore */
        }
        const m = path.match(/[?&]project=([^&]+)/);
        return m ? decodeURIComponent(m[1]) : null;
      }

      onMount(() => {
        initShellTheme();
        restoreSession();
        restoreIntentLog();
        publishRouteFocus($page.url.pathname);
        const unsubHuman = installHumanIntentCapture();
        // Open agent panel by default (expanded)
        openAgentPanel();
        // Agent tools (navigate_to / open-ide) → SPA routes (stay in shell)
        // IntentExecutor also calls goto() for Present steps; this handles coarse navigation events.
        const unsubNav = onAgentNavigation((nav) => {
          if (nav.action === 'open-ide') {
            const project = nav.project || projectFromViewerPath(nav.path || '') || '';
            if (project) {
              void goto(ideEmbedPath(project));
              return;
            }
            // Fallback: path already an embed route
            if (nav.path?.includes('/ide')) {
              void goto(nav.path);
              return;
            }
            void goto('/projects');
            return;
          }
          if (nav.action === 'goto' && nav.path) {
            // Legacy absolute viewer links → embed so agent + shell stay put
            if (nav.path.startsWith('/viewer') || nav.path.includes('/viewer/?')) {
              const project = projectFromViewerPath(nav.path) || nav.project || '';
              if (project) {
                void goto(ideEmbedPath(project));
                return;
              }
            }
            void goto(nav.path);
            return;
          }
          if (nav.action === 'switch-project' && nav.project) {
            void goto(`/projects/${encodeURIComponent(nav.project)}`);
          }
        });
        // Cmd+K / Ctrl+K toggles agent panel; Cmd+B collapses main nav
        const handler = (e: KeyboardEvent) => {
          if ((e.metaKey || e.ctrlKey) && e.key === 'k') {
            e.preventDefault();
            // Minimized → expand; open expanded → close; closed → open
            if (!$agentPanelOpen) {
              openAgentPanel();
            } else if ($agentPanelMinimized) {
              agentPanelMinimized.set(false);
              agentUnreadCount.set(0);
            } else {
              agentPanelOpen.set(false);
            }
          }
          if ((e.metaKey || e.ctrlKey) && e.key === 'b') {
            e.preventDefault();
            toggleSidebarCollapsed();
          }
        };
        window.addEventListener('keydown', handler);
        return () => {
          window.removeEventListener('keydown', handler);
          unsubNav();
          unsubHuman();
        };
      });
    </script>

    <div class="shell" class:shell--ide={ideEmbedMode}>
      <Sidebar />
      <div class="content">
        <main class="shell-main" class:shell-main--ide={ideEmbedMode}>
          <AgentSurface />
          {@render children()}
        </main>
        {#if !ideEmbedMode}
          <StatusBar status="connected" />
        {/if}
      </div>
      <AgentDock />
    </div>

    <style>
      .shell {
        display: flex;
        flex-direction: row;
        align-items: stretch;
        height: 100vh;
        width: 100%;
        overflow: hidden;
        background: transparent;
      }
      /* Main column: grows/shrinks as AgentDock width changes */
      .content {
        flex: 1 1 0%;
        min-width: 0;
        max-width: 100%;
        display: flex;
        flex-direction: column;
        overflow: hidden;
      }
      main.shell-main {
        flex: 1 1 auto;
        overflow-y: auto;
        max-width: none;
        margin: 0;
        padding: 1.75rem 1.75rem 2rem;
        animation: none;
        min-height: 0;
        min-width: 0;
      }
      main.shell-main--ide {
        overflow: hidden;
        padding: 0;
        display: flex;
        flex-direction: column;
        flex: 1 1 auto;
        min-height: 0;
        min-width: 0;
        /* flex child IdeApp fills — avoid absolute overlay that ignores sibling dock */
        position: relative;
      }
    </style>
