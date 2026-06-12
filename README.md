# PitStopX

A cross-platform **system-tray / menu-bar app** for **Windows, macOS, and
Linux** that surfaces Claude Code account **usage limits** and lets you **switch
between Claude accounts with one click** — so when one account hits its 5-hour or
weekly rate limit you can flip to another and keep working without restarting
sessions.

PitStopX is the portable successor to the macOS-only **PitStop** (Swift/AppKit),
built on **Tauri v2 (Rust core + native WebView)**. See
[`TECHNICAL_SPEC.md`](TECHNICAL_SPEC.md) for the full design.

---

## What it does

1. **See** the active account's proximity to its 5-hour and weekly limits, drawn
   right into the tray icon (percentage + warning color) with detail in the
   tooltip.
2. **Switch** the live Claude Code login to another saved account in one click.
3. **Be warned** by a native notification before a limit stalls your work,
   including which saved account is the "best pit" to switch to.

## Architecture at a glance

- **Rust core is the single source of truth** — owns the 120s refresh loop,
  per-account caches, backoff state, and all keychain/file writes.
- **WebView popover** (Svelte) is a pure view + action surface; it never touches
  secrets. State flows one way: Rust pushes `UiSnapshot` events, the panel calls
  back via `invoke`.
- Two **platform abstractions** isolate every OS difference:
  - `SecretStore` — where PitStopX keeps *its own* credential copies
    (macOS Keychain via `security` / Windows Credential Manager / Linux Secret
    Service, with an encrypted-file fallback).
  - `ClaudeSource` — where Claude Code keeps *its* live login (macOS Keychain /
    `~/.claude/.credentials.json` on Windows/Linux), detected at runtime.

```
src-tauri/src/
  main.rs          entry + CLI modes (--check / --print-paths / --screenshot)
  lib.rs           Tauri assembly: plugins, tray, popover, refresh loop
  app.rs           AppState, refresh pipeline, backoff, threshold notifications
  actions.rs       Tauri commands + shared action impls
  tray.rs          dynamic tray-icon rendering + native menu
  ui_events.rs     UiSnapshot DTOs + snapshot builder
  usage_api.rs     usage fetch + OAuth refresh
  credentials.rs   credential blob + ~/.claude.json parse/patch
  profile_store.rs Profile model + persistence + capture/switch
  secrets/         SecretStore trait + per-OS backends
  claude_source.rs ClaudeSource trait + per-OS backends
  notify.rs        notification wrapper
  format.rs        percent/date/relative formatting
  paths.rs         per-OS config/data/log dirs
src/               Svelte popover (App.svelte, lib/AccountRow.svelte, ...)
```

## Prerequisites

- **Rust** (stable, edition 2021) with a working C toolchain/linker:
  - Windows: Visual Studio Build Tools with the *Desktop development with C++*
    workload (provides `link.exe`).
  - macOS: Xcode command-line tools.
  - Linux: `webkit2gtk-4.1`, `libayatana-appindicator3`, `librsvg2`, plus the
    usual build tools (see `.github/workflows/ci.yml`).
- **Node.js** 20+ and npm.
- The **Tauri CLI**: `npm i` installs `@tauri-apps/cli` as a dev dependency.

## Build & run

```bash
# 1. Install frontend deps
npm install

# 2. Generate the app icon set (one-time, or whenever icon.svg changes)
node scripts/make-icon.mjs

# 3. Run in development (hot-reloading WebView + Rust)
npm run tauri dev

# 4. Produce per-OS installers
npm run tauri build
```

Artifacts per OS: `.dmg`/`.app` (macOS), `.msi`/NSIS `.exe` (Windows),
`.AppImage`/`.deb`/`.rpm` (Linux).

## CLI / diagnostic modes

The same binary supports headless modes (parsed before the tray starts):

```bash
pitstopx --check         # print saved accounts + live usage, no tray
pitstopx --print-paths   # show resolved config/data/log dirs + creds location
pitstopx --screenshot    # run with masked sample emails for captures
```

## Tests

```bash
npm test                       # frontend unit tests (Vitest)
cd src-tauri && cargo test     # Rust unit tests (format, credentials, ...)
cargo clippy --all-targets     # lints
cargo fmt --check              # formatting
```

## Security model (summary)

- The credential blob (incl. refresh + MCP OAuth tokens) lives **only** in the OS
  secret store (macOS/Windows) or the secret store **or** an encrypted fallback
  file (Linux) — never in `profiles.json`.
- On macOS all keychain access routes through the Apple-signed `/usr/bin/security`
  so the one-time "Always Allow" grant survives rebuilds and is shared with
  Claude Code.
- Claude Code's own credentials on Windows/Linux are a plaintext file
  (`~/.claude/.credentials.json`); PitStopX preserves its user-only permissions
  and writes atomically. This is Claude Code's storage choice, surfaced here so
  the baseline is clear.
- The usage endpoint, OAuth refresh flow, and public client id are the same
  **unofficial** surface Claude Code uses; if Anthropic changes them,
  `usage_api.rs` must be updated.

See [`TECHNICAL_SPEC.md`](TECHNICAL_SPEC.md) §14 for the full model and
trade-offs.

## License

MIT.
