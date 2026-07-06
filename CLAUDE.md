# PitStopX

Cross-platform system-tray / menu-bar **background agent** that surfaces AI coding-assistant usage limits (Anthropic **Claude Code**, OpenAI **Codex**) and lets you switch the machine's live login between saved accounts in one click, so work continues past a rate limit. Stack: **Tauri v2** (Rust core = single source of truth) + **Svelte 4 / TypeScript / Vite** webview popover. Targets **macOS, Windows, Linux**. It is a port of the macOS-only Swift app "PitStop" by Livin21. App identifier `dev.britto.pitstopx`; a second launch is intercepted by single-instance and focuses the existing tray.

## Commands

Run from repo root unless noted. Tauri drives the frontend automatically — **never** run `vite build` before `tauri build`.

```bash
# Full Tauri app
npm run tauri dev                       # run the app in dev (runs `npm run dev` first)
npm run tauri build                     # build + bundle installers (runs `npm run build` first)
npm run tauri build -- --no-bundle      # release binary only, no installers (Arch job uses this)
node scripts/make-icon.mjs              # REQUIRED once on fresh checkout — icons are generated, not committed

# Frontend only
npm run dev                             # Vite dev server on :1420 (frontend only)
npm run build                           # Vite prod build -> dist/
npm run check                           # svelte-check typecheck (this is the frontend "lint")
npm test                                # vitest run (src/lib/usage.test.ts)

# Rust — MUST run with working directory src-tauri
cd src-tauri && cargo test
cd src-tauri && cargo clippy --all-targets -- -D warnings
cd src-tauri && cargo fmt --check

# Diagnostic CLI modes (handled before any Tauri code boots)
pitstopx --check          # print accounts + usage, no tray
pitstopx --print-paths    # resolved config/data/log dirs + detected creds locations
pitstopx --screenshot     # demo mode with masked sample data (sets PITSTOPX_DEMO=1)

# Packaging
npm run build:windows     # scripts/build-windows.ps1 (pins Rust 1.88.0, --bundles nsis,msi)
cd pkg/arch && makepkg -f --nodeps && sudo pacman -U ./pitstopx-*.pkg.tar.zst
```

## Architecture

The **Rust core owns all truth**: a 120s Tokio refresh loop, per-account usage caches, per-account backoff gates, threshold-notification buckets, and every keychain/file write. The **Svelte webview is a pure view+action surface** — it never touches secrets. State flows one way: Rust pushes `UiSnapshot` events (camelCase JSON) over Tauri IPC; the panel calls back only via `invoke` for actions.

**Two-crate build:** one `src-tauri` package produces `pitstopx_lib` (all logic) + a thin `pitstopx` binary ([src-tauri/src/main.rs](src-tauri/src/main.rs)) that wraps it. `main.rs` parses argv into `CliMode` and runs headless CLI modes *before any Tauri code*, else calls `pitstopx_lib::run()` ([src-tauri/src/lib.rs](src-tauri/src/lib.rs)) — the GUI entrypoint that builds the Tauri app (5 plugins, managed `SharedController` state, 14 invoke commands, programmatic tray, hidden `popover` window).

**Two-surface tray UX:** left-click opens the frameless always-on-top popover (rich account rows + usage bars); right-click opens the **native** Tauri context menu (Save Current, Remove submenu, Refresh Now, display style/metric radios, Launch at Login, Quit). The context menu is native — the webview only renders the popover.

**Trait seams hide all OS/provider specifics:**
- `AccountSource` (per provider) — locate/read/write each provider's *live* login on disk.
- `SecretStore` (per OS) — PitStopX's *own* encrypted copies of credential blobs.
- per-provider `engine` — OAuth token refresh + usage fetch.

Every account carries a `Provider` tag (persisted in `profiles.json`); there is **no global "current provider"** — both the credential seam and usage dispatch branch per-account.

### Where things live

