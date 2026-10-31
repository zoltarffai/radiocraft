<script lang="ts">
  import { session } from "./session.svelte";
  import type { CriteriaRow, LookupRow, SelectionMode } from "./protocol";

  let selectedId = $state<number | null>(null);
  // Rule being edited (null = closed, id 0 = a new unsaved rule line).
  let editRow = $state<CriteriaRow | null>(null);
  let loading = $state(false);

  const rotations = $derived(session.rotations);
  const criteria = $derived(session.criteria);
  const lookups = $derived(session.lookups);
  const ready = $derived(session.conn === "open" && session.dbConnected);
  let loadedFor = $state(0);

  $effect(() => {
    if (ready && loadedFor !== session.dbConnects) {
      loadedFor = session.dbConnects;
      void refreshLists();
    }
  });

  async function refreshLists() {
    try {
      await session.loadRotations();
      if (selectedId !== null && !rotations.some((r) => r.ID === selectedId)) selectedId = null;
      if (selectedId === null && rotations.length > 0) await select(rotations[0].ID);
      else if (selectedId !== null) await session.loadCriteria(selectedId);
      await session.loadLookups();
    } catch (e) {
      session.lastError = String(e);
    }
  }

  async function select(id: number) {
    selectedId = id;
    editRow = null;
    try {
      await session.loadCriteria(id);
    } catch (e) {
      session.lastError = String(e);
    }
  }

  async function call(fn: () => Promise<unknown>) {
    loading = true;
    try {
      await fn();
      if (selectedId !== null) await session.loadCriteria(selectedId);
    } catch (e) {
      session.lastError = String(e);
    } finally {
      loading = false;
    }
  }

  async function create() {
    const name = window.prompt("New rotation name:");
    if (!name?.trim()) return;
    void call(async () => {
      const id = await session.createRotation(name.trim());
      await session.loadRotations();
      await select(id);
    });
  }

  async function rename() {
    if (selectedId === null) return;
    const current = rotations.find((r) => r.ID === selectedId)?.name ?? "";
    const name = window.prompt("Rename rotation:", current);
    if (!name?.trim()) return;
    void call(async () => {
      await session.renameRotation(selectedId as number, name.trim());
      await session.loadRotations();
    });
  }

  async function removeRotation() {
    if (selectedId === null) return;
    if (!window.confirm("Delete this rotation and all its rule lines?")) return;
    void call(async () => {
      await session.deleteRotation(selectedId as number);
      selectedId = null;
      session.criteria = [];
      editRow = null;
      await session.loadRotations();
    });
  }

  function blankRule(): CriteriaRow {
    return {
      id: 0,
      rotation_id: selectedId ?? 0,
      name: `rule ${criteria.length + 1}`,
      enabled: true,
      category_id: 0,
      category_type: "Include",
      subcategory_id: 0,
      subcategory_type: "Include",
      genre_id: 0,
      weight_min: 0,
      repeat_track_interval: 0,
      repeat_artist_interval: 0,
      repeat_title_interval: 0,
      repeat_album_interval: 0,
      time_start: null,
      time_end: null,
      days: 0x7f,
      language: null,
      mood: null,
      gender: null,
      selection: "Weight",
      repeat_rule: 0,
      ord: criteria.length + 1,
    };
  }

  function startAdd() {
    if (editRow?.id === 0) editRow = null;
    else editRow = blankRule();
  }

  function startEdit(row: CriteriaRow) {
    editRow = editRow?.id === row.id ? null : { ...row };
  }

  function duplicate(row: CriteriaRow) {
    editRow = { ...row, id: 0, name: `${row.name} copy`, ord: criteria.length + 1 };
  }

  async function save() {
    const e = editRow;
    if (!e || selectedId === null) return;
    void call(async () => {
      if (e.id === 0) await session.criteriaAdd(selectedId as number, e);
      else await session.criteriaUpdate(e);
      editRow = null;
    });
  }

  const num = (v: number | string) => (Number.isFinite(Number(v)) ? Number(v) : 0);

  // Time inputs are "HH:MM" (seconds optional); the wire wants "HH:MM:SS" or null.
  function toWireTime(v: string): string | null {
    const t = v.trim();
    if (!t) return null;
    return t.length === 5 ? `${t}:00` : t;
  }
  const timeValue = (v: string | null) => (v ? v.slice(0, 5) : "");

  const DAY_NAMES = ["Su", "Mo", "Tu", "We", "Th", "Fr", "Sa"];
  function dayOn(bit: number): boolean {
    return ((editRow?.days ?? 0) & (1 << bit)) !== 0;
  }
  function toggleDay(bit: number) {
    if (!editRow) return;
    editRow.days ^= 1 << bit;
  }

  const lookupName = (rows: LookupRow[], id: number) =>
    rows.find((r) => r.ID === id)?.name ?? `#${id}`;
  function subLabel(s: LookupRow): string {
    const parent = s.parentid ? lookups.categories.find((c) => c.ID === s.parentid) : null;
    return parent ? `${parent.name} › ${s.name}` : s.name;
  }

  function filterLabel(row: CriteriaRow): string {
    const parts: string[] = [];
    if (row.category_id) parts.push(`${row.category_type === "Include" ? "cat" : "!cat"}:${lookupName(lookups.categories, row.category_id)}`);
    if (row.subcategory_id) parts.push(`${row.subcategory_type === "Include" ? "sub" : "!sub"}:${lookupName(lookups.subcategories, row.subcategory_id)}`);
    if (row.genre_id) parts.push(`gen:${lookupName(lookups.genres, row.genre_id)}`);
    return parts.length ? parts.join(" ") : "any song";
  }
  function daypart(row: CriteriaRow): string {
    if (!row.time_start && !row.time_end) return "all day";
    return `${row.time_start?.slice(0, 5) ?? "…"}–${row.time_end?.slice(0, 5) ?? "…"}`;
  }
  const SELECTION_LABELS: Record<SelectionMode, string> = {
    Weight: "Weighted",
    LeastPlayed: "Least played",
    Oldest: "Oldest played",
    Newest: "Newest added",
    Random: "Shuffle",
  };
