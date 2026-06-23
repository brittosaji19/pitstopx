<script lang="ts">
  import { invoke } from "@tauri-apps/api/core";
  import { onMount } from "svelte";
  import type { AccountRowDTO } from "./types";
  import { barClass, pctText, fillWidth } from "./usage";

  export let row: AccountRowDTO;

  let switching = false;
  let reauthing = false;
  let removing = false;
  let confirmRemove = false;

  // Animate the meter fills up from zero on first paint.
  let mounted = false;
  onMount(() => requestAnimationFrame(() => (mounted = true)));

  async function onSwitch() {
    if (!row.switchable || switching) return;
    switching = true;
    try {
      await invoke("switch_to", { email: row.email, provider: row.providerId });
    } catch (err) {
      console.error("switch_to failed", err);
    } finally {
      switching = false;
    }
  }

  async function onReauth() {
    if (reauthing) return;
    reauthing = true;
    try {
      await invoke("login_new", { provider: row.providerId });
    } catch (err) {
      console.error("login_new (reauth) failed", err);
    } finally {
      reauthing = false;
    }
  }

  async function onRemove() {
    if (removing) return;
    removing = true;
    try {
      await invoke("remove_account", { email: row.email, provider: row.providerId });
    } catch (err) {
      console.error("remove_account failed", err);
    } finally {
      removing = false;
      confirmRemove = false;
    }
  }
</script>

<article class="card" class:active={row.isActive} data-provider={row.providerId}>
  <div class="card-top">
    <span class="badge" style={`--provider-accent:${row.providerAccent}`} title={`Provider: ${row.providerLabel}`}>
      {#if row.isActive}<span class="live-dot" aria-hidden="true"></span>{/if}
      {row.providerLabel}
    </span>
    <span class="email" title={row.email}>{row.email}</span>

    {#if row.switchable}
      <button class="btn btn-switch" on:click={onSwitch} disabled={switching} title="Switch to this account">
        {switching ? "Switching…" : "Switch"}
      </button>
    {:else}
      <span class="plan" title={row.planLabel}>{row.planLabel}</span>
    {/if}
  </div>

  <div class="meters">
    {#each row.bars as bar (bar.label)}
      <div class="meter">
        <span class="meter-label">{bar.label}</span>
        <div class="meter-track">
          <div
            class="meter-fill {barClass(bar.utilization)}"
            style={`width:${mounted ? fillWidth(bar) : "0%"}`}
          ></div>
        </div>
        <span class="meter-pct">{pctText(bar.utilization)}</span>
        <span class="meter-reset">{bar.resetText}</span>
      </div>
    {/each}
  </div>

  {#if row.modelsLine}
    <div class="models">{row.modelsLine}</div>
  {/if}
  {#if row.statusLine}
    <div class="status">{row.statusLine}</div>
  {/if}

  {#if row.needsReauth || row.removable}
    <div class="card-actions">
      {#if row.needsReauth}
        <button class="btn btn-reauth" on:click={onReauth} disabled={reauthing} title="Re-authenticate this account">
          {reauthing ? "Opening…" : "↻ Re-authenticate"}
        </button>
      {/if}
      {#if row.removable}
        {#if confirmRemove}
          <button class="btn btn-danger solid" on:click={onRemove} disabled={removing} title="Confirm removal">
            {removing ? "Removing…" : "Confirm"}
          </button>
          <button class="btn" on:click={() => (confirmRemove = false)} disabled={removing}>Cancel</button>
        {:else}
          <button class="btn btn-danger" on:click={() => (confirmRemove = true)} title="Remove this saved account">
            ✕ Remove
          </button>
        {/if}
      {/if}
    </div>
  {/if}
</article>
