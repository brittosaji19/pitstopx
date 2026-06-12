<script lang="ts">
  import { invoke } from "@tauri-apps/api/core";
  import type { AccountRowDTO } from "./types";
  import { barClass, pctText, fillWidth } from "./usage";

  export let row: AccountRowDTO;

  let switching = false;

  async function onSwitch() {
    if (!row.switchable || switching) return;
    switching = true;
    try {
      await invoke("switch_to", { email: row.email });
    } catch (err) {
      console.error("switch_to failed", err);
    } finally {
      switching = false;
    }
  }
</script>

<div class="row" class:active={row.isActive}>
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
</div>
