# RadioCraft

A faithful cross-platform rebuild of **RadioDJ v3.0.0.5** (Windows/.NET radio playout
application) in Rust — reverse-engineered from the shipped binaries, bundled SQL
scripts, manuals, and config files.

## Layout

```
docs/       rebuild specifications (database schema, feature ledger, plugin ABI, architecture)
analysis/   reverse-engineering corpus (class maps, plugin API signatures, manual text dumps)
              analysis/decompiled-csharp/ = full C# decompile (ilspycmd) of all binaries
il/         offline IL disassembly of the original binaries (RadioDJ.exe, 13 plugin DLLs)
reference/  bundled originals: MySQL schema DDL, sample settings XMLs
crates/     the Rust workspace (7 crates, GPL-3.0-only)
scripts/    build + packaging helpers
.github/    CI + manylinux container recipe
```

## Crates

| crate | role |
|---|---|
| `radiocraft-core` | domain model, AutoDJ scheduler, events, `$var$` template engine, booking rules (pure Rust) |
| `radiocraft-db` | sqlx/MySQL layer: entities, `UpdateTracks2` port, schema bootstrap, settings table store, data-only backup |
| `radiocraft-audio` | playout engine: decode (symphonia) + output (cpal), mixer, cue/crossfade, VDF, FX chain, BS.1770 loudness, waveform cache, WAV recorder |
| `radiocraft-settings` | `settings_*.xml` / plugin XML read-write (NewDataSet layout), language tables |
| `radiocraft-net` | now-playing exporters (file/web/serial), web server (nowplaying + requests API), AutoBackup scheduler |
| `radiocraft-plugin` | third-party plugin ABI: `Host`/`Plugin` traits + C-ABI cdylib vtable (mirrors rdjInterface), loader |
| `radiocraft-app` | egui/eframe GUI: main window, zones, playlist, library, editors, tray |

## Development

```sh
cargo check --workspace && cargo test --workspace
```

Linux build deps: `alsa-lib-devel libX11-devel libXcursor-devel libXi-devel
libXrandr-devel libXinerama-devel libXxf86vm-devel mesa-libGL-devel` (RPM) or the
apt equivalents.

## Packaging

- AppImage (glibc 2.28+ baseline): `scripts/build-appimage.sh` (container from `.github/Dockerfile.manyrust`)
- Windows (`x86_64-pc-windows-gnu`, mingw): `scripts/build-windows.sh`
- macOS: GitHub Actions (`macos-latest`, `cargo build --workspace --release`)

## Legal note

Clean-room reimplementation from behavioral observation. The original RadioDJ is
copyright of its author (radiodj.ro); do not distribute original binaries with this
project. See `reference/` for the upstream GPL-licensed SQL scripts.
