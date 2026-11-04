<script lang="ts">
  import { session } from "./session.svelte";

  /** RadioDJ's "Web streams" window: play an internet stream URL (or any
   *  file path) directly on the main player. Stream rows can also be kept
   *  in the library (type "Stream") from the Search tab. */
  let url = $state("");
  let busy = $state(false);

  const now = $derived(session.now);
  const z = $derived(session.zoneState("main"));
  const live = $derived(z?.playing ?? false);
  const streamTitle = $derived(now?.stream_title ?? "");
  const isStream = $derived((now?.track_type ?? 0) === 5);

  async function call(fn: () => Promise<unknown>) {
    if (busy) return;
    busy = true;
    try {
      await fn();
    } catch (e) {
      session.lastError = String(e);
    } finally {
      busy = false;
    }
  }
</script>

<section class="streams">
  <div class="row">
    <input
      type="text"
      placeholder="Stream URL (http…) or file path"
      bind:value={url}
      onkeydown={(e) => e.key === "Enter" && url.trim() && void call(() => session.playFile(url.trim()))}
    />
    <button
      class="primary"
      disabled={!url.trim() || busy || session.conn !== "open"}
      onclick={() => void call(() => session.playFile(url.trim()))}
    >Play</button>
    <button class="danger" disabled={!live || busy} onclick={() => void call(() => session.stop("main"))}>
      Stop
    </button>
  </div>

  <div class="now">
    {#if live && isStream}
      <span class="dot live"></span>
      <span class="title" title={streamTitle || now?.title}>{streamTitle || now?.title}</span>
    {:else if live}
      <span class="dot live"></span>
      <span class="title">{now?.artist} — {now?.title}</span>
    {:else}
      <span class="dot"></span>
      <span class="dim">Nothing playing.</span>
    {/if}
  </div>
</section>

<style>
  .streams {
    display: flex;
    flex-direction: column;
    gap: 10px;
  }

  .row {
    display: flex;
    gap: 8px;
  }

  .row input {
    flex: 1;
    min-width: 20ch;
  }

  .now {
    display: flex;
    align-items: center;
    gap: 8px;
    border: 1px solid var(--border);
    border-radius: 8px;
    padding: 8px 10px;
    background: var(--panel-2);
    min-width: 0;
  }

  .dot {
    width: 9px;
    height: 9px;
    border-radius: 50%;
    background: var(--border);
    flex: none;
  }

  .dot.live {
    background: var(--ok);
  }

  .title {
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .dim {
    color: var(--fg-dim);
  }
</style>
