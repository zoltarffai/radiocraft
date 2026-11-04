<script lang="ts">
  import { onMount } from "svelte";
  import { session } from "./lib/session.svelte";
  import CurrentActions from "./lib/CurrentActions.svelte";
  import QueuePanel from "./lib/QueuePanel.svelte";
  import InstantActions from "./lib/InstantActions.svelte";
  import FeatureTabs from "./lib/FeatureTabs.svelte";
  import HistoryDetails from "./lib/HistoryDetails.svelte";
  import PlayerTabs from "./lib/PlayerTabs.svelte";
  import FunctionBar from "./lib/FunctionBar.svelte";

  onMount(() => {
    session.start();
    return () => session.closeSocket();
  });

  const lastError = $derived(session.lastError);
</script>

<main class="rjdj">
  <!-- 1. current actions (now playing + transport + timeline) -->
  <div class="s1"><CurrentActions /></div>

  <div class="half left">
    <!-- 2. playlist (upcoming tracks / play queue) -->
    <div class="s2"><QueuePanel /></div>

    <!-- 3. instant actions and toggles -->
    <div class="s3"><InstantActions /></div>

    <!-- 5. tabbed play history + selected track details -->
    <div class="s5"><HistoryDetails /></div>
  </div>

  <div class="half right">
    <!-- 4. tabbed feature window: track search, playlists, carts, rotations, streams -->
    <div class="s4"><FeatureTabs /></div>

    <!-- 6. tabbed aux / instant players -->
    <div class="s6"><PlayerTabs /></div>
  </div>

  <!-- 7. function selection + status -->
  <div class="s7"><FunctionBar /></div>
</main>

{#if lastError}
  <div class="toast" role="alert">
    <span>{lastError}</span>
    <button onclick={() => (session.lastError = "")}>✕</button>
  </div>
{/if}

<style>
  /* RadioDJ-style shell:
     ├───────────────────1───────────────────┤
     ├─────────────────┬─────┬───────────────┤
     │        2        │  3  │      4  (80%) │
     ├─────────────────┴─────┼───────────────┤
     │         5 (50%)       │      6 (20%)  │
     ├───────────────────7───────────────────┤
     Left and right halves have independent row splits:
     left = 50/50 (2+3 over 5, 3 = 10 % of the 2+3 band),
     right = 80/20 (4 over 6).
  */
  .rjdj {
    display: grid;
    grid-template-columns: minmax(300px, 1fr) minmax(300px, 1fr);
    grid-template-rows: auto minmax(0, 1fr) auto;
    grid-template-areas:
      "s1 s1"
      "left right"
      "s7 s7";
    gap: 10px;
    padding: 10px;
    height: 100dvh;
    max-width: 1720px;
    margin: 0 auto;
    align-content: stretch;
  }

  .s1 { grid-area: s1; }
  .s7 { grid-area: s7; }

  .half {
    display: grid;
    gap: 10px;
    min-height: 0;
  }

  .half.left {
    grid-area: left;
    grid-template-columns: minmax(240px, 9fr) minmax(96px, 1fr);
    grid-template-rows: minmax(180px, 1fr) minmax(180px, 1fr);
    grid-template-areas:
      "s2 s3"
      "s5 s5";
  }

  .half.right {
    grid-area: right;
    grid-template-rows: minmax(180px, 8fr) minmax(120px, 2fr);
    grid-template-areas:
      "s4"
      "s6";
  }

  .s2 { grid-area: s2; min-height: 0; }
  .s3 { grid-area: s3; min-height: 0; }
  .s4 { grid-area: s4; min-height: 0; }
  .s5 { grid-area: s5; min-height: 0; }
  .s6 { grid-area: s6; min-height: 0; }

  .rjdj :global(.panel) {
    height: 100%;
    min-height: 0;
    overflow: auto;
  }

  @media (max-width: 820px) {
    /* let every section become a direct grid item of the shell */
    .half {
      display: contents;
    }

    .rjdj {
      grid-template-columns: 1fr;
      grid-template-rows: none;
      height: auto;
      min-height: 100dvh;
      grid-template-areas:
        "s1"
        "s2"
        "s3"
        "s4"
        "s5"
        "s6"
        "s7";
    }

    .s4 { min-height: 380px; }
    .s2, .s5 { min-height: 280px; }
  }

  .toast {
    position: fixed;
    left: 50%;
    bottom: 18px;
    transform: translateX(-50%);
    display: flex;
    align-items: center;
    gap: 10px;
    background: #3a1f22;
    border: 1px solid var(--err);
    color: #f3d0cd;
    border-radius: 10px;
    padding: 8px 12px;
    font-size: 13px;
    z-index: 60;
  }
</style>
