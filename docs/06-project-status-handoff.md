# Project status & handoff — RadioCraft

Snapshot date: 2026-11-02. Latest code commits: `80ff7a2` (packaging: tauri
deb/rpm bundles built in the rc-tauri container; `scripts/package-linux.sh` +
`scripts/cargo-tauri` CLI shim + FHS web-dist fallback) on top of `a14ce31`
(aux players backend: `Zone::AuxPlayer(u8)` dynamic zones + `auxplayers.rs` op
set, `Plugin_AuxPlayers` parity) + `34c7c7f` (web aux players panel),
`d1c0e7d` (docs), `608511b` (web analyze progress), `b740530`/`35ea625`
(.NET G15 number formatting), `e5bbcf9` (analyze-tracks sweep), `d8b5775`
(cue-point detection port), `8aee5ac` (stream TLS fallback), `688b1c2`
(stream smoke fixes), `11e4c45` (internet streams Type 5), `1b5c493`
(cart-wall editor), `835a86f`+`1eb2ed4` (instant/cart-wall
playback), `281a1bb` (cart wall CRUD), and the master FX commit `4317b15`.
No tags exist yet; branch is `master`. This document is the
single source of truth for "where everything stands" when a fresh session
starts. Read it together with:

- `docs/04-architecture.md` — v0.1 locked architecture (crates, audio, plugin ABI)
- `docs/05-v0.2-web-ui-architecture.md` — v0.2 locked contract (topology, WS/REST, milestones)
- `docs/01…03` — reverse-engineered specs (DB schema, feature ledger, plugin ABI)
- `analysis/decompiled-csharp/` — authoritative C# behavior reference (ilspycmd decompile of
  RadioDJ.exe + all 13 plugins + PluginInterface)

## 1. Objective

Recreate **RadioDJ v3.0.0.5** (Windows/.NET 4.8 radio playout app) as
**RadioCraft**, a Rust rebuild, in `/home/john/Dev/radiocraft/`.

- Target: `RadioDJ.exe` — managed IL only, .NET 4.8, assembly version 3.0.0.5,
  93 types / 8083 methods, sha256 `ddcd8f1686e3a6ae956e7b54aeeff4d9475c9fda61ef7fcb7e0d4ae1568d6e1f`,
  MVID `e6a773fd-f670-4112-aca0-96c91dfa013a`.
- v0.1 (done): faithful engine/db/settings/net/plugin layers, egui GUI, Windows exe verified
  under Wine, AppImage built.
- v0.2 (scope locked by user): **Linux native only** (Windows/macOS targets dropped), Rust
  engine + **web technology for presentation**, "more polished and modern UI", and BOTH shells
  with the same UI code: a native Tauri window and a plain browser mode. v0.2 is now
  feature-complete (milestones below); "all features of RadioDJ" remains the long-term goal.

## 2. Current state (as of `80ff7a2`)

Everything is committed; working tree clean. All tests green, clippy `-D warnings` clean
for every crate except `radiocraft-window` on the host (see §7 for the exclude rule).
Post-milestone feature-parity track (all in §5/§8): plugin hosting `eaf8170`+`27e3da6`+
`577596f`+`77bafdb`, master sound FX `4317b15`, cart wall config/CRUD `281a1bb`,
instant/cart-wall playback `835a86f`+`1eb2ed4`, cart-wall editor `1b5c493`, internet
streams Type 5 with ICY live titles `11e4c45`, stream smoke fixes `688b1c2`, stream TLS
interop fallback to OpenSSL `8aee5ac`, cue-point detection port `d8b5775`, analyze-tracks
sweep `e5bbcf9`, .NET G15 number formatting `35ea625` + duration-decimal fix `b740530`,
web analyze progress `608511b`, aux players (`Plugin_AuxPlayers` parity) `a14ce31` +
`34c7c7f` (see §8), deb/rpm bundles for the window shell `80ff7a2` (see §7).

| v0.2 milestone (docs/05 numbering) | Status | Commit |
|---|---|---|
| 1. `radiocraft-server` skeleton (WS/REST over engine) | ✅ | `4442143` |
| 2. Web UI dashboard parity (transport, playlists, carts) | ✅ | `d452832`, `e1387db` |
| 2.5 snapshot/status/db_connected, library auto-load | ✅ | `0cd648d` |
| CI/packaging Linux-only; AppImage wraps server+web | ✅ | `6d7e0fc` |
| egui `radiocraft-app` retired, logic migrated | ✅ | `10707cf` |
| 4. Tauri window shell + tray; close policy | ✅ | `717c412` |
| 4b. deb/rpm bundles for the window shell | ✅ installed + smoke-tested | `80ff7a2` |
| 5. LAN/token auth mode | ✅ verified live | (token mode predates `717c412`) |
| 3. Library/playlist/history window parity | ✅ | `24a20e9`, `d5f72d9`, `0904464` |

Verified live (desktop session): window opens against live server + MariaDB; play → booking
written to `history`+`songs`; close→tray honors `MinimizeToTray=True`; close with `False`
exits; spawn mode SIGTERMs child which shuts down gracefully (`SIGTERM received`, port
released). Token mode: wrong token → `{"code":"unauthorized"}` + close; correct token OK;
no-`hello` client receives only `ready`.

## 3. Workspace layout

Cargo workspace (8 crates, all Linux):

| crate | LOC | role |
|---|---|---|
| `radiocraft-core` | ~3.0k | domain model, AutoDJ scheduler, events, `$var$` templates, booking rules |
| `radiocraft-db` | ~2.7k | sqlx/MySQL, entities, `UpdateTracks2` port, schema bootstrap, settings store, backup; `PlaylistItemRow`/`CartButtonRow` joins |
| `radiocraft-audio` | ~4.6k | playout engine: symphonia decode, cpal ALSA sink, mixer, cue/crossfade, VDF, FX, BS.1770-4 loudness, waveform cache, recorder; `start_pump()` thread; getters |
| `radiocraft-settings` | ~1.3k | `settings_*.xml` (NewDataSet layout), language tables; `DatabaseSettings` incl. `MinimizeToTray`, `FILE: settings_database.xml` |
| `radiocraft-net` | ~1.7k | RadioDJ's own now-playing exporters (file/web/serial — NOT replaced by axum server), `autost.rs` (`CloseAction`/`ClosePolicy`/`close_action_label`), `nowplaying.rs` (`encoding_code`, `ExportDraft`), AutoBackup |
| `radiocraft-plugin` | ~3.0k | plugin ABI: Host/Plugin traits + cdylib C-ABI vtable mirroring rdjInterface (70 optional slots, NULL-tolerant, catch_unwind fences, JSON payloads) |
| `radiocraft-plugin-example` | ~0.2k | reference cdylib plugin / ABI fixture (Echo, Remember/Recall, CountStates), loaded by `tests/plugin_dylib_e2e.rs` |
| `radiocraft-server` | ~6.6k | axum WS/REST + static web host. modules: `frames auth autodj cartwall dispatch frames grid http playback playlist plugin_host pump rotations run settings soundfx state ws` + tests `commands.rs http_e2e.rs plugin_dylib_e2e.rs settings_ops.rs ws_e2e.rs` (47 lib tests) |
| `radiocraft-window` | ~240 | Tauri 2 shell (see §5) |

