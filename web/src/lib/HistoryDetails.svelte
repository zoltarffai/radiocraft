<script lang="ts">
  import HistoryPanel from "./HistoryPanel.svelte";
  import DetailsPanel from "./DetailsPanel.svelte";
  import { session } from "./session.svelte";

  /** Section 5 — tabbed play history + selected-track details. */
  let tab = $state<"history" | "details">("history");

  async function showDetails(songId: number) {
    await session.selectSong(songId);
    tab = "details";
  }

  // A selection made elsewhere (e.g. the Search tab) jumps to the details tab.
  let lastSel = $state<number | null>(null);
  $effect(() => {
    const id = session.selectedSongId;
    if (id !== null && id !== lastSel) {
      lastSel = id;
      tab = "details";
    }
  });
</script>

<section class="panel history-details">
  <div class="tabs" role="tablist">
    <button
      role="tab"
      aria-selected={tab === "history"}
      class:active={tab === "history"}
      onclick={() => (tab = "history")}
    >Played tracks</button>
    <button
      role="tab"
      aria-selected={tab === "details"}
      class:active={tab === "details"}
      onclick={() => (tab = "details")}
    >Show details</button>
  </div>
  <div class="body">
    {#if tab === "history"}
      <HistoryPanel {showDetails} />
    {:else}
      <DetailsPanel />
    {/if}
  </div>
</section>

<style>
  .history-details {
    gap: 0;
    padding: 0;
    min-height: 0;
    display: flex;
    flex-direction: column;
  }

  .tabs {
    display: flex;
    gap: 2px;
    padding: 6px 6px 0;
    border-bottom: 1px solid var(--border);
    background: var(--panel-2);
    border-radius: 10px 10px 0 0;
  }

  .tabs button {
    border: 1px solid transparent;
    border-bottom: none;
    border-radius: 8px 8px 0 0;
    background: transparent;
    padding: 7px 14px;
    font-size: 13px;
    color: var(--fg-dim);
  }

  .tabs button.active {
    background: var(--panel);
    border-color: var(--border);
    color: var(--fg);
    font-weight: 600;
  }

  .body {
    flex: 1;
    min-height: 0;
    overflow: auto;
    padding: 12px;
    display: flex;
    flex-direction: column;
  }

  .body > :global(section) {
    border: none;
    background: transparent;
    padding: 0;
    border-radius: 0;
  }

  /* the tab label already names the feature; drop the duplicate panel title */
  .body > :global(section) > :global(h2) {
    display: none;
  }
</style>