</script>

<section class="panel rotations">
  <h2>Rotations</h2>
  <div class="row">
    <select
      value={selectedId ?? ""}
      onchange={(e) => {
        const v = Number((e.currentTarget as HTMLSelectElement).value);
        if (v > 0) void select(v);
      }}>
      <option value="" disabled>Choose a rotation…</option>
      {#each rotations as r (r.ID)}
        <option value={r.ID}>{r.name}</option>
      {/each}
    </select>
    <button onclick={() => void create()}>New</button>
    <button disabled={selectedId === null} onclick={() => void rename()}>Rename</button>
    <button class="danger" disabled={selectedId === null} onclick={() => void removeRotation()}>Delete</button>
    <button class="primary" disabled={selectedId === null || loading} onclick={startAdd}>
      {editRow?.id === 0 ? "Cancel rule" : "Add rule"}
    </button>
    <span class="count mono">{criteria.length} rule{criteria.length === 1 ? "" : "s"}</span>
  </div>

  <div class="scroll">
    <table>
      <thead>
        <tr>
          <th>Ord</th><th>Rule</th><th>Filter</th><th>Daypart</th><th>Days</th>
          <th>Order</th><th>Repeats (min)</th><th>Actions</th>
        </tr>
      </thead>
      <tbody>
        {#each criteria as row (row.id)}
          <tr class={!row.enabled ? "off" : ""}>
            <td class="mono dim">{row.ord}</td>
            <td title={row.name}>{row.name}</td>
            <td title={filterLabel(row)}>{filterLabel(row)}</td>
            <td class="mono">{daypart(row)}</td>
            <td class="mono dim">{row.days === 0x7f ? "all" : row.days.toString(2)}</td>
            <td>{SELECTION_LABELS[row.selection]}</td>
            <td class="mono dim">
              {row.repeat_track_interval}/{row.repeat_artist_interval}/{row.repeat_title_interval}/{row.repeat_album_interval}
            </td>
            <td class="actions">
              <button title="Edit this rule" onclick={() => startEdit(row)}>Edit</button>
              <button title="Duplicate into a new rule" onclick={() => duplicate(row)}>Dup</button>
              <button class="danger" title="Delete this rule" onclick={() => void call(() => session.criteriaDelete(row.id))}>
                ✕
              </button>
            </td>
          </tr>
          {#if editRow && editRow.id === row.id}
            <tr class="ruleedit"><td colspan={8}>{@render ruleForm()}</td></tr>
          {/if}
        {/each}
        {#if editRow?.id === 0}
          <tr class="ruleedit"><td colspan={8}>{@render ruleForm()}</td></tr>
        {:else if criteria.length === 0}
          <tr class="empty">
            <td colspan={8}>{ready ? "No rule lines — press Add rule." : "Connect the database first."}</td>
          </tr>
        {/if}
      </tbody>
    </table>
  </div>
</section>

{#snippet ruleForm()}
  {#if editRow}
    <div class="form">
      <div class="grid">
        <label>
          Name
          <input type="text" bind:value={editRow.name} />
        </label>
        <label>
          Order (ord)
          <input type="number" min="0" max="127" bind:value={editRow.ord} />
        </label>
        <label>
          Selection
          <select bind:value={editRow.selection}>
            <option value="Weight">Weighted</option>
            <option value="LeastPlayed">Least played</option>
            <option value="Oldest">Oldest played</option>
            <option value="Newest">Newest added</option>
            <option value="Random">Shuffle</option>
          </select>
        </label>
        <label>
          Weight floor
          <input type="number" min="0" step="0.1" value={editRow.weight_min}
            oninput={(e) => (editRow!.weight_min = num((e.currentTarget as HTMLInputElement).value))} />
        </label>

        <label>
          Category
          <select
            value={String(editRow.category_id)}
            onchange={(e) => (editRow!.category_id = Number((e.currentTarget as HTMLSelectElement).value))}>
            <option value="0">any</option>
            {#each lookups.categories as c (c.ID)}
              <option value={String(c.ID)}>{c.name}</option>
            {/each}
          </select>
        </label>
        <label>
          Category match
          <select bind:value={editRow.category_type}>
            <option value="Include">include</option>
            <option value="Exclude">exclude</option>
          </select>
        </label>
        <label>
          Subcategory
          <select
            value={String(editRow.subcategory_id)}
            onchange={(e) => (editRow!.subcategory_id = Number((e.currentTarget as HTMLSelectElement).value))}>
            <option value="0">any</option>
            {#each lookups.subcategories as s (s.ID)}
              <option value={String(s.ID)}>{subLabel(s)}</option>
            {/each}
          </select>
        </label>
        <label>
          Subcategory match
          <select bind:value={editRow.subcategory_type}>
            <option value="Include">include</option>
            <option value="Exclude">exclude</option>
          </select>
        </label>
        <label>
          Genre
          <select
            value={String(editRow.genre_id)}
            onchange={(e) => (editRow!.genre_id = Number((e.currentTarget as HTMLSelectElement).value))}>
            <option value="0">any</option>
            {#each lookups.genres as g (g.ID)}
              <option value={String(g.ID)}>{g.name}</option>
            {/each}
          </select>
        </label>

        <label>
          Track repeat (min)
          <input type="number" min="0" bind:value={editRow.repeat_track_interval} />
        </label>
        <label>
          Artist repeat (min)
          <input type="number" min="0" bind:value={editRow.repeat_artist_interval} />
        </label>
        <label>
          Title repeat (min)
          <input type="number" min="0" bind:value={editRow.repeat_title_interval} />
        </label>
        <label>
          Album repeat (min)
          <input type="number" min="0" bind:value={editRow.repeat_album_interval} />
        </label>

        <label>
          Daypart start
          <input
            type="time"
            value={timeValue(editRow.time_start)}
            onchange={(e) => (editRow!.time_start = toWireTime((e.currentTarget as HTMLInputElement).value))} />
        </label>
        <label>
          Daypart end
          <input
            type="time"
            value={timeValue(editRow.time_end)}
            onchange={(e) => (editRow!.time_end = toWireTime((e.currentTarget as HTMLInputElement).value))} />
        </label>
        <div class="days">
          <span>Days</span>
          <div>
            {#each DAY_NAMES as d, bit (d)}
              <label class="day">
                <input type="checkbox" checked={dayOn(bit)} onchange={() => toggleDay(bit)} />
                {d}
              </label>
            {/each}
          </div>
        </div>
        <label>
          Language
          <input
            type="text"
            value={editRow.language ?? ""}
            oninput={(e) => (editRow!.language = (e.currentTarget as HTMLInputElement).value.trim() || null)} />
        </label>
        <label>
          Mood
          <input
            type="text"
            value={editRow.mood ?? ""}
            oninput={(e) => (editRow!.mood = (e.currentTarget as HTMLInputElement).value.trim() || null)} />
        </label>
        <label>
          Gender
          <input
            type="text"
            value={editRow.gender ?? ""}
            oninput={(e) => (editRow!.gender = (e.currentTarget as HTMLInputElement).value.trim() || null)} />
        </label>
      </div>
      <div class="tail">
        <label class="check">
          <input type="checkbox" bind:checked={editRow.enabled} />
          Enabled
        </label>
        <label class="check">
          <input
            type="checkbox"
            checked={editRow.repeat_rule !== 0}
            onchange={(e) => (editRow!.repeat_rule = (e.currentTarget as HTMLInputElement).checked ? 1 : 0)} />
          Allow the same track twice in a row
        </label>
        <span class="spacer"></span>
        <button class="primary" onclick={() => void save()}>Save rule</button>
        <button onclick={() => (editRow = null)}>Cancel</button>
      </div>
    </div>
  {/if}
{/snippet}

<style>
  .row {
    display: flex;
    gap: 8px;
    align-items: center;
  }

  .count {
    margin-left: auto;
    color: var(--fg-dim);
    font-size: 12px;
  }

  .scroll {
    overflow: auto;
    max-height: 460px;
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
    max-width: 26ch;
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

  tr.off td {
    opacity: 0.5;
  }

  .actions {
    display: flex;
    gap: 4px;
  }

  .actions button {
    padding: 2px 7px;
    font-size: 12px;
  }

  .ruleedit td {
    background: var(--panel-2);
    white-space: normal;
  }

  .form {
    display: flex;
    flex-direction: column;
    gap: 10px;
    padding: 6px 0;
  }

  .grid {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(150px, 1fr));
    gap: 8px 10px;
  }

  .grid label {
    display: flex;
    flex-direction: column;
    gap: 3px;
    font-size: 11px;
    color: var(--fg-dim);
  }

  .days {
    display: flex;
    flex-direction: column;
    gap: 3px;
    font-size: 11px;
    color: var(--fg-dim);
  }

  .days > div {
    display: flex;
    gap: 6px;
    flex-wrap: wrap;
  }

  .day {
    display: flex;
    align-items: center;
    gap: 3px;
    font-size: 11px;
  }

  .tail {
    display: flex;
    gap: 14px;
    align-items: center;
    flex-wrap: wrap;
  }

  .tail .check {
    display: flex;
    align-items: center;
    gap: 6px;
    font-size: 12px;
    color: var(--fg-dim);
  }

  .tail .spacer {
    margin-left: auto;
  }
</style>
