//! Reactive session: the WebSocket connection plus the state it feeds.
//!
//! One long-lived socket shared by every panel; commands are correlated by
//! id and server pushes update the `$state` fields components read.

import type {
  AutodjPlan,
  AutodjStatus,
  CartButtonRow,
  CartButtonSpec,
  CartHeader,
  CartNowInfo,
  CartwallConfig,
  ClientMsg,
  CriteriaRow,
  DbConfigView,
  ExportDraftView,
  ExportTestResult,
  Frame,
  FxConfig,
  HistoryRow,
  LevelInfo,
  LibraryFilterArgs,
  LookupSet,
  NowInfo,
  PlaylistHeader,
  PlaylistItemRow,
  QueueRow,
  RotationHeader,
  SchedulerConfig,
  SinkInfo,
  SoundcardsView,
  SoundOptions,
  SongFull,
  SongRow,
  StatusInfo,
  ZoneMix,
  ZoneName,
  ZoneState,
} from "./protocol";
import { dbFromWire, PROTOCOL_VERSION } from "./protocol";

export type ConnState = "connecting" | "open" | "closed";

type Pending = {
  resolve: (data: Record<string, unknown>) => void;
  reject: (err: Error) => void;
};

const RECONNECT_MS = 2500;
const PING_MS = 25_000;

/** Everything the UI knows about the server, updated by pushes. */
class Session {
  // connection bookkeeping (not reactive on purpose: `conn` is).
  private ws: WebSocket | null = null;
  private nextId = 1;
  private pending = new Map<number, Pending>();
  private pingTimer: number | null = null;
  private retryTimer: number | null = null;
  private wantOpen = false;
  private token: string | null = null;

  conn = $state<ConnState>("closed");
  ready = $state<{ server: string; version: string; authenticated: boolean } | null>(null);
  authRequired = $state(false);

  now = $state<NowInfo | null>(null);
  zones = $state<ZoneState[]>([]);
  master = $state<LevelInfo>({ level_db: null, peak_db: null, peak_hold_db: null, rms_db: null });
  masterDb = $state<number>(0);
  mix = $state<ZoneMix[]>([]);
  status = $state<StatusInfo>({ level: "info", message: "Not connected" });

  /** Recent plugin host events (state fan-out / UI requests), newest last. */
  pluginEvents = $state<{ plugin: string; at: string; text: string }[]>([]);

  /** Library/window data loaded on demand by the windows. */
  dbConnected = $state(false);
  /** Increments on every successful DB connect; windows use it to auto-refresh. */
  dbConnects = $state(0);
  songs = $state<SongRow[]>([]);
  playlists = $state<PlaylistHeader[]>([]);
  playlistItems = $state<PlaylistItemRow[]>([]);
  carts = $state<CartHeader[]>([]);
  cartButtons = $state<CartButtonRow[]>([]);
  /** Cartwall button currently rolled on the cart zone (from player frames). */
  cartNow = $state<CartNowInfo | null>(null);
  /** Instant/cart wall button rolled on the instant zone (player frame `instant`). */
  instantNow = $state<CartNowInfo | null>(null);
  /** Wall currently shown on the cart/instant panels (shared by both). */
  selectedCart = $state<number | null>(null);
  /** Grid config behind both walls (`cartwall_get_config` / `_set_config`). */
  cartwallConfig = $state<CartwallConfig | null>(null);
  queue = $state<QueueRow[]>([]);
  history = $state<HistoryRow[]>([]);
  /** Row count of the last library_filter/history_filter (pagination). */
  libraryTotal = $state(0);
  historyTotal = $state(0);

  // ---- settings-editor state (list_devices / *_get / *_set ops) -------------
  /** Output device names from the audio backend. */
  devices = $state<string[]>([]);
  /** The sink currently feeding playout. */
  sink = $state<SinkInfo | null>(null);
  soundcards = $state<SoundcardsView | null>(null);
  soundOptions = $state<SoundOptions | null>(null);
  /** Master sound-processing config (fx_get / fx_set). */
  fx = $state<FxConfig | null>(null);
  /** Now-playing export draft (export_get / export_set). */
  exportDraft = $state<ExportDraftView | null>(null);
  /** Last export_test payload. */
  exportTest = $state<ExportTestResult | null>(null);

