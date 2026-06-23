<script lang="ts">
  import { invoke } from "@tauri-apps/api/core";
  import { listen, type UnlistenFn } from "@tauri-apps/api/event";
  import { onDestroy, onMount } from "svelte";
  import type { Settings } from "./types";

  const SHORTCUT_EVENT = "pitstopx://shortcut";

  let claudeBin = "";
  let codexBin = "";
  let claudeResolved: string | null = null;
  let codexResolved: string | null = null;
  let saving: "anthropic" | "openai" | null = null;

  // Global open-popover hotkey (accelerator string like "CmdOrCtrl+Shift+U").
  let shortcut = "";
  let shortcutErr = "";
  let savingShortcut = false;
  // Linux portal: the compositor owns the key, so we show it read-only and hand
  // off rebinding to GNOME instead of editing the accelerator here.
  let shortcutManaged = false;
  let shortcutTrigger: string | null = null;

  async function load() {
    try {
      const s = await invoke<Settings>("get_settings");
      claudeBin = s.claudeBin ?? "";
      codexBin = s.codexBin ?? "";
      claudeResolved = s.claudeResolved;
      codexResolved = s.codexResolved;
      shortcut = s.shortcut ?? "";
      shortcutManaged = s.shortcutManaged;
      shortcutTrigger = s.shortcutTrigger;
    } catch (err) {
      console.error("get_settings failed", err);
    }
  }

  onMount(load);

  // The backend emits this when a portal-managed binding changes (initial bind
  // or after the user reconfigures it in GNOME), so refresh the displayed key.
  let unlistenShortcut: UnlistenFn | null = null;
  onMount(async () => {
    unlistenShortcut = await listen(SHORTCUT_EVENT, load);
  });
  onDestroy(() => unlistenShortcut?.());

  // GNOME reports the managed trigger as a GTK accelerator label like
  // "Press <Shift><Control>u"; render it as "Shift + Ctrl + U".
  function prettyTrigger(desc: string | null): string {
    if (!desc) return "Not set";
    const body = desc.replace(/^Press\s+/i, "");
    const modMap: Record<string, string> = {
      Shift: "Shift",
      Control: "Ctrl",
      Primary: "Ctrl",
      Ctrl: "Ctrl",
      Alt: "Alt",
      Super: "Super",
      Meta: "Meta",
      Hyper: "Hyper",
    };
    const mods = [...body.matchAll(/<([^>]+)>/g)].map((m) => modMap[m[1]] ?? m[1]);
    const key = body.slice(body.lastIndexOf(">") + 1).trim();
    const parts = [...mods];
    if (key) parts.push(key.length === 1 ? key.toUpperCase() : key);
    return parts.length ? parts.join(" + ") : body;
  }

  // Ask the compositor to open its reconfiguration UI (Linux portal). The new
  // key arrives via SHORTCUT_EVENT, so no manual reload is needed.
  async function changeManagedShortcut() {
    if (savingShortcut) return;
    savingShortcut = true;
    shortcutErr = "";
    try {
      await invoke("configure_shortcut");
    } catch (err) {
      shortcutErr = `${err}. Change it in GNOME Settings → Keyboard.`;
    } finally {
      savingShortcut = false;
    }
  }

  // Readable form for display; CmdOrCtrl renders as ⌘ on macOS / Ctrl elsewhere.
  function prettyShortcut(accel: string): string {
    return accel
      .split("+")
      .map((p) => (p === "CmdOrCtrl" ? "Ctrl/⌘" : p))
      .join(" + ");
  }

  function keyName(k: string): string {
    if (k.length === 1) return k.toUpperCase();
    const map: Record<string, string> = {
      " ": "Space",
      ArrowUp: "Up",
      ArrowDown: "Down",
      ArrowLeft: "Left",
      ArrowRight: "Right",
      Enter: "Enter",
      Tab: "Tab",
      Backspace: "Backspace",
      Delete: "Delete",
      Home: "Home",
      End: "End",
      PageUp: "PageUp",
      PageDown: "PageDown",
    };
    if (map[k]) return map[k];
    return /^F([1-9]|1[0-9]|2[0-4])$/.test(k) ? k : "";
  }

  // Capture a key combo from the (read-only) input and build an accelerator.
  function onShortcutKeydown(e: KeyboardEvent) {
    e.preventDefault();
    if (["Control", "Shift", "Alt", "Meta", "OS", "Dead"].includes(e.key)) return;
    const mods: string[] = [];
    if (e.ctrlKey || e.metaKey) mods.push("CmdOrCtrl");
    if (e.altKey) mods.push("Alt");
    if (e.shiftKey) mods.push("Shift");
    const main = keyName(e.key);
    if (!main) return;
    if (mods.length === 0) {
      shortcutErr = "Use at least one modifier (Ctrl/⌘, Alt, or Shift).";
      return;
    }
    shortcutErr = "";
    shortcut = [...mods, main].join("+");
  }

  async function saveShortcut(accel: string) {
    if (savingShortcut) return;
    savingShortcut = true;
    shortcutErr = "";
    try {
      await invoke("set_shortcut", { shortcut: accel });
      await load();
    } catch (err) {
      shortcutErr = String(err);
    } finally {
      savingShortcut = false;
    }
  }

  // Persist a manual CLI path (blank reverts to auto-detect), then refresh the
  // resolved hints so the user immediately sees what will be used.
  async function save(provider: "anthropic" | "openai", path: string) {
    if (saving) return;
    saving = provider;
    try {
      await invoke("set_cli_path", { provider, path });
      await load();
    } catch (err) {
      console.error("set_cli_path failed", err);
    } finally {
      saving = null;
    }
  }
