<script lang="ts">
  import { session } from "./session.svelte";
  import { ZONES, dbFromWire, type ZoneName } from "./protocol";
  import Meter from "./Meter.svelte";

  const master = $derived(session.master);
  const masterDb = $derived(session.masterDb);

  const FLOOR = -40;

  function label(db: number): string {
    return Number.isFinite(db) ? `${db.toFixed(1)} dB` : "-inf";
  }

  async function call(fn: () => Promise<unknown>) {
    try {
      await fn();
    } catch (e) {
      session.lastError = String(e);
    }
  }
</script>

<section class="panel mixer">
  <h2>Mix</h2>

  <div class="master">
    <span class="tag">Master</span>
    <input
      type="range"
      min={FLOOR}
      max={0}
      step={0.5}
      value={Number.isFinite(masterDb) ? masterDb : FLOOR}
      oninput={(e) => void call(() => session.setMasterDb(Number(e.currentTarget.value)))}
    />
    <span class="mono val">{label(masterDb)}</span>
  </div>

  <Meter db={master.level_db ?? -Infinity} peakDb={dbFromWire(master.peak_hold_db)} height={12} />

  <div class="zones">
    {#each ZONES as zone}
      {@const mix = session.zoneMix(zone)}
      {@const zs = session.zoneState(zone)}
      <div class="zcol" class:live={zs?.playing}>
        <div class="zname">{zone}</div>
        <input
          type="range"
          class="vfader"
          min={FLOOR}
          max={0}
          step={0.5}
          value={mix ? dbFromWire(mix.fader_db) : 0}
          disabled={!mix}
          oninput={(e) => void call(() => session.setZoneFader(zone as ZoneName, Number(e.currentTarget.value)))}
        />
        <span class="mono zval">{mix ? label(dbFromWire(mix.fader_db)) : "—"}</span>
        <button
          class="zmute"
          class:on={mix?.muted}
          disabled={!mix}
          onclick={() => void call(() => session.setZoneMuted(zone as ZoneName, !mix?.muted))}
          >M</button
        >
      </div>
    {/each}
  </div>
</section>

<style>
  .master {
    display: flex;
    align-items: center;
    gap: 8px;
  }

  .master input {
    flex: 1;
    accent-color: var(--accent);
  }

  .master input[type="range"] {
    border: none;
    padding: 0;
    background: none;
  }

  .tag,
  .zname,
  .zval {
    color: var(--fg-dim);
    font-size: 12px;
  }

  .val {
    min-width: 6em;
    text-align: right;
  }

  .zones {
    display: grid;
    grid-template-columns: repeat(4, 1fr);
    gap: 10px;
  }

  .zcol {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 6px;
    padding: 6px;
    border: 1px solid var(--border);
    border-radius: 8px;
  }

  .zcol.live {
    border-color: var(--accent-dim);
  }

  .zcol.live .zname {
    color: var(--accent);
  }

  .vfader {
    writing-mode: vertical-lr;
    direction: rtl;
    width: 22px;
    height: 90px;
    accent-color: var(--accent);
    border: none;
    padding: 0;
    background: none;
  }

  .zmute {
    width: 26px;
    padding: 3px 0;
    font-size: 12px;
  }

  .zmute.on {
    border-color: var(--err);
    color: #f3b5b1;
    background: #3a1f22;
  }
</style>