Non-cargo: `web/` — Svelte 5 + Vite 8 (rolldown) + TypeScript UI.
`web/src/lib/`: `protocol.ts` (wire), `session.svelte.ts` (WS client state, reads `?token=`),
`App.svelte`, and panels `NowPlaying` `Meter` `ZoneMixer` `CartWall` `Library` `Database`
`HistoryPanel` `QueuePanel` `PlaylistEditor` `AutoDJPanel` `RotationEditor` `SettingsPanel`
`PluginPanel`.

Artifacts: `dist/RadioCraft-0.1.0-x86_64.AppImage` (v0.1; AppImage script `scripts/build-appimage.sh`
now packages radiocraft-server + web/dist).

## 4. Runtime topology (v0.2)

```
radiocraft-server (single source of truth, owns engine + DB)
   └─ HTTP :8030 — serves web/dist, WS /ws, REST /api/*
radiocraft-window (Tauri webview) ──┐ same UI, connects via WS
browser (Brave etc.) ───────────────┘
```

- Server bootstrap shared in `radiocraft-server/src/run.rs`: `default_config_dir()`
  (XDG_CONFIG_HOME → `$XDG_CONFIG_HOME/radiocraft`, else `~/.config/radiocraft`),
  `resolve_web_dist(fallbacks)` (env `RADIOCRAFT_WEB_DIST` → sibling `web/dist` →
  `../../web/dist`), `serve(app, auth, web_dist, bind)` returns actual port, port `0` works.
- Auth: optional `RADIOCRAFT_AUTH_TOKEN` env var; token compared as SHA-256 (constant-time);
  sent as `password` field of `hello` frame; localhost-with-no-token = open.
- Window shell modes: positional arg = config dir; `--url`/`RADIOCRAFT_UI_URL` = pure attach
  (no child); otherwise reuse a listening server on `http://127.0.0.1:8030/` else spawn
  sibling `radiocraft-server` binary and SIGTERM it on `RunEvent::Exit` (books rolling item).
  Forwards token as `?token=` query. Tray: Show/Exit menu, left-click toggle;
  `CloseRequested` → `ClosePolicy::on_close` → HideToTray (`api.prevent_close()`) or exit.

## 5. Wire contract (authoritative; also `web/src/lib/protocol.ts`)

WS frames (JSON): server sends `{"t":"ready"}` first. Client (optional) `{"t":"hello",
"password":"…"}`. Client `{"t":"sub"}` → server replies result frame id 0 with snapshot
`{player, state, status, db_connected}`, then pushes `player` / `state` / `status` /
`meters` (20 Hz). Commands: `{"t":"cmd","id":N,"op":"…","args":{…}}` — **params nest under
`args`**. Errors: `{"code":"unauthorized"}` closes socket. There is **no `get_zonestate`**
op — state is pushed. `sub`-gated pushes only.

Ops (dispatch.rs, ~90): `play pause resume stop stop_all stop_at seek segue_now cue
get_setting set_setting set_zone_fader_db set_zone_muted set_master_db get_db_config
db_configure db_connect list_songs search_songs list_playlists list_playlist_items
list_carts list_cart_buttons list_history play_cart play_instant
cartwall_get_config cartwall_set_config cartwall_create cartwall_rename cartwall_delete
cartwall_save_buttons` — plus the milestone-3 parity set:
library `library_filter library_get library_add library_update library_delete
library_mass_set library_mass_delete`; history `history_filter history_add history_delete
play_history` (rewind = re-add to library, returns `{"restored":id}`); playlists
`playlist_create playlist_rename playlist_delete playlist_add_item playlist_remove_item
playlist_move_item playlist_set_cues playlist_set_song`; queue
`list_queue queue_add_song queue_add_file queue_remove queue_clear queue_eta_set
queue_move` (ETA swap, args `{queueId,with}`); transport extra `play_next`; AutoDJ
`autodj_set` (args `{enabled, config?}` → `{enabled,config}`), `autodj_status`
(`{enabled,lastPickSongId,config}`), `autodj_plan` (dry run → `{rotationId,rotationName,
criteriaId,pick,ordered,rejected,lastPickSongId}`), `autodj_play_next` (`{now,plan}`);
tracks `analyze_tracks` (background sweep, see §8).

Cart play parity (`play_cart`, mirrors `Plugin_InstantPlayers/CartButtonControl.set_PlayCart`):
args `{buttonId}` or `{cartId,"number":slot}` (+ optional `fade` ms). Press rolls the linked
`songs` row (via `carts_list.swID`) on the **cart zone** at position 0 and books history
immediately at start (lightweight `UpdateHistory` insert — no rotation counters, no
`UpdateTracks2`); pressing the rolled button again stops it with a fade. Carts never enter
the main now-playing hub; the cart zone auto-ducks main via the mixer. The `player` frame
carries `cart: {button_id,cart_id,song_id,title}|null` for the wall highlight; `stop` on the
cart zone or natural finish clears it. Results: `{cart,historyId,duration_secs}` or
`{stopped:true,buttonId}`. Covered live by `tests/commands.rs::live_cart_play`.

Cart wall CRUD (`cartwall` module, `src/cartwall.rs`): config lives in
`settings_cartwall.xml` (KvStore, keys `Columns`=4, `Rows`=6, `StopFadeDuration`=1s,
`AutoRoll` — names follow Plugin_InstantPlayers' FormConfig defaults; grid capacity is
Columns×Rows). Ops `cartwall_get_config` / `cartwall_set_config` (partial), and CRUD over
the plain `carts` + `carts_list` tables (no instant-specific table exists in RadioDJ):
`cartwall_create {name}` → `{cartId}`, `cartwall_rename {cartId,name}`, `cartwall_delete
{cartId}` (also clears `carts_list` rows and the rolled highlight), `cartwall_save_buttons
{cartId, buttons:[{slot,songId,color}]}` delete-then-insert in one transaction with
slot/song validation. `list_carts` / `list_cart_buttons` feed the UI. Wall changes fan out
to plugins as `ConfigChanged`.

