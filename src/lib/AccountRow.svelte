<script lang="ts">
  import { invoke } from "@tauri-apps/api/core";
  import type { AccountRowDTO } from "./types";
  import { barClass, pctText, fillWidth } from "./usage";

  export let row: AccountRowDTO;

  let switching = false;
  let reauthing = false;
  let removing = false;
  let confirmRemove = false;

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

  // Re-authenticate: launch the provider's login flow. The fresh credentials are
  // captured on the next refresh, clearing the unauthorized state.
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

  // Remove the saved (inactive) account from the app store. Two-step confirm to
  // avoid accidental deletion.
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

<div class="row" class:active={row.isActive} data-provider={row.providerId}>
  <div class="provider-line">
    <span
      class="provider-badge"
      style={`--provider-accent:${row.providerAccent}`}
      title={`Provider: ${row.providerLabel}`}
    >
      {row.providerLabel}
    </span>
  </div>
  <div class="row-head">
    <span class="dot" class:on={row.isActive} aria-hidden="true"></span>
    <span class="email" class:active={row.isActive} title={row.email}>{row.email}</span>

    {#if row.switchable}
      <button
        class="chip switch"
        on:click={onSwitch}
        disabled={switching}
        title="Switch to this account"
      >
        {switching ? "Switching…" : "Switch"}
      </button>
    {:else}
      <span class="chip plan" title={row.planLabel}>{row.planLabel}</span>
    {/if}
  </div>

  <div class="bars">
    {#each row.bars as bar (bar.label)}
      <div class="bar-row">
        <span class="bar-label">{bar.label}</span>
        <div class="bar-track">
          <div class={barClass(bar.utilization)} style={`width:${fillWidth(bar)}`}></div>
        </div>
        <span class="bar-pct">{pctText(bar.utilization)}</span>
        <span class="bar-reset">{bar.resetText}</span>
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
    <div class="actions">
      {#if row.needsReauth}
        <button
          class="chip reauth"
          on:click={onReauth}
          disabled={reauthing}
          title="Re-authenticate this account"
        >
          {reauthing ? "Opening…" : "↻ Re-authenticate"}
        </button>
      {/if}
      {#if row.removable}
        {#if confirmRemove}
          <button
            class="chip remove confirm"
            on:click={onRemove}
            disabled={removing}
            title="Confirm removal"
          >
            {removing ? "Removing…" : "Confirm remove"}
          </button>
          <button class="chip" on:click={() => (confirmRemove = false)} disabled={removing}>
            Cancel
          </button>
        {:else}
          <button
            class="chip remove"
            on:click={() => (confirmRemove = true)}
            title="Remove this saved account"
          >
            ✕ Remove
          </button>
        {/if}
      {/if}
    </div>
  {/if}
</div>