  /** AutoDJ switch + knobs (autodj_status / autodj_set). */
  autodjEnabled = $state(false);
  autodjLastPick = $state<number | null>(null);
  autodjConfig = $state<SchedulerConfig | null>(null);

  // Rotation editor state (list_rotations / list_criteria / list_lookup).
  rotations = $state<RotationHeader[]>([]);
  criteria = $state<CriteriaRow[]>([]);
  lookups = $state<LookupSet>({ categories: [], subcategories: [], genres: [] });
  /** Last autodj_plan dry-run payload, shown by the AutoDJ panel. */
  autodjPlan = $state<AutodjPlan | null>(null);

  /** Reactive log of the last error frame, for the toast strip. */
  lastError = $state<string | null>(null);

  constructor() {
    if (typeof window !== "undefined") {
      const hash = new URLSearchParams(window.location.hash.slice(1));
      const url = new URLSearchParams(window.location.search);
      this.token = hash.get("token") ?? url.get("token");
    }
  }

  /** Open the socket (idempotent) and start reconnecting on close. */
  start(): void {
    this.wantOpen = true;
    this.open();
  }

  /** Close the socket now and stop reconnecting. */
  closeSocket(): void {
    this.wantOpen = false;
    if (this.retryTimer !== null) window.clearTimeout(this.retryTimer);
    this.ws?.close();
  }

  private open(): void {
    if (this.ws && (this.ws.readyState === WebSocket.CONNECTING || this.ws.readyState === WebSocket.OPEN)) return;
    this.conn = "connecting";
    const proto = location.protocol === "https:" ? "wss:" : "ws:";
    const ws = new WebSocket(`${proto}//${location.host}/ws`);
    this.ws = ws;
    ws.onmessage = (ev) => this.onFrame(ev.data as string);
    ws.onopen = () => {
      this.conn = "open";
      const hello: ClientMsg = { t: "hello", username: "operator" };
      if (this.token) hello.password = this.token;
      ws.send(JSON.stringify(hello));
      ws.send(JSON.stringify({ t: "sub" } satisfies ClientMsg));
      this.pingTimer = window.setInterval(() => this.send({ t: "ping" }), PING_MS);
    };
    ws.onclose = () => {
      this.conn = "closed";
      if (this.pingTimer !== null) window.clearInterval(this.pingTimer);
      this.pingTimer = null;
      for (const p of this.pending.values()) p.reject(new Error("connection closed"));
      this.pending.clear();
      if (this.wantOpen) this.retryTimer = window.setTimeout(() => this.open(), RECONNECT_MS);
    };
    ws.onerror = () => ws.close();
  }

  private send(msg: ClientMsg): void {
    if (this.ws?.readyState !== WebSocket.OPEN) return;
    this.ws.send(JSON.stringify(msg));
  }

  /** Invoke an op; resolves with the merged `result` payload. */
  cmd<T = Record<string, unknown>>(op: string, args?: unknown): Promise<T> {
    return new Promise<T>((resolve, reject) => {
      if (this.ws?.readyState !== WebSocket.OPEN) {
        reject(new Error("not connected to radiocraft-server"));
        return;
      }
      const id = this.nextId++;
      this.pending.set(id, {
        resolve: resolve as (d: Record<string, unknown>) => void,
        reject,
      });
      this.send({ t: "cmd", id, op, args });
    });
  }

  // ---- transport shortcuts (dispatch.rs op names) -------------------------