Instant wall play parity (`play_instant`, Plugin_InstantPlayers): same args as `play_cart`.
The instant zone carries its own now-slot (`App.instant_now`, same shape as the cart one),
and — unlike the cart zone's automatic mixer duck — pressing a button ducks the main
program for the track's full duration the way `CartButtonControl` does: `App` keeps
`duck_until` and calls `engine.set_duck_db` (level = `engine_config.duck_db` when already
negative, else −10 dB), the pump expires the duck at track end, and any `stop`/`stop_all`
releases it early. Toggle-stop fades with `StopFadeDuration` seconds. Plugin fan-out
reuses `CartStarted`/`CartEnded` with `{"id":<button_id>,"plugin":true}` so subscribers can
tell instant button ids from carts. The `player` frame gained `instant: {button_id,
cart_id,song_id,title}|null` beside `cart`. Results match `play_cart` shapes. Web: the
`CartWall.svelte` panel takes a `zone` prop and is mounted twice in `App.svelte` (Carts +
Instant wall), sharing wall selection/buttons via `session.selectedCart`; session state
`instantNow` feeds the second grid's highlight. Covered live by
`tests/commands.rs::live_instant_play`.

Key result shapes: `{songs}`, `{total,songs}`, `{history}`, `{total,history}`,
`{playlists}`, `{items}`, `{playlistId}`, `{song}`, `{songId}`, `{itemId}`, `{queued}`,
`{restored}`, `{updated}`, `{removed}`. `playlist_remove_item` refuses itemId 0
(locked now-playing row); `playlist_move_item` uses 1-based `{from,to}` then reindexes;
`queue_add_file` uses sentinel `swID:-1`/`vtID:-1`; `queue_eta_set` eta accepts RFC3339
or local `YYYY-MM-DD HH:MM:SS`. `queuelist` has no cue/fade columns — queue edits are
ETA/order/swap only.

REST mirrors: `/api/state`, `/api/songs`, `/api/playlists`, `/api/playlists/{id}/items`,
`/api/carts`, `/api/carts/{id}/buttons`, `/api/history` — plus the universal command
mirror `POST /api/cmd/{op}` with the op's args as the JSON body (empty body = no args).
It runs the same `dispatch()` as the WS surface: a `result` frame's flat payload is the
200 response body; failures answer 400 `{code:"cmd_failed", message}` and unparsable
bodies 400 `{code:"bad_json"}`. Every WS op is therefore reachable over REST.
When `RADIOCRAFT_AUTH_TOKEN` is set the whole `/api/*` surface (GET reads and cmd posts)
requires `Authorization: Bearer <token>` or `X-Auth-Token`, answering 401 otherwise;
`/health` stays open. Tests: `crates/radiocraft-server/tests/http_e2e.rs`.

Cue times: `CueKnobs` in `radiocraft-server/src/playback.rs` (camelCase, 0 = not set).
RadioDJ `cue_times` DB string: `&sta= &int= &pin=/&pou= &hin=/&hou= &out= &xta= &end=
&fin=/&fou=` (fade ms), decimal comma tolerated — `parse_cue_times` implements this.

## 6. Engine invariants (test-guarded — do not touch)

- One-transaction `UpdateTracks2` booking: artist/title/album_played for track types 0/9;
  `limit_action` 1=disable, 2=delete, negative=id_subcat relocate; weight decay
  `weight-pWeight` if ≥0. Sentinels `'2002-01-01 00:00:01'`, `'&'`, `'1900'`. Signed `-1`
  queuelist sentinels. Track types 0..14 per docs/03 §6.
- Stop fade: `-1` = 3500 ms default, `-999` = hard cut.
- Casing quirk: `playlists_list` uses lowercase `swplay`/`vtplay`; `queuelist` uses
  `swPlay`/`vtPlay` (schema casing differs on purpose).
- Plugin ABI leaks by design: `HostHandle::new` leaks `Box<&dyn Host>`, `init_shim` leaks
  `HostBridge` — never rebuild host handles per-plugin.
- BS.1770-4: 0.5-amp dual-mono 1 kHz tone = −6.0 LUFS (0 LUFS needs amp ≈ 0.637).
- Server `main.rs` must call `app.engine.start_pump()` (regression test
  `pump_thread_advances_playback`).

## 7. Build & verification infrastructure

- Host: Nobara 44 (KDE Plasma, Wayland + X11 `:0`), glibc 2.43, 16 CPU/14 GB (do release
  builds sequentially), sudo requires a password. Node v24.22.2 / npm 11.19.1. `export PATH=$HOME/.cargo/bin:$PATH`.
- **podman** (docker absent). Images: `localhost/manyrust` (AlmaLinux 8.10, glibc 2.28, for
  AppImage), `localhost/rc-tauri` (built from `.github/Dockerfile.fedora-tauri`, fedora:44 +
  webkit2gtk4.1/gtk3/libappindicator-gtk3/librsvg2/alsa-lib devel + cargo-clippy + nodejs +
  global `@tauri-apps/cli@2.12` + `/usr/local/bin/cargo-tauri` shim + rpm-build + dpkg, so
  `cargo tauri build` works inside it).
- Tauri compile flow (host lacks the -devel packages):
  1. Fresh stage copy — NEVER share host `target/` with container:
     `rm -rf /tmp/rc-tauri-ctx && tar -c -C . Cargo.toml Cargo.lock crates scripts .github | tar -x -C /tmp/rc-tauri-ctx`
  2. `podman build -f .github/Dockerfile.fedora-tauri -t localhost/rc-tauri .` (in that dir)
  3. `podman run --rm -v .:/src:Z -v rc-window-target:/src/target:Z localhost/rc-tauri cargo build -p radiocraft-window`
     (podman volume `rc-window-target` keeps incremental caches; copy resulting `Cargo.lock` back)
  4. Extract binary: `podman create -v rc-window-target:/src/target localhost/rc-tauri true`
     then `podman cp <ctr>:/src/target/debug/radiocraft-window …`.
