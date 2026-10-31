<script lang="ts">
  import { session } from "./session.svelte";

  let busy = $state(false);
  let loadedFor = $state(-1);

  const ready = $derived(session.conn === "open" && session.dbConnected);
  const enabled = $derived(session.autodjEnabled);
  const config = $derived(session.autodjConfig);
  const plan = $derived(session.autodjPlan);

  // Pull the switch state once per database connection so the toggle
  // reflects the persisted AutoDJStart setting.
  $effect(() => {
    if (ready && loadedFor !== session.dbConnects) {
      loadedFor = session.dbConnects;
      void session.refreshAutodj().catch((e) => (session.lastError = String(e)));
    }
  });

  async function call<T>(fn: () => Promise<T>) {
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

  function toggle() {
    void call(() => session.setAutodj(!enabled));
  }

  function setKnob(key: "respect_play_limit" | "respect_repeat_intervals" | "respect_availability", value: boolean) {
    void call(() => session.setAutodj(enabled, { [key]: value }));
  }

  function setMaxCandidates(raw: string) {
    const v = raw.trim() === "" ? null : Number(raw);
    void call(() =>
      session.setAutodj(enabled, { max_candidates: v === null || !Number.isFinite(v) ? null : Math.trunc(v) }),
    );
  }
</script>

<section class="panel autodj">
  <div class="head">
    <h2>AutoDJ</h2>
    <button
      class="switch"
      class:on={enabled}
      role="switch"
      aria-checked={enabled}
      onclick={toggle}
      disabled={!ready || busy}
    >
      {enabled ? "Running" : "Off"}
    </button>
    <span class="spacer"></span>
    <button onclick={() => void call(() => session.autodjPlanNext())} disabled={!ready || busy}>
      Preview next
    </button>
    <button class="primary" onclick={() => void call(() => session.autodjPlayNext())} disabled={!ready || busy}>
      Play next
    </button>
  </div>

  {#if config}
    <div class="knobs">
      <label><input type="checkbox" checked={config.respect_play_limit} onchange={(e) => setKnob("respect_play_limit", e.currentTarget.checked)} /> Play limits</label>
      <label><input type="checkbox" checked={config.respect_repeat_intervals} onchange={(e) => setKnob("respect_repeat_intervals", e.currentTarget.checked)} /> Repeat intervals</label>
      <label><input type="checkbox" checked={config.respect_availability} onchange={(e) => setKnob("respect_availability", e.currentTarget.checked)} /> Availability</label>
      <label class="num">
        Max candidates
        <input
          class="mono"
          type="number"
          min="1"
          value={config.max_candidates ?? ""}
          placeholder="unlimited"
          onchange={(e) => setMaxCandidates(e.currentTarget.value)}
        />
      </label>
    </div>
  {/if}

  {#if plan}
    <div class="plan">
      <div class="planhead mono">
        {plan.rotationName ?? "(no rotation)"}
        {#if plan.pick}
          → {plan.pick.artist} – {plan.pick.title}
        {:else if plan.rejected.length > 0}
          → nothing playable
        {/if}
      </div>
      {#if plan.pick}
        {@const p = plan.pick}
        <ul>
          {#if p.sweeperBefore}<li class="sweeper">♪ sweeper before: {p.sweeperBefore.title}</li>{/if}
          <li class="pick">{p.artist} – {p.title}<span class="mono dim"> {p.duration.toFixed(1)}s</span></li>
          {#if p.voiceTrack}<li class="sweeper">🎙 voice track: {p.voiceTrack.title}</li>{/if}
          {#if p.sweeperWithin}<li class="sweeper">♪ sweeper at +{p.sweeperOffset.toFixed(0)}s: {p.sweeperWithin.title}</li>{/if}
        </ul>
      {/if}
      {#if plan.rejected.length > 0}
        <details>
          <summary>{plan.rejected.length} rejected</summary>
          <table class="rej">
            <tbody>
              {#each plan.rejected as r, i (i)}
                <tr><td class="mono">#{r.songId}</td><td>{r.reason}</td></tr>
              {/each}
            </tbody>
          </table>
        </details>
      {/if}
    </div>
  {:else}
    <p class="empty">{ready ? "Preview the next slot or let AutoDJ fill the program." : "Connect the database first."}</p>
  {/if}
</section>

<style>
  .head {
    display: flex;
    align-items: center;
    gap: 10px;
    flex-wrap: wrap;
  }

  .head h2 {
    margin: 0;
  }

  .spacer {
    flex: 1;
  }

  .switch {
    padding: 4px 14px;
    border-radius: 999px;
    border: 1px solid var(--border);
    background: var(--panel-2);
    color: var(--fg-dim);
    cursor: pointer;
  }

  .switch.on {
    background: color-mix(in srgb, var(--accent) 30%, var(--bg));
    border-color: var(--accent);
    color: var(--accent);
  }

  .knobs {
    display: flex;
    align-items: center;
    gap: 14px;
    flex-wrap: wrap;
    margin-top: 10px;
    color: var(--fg-dim);
    font-size: 13px;
  }

  .knobs label {
    display: flex;
    align-items: center;
    gap: 5px;
  }

  .knobs input[type="number"] {
    width: 80px;
  }

  .plan {
    margin-top: 10px;
  }

  .planhead {
    color: var(--fg-dim);
    font-size: 13px;
  }

  .plan ul {
    list-style: none;
    margin: 6px 0 0;
    padding: 0;
  }

  .plan li {
    padding: 3px 0;
  }

  .plan li.pick {
    font-weight: 600;
  }

  .plan li.sweeper {
    color: var(--fg-dim);
    font-size: 13px;
    padding-left: 18px;
  }

  .dim {
    color: var(--fg-dim);
    font-weight: 400;
    font-size: 12px;
  }

  details {
    margin-top: 8px;
    color: var(--fg-dim);
  }

  .rej td {
    padding: 2px 12px 2px 0;
    font-size: 12px;
  }

  .empty {
    color: var(--fg-dim);
  }
</style>
