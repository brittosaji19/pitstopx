<script lang="ts">
  import { onMount, onDestroy } from "svelte";
  import { invoke } from "@tauri-apps/api/core";
  import { listen, type UnlistenFn } from "@tauri-apps/api/event";
  import { getCurrentWindow, LogicalSize } from "@tauri-apps/api/window";
  import { fly, scale, fade } from "svelte/transition";
  import { flip } from "svelte/animate";
  import { cubicOut } from "svelte/easing";
  import AccountRow from "./lib/AccountRow.svelte";
  import SettingsView from "./lib/SettingsView.svelte";
  import type { UiSnapshot } from "./lib/types";

  const SNAPSHOT_EVENT = "pitstopx://snapshot";
  const RESET_VIEW_EVENT = "pitstopx://reset-view";
  const WIDTH = 372;
  const MAX_H = 600;

  let view: "accounts" | "settings" = "accounts";
  let snapshot: UiSnapshot | null = null;
  let unlisten: UnlistenFn | null = null;
  let unlistenReset: UnlistenFn | null = null;
  let actionMsg = "";
  let busy = false;
  let spinning = false;
  let msgTimer: ReturnType<typeof setTimeout> | undefined;

  let panelEl: HTMLElement;
  let ro: ResizeObserver | null = null;
  let fitRAF = 0;

  function flash(msg: string) {
    actionMsg = msg;
    clearTimeout(msgTimer);
    msgTimer = setTimeout(() => (actionMsg = ""), 4000);
  }

  // Size the window to the panel's content so the UI never needs to scroll.
  function fitWindow() {
    cancelAnimationFrame(fitRAF);
    fitRAF = requestAnimationFrame(async () => {
      if (!panelEl) return;
      const h = Math.ceil(panelEl.getBoundingClientRect().height); // panel is edge-to-edge
      try {
        await getCurrentWindow().setSize(new LogicalSize(WIDTH, Math.min(MAX_H, Math.max(120, h))));
      } catch (err) {
        /* window may be closing */
      }
    });
  }

  onMount(async () => {
    ro = new ResizeObserver(() => fitWindow());
    if (panelEl) ro.observe(panelEl);

    unlisten = await listen<UiSnapshot>(SNAPSHOT_EVENT, (e) => {
      snapshot = e.payload;
    });
    unlistenReset = await listen(RESET_VIEW_EVENT, () => {
      view = "accounts";
    });
    try {
      await invoke("request_snapshot");
    } catch (err) {
      console.error("request_snapshot failed", err);
    }
  });

  onDestroy(() => {
    ro?.disconnect();
    unlisten?.();
    unlistenReset?.();
    clearTimeout(msgTimer);
  });

  function formatUpdated(iso: string | null): string {
    if (!iso) return "never";
    return new Date(iso).toLocaleTimeString(undefined, {
      hour: "2-digit",
      minute: "2-digit",
      second: "2-digit",
    });
  }

  function toggleSettings() {
    view = view === "settings" ? "accounts" : "settings";
  }

  // Close = hide the popover (it's a tray app; Quit lives in the tray menu).
  async function close() {
    view = "accounts";
    try {
      await getCurrentWindow().hide();
    } catch (err) {
      /* window may already be hidden */
    }
  }

  async function refresh() {
    spinning = true;
    setTimeout(() => (spinning = false), 600);
    try {
      await invoke("refresh_now");
    } catch (err) {
      console.error("refresh_now failed", err);
    }
  }

  async function login(provider: "anthropic" | "openai", label: string) {
    if (busy) return;
    busy = true;
    flash(`Opening ${label} login…`);
    try {
      await invoke("login_new", { provider });
      flash(`Finish ${label} login in the terminal that opened`);
    } catch (err) {
      // Backend returns a user-facing message (e.g. "Codex CLI isn't installed…").
      console.error("login_new failed", err);
      flash(String(err));
    } finally {
      busy = false;
    }
  }

  async function saveCurrent() {
    if (busy) return;
    busy = true;
    flash("Saving…");
    try {
      await invoke("save_current");
      flash("Saved current account ✓");
    } catch (err) {
      console.error("save_current failed", err);
      flash(`Save failed: ${err}`);
    } finally {
      busy = false;
    }
  }
</script>

<main class="panel" bind:this={panelEl}>
  <header class="topbar">
    <div class="brand">
      <span class="brand-dot" aria-hidden="true"></span>
      <span class="brand-name">{view === "settings" ? "Settings" : "PitStopX"}</span>
    </div>
    <div class="topbar-actions">
      {#if view !== "settings"}
        <button class="icon-btn" class:spin={spinning} on:click={refresh} title="Refresh now">↻</button>
      {/if}
      <button class="icon-btn" on:click={toggleSettings} title={view === "settings" ? "Back" : "Settings"}>
        {view === "settings" ? "←" : "⚙"}
      </button>
      <button class="icon-btn close" on:click={close} title="Close">✕</button>
    </div>
  </header>

  {#if view === "settings"}
    <div class="view" in:fade={{ duration: 140 }}>
      <SettingsView />
    </div>
  {:else}
    <div class="view" in:fade={{ duration: 140 }}>
      {#if !snapshot}
        <div class="loading"><span class="spinner"></span> Loading…</div>
      {:else if snapshot.rows.length === 0}
        <div class="empty">
          No accounts found — log in with <code>claude</code> or <code>codex</code> first.
        </div>
      {:else}
        {#if snapshot.lastTopLevelError}
          <div class="banner" role="alert">⚠ {snapshot.lastTopLevelError}</div>
        {/if}
        <div class="cards">
          {#each snapshot.rows as row (row.providerId + ":" + row.email)}
            <div
              class="card-wrap"
              animate:flip={{ duration: 320, easing: cubicOut }}
              in:fly={{ y: 10, duration: 260, easing: cubicOut }}
              out:scale={{ start: 0.96, duration: 150 }}
            >
              <AccountRow {row} />
            </div>
          {/each}
        </div>
      {/if}

      <div class="addbar">
        <span class="addbar-label">Add</span>
        <button class="pill" on:click={() => login("anthropic", "Claude")} disabled={busy}>＋ Claude</button>
        <button class="pill" on:click={() => login("openai", "Codex")} disabled={busy}>＋ Codex</button>
        <button class="pill" on:click={saveCurrent} disabled={busy} title="Save the current account so you can switch back later">
          Save
        </button>
      </div>
    </div>
  {/if}

  <footer class="statusbar">
    <span class="status-text" class:flash={!!actionMsg}>
      {#if actionMsg}{actionMsg}{:else if snapshot}Updated {formatUpdated(snapshot.lastRefresh)}{/if}
    </span>
  </footer>
</main>
