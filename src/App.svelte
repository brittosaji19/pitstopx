<script lang="ts">
  import { onMount, onDestroy } from "svelte";
  import { invoke } from "@tauri-apps/api/core";
  import { listen, type UnlistenFn } from "@tauri-apps/api/event";
  import AccountRow from "./lib/AccountRow.svelte";
  import type { UiSnapshot } from "./lib/types";

  const SNAPSHOT_EVENT = "pitstopx://snapshot";

  let snapshot: UiSnapshot | null = null;
  let unlisten: UnlistenFn | null = null;

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

  onDestroy(() => unlisten?.());

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
</script>

<main class="panel">
  {#if !snapshot}
    <div class="empty">Loading…</div>
  {:else if snapshot.rows.length === 0}
    <div class="empty">
      No accounts found — log in with <code>claude</code> first.
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
    <button class="refresh" on:click={refresh} title="Refresh now">↻</button>
    <span class="updated">
      Updated {formatUpdated(snapshot?.lastRefresh ?? null)} · every 2 min
    </span>
  </footer>
</main>
