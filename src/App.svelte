<script lang="ts">
  import { onMount, onDestroy } from "svelte";
  import { invoke } from "@tauri-apps/api/core";
  import { listen, type UnlistenFn } from "@tauri-apps/api/event";
  import AccountRow from "./lib/AccountRow.svelte";
  import type { UiSnapshot } from "./lib/types";

  const SNAPSHOT_EVENT = "pitstopx://snapshot";

  let snapshot: UiSnapshot | null = null;
  let unlisten: UnlistenFn | null = null;
  let actionMsg = "";
  let busy = false;
  let msgTimer: ReturnType<typeof setTimeout> | undefined;

  function flash(msg: string) {
    actionMsg = msg;
    clearTimeout(msgTimer);
    msgTimer = setTimeout(() => (actionMsg = ""), 4000);
  }

  onMount(async () => {
    // Subscribe before requesting, so we don't miss the reply.
    unlisten = await listen<UiSnapshot>(SNAPSHOT_EVENT, (e) => {
      snapshot = e.payload;
    });
    // Pull the current snapshot for an instant first paint.
    try {
      await invoke("request_snapshot");
    } catch (err) {
      console.error("request_snapshot failed", err);
    }
  });

  onDestroy(() => {
    unlisten?.();
    clearTimeout(msgTimer);
  });

  function formatUpdated(iso: string | null): string {
    if (!iso) return "never";
    const d = new Date(iso);
    return d.toLocaleTimeString(undefined, {
      hour: "2-digit",
      minute: "2-digit",
      second: "2-digit",
    });
  }

  async function refresh() {
    try {
      await invoke("refresh_now");
    } catch (err) {
      console.error("refresh_now failed", err);
    }
  }

  // Snapshot the currently-logged-in account(s) into the app so they can be
  // switched back to later.
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

<main class="panel">
  {#if !snapshot}
    <div class="empty">Loading…</div>
  {:else if snapshot.rows.length === 0}
    <div class="empty">
      No accounts found — log in with <code>claude</code> or <code>codex</code> first.
    </div>
  {:else}
    {#if snapshot.lastTopLevelError}
      <div class="banner" role="alert">⚠ {snapshot.lastTopLevelError}</div>
    {/if}
    <div class="rows">
      {#each snapshot.rows as row (row.providerId + ":" + row.email)}
        <AccountRow {row} />
      {/each}
    </div>
  {/if}

  <footer class="footer">
    <button class="action save" on:click={saveCurrent} disabled={busy} title="Save the current account so you can switch back to it later">
      ＋ Save current
    </button>
    <button class="action refresh" on:click={refresh} title="Refresh now">↻</button>
    <span class="updated">
      {#if actionMsg}
        {actionMsg}
      {:else}
        Updated {formatUpdated(snapshot?.lastRefresh ?? null)}
      {/if}
    </span>
  </footer>
</main>
