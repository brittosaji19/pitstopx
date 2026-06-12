# PitStopX — Cross-Platform Technical Specification

> A cross-platform **system-tray / menu-bar application** for **Windows, macOS,
> and Linux** that surfaces Claude Code account **usage limits** and enables
> **one-click switching between Claude accounts**, so that when one account hits
> its 5-hour or weekly rate limit the user can flip to another and keep working
> without restarting sessions.

PitStopX is the cross-platform successor to the macOS-only **PitStop** (Swift /
AppKit). It preserves PitStop's behavior and UX intent while replacing the
native-macOS stack with a portable one and abstracting every OS-specific surface
(tray, secret storage, Claude Code credential location, autostart, notifications)
behind a platform layer.

This document specifies the full application: the technology stack and why it was
chosen, the architecture, the data model, every feature, the platform
abstractions, the build/packaging pipeline per OS, and the security model.

---

## 1. Overview

| Property | Value |
|---|---|
| Name | PitStopX |
| App identifier | `dev.britto.pitstopx` |
| Type | Background tray/menu-bar agent (no taskbar/Dock presence) |
| Target OSes | **Windows 10/11**, **macOS 12+**, **Linux** (X11 + Wayland, GNOME/KDE) |
| Architectures | x86-64 and ARM64 (Apple Silicon, Windows-on-ARM, aarch64 Linux) |
| Framework | **Tauri v2** (Rust core + WebView UI) |
| Backend language | **Rust** (stable, edition 2021) |
| Frontend | **Svelte + TypeScript**, bundled by **Vite** |
| Async runtime | **Tokio** |
| Distribution | Per-OS installers: `.dmg`/`.app` (macOS), `.msi`/NSIS `.exe` (Windows), `.AppImage`/`.deb`/`.rpm` (Linux) |

### 1.1 Why this stack

The application is a small, **always-resident background utility** whose job is a
tray icon, a popover menu, periodic HTTP polling, and secure credential
shuffling. The selection criteria were: small memory/disk footprint, first-class
cross-platform **system-tray** support, a safe systems language for credential
and keychain handling, and a uniform **secret-store** abstraction across the
three OS credential backends.

**Tauri v2 + Rust** is the most suitable choice:

- **Footprint.** Tauri ships a tiny Rust binary and uses the OS's *native* WebView
  (WebView2 on Windows, WKWebView on macOS, WebKitGTK on Linux) instead of
  bundling a browser engine. Idle RAM is a fraction of an Electron equivalent —
  important for a process that runs all day in the tray.
- **Native tray, menus, notifications, autostart** are built into Tauri v2 /
  official plugins, cross-platform, from one codebase.
- **Rust** gives memory-safe, dependency-light handling of the security-sensitive
  parts (keychain access, OAuth refresh, blob patching), with the mature
  `keyring` crate abstracting **macOS Keychain**, **Windows Credential Manager**,
  and **Linux Secret Service** behind one API.
- **Web UI** makes the rich account rows (usage bars, hover "Switch" pill) far
  easier to build and theme than native per-OS UI toolkits, while the *menu
  chrome* itself uses native tray menus.

**Alternatives considered and rejected:**

