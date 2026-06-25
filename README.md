# PitStopX

A cross-platform **system-tray / menu-bar app** for **Windows, macOS, and
Linux** that surfaces AI coding-assistant **usage limits** and lets you **switch
between accounts with one click** — so when one account hits its 5-hour or weekly
rate limit you can flip to another and keep working without restarting sessions.

**Providers tracked:** **Anthropic** (Claude Code) and **OpenAI** (Codex). Each
account row shows its provider badge; usage, refresh, and switching are handled
per provider behind a common `AccountSource` seam, so adding more providers is a
localized change.

PitStopX is a cross-platform port of the macOS-only
**[PitStop](https://github.com/Livin21/pitstop)** (Swift/AppKit) by
[Livin21](https://github.com/Livin21), rebuilt on **Tauri v2 (Rust core + native
WebView)** to also run on Windows and Linux. See
[`TECHNICAL_SPEC.md`](TECHNICAL_SPEC.md) for the full design.

---

## What it does

1. **See** the active account's proximity to its 5-hour and weekly limits, drawn
   right into the tray icon (percentage + warning color) with detail in the
   tooltip.
2. **Switch** the live Claude Code login to another saved account in one click.
3. **Be warned** by a native notification before a limit stalls your work,
   including which saved account is the "best pit" to switch to.

## Install

Grab the build for your platform from the
[**Releases**](https://github.com/brittosaji19/pitstopx/releases) page, then
follow the steps below. CI produces these bundles for every tagged release (see
[`.github/workflows/release.yml`](.github/workflows/release.yml)). To build from
source instead, jump to [Build & run](#build--run).

> [!NOTE]
> Builds are currently **unsigned**, so each OS shows a one-time
> "unverified developer" warning on first launch — the steps below say how to
> get past it. All Linux packages are **x86_64**.

### macOS

Download the `.dmg` for your chip — **Apple Silicon** (`aarch64`) or **Intel**
(`x64`) — open it, and drag **PitStopX** into *Applications*. Requires macOS 12
(Monterey) or newer.

Because the build is unsigned, Gatekeeper blocks the first launch. Either
right-click the app → **Open** → **Open**, or clear the quarantine flag:

```bash
xattr -dr com.apple.quarantine /Applications/PitStopX.app
```

### Windows

Download an installer for your architecture (**x64** or **arm64**):

- **`*-setup.exe`** — NSIS installer (recommended), or
- **`*.msi`** — MSI (for managed / silent deploys).

If SmartScreen appears, click **More info → Run anyway**. The WebView2 runtime
is fetched automatically when missing. Silent install:

```powershell
.\PitStopX_*-setup.exe /S                       # NSIS
msiexec /i PitStopX_*_en-US.msi /quiet          # MSI
```

### Linux

Pick the package matching your distribution and run the command from the folder
you downloaded it into (the filename includes the version). Every package needs
a system-tray / `StatusNotifierItem` host — on GNOME install the
[AppIndicator extension](https://extensions.gnome.org/extension/615/appindicator-support/);
most other desktops work out of the box.

**Debian / Ubuntu** (and derivatives: Mint, Pop!\_OS, Kali, Zorin, elementary)

```bash
sudo apt install ./*.deb        # resolves libwebkit2gtk-4.1-0 + libayatana-appindicator3-1
# older dpkg without local-file support:
sudo dpkg -i ./*.deb || sudo apt -f install
```

**Fedora / RHEL / CentOS Stream / Rocky / AlmaLinux**

```bash
sudo dnf install ./*.rpm
```

**openSUSE**

```bash
sudo zypper install ./*.rpm
```

**Arch Linux** (and Manjaro / EndeavourOS / Garuda) — install the pacman package
attached to the release:

```bash
sudo pacman -U ./*.pkg.tar.zst
```

Prefer to build it yourself? A `PKGBUILD` lives in
[`pkg/arch/`](pkg/arch/PKGBUILD):

```bash
npm ci                              # from the repo root
npm run tauri build -- --no-bundle  # produces the release binary
cd pkg/arch && makepkg -f --nodeps
sudo pacman -U ./pitstopx-*.pkg.tar.zst
```

**Any other distribution — AppImage.** The `.AppImage` carries its own WebView
and runs on most glibc-based distros with no install:

```bash
chmod +x ./*.AppImage
./PitStopX_*.AppImage
```

It needs FUSE to self-mount. If you hit a FUSE error, either install it
(`sudo apt install libfuse2` / `sudo dnf install fuse-libs`) or run it extracted:

```bash
./PitStopX_*.AppImage --appimage-extract-and-run
```

## Architecture at a glance

- **Rust core is the single source of truth** — owns the 120s refresh loop,
  per-account caches, backoff state, and all keychain/file writes.
- **WebView popover** (Svelte) is a pure view + action surface; it never touches
  secrets. State flows one way: Rust pushes `UiSnapshot` events, the panel calls
  back via `invoke`.
- Abstractions isolate every OS *and* provider difference:
  - `SecretStore` — where PitStopX keeps *its own* credential copies
    (macOS Keychain via `security` / Windows Credential Manager / Linux Secret
    Service). Blobs larger than the Windows Credential Manager limit (~2560 B,
    e.g. Codex's multi-KB `auth.json`) transparently fall back to an
    age-encrypted file; Linux uses the same file fallback when no Secret Service
    is present.
  - `AccountSource` — where each provider keeps *its* live login, detected at
    runtime: Claude (macOS Keychain / `~/.claude/.credentials.json`) and Codex
    (`$CODEX_HOME/auth.json`, default `~/.codex/auth.json`; identity decoded from
    the `id_token` JWT).
  - `engine` — per-provider token refresh + usage fetch: Anthropic
    `oauth/usage`, OpenAI Codex `backend-api/wham/usage`.

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
`.AppImage`/`.deb`/`.rpm` (Linux). For the Arch `.pkg.tar.zst`, build the binary
with `npm run tauri build -- --no-bundle` then run `makepkg` in
[`pkg/arch/`](pkg/arch/PKGBUILD) (Tauri's bundler has no pacman target).

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

## Credits

PitStopX is a cross-platform port of
**[PitStop](https://github.com/Livin21/pitstop)** by
[Livin21](https://github.com/Livin21) — the original macOS-only menu-bar app that
inspired this project. Thanks for the idea and design.

## License

MIT.
