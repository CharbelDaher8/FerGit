<script lang="ts">
  import { open } from "@tauri-apps/plugin-dialog";
  import DetailsPanel from "./lib/DetailsPanel.svelte";
  import GraphView from "./lib/GraphView.svelte";
  import { session } from "./lib/session.svelte";

  /** Index of the selected row in the open repository. */
  let selected = $state<number | null>(null);
  let detailsOpen = $state(true);

  const selectedRow = $derived(selected === null ? undefined : session.rows?.get(selected));

  async function openRepository(path: string): Promise<void> {
    await session.open(path);
    selected = null;
    detailsOpen = true;
  }

  async function chooseRepository(): Promise<void> {
    const path = await open({ directory: true, title: "Open repository" });
    if (typeof path === "string") await openRepository(path);
  }

  // Dev-only startup hooks, for testing without the folder picker.
  if (import.meta.env.DEV && import.meta.env.VITE_FERGIT_OPEN) {
    const select = Number(import.meta.env.VITE_FERGIT_SELECT ?? "");
    void openRepository(import.meta.env.VITE_FERGIT_OPEN).then(() => {
      if (import.meta.env.VITE_FERGIT_SELECT && Number.isInteger(select)) selected = select;
    });
  }
</script>

<svelte:window onfocus={() => session.refreshSoon()} />

<div class="app">
  <header class="topbar">
    <button class="button" class:primary={!session.rows} onclick={chooseRepository}>
      Open repository…
    </button>
    {#if session.info && session.rows}
      <div class="repo" title={session.info.root}>
        <span class="repo-name">{session.info.name}</span>
        <span class="repo-root">{session.info.root}</span>
      </div>
      <span class="row-count">{session.rows.total.toLocaleString()} rows</span>
      <button
        class="button"
        onclick={() => session.refresh()}
        disabled={session.refreshing}
        title="Re-read the repository"
      >
        Refresh
      </button>
    {/if}
  </header>

  {#if session.error}
    <div class="banner" role="alert">
      <span class="banner-message">{session.error}</span>
      <button
        class="icon-button"
        aria-label="Dismiss error"
        title="Dismiss"
        onclick={() => session.dismissError()}>×</button
      >
    </div>
  {/if}

  <main class="main">
    {#if session.rows}
      {#key session.rows}
        <GraphView rows={session.rows} bind:selected onactivate={() => (detailsOpen = true)} />
      {/key}
      {#if detailsOpen && selected !== null}
        <DetailsPanel row={selectedRow} onclose={() => (detailsOpen = false)} />
      {/if}
    {:else}
      <div class="empty">
        <h1>No repository open</h1>
        <p>Open a folder that contains a git repository to browse its history.</p>
        <button class="button primary" onclick={chooseRepository}>Open repository…</button>
      </div>
    {/if}
  </main>
</div>

<style>
  .app {
    display: flex;
    flex-direction: column;
    height: 100%;
  }

  .topbar {
    display: flex;
    flex: none;
    align-items: center;
    gap: 10px;
    height: 40px;
    padding: 0 10px;
    border-bottom: 1px solid var(--border);
    background: var(--bg-subtle);
  }

  .repo {
    display: flex;
    flex: 1;
    align-items: baseline;
    gap: 8px;
    min-width: 0;
  }

  .repo-name {
    flex: none;
    font-weight: 600;
  }

  .repo-name,
  .repo-root {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .repo-root {
    color: var(--fg-muted);
    font-size: 12px;
  }

  .row-count {
    flex: none;
    color: var(--fg-subtle);
    font-size: 12px;
    font-variant-numeric: tabular-nums;
  }

  .banner {
    display: flex;
    flex: none;
    align-items: flex-start;
    gap: 8px;
    padding: 6px 6px 6px 12px;
    border-bottom: 1px solid var(--danger-border);
    background: var(--danger-bg);
    color: var(--danger-fg);
  }

  .banner-message {
    flex: 1;
    max-height: 6em;
    overflow: auto;
    padding-top: 3px;
    white-space: pre-wrap;
    overflow-wrap: anywhere;
  }

  .banner .icon-button {
    color: inherit;
  }

  .main {
    display: flex;
    flex: 1;
    min-height: 0;
  }

  .empty {
    display: flex;
    flex: 1;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    gap: 4px;
    padding: 24px;
    color: var(--fg-muted);
    text-align: center;
  }

  .empty h1 {
    margin: 0;
    color: var(--fg);
    font-size: 16px;
    font-weight: 600;
  }

  .empty p {
    margin: 0 0 12px;
  }
</style>