  play(songId: number, zone: ZoneName = "main"): Promise<unknown> {
    return this.cmd("play", { songId, zone });
  }
  cue(songId: number, zone: ZoneName = "main"): Promise<unknown> {
    return this.cmd("cue", { songId, zone });
  }
  playFile(path: string, zone: ZoneName = "main"): Promise<unknown> {
    return this.cmd("play", { path, zone });
  }
  segueNow(zone: ZoneName, fade = -1): Promise<unknown> {
    return this.cmd("segue_now", { zone, fade });
  }
  stop(zone: ZoneName = "main", fade = -1): Promise<unknown> {
    return this.cmd("stop", { zone, fade });
  }
  stopAll(fade = -1): Promise<unknown> {
    return this.cmd("stop_all", { fade });
  }
  seek(secs: number, zone: ZoneName = "main"): Promise<unknown> {
    return this.cmd("seek", { zone, secs });
  }
  pause(zone: ZoneName = "main"): Promise<unknown> {
    return this.cmd("pause", { zone });
  }
  resume(zone: ZoneName = "main"): Promise<unknown> {
    return this.cmd("resume", { zone });
  }
  setMasterDb(db: number): Promise<unknown> {
    return this.cmd("set_master_db", { db });
  }
  setZoneFader(zone: ZoneName, db: number): Promise<unknown> {
    return this.cmd("set_zone_fader_db", { zone, db });
  }
  setZoneMuted(zone: ZoneName, muted: boolean): Promise<unknown> {
    return this.cmd("set_zone_muted", { zone, muted });
  }

  // ---- db -----------------------------------------------------------------

  async dbConfig(): Promise<DbConfigView> {
    return (await this.cmd<DbConfigView & Partial<DbConfigView>>("get_db_config")) as DbConfigView;
  }
  configureDb(cfg: Partial<DbConfigView>): Promise<unknown> {
    return this.cmd("db_configure", cfg);
  }
  async connectDb(): Promise<number> {
    const r = await this.cmd<{ songs: number }>("db_connect");
    this.dbConnected = true;
    this.dbConnects++;
    return r.songs;
  }
  async loadSongs(needle?: string): Promise<void> {
    const r = needle
      ? await this.cmd<{ songs: SongRow[] }>("search_songs", { needle, limit: 300 })
      : await this.cmd<{ songs: SongRow[] }>("list_songs", { limit: 300 });
    this.songs = r.songs ?? [];
  }
  async loadPlaylists(): Promise<void> {
    const r = await this.cmd<{ playlists: PlaylistHeader[] }>("list_playlists");
    this.playlists = r.playlists ?? [];
  }
  async loadPlaylistItems(id: number): Promise<void> {
    const r = await this.cmd<{ items: PlaylistItemRow[] }>("list_playlist_items", { playlistId: id });
    this.playlistItems = r.items ?? [];
  }
  async loadCarts(): Promise<void> {
    const r = await this.cmd<{ carts: CartHeader[] }>("list_carts");
    this.carts = r.carts ?? [];
  }
  async loadCartButtons(id: number): Promise<void> {
    const r = await this.cmd<{ buttons: CartButtonRow[] }>("list_cart_buttons", { cartId: id });
    this.cartButtons = r.buttons ?? [];
    this.selectedCart = id;
  }

  // ---- cart/instant wall editor ops (cartwall.rs) --------------------------

  /** Fetch the shared grid config into `cartwallConfig`. */
  async loadCartwallConfig(): Promise<void> {
    this.cartwallConfig = await this.cmd<CartwallConfig>("cartwall_get_config");
  }
  /** Partial update (`cartwall_set_config` echoes the whole config back). */
  async setCartwallConfig(patch: Partial<CartwallConfig>): Promise<void> {
    this.cartwallConfig = await this.cmd<CartwallConfig>("cartwall_set_config", patch);
  }
  async createCartWall(name: string): Promise<number> {
    const r = await this.cmd<{ cartId: number }>("cartwall_create", { name });
    return r.cartId;
  }
  renameCartWall(id: number, name: string): Promise<unknown> {
    return this.cmd("cartwall_rename", { cartId: id, name });
  }
  deleteCartWall(id: number): Promise<unknown> {
    return this.cmd("cartwall_delete", { cartId: id });
  }
  /** Replace a wall's full button layout (only assigned slots are sent). */
  saveCartButtons(cartId: number, buttons: CartButtonSpec[]): Promise<unknown> {
    return this.cmd("cartwall_save_buttons", { cartId, buttons });
  }

  // ---- library edit ops -----------------------------------------------------

