<script lang="ts">
  import { session } from "./session.svelte";
  import type { HistoryRow } from "./protocol";

  let needle = $state("");
  let loading = $state(false);
  let offset = $state(0);
  const PAGE = 200;

  const rows = $derived(session.history);
  const total = $derived(session.historyTotal);
  const ready = $derived(session.conn === "open" && session.dbConnected);
  let loadedFor = $state(0);

  // Auto-load the first page once the database becomes ready (after connect).
  $effect(() => {
    if (ready && loadedFor !== session.dbConnects) {
      loadedFor = session.dbConnects;
      void refresh();
    }
  });

  async function refresh(reset = true) {
    if (reset) offset = 0;
    loading = true;
    try {
      await session.filterHistory(needle.trim(), PAGE, offset);
    } catch (e) {
      session.lastError = String(e);
    } finally {
      loading = false;
    }
  }

  async function call(fn: () => Promise<unknown>) {
    try {
      await fn();
      await refresh(false);
    } catch (e) {
      session.lastError = String(e);
    }
  }

  function when(row: HistoryRow): string {
    if (!row.date_played) return "—";
    const d = new Date(row.date_played);
    return Number.isNaN(d.getTime()) ? row.date_played : d.toLocaleString();
  }
</script>

<section class="panel history">
  <h2>History</h2>
  <div class="row">
    <input
      type="search"
      placeholder="Filter artist / title…"
      bind:value={needle}
      onkeydown={(e) => e.key === "Enter" && void refresh()}
    />
    <button onclick={() => void refresh()} disabled={!ready || loading}>{loading
      ? "…"
      : "Search"}</button>
    <button
      disabled={!ready || offset === 0}
      onclick={() => {
        offset = Math.max(0, offset - PAGE);
        void refresh(false);
      }}>Prev</button>
    <button
      disabled={!ready || offset + PAGE >= total}
      onclick={() => {
        offset += PAGE;
        void refresh(false);
      }}>Next</button>
    <span class="count mono">{total} plays</span>
  </div>
  <div class="scroll">
    <table>
      <thead>
        <tr><th>When</th><th>Artist</th><th>Title</th><th>Actions</th></tr>
      </thead>
      <tbody>
        {#each rows as row (row.ID)}
          <tr>
            <td class="mono dim">{when(row)}</td>
            <td title={row.artist}>{row.artist}</td>
            <td title={row.title}>{row.title}</td>
            <td class="actions">
              <button title="Rewind this play" onclick={() => void call(() => session.playFromHistory(row.ID))}>
                Rewind
              </button>
              <button
                title="Add to play queue"
                disabled={!row.trackID}
                onclick={() => void call(() => session.queueAddSong(row.trackID))}>
                Queue
              </button>
              <button class="danger" title="Delete row" onclick={() => void call(() => session.deleteHistory(row.ID))}>
                ✕
              </button>
            </td>
          </tr>
        {:else}
          <tr class="empty"><td colspan={4}>{ready ? "No plays yet." : "Connect the database first."}</td></tr>
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

  .row input[type="search"] {
    flex: 1;
    min-width: 12ch;
  }

  .count {
    margin-left: auto;
    color: var(--fg-dim);
    font-size: 12px;
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
    max-width: 22ch;
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

  .empty td {
    color: var(--fg-dim);
    text-align: center;
    padding: 14px;
  }
</style>
