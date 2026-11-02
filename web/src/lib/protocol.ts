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
  /** Live ICY broadcast title while an internet stream plays. */
  stream_title?: string;
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

/** Host state-change fan-out (`host.rs::StateChange`, snake_case). */
export type StateChangeName =
  | "player_started"
  | "player_ended"
  | "cart_started"
  | "cart_ended"
  | "currently_playing_changed"
  | "auto_dj_enabled"
  | "auto_dj_disabled"
  | "assisted_enabled"
  | "manual_enabled"
  | "paused"
  | "resumed"
  | "playlist_changed"
  | "tracks_reloaded"
  | "timer"
  | { other: number };

/** Payload of a plugin state change (`host.rs::StateInfo`). */
export interface StateInfo {
  feature: StateChangeName;
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  data?: any;
}

/** UI window a plugin asks the host to open (`host.rs::PluginUiRequest`). */
export type PluginUiRequest =
  | { kind: "track_details"; track: Record<string, unknown>; refresh_playlist: boolean; position: number }
  | { kind: "track_details_window"; track: Record<string, unknown> }
  | { kind: "track_editor"; track: Record<string, unknown>; plugin_name: string; position: number }
  | { kind: "categories_editor" }
  | {
      kind: "tracks_window";
      plugin_name: string;
      track_type: number;
      max_track_duration: number;
      position: number;
    }
  | { kind: "import_folder" };

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
  | {
      t: "player";
      now: NowInfo | null;
      zones: ZoneState[];
      cart: CartNowInfo | null;
      instant: CartNowInfo | null;
    }
  | { t: "meters"; master: LevelInfo; zones: ZoneState[] }
  | { t: "state"; master_db: DbWire; zones: ZoneMix[] }
  | ({ t: "status" } & StatusInfo)
  | {
      t: "plugin_event";
      plugin: string;
      state?: StateInfo;
      ui?: PluginUiRequest;
    }
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

/** Grid config of the cart/instant walls (`settings_cartwall.xml`). */
export interface CartwallConfig {
  columns: number;
  rows: number;
  /** Fade length in seconds when a rolled button is pressed again. */
  fade: number;
  autoRoll: boolean;
}

/** One row of a wall layout as `cartwall_save_buttons` takes it. */
export interface CartButtonSpec {
  slot: number;
  songId: number;
  color: string;
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

/** `list_rotations` row. */
export interface RotationHeader {
  ID: number;
  name: string;
}

/** `list_lookup` picker row (category/subcategory/genre). */
export interface LookupRow {
  ID: number;
  name: string;
  parentid?: number;
}

/** `list_lookup` payload. */
export interface LookupSet {
  categories: LookupRow[];
  subcategories: LookupRow[];
  genres: LookupRow[];
}

export type CategoryPolarity = "Exclude" | "Include";

/** Scheduler ordering strategy (`SelectionMode`). */
export type SelectionMode = "Weight" | "LeastPlayed" | "Oldest" | "Newest" | "Random";

/**
 * One rotation rule line, the full core `RotationCriteria` shape as carried
 * in `rotations_list.data`. `*_id = 0` means "any"; times are "HH:MM:SS"
 * (daypart `null` = all day); `days` is a bitmask, bit 0 = Sunday, 127 = all.
 */
export interface CriteriaRow {
  id: number;
  rotation_id: number;
  name: string;
  enabled: boolean;
  category_id: number;
  category_type: CategoryPolarity;
  subcategory_id: number;
  subcategory_type: CategoryPolarity;
  genre_id: number;
  weight_min: number;
  repeat_track_interval: number;
  repeat_artist_interval: number;
  repeat_title_interval: number;
  repeat_album_interval: number;
  time_start: string | null;
  time_end: string | null;
  days: number;
  language: string | null;
  mood: string | null;
  gender: string | null;
  selection: SelectionMode;
  /** 0 = never repeat the same track back-to-back. */
  repeat_rule: number;
  ord: number;
}

export interface DbConfigView {
  host: string;
  port: number;
  database: string;
  username: string;
  password: string;
}

/** Active audio sink (`radiocraft-audio::device::SinkInfo`). */
export interface SinkInfo {
  name: string;
  sample_rate: number;
  channels: number;
  is_device: boolean;
}

/** `soundcards_get` / `soundcards_set` payload. */
export interface SoundcardsView {
  audio_mode: number;
  input_fade_in: number;
  input_fade_out: number;
  audio_device: string;
  current: SinkInfo;
}

/** `sound_options_get/set` payload (keys of `settings_general.xml`). */
export interface SoundOptions {
  volume_out: number;
  dur_fadeout: number;
  vol_overlay: number;
  vol_onsweeper: number;
  vol_vt: number;
}

/** Master sound processing (`fx_get` / `fx_set`), keys named like the
 *  RadioDJ Plugin_SoundProcessing settings. */
export interface FxConfig {
  CompressorEnabled: boolean;
  eqON: boolean;
  AGCEnabled: boolean;
  StereoEnhancerEnabled: boolean;
  fxCompByPreset: boolean;
  /** 0 Default, 1 Hard, 2 Hard2, 3 HardCommercial, 4 Medium, 5 Soft, 6 Soft2 */
  fxCompPreset: number;
  /** AGC strength: 0 Hard, 1 Medium, 2 Soft */
  fxDampPreset: number;
  /** dBFS, -60..0 */
  cThreshold: number;
  cRatio: number;
  /** ms, 1..500 */
  cAttack: number;
  /** ms, 50..3000 */
  cRelease: number;
  /** makeup gain dB, -60..60 */
  cGain: number;
  /** output trim, linear percent 0..100 */
  vol_Out: number;
  /** stereo enhancer amount, 1..10 */
  fxStereo: number;
  /** 10 peaking-band gains in dB (32/63/125/250/500/1k/2k/4k/8k/16k) */
  eqValues: number[];
}

/** Now-playing export form (`radiocraft-net::nowplaying::ExportDraft`). */
export interface ExportDraftView {
  export_to_file: boolean;
  target_path: string;
  /** 0 = ANSI, 1 = UTF-8, 2 = UTF-16. */
  encoding: number;
  txt_file_template: string;

  export_to_web: boolean;
  web_urls: string[];
  web_methods: string[];
  target_data: string;

  export_to_serial: boolean;
  serial_template: string;
  target_port: string;
  serial_baud: number;
  serial_parity: number;
  serial_stop: number;
  serial_data: number;
  serial_dtr: boolean;
  serial_rts: boolean;

  enable_custom_title: boolean;
  custom_title: string;
  /** `0;1;5;` style track-type list kept as raw text. */
  types_text: string;
}

/** `export_test` payload. */
export interface ExportTestResult {
  snapshot: Record<string, unknown>;
  file: boolean;
  web: boolean;
  serial: boolean;
}

/** Decode a dB wire value: `null` means digital silence. */
export function dbFromWire(v: DbWire): number {
  return v === null || v === undefined ? -Infinity : v;
}