  /** Run a library_filter; results land in `songs` + `libraryTotal`. */
  async filterLibrary(args: LibraryFilterArgs): Promise<void> {
    const r = await this.cmd<{ total: number; songs: SongRow[] }>("library_filter", args);
    this.songs = r.songs ?? [];
    this.libraryTotal = r.total ?? 0;
  }
  async getSong(id: number): Promise<SongFull> {
    const r = await this.cmd<{ song: SongFull }>("library_get", { songId: id });
    return r.song;
  }
  updateSong(song: SongFull): Promise<unknown> {
    return this.cmd("library_update", { song });
  }
  deleteSong(id: number): Promise<unknown> {
    return this.cmd("library_delete", { songId: id });
  }
  massSet(ids: number[], field: "enabled" | "weight" | "genre" | "subcat", value: unknown): Promise<unknown> {
    return this.cmd("library_mass_set", { ids, field, value });
  }
  massDelete(ids: number[]): Promise<unknown> {
    return this.cmd("library_mass_delete", { ids });
  }

  // ---- history ops ----------------------------------------------------------

  /** Run a history_filter; results land in `history` + `historyTotal`. */
  async filterHistory(text = "", limit = 200, offset = 0): Promise<void> {
    const r = await this.cmd<{ total: number; history: HistoryRow[] }>("history_filter", {
      text,
      limit,
      offset,
    });
    this.history = r.history ?? [];
    this.historyTotal = r.total ?? 0;
  }
  deleteHistory(id: number): Promise<unknown> {
    return this.cmd("history_delete", { historyId: id });
  }
  playFromHistory(id: number): Promise<unknown> {
    return this.cmd("play_history", { historyId: id });
  }
  /** Park the currently-loaded item in the queue ("play next"). */
  playNext(): Promise<unknown> {
    return this.cmd("play_next");
  }

  // ---- playlist edit ops ------------------------------------------------------

  async createPlaylist(name: string): Promise<number> {
    const r = await this.cmd<{ playlistId: number }>("playlist_create", { name });
    return r.playlistId;
  }
  renamePlaylist(id: number, name: string): Promise<unknown> {
    return this.cmd("playlist_rename", { playlistId: id, name });
  }
  deletePlaylist(id: number): Promise<unknown> {
    return this.cmd("playlist_delete", { playlistId: id });
  }
  playlistAddItem(playlistId: number, songId: number): Promise<unknown> {
    return this.cmd("playlist_add_item", { playlistId, songId });
  }
  playlistRemoveItem(itemId: number): Promise<unknown> {
    return this.cmd("playlist_remove_item", { itemId });
  }
  /** 1-based positions, reindexed afterwards. */
  playlistMoveItem(playlistId: number, from: number, to: number): Promise<unknown> {
    return this.cmd("playlist_move_item", { playlistId, from, to });
  }
  playlistSetCues(
    itemId: number,
    cues: { cstart: number; cnext: number; cend: number; fin: number; fout: number }
  ): Promise<unknown> {
    return this.cmd("playlist_set_cues", { itemId, ...cues });
  }
  playlistSetSong(itemId: number, songId: number): Promise<unknown> {
    return this.cmd("playlist_set_song", { itemId, songId });
  }

  // ---- autodj ops -------------------------------------------------------------

  /** Refresh the AutoDJ switch + knobs from the server. */
  async refreshAutodj(): Promise<void> {
    const r = await this.cmd<AutodjStatus>("autodj_status");
    this.autodjEnabled = r.enabled;
    this.autodjLastPick = r.lastPickSongId ?? null;
    this.autodjConfig = r.config;
  }
  /** Toggle AutoDJ (persisted server-side); optionally retune in one call. */
  async setAutodj(enabled: boolean, config?: Partial<SchedulerConfig>): Promise<void> {
    const args: Record<string, unknown> = { enabled };
    if (config) args.config = config;
    const r = await this.cmd<AutodjStatus>("autodj_set", args);
    this.autodjEnabled = r.enabled;
    this.autodjConfig = r.config;
  }
  /** Dry-run the next planned slot; result lands in `autodjPlan`. */
  async autodjPlanNext(): Promise<void> {
    this.autodjPlan = await this.cmd<AutodjPlan>("autodj_plan");
    this.autodjLastPick = this.autodjPlan.lastPickSongId ?? null;
  }
  /** Plan and roll the next pick now. */
  async autodjPlayNext(): Promise<void> {
    const r = await this.cmd<{ plan: AutodjPlan }>("autodj_play_next");
    this.autodjPlan = r.plan;
    this.autodjLastPick = r.plan.lastPickSongId ?? null;
  }

