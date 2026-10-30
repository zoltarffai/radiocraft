<script lang="ts">
  import { session } from "./session.svelte";
  import type { SongRow } from "./protocol";

  let needle = $state("");
  let selected = $state<SongRow | null>(null);
  let loading = $state(false);

  const songs = $derived(session.songs);
  const ready = $derived(session.conn === "open" && session.dbConnected);

  async function search() {
    loading = true;
    try {
      await session.loadSongs(needle.trim() || undefined);
    } catch (e) {
      session.lastError = String(e);
    } finally {
      loading = false;
    }
  }

  async function play(song: SongRow, cue: boolean) {
    try {
      await session[cue ? "cue" : "play"](song.ID);
    } catch (e) {
      session.lastError = String(e);
    }
  }

  function fmt(duration: string): string {
    return duration || "";
  }
</script>

<section class="panel library">
  <h2>Library</h2>

  <div class="row">
    <input
      type="search"
      placeholder="Search artist / title…"
      bind:value={needle}
      onkeydown={(e) => e.key === "Enter" && void search()}
      disabled={!ready}
    />
    <button onclick={() => void search()} disabled={!ready || loading}>{loading ? "…" : "Search"}</button>
    <span class="count mono">{songs.length ? `${songs.length}` : ""}</span>
  </div>

  <div class="scroll">
    <table>
      <thead>
        <tr><th>Artist</th><th>Title</th><th>Album</th><th>Length</th><th></th></tr>
      </thead>
      <tbody>
        {#each songs as song (song.ID)}
          <tr class:sel={selected?.ID === song.ID} onclick={() => (selected = song)}>
            <td>{song.artist}</td>
            <td>{song.title}</td>
            <td class="dim">{song.album}</td>
            <td class="mono dim">{fmt(song.duration)}</td>
            <td class="actions">
              <button onclick={(e) => { e.stopPropagation(); void play(song, false); }}>Play</button>
              <button onclick={(e) => { e.stopPropagation(); void play(song, true); }}>Cue</button>
            </td>
          </tr>
        {:else}
          <tr class="empty"><td colspan={5}>{ready ? "No songs." : "Connect the database first."}</td></tr>
        {/each}
      </tbody>
    </table>
  </div>
</section>

<style>
  .row {
    display: flex;
    align-items: center;
    gap: 8px;
  }

  .row input {
    flex: 1;
  }

  .count {
    color: var(--fg-dim);
    min-width: 3em;
    text-align: right;
  }

  .scroll {
    overflow: auto;
    max-height: 260px;
    border: 1px solid var(--border);
    border-radius: 8px;
  }

  table {
    width: 100%;
    border-collapse: collapse;
    font-size: 13px;
  }

  th,
  td {
    text-align: left;
    padding: 5px 8px;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
    max-width: 24ch;
  }

  thead th {
    position: sticky;
    top: 0;
    background: var(--panel-2);
    color: var(--fg-dim);
    font-weight: 500;
  }

  tbody tr {
    cursor: pointer;
  }

  tbody tr:hover {
    background: var(--panel-2);
  }

  tr.sel {
    background: var(--accent-dim);
  }

  .dim {
    color: var(--fg-dim);
  }

  .actions {
    display: flex;
    gap: 6px;
  }

  .empty td {
    color: var(--fg-dim);
    text-align: center;
    padding: 14px;
  }
</style>
