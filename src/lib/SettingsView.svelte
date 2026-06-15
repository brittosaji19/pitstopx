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

  async function load() {
    try {
      const s = await invoke<Settings>("get_settings");
      claudeBin = s.claudeBin ?? "";
      codexBin = s.codexBin ?? "";
      claudeResolved = s.claudeResolved;
      codexResolved = s.codexResolved;
    } catch (err) {
      console.error("get_settings failed", err);
    }
  }

  onMount(load);

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