  // ---- rotation editor ops ----------------------------------------------------

  async loadRotations(): Promise<void> {
    const r = await this.cmd<{ rotations: RotationHeader[] }>("list_rotations");
    this.rotations = r.rotations ?? [];
  }
  async createRotation(name: string): Promise<number> {
    const r = await this.cmd<{ rotationId: number }>("rotation_create", { name });
    return r.rotationId;
  }
  renameRotation(rotationId: number, name: string): Promise<unknown> {
    return this.cmd("rotation_rename", { rotationId, name });
  }
  deleteRotation(rotationId: number): Promise<unknown> {
    return this.cmd("rotation_delete", { rotationId });
  }
  async loadCriteria(rotationId: number): Promise<void> {
    const r = await this.cmd<{ criteria: CriteriaRow[] }>("list_criteria", { rotationId });
    this.criteria = r.criteria ?? [];
  }
  async criteriaAdd(rotationId: number, criteria: Partial<CriteriaRow>): Promise<number> {
    const r = await this.cmd<{ criteriaId: number }>("criteria_add", { rotationId, criteria });
    return r.criteriaId;
  }
  /** Patch is merged onto the stored row server-side; `id` selects the row. */
  criteriaUpdate(criteria: { id: number } & Partial<CriteriaRow>): Promise<unknown> {
    return this.cmd("criteria_update", { criteria });
  }
  criteriaDelete(criteriaId: number): Promise<unknown> {
    return this.cmd("criteria_delete", { criteriaId });
  }
  async loadLookups(): Promise<void> {
    const r = await this.cmd<LookupSet>("list_lookup");
    this.lookups = {
      categories: r.categories ?? [],
      subcategories: r.subcategories ?? [],
      genres: r.genres ?? [],
    };
  }

  // ---- settings-editor ops ----------------------------------------------------

  /** Refresh output devices + the active sink. */
  async loadDevices(): Promise<void> {
    const r = await this.cmd<{ devices: string[]; current: SinkInfo }>("list_devices");
    this.devices = r.devices ?? [];
    this.sink = r.current ?? null;
  }
  /** Switch the live output device (empty/`null` = system default). */
  async setDevice(name: string | null): Promise<void> {
    const r = await this.cmd<{ current: SinkInfo }>("set_device", { name });
    this.sink = r.current;
  }
  async loadSoundcards(): Promise<void> {
    this.soundcards = (await this.cmd<SoundcardsView>("soundcards_get")) as SoundcardsView;
  }
  /** Patch any subset of `{audioMode, inputFadeIn, inputFadeOut}`. */
  async setSoundcards(patch: {
    audioMode?: number;
    inputFadeIn?: number;
    inputFadeOut?: number;
  }): Promise<void> {
    this.soundcards = (await this.cmd<SoundcardsView>("soundcards_set", patch)) as SoundcardsView;
  }
  async loadSoundOptions(): Promise<void> {
    this.soundOptions = (await this.cmd<SoundOptions>("sound_options_get")) as SoundOptions;
  }
  /** Patch any subset of the `settings_general.xml` knobs. */
  async setSoundOptions(patch: Partial<SoundOptions>): Promise<void> {
    this.soundOptions = (await this.cmd<SoundOptions>("sound_options_set", patch)) as SoundOptions;
  }
  async loadFx(): Promise<void> {
    const r = await this.cmd<{ config: FxConfig }>("fx_get");
    this.fx = r.config;
  }
  /** Patch any subset of the master sound-processing config; the server
   *  persists and live-applies the chain. */
  async setFx(patch: Partial<FxConfig>): Promise<void> {
    const r = await this.cmd<{ config: FxConfig }>("fx_set", patch);
    this.fx = r.config;
  }
  async loadExportDraft(): Promise<void> {
    const r = await this.cmd<{ export: ExportDraftView }>("export_get");
    this.exportDraft = r.export ?? null;
  }
  async saveExportDraft(draft: ExportDraftView): Promise<void> {
    const r = await this.cmd<{ export: ExportDraftView }>("export_set", { export: draft });
    this.exportDraft = r.export ?? draft;
  }
  /** Fire the exporter once (live track or sample) and show what it rendered. */
  async testExport(): Promise<void> {
    this.exportTest = (await this.cmd<ExportTestResult>("export_test")) as ExportTestResult;
  }

