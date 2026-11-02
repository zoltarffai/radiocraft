<script lang="ts">
  import { session } from "./session.svelte";
  import type { CartButtonSpec, SongRow } from "./protocol";

  /**
   * Wall configuration, the counterpart of Plugin_InstantPlayers' config
   * form: walls are plain `carts` rows, buttons are `carts_list` rows laid
   * out by slot number inside a Columns x Rows grid. Both the cart and the
   * instant panel drive the wall selected here.
   */

  type Cell = { songId: number; title: string; color: string };

  let loading = $state(false);
  let dirty = $state(false);
  /** Local copy of the grid config so Apply is an explicit step. */
  let cfg = $state({ columns: 4, rows: 6, fade: 1, autoRoll: false });
  /** slot -> assignment; only filled slots are saved. */
  let layout = $state<Record<number, Cell>>({});
  /** Slot whose song picker is open (0 = closed). */
  let picking = $state(0);
  let query = $state("");
  let results = $state<SongRow[]>([]);
  let searching = $state(false);

  const carts = $derived(session.carts);
  const selected = $derived(session.selectedCart);
  const ready = $derived(session.conn === "open" && session.dbConnected);
  const capacity = $derived(cfg.columns * cfg.rows);
  const assigned = $derived(Object.keys(layout).length);
  /** Slot of the button currently rolled on either zone for this wall. */
  const liveSlot = $derived.by(() => {
    const now =
      session.cartNow?.cart_id === selected
        ? session.cartNow
        : session.instantNow?.cart_id === selected
          ? session.instantNow
          : null;
    if (!now) return 0;
    return session.cartButtons.find((b) => b.ID === now.button_id)?.swButton ?? 0;
  });
  let loadedFor = $state(0);

  $effect(() => {
    if (ready && loadedFor !== session.dbConnects) {
      loadedFor = session.dbConnects;
      void refresh();
    }
  });

  // Re-seed the draft layout whenever the panel's wall buttons are reloaded.
  $effect(() => {
    const rows = [...session.cartButtons].sort((a, b) => a.swButton - b.swButton);
    const next: Record<number, Cell> = {};
    for (const r of rows) {
      if (r.swID > 0) next[r.swButton] = { songId: r.swID, title: r.title ?? "", color: r.color || "" };
    }
    layout = next;
    dirty = false;
    picking = 0;
    query = "";
    results = [];
  });

  async function refresh() {
    try {
      await session.loadCartwallConfig();
      if (session.cartwallConfig) cfg = { ...session.cartwallConfig };
      await session.loadCarts();
      if (selected !== null && !carts.some((c) => c.ID === selected)) {
        session.selectedCart = null;
        session.cartButtons = [];
      }
      if (selected === null && carts.length > 0) await session.loadCartButtons(carts[0].ID);
    } catch (e) {
      session.lastError = String(e);
    }
  }

  async function call(fn: () => Promise<unknown>) {
    loading = true;
    try {
      await fn();
    } catch (e) {
      session.lastError = String(e);
    } finally {
      loading = false;
    }
  }

  async function selectWall(id: number) {
    picking = 0;
    await call(() => session.loadCartButtons(id));
  }

  function createWall() {
    const name = window.prompt("New cart wall name:");
    if (!name?.trim()) return;
    void call(async () => {
      const id = await session.createCartWall(name.trim());
      await session.loadCarts();
      await session.loadCartButtons(id);
    });
  }

  function renameWall() {
    if (selected === null) return;
    const current = carts.find((c) => c.ID === selected)?.name ?? "";
    const name = window.prompt("Rename cart wall:", current);
    if (!name?.trim()) return;
    void call(async () => {
      await session.renameCartWall(selected as number, name.trim());
      await session.loadCarts();
    });
  }

  function deleteWall() {
    if (selected === null) return;
    if (!window.confirm("Delete this cart wall and all of its buttons?")) return;
    void call(async () => {
      await session.deleteCartWall(selected as number);
      session.selectedCart = null;
      session.cartButtons = [];
      await session.loadCarts();
      if (carts.length > 0) await session.loadCartButtons(carts[0].ID);
    });
  }

  function applyConfig() {
    void call(async () => {
      await session.setCartwallConfig({
        columns: cfg.columns,
        rows: cfg.rows,
        fade: cfg.fade,
        autoRoll: cfg.autoRoll,
      });
      // Buttons beyond the new grid would be rejected on save; reload so the
      // editor shows exactly what the wall can hold.
      if (selected !== null) await session.loadCartButtons(selected);
    });
  }

  function saveLayout() {
    if (selected === null) return;
    const specs: CartButtonSpec[] = Object.entries(layout)
      .map(([slot, cell]) => ({ slot: Number(slot), songId: cell.songId, color: cell.color }))
      .filter((s) => s.slot >= 1 && s.slot <= capacity && s.songId > 0)
      .sort((a, b) => a.slot - b.slot);
    void call(async () => {
      await session.saveCartButtons(selected as number, specs);
      await session.loadCartButtons(selected as number);
    });
  }

  function clearSlot(slot: number) {
    const next = { ...layout };
    delete next[slot];
    layout = next;
    dirty = true;
    if (picking === slot) picking = 0;
  }

  function openPicker(slot: number) {
    picking = picking === slot ? 0 : slot;
    query = "";
    results = [];
  }

  function setColor(slot: number, color: string) {
    const cell = layout[slot];
    if (!cell) return;
    layout = { ...layout, [slot]: { ...cell, color } };
    dirty = true;
  }

  async function searchSongs() {
    if (!query.trim()) {
      results = [];
      return;
    }
    searching = true;
    try {
      const r = await session.cmd<{ songs: SongRow[] }>("search_songs", { needle: query.trim(), limit: 25 });
      results = r.songs ?? [];
    } catch (e) {
      session.lastError = String(e);
    } finally {
      searching = false;
    }
  }

  function assign(song: SongRow) {
    const cell = layout[picking];
    layout = {
      ...layout,
      [picking]: {
        songId: song.ID,
        title: `${song.artist ? `${song.artist} – ` : ""}${song.title}`,
        color: cell?.color ?? "",
      },
    };
    dirty = true;
    picking = 0;
    query = "";
    results = [];
  }

  const slots = $derived.by(() => Array.from({ length: capacity }, (_, i) => i + 1));
