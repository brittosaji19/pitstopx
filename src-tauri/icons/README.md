# Icons

These binary icon assets are **generated**, not committed, to keep the repo
text-only. Before the first `tauri build`, generate them from the vector source:

```bash
node scripts/make-icon.mjs
```

This produces, into this directory, every size/format the bundler references in
`tauri.conf.json`:

- `32x32.png`, `128x128.png`, `128x128@2x.png` — Linux / generic
- `icon.icns` — macOS
- `icon.ico` — Windows

The tray icon is **not** an asset: it is rendered at runtime in
`src/tray.rs` (gauge motif + percentage + warning badge), so no tray PNG is
needed here.
