//! Reactive session: the WebSocket connection plus the state it feeds.
//!
//! One long-lived socket shared by every panel; commands are correlated by
//! id and server pushes update the `$state` fields components read.

import type {
  CartButtonRow,
  CartHeader,
  ClientMsg,
  DbConfigView,
  Frame,
  HistoryRow,
  LevelInfo,
  LibraryFilterArgs,
  NowInfo,
  PlaylistHeader,
  PlaylistItemRow,
  QueueRow,
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

  /** Library/window data loaded on demand by the windows. */
  dbConnected = $state(false);
  /** Increments on every successful DB connect; windows use it to auto-refresh. */
  dbConnects = $state(0);
  songs = $state<SongRow[]>([]);
  playlists = $state<PlaylistHeader[]>([]);
  playlistItems = $state<PlaylistItemRow[]>([]);
  carts = $state<CartHeader[]>([]);
  cartButtons = $state<CartButtonRow[]>([]);
  queue = $state<QueueRow[]>([]);
  history = $state<HistoryRow[]>([]);
  /** Row count of the last library_filter/history_filter (pagination). */
  libraryTotal = $state(0);
  historyTotal = $state(0);

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