```
src/                      Svelte 4 frontend (pure view)
  main.ts                 instantiates root App component
  App.svelte              owns snapshot state, view toggle, all event subs, content-fit window sizing
  lib/AccountRow.svelte   one account card; invokes switch/reauth/remove directly
  lib/SettingsView.svelte hotkey capture + CLI path overrides
  lib/usage.ts (+.test)   pure meter formatting helpers (unit-tested)
  lib/types.ts            DTOs mirroring Rust serde structs (camelCase) — the contract boundary

src-tauri/src/
  main.rs                 binary entry: tracing, argv -> CliMode, headless modes
  lib.rs                  Tauri Builder, tray build, menu-event dispatch, popover window + global shortcut
  cli.rs                  --check / --print-paths headless modes
  app.rs                  AppState (single source of truth), Controller, THE 120s refresh pipeline, backoff, thresholds, tray/menu re-render
  engine.rs               per-account provider dispatch: fetch() = parse blob -> refresh-if-inactive -> fetch_usage
  actions.rs              #[tauri::command] handlers + do_* shared impls (panel AND menu converge here)
  provider.rs             Provider enum (Anthropic, OpenAI): id/accent/login_command
  source.rs               AccountSource trait, Identity/LiveAccount, secret_key() namespacing, build_all()
  claude_source.rs        Anthropic live login (Keychain or ~/.claude/.credentials.json) + atomic_write()
  codex_source.rs         OpenAI live login (~/.codex/auth.json), JWT-claim identity
  usage_api.rs            Anthropic usage + OAuth refresh; defines shared UsageReport/Window/UsageError
  codex_usage.rs          OpenAI usage (tolerant multi-alias parser) + OAuth refresh
  credentials.rs          CredentialBlob (opaque-JSON, only rewrites claudeAiOauth tokens), ClaudeConfig
  profile_store.rs        Profile + ProfileStore: profiles.json, capture_current/switch_to
  secrets/               SecretStore trait + mod.rs runtime backend pick; macos.rs, windows.rs, linux.rs, encrypted_file.rs
  tray.rs                 tiny-skia icon rendering + 5x7 bitmap font + native menu (build_menu)
  ui_events.rs            UiSnapshot DTO + SNAPSHOT/RESET_VIEW event names
  notify.rs               Notifier (lazy permission, queue-then-flush/drop)
  format.rs               display formatting (percent, reset times)
  login.rs                locate + launch provider login CLI in a real terminal
  linux_shortcut.rs       XDG GlobalShortcuts portal (ashpd) hotkey, X11 fallback
  paths.rs, prefs.rs      per-OS dirs; indicator prefs + CLI-path overrides (tauri-plugin-store)
```

## Key data flows

**Usage fetch → display** (all in [app.rs](src-tauri/src/app.rs), NOT engine.rs): `refresh_all` is single-flight (a `refresh_lock` mutex + `refreshing`/`refresh_queued` coalescing). Each `run_refresh_once`: (1) `capture_current()` snapshots the live account, (2) reload profiles + active accounts, (3) per-profile `engine::fetch()` — skips accounts inside their backoff gate, refreshes inactive-and-expired OAuth tokens, persists rotated blobs, records errors + backoff, (4) `pick_primary` (highest-utilization active account or pinned `tray_account`), then `update_tray` + `emit_snapshot` + `check_thresholds`. Utilization is normalized to **0..1** at the single parse boundary; everything downstream works in fractions. Icon renders the % *into the bitmap* on Windows/Linux (icon-only trays); macOS gets a wide two-bar indicator.

**Account switch** (`switch_to`): **capture the current account FIRST** — a failed snapshot *aborts the switch* so the outgoing refresh token is never lost. Then read the saved blob from the `SecretStore` and `write_live()` restores blob + identity. The **whole credential blob is swapped** so per-account MCP OAuth tokens travel with the account; for Claude, `oauthAccount` in `~/.claude.json` is also restored. **New login** (`do_login`) is order-sensitive and destructive: `ensure_installed` (fail-fast + notify) → `capture_current` → `clear_live` (wipes live creds) → `login::launch` (opens a terminal) → poll refresh at 10/20/40/80/120s. Reordering risks logging the user out with no way back.

## Conventions & gotchas

