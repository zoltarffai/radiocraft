<script lang="ts">
  import { session } from "./session.svelte";
  import type { PlaylistHeader, PlaylistItemRow } from "./protocol";

  let selectedId = $state<number | null>(null);
  let newItem = $state("");
  // The wire row keeps decimals as strings; the editor works on numbers.
  type CueEdit = { ID: number; cstart: number; cnext: number; cend: number; fin: number; fout: number };
  let editRow = $state<CueEdit | null>(null);
  let loading = $state(false);

  const playlists = $derived(session.playlists);
  const items = $derived(session.playlistItems);
  const ready = $derived(session.conn === "open" && session.dbConnected);
  let loadedFor = $state(0);

  $effect(() => {
    if (ready && loadedFor !== session.dbConnects) {
      loadedFor = session.dbConnects;
      void refreshLists();
    }
  });

  async function refreshLists() {
    try {
      await session.loadPlaylists();
      if (selectedId !== null && !playlists.some((p) => p.ID === selectedId)) selectedId = null;
      if (selectedId === null && playlists.length > 0) await select(playlists[0].ID);
      else if (selectedId !== null) await session.loadPlaylistItems(selectedId);
    } catch (e) {
      session.lastError = String(e);
    }
  }

  async function select(id: number) {
    selectedId = id;
    editRow = null;
    try {
      await session.loadPlaylistItems(id);
    } catch (e) {
      session.lastError = String(e);
    }
  }

  async function call(fn: () => Promise<unknown>) {
    loading = true;
    try {
      await fn();
      if (selectedId !== null) await session.loadPlaylistItems(selectedId);
    } catch (e) {
      session.lastError = String(e);
    } finally {
      loading = false;
    }
  }

  async function create() {
    const name = window.prompt("New playlist name:");
    if (!name?.trim()) return;
    void call(async () => {
      const id = await session.createPlaylist(name.trim());
      await session.loadPlaylists();
      await select(id);
    });
  }

  async function rename() {
    if (selectedId === null) return;
    const current = playlists.find((p) => p.ID === selectedId)?.name ?? "";
    const name = window.prompt("Rename playlist:", current);
    if (!name?.trim()) return;
    void call(async () => {
      await session.renamePlaylist(selectedId as number, name.trim());
      await session.loadPlaylists();
    });
  }

  async function removePlaylist() {
    if (selectedId === null) return;
    if (!window.confirm("Delete this playlist and all its items?")) return;
    void call(async () => {
      await session.deletePlaylist(selectedId as number);
      selectedId = null;
      session.playlistItems = [];
      await session.loadPlaylists();
    });
  }

  function addItem() {
    if (selectedId === null || !/^\s*\d+\s*$/.test(newItem)) {
      session.lastError = "enter a numeric song id to add";
      return;
    }
    void call(async () => {
      await session.playlistAddItem(selectedId as number, Number(newItem));
      newItem = "";
    });
  }

  function move(index: number, dir: -1 | 1) {
    if (selectedId === null) return;
    const target = index + dir;
    if (target < 0 || target >= items.length) return;
    void call(() => session.playlistMoveItem(selectedId as number, index + 1, target + 1));
  }

  function startEdit(row: PlaylistItemRow) {
    if (editRow?.ID === row.ID) {
      editRow = null;
      return;
    }
    const num = (v: string) => (Number.isFinite(Number(v)) ? Number(v) : 0);
    editRow = {
      ID: row.ID,
      cstart: num(row.cstart),
      cnext: num(row.cnext),
      cend: num(row.cend),
      fin: num(row.fin),
      fout: num(row.fout),
    };
  }

  async function saveCues(row: PlaylistItemRow) {
    const e = editRow;
    if (!e) return;
    void call(async () => {
      await session.playlistSetCues(row.ID, {
        cstart: e.cstart,
        cnext: e.cnext,
        cend: e.cend,
        fin: e.fin,
        fout: e.fout,
      });
      editRow = null;
    });
  }
</script>

