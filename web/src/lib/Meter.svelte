<script lang="ts">
  interface Props {
    /** current dB value (or -inf) */
    db: number;
    /** peak hold dB to tick-mark, `null`/omitted for no tick */
    peakDb?: number | null;
    height?: number;
  }

  let { db, peakDb = null, height = 10 }: Props = $props();

  const FULL_SCALE = 60; // dB window: -60..0 mapped to 0..100%

  function pct(db: number): number {
    if (!Number.isFinite(db)) return 0;
    return Math.min(100, Math.max(0, ((db + FULL_SCALE) / FULL_SCALE) * 100));
  }

  const fillPct = $derived(pct(db));
  const tickPct = $derived(peakDb === null ? null : pct(peakDb));
</script>

<div class="meter" style="height: {height}px" role="meter" aria-valuenow={db} aria-valuemin={-FULL_SCALE} aria-valuemax={0}>
  <div class="fill" style="width: {fillPct}%"></div>
  {#if tickPct !== null && tickPct > 0}
    <div class="tick" style="left: {tickPct}%"></div>
  {/if}
</div>
