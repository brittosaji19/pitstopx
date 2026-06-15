<script lang="ts">
  import { invoke } from "@tauri-apps/api/core";
  import { createEventDispatcher, onMount } from "svelte";
  import type { Settings } from "./types";

  const dispatch = createEventDispatcher<{ close: void }>();

  let claudeBin = "";
  let codexBin = "";
  let claudeResolved: string | null = null;
  let codexResolved: string | null = null;
  let saving: "anthropic" | "openai" | null = null;

  // Global open-popover hotkey (accelerator string like "CmdOrCtrl+Shift+U").
  let shortcut = "";
  let shortcutErr = "";
  let savingShortcut = false;

  async function load() {
    try {
      const s = await invoke<Settings>("get_settings");
      claudeBin = s.claudeBin ?? "";
      codexBin = s.codexBin ?? "";
      claudeResolved = s.claudeResolved;
      codexResolved = s.codexResolved;
      shortcut = s.shortcut ?? "";
    } catch (err) {
      console.error("get_settings failed", err);
    }
  }

  onMount(load);

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

<div class="settings">
  <header class="settings-head">
    <button class="icon-btn" title="Back" on:click={() => dispatch("close")}>←</button>
    <span class="settings-title">Settings</span>
  </header>

  <div class="settings-body">
    <div class="setting-group">
      <div class="setting-label">Open PitStopX shortcut</div>
      <div class="setting-row">
        <input
          class="setting-input"
          readonly
          value={prettyShortcut(shortcut)}
          placeholder="Click, then press a key combo"
          on:keydown={onShortcutKeydown}
        />
        <button class="action" on:click={() => saveShortcut(shortcut)} disabled={savingShortcut}>
          {savingShortcut ? "Saving…" : "Save"}
        </button>
        <button class="action" on:click={() => saveShortcut("")} disabled={savingShortcut} title="No shortcut">
          Clear
        </button>
      </div>
      <div class="setting-hint" class:err={!!shortcutErr}>
        {shortcutErr ||
          "Global hotkey to open the panel — click the field and press a combo (e.g. Ctrl/⌘ + Shift + U)."}
      </div>
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
        <button class="action" on:click={() => save("anthropic", claudeBin)} disabled={saving === "anthropic"}>
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
        <button class="action" on:click={() => save("openai", codexBin)} disabled={saving === "openai"}>
          {saving === "openai" ? "Saving…" : "Save"}
        </button>
      </div>
      <div class="setting-hint" class:err={!codexResolved} title={codexResolved ?? ""}>
        {codexResolved ? `Using: ${codexResolved}` : "Not found — set the full path to the codex executable"}
      </div>
    </div>
  </div>
</div>