</script>

<div class="settings-body">
    <div class="setting-group">
      <div class="setting-label">Open PitStopX shortcut</div>
      {#if shortcutManaged}
        <!-- Linux portal: the compositor owns the key. Show what it assigned and
             hand off rebinding to GNOME. -->
        <div class="setting-row">
          <input
            class="setting-input"
            readonly
            value={prettyTrigger(shortcutTrigger)}
            title={shortcutTrigger ?? "Managed by your desktop"}
          />
          <button class="pill" on:click={changeManagedShortcut} disabled={savingShortcut}>
            {savingShortcut ? "Opening…" : "Change…"}
          </button>
        </div>
        <div class="setting-hint" class:err={!!shortcutErr}>
          {shortcutErr ||
            "Your desktop manages this global hotkey. Use “Change…” to reassign it (or set it in GNOME Settings → Keyboard)."}
        </div>
      {:else}
        <div class="setting-row">
          <input
            class="setting-input"
            readonly
            value={prettyShortcut(shortcut)}
            placeholder="Click, then press a key combo"
            on:keydown={onShortcutKeydown}
          />
          <button class="pill" on:click={() => saveShortcut(shortcut)} disabled={savingShortcut}>
            {savingShortcut ? "Saving…" : "Save"}
          </button>
          <button class="pill" on:click={() => saveShortcut("")} disabled={savingShortcut} title="No shortcut">
            Clear
          </button>
        </div>
        <div class="setting-hint" class:err={!!shortcutErr}>
          {shortcutErr ||
            "Global hotkey to open the panel — click the field and press a combo (e.g. Ctrl/⌘ + Shift + U)."}
        </div>
      {/if}
    </div>

    <p class="settings-note">
      Set the full path to a provider's CLI if PitStopX can't find it automatically. Leave blank to
      auto-detect.
    </p>

    <div class="setting-group">
      <div class="setting-label">Claude CLI path</div>
      <div class="setting-row">
        <input
          class="setting-input"
          bind:value={claudeBin}
          placeholder="Auto-detect (leave blank)"
          spellcheck="false"
          autocapitalize="off"
          autocomplete="off"
        />
        <button class="pill" on:click={() => save("anthropic", claudeBin)} disabled={saving === "anthropic"}>
          {saving === "anthropic" ? "Saving…" : "Save"}
        </button>
      </div>
      <div class="setting-hint" class:err={!claudeResolved} title={claudeResolved ?? ""}>
        {claudeResolved ? `Using: ${claudeResolved}` : "Not found — set the full path to the claude executable"}
      </div>
    </div>

    <div class="setting-group">
      <div class="setting-label">Codex CLI path</div>
      <div class="setting-row">
        <input
          class="setting-input"
          bind:value={codexBin}
          placeholder="Auto-detect (leave blank)"
          spellcheck="false"
          autocapitalize="off"
          autocomplete="off"
        />
        <button class="pill" on:click={() => save("openai", codexBin)} disabled={saving === "openai"}>
          {saving === "openai" ? "Saving…" : "Save"}
        </button>
      </div>
      <div class="setting-hint" class:err={!codexResolved} title={codexResolved ?? ""}>
        {codexResolved ? `Using: ${codexResolved}` : "Not found — set the full path to the codex executable"}
      </div>
    </div>
</div>
