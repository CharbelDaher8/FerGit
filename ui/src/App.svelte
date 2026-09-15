<script lang="ts">
  import { open } from "@tauri-apps/plugin-dialog";
  import { untrack } from "svelte";
  import type { FileChange } from "./lib/bindings";
  import DetailsPanel from "./lib/DetailsPanel.svelte";
  import DiffView from "./lib/DiffView.svelte";
  import GraphView from "./lib/GraphView.svelte";
  import { Inspector, subjectOf, type FileList } from "./lib/inspector.svelte";
  import { session } from "./lib/session.svelte";

  let detailsOpen = $state(true);
  const inspector = new Inspector();
  /** Where focus was when a diff opened; it goes back there when the diff closes. */
  let focusBeforeDiff: HTMLElement | null = null;

  // Tell the inspector what is selected, and when the repository may have changed (including
  // refreshes that leave the generation alone, which matter for the index and worktree).
  $effect(() => {
    const view = session.view;
    const subject = view ? subjectOf(view) : ({ kind: "none" } as const);
    const head = session.info?.head ?? null;
    const version = `${view?.generation ?? 0}:${session.updates}`;
    untrack(() => inspector.show(subject, head, version));
  });
  $effect(() => () => inspector.dispose());

  function openFile(list: FileList, file: FileChange): void {
    if (!inspector.diff) {
      focusBeforeDiff = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    }
    inspector.openFile(list, file);
  }

  function closeDiff(): void {
    inspector.closeDiff();
    focusBeforeDiff?.focus();
    focusBeforeDiff = null;
  }

  function onkeydown(event: KeyboardEvent): void {
    if (event.key !== "Escape" || event.defaultPrevented) return;
    const view = session.view;
    if (inspector.diff) {
      event.preventDefault();
      closeDiff();
    } else if (view && view.compared !== null) {
      event.preventDefault();
      view.compare(null);
    }
  }

  async function openRepository(path: string): Promise<void> {
    await session.open(path);
    detailsOpen = true;
  }

  async function chooseRepository(): Promise<void> {
    const path = await open({ directory: true, title: "Open repository" });
    if (typeof path === "string") await openRepository(path);
  }

  // Follow changes the backend detects on disk for as long as the app runs.
  $effect(() => session.followChanges());

  // Dev-only startup hooks, for testing without the folder picker.
  if (import.meta.env.DEV && import.meta.env.VITE_FERGIT_OPEN) {
    const select = Number(import.meta.env.VITE_FERGIT_SELECT ?? "");
    void openRepository(import.meta.env.VITE_FERGIT_OPEN).then(() => {
      if (import.meta.env.VITE_FERGIT_SELECT && Number.isInteger(select)) {
        session.view?.select(select, true);
      }
    });
  }
</script>

<svelte:window onfocus={() => session.refreshSoon()} {onkeydown} />

<div class="app">
  <header class="topbar">
    <button class="button" class:primary={!session.view} onclick={chooseRepository}>
      Open repository…
    </button>
    {#if session.info && session.view}
      <div class="repo" title={session.info.root}>
        <span class="repo-name">{session.info.name}</span>
        <span class="repo-root">{session.info.root}</span>
      </div>
      <span class="row-count">{session.view.total.toLocaleString()} rows</span>
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
    {#if session.view}
      <div class="workspace">
        <!-- The graph stays laid out under an open diff, so closing the diff finds it unchanged. -->
        <div class="graph-layer" inert={inspector.diff !== null}>
          <GraphView view={session.view} onactivate={() => (detailsOpen = true)} />
        </div>
        {#if inspector.diff}
          <div class="diff-layer">
            {#key inspector.diff.request}
              <DiffView diff={inspector.diff} onclose={closeDiff} />
            {/key}
          </div>
        {/if}
      </div>
      {#if detailsOpen && session.view.selected !== null}
        <DetailsPanel
          view={session.view}
          {inspector}
          onopen={openFile}
          onclose={() => (detailsOpen = false)}
        />
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

  /* Not `.primary`: that is also the class of a primary button, and styles here are scoped to the
     component, not to one element, so the layout rule would stretch the buttons. */
  .workspace {
    position: relative;
    display: flex;
    flex: 1;
    min-width: 0;
  }

  .graph-layer {
    display: flex;
    flex: 1;
    min-width: 0;
  }

  .diff-layer {
    position: absolute;
    inset: 0;
    z-index: 3;
    display: flex;
    background: var(--bg);
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
