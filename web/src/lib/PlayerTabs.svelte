<script lang="ts">
  import AuxPlayers from "./AuxPlayers.svelte";
  import CartWall from "./CartWall.svelte";

  /** Section 6 — tabbed players window: aux players plus the instant wall
   *  (RadioDJ's Plugin_AuxPlayers / Plugin_InstantPlayers). */
  let tab = $state<"aux" | "instant">("aux");
</script>

<section class="panel player-tabs">
  <div class="tabs" role="tablist">
    <button
      role="tab"
      aria-selected={tab === "aux"}
      class:active={tab === "aux"}
      onclick={() => (tab = "aux")}
    >Aux players</button>
    <button
      role="tab"
      aria-selected={tab === "instant"}
      class:active={tab === "instant"}
      onclick={() => (tab = "instant")}
    >Instant players</button>
  </div>
  <div class="body">
    {#if tab === "aux"}
      <AuxPlayers />
    {:else}
      <CartWall zone="instant" />
    {/if}
  </div>
</section>

<style>
  .player-tabs {
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
