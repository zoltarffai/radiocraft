# 04 — RadioCraft Architecture (final, locked)

Target: faithful cross-platform rebuild of RadioDJ v3.0.0.5 in Rust. GUI, DB, audio, plugin ABI, integrations per docs 01–03.

## Crate map (Cargo workspace, edition 2021)

```
crates/
  radiocraft-core        domain model: SongData, track types, playlist, AutoDJ scheduler, events engine,
                         template-var engine ($var$), cue model, weight/limit rules. No I/O deps.
  radiocraft-db          sqlx MySQL layer: connection mgmt (DbServer/DbUsername/DbPass/DbName/SSL),
                         schema bootstrap from reference/schema/database.sql, entity models,
                         UpdateTracks2 as transaction, settings table store, backup exporter.
  radiocraft-audio       playout engine: Source trait + symphonia/cpal backend, mixer with per-zone channels,
                         cue-seek playback, crossfades, VDF logic, loudness/BS.1770 scan, FX chain (EQ,
                         compressor, AGC, stereo-enh, limiter), waveform cache, recorder, VU levels.
  radiocraft-settings    settings_*.xml + plugin XML read/write (NewDataSet/Settings/Key/Value), schema of all
                         keys from docs/02, store-to-DB mode.
  radiocraft-net         integrations: now-playing exporters (file/web/serial), web-nowplaying template server,
                         HTTP request/remote server, ICY stream title, reports data, AutoBackup scheduler.
  radiocraft-plugin      plugin ABI: HostTrait + PluginTrait mirroring rdjInterface.IHost (73), SongData cdylib
                         C ABI (extern "C" vtable + JSON payloads), plugin loader (dlopen), event-action registry.
  radiocraft-app         egui/eframe GUI: main window, 4 zones, playlist, library browse, search, history,
                         AutoDJ/event/rotation editors, settings, tray (tray-icon crate), shortcuts, localization.
```

Dependency edges: app → (core, db, audio, settings, net, plugin); audio → core (model types only); net → core+db; plugin → core; db → core; settings → core. All leaf crates expose plain Rust APIs; the app is the only binary.

## Key runtime invariants (from docs/01 appendix)
- Booking every start in ONE transaction = `UpdateTracks2` port (radiocraft-db::booking).
- Replay avoidance: timestamps on artist/title/album + rotation negative-age filters.
- Sentinels: '2002-01-01 00:00:01' empty date, '&', '1900' year.
- $var$ substitution: case-sensitive, `_enc` URL-encode, `next{1..5}_` prefixes (radiocraft-core::template).
- Track types 0..14 with per-type title/color templates.

## Backend decisions (cross-platform, FOSS)
- Audio: default backend symphonia (decode) + cpal (output), in-tree mixer/FX with BASS-compatible semantics (cue times in seconds; loudness via our own BS.1770 integrated-loudness impl; `bass`-style bass units for BCue* compatibility). Optional BASS backend behind feature flag later.
- DB: sqlx `mysql` feature, runtime (text) queries + `FromRow`, no compile-time DATABASE_URL. MariaDB/MySQL compatible.
- GUI: eframe/egui (Wayland+X11+mac+win), tray-icon, arboard (clipboard), dirs.
- XML: quick-xml with hand-written mappers for `NewDataSet/Settings` and language files.
- Plugins: cdylib C ABI (`extern "C"` function table) + JSON text payloads for complex types; dlopen via `libloading`. Script plugins (lua) optional.
- Packaging: AppImage (manylinux glibc), Windows (mingw-w64 cross), macOS (GitHub Actions).

## Build/verify loop
cargo check/clippy on host; container builds for AppImage; mingw for windows-gnu; CI for mac. Each crate has unit tests; core logic (scheduler, template engine, booking) mirrors docs/01 semantics with SQL-level integration tests skipped against MySQL.

## Work assignments (agents)
- A: radiocraft-core (model + scheduler + template + events) — owns docs 01–03 semantics
- B: radiocraft-db (sqlx + UpdateTracks2 + bootstrap + backup)
- C: radiocraft-audio (engine + FX + loudness)
- D: radiocraft-settings + radiocraft-net (exporters + servers + AutoBackup)
- E: radiocraft-plugin (ABI + loader)
- F: radiocraft-app (GUI screens) — last, after A–D compile
