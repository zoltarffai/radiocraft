<script lang="ts">
  import { session } from "./session.svelte";
  import type { ZoneName } from "./protocol";

  let zone = $state<ZoneName>("main");
  let scrub = $state<number | null>(null);

  const now = $derived(session.now);
  const z = $derived(session.zoneState(zone));

  // Internet stream: the live ICY broadcast title replaces the stored
  // row title (RadioDJ's UseStreamTitle behaviour).
  const streamTitle = $derived(zone === "main" ? (now?.stream_title ?? "") : "");

  const position = $derived(scrub ?? z?.position_secs ?? 0);
  const duration = $derived(
    (z ? (z.position_secs ?? 0) + (z.remaining_secs ?? 0) : (now?.duration_secs ?? 0)) || 0
  );
  const live = $derived(z?.playing ?? false);

  function fmt(secs: number): string {
    if (!Number.isFinite(secs)) return "--:--";
    const s = Math.max(0, Math.floor(secs));
    const m = Math.floor(s / 60);
    return `${String(m).padStart(2, "0")}:${String(s % 60).padStart(2, "0")}`;
  }

  async function call(fn: () => Promise<unknown>) {
    try {
      await fn();
    } catch (e) {
      session.lastError = String(e);
    }
  }
</script>

<section class="panel now">
  <h2>Now playing — {zone}</h2>

  <div class="titles">
    <div class="title" class:stream={!!streamTitle} title={streamTitle || now?.title}>
      {streamTitle || now?.title || "—"}
    </div>
    <div class="artist" title={streamTitle ? "" : now?.artist}>{streamTitle ? "" : now?.artist || ""}</div>
  </div>

  <div class="timeline">
    <span class="mono time">{fmt(position)}</span>
    {#if scrub === null}
      <progress value={position} max={Math.max(duration, 0.001)}></progress>
    {:else}
      <input
        type="range"
        min={0}
        max={Math.max(duration, 0.001)}
        step={0.1}
        bind:value={scrub}
        onchange={() => {
          void call(() => session.seek(scrub as number, zone));
          scrub = null;
        }}
      />
    {/if}
    <span class="mono time">{fmt(duration)}</span>
  </div>

  <div class="row">
    <label class="zsel">
      Zone
      <select bind:value={zone}>
        <option value="main">Main</option>
        <option value="aux">Aux</option>
        <option value="cart">Cart</option>
        <option value="instant">Instant</option>
      </select>
    </label>
    <button class="primary" disabled={!live} onclick={() => void call(() => session.pause(zone))}>Pause</button>
    <button disabled={!z?.paused} onclick={() => void call(() => session.resume(zone))}>Resume</button>
    <button disabled={!live} onclick={() => void call(() => session.stop(zone))}>Stop</button>
    <button class="danger" disabled={!live} onclick={() => void call(() => session.segueNow(zone))}>Segue now</button>
  </div>
</section>

<style>
  .titles {
    min-height: 3.2em;
  }

  .title {
    font-size: 22px;
    font-weight: 650;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .title.stream {
    color: var(--accent);
  }

  .artist {
    color: var(--fg-dim);
    font-size: 15px;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .timeline {
    display: flex;
    align-items: center;
    gap: 8px;
  }

  .timeline progress,
  .timeline input[type="range"] {
    flex: 1;
    accent-color: var(--accent);
    height: 8px;
  }

  .time {
    color: var(--fg-dim);
    font-size: 13px;
    min-width: 4.5em;
  }

  .time:last-child {
    text-align: right;
  }

  .row {
    display: flex;
    align-items: center;
    gap: 8px;
    flex-wrap: wrap;
  }

  .zsel {
    display: flex;
    align-items: center;
    gap: 6px;
    color: var(--fg-dim);
    margin-right: auto;
  }
</style>
