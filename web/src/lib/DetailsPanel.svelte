<script lang="ts">
  import { session } from "./session.svelte";

  /** Section 5 "Show details" tab — RadioDJ's selected-track information box. */
  const song = $derived(session.selectedSong);

  const fields: { key: string; label: string }[] = [
    { key: "artist", label: "Artist" },
    { key: "title", label: "Title" },
    { key: "album", label: "Album" },
    { key: "year", label: "Year" },
    { key: "duration", label: "Duration" },
    { key: "cue_times", label: "Cues (in/next/out)" },
    { key: "weight", label: "Weight" },
    { key: "count_played", label: "Times played" },
    { key: "id_genre", label: "Genre id" },
    { key: "id_subcat", label: "Subcat id" },
    { key: "song_type", label: "Type" },
    { key: "path", label: "File" },
  ];

  async function call(fn: () => Promise<unknown>) {
    try {
      await fn();
    } catch (e) {
      session.lastError = String(e);
    }
  }
</script>

<section class="details">
  {#if song}
    <div class="head">
      <span class="mono dim">song #{song.ID}</span>
      <span class="spacer"></span>
      <button
        onclick={() => void call(() => session.queueAddSong(song.ID))}
        disabled={!session.dbConnected}
      >Add to playlist</button>
      <button class="primary" onclick={() => void call(() => session.play(song.ID))}>Play now</button>
    </div>
    <dl>
      {#each fields as f (f.key)}
        <dt>{f.label}</dt>
        <dd class="mono" title={String(song[f.key] ?? "")}>{String(song[f.key] ?? "")}</dd>
      {/each}
    </dl>
  {:else}
    <p class="dim">No track selected. Click the details button on a search or history row.</p>
  {/if}
</section>

<style>
  .details {
    display: flex;
    flex-direction: column;
    gap: 10px;
  }

  .head {
    display: flex;
    align-items: center;
    gap: 8px;
  }

  .spacer {
    flex: 1;
  }

  dl {
    display: grid;
    grid-template-columns: max-content minmax(0, 1fr);
    gap: 4px 12px;
    margin: 0;
    font-size: 13px;
  }

  dt {
    color: var(--fg-dim);
  }

  dd {
    margin: 0;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .dim {
    color: var(--fg-dim);
  }
</style>