</script>

<section class="panel wallcfg">
  <h2>Cart wall setup</h2>

  <div class="row">
    <select
      value={selected ?? ""}
      onchange={(e) => {
        const v = Number((e.currentTarget as HTMLSelectElement).value);
        if (v > 0) void selectWall(v);
      }}
    >
      <option value="" disabled>Choose a cart wall…</option>
      {#each carts as c (c.ID)}
        <option value={c.ID}>{c.name}</option>
      {/each}
    </select>
    <button onclick={createWall}>New wall</button>
    <button disabled={selected === null} onclick={renameWall}>Rename</button>
    <button class="danger" disabled={selected === null} onclick={deleteWall}>Delete wall</button>
    <span class="count mono">{assigned} of {capacity} slots</span>
  </div>

  <div class="row cfg">
    <label>
      Columns
      <input type="number" min="1" max="20" step="1" bind:value={cfg.columns} />
    </label>
    <label>
      Rows
      <input type="number" min="1" max="20" step="1" bind:value={cfg.rows} />
    </label>
    <label>
      Stop fade (s)
      <input type="number" min="0" max="30" step="1" bind:value={cfg.fade} />
    </label>
    <label class="check">
      <input type="checkbox" bind:checked={cfg.autoRoll} />
      Auto-roll
    </label>
    <button
      class="primary"
      disabled={loading || !session.cartwallConfig || (cfg.columns === session.cartwallConfig.columns &&
        cfg.rows === session.cartwallConfig.rows &&
        cfg.fade === session.cartwallConfig.fade &&
        cfg.autoRoll === session.cartwallConfig.autoRoll)}
      onclick={applyConfig}
    >
      Apply grid
    </button>
  </div>

  {#if selected === null}
    <p class="dim">{ready ? "No cart wall selected — press New wall." : "Connect the database first."}</p>
  {:else}
    <div class="slots" style:grid-template-columns="repeat({cfg.columns}, minmax(0, 1fr))">
      {#each slots as slot (slot)}
        {@const cell = layout[slot]}
        <div class="slot" class:filled={!!cell} class:live={liveSlot === slot}>
          <div class="slothdr">
            <span class="mono num">{slot}</span>
            {#if cell}
              <input
                type="color"
                value={cell.color || "#3b4252"}
                title="Button colour"
                onchange={(e) => setColor(slot, (e.currentTarget as HTMLInputElement).value)}
              />
            {/if}
          </div>
          <div class="slotbody" title={cell?.title ?? ""}>
            {cell ? cell.title || `song #${cell.songId}` : "empty"}
          </div>
          <div class="slotact">
            <button class="tiny" onclick={() => openPicker(slot)}>{cell ? "Change" : "Assign"}</button>
            {#if cell}
              <button class="tiny danger" onclick={() => clearSlot(slot)}>✕</button>
            {/if}
          </div>
        </div>
      {/each}
    </div>

    {#if picking > 0}
      <div class="picker">
        <div class="row">
          <input
            placeholder="Search the library by artist or title…"
            bind:value={query}
            oninput={() => void searchSongs()}
          />
          <span class="count mono">{searching ? "…" : `${results.length} match${results.length === 1 ? "" : "es"}`}</span>
        </div>
        {#if results.length > 0}
          <ul>
            {#each results as s (s.ID)}
              <li>
                <button class="pickrow" onclick={() => assign(s)}>
                  <span class="mono sid">{s.ID}</span>
                  <span>{s.artist || "—"}</span>
                  <span>{s.title}</span>
                  <span class="mono dim">{s.duration}</span>
                </button>
              </li>
            {/each}
          </ul>
        {:else}
          <p class="dim">{query.trim() ? "No library rows matched." : "Type to search the library."}</p>
        {/if}
      </div>
    {/if}

    <div class="row">
      <button class="primary" disabled={loading || !dirty} onclick={saveLayout}>Save buttons</button>
      {#if dirty}
        <button
          disabled={loading}
          onclick={() => selected !== null && void session.loadCartButtons(selected)}
        >
          Discard changes
        </button>
        <span class="warntext mono">unsaved layout</span>
      {/if}
    </div>
  {/if}
</section>

<style>
  .row {
    display: flex;
    gap: 8px;
    align-items: center;
    flex-wrap: wrap;
    margin: 6px 0;
  }

  .cfg label {
    display: flex;
    align-items: center;
    gap: 6px;
    color: var(--fg-dim);
    font-size: 13px;
  }

  .cfg input[type="number"] {
    width: 62px;
  }

  .count {
    margin-left: auto;
    color: var(--fg-dim);
    font-size: 12px;
  }

  .dim {
    color: var(--fg-dim);
    font-size: 13px;
  }

  .warntext {
    color: var(--warn);
    font-size: 12px;
  }

  .slots {
    display: grid;
    gap: 8px;
    margin: 10px 0;
  }

  .slot {
    border: 1px solid var(--border);
    border-radius: 8px;
    padding: 6px;
    background: var(--panel-2);
    display: flex;
    flex-direction: column;
    gap: 4px;
    min-height: 76px;
  }

  .slot.filled {
    background: color-mix(in srgb, var(--bg) 70%, var(--panel-2));
  }

  .slot.live {
    outline: 2px solid var(--accent);
  }

  .slothdr {
    display: flex;
    align-items: center;
    justify-content: space-between;
  }

  .num {
    color: var(--fg-dim);
    font-size: 12px;
  }

  .slotbody {
    font-size: 12px;
    line-height: 1.25;
    overflow: hidden;
    display: -webkit-box;
    -webkit-line-clamp: 2;
    line-clamp: 2;
    -webkit-box-orient: vertical;
  }

  .slotact {
    display: flex;
    gap: 4px;
    margin-top: auto;
  }

  button.tiny {
    padding: 2px 7px;
    font-size: 12px;
  }

  input[type="color"] {
    width: 22px;
    height: 18px;
    padding: 0;
    border: 1px solid var(--border);
    background: none;
  }

  .picker {
    border: 1px solid var(--border);
    border-radius: 8px;
    padding: 8px;
    background: var(--panel-2);
  }

  .picker ul {
    list-style: none;
    margin: 0;
    padding: 0;
    max-height: 260px;
    overflow: auto;
  }

  .pickrow {
    display: grid;
    grid-template-columns: 4ch 1fr 1.4fr 8ch;
    gap: 8px;
    width: 100%;
    text-align: left;
    padding: 4px 6px;
    background: none;
    border: none;
    border-bottom: 1px solid var(--border);
    cursor: pointer;
    font-size: 13px;
  }

  .pickrow:hover {
    background: color-mix(in srgb, var(--accent) 18%, transparent);
  }

  .sid {
    color: var(--fg-dim);
  }
</style>
