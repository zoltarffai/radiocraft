<script lang="ts">
  import { session } from "./session.svelte";

  /** Mirrors `plugin_host::PluginEntry` from `plugin_list`. */
  interface PluginInfo {
    id: number;
    name: string;
    version: string;
    description: string;
    path: string | null;
    actions: string[];
  }

  let plugins = $state<PluginInfo[]>([]);
  let busy = $state(false);
  let path = $state("");
  let action = $state("");
  let argsRaw = $state("");

  const ready = $derived(session.conn === "open");
  const allActions = $derived([...new Set(plugins.flatMap((p) => p.actions))]);

  // Refresh once the socket opens, and whenever we load/unload below.
  let loadedFor = $derived(ready && session.conn);
  $effect(() => {
    if (loadedFor) void refresh().catch((e) => (session.lastError = String(e)));
  });

  async function refresh() {
    const res = await session.cmd<{ plugins: PluginInfo[] }>("plugin_list");
    plugins = res.plugins;
  }

  async function call(fn: () => Promise<void>) {
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

  function load() {
    const p = path.trim();
    if (p === "") return;
    void call(async () => {
      await session.cmd("plugin_load", { path: p });
      path = "";
      await refresh();
    });
  }

  function unload(id: number) {
    void call(async () => {
      await session.cmd("plugin_unload", { id });
      await refresh();
    });
  }

  /** `Name (Arg1, Arg2)` display form -> plain action name. */
  function actionName(display: string): string {
    return display.replace(/\s*\([^)]*\)\s*$/, "");
  }

  function invoke() {
    const name = action.trim() === "" ? "" : actionName(action);
    if (name === "") return;
    const args = argsRaw
      .split(",")
      .map((s) => s.trim())
      .filter((s) => s !== "");
    void call(async () => {
      await session.cmd("plugin_invoke", { action: name, args });
    });
  }
</script>

<section class="panel plugins">
  <div class="head">
    <h2>Plugins</h2>
    <span class="mono dim">{plugins.length} loaded</span>
    <span class="spacer"></span>
    <button onclick={() => void call(refresh)} disabled={!ready || busy}>Refresh</button>
  </div>

  <table>
    <thead>
      <tr>
        <th>Name</th>
        <th>Version</th>
        <th>Description</th>
        <th>Actions</th>
        <th></th>
      </tr>
    </thead>
    <tbody>
      {#if plugins.length === 0}
        <tr>
          <td colspan="5" class="empty dim">No plugins loaded. Drop files in the server's Plugins folder or load a path below.</td>
        </tr>
      {:else}
        {#each plugins as p (p.id)}
          <tr>
            <td>{p.name}</td>
            <td class="mono">{p.version}</td>
            <td class="dim">{p.description}</td>
            <td class="mono actions">{p.actions.join(", ")}</td>
            <td><button onclick={() => unload(p.id)} disabled={!ready || busy}>Unload</button></td>
          </tr>
        {/each}
      {/if}
    </tbody>
  </table>

  <div class="row">
    <input class="mono" placeholder="plugin path (.so, relative to Plugins/)" bind:value={path} onkeydown={(e) => e.key === "Enter" && load()} disabled={!ready} />
    <button onclick={load} disabled={!ready || busy}>Load</button>
  </div>

  <div class="row">
    <input list="rc-plugin-actions" placeholder="action" bind:value={action} disabled={!ready} />
    <datalist id="rc-plugin-actions">
      {#each allActions as a (a)}
        <option value={a}></option>
      {/each}
    </datalist>
    <input placeholder="args (comma separated)" bind:value={argsRaw} onkeydown={(e) => e.key === "Enter" && invoke()} disabled={!ready} />
    <button onclick={invoke} disabled={!ready || busy || action.trim() === ""}>Invoke</button>
  </div>

  <div class="events">
    <div class="dim">Recent plugin events</div>
    {#if session.pluginEvents.length === 0}
      <div class="dim empty">Nothing yet.</div>
    {:else}
      {#each session.pluginEvents.slice(-12).reverse() as ev (ev.at + ev.plugin + ev.text)}
        <div class="event mono">
          <span class="dim">{new Date(ev.at).toLocaleTimeString()}</span>
          <span>{ev.plugin}</span>
          <span>{ev.text}</span>
        </div>
      {/each}
    {/if}
  </div>
</section>

<style>
  .head {
    display: flex;
    align-items: center;
    gap: 10px;
    margin-bottom: 8px;
  }

  h2 {
    margin: 0;
    font-size: 14px;
  }

  .spacer {
    flex: 1;
  }

  .dim {
    color: var(--fg-dim);
    font-size: 12px;
  }

  table {
    width: 100%;
    border-collapse: collapse;
    font-size: 12px;
  }

  th,
  td {
    text-align: left;
    padding: 4px 8px;
    border-bottom: 1px solid var(--border);
    vertical-align: top;
  }

  th {
    color: var(--fg-dim);
    font-weight: 500;
  }

  td.actions {
    word-break: break-word;
  }

  .row {
    display: flex;
    gap: 8px;
    margin-top: 8px;
  }

  .row input {
    flex: 1;
    min-width: 0;
  }

  .events {
    margin-top: 10px;
    display: flex;
    flex-direction: column;
    gap: 2px;
  }

  .event {
    display: flex;
    gap: 10px;
    font-size: 12px;
  }

  .empty {
    padding: 4px 0;
  }
</style>
