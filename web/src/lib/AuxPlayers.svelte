<script lang="ts">
  import { session } from "./session.svelte";
  import type { AuxPlayerInfo } from "./protocol";

  let busy = $state(false);
  let loadText = $state<Record<number, string>>({});

  const cfg = $derived(session.auxConfig);
  const count = $derived(cfg?.playersCount ?? 0);

  // Player numbers the config exposes (RadioDJ's "Player N", 1-based).
  const playerNums = $derived(Array.from({ length: count }, (_, i) => i + 1));

  const loaded = $derived(new Map(session.auxPlayers.map((p) => [p.player, p])));

  async function applyPlayersCount(v: number) {
    if (busy || v === cfg?.playersCount) return;
    busy = true;
    try {
      await session.setAuxConfig({ playersCount: v });
    } catch (e) {
      session.lastError = String(e);
    } finally {
      busy = false;
    }
  }

  async function applyStopFade(v: number) {
    if (busy) return;
    busy = true;
    try {
      await session.setAuxConfig({ stopFadeMs: v });
    } catch (e) {
      session.lastError = String(e);
    } finally {
      busy = false;
    }
  }

  /** Load accepts a numeric songs.ID or a raw file path / stream URL. */
  async function loadTrack(player: number) {
    const raw = (loadText[player] ?? "").trim();
    if (!raw || busy) return;
    busy = true;
    try {
      const asId = Number(raw);
      const track =
        Number.isInteger(asId) && asId > 0 && !raw.includes("/")
          ? { songId: asId }
          : { path: raw };
      await session.auxLoad(player, track);
      loadText[player] = "";
    } catch (e) {
      session.lastError = String(e);
    } finally {
      busy = false;
    }
  }

  async function run(fn: () => Promise<unknown>) {
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

  function fmt(secs: number | undefined): string {
    if (secs === undefined) return "0:00";
    const s = Math.max(0, Math.round(secs));
    return `${Math.floor(s / 60)}:${String(s % 60).padStart(2, "0")}`;
  }

  // Pull the config once the session is up.
  $effect(() => {
    if (session.conn === "open" && session.auxConfig === null) {
      void session.loadAuxConfig().catch((e) => (session.lastError = String(e)));
    }
  });
</script>

<section class="panel auxplayers">
  <h2>Aux players</h2>

  <div class="row cfg">
    <label class="num">
      Players
      <input
        type="number"
        min="1"
        max="15"
        value={cfg?.playersCount ?? 3}
        onchange={(e) => void applyPlayersCount(Number(e.currentTarget.value))}
      />
    </label>
    <label class="num">
      Stop fade (ms)
      <input
        type="number"
        min="0"
        max="10000"
        step="100"
        value={cfg?.stopFadeMs ?? 0}
        onchange={(e) => void applyStopFade(Number(e.currentTarget.value))}
      />
    </label>
    <span class="hint">Extra configurable players (`Plugin_AuxPlayers`)</span>
  </div>

  {#if count === 0}
    <p class="empty">Aux players are not configured.</p>
  {:else}
    {#each playerNums as n (n)}
      {@const info = loaded.get(n)}
      {@const zone = session.zoneState(`aux${n}`)}
      {@const playing = zone?.playing ?? false}
      {@const pos = zone?.position_secs ?? 0}
      {@const dur = info?.duration_secs ?? 0}
      <div class="player" class:live={playing}>
        <div class="row head">
          <span class="name">Player {n}</span>
          <span class="title" title={info ? `${info.artist} ${info.title}` : ""}>
            {info ? `${info.artist} — ${info.title}` : "(no track loaded)"}
          </span>
          {#if info?.loop_enabled}
            <span class="badge loop">loop</span>
          {/if}
          <span class="mono time">{fmt(pos)} / {fmt(dur)}</span>
        </div>

        <div class="row transport">
          <button
            class="play"
            class:live={playing}
            title={playing ? "Stop (with fade)" : "Play from start"}
            onclick={() => void run(() => session.auxPlay(n))}
          >
            {playing ? "■" : "▶"}
          </button>
          <button
            class="loopbtn"
            class:live={info?.loop_enabled ?? false}
            title="Loop region (needs the track's loop cue)"
            disabled={info === undefined}
            onclick={() => void run(() => session.auxLoop(n, !(info?.loop_enabled ?? false)))}
          >
            ⟳
          </button>
          <button title="Fade-stop, keep track" disabled={!playing} onclick={() => void run(() => session.auxStop(n))}>
            Stop
          </button>
          <button title="Unload" disabled={info === undefined} onclick={() => void run(() => session.auxClear(n))}>
            Clear
          </button>
          {#if dur > 0}
            <input
              class="seek"
              type="range"
              min="0"
              max={dur}
              step="0.1"
              value={pos}
              onchange={(e) => void run(() => session.auxSeek(n, Number(e.currentTarget.value)))}
            />
          {/if}
        </div>

        <div class="row load">
          <input
            type="text"
            placeholder="song ID, file path or stream URL"
            value={loadText[n] ?? ""}
            oninput={(e) => (loadText[n] = e.currentTarget.value)}
            onkeydown={(e) => {
              if (e.key === "Enter") void loadTrack(n);
            }}
          />
          <button onclick={() => void loadTrack(n)}>Load</button>
        </div>
      </div>
    {/each}
  {/if}
</section>

<style>
  .row {
    display: flex;
    align-items: center;
    gap: 8px;
    flex-wrap: wrap;
  }

  .cfg {
    margin-bottom: 10px;
  }

  .num {
    display: flex;
    align-items: center;
    gap: 6px;
    color: var(--fg-dim);
  }

  .num input {
    width: 70px;
  }

  .hint {
    color: var(--fg-dim);
    font-size: 12px;
  }

  .empty {
    color: var(--fg-dim);
  }

  .player {
    border: 1px solid var(--border);
    border-radius: 8px;
    padding: 8px 10px;
    margin-top: 8px;
    display: flex;
    flex-direction: column;
    gap: 6px;
  }

  .player.live {
    border-color: color-mix(in srgb, var(--accent) 60%, var(--border));
  }

  .head .name {
    font-weight: 600;
    white-space: nowrap;
  }

  .head .title {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    color: var(--fg-dim);
  }

  .time {
    font-size: 12px;
    color: var(--fg-dim);
    white-space: nowrap;
  }

  .badge.loop {
    font-size: 11px;
    color: var(--accent);
    border: 1px solid var(--accent);
    border-radius: 4px;
    padding: 0 4px;
  }

  .play {
    min-width: 34px;
  }

  .play.live {
    color: var(--accent);
  }

  .loopbtn.live {
    color: var(--accent);
    border-color: var(--accent);
  }

  .seek {
    flex: 1;
    min-width: 120px;
  }

  .load input {
    flex: 1;
    min-width: 160px;
  }
</style>
