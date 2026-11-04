<script lang="ts">
  import { onMount, type Component } from "svelte";
  import { session } from "./session.svelte";
  import Database from "./Database.svelte";
  import SettingsPanel from "./SettingsPanel.svelte";
  import PluginPanel from "./PluginPanel.svelte";
  import CartwallEditor from "./CartwallEditor.svelte";
  import ZoneMixer from "./ZoneMixer.svelte";
  import AutoDJPanel from "./AutoDJPanel.svelte";

  /** Section 7 — function selection + status strip along the bottom. The
   *  function buttons open RadioDJ-style tool windows as modal panels. */
  type Fn = { id: string; label: string; panel: Component };

  const functions: Fn[] = [
    { id: "db", label: "Database", panel: Database },
    { id: "options", label: "Options", panel: SettingsPanel },
    { id: "cartwall", label: "Cart wall editor", panel: CartwallEditor },
    { id: "mixer", label: "Mixer", panel: ZoneMixer },
    { id: "autodj", label: "AutoDJ setup", panel: AutoDJPanel },
    { id: "plugins", label: "Plugins", panel: PluginPanel },
  ];

  let openFn = $state<Fn | null>(null);
  const OpenPanel = $derived(openFn?.panel);

  let clock = $state("");
  onMount(() => {
    const tick = () => (clock = new Date().toLocaleTimeString());
    tick();
    const t = setInterval(tick, 1000);
    return () => clearInterval(t);
  });

  const status = $derived(session.status);
  const conn = $derived(session.conn);
  const db = $derived(session.dbConnected);
  const ready = $derived(session.ready);
</script>

<section class="panel function-bar">
  <div class="fns">
    {#each functions as f (f.id)}
      <button onclick={() => (openFn = f)}>{f.label}</button>
    {/each}
  </div>

  <span class="spacer"></span>

  <div class="statusline">
    <span class="status mono" data-level={status.level}>{status.message}</span>
    <span class="chip" data-state={conn}>{conn}</span>
    <span class="chip" data-on={db}>{db ? "db ok" : "db off"}</span>
    <span class="mono ver">{ready ? `${ready.server} ${ready.version}` : ""}</span>
    <span class="mono clock">{clock}</span>
  </div>
</section>

{#if openFn}
  <!-- svelte-ignore a11y_click_events_have_key_events -->
  <div class="backdrop" role="dialog" aria-modal="true" aria-label={openFn.label} tabindex={-1}
       onkeydown={(e) => e.key === "Escape" && (openFn = null)}
       onclick={(e) => e.target === e.currentTarget && (openFn = null)}>
    <div class="window">
      <div class="winhead">
        <span class="wintitle">{openFn.label}</span>
        <span class="spacer"></span>
        <button onclick={() => (openFn = null)} aria-label="Close">✕</button>
      </div>
      <div class="winbody">
        {#if OpenPanel}<OpenPanel />{/if}
      </div>
    </div>
  </div>
{/if}

<style>
  .function-bar {
    flex-direction: row;
    align-items: center;
    gap: 10px;
    flex-wrap: wrap;
  }

  .fns {
    display: flex;
    gap: 8px;
    flex-wrap: wrap;
  }

  .fns button {
    font-weight: 600;
  }

  .spacer {
    flex: 1;
  }

  .statusline {
    display: flex;
    align-items: center;
    gap: 10px;
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

  .chip {
    font-size: 11px;
    padding: 2px 10px;
    border-radius: 999px;
    border: 1px solid var(--border);
    color: var(--fg-dim);
  }

  .chip[data-state="open"] {
    color: var(--ok);
    border-color: var(--ok);
  }

  .chip[data-state="connecting"] {
    color: var(--warn);
  }

  .chip[data-state="closed"] {
    color: var(--err);
    border-color: var(--err);
  }

  .chip[data-on="true"] {
    color: var(--ok);
    border-color: var(--ok);
  }

  .ver {
    font-size: 11px;
    color: var(--fg-dim);
  }

  .clock {
    font-size: 15px;
    color: var(--fg);
    min-width: 8ch;
    text-align: right;
  }

  .backdrop {
    position: fixed;
    inset: 0;
    background: #000000a0;
    display: flex;
    align-items: center;
    justify-content: center;
    padding: 24px;
    z-index: 50;
  }

  .window {
    background: var(--panel);
    border: 1px solid var(--border);
    border-radius: 10px;
    max-width: min(1000px, 95vw);
    max-height: 90vh;
    display: flex;
    flex-direction: column;
    box-shadow: 0 12px 40px #000000aa;
  }

  .winhead {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 8px 12px;
    border-bottom: 1px solid var(--border);
    background: var(--panel-2);
    border-radius: 10px 10px 0 0;
  }

  .wintitle {
    font-weight: 700;
    font-size: 13px;
  }

  .winbody {
    overflow: auto;
    padding: 12px;
  }

  .winbody > :global(section) {
    border: none;
    background: transparent;
    padding: 0;
    border-radius: 0;
  }
</style>
