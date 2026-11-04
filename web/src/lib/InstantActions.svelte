<script lang="ts">
  import { session } from "./session.svelte";

  /** Section 3 — instant actions and toggles: the big stacked buttons in
   *  RadioDJ's left column (music on/off, AutoDJ, stop all, master mute). */
  let busy = $state(false);

  const ready = $derived(session.conn === "open");
  const z = $derived(session.zoneState("main"));
  const playing = $derived(z?.playing ?? false);
  const paused = $derived(z?.paused ?? false);
  const autodj = $derived(session.autodjEnabled);
  const muted = $derived(session.zoneMix("main")?.muted ?? false);
  const masterDb = $derived(session.masterDb);

  // Keep the AutoDJ switch honest even when this panel is used on its own.
  let loadedFor = $state(-1);
  $effect(() => {
    if (session.conn === "open" && session.dbConnected && loadedFor !== session.dbConnects) {
      loadedFor = session.dbConnects;
      void session.refreshAutodj().catch((e) => (session.lastError = String(e)));
    }
  });

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

<section class="panel instant-actions">
  <h2>Instant actions</h2>

  <button
    class="tall {playing && !paused ? 'lit' : 'idle'}"
    disabled={!ready || busy || (!playing && !paused)}
    title="Pause or resume the main player"
    onclick={() =>
      void call(() => (playing && !paused ? session.pause("main") : session.resume("main")))}
  >
    {playing && !paused ? "MUSIC ON" : "MUSIC OFF"}
  </button>

  <button
    class="tall {autodj ? 'lit' : 'idle'}"
    disabled={!ready || busy || !session.dbConnected}
    title="Toggle AutoDJ"
    onclick={() => void call(() => session.setAutodj(!autodj))}
  >
    AUTO DJ {autodj ? "ON" : "OFF"}
  </button>

  <button
    class="tall danger"
    disabled={!ready || busy}
    title="Stop the main, aux, cart and instant players"
    onclick={() => void call(() => session.stopAll())}
  >
    STOP ALL
  </button>

  <button
    class="tall {muted ? 'lit' : 'idle'}"
    disabled={!ready || busy}
    title="Mute the master output for the main zone"
    onclick={() => void call(() => session.setZoneMuted("main", !muted))}
  >
    {muted ? "UNMUTE" : "MUTE"}
  </button>

  <div class="fader">
    <span class="lbl mono">MASTER</span>
    <input
      type="range"
      class="vfader"
      min={-40}
      max={0}
      step={0.5}
      value={Number.isFinite(masterDb) ? masterDb : -40}
      disabled={!ready}
      oninput={(e) => void call(() => session.setMasterDb(Number(e.currentTarget.value)))}
    />
    <span class="mono val">{Number.isFinite(masterDb) ? `${masterDb.toFixed(1)}` : "-inf"} dB</span>
  </div>

  <span class="spacer"></span>
</section>

<style>
  .instant-actions {
    gap: 8px;
  }

  button.tall {
    font-size: 14px;
    font-weight: 700;
    letter-spacing: 0.06em;
    padding: 18px 10px;
  }

  button.tall.lit {
    background: var(--accent-dim);
    border-color: var(--accent);
    color: #eaf4ff;
  }

  button.tall.idle {
    background: var(--panel-2);
  }

  button.tall.danger {
    background: #3a1f22;
    border-color: var(--err);
    color: #f3b5b1;
  }

  .fader {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 6px;
    padding-top: 8px;
    border-top: 1px solid var(--border);
  }

  .fader .lbl {
    font-size: 10px;
    color: var(--fg-dim);
  }

  .vfader {
    writing-mode: vertical-lr;
    direction: rtl;
    width: 22px;
    height: 120px;
    accent-color: var(--accent);
  }

  .val {
    font-size: 11px;
    color: var(--fg-dim);
  }

  .spacer {
    flex: 1;
  }
</style>
