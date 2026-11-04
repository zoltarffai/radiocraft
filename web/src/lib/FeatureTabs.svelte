<script lang="ts">
  import Library from "./Library.svelte";
  import PlaylistEditor from "./PlaylistEditor.svelte";
  import CartWall from "./CartWall.svelte";
  import RotationEditor from "./RotationEditor.svelte";
  import WebStreams from "./WebStreams.svelte";

  /** Section 4 — tabbed feature window (RadioDJ's search window area):
   *  track search, playlists, cart wall, rotations, web streams. */
  type Tab = "search" | "playlists" | "carts" | "rotations" | "streams";

  let tab = $state<Tab>("search");

  const tabs: { id: Tab; label: string }[] = [
    { id: "search", label: "Search" },
    { id: "playlists", label: "Playlists" },
    { id: "carts", label: "Cart wall" },
    { id: "rotations", label: "Rotations" },
    { id: "streams", label: "Streams" },
  ];
</script>

<section class="panel feature-tabs">
  <div class="tabs" role="tablist">
    {#each tabs as t (t.id)}
      <button
        role="tab"
        aria-selected={tab === t.id}
        class:active={tab === t.id}
        onclick={() => (tab = t.id)}
      >{t.label}</button>
    {/each}
  </div>
  <div class="body">
    {#if tab === "search"}
      <Library />
    {:else if tab === "playlists"}
      <PlaylistEditor />
    {:else if tab === "carts"}
      <CartWall />
    {:else if tab === "rotations"}
      <RotationEditor />
    {:else}
      <WebStreams />
    {/if}
  </div>
</section>

<style>
  .feature-tabs {
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
    flex-wrap: wrap;
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
