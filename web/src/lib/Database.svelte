<script lang="ts">
  import { session } from "./session.svelte";
  import type { DbConfigView } from "./protocol";

  let cfg = $state<DbConfigView | null>(null);
  let busy = $state(false);
  let note = $state("");

  const connected = $derived(session.dbConnected);
  const conn = $derived(session.conn);

  async function load() {
    busy = true;
    note = "";
    try {
      cfg = await session.dbConfig();
      note = `Configured: ${cfg.host}:${cfg.port}/${cfg.database}`;
    } catch (e) {
      note = String(e);
    } finally {
      busy = false;
    }
  }

  async function save() {
    if (!cfg) return;
    busy = true;
    note = "";
    try {
      await session.configureDb(cfg);
      note = "Configuration saved.";
    } catch (e) {
      note = String(e);
    } finally {
      busy = false;
    }
  }

  async function connect() {
    if (!cfg) return;
    busy = true;
    note = "";
    try {
      await session.configureDb(cfg);
      const songs = await session.connectDb();
      note = `Connected, ${songs} songs in library.`;
    } catch (e) {
      session.dbConnected = false;
      note = String(e);
    } finally {
      busy = false;
    }
  }

  $effect(() => {
    if (conn === "open" && cfg === null) void load();
  });
</script>

<section class="panel db">
  <h2>Database</h2>

  {#if cfg === null}
    <button disabled={busy || conn !== "open"} onclick={() => void load()}>
      {busy ? "Loading…" : "Load configuration"}
    </button>
  {:else}
    <div class="fields">
      <label>Host <input bind:value={cfg.host} /></label>
      <label>Port <input type="number" bind:value={cfg.port} /></label>
      <label>Database <input bind:value={cfg.database} /></label>
      <label>User <input bind:value={cfg.username} /></label>
      <label>Password <input type="password" bind:value={cfg.password} /></label>
    </div>
    <div class="row">
      <button onclick={() => void save()} disabled={busy}>Save</button>
      <button class="primary" onclick={() => void connect()} disabled={busy}>
        {busy ? "Working…" : "Connect"}
      </button>
      <span class="badge" class:ok={connected} class:off={!connected}>
        {connected ? "connected" : "offline"}
      </span>
    </div>
  {/if}

  {#if note}<p class="note mono">{note}</p>{/if}
</section>

<style>
  .fields {
    display: grid;
    grid-template-columns: 2fr 1fr 2fr;
    gap: 8px;
  }

  .fields label {
    display: flex;
    flex-direction: column;
    gap: 3px;
    color: var(--fg-dim);
    font-size: 12px;
  }

  .row {
    display: flex;
    align-items: center;
    gap: 8px;
  }

  .badge {
    margin-left: auto;
    font-size: 12px;
    padding: 2px 8px;
    border-radius: 999px;
    border: 1px solid var(--border);
    color: var(--fg-dim);
  }

  .badge.ok {
    border-color: var(--ok);
    color: var(--ok);
  }

  .note {
    margin: 0;
    color: var(--fg-dim);
    font-size: 12px;
    word-break: break-word;
  }
</style>
