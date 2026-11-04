<script lang="ts">
  import { session } from "./session.svelte";
  import type { SongFull, SongRow } from "./protocol";

  let needle = $state("");
  let sort = $state<"artist" | "title" | "album" | "duration" | "year" | "weight" | "count">("artist");
  let enabled = $state<"all" | "on" | "off">("all");
  let offset = $state(0);
  const PAGE = 100;

  let loading = $state(false);
  let selected = $state<Set<number>>(new Set());
  type EditableSong = SongFull & { weight: number };

  let editing = $state<EditableSong | null>(null);

  // Internet-stream row (RadioDJ song_type 5): URL lives in `path`.
  let addingStream = $state(false);
  let streamUrl = $state("");
  let streamTitle = $state("");
  let streamArtist = $state("");

  // Track analyzer (Plugin_TracksManager): dB thresholds + busy flag.
  let analyzing = $state(false);
  let analyzeReport = $state<AnalyzeReport | null>(null);
  let anDbIn = $state(-24);
  let anDbNext = $state(-5);
  let anDbOut = $state(-30);

  type AnalyzeReport = {
    analyzed: number;
    skipped: number;
    total: number;
    errors: string[];
    status: string;
  };

  // The sweep is a server background job: the cmd reply only confirms start,
  // the completion report arrives as a broadcast (id 0) result frame.
  $effect(() => {
    const off = session.onBroadcast("analyze_tracks", (data) => {
      analyzing = false;
      analyzeReport = data as unknown as AnalyzeReport;
      void search(false);
    });
    return off;
  });

  const songs = $derived(session.songs);
  const total = $derived(session.libraryTotal);
  const ready = $derived(session.conn === "open" && session.dbConnected);
  let loadedFor = $state(0);

  // Auto-load the first page once the library becomes ready (after connect).
  $effect(() => {
    if (ready && loadedFor !== session.dbConnects) {
      loadedFor = session.dbConnects;
      void search();
    }
  });

  async function search(reset = true) {
    if (reset) offset = 0;
    loading = true;
    try {
      await session.filterLibrary({
        text: needle.trim() || undefined,
        sort,
        enabled: enabled === "all" ? null : enabled === "on",
        limit: PAGE,
        offset,
      });
    } catch (e) {
      session.lastError = String(e);
    } finally {
      loading = false;
    }
  }

  async function call(fn: () => Promise<unknown>) {
    try {
      await fn();
    } catch (e) {
      session.lastError = String(e);
    }
  }

  async function play(song: SongRow, cue: boolean) {
    try {
      await session[cue ? "cue" : "play"](song.ID);
    } catch (e) {
      session.lastError = String(e);
    }
  }

  function toggle(id: number) {
    const next = new Set(selected);
    if (next.has(id)) next.delete(id);
    else next.add(id);
    selected = next;
  }

  function toggleAll() {
    selected = selected.size === songs.length ? new Set() : new Set(songs.map((s) => s.ID));
  }

  const ids = $derived([...selected]);

  async function mass(field: "enabled" | "weight" | "genre" | "subcat", value: unknown) {
    if (ids.length === 0) return;
    try {
      await session.massSet(ids, field, value);
      await search(false);
    } catch (e) {
      session.lastError = String(e);
    }
  }

  function massWeight() {
    const raw = window.prompt("New weight for selected songs:");
    if (raw === null) return;
    const v = Number(raw);
    if (!Number.isFinite(v)) {
      session.lastError = "weight must be a number";
      return;
    }
    void mass("weight", v);
  }

  function massGenre() {
    const raw = window.prompt("New genre id for selected songs (0 = none):");
    if (raw === null) return;
    const v = Number(raw);
    if (!Number.isInteger(v) || v < 0) {
      session.lastError = "genre id must be a non-negative integer";
      return;
    }
    void mass("genre", v);
  }

  async function massDelete() {
    if (ids.length === 0) return;
    if (!window.confirm(`Delete ${ids.length} song(s) from the library?`)) return;
    try {
      await session.massDelete(ids);
      selected = new Set();
      await search(false);
    } catch (e) {
      session.lastError = String(e);
    }
  }

  async function analyzeTracks() {
    if (analyzing) return;
    const scope = ids.length > 0 ? `${ids.length} selected track(s)` : "the entire library";
    if (!window.confirm(`Re-analyze cue points for ${scope}? Stored cue positions are overwritten.`)) return;
    analyzing = true;
    analyzeReport = null;
    try {
      // Reply only acknowledges the sweep started; the report lands via onBroadcast.
      await session.cmd("analyze_tracks", {
        ...(ids.length > 0 ? { songIds: ids } : {}),
        dbIn: anDbIn,
        dbNext: anDbNext,
        dbEnd: anDbOut,
      });
    } catch (e) {
      session.lastError = String(e);
      analyzing = false;
    }
  }

  async function openEdit(song: SongRow) {
    try {
      const s = await session.getSong(song.ID);
      // Weight crosses the decimal string → number boundary for the inputs.
      const next = { ...s, weight: Number(s.weight) || 0 } as EditableSong;
      editing = next;
    } catch (e) {
      session.lastError = String(e);
    }
  }

  async function saveEdit() {
    if (!editing) return;
    try {
      await session.updateSong(editing);
      editing = null;
      await search(false);
    } catch (e) {
      session.lastError = String(e);
    }
  }

  async function removeOne(song: SongRow) {
    if (!window.confirm(`Delete "${song.artist} — ${song.title}" from the library?`)) return;
    try {
      await session.deleteSong(song.ID);
      await search(false);
    } catch (e) {
      session.lastError = String(e);
    }
  }

  async function addStream() {
    const url = streamUrl.trim();
    if (!/^https?:\/\//i.test(url)) {
      session.lastError = "Stream URL must start with http:// or https://";
      return;
    }
    await call(() =>
      session.cmd("stream_add", {
        url,
        title: streamTitle.trim(),
        artist: streamArtist.trim(),
      })
    );
    addingStream = false;
    streamUrl = streamTitle = streamArtist = "";
    void search();
  }

  function fmt(duration: string): string {
    return duration || "";
  }