<section class="panel playlists">
  <h2>Playlists</h2>
  <div class="row">
    <select
      value={selectedId ?? ""}
      onchange={(e) => {
        const v = Number((e.currentTarget as HTMLSelectElement).value);
        if (v > 0) void select(v);
      }}>
      <option value="" disabled>Choose a playlist…</option>
      {#each playlists as p (p.ID)}
        <option value={p.ID}>{p.name}</option>
      {/each}
    </select>
    <button onclick={() => void create()} disabled={!ready}>New</button>
    <button onclick={() => void rename()} disabled={selectedId === null}>Rename</button>
    <button class="danger" onclick={() => void removePlaylist()} disabled={selectedId === null}>Delete</button>
  </div>

  <div class="row">
    <input
      type="text"
      inputmode="numeric"
      placeholder="song id to add…"
      bind:value={newItem}
      onkeydown={(e) => e.key === "Enter" && addItem()}
      disabled={selectedId === null}
    />
    <button onclick={addItem} disabled={selectedId === null || loading}>Add item</button>
    <span class="count mono">{items.length} items</span>
  </div>

  <div class="scroll">
    <table>
      <thead>
        <tr><th>Ord</th><th>Artist</th><th>Title</th><th>sID</th><th>Actions</th></tr>
      </thead>
      <tbody>
        {#each items as row, i (row.ID)}
          <tr>
            <td class="mono dim">{row.ord}</td>
            <td title={row.artist ?? ""}>{row.artist ?? "(no song)"}</td>
            <td title={row.title ?? ""}>{row.title ?? ""}</td>
            <td class="mono dim">{row.sID}</td>
            <td class="actions">
              <button title="Move up" disabled={i === 0} onclick={() => move(i, -1)}>↑</button>
              <button title="Move down" disabled={i === items.length - 1} onclick={() => move(i, 1)}>↓</button>
              <button title="Play this item" disabled={!row.sID} onclick={() => void session.play(row.sID)}>▶</button>
              <button title="Queue this item" disabled={!row.sID} onclick={() => void call(() => session.queueAddSong(row.sID))}>
                Queue
              </button>
              <button title="Edit cues" onclick={() => startEdit(row)}>Cues</button>
              <button class="danger" title="Remove item" onclick={() => void call(() => session.playlistRemoveItem(row.ID))}>
                ✕
              </button>
            </td>
          </tr>
          {#if editRow && editRow.ID === row.ID}
            <tr class="cueedit">
              <td colspan={5}>
                <div class="cues">
                  <label>
                    cstart
                    <input type="number" min="0" step="0.001" bind:value={editRow.cstart} />
                  </label>
                  <label>
                    cnext
                    <input type="number" min="0" step="0.001" bind:value={editRow.cnext} />
                  </label>
                  <label>
                    cend
                    <input type="number" min="0" step="0.001" bind:value={editRow.cend} />
                  </label>
                  <label>
                    fin
                    <input type="number" min="0" step="0.001" bind:value={editRow.fin} />
                  </label>
                  <label>
                    fout
                    <input type="number" min="0" step="0.001" bind:value={editRow.fout} />
                  </label>
                  <button class="primary" onclick={() => void saveCues(row)}>Save cues</button>
                  <button onclick={() => (editRow = null)}>Cancel</button>
                </div>
              </td>
            </tr>
          {/if}
        {:else}
          <tr class="empty"><td colspan={5}>{ready ? "No items — pick a playlist or press New." : "Connect the database first."}</td></tr>
        {/each}
      </tbody>
    </table>
  </div>
</section>

<style>
  .row {
    display: flex;
    gap: 8px;
    align-items: center;
  }

  .count {
    margin-left: auto;
    color: var(--fg-dim);
    font-size: 12px;
  }

  .scroll {
    overflow: auto;
    max-height: 300px;
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

  .dim {
    color: var(--fg-dim);
  }

  .actions {
    display: flex;
    gap: 4px;
  }

  .actions button {
    padding: 2px 7px;
    font-size: 12px;
  }

  .cueedit td {
    background: var(--panel-2);
    white-space: normal;
  }

  .cues {
    display: flex;
    gap: 10px;
    align-items: end;
    flex-wrap: wrap;
    padding: 4px 0;
  }

  .cues label {
    display: flex;
    flex-direction: column;
    gap: 3px;
    font-size: 11px;
    color: var(--fg-dim);
  }

  .cues input {
    width: 8ch;
  }

  .empty td {
    color: var(--fg-dim);
    text-align: center;
    padding: 14px;
  }
</style>
