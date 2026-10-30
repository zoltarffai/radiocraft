<script lang="ts">
  import { session } from "./session.svelte";
  import type { HistoryRow } from "./protocol";

  let rows = $state<HistoryRow[]>([]);
  let loading = $state(false);

  const ready = $derived(session.conn === "open" && session.dbConnected);

  async function refresh() {
    loading = true;
    try {
      const res = await fetch(`/api/history?limit=${200}`);
      if (!res.ok) throw new Error(`HTTP ${res.status}`);
      const body = (await res.json()) as { history: HistoryRow[] };
      rows = body.history ?? [];
    } catch (e) {
      session.lastError = String(e);
    } finally {
      loading = false;
    }
  }

  function when(row: HistoryRow): string {
    const d = new Date(row.datetime_play);
    return Number.isNaN(d.getTime()) ? row.datetime_play : d.toLocaleString();
  }

  $effect(() => {
    if (ready) void refresh();
  });
</script>

<section class="panel history">
  <h2>History</h2>
  <div class="row">
    <button onclick={() => void refresh()} disabled={!ready || loading}>{loading ? "…" : "Refresh"}</button>
  </div>
  <div class="scroll">
    <table>
      <thead>
        <tr><th>When</th><th>Artist</th><th>Title</th></tr>
      </thead>
      <tbody>
        {#each rows as row (row.ID)}
          <tr>
            <td class="mono dim">{when(row)}</td>
            <td>{row.artist}</td>
            <td>{row.title}</td>
          </tr>
        {:else}
          <tr class="empty"><td colspan={3}>{ready ? "No plays yet." : "Connect the database first."}</td></tr>
        {/each}
      </tbody>
    </table>
  </div>
</section>

<style>
  .row {
    display: flex;
    gap: 8px;
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
    max-width: 28ch;
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

  .empty td {
    color: var(--fg-dim);
    text-align: center;
    padding: 14px;
  }
</style>
