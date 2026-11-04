<script lang="ts">
  // Options surface that lives in XML, not the database: output device,
  // sound-card / sound-option knobs, and the now-playing export plugin.
  import { session } from "./session.svelte";
  import type { ExportDraftView, FxConfig } from "./protocol";

  let busy = $state(false);
  let loadedFor = $state(-1);
  let tab = $state<"sound" | "fx" | "export">("sound");

  // Local editable copy of the export draft; flushed on Save.
  let draft = $state<ExportDraftView | null>(null);
  let draftFor = $state(-1);

  const online = $derived(session.conn === "open");
  const generation = $derived(online ? session.dbConnects : -1);

  // (Re)load everything once the socket is up. These ops do not need the
  // database, but reconnects bump `dbConnects` too, so re-sync then.
  $effect(() => {
    if (online && loadedFor !== generation) {
      loadedFor = generation;
      void call(async () => {
        await session.loadDevices();
        await session.loadSoundcards();
        await session.loadSoundOptions();
        await session.loadFx();
        await session.loadExportDraft();
      });
    }
  });

  // Editable URL/method rows (flushed into the draft on Save).
  let webRows = $state<{ url: string; method: string }[]>([]);

  // Keep the form copy in sync whenever a fresh draft arrives.
  $effect(() => {
    const d = session.exportDraft;
    if (d && draftFor !== session.dbConnects) {
      draftFor = session.dbConnects;
      draft = structuredClone(d);
      webRows = (d.web_urls ?? []).map((url, i) => ({
        url,
        method: d.web_methods?.[i] ?? "POST",
      }));
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

  // ---- sound device -----------------------------------------------------------

  const deviceName = $derived(session.sink?.name ?? "");

  function pickDevice(e: Event) {
    const v = (e.currentTarget as HTMLSelectElement).value;
    void call(() => session.setDevice(v === "" ? null : v));
  }

  // ---- sound cards / sound options -------------------------------------------

  function num(raw: string): number | null {
    const v = raw.trim() === "" ? NaN : Number(raw);
    return Number.isFinite(v) ? Math.trunc(v) : null;
  }

  function setCard(key: "audioMode" | "inputFadeIn" | "inputFadeOut", raw: string) {
    const v = num(raw);
    if (v === null) return;
    void call(() => session.setSoundcards({ [key]: v }));
  }

  function setOption(key: string, raw: string) {
    const v = num(raw);
    if (v === null) return;
    void call(() => session.setSoundOptions({ [key]: v }));
  }

  // ---- master sound processing (Plugin_SoundProcessing) -----------------------

  function setFx(patch: Partial<FxConfig>) {
    void call(() => session.setFx(patch));
  }

  function numFx(key: keyof FxConfig, raw: string) {
    const v = Number(raw);
    if (!Number.isFinite(v)) return;
    setFx({ [key]: v } as Partial<FxConfig>);
  }

  const checked = (e: Event) => (e.currentTarget as HTMLInputElement).checked;

  const EQ_BANDS = [32, 63, 125, 250, 500, 1000, 2000, 4000, 8000, 16000];

  function setEq(i: number, raw: string) {
    const v = Number(raw);
    if (!Number.isFinite(v) || !session.fx) return;
    const vals = [...session.fx.eqValues];
    vals[i] = v;
    setFx({ eqValues: vals });
  }

  // ---- now-playing export ------------------------------------------------------

  function flushWebRows() {
    if (!draft) return;
    draft.web_urls = webRows.map((w) => w.url).filter((u) => u.trim() !== "");
    draft.web_methods = webRows.map((w) => w.method);
  }

  function addWebRow() {
    if (!draft) return;
    flushWebRows();
    webRows.push({ url: "", method: "POST" });
  }

  function removeWebRow(i: number) {
    if (!draft) return;
    flushWebRows();
    webRows.splice(i, 1);
  }

  function saveDraft() {
    if (!draft) return;
    flushWebRows();
    void call(() => session.saveExportDraft(draft!));
  }
</script>

<section class="panel settings">
  <div class="head">
    <h2>Options</h2>
    <div class="tabs" role="tablist">
      <button class="tab" class:on={tab === "sound"} role="tab" aria-selected={tab === "sound"} onclick={() => (tab = "sound")}>Sound</button>
      <button class="tab" class:on={tab === "fx"} role="tab" aria-selected={tab === "fx"} onclick={() => (tab = "fx")}>Processing</button>
      <button class="tab" class:on={tab === "export"} role="tab" aria-selected={tab === "export"} onclick={() => (tab = "export")}>Now playing</button>
    </div>
    <span class="spacer"></span>
    {#if tab === "export"}
      <button onclick={() => void call(() => session.testExport())} disabled={!online || busy}>Test export</button>
      <button class="primary" onclick={saveDraft} disabled={!online || busy || !draft}>Save</button>
    {:else if tab === "fx"}
      <button onclick={() => void call(() => session.loadFx())} disabled={!online || busy}>Reload</button>
    {:else}
      <button onclick={() => void call(async () => { await session.loadDevices(); await session.loadSoundcards(); await session.loadSoundOptions(); })} disabled={!online || busy}>Reload</button>
    {/if}
  </div>

  {#if tab === "sound"}
    {#if session.sink}
      <div class="group">
        <h3>Output device</h3>
        <div class="fields">
          <label class="wide">
            Device
            <select onchange={pickDevice} disabled={!online || busy}>
              <option value="" selected={session.devices.length === 0 || !session.sink.is_device}>System default</option>
              {#each session.devices as d, i (i)}
                <option value={d} selected={d === session.sink.name}>{d}</option>
              {/each}
            </select>
          </label>
          <span class="mono sinkinfo">
            {session.sink.name} · {session.sink.sample_rate} Hz · {session.sink.channels} ch
            {#if !session.sink.is_device}<em>(not a live device)</em>{/if}
          </span>
        </div>
      </div>
    {/if}

    {#if session.soundcards}
      {@const sc = session.soundcards}
      <div class="group">
        <h3>Sound card</h3>
        <div class="fields">
          <label>
            Audio mode
            <input class="mono" type="number" min="0" value={sc.audio_mode} onchange={(e) => setCard("audioMode", e.currentTarget.value)} disabled={!online || busy} />
          </label>
          <label>
            Input fade in (ms)
            <input class="mono" type="number" min="0" step="100" value={sc.input_fade_in} onchange={(e) => setCard("inputFadeIn", e.currentTarget.value)} disabled={!online || busy} />
          </label>
          <label>
            Input fade out (ms)
            <input class="mono" type="number" min="0" step="100" value={sc.input_fade_out} onchange={(e) => setCard("inputFadeOut", e.currentTarget.value)} disabled={!online || busy} />
          </label>
        </div>
      </div>
    {/if}

    {#if session.soundOptions}
      {@const so = session.soundOptions}
      <div class="group">
        <h3>Sound options</h3>
        <div class="fields">
          <label>
            Master volume (0–100)
            <input class="mono" type="number" min="0" max="100" value={so.volume_out} onchange={(e) => setOption("volume_out", e.currentTarget.value)} disabled={!online || busy} />
          </label>
          <label>
            Auto-fade out (ms)
            <input class="mono" type="number" min="0" step="100" value={so.dur_fadeout} onchange={(e) => setOption("dur_fadeout", e.currentTarget.value)} disabled={!online || busy} />
          </label>
          <label>
            Overlay level (dB)
            <input class="mono" type="number" value={so.vol_overlay} onchange={(e) => setOption("vol_overlay", e.currentTarget.value)} disabled={!online || busy} />
          </label>
          <label>
            Sweeper level (dB)
            <input class="mono" type="number" value={so.vol_onsweeper} onchange={(e) => setOption("vol_onsweeper", e.currentTarget.value)} disabled={!online || busy} />
          </label>
          <label>
            Voice-track level (dB)
            <input class="mono" type="number" value={so.vol_vt} onchange={(e) => setOption("vol_vt", e.currentTarget.value)} disabled={!online || busy} />
          </label>
          <label>
            Loudness target (dBFS)
            <input class="mono" type="number" max="-1" value={so.bs_target} onchange={(e) => setOption("bs_target", e.currentTarget.value)} disabled={!online || busy} />
          </label>
          <label>
            Gain from BS.1770
            <input type="checkbox" checked={so.use_bs_loudness} onchange={(e) => void call(() => session.setSoundOptions({ use_bs_loudness: e.currentTarget.checked }))} disabled={!online || busy} />
          </label>
        </div>
        <p class="hint mono">volume_out / dur_fadeout / vol_overlay / vol_onsweeper / vol_vt / bs_target — applied live</p>
      </div>
    {/if}
  {:else if tab === "fx"}
    {#if session.fx}
      {@const fx = session.fx}
      <div class="group">
        <h3>Output</h3>
        <div class="fields">
          <label>
            Output level (0–100)
            <input class="mono" type="number" min="0" max="100" value={fx.vol_Out} onchange={(e) => numFx("vol_Out", e.currentTarget.value)} disabled={!online || busy} />
          </label>
          <label class="chk"><input type="checkbox" checked={fx.StereoEnhancerEnabled} onchange={(e) => setFx({ StereoEnhancerEnabled: checked(e) })} disabled={!online || busy} /> Stereo enhancer</label>
          <label>
            Width (1–10)
            <input class="mono" type="number" min="1" max="10" value={fx.fxStereo} onchange={(e) => numFx("fxStereo", e.currentTarget.value)} disabled={!online || busy || !fx.StereoEnhancerEnabled} />
          </label>
        </div>
        <p class="hint mono">vol_Out is a linear output level, applied live to every zone</p>
      </div>

      <div class="group">
        <h3>Compressor</h3>
        <div class="fields">
          <label class="chk"><input type="checkbox" checked={fx.CompressorEnabled} onchange={(e) => setFx({ CompressorEnabled: checked(e) })} disabled={!online || busy} /> Enable</label>
          <label class="chk"><input type="checkbox" checked={fx.fxCompByPreset} onchange={(e) => setFx({ fxCompByPreset: checked(e) })} disabled={!online || busy || !fx.CompressorEnabled} /> By preset</label>
          <label>
            Preset
            <select value={fx.fxCompPreset} onchange={(e) => numFx("fxCompPreset", e.currentTarget.value)} disabled={!online || busy || !fx.CompressorEnabled || !fx.fxCompByPreset}>
              <option value={0}>Default</option>
              <option value={1}>Hard</option>
              <option value={2}>Hard 2</option>
              <option value={3}>Hard commercial</option>
              <option value={4}>Medium</option>
              <option value={5}>Soft</option>
              <option value={6}>Soft 2</option>
            </select>
          </label>
          <label>
            Threshold (dB)
            <input class="mono" type="number" min="-60" max="0" value={fx.cThreshold} onchange={(e) => numFx("cThreshold", e.currentTarget.value)} disabled={!online || busy || !fx.CompressorEnabled || fx.fxCompByPreset} />
          </label>
          <label>
            Ratio
            <input class="mono" type="number" min="1" max="20" step="0.5" value={fx.cRatio} onchange={(e) => numFx("cRatio", e.currentTarget.value)} disabled={!online || busy || !fx.CompressorEnabled || fx.fxCompByPreset} />
          </label>
          <label>
            Attack (ms)
            <input class="mono" type="number" min="1" max="500" value={fx.cAttack} onchange={(e) => numFx("cAttack", e.currentTarget.value)} disabled={!online || busy || !fx.CompressorEnabled || fx.fxCompByPreset} />
          </label>
          <label>
            Release (ms)
            <input class="mono" type="number" min="50" max="3000" step="50" value={fx.cRelease} onchange={(e) => numFx("cRelease", e.currentTarget.value)} disabled={!online || busy || !fx.CompressorEnabled || fx.fxCompByPreset} />
          </label>
          <label>
            Makeup gain (dB)
            <input class="mono" type="number" min="-60" max="60" value={fx.cGain} onchange={(e) => numFx("cGain", e.currentTarget.value)} disabled={!online || busy || !fx.CompressorEnabled || fx.fxCompByPreset} />
          </label>
        </div>
      </div>

      <div class="group">
        <h3>Equalizer</h3>
        <div class="fields">
          <label class="chk"><input type="checkbox" checked={fx.eqON} onchange={(e) => setFx({ eqON: checked(e) })} disabled={!online || busy} /> Enable (10 peaking bands)</label>
        </div>
        <div class="eqgrid">
          {#each EQ_BANDS as f, i (f)}
            <label class="eqband">
              <span>{f >= 1000 ? f / 1000 + "k" : f}</span>
              <input class="mono" type="number" min="-12" max="12" value={fx.eqValues[i]} onchange={(e) => setEq(i, e.currentTarget.value)} disabled={!online || busy || !fx.eqON} />
            </label>
          {/each}
        </div>
      </div>

      <div class="group">
        <h3>AGC</h3>
        <div class="fields">
          <label class="chk"><input type="checkbox" checked={fx.AGCEnabled} onchange={(e) => setFx({ AGCEnabled: checked(e) })} disabled={!online || busy} /> Enable</label>
          <label>
            Preset
            <select value={fx.fxDampPreset} onchange={(e) => numFx("fxDampPreset", e.currentTarget.value)} disabled={!online || busy || !fx.AGCEnabled}>
              <option value={0}>Hard</option>
              <option value={1}>Medium</option>
              <option value={2}>Soft</option>
            </select>
          </label>
        </div>
        <p class="hint mono">Stored in Plugins/Plugin_SoundProcessing.xml — applied live</p>
      </div>
    {:else}
      <p class="hint">Sound-processing settings not loaded yet.</p>
    {/if}
  {:else if draft}
    <div class="group">
      <h3>File</h3>
      <div class="fields">
        <label class="chk"><input type="checkbox" bind:checked={draft.export_to_file} /> Export to file</label>
        <label class="wide">
          Path
          <input bind:value={draft.target_path} placeholder="/var/run/radiocraft/nowplaying.txt" disabled={!draft.export_to_file} />
        </label>
        <label>
          Encoding
          <select bind:value={draft.encoding} disabled={!draft.export_to_file}>
            <option value={0}>ANSI</option>
            <option value={1}>UTF-8</option>
            <option value={2}>UTF-16</option>
          </select>
        </label>
      </div>
      <textarea class="mono tmpl" rows="2" bind:value={draft.txt_file_template} placeholder="$artist$ - $title$ ($duration$)" disabled={!draft.export_to_file}></textarea>
    </div>

    <div class="group">
      <h3>Web</h3>
      <div class="fields">
        <label class="chk"><input type="checkbox" bind:checked={draft.export_to_web} /> POST to URLs</label>
        <label class="wide">
          Data template
          <input bind:value={draft.target_data} placeholder="-" disabled={!draft.export_to_web} />
        </label>
      </div>
      {#each webRows as item, i (i)}
        <div class="webrow">
          <input bind:value={item.url} placeholder="https://example.com/np" disabled={!draft.export_to_web} />
          <select bind:value={item.method} disabled={!draft.export_to_web}>
            <option>POST</option>
            <option>GET</option>
          </select>
          <button class="danger" onclick={() => removeWebRow(i)} disabled={!draft.export_to_web}>✕</button>
        </div>
      {/each}
      <button onclick={addWebRow} disabled={!draft.export_to_web}>Add URL</button>
    </div>

    <div class="group">
      <h3>Serial</h3>
      <div class="fields">
        <label class="chk"><input type="checkbox" bind:checked={draft.export_to_serial} /> Send over serial</label>
        <label>
          Port
          <input bind:value={draft.target_port} placeholder="/dev/ttyS0" disabled={!draft.export_to_serial} />
        </label>
        <label>
          Baud
          <input class="mono" type="number" bind:value={draft.serial_baud} disabled={!draft.export_to_serial} />
        </label>
        <label>
          Data bits
          <input class="mono" type="number" min="5" max="8" bind:value={draft.serial_data} disabled={!draft.export_to_serial} />
        </label>
        <label>
          Parity
          <select bind:value={draft.serial_parity} disabled={!draft.export_to_serial}>
            <option value={0}>None</option>
            <option value={1}>Odd</option>
            <option value={2}>Even</option>
          </select>
        </label>
        <label>
          Stop bits
          <select bind:value={draft.serial_stop} disabled={!draft.export_to_serial}>
            <option value={1}>1</option>
            <option value={2}>2</option>
          </select>
        </label>
        <label class="chk"><input type="checkbox" bind:checked={draft.serial_dtr} /> DTR</label>
        <label class="chk"><input type="checkbox" bind:checked={draft.serial_rts} /> RTS</label>
      </div>
      <textarea class="mono tmpl" rows="2" bind:value={draft.serial_template} placeholder="$artist$ - $title$" disabled={!draft.export_to_serial}></textarea>
    </div>

    <div class="group">
      <h3>Title</h3>
      <div class="fields">
        <label class="chk"><input type="checkbox" bind:checked={draft.enable_custom_title} /> Custom title</label>
        <label class="wide">
          Custom title
          <input bind:value={draft.custom_title} disabled={!draft.enable_custom_title} />
        </label>
        <label class="wide">
          Track types
          <input class="mono" bind:value={draft.types_text} placeholder="0;1;5;6;" />
        </label>
      </div>
      <p class="hint mono">tokens: $artist$ $title$ $album$ $duration$ $station_name$ … unknown tokens stay literal</p>
    </div>

    {#if session.exportTest}
      {@const t = session.exportTest}
      <div class="group">
        <h3>Last test</h3>
        <p class="mono channels">
          file <span data-on={t.file}>{t.file ? "✓" : "—"}</span>
          web <span data-on={t.web}>{t.web ? "✓" : "—"}</span>
          serial <span data-on={t.serial}>{t.serial ? "✓" : "—"}</span>
        </p>
        <pre class="mono snap">{JSON.stringify(t.snapshot, null, 1)}</pre>
      </div>
    {/if}
  {:else}
    <p class="empty">{online ? "Loading options…" : "Connect to the server first."}</p>
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

  .tabs {
    display: flex;
    gap: 4px;
  }

  .tab {
    padding: 4px 12px;
    border-radius: 6px 6px 0 0;
    border-bottom-color: transparent;
    color: var(--fg-dim);
  }

  .tab.on {
    color: var(--fg);
    border-color: var(--accent);
    background: var(--panel-2);
  }

  .group {
    display: flex;
    flex-direction: column;
    gap: 8px;
    padding: 10px 12px;
    border: 1px solid var(--border);
    border-radius: 8px;
    background: var(--panel-2);
  }

  .group h3 {
    margin: 0;
    font-size: 11px;
    font-weight: 600;
    letter-spacing: 0.08em;
    text-transform: uppercase;
    color: var(--fg-dim);
  }

  .fields {
    display: flex;
    flex-wrap: wrap;
    gap: 8px 14px;
    align-items: center;
  }

  .fields label {
    display: flex;
    align-items: center;
    gap: 6px;
    font-size: 13px;
    color: var(--fg-dim);
  }

  .fields label.wide {
    flex: 1;
    min-width: 240px;
  }

  .fields label.wide input,
  .fields label.wide select {
    flex: 1;
  }

  .fields input[type="number"] {
    width: 84px;
  }

  label.chk {
    color: var(--fg);
  }

  .eqgrid {
    display: grid;
    grid-template-columns: repeat(auto-fit, minmax(96px, 1fr));
    gap: 6px 10px;
    margin-top: 8px;
  }

  .eqband {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 6px;
    font-size: 12px;
    color: var(--fg-dim);
  }

  .eqband input[type="number"] {
    width: 56px;
  }

  .sinkinfo {
    font-size: 12px;
    color: var(--fg-dim);
  }

  .tmpl {
    width: 100%;
    background: var(--bg);
    color: var(--fg);
    border: 1px solid var(--border);
    border-radius: 6px;
    padding: 6px 8px;
    resize: vertical;
  }

  .webrow {
    display: flex;
    gap: 8px;
    align-items: center;
  }

  .webrow input {
    flex: 1;
  }

  .hint {
    margin: 0;
    font-size: 11px;
    color: var(--fg-dim);
  }

  .channels {
    margin: 0;
    display: flex;
    gap: 14px;
  }

  .channels span {
    color: var(--fg-dim);
  }

  .channels span[data-on="true"] {
    color: var(--ok);
  }

  .snap {
    margin: 0;
    max-height: 180px;
    overflow: auto;
    font-size: 12px;
    background: var(--bg);
    border: 1px solid var(--border);
    border-radius: 6px;
    padding: 8px;
    white-space: pre-wrap;
  }

  .empty {
    color: var(--fg-dim);
    margin: 6px 0;
  }
</style>
