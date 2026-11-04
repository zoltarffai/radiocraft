<script lang="ts">
  import { session } from "./session.svelte";

  /** Section 3 — instant actions and toggles. Six evenly sized buttons;
   *  empty dashed placeholders mark the slots that are not defined yet. */
  let muted = $state(false);

  const z = $derived(session.zoneState("main"));
  const musicOn = $derived(z?.playing ?? false);

  async function call(fn: () => Promise<unknown>) {
    try {
      await fn();
    } catch (e) {
      session.lastError = String(e);
    }
  }

  async function mute() {
    muted = !muted;
    try {
      await session.setZoneMuted("main", muted);
    } catch (e) {
      muted = !muted;
      session.lastError = String(e);
    }
  }
</script>

<section class="panel instant-actions">
  <h2>Instant actions</h2>
  <div class="btns">
    <button
      class="toggle"
      class:on={musicOn}
      title="Start/stop the main player (music on/off)"
      disabled={session.conn !== "open"}
      onclick={() => void call(() => (musicOn ? session.stop("main") : session.playNext()))}
    >
      {musicOn ? "MUSIC ON" : "MUSIC OFF"}
    </button>
    <button
      class="toggle"
      class:on={session.autodjEnabled}
      title="Enable/disable the Auto DJ scheduler"
      onclick={() => void call(() => session.setAutodj(!session.autodjEnabled))}
    >
      AUTO DJ {session.autodjEnabled ? "ON" : "OFF"}
    </button>
    <button class="danger" title="Stop all players" onclick={() => void call(() => session.stopAll())}>STOP ALL</button>
    <button class="toggle" class:on={muted} title="Mute the master output" onclick={() => void mute()}>MUTE</button>
    <span class="ph" title="Not defined yet"></span>
    <span class="ph" title="Not defined yet"></span>
  </div>
</section>

<style>
  .instant-actions {
    display: flex;
    flex-direction: column;
    gap: 10px;
  }

  .btns {
    flex: 1;
    display: grid;
    grid-template-columns: 1fr;
    grid-template-rows: repeat(6, 1fr);
    gap: 8px;
  }

  .btns > * {
    min-height: 0;
    font-size: 12px;
    padding: 2px 4px;
  }

  .toggle {
    background: #22303d;
  }

  .toggle.on {
    background: #1d3a2a;
    border-color: var(--ok);
    color: #9ff0c3;
  }

  .danger {
    background: #3a1f22;
    border-color: var(--err);
    color: #f3b5b1;
  }

  .ph {
    border: 1px dashed var(--border);
    border-radius: 8px;
    background: transparent;
  }
</style>
