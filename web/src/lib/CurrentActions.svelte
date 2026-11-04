<script lang="ts">
  import { session } from "./session.svelte";
  import { dbFromWire } from "./protocol";
  import Meter from "./Meter.svelte";

  /** Section 1 — current actions: full-width now-playing banner. Small icon
   *  transport buttons on the left, track info center, meters right, and the
   *  track timeline across the whole bottom edge (RadioDJ's main banner). */
  let scrub = $state<number | null>(null);

  const now = $derived(session.now);
  const z = $derived(session.zoneState("main"));
  const live = $derived(z?.playing ?? false);
  const paused = $derived(z?.paused ?? false);

  const streamTitle = $derived(now?.stream_title ?? "");
  const title = $derived(streamTitle || now?.title || "— no track —");
  const artist = $derived(streamTitle ? "(internet stream)" : (now?.artist ?? ""));

  const position = $derived(scrub ?? z?.position_secs ?? 0);
  const duration = $derived(
    (z ? (z.position_secs ?? 0) + (z.remaining_secs ?? 0) : (now?.duration_secs ?? 0)) || 0
  );
  const remaining = $derived(Math.max(0, duration - position));

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

<section class="panel current-actions">
  <div class="transport">
    <button
      class="icon next"
      title="Play the next track now (segue)"
      disabled={!live && !paused}
      onclick={() => void call(() => session.segueNow("main"))}
    >⏭</button>
    <button class="icon stop" title="Stop playback" disabled={!live} onclick={() => void call(() => session.stop("main"))}>⏹</button>
    {#if paused}
      <button class="icon" title="Resume playback" onclick={() => void call(() => session.resume("main"))}>▶</button>
    {:else}
      <button class="icon" title="Pause playback" disabled={!live} onclick={() => void call(() => session.pause("main"))}>⏸</button>
    {/if}
  </div>

  <div class="song">
    <div class="title" title={title}>{title}</div>
    <div class="artist mono" class:stream={!!streamTitle}>{artist}</div>
  </div>

  <div class="meters">
    <div class="meter-row">
      <span class="lbl mono">MASTER</span>
      <Meter db={session.master.level_db ?? -Infinity} peakDb={dbFromWire(session.master.peak_hold_db)} height={12} />
    </div>
    <div class="meter-row">
      <span class="lbl mono">AUTODJ</span>
      <span class="sw {session.autodjEnabled ? 'on' : 'off'}">{session.autodjEnabled ? "ON" : "OFF"}</span>
    </div>
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
          void call(() => session.seek(scrub as number, "main"));
          scrub = null;
        }}
      />
    {/if}
    <span class="mono time rem" title="time remaining">-{fmt(remaining)}</span>
  </div>
</section>

<style>
  .current-actions {
    display: grid;
    grid-template-columns: auto minmax(0, 1fr) auto;
    grid-template-areas:
      "transport song meters"
      "timeline timeline timeline";
    gap: 8px 16px;
    align-items: center;
    padding: 10px 14px;
  }

  .transport {
    grid-area: transport;
    display: flex;
    gap: 6px;
  }

  button.icon {
    width: 38px;
    height: 38px;
    padding: 0;
    font-size: 17px;
    line-height: 1;
    display: grid;
    place-items: center;
    border-radius: 8px;
  }

  button.next {
    background: #1d3a2a;
    border-color: var(--ok);
    color: #9ff0c3;
  }

  button.stop {
    background: #3a1f22;
    border-color: var(--err);
    color: #f3b5b1;
  }

  .song {
    grid-area: song;
    min-width: 0;
    display: flex;
    flex-direction: column;
    gap: 2px;
  }

  .title {
    font-size: 24px;
    font-weight: 700;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .artist {
    color: var(--fg-dim);
    font-size: 13px;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .artist.stream {
    color: var(--accent);
  }

  .meters {
    grid-area: meters;
    display: flex;
    flex-direction: column;
    gap: 8px;
    min-width: 190px;
  }

  .meter-row {
    display: flex;
    align-items: center;
    gap: 8px;
  }

  .lbl {
    font-size: 10px;
    color: var(--fg-dim);
    min-width: 5ch;
  }

  .sw {
    font-size: 11px;
    font-weight: 700;
    padding: 2px 10px;
    border-radius: 999px;
    border: 1px solid var(--border);
  }

  .sw.on {
    color: var(--ok);
    border-color: var(--ok);
  }

  .sw.off {
    color: var(--fg-dim);
  }

  .timeline {
    grid-area: timeline;
    display: flex;
    align-items: center;
    gap: 8px;
    border-top: 1px solid var(--border);
    padding-top: 8px;
  }

  .timeline progress,
  .timeline input[type="range"] {
    flex: 1;
    accent-color: var(--accent);
    height: 10px;
  }

  .time {
    color: var(--fg-dim);
    font-size: 13px;
    min-width: 4.5em;
  }

  .time.rem {
    text-align: right;
    color: var(--warn);
  }

  @media (max-width: 1100px) {
    .current-actions {
      grid-template-columns: auto minmax(0, 1fr);
      grid-template-areas:
        "transport song"
        "meters meters"
        "timeline timeline";
    }

    .meters {
      min-width: 0;
      flex-direction: row;
      align-items: center;
    }
  }
</style>
