<script lang="ts">
  import { open } from "@tauri-apps/plugin-dialog";
  import { tick, untrack } from "svelte";
  import { followCredentialPrompts, operations, perform } from "./lib/actions";
  import type { FileChange } from "./lib/bindings";
  import ContextMenu from "./lib/ContextMenu.svelte";
  import DetailsPanel from "./lib/DetailsPanel.svelte";
  import Dialog from "./lib/Dialog.svelte";
  import { dialogs } from "./lib/dialogs.svelte";
  import DiffView from "./lib/DiffView.svelte";
  import GraphView from "./lib/GraphView.svelte";
  import { Inspector, subjectOf, type FileList } from "./lib/inspector.svelte";
  import KeyHelp from "./lib/KeyHelp.svelte";
  import { KeyInterpreter, PENDING_G_TIMEOUT_MS, type Command, type KeyContext } from "./lib/keys";
  import { menuFor, type MenuContext, type MenuEntry, type MenuRequest } from "./lib/menu";
  import OperationStatus from "./lib/OperationStatus.svelte";
  import { session } from "./lib/session.svelte";
  import { settings } from "./lib/settings.svelte";
  import StateBanner from "./lib/StateBanner.svelte";

  let detailsOpen = $state(true);
  /** The open context menu; where focus was before it opened goes back there when it closes. */
  let menu = $state.raw<{ x: number; y: number; entries: MenuEntry[]; returnFocus: HTMLElement | null } | null>(null);

  function openMenu({ row, ref, x, y }: MenuRequest): void {
    const entries = menuFor(row, ref, menuContext());
    if (entries.length === 0) return;
    const returnFocus = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    menu = { x, y, entries, returnFocus };
  }

  /** What the menu needs to know about the repository: the current branch, and what is going on. */
  function menuContext(): MenuContext {
    const info = session.info;
    const view = session.view;
    const comparison = view?.comparison;
    const [older, newer] = comparison ? [view!.row(comparison.older), view!.row(comparison.newer)] : [];
    const commits = older?.kind === "commit" && newer?.kind === "commit";
    return {
      branch: info?.branch ?? null,
      head: info?.head ?? null,
      busy: (info?.state.kind ?? "clean") !== "clean",
      compared: commits ? { older: older.id, newer: newer.id } : null,
    };
  }

  function closeMenu(): void {
    menu?.returnFocus?.focus({ preventScroll: true });
    menu = null;
  }

  // Report operations' progress, and ask for the credentials git needs, for as long as the app runs.
  $effect(() => operations.followProgress());
  $effect(() => followCredentialPrompts());
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

  // Keyboard: every shortcut goes through here. The interpreter turns presses into commands; `run`
  // carries each one out in the pane the key was pressed in.
  const keys = new KeyInterpreter();
  /** The half-typed sequence (a count, a `g`), shown like vim's showcmd. */
  let pendingKeys = $state("");
  let helpOpen = $state(false);
  let pendingTimer: ReturnType<typeof setTimeout> | undefined;
  let graphView = $state<ReturnType<typeof GraphView>>();
  let detailsPanel = $state<ReturnType<typeof DetailsPanel>>();
  let diffView = $state<ReturnType<typeof DiffView>>();

  function onkeydown(event: KeyboardEvent): void {
    if (event.defaultPrevented) return;
    const context = keyContext(event.target);
    // The keyboard's menu key, or Shift+F10, opens the selected commit's context menu.
    if (context === "graph" && (event.key === "ContextMenu" || (event.shiftKey && event.key === "F10"))) {
      if (graphView?.openMenu()) event.preventDefault();
      return;
    }
    const result = keys.press(
      {
        key: event.key,
        ctrlKey: event.ctrlKey,
        altKey: event.altKey,
        metaKey: event.metaKey,
        editable: isEditable(event.target),
        context,
      },
      event.timeStamp,
    );
    if (result.handled) event.preventDefault();
    showPendingKeys();
    if (result.command) run(result.command, context);
  }

  function showPendingKeys(): void {
    pendingKeys = keys.pending;
    clearTimeout(pendingTimer);
    if (pendingKeys.endsWith("g")) {
      pendingTimer = setTimeout(() => {
        keys.expire(performance.now());
        pendingKeys = keys.pending;
      }, PENDING_G_TIMEOUT_MS + 50);
    }
  }

  /** Which pane a key press belongs to, from what has focus. */
  function keyContext(target: EventTarget | null): KeyContext {
    if (helpOpen) return "help";
    const element = target instanceof Element ? target : null;
    if (element?.closest(".details")) {
      // Other controls in the panel (parent links, "Show all") keep their own keys, Enter included.
      const control = element.closest("button, a, input, select, textarea");
      return control && !control.matches("button.file") ? "other" : "files";
    }
    if (element?.closest(".topbar, .banner, .empty, .failure")) return "other";
    return inspector.diff ? "diff" : "graph";
  }

  function isEditable(target: EventTarget | null): boolean {
    if (!(target instanceof HTMLElement)) return false;
    if (target.isContentEditable || target instanceof HTMLTextAreaElement || target instanceof HTMLSelectElement) {
      return true;
    }
    const nonText = ["checkbox", "radio", "button", "submit", "reset", "range", "color", "file", "image"];
    return target instanceof HTMLInputElement && !nonText.includes(target.type);
  }

  function run(command: Command, context: KeyContext): void {
    const view = session.view;
    switch (command.kind) {
      case "escape":
        if (helpOpen) helpOpen = false;
        else if (inspector.diff) closeDiff();
        else if (view && view.compared !== null) view.compare(null);
        return;
      case "help":
        helpOpen = !helpOpen;
        return;
      case "undo":
        if (view) void perform({ kind: "undo" });
        return;
      case "close":
        closeDiff();
        return;
      case "open":
        detailsPanel?.openFocused();
        return;
      case "focus":
        if (command.pane === "files") void focusFileList();
        else if (inspector.diff) diffView?.focus();
        else graphView?.focus();
        return;
      case "scrollX":
        diffView?.scrollColumns(command.by);
        return;
      case "lineEdge":
        diffView?.scrollToEdge(command.edge);
        return;
      case "move":
        if (context === "files") detailsPanel?.moveFile(command.by);
        else if (context === "diff") diffView?.scrollLines(command.by);
        else view?.moveSelection(command.by);
        return;
      case "page":
        if (context === "diff") diffView?.scrollPages(command.by);
        else view?.pageSelection(command.by);
        return;
      case "goto":
        if (context === "files") detailsPanel?.gotoFile(command.row);
        else if (context === "diff") diffView?.scrollToRow(command.row);
        else view?.selectRow(command.row);
        return;
    }
  }

  /** Opens the details panel if needed (selecting the top row if nothing is) and focuses its files. */
  async function focusFileList(): Promise<void> {
    const view = session.view;
    if (!view) return;
    if (view.selected === null) view.moveSelection(1);
    detailsOpen = true;
    await tick();
    detailsPanel?.focusFiles();
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
      <label class="toggle" title="Label where branches split and join on the graph (names are inferred)">
        <input type="checkbox" bind:checked={settings.showRelations} />
        Relationships
      </label>
      <button
        class="button"
        onclick={() => void perform({ kind: "fetch", remote: null })}
        title="Fetch every remote, removing remote branches that were deleted there"
      >
        Fetch
      </button>
      <button
        class="button"
        onclick={() => void perform({ kind: "undo" })}
        title="Undo the last operation made here from FerGit, after saying what it restores (u)"
      >
        Undo
      </button>
      <button
        class="button"
        onclick={() => session.refresh()}
        disabled={session.refreshing}
        title="Re-read the repository"
      >
        Refresh
      </button>
    {/if}
    <button
      class="icon-button keys-button"
      aria-label="Keyboard shortcuts"
      title="Keyboard shortcuts (?)"
      onclick={() => (helpOpen = !helpOpen)}>?</button
    >
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

  {#if session.info && session.view}
    <StateBanner
      repoState={session.info.state}
      busy={operations.active.length > 0}
      onaction={(kind) => void perform({ kind })}
    />
  {/if}

  <OperationStatus {operations} />

  <main class="main">
    {#if session.view}
      <div class="workspace">
        <!-- The graph stays laid out under an open diff, so closing the diff finds it unchanged. -->
        <div class="graph-layer" inert={inspector.diff !== null}>
          <GraphView
            bind:this={graphView}
            view={session.view}
            onactivate={() => (detailsOpen = true)}
            onmenu={openMenu}
          />
        </div>
        {#if inspector.diff}
          <div class="diff-layer">
            {#key inspector.diff.request}
              <DiffView bind:this={diffView} diff={inspector.diff} onclose={closeDiff} />
            {/key}
          </div>
        {/if}
      </div>
      {#if detailsOpen && session.view.selected !== null}
        <DetailsPanel
          bind:this={detailsPanel}
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

  {#if pendingKeys}
    <div class="pending-keys" aria-live="polite" title="Keys typed so far">{pendingKeys}</div>
  {/if}
  {#if helpOpen}
    <KeyHelp onclose={() => (helpOpen = false)} />
  {/if}
  {#if menu}
    <ContextMenu x={menu.x} y={menu.y} entries={menu.entries} onpick={(action) => void perform(action)} onclose={closeMenu} />
  {/if}
  {#if dialogs.current}
    {#key dialogs.current}
      <Dialog spec={dialogs.current.spec} onclose={(values) => dialogs.close(values)} />
    {/key}
  {/if}
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

  .toggle {
    display: flex;
    flex: none;
    align-items: center;
    gap: 5px;
    color: var(--fg-muted);
    font-size: 12px;
    white-space: nowrap;
    user-select: none;
  }

  .toggle input {
    margin: 0;
    accent-color: var(--primary-bg);
  }

  .row-count {
    flex: none;
    color: var(--fg-subtle);
    font-size: 12px;
    font-variant-numeric: tabular-nums;
  }

  .keys-button {
    flex: none;
    font-size: 13px;
    font-weight: 600;
  }

  /* Like vim's showcmd: small, in a corner, out of the way of the mouse. */
  .pending-keys {
    position: fixed;
    right: 14px;
    bottom: 10px;
    z-index: 10;
    min-width: 24px;
    padding: 1px 8px;
    border: 1px solid var(--border-strong);
    border-radius: 4px;
    background: var(--bg-subtle);
    color: var(--fg-muted);
    font-family: var(--font-mono);
    font-size: 12px;
    text-align: center;
    pointer-events: none;
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