- CI (`.github/workflows/ci.yml`): `test (linux)` = ubuntu + apt deps
  (`libasound2-dev libx11-dev pkg-config libwebkit2gtk-4.1-dev libgtk-3-dev
  libayatana-appindicator3-dev librsvg2-dev`) → `cargo test --workspace` +
  `cargo clippy --workspace --all-targets -- -D warnings`; `appimage` job builds web then
  `CONTAINER=docker IMAGE=manyrust bash scripts/build-appimage.sh`.
- Host now has the native window toolchain (2026-11-03: `dbus-devel clang
  glib2-devel gtk3-devel webkit2gtk4.1-devel libappindicator-gtk3-devel
  librsvg2-devel` installed via sudo dnf), so `cargo build/test/clippy
  --workspace` pass on the host without exclusions. On a box without those
  packages fall back to `--exclude radiocraft-window` for test/clippy
  (documented in docs/05). sudo still requires a password (no passwordless
  sudo).
- Packaging gotchas: linuxdeploy corrupts musl static binaries → manylinux glibc-dynamic only;
  `APPIMAGE_EXTRACT_AND_RUN=1` in containers.
- deb/rpm bundles: `bash scripts/package-linux.sh` (builds server + web on host, stages a
  clean copy under /tmp/rc-pkg, runs `cargo tauri build -b deb rpm` in `localhost/rc-tauri`
  with the `rc-window-target` volume, copies bundles to `dist/`, then verifies layout with
  dpkg-deb/rpm -qlp). Install smoke passed: both packages install in the container and the
  installed `/usr/bin/radiocraft-server` finds its web UI at `/usr/lib/RadioCraft/web/dist`.

## 8. Remaining work / known gaps

Milestone 3 (UI surface parity) is closed: library filter/sort/paging + edit form +
mass-select ops, playlist editing (create/rename/delete/add/remove/move/cues), queue
panel (add/remove/clear/ETA/swap), history window (filter/delete/rewing play,
`play_history`), `play_next`. Covered end-to-end by `crates/radiocraft-server/tests/
commands.rs` (offline op-surface tests + live-DB round trip gated on `RC_DB_TEST_DSN`,
see §9).

AutoDJ is now wired end-to-end: `radiocraft-db::autodj` executes rotation criteria SQL into
`TrackCandidate`s, `radiocraft-server::autodj` plans slots with the core `Scheduler`
(two-phase pool fetch), the pump auto-fills the main zone on idle (5 s backoff), and the
web UI has an AutoDJ panel (toggle, knobs, plan preview, play-next). Ops: `autodj_set` /
`autodj_status` / `autodj_plan` / `autodj_play_next`; the switch persists as the
`AutoDJStart` general setting.

Rotation/criteria editors are now closed too: backend ops in `radiocraft-server/src/
rotations.rs` (`list_rotations` / `rotation_create` / `rotation_rename` / `rotation_delete` /
`list_criteria` / `criteria_add` / `criteria_update` / `criteria_delete` / `list_lookup`) with
merge-patch semantics on `criteria_update` and full core `RotationCriteria` JSON stored in
`rotations_list.data`; web panel `web/src/lib/RotationEditor.svelte` (rotation CRUD + rule
editor: category/subcategory/genre pickers with include/exclude polarity, day mask, daypart
times, weight floor, four repeat intervals, selection mode, duplicate). Live WS smoke:
`/tmp/opencode/rc-rotation-smoke.mjs`.

Settings editors (the XML-backed Options surface) are closed: backend ops in
`radiocraft-server/src/settings.rs` (`list_devices` / `set_device` / `soundcards_get` /
`soundcards_set` / `sound_options_get` / `sound_options_set` / `export_get` / `export_set` /
`export_test`) — every write persists to the XML store first, then re-applies live (engine
sink swap via `PlayoutEngine::set_sink`, knob re-apply through `set_general_setting`,
exporter rebuild). The chosen device persists as the `audio_device` extra key in
`settings_soundcards.xml`; the now-playing draft round-trips `Plugins/Plugin_NowPlaying.xml`
(file/web/serial channels, templates, custom title, track-type list). Offline tests:
`crates/radiocraft-server/tests/settings_ops.rs`; live WS smoke:
`/tmp/opencode/rc-settings-smoke.mjs`. Web panel `web/src/lib/SettingsPanel.svelte`
(Sound tab: device picker, sound-card fades, sound-option knobs; Processing tab: master
FX — output level, stereo enhancer, compressor with preset/manual params, 10-band EQ grid,
AGC preset; Now-playing tab: full
export editor with per-row URL/method list and Test-export snapshot readout). Scheduler
options are the AutoDJ panel's knobs (`SchedulerConfig` via `autodj_set`), already shipped.

REST parity is closed too: `POST /api/cmd/{op}` in `radiocraft-server/src/http.rs` mirrors
every WS op through the same dispatcher (flat payload in, flat payload out), with the auth
token gate extended over the whole `/api/*` surface (`Authorization: Bearer` or
`X-Auth-Token`; `/health` stays open). Live curl smoke verified against
`RADIOCRAFT_AUTH_TOKEN=letmein ./target/debug/radiocraft-server`: 401 without/with a bad
token, flat results with it, `cmd_failed`/`bad_json` error mapping.

Plugin hosting is now closed too (was the last open wiring item): `radiocraft-server/src/
plugin_host.rs` hosts external cdylib plugins against the `radiocraft-plugin` ABI.
`PluginManager::start` spawns the single-threaded `rc-plugins` dispatcher; the boot scan of
`<config-dir>/Plugins/*.so` runs **on that thread** (never from a tokio worker — manager
control calls use `blocking_recv` and panic inside a runtime). `load_into_registry` is the
single load path for boot + `plugin_load`; per-plugin `ServerHost` shims implement the full
`Host` trait and are leaked by design. SQL from plugins round-trips onto the captured main
runtime (`exec_sql_in`), falling back to a private current-thread runtime. WS/REST ops:
`plugin_list` / `plugin_load` (relative paths resolve under `<config-dir>/Plugins`) /
`plugin_unload` / `plugin_invoke` — all via `spawn_blocking`. State fan-out is diff-based in
`pump.rs`: `PlayerStarted/Ended` (SongData JSON), `CurrentlyPlayingChanged`, `CartStarted/
Ended`, `Timer` (1 s), plus AutoDJ enable/disable; events reach clients as `plugin_event`
frames. Settings host calls persist through `PluginSettings::save_at` (the `save()` path
fails for not-yet-existing files). Fixture: `crates/radiocraft-plugin-example` (cdylib,
`export_plugin!`, actions Echo/Remember/Recall/CountStates) — a dev-dependency of
`radiocraft-server` so `tests/plugin_dylib_e2e.rs` loads the real `.so` and also checks the
ABI gate rejects a bogus dylib. Web panel: `web/src/lib/PluginPanel.svelte` (load/unload/
invoke table + recent `plugin_event` log). Live-verified over REST: boot scan loads the
fixture, Echo lands in the status line, clean `SIGTERM` shutdown logs "plugin dispatcher
thread stopped".

