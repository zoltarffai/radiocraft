/**
 * The radiocraft-server wire protocol, mirroring crates/radiocraft-server/src/frames.rs.
 * Keep this file in sync with frames.rs — the server is the source of truth.
 */

/** dB values arrive as `null` when the value is `-inf` (silence). */
export type DbWire = number | null;

export type ZoneName = "main" | "aux" | "cart" | "instant";
export const ZONES: ZoneName[] = ["main", "aux", "cart", "instant"];

export const PROTOCOL_VERSION = 1;

export interface ZoneState {
  zone: string;
  playing: boolean;
  paused: boolean;
  position_secs?: number;
  remaining_secs?: number;
}

export interface NowInfo {
  song_id?: number;
  artist: string;
  title: string;
  album: string;
  path: string;
  track_type: number;
  duration_secs: number;
  segment_secs?: number;
  /** RFC 3339, server local clock. */
  started_at: string;
}

export interface LevelInfo {
  level_db: DbWire;
  peak_db: DbWire;
  peak_hold_db: DbWire;
  rms_db: DbWire;
}

export interface ZoneMix {
  zone: string;
  fader_db: DbWire;
  muted: boolean;
}

export interface StatusInfo {
  level: string;
  message: string;
}

/** Server → client frames (`t` tag). */
export type Frame =
  | {
      t: "ready";
      protocol: number;
      server: string;
      version: string;
      zones: string[];
      authenticated: boolean;
    }
  | ({ t: "result"; id: number; op: string } & Record<string, unknown>)
  | { t: "error"; id: number | null; code: string; message: string }
  | { t: "player"; now: NowInfo | null; zones: ZoneState[]; cart: CartNowInfo | null }
  | { t: "meters"; master: LevelInfo; zones: ZoneState[] }
  | { t: "state"; master_db: DbWire; zones: ZoneMix[] }
  | ({ t: "status" } & StatusInfo)
  | { t: "pong" };

/** Client → server messages. */
export type ClientMsg =
  | { t: "ping" }
  | { t: "hello"; username?: string; password?: string }
  | { t: "sub" }
  | { t: "unsub" }
  | { t: "cmd"; id: number; op: string; args?: unknown };

/** Cue knob patch (`playback.rs::CueKnobs`, camelCase; 0 = not set). */
export interface CuePatch {
  start?: number;
  next?: number;
  end?: number;
  loopIn?: number;
  loopOut?: number;
  fadeInMs?: number;
  fadeOutMs?: number;
}

export interface PlayArgs {
  zone?: ZoneName;
  songId?: number;
  path?: string;
  title?: string;
  cue?: CuePatch;
}

export interface SongRow {
  ID: number;
  path: string;
  artist: string;
  title: string;
  album: string;
  song_type: number;
  duration: string;
  cue_times: string;
  weight: string;
  count_played: number;
  enabled: boolean;
  id_genre: number;
  id_subcat: number;
  year: string;
  artist_played?: string | null;
  title_played?: string | null;
  album_played?: string | null;
  date_played?: string | null;
}

/**
 * A full `songs` row (`entities.rs::Song`) for library_get / library_update.
 * `library_update` echoes the whole row back, so unknown fields are kept
 * verbatim by spreading the loaded row before editing.
 */
export type SongFull = SongRow & {
  enabled: boolean;
  original_artist: string;
  original_duration: string;
  id_genre: number;
  id_subcat: number;
  [key: string]: unknown;
};

/** One `history` row (`entities.rs::HistoryEntry`). */
export interface HistoryRow {
  ID: number;
  trackID: number;
  date_played?: string | null;
  song_type: number;
  id_subcat: number;
  id_genre: number;
  duration: string;
  artist: string;
  original_artist: string;
  title: string;
  album: string;
  composer: string;
  label: string;
  year: string;
  track_no: number;
  disc_no: number;
  publisher: string;
  copyright: string;
  isrc: string;
  listeners?: number | null;
}

/** One `queuelist` row (`entities.rs::QueueItem`). */
export interface QueueRow {
  ID: number;
  songID: number;
  ETA?: string | null;
  duration: string;
  artist: string;
  associated_artists: string;
  title: string;
  album: string;
  swID: number;
  swPlay: string;
  vtID: number;
  vtPlay: string;
}

/** `library_filter` args (`dispatch.rs::library_filter_args`). */
export interface LibraryFilterArgs {
  text?: string;
  idCategory?: number;
  idSubcat?: number;
  idGenre?: number;
  idRotation?: number;
  enabled?: boolean | null;
  sort?: "artist" | "title" | "album" | "duration" | "year" | "weight" | "count";
  limit?: number;
  offset?: number;
}

export interface PlaylistHeader {
  ID: number;
  name: string;
}

export interface PlaylistItemRow {
  ID: number;
  pID: number;
  sID: number;
  cstart: string;
  cnext: string;
  cend: string;
  fin: string;
  fout: string;
  swID: number;
  swplay: string;
  vtID: number;
  vtplay: string;
  swfirst: boolean;
  ord: number;
  artist?: string | null;
  title?: string | null;
  path?: string | null;
}

export interface CartHeader {
  ID: number;
  name: string;
}

/** Cartwall button currently rolled on the cart zone (player frame `cart`). */
export interface CartNowInfo {
  /** carts_list.ID of the button. */
  button_id: number;
  /** carts.ID of the cart wall it belongs to. */
  cart_id: number;
  /** songs.ID of the linked track. */
  song_id: number;
  title: string;
}

export interface CartButtonRow {
  ID: number;
  pID: number;
  swID: number;
  swButton: number;
  color: string;
  artist?: string | null;
  title?: string | null;
  path?: string | null;
  duration?: string | null;
}

/** Tuning knobs the scheduler applies when planning (autodj_* ops). */
export interface SchedulerConfig {
  respect_play_limit: boolean;
  respect_repeat_intervals: boolean;
  respect_availability: boolean;
  forced_song_type: number | null;
  max_candidates: number | null;
}

/** AutoDJ state (autodj_status / autodj_set results). */
export interface AutodjStatus {
  enabled: boolean;
  lastPickSongId: number | null;
  config: SchedulerConfig;
}

/** Planned song slot (autodj_plan `pick`). */
export interface AutodjPick {
  songId: number;
  artist: string;
  title: string;
  album: string;
  path: string;
  duration: number;
  weight: number;
  sweeperBefore: AutodjSweeper | null;
  sweeperWithin: AutodjSweeper | null;
  voiceTrack: AutodjSweeper | null;
  sweeperOffset: number;
}

export interface AutodjSweeper {
  songId: number;
  title: string;
  path: string;
  duration: number;
}

/** One candidate the scheduler rejected, with the reason. */
export interface AutodjRejection {
  songId: number;
  reason: string;
}

/** autodj_plan / autodj_play_next `plan` payload. */
export interface AutodjPlan {
  rotationId: number | null;
  rotationName: string | null;
  criteriaId: number | null;
  pick: AutodjPick | null;
  ordered: Array<{ songId: number; title: string; artist: string }>;
  rejected: AutodjRejection[];
  lastPickSongId: number | null;
}

export interface DbConfigView {
  host: string;
  port: number;
  database: string;
  username: string;
  password: string;
}

/** Decode a dB wire value: `null` means digital silence. */
export function dbFromWire(v: DbWire): number {
  return v === null || v === undefined ? -Infinity : v;
}
