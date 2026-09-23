<script lang="ts">
  import { getCurrentWindow } from "@tauri-apps/api/window";
  import { open } from "@tauri-apps/plugin-dialog";
  import { tick, untrack } from "svelte";
  import type { FileChange } from "./lib/bindings";
  import DetailsPanel from "./lib/DetailsPanel.svelte";
  import DiffView from "./lib/DiffView.svelte";
  import GraphView from "./lib/GraphView.svelte";
  import { Inspector, subjectOf, type FileList } from "./lib/inspector.svelte";
  import KeyHelp from "./lib/KeyHelp.svelte";
  import { KeyInterpreter, PENDING_G_TIMEOUT_MS, type Command, type KeyContext } from "./lib/keys";
  import { session } from "./lib/session.svelte";
  import { settings, theme } from "./lib/settings.svelte";
  import { applyTheme, nextThemeMode, type ThemeMode } from "./lib/theme.svelte";

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
    if (element?.closest(".topbar, .banner, .empty")) return "other";
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
      case "theme":
        theme.cycle();
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

  /** How the theme button shows each mode. */
  const THEME_LABELS: Record<ThemeMode, { icon: string; name: string }> = {
    system: { icon: "◐", name: "System" },
    light: { icon: "☀︎", name: "Light" }, // ☀ as text, not an emoji
    dark: { icon: "☾", name: "Dark" },
  };
  const themeLabel = $derived(THEME_LABELS[theme.mode]);
  const nextThemeLabel = $derived(THEME_LABELS[nextThemeMode(theme.mode)]);

  // Theme: follow the OS while the app runs, show the resolved theme on the page, and give the
  // native window (its title bar) the chosen mode, `null` meaning the OS's.
  $effect(() => theme.follow());
  $effect(() => applyTheme(document.documentElement, theme.theme));
  $effect(() => {
    const mode = theme.mode;
    // Cosmetic, so failures (no Tauri window, as with the mock backend) are ignored.
    try {
      getCurrentWindow()
        .setTheme(mode === "system" ? null : mode)
        .catch(() => {});
    } catch {
      // Not running in a Tauri window.
    }
  });

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
        onclick={() => session.refresh()}
        disabled={session.refreshing}
        title="Re-read the repository"
      >
        Refresh
      </button>
    {/if}
    <button
      class="icon-button theme-button"
      aria-label="Theme: {themeLabel.name}"
      title="Theme: {themeLabel.name}. Switch to {nextThemeLabel.name} (T)"
      onclick={() => theme.cycle()}>{themeLabel.icon}</button
    >
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

  <main class="main">
    {#if session.view}
      <div class="workspace">
        <!-- The graph stays laid out under an open diff, so closing the diff finds it unchanged. -->
        <div class="graph-layer" inert={inspector.diff !== null}>
          <GraphView bind:this={graphView} view={session.view} onactivate={() => (detailsOpen = true)} />
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

  .theme-button {
    flex: none;
    margin-left: auto;
    font-size: 14px;
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
