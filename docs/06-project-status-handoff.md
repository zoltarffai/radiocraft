# Project status & handoff — RadioCraft

Snapshot date: 2026-10-31. Latest code commit: `717c412` (only docs/README-only
commits follow it). No tags exist yet; branch is `master`. This document is the
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

## 2. Current state (as of `0904464`)

Everything is committed; working tree clean. All tests green, clippy `-D warnings` clean
for every crate except `radiocraft-window` on the host (see §7 for the exclude rule).

| v0.2 milestone (docs/05 numbering) | Status | Commit |
|---|---|---|
| 1. `radiocraft-server` skeleton (WS/REST over engine) | ✅ | `4442143` |
| 2. Web UI dashboard parity (transport, playlists, carts) | ✅ | `d452832`, `e1387db` |
| 2.5 snapshot/status/db_connected, library auto-load | ✅ | `0cd648d` |
| CI/packaging Linux-only; AppImage wraps server+web | ✅ | `6d7e0fc` |
| egui `radiocraft-app` retired, logic migrated | ✅ | `10707cf` |
| 4. Tauri window shell + tray; close policy | ✅ | `717c412` |
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
| `radiocraft-server` | ~2.7k | axum WS/REST + static web host. modules: `frames auth playback state dispatch pump http ws run grid playlist` + tests `ws_e2e.rs http_e2e.rs` (38 lib tests) |
| `radiocraft-window` | ~240 | Tauri 2 shell (see §5) |

Non-cargo: `web/` — Svelte 5 + Vite 8 (rolldown) + TypeScript UI.
`web/src/lib/`: `protocol.ts` (wire), `session.svelte.ts` (WS client state, reads `?token=`),
`App.svelte`, `NowPlaying/Meter/ZoneMixer/Library/Database/HistoryPanel.svelte`.

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

Ops (dispatch.rs, 55): `play pause resume stop stop_all stop_at seek segue_now cue
get_setting set_setting set_zone_fader_db set_zone_muted set_master_db get_db_config
db_configure db_connect list_songs search_songs list_playlists list_playlist_items
list_carts list_cart_buttons list_history play_cart` — plus the milestone-3 parity set:
library `library_filter library_get library_add library_update library_delete
library_mass_set library_mass_delete`; history `history_filter history_add history_delete
play_history` (rewind = re-add to library, returns `{"restored":id}`); playlists
`playlist_create playlist_rename playlist_delete playlist_add_item playlist_remove_item
playlist_move_item playlist_set_cues playlist_set_song`; queue
`list_queue queue_add_song queue_add_file queue_remove queue_clear queue_eta_set
queue_move` (ETA swap, args `{queueId,with}`); transport extra `play_next`.

Cart play parity (`play_cart`, mirrors `Plugin_InstantPlayers/CartButtonControl.set_PlayCart`):
args `{buttonId}` or `{cartId,"number":slot}` (+ optional `fade` ms). Press rolls the linked
`songs` row (via `carts_list.swID`) on the **cart zone** at position 0 and books history
immediately at start (lightweight `UpdateHistory` insert — no rotation counters, no
`UpdateTracks2`); pressing the rolled button again stops it with a fade. Carts never enter
the main now-playing hub; the cart zone auto-ducks main via the mixer. The `player` frame
carries `cart: {button_id,cart_id,song_id,title}|null` for the wall highlight; `stop` on the
cart zone or natural finish clears it. Results: `{cart,historyId,duration_secs}` or
`{stopped:true,buttonId}`. Covered live by `tests/commands.rs::live_cart_play`.

Key result shapes: `{songs}`, `{total,songs}`, `{history}`, `{total,history}`,
`{playlists}`, `{items}`, `{playlistId}`, `{song}`, `{songId}`, `{itemId}`, `{queued}`,
`{restored}`, `{updated}`, `{removed}`. `playlist_remove_item` refuses itemId 0
(locked now-playing row); `playlist_move_item` uses 1-based `{from,to}` then reindexes;
`queue_add_file` uses sentinel `swID:-1`/`vtID:-1`; `queue_eta_set` eta accepts RFC3339
or local `YYYY-MM-DD HH:MM:SS`. `queuelist` has no cue/fade columns — queue edits are
ETA/order/swap only.

