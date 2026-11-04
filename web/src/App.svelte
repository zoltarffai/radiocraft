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
  <!-- 1. current actions (now playing + next/stop + meters) -->
  <div class="s1"><CurrentActions /></div>

  <!-- 2. playlist (upcoming tracks / play queue) -->
  <div class="s2"><QueuePanel /></div>

  <!-- 3. instant actions and toggles -->
  <div class="s3"><InstantActions /></div>

  <!-- 4. tabbed feature window: track search, playlists, carts, rotations, streams -->
  <div class="s4"><FeatureTabs /></div>

  <!-- 5. tabbed play history + selected track details -->
  <div class="s5"><HistoryDetails /></div>

  <!-- 6. tabbed aux / instant players -->
  <div class="s6"><PlayerTabs /></div>

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
     ├─────────────────1─────────────────┤
     ├─────────────┬─────┬───────────────┤
     │      2      │  3  │       4       │
     ├─────────────┴─────┼───────────────┤
     │          5        │       6       │
     ├─────────────────7─────────────────┤
     Left half = 5 (full width of half); 2+3 above it,
     with 3 = 10 % of the 2+3 band, right of 2.
  */
  .rjdj {
    display: grid;
    grid-template-columns: minmax(280px, 45fr) minmax(110px, 5fr) minmax(340px, 50fr);
    grid-template-rows: auto minmax(240px, 1.1fr) minmax(240px, 1fr) auto;
    grid-template-areas:
      "s1 s1 s1"
      "s2 s3 s4"
      "s5 s5 s6"
      "s7 s7 s7";
    gap: 10px;
    padding: 10px;
    min-height: 100vh;
    max-width: 1720px;
    margin: 0 auto;
    align-content: stretch;
  }

  .s1 { grid-area: s1; }
  .s2 { grid-area: s2; min-height: 0; }
  .s3 { grid-area: s3; min-height: 0; }
  .s4 { grid-area: s4; min-height: 0; }
  .s5 { grid-area: s5; min-height: 0; }
  .s6 { grid-area: s6; min-height: 0; }
  .s7 { grid-area: s7; }

  .rjdj :global(.panel) {
    height: 100%;
    min-height: 0;
    overflow: auto;
  }

  /* keep the proportions on smaller screens, just tighter minima */
  @media (max-width: 1100px) {
    .rjdj {
      grid-template-columns: minmax(220px, 44fr) minmax(92px, 6fr) minmax(280px, 50fr);
    }
  }

  @media (max-width: 820px) {
    .rjdj {
      grid-template-columns: 1fr;
      grid-template-rows: none;
      grid-template-areas:
        "s1"
        "s3"
        "s2"
        "s4"
        "s5"
        "s6"
        "s7";
    }
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
