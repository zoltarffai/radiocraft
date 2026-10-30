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
  | { t: "player"; now: NowInfo | null; zones: ZoneState[] }
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
  artist_played?: string | null;
  title_played?: string | null;
  album_played?: string | null;
  date_played?: string | null;
}

export interface HistoryRow {
  ID: number;
  song_id: number;
  station: string;
  artist: string;
  title: string;
  datetime_play: string;
  segment_length: string;
  daypart?: string;
  tod?: string;
  artist_played?: string | null;
  album_played?: string | null;
  title_played?: string | null;
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
