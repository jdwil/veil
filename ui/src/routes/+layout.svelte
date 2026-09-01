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
      import { reviewPrompt, clearReviewPrompt } from '$lib/review/store';
      let { children }: { children: Snippet } = $props();

      /** Dismiss the review prompt and jump to the condensed review for the slug. */
      function openReview(slug: string) {
        clearReviewPrompt();
        void goto(`/review/${encodeURIComponent(slug)}`);
      }

      /** Full-bleed main when IDE is embedded (no page padding/scroll chrome). */
      const ideEmbedMode = $derived(/\/projects\/[^/]+\/ide\/?$/.test($page.url.pathname));
      const reviewMode = $derived($page.url.pathname.startsWith('/review'));

      /** Review needs the column; keep the agent as a strip unless the operator expands it. */
      $effect(() => {
        if (reviewMode) agentPanelMinimized.set(true);
      });

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
        // Open the dock; Review keeps it as a strip so the walk has the column.
        if (reviewMode) {
          agentPanelOpen.set(true);
          agentPanelMinimized.set(true);
        } else {
          openAgentPanel();
        }
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
        {#if $reviewPrompt && !$page.url.pathname.startsWith(`/review/${$reviewPrompt.slug}`)}
          <div class="review-prompt" role="status" aria-live="polite">
            <span class="review-prompt__dot" aria-hidden="true"></span>
            <span class="review-prompt__text">
              {$reviewPrompt.slug} has {$reviewPrompt.count}
              {$reviewPrompt.count === 1 ? 'change' : 'changes'} ready to review.
            </span>
            <button type="button" class="review-prompt__go" onclick={() => openReview($reviewPrompt.slug)}>
              Review &amp; ship
            </button>
            <button
              type="button"
              class="review-prompt__dismiss"
              aria-label="Dismiss"
              onclick={() => clearReviewPrompt()}
            >
              ×
            </button>
          </div>
        {/if}
        <main class="shell-main" class:shell-main--ide={ideEmbedMode} class:shell-main--review={reviewMode}>
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
      .review-prompt {
        display: flex;
        align-items: center;
        gap: 0.65rem;
        padding: 0.55rem 1rem;
        background: color-mix(in oklab, var(--dk-accent, #6366f1) 16%, var(--dk-surface, #1a1a1a));
        border-bottom: 1px solid color-mix(in oklab, var(--dk-accent, #6366f1) 40%, transparent);
        font-size: 0.88rem;
        animation: review-prompt-in 0.28s ease-out both;
      }
      @keyframes review-prompt-in {
        from { opacity: 0; transform: translateY(-6px); }
        to { opacity: 1; transform: translateY(0); }
      }
      .review-prompt__dot {
        width: 0.55rem;
        height: 0.55rem;
        border-radius: 50%;
        background: var(--dk-accent, #6366f1);
        box-shadow: 0 0 0 0 color-mix(in oklab, var(--dk-accent, #6366f1) 60%, transparent);
        animation: review-prompt-pulse 1.8s ease-out infinite;
        flex: 0 0 auto;
      }
      @keyframes review-prompt-pulse {
        0% { box-shadow: 0 0 0 0 color-mix(in oklab, var(--dk-accent, #6366f1) 55%, transparent); }
        70% { box-shadow: 0 0 0 0.5rem transparent; }
        100% { box-shadow: 0 0 0 0 transparent; }
      }
      .review-prompt__text { flex: 1 1 auto; min-width: 0; }
      .review-prompt__go {
        border: 0;
        border-radius: 6px;
        padding: 0.3rem 0.7rem;
        font-size: 0.82rem;
        font-weight: 600;
        cursor: pointer;
        background: var(--dk-accent, #6366f1);
        color: #fff;
      }
      .review-prompt__go:hover { filter: brightness(1.08); }
      .review-prompt__dismiss {
        border: 0;
        background: none;
        color: inherit;
        font-size: 1.1rem;
        line-height: 1;
        cursor: pointer;
        opacity: 0.6;
        padding: 0 0.2rem;
      }
      .review-prompt__dismiss:hover { opacity: 1; }
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
      main.shell-main--review {
        overflow: hidden;
        padding: 1rem 1.25rem 0.85rem;
        display: flex;
        flex-direction: column;
      }
      main.shell-main--review > :global(*) {
        flex: 1 1 auto;
        min-height: 0;
        display: flex;
        flex-direction: column;
      }
    </style>