  // ---- play queue ops ---------------------------------------------------------

  async loadQueue(limit = 200): Promise<void> {
    const r = await this.cmd<{ items: QueueRow[] }>("list_queue", { limit });
    this.queue = r.items ?? [];
  }
  queueAddSong(songId: number): Promise<unknown> {
    return this.cmd("queue_add_song", { songId });
  }
  queueAddFile(path: string, title?: string): Promise<unknown> {
    return this.cmd("queue_add_file", { path, title });
  }
  queueRemove(id: number): Promise<unknown> {
    return this.cmd("queue_remove", { queueId: id });
  }
  queueClear(): Promise<unknown> {
    return this.cmd("queue_clear");
  }
  /** eta: RFC3339 or local 'YYYY-MM-DD HH:MM:SS'. */
  queueEtaSet(id: number, eta: string): Promise<unknown> {
    return this.cmd("queue_eta_set", { queueId: id, eta });
  }
  /** Reorder by swapping ETAs with another row. */
  queueMove(id: number, withId: number): Promise<unknown> {
    return this.cmd("queue_move", { queueId: id, with: withId });
  }

  // ---- frame handling ------------------------------------------------------

  private onFrame(raw: string): void {
    let f: Frame;
    try {
      f = JSON.parse(raw) as Frame;
    } catch {
      return;
    }
    switch (f.t) {
      case "ready":
        this.authRequired = !f.authenticated;
        this.ready = { server: f.server, version: f.version, authenticated: f.authenticated };
        break;
      case "player":
        this.now = f.now;
        this.zones = f.zones;
        this.cartNow = f.cart;
        this.instantNow = f.instant;
        break;
      case "meters":
        this.master = f.master;
        this.zones = f.zones;
        break;
      case "state": {
        this.masterDb = dbFromWire(f.master_db);
        this.mix = f.zones;
        break;
      }
      case "status":
        this.status = { level: f.level, message: f.message };
        if (f.level === "error" && f.message.toLowerCase().includes("database")) this.dbConnected = false;
        else if (/^Connected to /i.test(f.message)) {
          if (!this.dbConnected) this.dbConnects++;
          this.dbConnected = true;
        }
        break;
      case "result": {
        const p = this.pending.get(f.id);
        if (p) {
          this.pending.delete(f.id);
          const { t: _t, id: _id, op: _op, ...data } = f as Record<string, unknown>;
          p.resolve(data);
        } else if (f.op === "sub") {
          // Snapshot frames pushed right after subscribing (player/state/status).
          const data = f as unknown as {
            player?: Frame;
            state?: Frame;
            status?: Frame;
            db_connected?: boolean;
          };
          if (data.player) this.onFrame(JSON.stringify(data.player));
          if (data.state) this.onFrame(JSON.stringify(data.state));
          if (data.status) this.onFrame(JSON.stringify(data.status));
          if (data.db_connected) {
            if (!this.dbConnected) this.dbConnects++;
            this.dbConnected = true;
          }
        }
        break;
      }
      case "error": {
        if (f.id !== null) {
          const p = this.pending.get(f.id);
          if (p) {
            this.pending.delete(f.id);
            p.reject(new Error(f.message));
          }
        }
        if (f.code === "unauthorized") this.authRequired = true;
        this.lastError = `${f.code}: ${f.message}`;
        break;
      }
      case "plugin_event": {
        const desc = f.state
          ? typeof f.state.feature === "string"
            ? f.state.feature
            : `other(${f.state.feature.other})`
          : f.ui
            ? `ui:${f.ui.kind}`
            : "event";
        this.pluginEvents = [...this.pluginEvents.slice(-49), { plugin: f.plugin, at: new Date().toISOString(), text: desc }];
        break;
      }
      case "pong":
        break;
    }
  }

  /** Per-zone helper for components. */
  zoneState(zone: ZoneName): ZoneState | undefined {
    return this.zones.find((z) => z.zone === zone);
  }
  zoneMix(zone: ZoneName): ZoneMix | undefined {
    return this.mix.find((z) => z.zone === zone);
  }
}

export const session = new Session();
