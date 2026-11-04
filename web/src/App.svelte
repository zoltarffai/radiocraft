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
     ├──────────────1──────────────┤
     ├────┬──────┬──────────┬──────┤
     │    │  2   │    4     │  5   │
     │  3 ├──────┼──────────┤      │
     │    │      │    6     │      │
     ├────┴──────┴──────────┴──────┤
     ├──────────────7──────────────┤
  */
  .rjdj {
    display: grid;
    grid-template-columns: 170px minmax(280px, 0.9fr) minmax(420px, 1.35fr) minmax(320px, 1fr);
    grid-template-rows: auto minmax(280px, 1.15fr) minmax(220px, 0.85fr) auto;
    grid-template-areas:
      "s1 s1 s1 s1"
      "s3 s2 s4 s5"
      "s3 s2 s6 s5"
      "s7 s7 s7 s7";
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

  /* stack the narrow layout on smaller screens */
  @media (max-width: 1280px) {
    .rjdj {
      grid-template-columns: 150px minmax(260px, 1fr) minmax(320px, 1.2fr);
      grid-template-rows: auto minmax(260px, 1fr) minmax(220px, 1fr) minmax(200px, 0.8fr) auto;
      grid-template-areas:
        "s1 s1 s1"
        "s3 s2 s4"
        "s3 s5 s5"
        "s3 s6 s6"
        "s7 s7 s7";
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