- **`time` is hard-pinned `=0.3.47`** in Cargo.toml — 0.3.48 has a time-macros codegen bug that breaks tauri-utils. Windows builds also pin **Rust 1.88.0** (default 1.96 miscompiles `time`).
- **All logic lives in `pitstopx_lib`;** `main.rs` is a thin wrapper. CLI/diagnostic modes are fully handled in `main.rs` *before* any Tauri code — `run()` assumes only the tray path reaches it.
- **`refresh_all_task()` exists only** to box `refresh_all` into `Pin<Box<dyn Future + Send>>` and break cyclic Send inference in the backoff-retry recursion. Removing it breaks compilation.
- **!Send discipline:** every Tauri tray/menu/window type is `!Send`. Snapshot needed data into Send structs (`TrayVisual`, `MenuModel`, `UiSnapshot`) under the async lock, then mutate UI inside `app.run_on_main_thread(...)`. Window/GTK ops MUST run on the main thread.
- **Secrets NEVER touch `profiles.json`** — only non-secret metadata (email, provider, subscription_type, tier, oauthAccount identity) goes there; the credential blob only goes to the `SecretStore`.
- **CredentialBlob is opaque JSON** — only token fields inside `claudeAiOauth` (Claude) / `tokens.*` (Codex) are rewritten; `mcpOAuth` and unknown keys survive byte-for-byte (enables byte-equality short-circuit in `capture_current`).
- **Per-OS secret backends** picked at runtime by `secrets::build()`, service name `PitStopX-profile`:
  - **macOS** — shells `/usr/bin/security` (NOT the keyring crate) so the one-time "Always Allow" grant survives rebuilds and is shared with Claude Code. The blob is passed as an argv value → briefly visible in the process table (accepted, same as Claude Code).
  - **Windows** — Credential Manager (DPAPI); blobs over **2400 B** transparently fall back to the age-encrypted file, and reads check Cred Manager first then file.
  - **Linux** — Secret Service probed ONCE at startup; if absent, silently degrades to the age file store (logged as DEGRADED — starting a keyring daemon later won't switch backends until restart).
  - **age file fallback** is deliberately weaker: passphrase machine-bound to `/etc/machine-id` (Linux) or a plaintext `.fallback-key` (Windows). Anyone who can read the data dir + key file can decrypt.
- **Token refresh policy:** only INACTIVE accounts are refreshed (the provider CLI keeps the active one fresh). Codex/OpenAI inactive tokens are **never rotated** (single-use rotating refresh tokens would revoke `~/.codex/auth.json`). 120s expiry safety margin on both paths.
- **Backoff:** RateLimited → honor Retry-After else exponential capped at 15 min; Unauthorized (401/403) → back off **1 hour** (rejected token won't self-heal). Refresh Now clears ALL backoff.
- **Unofficial API surface:** Anthropic `api/oauth/usage` + `console.anthropic.com/v1/oauth/token` (public PKCE client `9d1c250a-…`); Codex `chatgpt.com/backend-api/wham/usage` (requires browser-mimicking Origin/Referer + ChatGPT-Account-Id) + `auth.openai.com/oauth/token`. Schemas drift — `codex_usage.rs` uses a tolerant multi-alias parser. All HTTP via reqwest **rustls** (no OpenSSL), 15s timeout.
- **Codex = a single monthly window** occupying the `seven_day` slot (43200 min default); no 5-hour bar renders. JWT claims are decoded *without signature verification* (it's the user's own local token).
- **Tray icon is drawn with a built-in 5x7 bitmap font** (`glyph_for`/`draw_glyph`) so the build stays hermetic — no font asset. `render_icon` branches with runtime `cfg!()` (not `#[cfg]`) so both renderers stay compiled. macOS = wide `render_rectangular` (5h + 7d bars, template icon); Windows/Linux = `render_square` gauge. Stale state dims to alpha 0.45 rather than hiding.
- **Global hotkey diverges by OS:** tauri-plugin-global-shortcut on Windows/macOS; **XDG GlobalShortcuts portal** (ashpd) on Linux — where the compositor OWNS the key, `configure_shortcut` opens GNOME's reconfigure UI, and a matching installed `<id>.desktop` is required. Wayland can't query cursor/tray geometry, so `position_near_tray` falls back to a top-right/centered anchor.
- **Linux appindicator never delivers a tray left-click** — the menu's "Open PitStopX" is the only way into the popover there (handled inline on the main thread).
- **`ExitRequested` is intentionally a no-op** so the app stays a background agent with no windows. `do_quit` calls `app.exit(0)`.
- **CSP is strict** (`default-src 'self'; style-src 'self' 'unsafe-inline'; img-src 'self' data:`) — no external assets/fonts/scripts; everything is Vite-bundled.
- **`demo` cargo feature** (`--features demo`) builds masked-sample screenshot mode; off by default, no CI enables it. `PITSTOPX_DEMO=1` also triggers it.
- **Frontend is push-based** — never polls, holds no domain state; every mutating action invokes a command and waits for the re-emitted snapshot. Row order is computed in Rust (active first, then ascending utilization) so the UI never re-sorts under the cursor. `fitWindow()` content-sizes the frameless window (width 372, height clamp 120..600).
- **Builds are UNSIGNED** — every OS shows a one-time "unverified developer" warning; macOS needs quarantine removal. macOS uses `macos-private-api` (transparent window), which can affect Mac App Store eligibility.
- **CI/packaging pins:** release Linux job uses **ubuntu-22.04** (glibc for Debian 12); release triggers only on a created GitHub release or `workflow_dispatch` (NOT `v*` tag push); Tauri has no pacman target so Arch is a separate container job. `dirs::config_dir()` puts macOS config in `~/Library/Application Support` despite doc comments saying `~/.config`.

## Deeper design intent

- [README.md](README.md) — user-facing overview, install/build/CLI/security. **The more current doc**; already describes the multi-provider design.
- [TECHNICAL_SPEC.md](TECHNICAL_SPEC.md) — full design rationale, data model, parity matrix, roadmap. **STALE on multi-provider** (written Claude-only, describes `ClaudeSource` before it was generalized to `AccountSource`; §13 signing/notarization is roadmap, not reality). Trust README + code over the spec on anything provider- or signing-related.
- [src-tauri/icons/README.md](src-tauri/icons/README.md) — icons are generated (`node scripts/make-icon.mjs`), not committed.