REST mirrors: `/api/state`, `/api/songs`, `/api/playlists`, `/api/playlists/{id}/items`,
`/api/carts`, `/api/carts/{id}/buttons`, `/api/history`.

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
  builds sequentially), **no sudo**. Node v24.22.2 / npm 11.19.1. `export PATH=$HOME/.cargo/bin:$PATH`.
- **podman** (docker absent). Images: `localhost/manyrust` (AlmaLinux 8.10, glibc 2.28, for
  AppImage), `localhost/rc-tauri` (built from `.github/Dockerfile.fedora-tauri`, fedora:44 +
  webkit2gtk4.1/gtk3/libappindicator-gtk3/librsvg2/alsa-lib devel + cargo-clippy).
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
- On a dev box without WebKitGTK devel packages: `--exclude radiocraft-window` for
  test/clippy (documented in docs/05).
- Packaging gotchas: linuxdeploy corrupts musl static binaries → manylinux glibc-dynamic only;
  `APPIMAGE_EXTRACT_AND_RUN=1` in containers.

## 8. Remaining work / known gaps

Milestone 3 (UI surface parity) is closed: library filter/sort/paging + edit form +
mass-select ops, playlist editing (create/rename/delete/add/remove/move/cues), queue
panel (add/remove/clear/ETA/swap), history window (filter/delete/rewing play,
`play_history`), `play_next`. Covered end-to-end by `crates/radiocraft-server/tests/
commands.rs` (offline op-surface tests + live-DB round trip gated on `RC_DB_TEST_DSN`,
see §9).

Still open:

1. AutoDJ exposure over WS (scheduler is implemented in core but not surface-driven).
2. Settings editors (sound device, scheduler, now-playing export config).
3. REST mirrors for the new parity ops (currently WS-only; REST keeps the read-only set).
4. Packaging for the shell: `cargo tauri build` deb/rpm job (bundle config already in
   `tauri.conf.json`: targets deb/rpm, identifier `dev.radiocraft.window`).
5. Cosmetic: on shell exit two benign GTK teardown warnings (appindicator known quirk) +
   libayatana deprecation warning — harmless.

Deferred fidelity checks (optional): cross-check `LoudnessCalculator.cs`, Plugin_SoundProcessing
FX keys against decompiles.

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
- WS CLI smoke: `python3` one-liners against the wire (remember `args` nesting).
- Live-DB integration test (scratch DB `radiocraft_test` on the same MariaDB; schema
  auto-bootstraps on connect): `RC_DB_TEST_DSN="mysql://root:changeme@127.0.0.1:3306/
  radiocraft_test" cargo test -p radiocraft-server --test commands`. Test tone copies are
  written under `/tmp/opencode/smoke-audio/parity-*.wav`; the test scrubs its own residue
  at start so repeated runs are idempotent. Without `RC_DB_TEST_DSN` the live test skips.

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
- Svelte 5.57: exports `mount` (NOT `createApp`); templates forbid mixing `on:` legacy with
  new `oninput/onchange/onkeydown` on same element; no `svelte5` tsconfig option; vertical
  faders via CSS `writing-mode: vertical-rl; direction: rtl` (no `orient` attr).
- Tauri 2 specifics: needs `build.rs` with `tauri-build`; `generate_context!` requires
  `frontendDist` dir to exist (`crates/radiocraft-window/dist/index.html` placeholder — keep
  it tracked, `.gitignore` has `!crates/radiocraft-window/dist/` exception); icons must be
  truecolor+alpha PNG (`magick -define png:color-type=6`).
- Git identity is repo-local `RadioCraft <dev@radio…f.local>`; commit style: short scope
  prefix (`window:`, `server:`, `web:`, `ci,scripts:`).

## 11. Quick session-start checklist

1. `cd /home/john/Dev/radiocraft && export PATH=$HOME/.cargo/bin:$PATH`
2. `git log --oneline -3` (expect `0904464` or newer), `git status` (expect clean)
3. Check infra: `ss -H -lnt '( sport = :8030 )'` (server), `ss -H -lnt '( sport = :3306 )'`
   (MariaDB); restart per §9 if down.
4. `cargo test --workspace --exclude radiocraft-window && cargo clippy --workspace --exclude
   radiocraft-window --all-targets -- -D warnings`
5. Web: `cd web && npm run build && npx svelte-check` (0 errors expected; node_modules present).
6. Window crate changes → build via §7 podman flow (image `localhost/rc-tauri` and volume
   `rc-window-target` may still exist; rebuild image only if Dockerfile changed).
