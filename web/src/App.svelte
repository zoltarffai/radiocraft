<script lang="ts">
  import { onMount } from "svelte";
  import { session } from "./lib/session.svelte";
  import NowPlaying from "./lib/NowPlaying.svelte";
  import CartWall from "./lib/CartWall.svelte";
  import ZoneMixer from "./lib/ZoneMixer.svelte";
  import Database from "./lib/Database.svelte";
  import Library from "./lib/Library.svelte";
  import HistoryPanel from "./lib/HistoryPanel.svelte";
  import QueuePanel from "./lib/QueuePanel.svelte";
  import PlaylistEditor from "./lib/PlaylistEditor.svelte";
  import AutoDJPanel from "./lib/AutoDJPanel.svelte";

  onMount(() => {
    session.start();
    return () => session.closeSocket();
  });

  const conn = $derived(session.conn);
  const ready = $derived(session.ready);
  const status = $derived(session.status);
  const lastError = $derived(session.lastError);
</script>

<div class="shell">
  <header>
    <span class="brand">RadioCraft</span>
    <span class="sub mono">{ready ? `${ready.server} ${ready.version}` : ""}</span>
    <span class="spacer"></span>
    <span class="status mono" data-level={status.level}>{status.message}</span>
    <span class="conn" data-state={conn}>{conn}</span>
  </header>

  <main>
    <section class="col top">
      <NowPlaying />
      <ZoneMixer />
      <AutoDJPanel />
    </section>
    <section class="col top">
      <Database />
    </section>

    <section class="col wide"><Library /></section>
    <section class="col wide"><CartWall /></section>
    <section class="col wide"><QueuePanel /></section>
    <section class="col wide"><PlaylistEditor /></section>
    <section class="col wide"><HistoryPanel /></section>
  </main>

  {#if lastError}
    <div class="toast" role="alert">
      <span class="mono">{lastError}</span>
      <button onclick={() => (session.lastError = null)}>Dismiss</button>
    </div>
  {/if}
</div>

<style>
  .shell {
    display: flex;
    flex-direction: column;
    height: 100%;
  }

  header {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 10px 16px;
    border-bottom: 1px solid var(--border);
    background: var(--panel);
  }

  .brand {
    font-weight: 700;
    font-size: 16px;
    color: var(--accent);
  }

  .sub {
    color: var(--fg-dim);
    font-size: 12px;
  }

  .spacer {
    flex: 1;
  }

  .status {
    font-size: 12px;
    color: var(--fg-dim);
  }

  .status[data-level="error"] {
    color: var(--err);
  }

  .status[data-level="warn"] {
    color: var(--warn);
  }

  .conn {
    font-size: 12px;
    padding: 2px 10px;
    border-radius: 999px;
    border: 1px solid var(--border);
  }

  .conn[data-state="open"] {
    color: var(--ok);
    border-color: var(--ok);
  }

  .conn[data-state="connecting"] {
    color: var(--warn);
  }

  .conn[data-state="closed"] {
    color: var(--err);
    border-color: var(--err);
  }

  main {
    flex: 1;
    overflow: auto;
    padding: 14px;
    display: grid;
    grid-template-columns: minmax(380px, 1fr) minmax(300px, 1fr);
    gap: 14px;
    align-content: start;
  }

  .col {
    display: flex;
    flex-direction: column;
    gap: 14px;
    min-width: 0;
  }

  .col.wide {
    grid-column: 1 / -1;
  }

  .toast {
    position: fixed;
    bottom: 14px;
    left: 50%;
    transform: translateX(-50%);
    display: flex;
    align-items: center;
    gap: 10px;
    background: #3a1f22;
    border: 1px solid var(--err);
    border-radius: 8px;
    padding: 8px 12px;
    max-width: 90vw;
  }

  .toast span {
    font-size: 12px;
    word-break: break-word;
  }

  @media (max-width: 900px) {
    main {
      grid-template-columns: 1fr;
    }
  }
</style>
