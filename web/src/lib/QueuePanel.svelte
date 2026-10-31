<script lang="ts">
  import { session } from "./session.svelte";
  import type { QueueRow } from "./protocol";

  let loading = $state(false);

  const rows = $derived(session.queue);
  const ready = $derived(session.conn === "open" && session.dbConnected);
  let loadedFor = $state(0);

  // Auto-load the first page once the queue becomes ready (after connect).
  $effect(() => {
    if (ready && loadedFor !== session.dbConnects) {
      loadedFor = session.dbConnects;
      void refresh();
    }
  });

  async function refresh() {
    loading = true;
    try {
      await session.loadQueue();
    } catch (e) {
      session.lastError = String(e);
    } finally {
      loading = false;
    }
  }

  async function call(fn: () => Promise<unknown>) {
    try {
      await fn();
      await refresh();
    } catch (e) {
      session.lastError = String(e);
    }
  }

  /** Swap ETA with the neighbouring row (1-based display index). */
  function move(index: number, dir: -1 | 1) {
    const target = index + dir;
    if (target < 0 || target >= rows.length) return;
    void call(() => session.queueMove(rows[index].ID, rows[target].ID));
  }

  /** Push an ETA forward by n seconds from now (quick schedule). */
  function scheduleSoon(row: QueueRow, secsFromNow: number) {
    const eta = new Date(Date.now() + secsFromNow * 1000);
    const pad = (n: number) => String(n).padStart(2, "0");
    const local = `${eta.getFullYear()}-${pad(eta.getMonth() + 1)}-${pad(eta.getDate())} ${pad(eta.getHours())}:${pad(eta.getMinutes())}:${pad(eta.getSeconds())}`;
    void call(() => session.queueEtaSet(row.ID, local));
  }

  function when(row: QueueRow): string {
    if (!row.ETA) return "—";
    const d = new Date(row.ETA);
    return Number.isNaN(d.getTime())
      ? row.ETA
      : d.toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" });
  }
</script>

<section class="panel queue">
  <h2>Play queue</h2>
  <div class="row">
    <button onclick={() => void refresh()} disabled={!ready || loading}>{loading
      ? "…"
      : "Refresh"}</button>
    <button class="danger" onclick={() => void call(() => session.queueClear())} disabled={!ready}>Clear</button>
    <span class="count mono">{rows.length} items</span>
  </div>
  <div class="scroll">
    <table>
      <thead>
        <tr><th>ETA</th><th>Artist</th><th>Title</th><th>Actions</th></tr>
      </thead>
      <tbody>
        {#each rows as row, i (row.ID)}
          <tr>
            <td class="mono dim" title={row.ETA ?? ""}>{when(row)}</td>
            <td title={row.artist}>{row.artist}</td>
            <td title={row.title}>{row.title}</td>
            <td class="actions">
              <button title="Move up" disabled={i === 0} onclick={() => move(i, -1)}>↑</button>
              <button title="Move down" disabled={i === rows.length - 1} onclick={() => move(i, 1)}>↓</button>
              <button title="Play in 1 minute" onclick={() => scheduleSoon(row, 60)}>1 min</button>
              <button title="Play in 15 minutes" onclick={() => scheduleSoon(row, 900)}>15 min</button>
              <button class="danger" title="Remove" onclick={() => void call(() => session.queueRemove(row.ID))}>✕</button>
            </td>
          </tr>
        {:else}
          <tr class="empty"><td colspan={4}>{ready ? "Queue is empty." : "Connect the database first."}</td></tr>
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
