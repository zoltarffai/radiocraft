# RadioCraft

A faithful rebuild of **RadioDJ v3.0.0.5** (Windows/.NET radio playout
application) in Rust — reverse-engineered from the shipped binaries, bundled SQL
scripts, manuals, and config files.

Target platform: **Linux native** (x86_64). The engine is pure Rust; the user
interface is web technology (Svelte 5 SPA) served by the playout server, and is
used from both a native Tauri window shell and any browser.

## Layout

```
docs/       rebuild specifications (database schema, feature ledger, plugin ABI,
            architecture, v0.2 web-UI contract, project status & handoff)
analysis/   reverse-engineering corpus (class maps, plugin API signatures, manual text dumps)
              analysis/decompiled-csharp/ = full C# decompile (ilspycmd) of all binaries
il/         offline IL disassembly of the original binaries (RadioDJ.exe, 13 plugin DLLs)
reference/  bundled originals: MySQL schema DDL, sample settings XMLs
crates/     the Rust workspace (8 crates, GPL-3.0-only)
web/        Svelte 5 + Vite UI (TypeScript, not a cargo crate)
scripts/    build + packaging helpers
.github/    CI + container recipes (manyrust for AppImage, fedora-tauri for the window shell)
```

## Crates

| crate | role |
|---|---|
| `radiocraft-core` | domain model, AutoDJ scheduler, events, `$var$` template engine, booking rules (pure Rust) |
| `radiocraft-db` | sqlx/MySQL layer: entities, `UpdateTracks2` port, schema bootstrap, settings table store, data-only backup |
| `radiocraft-audio` | playout engine: decode (symphonia) + output (cpal/ALSA), mixer, cue/crossfade, VDF, FX chain, BS.1770-4 loudness, waveform cache, WAV recorder |
| `radiocraft-settings` | `settings_*.xml` / plugin XML read-write (NewDataSet layout), language tables |
| `radiocraft-net` | RadioDJ's own now-playing exporters (file/web/serial), legacy web server, AutoBackup scheduler, close-to-tray policy |
| `radiocraft-plugin` | third-party plugin ABI: `Host`/`Plugin` traits + C-ABI cdylib vtable (mirrors rdjInterface), loader |
| `radiocraft-server` | axum WS/REST server owning the engine + MySQL, serves the built web UI; optional auth token |
| `radiocraft-window` | Tauri 2 webview shell: spawns/reuses the server, tray icon, RadioDJ close policy |

## Development

```sh
cargo check --workspace && cargo test --workspace
cd web && npm install && npm run dev      # UI dev server against the WS/REST server
```

The `radiocraft-window` crate needs WebKitGTK/GTK3 development packages
(see `.github/Dockerfile.fedora-tauri`); on hosts without them build/test with
`--exclude radiocraft-window`. Full container build flow: `docs/06-project-status-handoff.md` §7.

Linux build deps (engine + server): `alsa-lib-devel libX11-devel pkg-config` (RPM)
or the apt equivalents.

## Packaging

- AppImage (glibc 2.28+ baseline, ships server + web UI): `scripts/build-appimage.sh`
  (container from `.github/Dockerfile.manyrust`)
- `radiocraft-window` deb/rpm via `cargo tauri build` (not yet wired into CI)

## Status & next steps

See `docs/06-project-status-handoff.md` — single source of truth for what is
done, verified, and what remains (UI surface parity, packaging for the shell).

## Legal note

Clean-room reimplementation from behavioral observation. The original RadioDJ is
copyright of its author (radiodj.ro); do not distribute original binaries with this
project. See `reference/` for the upstream GPL-licensed SQL scripts.