| Option | Why not |
|---|---|
| Electron + Node | 3–5× the memory/disk of Tauri for an idle tray app; Chromium bundled per app. |
| Qt / C++ (or PySide) | Heavier toolchain; secret-store + tray still need per-OS glue; larger redistributables. |
| .NET MAUI / Avalonia (C#) | Good UI story but weaker/uneven Linux tray support; larger runtime. |
| Go + `systray` | Lightweight, but no first-class WebView for rich rows; secret-store and OAuth ergonomics weaker than Rust's crates. |
| Keep native per-OS (Swift + WinUI + GTK) | Triples the implementation and maintenance surface. |

### 1.2 Core value proposition (unchanged from PitStop)

1. **See** the active account's proximity to its 5-hour and weekly limits from the
   tray at a glance.
2. **Switch** the live Claude Code login to another saved account with one click.
3. **Be warned** (native notification) before a limit stalls work.

---

## 2. Technology stack (detail)

### 2.1 Backend (Rust core)

| Concern | Crate / API | Notes |
|---|---|---|
| App shell, tray, windows, menus | `tauri` v2 | `TrayIconBuilder`, `Menu`/`Submenu`, `WebviewWindowBuilder`. |
| Async runtime | `tokio` (multi-thread) | All I/O (HTTP, keychain CLI, file) is async. |
| HTTP | `reqwest` (rustls TLS, JSON) | Usage fetch + OAuth refresh; no OpenSSL dependency. |
| JSON | `serde` + `serde_json` | Credential blob, profiles, `~/.claude.json`, API responses. |
| Secret storage (PitStopX's own) | `keyring` v3 | macOS Keychain / Windows Credential Manager / Linux Secret Service. |
| Date/time | `chrono` | Reset-time + relative-time formatting (locale-aware where possible). |
| Notifications | `tauri-plugin-notification` | Cross-platform native notifications. |
| Autostart | `tauri-plugin-autostart` | Login items (macOS), Run key (Windows), `.desktop` autostart (Linux). |
| Single instance | `tauri-plugin-single-instance` | Prevents two trays. |
| Persisted prefs | `tauri-plugin-store` | Replaces `UserDefaults` (indicator style/metric, etc.). |
| Logging | `tracing` + `tracing-subscriber` | Structured logs to a per-OS log dir. |
| Error handling | `thiserror` + `anyhow` | Typed domain errors at the API boundary; `anyhow` at the edges. |
| Dynamic tray icon rendering | `tiny-skia` + `ab_glyph` | Render percentage + warning color *into* the tray icon (see §5.3). |

### 2.2 Frontend (popover UI)

| Concern | Tool |
|---|---|
| UI framework | Svelte + TypeScript |
| Bundler | Vite |
| Backend bridge | Tauri `invoke()` commands + events |
| Styling | Plain CSS with CSS custom properties; auto light/dark via `prefers-color-scheme` |

The frontend renders **only** the rich popover panel (account rows). The tray's
right-click/secondary menu uses the **native** Tauri menu. State flows one way:
Rust owns truth and pushes snapshots to the panel via Tauri events; the panel
calls back via `invoke` for actions (switch, save, remove, refresh, set prefs).

### 2.3 Repository layout

```
pitstopx/
├── src-tauri/                       # Rust core
│   ├── Cargo.toml
│   ├── tauri.conf.json              # bundle ids, windows, tray, capabilities
│   ├── capabilities/                # Tauri v2 permission scopes
│   ├── icons/                       # app + tray base icons (all sizes/formats)
│   └── src/
│       ├── main.rs                  # entry, CLI modes, tray + app setup
│       ├── app.rs                   # AppState, refresh loop, action handlers
│       ├── tray.rs                  # tray icon render + native menu build
│       ├── ui_events.rs             # snapshot <-> frontend bridge
│       ├── usage_api.rs             # usage fetch + OAuth refresh + models
│       ├── credentials.rs           # blob + ~/.claude.json parse/patch
│       ├── profile_store.rs         # account model + persistence + switching
│       ├── secrets/                 # secret-store abstraction
│       │   ├── mod.rs               # SecretStore trait
│       │   ├── macos.rs             # /usr/bin/security CLI
│       │   ├── windows.rs           # Credential Manager (keyring)
│       │   └── linux.rs             # Secret Service / libsecret + file fallback
│       ├── claude_source.rs         # locate + read Claude Code's own creds per OS
│       ├── notify.rs                # notification wrapper
│       └── format.rs                # percent/date/relative formatting
├── src/                             # Svelte frontend
│   ├── main.ts
│   ├── App.svelte                   # popover panel
│   ├── lib/AccountRow.svelte        # one account row (email, chip, usage bars)
│   ├── lib/types.ts                 # shared DTOs (mirror Rust serde structs)
│   └── styles.css
├── package.json / vite.config.ts / tsconfig.json
├── scripts/                         # icon generation, release helpers
└── README.md
```

---

## 3. High-level architecture

```
                 ┌───────────────────────────────────────────────┐
                 │                  Rust core                     │
   tray icon ◄───┤  tray.rs ── renders % into icon, builds menu   │
   native menu   │                                                │
                 │  app.rs ── AppState, 120s refresh loop,        │
                 │            backoff, threshold notifications     │
 popover panel   │     │            │              │              │
 (WebView) ◄─────┤  ui_events   usage_api     profile_store       │
   invoke()  ────►     │            │              │   │           │
                 │  notify.rs   reqwest      secrets/  claude_source
                 │                            (per-OS) (per-OS)    │
                 └───────────────────────────────────────────────┘
                              │                    │
              OS secret store (Keychain /      Claude Code creds
              Cred Manager / Secret Service)   (Keychain or ~/.claude/.credentials.json)
                                                   + ~/.claude.json (identity)
```

- The **Rust core is the single source of truth**. It owns the refresh loop,
  per-account caches, backoff state, and all keychain/file writes.
- The **WebView panel** is a pure view + action surface. It never touches secrets;
  it receives `UiSnapshot` events and emits action `invoke`s.
- Two **platform abstractions** isolate every OS difference: `SecretStore` (where
  PitStopX keeps its own copies) and `ClaudeSource` (where Claude Code keeps its
  live login).

---

## 4. Platform abstraction layer

This is the heart of the cross-platform design. Everything OS-specific lives
behind two traits plus a few helpers.

### 4.1 `SecretStore` — PitStopX's own secret storage

```rust
#[async_trait]
trait SecretStore {
    async fn read(&self, account: &str) -> Result<Option<Vec<u8>>>;
    async fn upsert(&self, account: &str, blob: &[u8]) -> Result<()>;
    async fn delete(&self, account: &str) -> Result<()>;
}
```

`account` = the profile email. The stored value is the full Claude Code credential
blob (verbatim). Per-OS implementations:

| OS | Backend | Detail |
|---|---|---|
| **macOS** | `/usr/bin/security` CLI (generic password, service `PitStopX-profile`) | Mirrors PitStop's rationale: routing through the **Apple-signed `security` binary** means one stable requester, so the keychain "Always Allow" grant survives app rebuilds and is shared with Claude Code. Uses the staged delete+add write so a failed write never leaves zero copies. |
| **Windows** | Windows **Credential Manager** via the `keyring` crate (`wincred`) | Stored as a generic credential `PitStopX-profile:<email>`. Encrypted at rest by DPAPI under the user account; no interactive prompt. |
| **Linux** | **Secret Service** (libsecret) via `keyring`, collection `PitStopX` | Requires a running secret service (GNOME Keyring / KWallet). **Fallback** when none is present: an age-encrypted file under the data dir, key derived from a machine-bound secret — clearly logged as a degraded mode. |

### 4.2 `ClaudeSource` — reading Claude Code's *own* live credentials & identity

Claude Code does **not** store its login the same way on every OS. This trait
locates and reads/writes the live login that PitStopX must snapshot and swap.

```rust
#[async_trait]
trait ClaudeSource {
    async fn read_live_blob(&self) -> Result<Option<Vec<u8>>>;     // claudeAiOauth + sections
    async fn write_live_blob(&self, blob: &[u8]) -> Result<()>;    // for switching
    fn identity_path(&self) -> PathBuf;                            // ~/.claude.json
}
```

| OS | Live credential location | Read/Write strategy |
|---|---|---|
| **macOS** | Keychain generic password, service **`Claude Code-credentials`** | In-place update (`security add-generic-password -U`) to preserve the item + its ACL. |
| **Windows** | File **`%USERPROFILE%\.claude\.credentials.json`** | Read/parse JSON; write atomically (temp file + rename) preserving file perms (user-only ACL). |
| **Linux** | File **`$HOME/.claude/.credentials.json`** (or Secret Service if the installed Claude Code uses it) | Same atomic-file strategy; auto-detect which is in use. |

> **Identity** (`oauthAccount`) lives in **`~/.claude.json`** on all three OSes
> (home directory), so only the credential *blob* location differs. The home dir
> is resolved via `dirs::home_dir()`.

Because the credential location is **detected at runtime** and abstracted, the
rest of the app (refresh loop, switching, capture) is fully OS-agnostic.

### 4.3 Other per-OS helpers

- **Autostart** — `tauri-plugin-autostart` (Login Items / `HKCU\…\Run` /
  XDG `~/.config/autostart/*.desktop`).
- **Notifications** — `tauri-plugin-notification` (UserNotifications / Windows
  toast / `notify-send`/`org.freedesktop.Notifications`).
- **Paths** — `dirs`/Tauri path API for config (`~/.config/pitstopx` or
  `%APPDATA%`), data, and log directories.

---

## 5. Tray & menu-bar behavior (the key cross-platform UX shift)

### 5.1 The problem PitStop's design doesn't translate

macOS uniquely lets a status item show **live text** ("`42%`") next to the icon.
Windows and Linux trays are **icon-only** — there is no text slot. So PitStopX
cannot rely on a text title; instead it **renders the percentage and warning
color directly into the tray icon bitmap**, and uses the tooltip for detail.

### 5.2 Indicator preferences (persisted via `tauri-plugin-store`)

Same semantics as PitStop, adapted to icon rendering:

- **`indicatorStyle`** (`store` key): `iconAndPercent` (default) | `iconOnly` |
  `percentOnly` (number-only glyph, no gauge motif).
- **`indicatorMetric`**: `binding` (Highest, default) | `fiveHour` | `weekly`.
  - `binding` → `max(5-hour, weekly)`, or "–" when both windows are absent.

### 5.3 Dynamic tray icon rendering — `tray.rs`

On each refresh (and on preference change) the tray icon is **re-drawn** with
`tiny-skia` + `ab_glyph` and pushed via `tray.set_icon(...)`:

- Base motif: a small checkered-flag / gauge glyph (matching the app icon).
- Overlaid percentage text (`NN%`) when style includes percent, in a high-contrast
  pill so it stays legible on light/dark taskbars.
- **Warning state** by color/badge: a 🟠 badge at `pct ≥ 75`, 🔴 at `pct ≥ 90`;
  **dimmed** (reduced alpha) when the active account's data is stale.
- Rendered at multiple DPIs; macOS uses a template variant so it adapts to the
  menu-bar ink, Windows/Linux use the colored variant.
- **Tooltip** carries the textual detail on all OSes:
  `"<email> — 5-hour NN% · weekly NN%"` (+ a stale note when applicable). This is
  the cross-platform replacement for PitStop's status-item tooltip.

### 5.4 The menu / popover

PitStopX presents two surfaces:

1. **Rich popover panel** (left-click the tray): a frameless, always-on-top
   WebView window anchored near the tray icon, showing the account rows
   (§7). It auto-hides on blur.
2. **Native context menu** (right-click / secondary action): built with Tauri's
   `Menu` API — the controls that are plain items:
   - **Save Current Account**
   - **Remove Account ▸** (submenu of non-active accounts)
   - **Refresh Now**
   - **Menu Bar / Tray Display ▸** (the `indicatorStyle` + `indicatorMetric`
     radio groups)
   - **Launch at Login** (checkbox)
   - **Quit PitStopX**
   - An informational "Updated <time> · refreshes every 2 min" disabled item.

> Rationale: rich, per-row interactive content (hover "Switch" pill, usage bars)
> is painful in a native menu cross-platform, so it lives in the WebView popover;
> simple commands stay in the native menu where they get proper OS styling and
> keyboard handling.

---

## 6. Application core — `app.rs`

`AppState` (held in Tauri's managed state, guarded by an async `Mutex`/`RwLock`)
holds exactly the state PitStop's `AppDelegate` did, now OS-agnostic:

- `profiles: Vec<Profile>`, `active_email: Option<String>`.
- `usage: HashMap<String, UsageReport>` — last **successful** report per account
  (kept on failure → graceful staleness).
- `fetch_error: HashMap<String, String>`.
- `next_fetch_allowed: HashMap<String, Instant>` — per-account backoff gate
  (always future-or-absent).
- `failure_count: HashMap<String, u32>`.
- `last_refresh: Option<DateTime>`, `last_top_level_error: Option<String>`.
- `refreshing: bool`, `refresh_queued: bool` (single-flight + coalescing).
- `notified_bucket: HashMap<String, u8>` (0/1/2 — once per threshold crossing).
- Constants: `REFRESH_INTERVAL = 120s`, `MENU_REFRESH_DEBOUNCE = 30s`.

### 6.1 Refresh pipeline (`refresh_all`)

A Tokio task, single-flighted by `refreshing`:

1. `capture_current()` — snapshot the live account into a profile (keeps the saved
   copy's tokens current). Failures recorded in `last_top_level_error`.
2. Reload profiles; refresh `active_email` from `~/.claude.json`.
3. For each profile (skipping those still inside their backoff window):
   - `fresh_credentials()` → non-expired creds (refresh via OAuth grant if needed,
     persist the result).
   - `usage_api::fetch_usage()` → store report; clear error/failure/backoff.
   - **Errors:**
     - `RateLimited(retry_after)` → backoff = `retry_after ?? min(120·2^(n-1), 900)`
       (2→4→…→15 min cap), honoring `Retry-After`.
     - `Unauthorized` → back off **1 hour** (a rejected token won't self-heal;
       cleared by Refresh Now or a re-login picked up by capture).
     - other → record message, no backoff.
4. `last_refresh = now`; re-render tray icon; push a fresh `UiSnapshot` to the
   panel; run `check_thresholds()`.
5. **One-shot backoff retry**: schedule a `tokio::time::sleep_until` for the
   earliest future gate (floored at 10s, skipped if ≥ the regular interval), so a
   rate-limited account doesn't idle a full cycle.

The regular cadence is a `tokio::time::interval(120s)` loop; opening the popover
triggers an immediate refresh **only** if data is older than the 30s debounce or a
backoff retry is overdue.

### 6.2 Credential freshness (`fresh_credentials`)

Identical logic to PitStop: read the blob (live item for the active account, saved
item otherwise); if not expired return as-is; if expired with a refresh token, run
the OAuth refresh grant, patch only `claudeAiOauth` in the blob, persist, return.
`is_expired` keeps the 2-minute safety margin (`now ≥ expires_at − 120s`).

### 6.3 Actions (Tauri commands invoked from the panel, plus native-menu handlers)

- `switch_to(email)` → `profile_store::switch_to`; update active; reset that
  account's notify bucket; post "Switched to …" notification; refresh.
- `save_current()` → `capture_current`; notify "Saved …" / "Nothing to save".
- `remove_account(email)` → delete saved item + profile; clear caches.
- `refresh_now()` → clear **all** backoff, then refresh (coalesced if in flight).
- `set_indicator_style/metric(value)` → persist; re-render tray.
- `set_launch_at_login(bool)` → autostart plugin.
- `quit()`.

### 6.4 Threshold notifications (`check_thresholds`)

For the **active** account with a current, error-free report:
`bucket = 2 if pct≥95 else 1 if pct≥80 else 0`; notify only on an **upward**
crossing. The body names the binding window's reset time and the **best pit** —
the saved non-active account with the lowest `max_utilization`:

- usage `< 80%` → "Best pit: <email> (NN% used) — switch from the menu."
- else if any other account → "All saved accounts are running hot — check the menu."
- else → "Add a second account in PitStopX to keep working."

---

## 7. Popover UI — Svelte frontend

### 7.1 Data contract

Rust serializes a `UiSnapshot` (`serde` → camelCase JSON), emitted on every
refresh and after each action:

```ts
interface UiSnapshot {
  activeEmail: string | null;
  lastRefresh: string | null;        // ISO; formatted client-side
  lastTopLevelError: string | null;
  rows: AccountRowDTO[];             // pre-sorted: active first, then emptiest
}
interface AccountRowDTO {
  email: string;
  planLabel: string;                 // "Acme AI · Team · 5x"
  isActive: boolean;
  bars: { label: "5h" | "7d"; utilization: number | null; resetText: string }[];
  modelsLine: string | null;         // "Opus wk 12% · Sonnet wk 10% · Extra 4%"
  statusLine: string | null;         // error / stale / loading
  switchable: boolean;
}
```

Row order is computed in Rust (active first, then ascending `max_utilization` —
emptiest next), so the UI never re-sorts under the cursor.

### 7.2 `AccountRow.svelte`

Recreates PitStop's row visuals as DOM/CSS:

- Coral active-dot; email (semibold when active, truncated); a **plan chip** that
  flips to a coral **"Switch" pill** on hover of a switchable row, click → `invoke('switch_to', { email })`.
- Two usage bars (`5h`, `7d`): rounded track + proportional fill colored **green
  < 70 %, orange 70–90 %, red ≥ 90 %**, right-aligned monospaced percentage,
  reset text (`"9:49 PM · 3h 34m"` / `"Thu 10:29 AM · 5d 16h"`).
- Optional `modelsLine` (per-model + extra-usage) and `statusLine` (orange;
  error/stale/loading with retry hint).
- Brand coral `#D97757`; light/dark via `prefers-color-scheme`.

### 7.3 Empty / loading states

- No profiles → "No accounts found — log in with `claude` first."
- A row with no report yet → `statusLine = "Loading…"`.
- Stale row → `"⚠ <error> · showing <time> data — retrying in Xm"`.

---

## 8. Domain model & persistence

### 8.1 `Profile` & `ProfileStore` — `profile_store.rs`

Non-secret metadata persisted to **`<config>/pitstopx/profiles.json`**
(`~/.config/pitstopx` on macOS/Linux, `%APPDATA%\pitstopx` on Windows):

- `email`, `saved_at`, `subscription_type?`, `rate_limit_tier?`,
  `oauth_account: serde_json::Value` (verbatim `oauthAccount` from `~/.claude.json`).
- Derived **`plan_label`**: join, with `" · "`, of the org name (dropping
  auto-generated `"<email>'s Organization"`), the capitalized `subscription_type`,
  and the tier suffix after `max_` (`"5x"`/`"20x"`).

`ProfileStore` methods mirror PitStop exactly:

- `capture_current()` — snapshot live blob + identity into a profile; **short-
  circuits** when nothing changed (blob + `oauthAccount` byte-equal) to avoid
  needless secret-store/file writes.
- `switch_to(email)` — **capture first** (a failed snapshot aborts the switch so
  the outgoing refresh token can't be lost), then write the saved blob into the
  live location (`ClaudeSource::write_live_blob`) and restore `oauthAccount` in
  `~/.claude.json` (atomic write of the whole file, only `oauthAccount` changed).
- `remove(email)`, `blob_for(email, is_active)` (live item for active, saved
  otherwise), `store_refreshed_blob(...)`.

Because the **whole blob is swapped**, per-account MCP OAuth tokens (e.g.
Atlassian) travel with the account on switch.

### 8.2 Credentials — `credentials.rs`

- `OAuthCredentials { access_token, refresh_token?, expires_at_ms, subscription_type?, rate_limit_tier? }`,
  `is_expired` with the 120s margin.
- `CredentialBlob::parse` (requires `claudeAiOauth.accessToken`) and
  `CredentialBlob::patching` (rewrites only the token fields inside
  `claudeAiOauth`, leaving `mcpOAuth` and other sections intact).
- `ClaudeConfig` over `~/.claude.json`: `oauth_account()`, `active_email()`,
  `set_oauth_account()` (atomic, single-key replace).

---

## 9. Usage API & OAuth — `usage_api.rs`

Unchanged protocol surface (the same unofficial OAuth surface Claude Code uses):

- **Usage:** `GET https://api.anthropic.com/api/oauth/usage` with
  `Authorization: Bearer <token>`, `anthropic-beta: oauth-2025-04-20`,
  `Content-Type: application/json`, 15s timeout.
  - `401/403` → `Unauthorized`; `429` → `RateLimited` (parse `Retry-After`
    seconds); non-`200` → `Http(code)`.
  - Parses `five_hour`, `seven_day`, `seven_day_opus`, `seven_day_sonnet`, and
    `extra_usage { is_enabled, utilization }`. `resets_at` parsed as ISO-8601
    (fractional seconds first, then plain).
- **Refresh:** `POST https://console.anthropic.com/v1/oauth/token`, JSON body
  `{ grant_type: "refresh_token", refresh_token, client_id }` with the public
  PKCE **client ID `9d1c250a-e61b-44d9-88ed-5944d1962f5e`**. `expires_at_ms =
  (now + expires_in) * 1000`. **Only inactive/saved profiles** are refreshed by
  PitStopX; the active account is kept fresh by Claude Code.
- `UsageReport::max_utilization = max(fiveHour, sevenDay)`; `binding_window`
  (ties → 5-hour) for reset display.

`reqwest` with `rustls` keeps TLS dependency-free and identical across OSes.

---

## 10. Formatting — `format.rs`

`chrono`-based, locale-aware where the platform exposes locale:

- `percent` → `"NN%"` / `"–"`.
- `reset` → `"resets 9:49 PM (in 3h 34m)"` (day-qualified when not today).
- `relative` / `relative_short` → `"in 3h 34m"` / `"3h 34m"`, day-aware.
- `compact_reset` → `"9:49 PM · 3h 34m"` / `"Thu 10:29 AM · 5d 16h"`.
- `updated` → `HH:MM:SS` for the "Updated …" line and stale stamps.

---

## 11. Notifications — `notify.rs`

Wraps `tauri-plugin-notification`. Requests OS permission lazily on first send
(macOS prompts; Windows/Linux generally implicit), queues notifications issued
before the grant resolves, and drops them if denied — the same state-machine
behavior as PitStop's `Notifier`. Emitted on: threshold crossings, account
switch, account save.

---

## 12. CLI / diagnostic modes — `main.rs`

The same binary supports headless modes (parsed from argv before building the
tray), useful for support and CI:

- `--check` — print saved accounts + live usage to stdout (capture, refresh
  stale tokens, fetch usage), no tray. Cross-platform replacement for PitStop's
  `--check`.
- `--screenshot` — run the app with masked sample emails for store/README
  captures.
- `--print-paths` — print the resolved config/data/log dirs and the detected
  Claude Code credential location per OS (diagnostic for the platform layer).

A second launch with no flag is intercepted by `tauri-plugin-single-instance` and
focuses the existing tray/popover instead of starting a second process.

---

## 13. Build & packaging (per OS)

Driven by `tauri build` (which wraps `cargo build --release` + bundling):

| OS | Artifacts | Signing / notes |
|---|---|---|
| **macOS** | `.app`, `.dmg` | Developer ID codesign + **notarization** (Tauri bundler hooks). Universal binary (x86-64 + arm64). |
| **Windows** | `.msi` (WiX) and/or NSIS `.exe`; WebView2 bootstrapper | Authenticode signing; WebView2 evergreen runtime auto-provisioned. |
| **Linux** | `.AppImage`, `.deb`, `.rpm` | Depends on WebKitGTK + libappindicator/`ayatana`; declared as package deps. |

- **CI**: a GitHub Actions matrix (macos-latest, windows-latest, ubuntu-latest)
  runs `cargo test`, `cargo clippy`, `cargo fmt --check`, frontend `vitest`, then
  `tauri build`, attaching artifacts to a release.
- **Versioning**: single source of truth in `tauri.conf.json`
  (`version`), surfaced into `Cargo.toml`/`package.json` at build.
- **Icons**: a `scripts/make-icon.*` step regenerates the gauge-over-checkered-
  flag app icon and tray base glyphs at all required sizes/formats (`.icns`,
  `.ico`, PNG set) from one vector definition.

---

## 14. Security model & trade-offs

- **Secrets** (the credential blob incl. refresh + MCP OAuth tokens) live **only**
  in the OS secret store on macOS/Windows and in the secret store **or** an
  encrypted fallback file on Linux — never in `profiles.json`. Non-secret identity
  lives in `profiles.json` and `~/.claude.json`.
- **macOS** keeps PitStop's exact model: all keychain access via the Apple-signed
  `/usr/bin/security`, so the one-time **"Always Allow"** grant survives rebuilds
  and is shared with Claude Code. (Trade-off: blob passed via `argv`, briefly
  visible in the process list — same exposure Claude Code has.)
- **Windows** Credential Manager entries are DPAPI-protected per user; no argv
  exposure (the `keyring` crate uses the API directly).
- **Linux** prefers Secret Service (D-Bus, no argv exposure). When no secret
  service is available, the encrypted-file fallback is used and **logged as
  degraded**; the threat model there is weaker and documented.
- **Claude Code's own credentials on Windows/Linux are a plaintext file**
  (`~/.claude/.credentials.json`) that PitStopX reads/writes; PitStopX preserves
  the file's user-only permissions and writes atomically. This is Claude Code's
  storage choice, not PitStopX's, but is called out so users understand the
  baseline.
- The usage endpoint, OAuth refresh flow, and public client ID are the same
  **unofficial** surface Claude Code uses; if Anthropic changes them,
  `usage_api.rs` must be updated.

---

## 15. Behavior parity matrix (PitStop → PitStopX)

| Behavior | PitStop (macOS) | PitStopX (cross-platform) |
|---|---|---|
| Indicator number | Text in the menu bar | Rendered **into** the tray icon + tooltip |
| Warning levels | 🟠 ≥75 %, 🔴 ≥90 %, dim when stale | Same, as icon badge/color + dim |
| Rich account rows | Custom `NSView` | Svelte popover panel |
| Simple commands | `NSMenu` | Native Tauri tray menu |
| Secret store | Keychain via `security` CLI | `SecretStore` per OS (Keychain / Cred Manager / Secret Service+fallback) |
| Live Claude creds | Keychain item | `ClaudeSource` per OS (Keychain / `.credentials.json`) |
| Identity (`oauthAccount`) | `~/.claude.json` | `~/.claude.json` (same on all OSes) |
| Refresh / backoff / thresholds | `AppDelegate` | `app.rs` (identical logic) |
| Autostart | `SMAppService` | `tauri-plugin-autostart` |
| Notifications | `UserNotifications` | `tauri-plugin-notification` |
| Persisted prefs | `UserDefaults` | `tauri-plugin-store` |

---

## 16. Open questions / follow-ups

1. **Linux secret backend variance.** Confirm GNOME Keyring vs KWallet behavior
   and finalize the encrypted-file fallback's key derivation.
2. **Claude Code credential location drift.** Verify the exact Windows/Linux file
   path and whether any Claude Code build uses Secret Service on Linux; keep
   `ClaudeSource` detection resilient to changes.
3. **Tray popover anchoring** on Wayland (where global cursor/tray geometry is
   restricted) — may require a centered fallback window.
4. **Windows toast actions** — optionally add a "Switch to best pit" action button
   directly on the threshold notification.
```