Still open:

1. ~~Cosmetic: on shell exit two benign GTK teardown warnings (appindicator known quirk) +
   libayatana deprecation warning — harmless.~~ Closed: `quiet_gtk_stack_noise()` in
   `crates/radiocraft-window/src/main.rs` installs a global GLib log writer func
   (`glib::log_set_writer_func`) that routes the noisy GTK/GDK/GObject/appindicator
   domains to `tracing` at debug level (`target: "gtk"`, visible with `RUST_LOG=debug`)
   and delegates everything else to `glib::log_writer_default`. Note: GLib's own records
   name the domain field `GLIB_DOMAIN`, not `DOMAIN` — matching only `DOMAIN` silently
   matches nothing. Verified live on the host X session: stderr is now completely clean
   at startup and on window close, exit stays clean, clippy clean.

Packaging for the shell is now closed (`80ff7a2`): `scripts/package-linux.sh` builds
`RadioCraft_0.2.0_amd64.deb` + `RadioCraft-0.2.0-1.x86_64.rpm` in the `localhost/rc-tauri`
container via `cargo tauri build -b deb rpm`. The tauri CLI is the napi npm package
`@tauri-apps/cli@2.12` (pinned to the `tauri` crate in Cargo.lock) — the host node 24/glibc
2.43 cannot load the native module, so the CLI only runs inside the image, where
`scripts/cargo-tauri` exposes it as `cargo tauri` (it calls the module's
`run(args, binName)` API; cargo already strips the `tauri` token). Bundle layout
(verified with dpkg-deb -c / rpm -qlp, and both packages were installed in the container
and booted against the live smoke DB): `/usr/bin/radiocraft-window` + sibling
`/usr/bin/radiocraft-server` (via `bundle.linux.{deb,rpm}.files`, satisfying the
`spawn_server` sibling lookup) + resources under `/usr/lib/RadioCraft/web/dist` (the
server's `resolve_web_dist` gained that FHS fallback, so no env var is needed).

Feature parity is otherwise closed: the `Plugin_AuxPlayers` contract (extra configurable
players beyond the built-in four zones) is done — see the aux-players paragraph below.
Auto-fill silence detection (ThresholdStart/Next/End) is also closed — see the
analyze-tracks paragraph below.

The cart/instant wall editor is now closed: `web/src/lib/CartwallEditor.svelte` mirrors
RadioDJ's wall configuration window — wall select/New/Rename/Delete, grid config
(Columns/Rows/Stop-fade s/Auto-roll with explicit Apply), a `columns×rows` slot grid with
per-slot Assign/Change/Clear plus color, a song picker over `search_songs`, live-slot
highlight from `cartNow`/`instantNow`, and dirty-flag Save/Discard. Selected wall is
hoisted in session state so both wall panels stay in sync. REST round trip verified
against the running server (create/rename/save_buttons/list/play_cart/play_instant/
delete + cleanup).

Internet streams (song Type 5) are now closed: `radiocraft-audio::decoder::StreamSource`
(ureq, `Icy-MetaData: 1`, icy-name/icy-description capture, auto-reconnect pump with
`ReadOutcome::{Have, Failed}`, unseekable `seek -> Ok(true)`, gzip) plays HTTP(S) URLs in
any zone. Streams are ordinary `songs` rows with `song_type = 5` and the URL in `path`
(no separate table — matches RadioDJ); new op `stream_add` inserts one (duration 0,
dedup via `song_by_path`). Ad-hoc `play {path}` / `queue_add_file` accept http(s) URLs
(bypass the file-exists check, `track_type = STREAM_TRACK_TYPE`). Streams are endless:
`duration None` on the player side so no VDF/stop-at plan, program duck skipped for
infinite duration, and only a never-connecting source reports EOF so AutoDJ takes over.
Live ICY metadata (`StreamTitle` MP3 tag or `icy-name` fallback) flows through a
`LiveTitle` shared cell (`Source::live_title` -> `Stream` -> `Player` -> `Engine`), the
pump polls `poll_stream_title()` and pushes fresh `player` frames, `NowInfo.stream_title`
carries it (web: `NowPlaying.svelte` shows it in accent colour over the stored row
title — RadioDJ's `UseStreamTitle` display behaviour). Web: Library "+ Stream" form.
Note: `Frame::Player.now` is now `Box<Option<NowInfo>>` (clippy `large_enum_variant`);
JSON shape unchanged. History booking keeps the stored row's artist/title (RadioDJ books
`curArtist`/`curTitle`, which it only rewrites for its own title overrides).

TracksManager analyze-tracks is now closed (`d8b5775` `e5bbcf9` `35ea625` `b740530`
`608511b`): `radiocraft-audio::cuepoints` ports Bass.Net `DetectCuePoints` recovered from
IL (two passes: forward first block >= level(S) -> cueStart, backward last block >=
level(N) -> cueNext, then third pass with (N,E) only for cueEnd; f32 peaks, block clamps,
EOF anchor; oracle-validated against live RadioDJ under wine, 13/13 assets). New op
`analyze_tracks` mirrors `bwAnalyzeTracks`: args `{songIds?}` (omitted = sweep all types
not in 5,8,9,11,12,13), thresholds `dbIn`/`dbNext`/`dbEnd` (defaults -24/-5/-30, min
-100); it is a background sweep — the cmd reply only says `{started:N}`, progress flows as
`status` pushes (`Analyzing n/total: path`), and completion broadcasts a `result` frame
with `id:0, op:"analyze_tracks"` carrying `{analyzed,skipped,total,errors,status}` (final
status text keeps RadioDJ's "analyzis" typo). Per track it rebuilds the full
`&sta=&int=&pin=&pou=&hin=&hou=&out=&xta=&end=&fin=&fou=` string with G15 formatting
(`playback::format_f64`, same routine .NET Framework `Conversions.ToString(double)` uses)
and rewrites `duration = CueNext - CueStart` (RadioDJ overwrites it; `decimal_from_f64`
now keeps G15 digits so DECIMAL(11,5) rounds like sqlserver, e.g. 6.99984 not 7.00000).
Web UI: Analyze tracks button shows live status while running and the analyzed/skipped
report on the broadcast (Session.onBroadcast). Live smoke verified end-to-end (§9).

Plugin_AuxPlayers parity is now closed (`a14ce31` backend, `34c7c7f` web panel):
`radiocraft-audio` gained `Zone::AuxPlayer(u8)` (1-based player number; the Zone enum
derives Ord so render/wire order is base zones then aux1..auxN), and the engine render
loop walks live slot keys sorted instead of the fixed four-zone array — aux slots are
created lazily like every other zone, and the mixer strips/ducks them like Cart/Instant.
`radiocraft-server/src/auxplayers.rs` keeps an `AuxTrack` slot per player and mirrors the
reference behavior: settings in `settings_Plugin_AuxPlayers.xml` (`PlayersCount` 1..15
default 3, `PluginZone` 1..10 default 4 — stored for config compatibility only, RadioCraft
renders every player through the engine mixer — `StopFadeDuration` ms 0..10000 default 0,
all ranges/defaults from `Var.cs` + `FormConfig.cs`), ops `aux_players_get_config` /
`set_config` (live-swap; players beyond the new count are stopped + unloaded) /
`load` (by `songId`, file `path` or stream URL) / `play` (start-from-0 vs fade-stop
toggle; history booked at play start like `APlayer.PlayAux`, ad-hoc song_id-0 loads
skipped; status line "Player N now playing: …") / `stop` (configured fade, keeps track) /
`seek` / `loop` (select the track's loop region like `ButLoop`, error if the track has
none) / `clear`. Player frames gained the `aux_players: [AuxPlayerInfo]` slot and
`auxN` entries in `zones` (`zone_name`/`parse_zone` round trip). Web panel
`web/src/lib/AuxPlayers.svelte` (per-player card: load by ID/path/URL, play/stop toggle,
loop, fade-stop, clear, seek slider; Players + Stop-fade config row), session commands
`auxLoad`/`auxPlay`/…. Live smoke: `/tmp/opencode/rc-aux-smoke.py` (defaults 3/4/0, arg
clamps, load/play with history row, aux1 position advance in meters frames, seek,
play-toggle stop, loop guard, clear, config grow to 5 + XML persisted + zone list
follows, shrink retires players while player 1 keeps playing).

Post-commit stream smoke fix (this commit): `StreamSource::read` was returning a sample
count where the `Source` trait contract requires FRAMES (stereo pairs), which panicked in
`player.rs` once the player sliced `scratch[..got * 2]`; it now pops interleaved frame
pairs and returns frames. Also hardened the decoder: `open_pipeline` forces
`pipeline.duration = None` (a stream server's first body chunk can let symphonia estimate
a Xing-style length that would otherwise seed a VDF/stop-at plan and end playout after
one chunk-copy), and `open_fails` only gives up (AutoDJ fallback) on a source that never
delivered a single sample (`ever_decoded` flag) rather than one that merely reconnected.
The default `EnvFilter` was `info,radiocraft=debug` — the crate targets are
`radiocraft_audio`/`radiocraft_server`, which that directive never matches (prefix
boundary must be `::`), so server DEBUG was silently off; now `info,radiocraft_server=debug`
(use `RUST_LOG=info,radiocraft_audio=debug` for engine internals).

Stream TLS interop (this commit): real-world Shoutcast DNAS hosts (verified against
`https://s3.voscast.com:10647/listen`) offer ONLY static-RSA TLS 1.2 suites
(e.g. `AES256-GCM-SHA384`); rustls deliberately does not implement those, so every
rustls variant fails with `received fatal alert: HandshakeFailure` while openssl/
curl/BASS succeed. Fix: `StreamSource::open_pipeline` splits the GET into `fetch
(url, openssl)`; on a TLS-looking error (`looks_like_tls_failure`) it retries once
through `native-tls`/OpenSSL (`ureq::native_tls::TlsConnector` via
`AgentBuilder::tls_connector`) and remembers the choice (`tls_openssl` flag) so
reconnects go straight to the stack that works. Behind cargo feature
`openssl-fallback` (default ON in `radiocraft-audio`; `--no-default-features` drops
OpenSSL and keeps pure rustls). This mirrors RadioDJ, which uses BASS + SCHANNEL/
OpenSSL, never rustls. Diagnostic example kept: `cargo run --example tlsprobe -p
radiocraft-audio -- <stream-url>` (default rustls / classic-kx / TLS1.2-only /
TLS1.2+classic-kx / native-tls matrix). Build note: OpenSSL linkage needs
`openssl-devel` (added to `.github/Dockerfile.manyrust`; Fedora host already has it).
Live check: server log shows `stream …: host rejected rustls, retrying via OpenSSL`
and main-zone `position_secs` keeps climbing across 90+ s of a live broadcast.

Fidelity check complete (this commit): `LoudnessCalculator.cs` cross-checked
against `radiocraft-audio::loudness`, using the decompile plus the real install
(`PluginInterface.dll`, `lib/bassloud.dll`, live `settings_general.xml`).
Verified: RadioDJ's loudness scan is the BASS `bassloud` add-on
(`BASS_Loudness_Start/SetChannel/GetLevel/GetLevelMulti`; per un4seen docs the
measurement is ITU-R BS.1770-4 — INTEGRATED is gated, CURRENT = 400 ms blocks,
PEAK/TRUEPEAK returned on a linear scale). The bulk scan
(`FormLoudnessCalculator`) selects `WHERE bs1770=0.000` and writes
`bs1770 = ROUND(measured,2)` (raw measurement) and
`bs_loudness = ROUND(CalculateGain(measured),2)`; its reset writes
`loudness=1.000, bs_loudness=1.000, bs1770=0.000` — so `loudness`/`bs_loudness`
hold **linear volume multipliers** (idle value 1.0), confirmed by playout:
`SetTrackVolume = BSLoudness` when `Vars.UseBSLoudness` else `Loudness`
(readable in `Plugin_AuxPlayers/APlayer.cs`; core has the same wrapper quartet
`GetLoudness`/`BS1770=`/`CalculateGain`/`Loudness=` but its call sites sit in
flattened control flow). Settings involved: `UseBSLoudness`, `BSTarget` — the
live user config has `True`/`−12`; the factory default was never recoverable
(wiki page is a PRO-version stub, forum threads inconclusive). The exact
`CalculateGain` math is NOT recoverable statically: the obfuscator encrypts
method bodies at rest — confirmed by parsing the real `PluginInterface.dll`
(SHA 7ac3ff21…): method-body RVAs land inside the first section but the bytes
there are random (the text decompile's "Invalid method header"). Any concrete
formula (e.g. `10^((target−measured)/20)`) is inference, not fact. Earlier
draft claims here (target −16, `AutoDJTargetLoudness`,
`LoudnessScan::normalize_gain`, `TargetLoudness::NORMALIZE_TARGET`) matched no
symbol in either the decompile or our crates and are removed.

Our side (`radiocraft-audio::loudness`): full ITU-R BS.1770-4 integrated
loudness — canonical EBU K-weighting coefficients, 400 ms non-overlapping
blocks, −70 LUFS absolute + −10 LU relative gates, LU offset −0.691 — plus
4× oversampled 32-tap true peak; unit-tested against reference values. Same
standard as bassloud, stricter measurement (bassloud's internals are closed).
Integration (done, 2026-11-03): the gaps above are wired end-to-end —
- `dispatch.rs` gained a `scan_loudness` command mirroring the bulk scan:
  explicit `songIds` or a sweep of `WHERE bs1770 = 0.000` (same filter as
  `FormLoudnessCalculator`, minus its `enabled=1` clause), `reset: true`
  runs the same statement/row filter as RadioDJ's `button2_Click` reset,
  target comes from the `bs_target()` setting (or a `target` arg), and the
  busy-guard + progress share the `analyze_tracks` slot. It writes
  `bs1770 = ROUND(measured,2)` and `bs_loudness = ROUND(gain,2)` via
  `update_song_loudness`; the `loudness` column keeps its DDL default 1.0
  on rows never scanned.
- playout applies the gain: `playback.rs::track_gain_from_values` selects
  `bs_loudness` vs `loudness` per the `use_bs_loudness` setting (exposed
  over IPC as `general.use_bs_loudness`), guards the unscanned 0.000
  default to 1.0 (RadioDJ relies on its reset having rewritten rows; we
  never mutate untouched rows), and the multiplier rides on
  `NowPlaying.gain_lin` / `AuxTrack.gain_lin` into
  `PlayRequest::with_track_gain_lin` for every play path: `start`,
  `play_cart`, `play_instant`, aux loads, and the plugin-host
  segment-restart re-cue.
- `radiocraft_audio::loudness::gain_to_target` is our (inferred-form)
  multiplier derivation from the target/measured dB ratio; `SongData`
  gained `bs_loudness`; the `scan_loudness` doc now states the correct
  column semantics.
(Plugin_SoundProcessing FX keys/defaults were verified in `4317b15`.)

## 9. Live smoke infrastructure (recreate if needed)

- MariaDB: `mariadbd` on `127.0.0.1:3306`, datadir `/tmp/opencode/mdb/data`, socket
  `/tmp/opencode/mdb/my.sock`, root/changeme, DB `radiodj3` (dbVersion=2006); test song ID 1 =
  6 s 440 Hz tone at `/tmp/opencode/smoke-audio/testtone.wav`. Start (if gone):
  `mariadbd --datadir=/tmp/opencode/mdb/data --socket=/tmp/opencode/mdb/my.sock -P 3306 -b`
  (binaries via system mariadb), then UI `Connect` or `db_configure`+`db_connect`.
- Smoke server: `./target/debug/radiocraft-server /tmp/opencode/rc-smoke &` (config dir has
  `settings_database.xml` + DB settings), pid file `/tmp/opencode/rc-smoke.pid`, log
  `/tmp/opencode/rc-smoke.log`. Kill precisely: `ps -eo pid,cmd | grep radiocraft-server`
  (NEVER `pkill -f` — it self-matches the harness shell).
- Window test harness (container-built binary at `/tmp/opencode/radiocraft-window`):
  run with `DISPLAY=:0 WAYLAND_DISPLAY= GDK_BACKEND=x11` (Wayland window invisible to xdotool),
  drive via fedora container: `podman run --rm --net=host -v /tmp/.X11-unix:/tmp/.X11-unix
  -v /run/user/1000/xauth_<random>:/$HOME/.Xauthority:ro -e DISPLAY=:0 -e XAUTHORITY=…
  docker.io/library/fedora:44 bash -c "dnf -y install -q xdotool; xdotool search --class
  radiocraft; xdotool windowclose $W"` (xauth file name under `/run/user/1000/` rotates).
  Screenshot attempts failed (`magick import` unusable, `spectacle -rbn` non-interactive
  hangs); verify via `ss -tnp state established '( dport = :8030 )'` instead.
- WS CLI smoke: `python3` one-liners against the wire (remember `args` nesting). Node
  scripts: `/tmp/opencode/rc-autodj-smoke.mjs` (AutoDJ ops) and
  `/tmp/opencode/rc-rotation-smoke.mjs` (rotation/criteria editor round trip incl.
  merge-patch check; creates and deletes its own `Smoke RotEditor <ts>` rotation).
- Live-DB integration test (scratch DB `radiocraft_test` on the same MariaDB; schema
  auto-bootstraps on connect): `RC_DB_TEST_DSN="mysql://root:changeme@127.0.0.1:3306/
  radiocraft_test" cargo test -p radiocraft-server --test commands`. Test tone copies are
  written under `/tmp/opencode/smoke-audio/parity-*.wav`; the test scrubs its own residue
  at start so repeated runs are idempotent. Without `RC_DB_TEST_DSN` the live test skips.
- Internet-stream smoke (`/tmp/opencode/rc-stream-smoke/`): fake ICY server `icyd.py` on
  `127.0.0.1:8099` (path `/stream.mp3`), serving `tone_id3.mp3` (13.8 s tone, ID3+Xing,
  ICY metadata block with `StreamTitle=Radiocraft Smoke FM`) one chunk per 0.5 s with
  deliberately shallow `SO_SNDBUF`/`SO_RCVBUF` (8192) and an abrupt mid-transfer drop
  every 3rd cycle, so reconnect paths fire ~every 14 s. Drive via `wsprobe.py`
  (hello+sub+`stream_add`+`play`) and WS frame watchers; server config dir `config/`
  (points at scratch DB `radiocraft_test`). Standalone decoder repro without the server:
  `cargo run --example streamtest -p radiocraft-audio -- http://127.0.0.1:8099/stream.mp3`
  (prints decode rate, title-version bumps = reconnect count, `duration None`).
  Verified live: playout survives unlimited icyd drops (`stream … reconnected (#N)` INFO
  per drop), position keeps advancing across reconnects, ICY title frames keep flowing,
  `dur: None` in now-playing.
- Analyze-tracks smoke (`/tmp/opencode/rc-analyze-smoke.py` + config dir
  `/tmp/opencode/rc-analyze-smoke/`, log `/tmp/opencode/rc-analyze-server.log`):
  server on `:8031` via `RADIOCRAFT_BIND` (port is env, not flag; config dir is the
  single positional arg). Script does hello → `sub` → db_configure/connect against
  `radiocraft_test` → `analyze_tracks {songIds}` → watches `status` pushes until the
  id-0 `analyze_tracks` broadcast. Note: WS pushes only flow after `sub`, `ready` frame
  arrives before the hello reply, and result payloads are flattened into the frame
  (no `data` key). Fixtures: `library_add` rows pointing at
  `crates/radiocraft-audio/tests/assets/t1.wav`/`t2.wav`; at S/N/E = -20/-60/-40 the
  expected DB state is t1 `duration 6.99984`,
  `cue_times &sta=8.00013637542725...&xta=14.9999771118...&end=14.9994554519...&fou=-0.000521659...`,
  t2 `duration 4.99980`, `&sta=5.0002040863...&xta=10&end=10&fou=0` (oracle-validated
  within 5e-5 s, same tolerance as the cuepoints unit tests).
- Aux-players smoke (`/tmp/opencode/rc-aux-smoke.py`, same `:8031` config dir):
  exercises the aux_players_* op set end to end (defaults, clamps, load/play/seek/
  stop/clear, meters-frame position advance on `aux1`, config grow/shrink with
  `settings_Plugin_AuxPlayers.xml` persistence). Player frames are change-driven
  snapshots (position 0 at play start); live positions ride the 20 Hz `meters`
  frames. Careful reading `library_add` results: the reply key is `songId` — the
  top-level `id` is the command id.

## 10. Toolchain / environment quirks (hard-won)

- **NEVER run bare `cargo fmt`** — host rustfmt reformats whole workspace, differs from
  committed style (polluted 42 files once, had to revert). Format only targeted files, or skip.
- No let-chains. `PathBuf → &Path` derefs automatically (avoid `.as_path()`).
- clippy deny: `cargo clippy … -- -D warnings`.
- MySQL/sqlx decode gotchas: `LAST_INSERT_ID()` and `MAX(ord)`/`COALESCE` over unsigned
  columns return `BIGINT UNSIGNED`/`DECIMAL` which the driver refuses to decode into `i64`
  — wrap in `CAST(... AS SIGNED)`. `query()` placeholder count must match binds at runtime
  (server-side prepared; wrong count = error 1210, caught only by live tests). Songs INSERT
  must list every `NOT NULL`-no-default column (`alternate_title composer label publisher
  copyright isrc` — the schema has no defaults for those).
- axum 0.8: `Message::Text(Utf8Bytes)`, handler state `State<ServerSocket>`; axum-test 21
  needs `ws` feature.
- rustls 0.23: no static-RSA TLS 1.2 suites at all (ECDHE only) — legacy Shoutcast DNAS
  hosts reject every rustls ClientHello; fix is the `native-tls` fallback, not config
  knobs. Builder order: `ClientConfig::builder_with_provider(..).with_protocol_versions
  (..)?` comes BEFORE `with_root_certificates`; plain `builder()` goes straight to
  `WantsVerifier`.
- ureq 2.12: `Error` has no `ConnectionFailed` constructor variant to build by hand;
  `AgentBuilder::tls_config(Arc<rustls::ClientConfig>)` vs `tls_connector(Arc<impl
  TlsConnector>)` (the latter takes `ureq::native_tls::TlsConnector` with feature
  `native-tls`).
- Svelte 5.57: exports `mount` (NOT `createApp`); templates forbid mixing `on:` legacy with
  new `oninput/onchange/onkeydown` on same element; no `svelte5` tsconfig option; vertical
  faders via CSS `writing-mode: vertical-rl; direction: rtl` (no `orient` attr).
- Tauri 2 specifics: needs `build.rs` with `tauri-build`; `generate_context!` requires
  `frontendDist` dir to exist (`crates/radiocraft-window/dist/index.html` placeholder — keep
  it tracked, `.gitignore` has `!crates/radiocraft-window/dist/` exception); icons must be
  truecolor+alpha PNG (`magick -define png:color-type=6`).
- Tauri bundler (2.12): `bundle.category` is a strict whitelist (Business, …, **Music**,
  News, …) — `AudioVideo` is NOT valid ("failed to build bundler settings: invalid category";
  the desktop file still gets `Categories=AudioVideo;Audio;Music;`).
  `bundle.resources` values install under `$PREFIX/lib/<productName>/` — for deb/rpm that is
  `/usr/lib/RadioCraft/…` (productName casing, NOT the kebab package name), and the server
  fallback path list must match that. `bundle.linux.{deb,rpm}.files` maps absolute install
  path -> file relative to the crate dir; used to ship the sibling server binary. deb package
  name is lowercased/kebabbed (`radio-craft`); rpm needs `rpm-build`, deb needs `dpkg` in the
  image. The npm CLI entry (`main.js`) exports `run(args, binName)` — args start at the
  subcommand (`["build", …]`), NOT full argv.
- Git identity is repo-local `RadioCraft <dev@radio…f.local>`; commit style: short scope
  prefix (`window:`, `server:`, `web:`, `ci,scripts:`).

## 11. Quick session-start checklist

1. `cd /home/john/Dev/radiocraft && export PATH=$HOME/.cargo/bin:$PATH`
2. `git log --oneline -3` (expect `80ff7a2` or newer), `git status` (expect clean)
3. Check infra: `ss -H -lnt '( sport = :8030 )'` (server), `ss -H -lnt '( sport = :3306 )'`
   (MariaDB); restart per §9 if down.
4. `cargo test --workspace --exclude radiocraft-window && cargo clippy --workspace --exclude
   radiocraft-window --all-targets -- -D warnings`
5. Web: `cd web && npm run build && npx svelte-check` (0 errors expected; node_modules present).
6. Window crate changes → build via §7 podman flow (image `localhost/rc-tauri` and volume
   `rc-window-target` may still exist; rebuild image only if Dockerfile changed).