</script>

<section class="panel library">
  <h2>Library</h2>

  <div class="row">
    <input
      type="search"
      placeholder="Search artist / title…"
      bind:value={needle}
      onkeydown={(e) => e.key === "Enter" && void search()}
      disabled={!ready}
    />
    <select bind:value={sort} disabled={!ready}>
      <option value="artist">sort: artist</option>
      <option value="title">sort: title</option>
      <option value="album">sort: album</option>
      <option value="year">sort: year</option>
      <option value="duration">sort: length</option>
      <option value="weight">sort: weight</option>
      <option value="count">sort: plays</option>
    </select>
    <select bind:value={enabled} disabled={!ready}>
      <option value="all">enabled: all</option>
      <option value="on">enabled only</option>
      <option value="off">disabled only</option>
    </select>
    <button onclick={() => void search()} disabled={!ready || loading}>{loading ? "…" : "Search"}</button>
    <button title="Add an internet stream to the library" onclick={() => (addingStream = true)} disabled={!ready}>
      + Stream
    </button>
    <button
      disabled={!ready || offset === 0}
      onclick={() => {
        offset = Math.max(0, offset - PAGE);
        void search(false);
      }}>Prev</button>
    <button
      disabled={!ready || offset + PAGE >= total}
      onclick={() => {
        offset += PAGE;
        void search(false);
      }}>Next</button>
    <span class="count mono">{total || songs.length}</span>
  </div>

  {#if selected.size > 0}
    <div class="row mass">
      <span class="mono dim">{selected.size} selected</span>
      <button onclick={() => void mass("enabled", true)}>Enable</button>
      <button onclick={() => void mass("enabled", false)}>Disable</button>
      <button onclick={massWeight}>Weight…</button>
      <button onclick={massGenre}>Genre…</button>
      <button class="danger" onclick={() => void massDelete()}>Delete</button>
      <button onclick={() => (selected = new Set())}>Clear</button>
    </div>
  {/if}

  <div class="row analyze">
    <span class="dim">analyze dB</span>
    <label>start <input type="number" min="-100" max="0" step="1" bind:value={anDbIn} disabled={analyzing} /></label>
    <label>next <input type="number" min="-100" max="0" step="1" bind:value={anDbNext} disabled={analyzing} /></label>
    <label>end <input type="number" min="-100" max="0" step="1" bind:value={anDbOut} disabled={analyzing} /></label>
    <button title="Detect cue in / next / out (with selection: selected rows only)" disabled={!ready || analyzing} onclick={() => void analyzeTracks()}>
      {analyzing ? "Analyzing…" : "Analyze"}
    </button>
    {#if analyzing}
      <span class="progress">{session.status.message}</span>
    {:else if analyzeReport}
      <span class="progress" title={analyzeReport.errors.join("\n")}>
        {analyzeReport.analyzed} analyzed, {analyzeReport.skipped} skipped
        {#if analyzeReport.errors.length > 0}(⚠ {analyzeReport.errors.length}){/if}
      </span>
      <button class="link" onclick={() => (analyzeReport = null)}>dismiss</button>
    {:else}
      <span class="dim">level thresholds (defaults -24 / -5 / -30)</span>
    {/if}
  </div>

  {#if editing}
    <div class="editform">
      <h3>Edit song #{editing.ID}</h3>
      <div class="fields">
        <label>Artist <input type="text" bind:value={editing.artist} /></label>
        <label>Title <input type="text" bind:value={editing.title} /></label>
        <label>Album <input type="text" bind:value={editing.album} /></label>
        <label>Year <input type="text" bind:value={editing.year} /></label>
        <label>Weight <input type="number" step="0.001" bind:value={editing.weight} /></label>
        <label>Genre id <input type="number" min="0" bind:value={editing.id_genre} /></label>
        <label>Subcat id <input type="number" min="0" bind:value={editing.id_subcat} /></label>
        <label class="check"><input type="checkbox" bind:checked={editing.enabled} /> enabled</label>
      </div>
      <div class="row">
        <button class="primary" onclick={() => void saveEdit()}>Save</button>
        <button onclick={() => (editing = null)}>Cancel</button>
        <span class="mono dim path" title={editing.path}>{editing.path}</span>
      </div>
    </div>
  {/if}

  {#if addingStream}
    <div class="editform">
      <h3>Add internet stream</h3>
      <div class="fields">
        <label>URL <input type="text" placeholder="https://…" bind:value={streamUrl} /></label>
        <label>Title <input type="text" placeholder="defaults to the URL" bind:value={streamTitle} /></label>
        <label>Artist <input type="text" bind:value={streamArtist} /></label>
      </div>
      <div class="row">
        <button class="primary" onclick={() => void addStream()}>Add stream</button>
        <button onclick={() => (addingStream = false)}>Cancel</button>
        <span class="dim">The live ICY broadcast title replaces the stored title while playing.</span>
      </div>
    </div>
  {/if}

  <div class="scroll">
    <table>
      <thead>
        <tr>
          <th><input type="checkbox" checked={songs.length > 0 && selected.size === songs.length} onchange={toggleAll} /></th>
          <th>Artist</th><th>Title</th><th>Album</th><th>Length</th><th></th>
        </tr>
      </thead>
      <tbody>
        {#each songs as song (song.ID)}
          <tr class:sel={selected.has(song.ID)}>
            <td><input type="checkbox" checked={selected.has(song.ID)} onchange={() => toggle(song.ID)} /></td>
            <td title={song.artist}>{song.artist}</td>
            <td title={song.title}>{song.title}</td>
            <td class="dim" title={song.album}>{song.album}</td>
            <td class="mono dim">{fmt(song.duration)}</td>
            <td class="actions">
              <button title="Play now" onclick={() => void play(song, false)}>▶</button>
              <button title="Cue next" onclick={() => void play(song, true)}>Cue</button>
              <button title="Add to play queue" onclick={() => void call(() => session.queueAddSong(song.ID))}>Queue</button>
              <button title="Show details" onclick={() => void session.selectSong(song.ID)}>ℹ</button>
              <button title="Edit" onclick={() => void openEdit(song)}>Edit</button>
              <button class="danger" title="Delete" onclick={() => void removeOne(song)}>✕</button>
            </td>
          </tr>
        {:else}
          <tr class="empty"><td colspan={6}>{ready ? "No songs match." : "Connect the database first."}</td></tr>
        {/each}
      </tbody>
    </table>
  </div>
</section>

<style>
  .row {
    display: flex;
    gap: 8px;
    align-items: center;
  }

  .mass {
    background: var(--panel-2);
    border: 1px solid var(--border);
    border-radius: 8px;
    padding: 6px 8px;
  }

  .count {
    margin-left: auto;
    color: var(--fg-dim);
    font-size: 12px;
  }

  .analyze label {
    display: flex;
    align-items: center;
    gap: 4px;
    font-size: 11px;
    color: var(--fg-dim);
  }

  .analyze input {
    width: 7ch;
  }
  .analyze .progress {
    color: var(--accent, #7aa2f7);
    max-width: 46ch;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  button.link {
    background: none;
    border: none;
    color: var(--muted, #9aa0ab);
    padding: 0 4px;
    text-decoration: underline;
  }

  .editform {
    border: 1px solid var(--border);
    border-radius: 8px;
    padding: 8px 10px;
    display: flex;
    flex-direction: column;
    gap: 8px;
  }

  .editform h3 {
    margin: 0;
    font-size: 13px;
    color: var(--fg-dim);
    font-weight: 500;
  }

  .fields {
    display: flex;
    gap: 10px;
    flex-wrap: wrap;
  }

  .fields label {
    display: flex;
    flex-direction: column;
    gap: 3px;
    font-size: 11px;
    color: var(--fg-dim);
  }

  .fields label.check {
    flex-direction: row;
    align-items: center;
    gap: 6px;
    font-size: 12px;
  }

  .fields input {
    width: 16ch;
  }

  .path {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    max-width: 40ch;
    font-size: 11px;
  }

  .scroll {
    overflow: auto;
    max-height: 320px;
    border: 1px solid var(--border);
    border-radius: 8px;
  }

  table {
    width: 100%;
    border-collapse: collapse;
    font-size: 13px;
  }

  th,
  td {
    text-align: left;
    padding: 5px 8px;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
    max-width: 24ch;
  }

  thead th {
    position: sticky;
    top: 0;
    background: var(--panel-2);
    color: var(--fg-dim);
    font-weight: 500;
  }

  .dim {
    color: var(--fg-dim);
  }

  tr.sel {
    background: color-mix(in srgb, var(--accent, #6ea8fe) 12%, transparent);
  }

  .actions {
    display: flex;
    gap: 4px;
  }

  .actions button {
    padding: 2px 7px;
    font-size: 12px;
  }

  .empty td {
    color: var(--fg-dim);
    text-align: center;
    padding: 14px;
  }
</style>
