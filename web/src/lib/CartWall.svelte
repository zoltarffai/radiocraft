<script lang="ts">
  import { session } from "./session.svelte";
  import type { CartButtonRow } from "./protocol";

  /** Transport the wall drives: `cart` is the core cartwall, `instant` is
   *  the instant/cart wall (`Plugin_InstantPlayers`) on its own zone. */
  let { zone = "cart" }: { zone?: "cart" | "instant" } = $props();

  let busy = $state(false);

  const heading = $derived(zone === "instant" ? "Instant wall" : "Carts");
  const isInstant = $derived(zone === "instant");
  const carts = $derived(session.carts);
  const buttons = $derived(session.cartButtons);
  const selectedCart = $derived(session.selectedCart);
  const active = $derived(isInstant ? session.instantNow : session.cartNow);
  const playing = $derived(session.zoneState(zone)?.playing ?? false);

  // RadioDJ lays buttons out by their `swButton` slot number.
  const grid = $derived.by(() => {
    const sorted = [...buttons].sort((a, b) => a.swButton - b.swButton);
    const max = sorted.reduce((m, b) => Math.max(m, b.swButton), 0);
    const cells: (CartButtonRow | null)[] = Array.from({ length: max }, () => null);
    for (const b of sorted) cells[b.swButton - 1] = b;
    return cells;
  });

  async function selectCart(id: number) {
    try {
      await session.loadCartButtons(id);
    } catch (e) {
      session.lastError = String(e);
    }
  }

  async function press(button: CartButtonRow) {
    if (busy) return;
    busy = true;
    try {
      await session.cmd(isInstant ? "play_instant" : "play_cart", { buttonId: button.ID });
    } catch (e) {
      session.lastError = String(e);
    } finally {
      busy = false;
    }
  }

  // Auto-select the first wall once the cart list arrives.
  $effect(() => {
    if (selectedCart === null && carts.length > 0) void selectCart(carts[0].ID);
  });
</script>

<section class="panel carts">
  <h2>{heading}</h2>

  <div class="row">
    <label class="csel">
      Cart
      <select
        value={selectedCart ?? ""}
        onchange={(e) => {
          const v = Number(e.currentTarget.value);
          if (Number.isFinite(v)) void selectCart(v);
        }}
      >
        {#each carts as c (c.ID)}
          <option value={c.ID}>{c.name}</option>
        {/each}
      </select>
    </label>
    {#if active && playing}
      <span class="live" title={active.title}>▶ {active.title}</span>
    {/if}
  </div>

  {#if grid.length === 0}
    <p class="empty">{carts.length === 0 ? "No carts in the database." : "This cart has no buttons."}</p>
  {:else}
    <div class="wall">
      {#each grid as cell, i (i)}
        {#if cell}
          {@const isLive = active?.button_id === cell.ID && playing}
          <button
            class="cartbtn"
            class:live={isLive}
            style:--btn-color={cell.color || "#3b4252"}
            title={cell.title ?? "(unassigned)"}
            disabled={cell.swID === 0}
            onclick={() => void press(cell)}
          >
            <span class="num">{cell.swButton}</span>
            <span class="lbl">{cell.title ?? ""}</span>
          </button>
        {:else}
          <span class="slot"></span>
        {/if}
      {/each}
    </div>
  {/if}
</section>

<style>
  .row {
    display: flex;
    align-items: center;
    gap: 10px;
    flex-wrap: wrap;
  }

  .csel {
    display: flex;
    align-items: center;
    gap: 6px;
    color: var(--fg-dim);
  }

  .live {
    color: var(--accent);
    font-size: 14px;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
    max-width: 100%;
  }

  .empty {
    color: var(--fg-dim);
  }

  .wall {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(110px, 1fr));
    gap: 8px;
    margin-top: 10px;
  }

  .slot {
    aspect-ratio: 1.4;
  }

  .cartbtn {
    aspect-ratio: 1.4;
    display: flex;
    flex-direction: column;
    align-items: flex-start;
    justify-content: space-between;
    padding: 6px 8px;
    background: color-mix(in srgb, var(--btn-color) 55%, var(--bg));
    border: 1px solid color-mix(in srgb, var(--btn-color) 80%, white 20%);
    border-radius: 8px;
    color: #fff;
    text-align: left;
    cursor: pointer;
  }

  .cartbtn:hover:not(:disabled) {
    background: color-mix(in srgb, var(--btn-color) 75%, var(--bg));
  }

  .cartbtn:disabled {
    opacity: 0.35;
    cursor: default;
  }

  .cartbtn.live {
    outline: 2px solid var(--accent);
    box-shadow: 0 0 12px color-mix(in srgb, var(--accent) 60%, transparent);
  }

  .num {
    font-size: 12px;
    opacity: 0.75;
  }

  .lbl {
    font-size: 13px;
    font-weight: 600;
    line-height: 1.2;
    display: -webkit-box;
    -webkit-line-clamp: 2;
    line-clamp: 2;
    -webkit-box-orient: vertical;
    overflow: hidden;
  }
</style>
