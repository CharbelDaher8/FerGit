<script lang="ts">
  import { getCurrentWindow } from "@tauri-apps/api/window";
  import { open } from "@tauri-apps/plugin-dialog";
  import { tick } from "svelte";
  import KeyHelp from "./lib/KeyHelp.svelte";
  import { KeyInterpreter, PENDING_G_TIMEOUT_MS, type Command, type KeyContext } from "./lib/keys";
  import { session } from "./lib/session.svelte";
  import TabPane from "./lib/TabPane.svelte";
  import type { Tab } from "./lib/tabs.svelte";
  import { settings, theme } from "./lib/settings.svelte";
  import { applyTheme, nextThemeMode, type ThemeMode } from "./lib/theme.svelte";

  const tabs = session.tabs;
  /** Each open tab's pane, by session. */
  const panes: Record<number, ReturnType<typeof TabPane> | undefined> = $state({});
  const activePane = $derived(tabs.active ? panes[tabs.active.session] : undefined);

  // Keyboard: every shortcut goes through here. The interpreter turns presses into commands; `run`
  // carries each one out, in the pane of the tab in front when it belongs to a pane.
  const keys = new KeyInterpreter();
  /** The half-typed sequence (a count, a `g`), shown like vim's showcmd. */
  let pendingKeys = $state("");
  let helpOpen = $state(false);
  let pendingTimer: ReturnType<typeof setTimeout> | undefined;

  function onkeydown(event: KeyboardEvent): void {
    if (event.defaultPrevented) return;
    const context = keyContext(event.target);
    const result = keys.press(
      {
        key: event.key,
        ctrlKey: event.ctrlKey,
        shiftKey: event.shiftKey,
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
    if (element?.closest(".tabstrip, .topbar, .banner, .empty")) return "other";
    return tabs.active?.inspector.diff ? "diff" : "graph";
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
    switch (command.kind) {
      case "escape":
        if (helpOpen) helpOpen = false;
        else activePane?.escape();
        return;
      case "help":
        helpOpen = !helpOpen;
        return;
      case "theme":
        theme.cycle();
        return;
      case "newTab":
        void chooseRepository();
        return;
      case "closeTab":
        if (tabs.active) void closeTab(tabs.active);
        return;
      case "switchTab":
        tabs.cycle(command.by);
        void focusActive();
        return;
      default:
        activePane?.run(command, context);
    }
  }

  /** Moves focus into the tab in front, so its keys work straight away. */
  async function focusActive(): Promise<void> {
    await tick();
    activePane?.focus();
  }

  function showTab(tab: Tab): void {
    tabs.activate(tab);
    void focusActive();
  }

  async function closeTab(tab: Tab): Promise<void> {
    const wasActive = tab === tabs.active;
    const closing = tabs.close(tab);
    if (wasActive) void focusActive();
    await closing;
  }

  async function openRepository(path: string): Promise<Tab> {
    const tab = await tabs.open(path);
    await focusActive();
    return tab;
  }

  async function chooseRepository(): Promise<void> {
    const path = await open({ directory: true, title: "Open repository" });
    if (typeof path === "string") await openRepository(path);
  }

  /** Middle-click closes a tab; stopping the press keeps the browser from starting autoscroll. */
  function onTabMouseDown(event: MouseEvent): void {
    if (event.button === 1) event.preventDefault();
  }

  function onTabAuxClick(event: MouseEvent, tab: Tab): void {
    if (event.button === 1) void closeTab(tab);
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

  // Reopen the tabs of the previous launch.
  void tabs.restore().then(focusActive);

  // Dev-only startup hooks, for testing without the folder picker.
  if (import.meta.env.DEV && import.meta.env.VITE_FERGIT_OPEN) {
    const select = Number(import.meta.env.VITE_FERGIT_SELECT ?? "");
    void openRepository(import.meta.env.VITE_FERGIT_OPEN).then((tab) => {
      if (import.meta.env.VITE_FERGIT_SELECT && Number.isInteger(select)) {
        tab.view.select(select, true);
      }
    });
  }
</script>

<svelte:window onfocus={() => session.refreshSoon()} {onkeydown} />

<div class="app">
  <header class="tabstrip">
    <div class="tabs" role="tablist" aria-label="Open repositories">
      {#each tabs.all as tab (tab.session)}
        <div class="tab" class:active={tab === tabs.active}>
          <button
            class="tab-label"
            role="tab"
            aria-selected={tab === tabs.active}
            title={tab.info.root}
            onclick={() => showTab(tab)}
            onmousedown={onTabMouseDown}
            onauxclick={(event) => onTabAuxClick(event, tab)}
          >
            {tab.info.name}
          </button>
          <button
            class="icon-button tab-close"
            aria-label="Close {tab.info.name}"
            title="Close (Ctrl+W)"
            onclick={() => closeTab(tab)}>×</button
          >
        </div>
      {/each}
    </div>
    <button
      class="icon-button"
      aria-label="Open repository in a new tab"
      title="Open repository (Ctrl+T)"
      onclick={chooseRepository}>+</button
    >
    <span class="spacer"></span>
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

  {#if tabs.active}
    {@const tab = tabs.active}
    <div class="topbar">
      <span class="repo-root" title={tab.info.root}>{tab.info.root}</span>
      <span class="row-count">{tab.view.total.toLocaleString()} rows</span>
      <label class="toggle" title="Label where branches split and join on the graph (names are inferred)">
        <input type="checkbox" bind:checked={settings.showRelations} />
        Relationships
      </label>
      <button class="button" onclick={() => tab.refresh()} disabled={tab.refreshing} title="Re-read the repository">
        Refresh
      </button>
    </div>
  {/if}

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
    {#each tabs.all as tab (tab.session)}
      <TabPane bind:this={panes[tab.session]} {tab} active={tab === tabs.active} />
    {/each}
    {#if tabs.all.length === 0}
      <div class="empty">
        {#if tabs.restoring}
          <p>Opening repositories…</p>
        {:else}
          <h1>No repository open</h1>
          <p>Open a folder that contains a git repository to browse its history.</p>
          <button class="button primary" onclick={chooseRepository}>Open repository…</button>
        {/if}
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

  .tabstrip {
    display: flex;
    flex: none;
    align-items: center;
    gap: 4px;
    height: 36px;
    padding: 0 8px 0 0;
    border-bottom: 1px solid var(--border);
    background: var(--bg-subtle);
  }

  .tabs {
    display: flex;
    align-self: stretch;
    min-width: 0;
    overflow-x: auto;
    scrollbar-width: none;
  }

  .tab {
    display: flex;
    flex: 0 1 auto;
    align-items: center;
    min-width: 72px;
    max-width: 220px;
    padding-right: 4px;
    border-right: 1px solid var(--border);
    color: var(--fg-muted);
  }

  .tab:hover {
    background: var(--bg-hover);
  }

  /* The tab in front joins the content below it. */
  .tab.active {
    margin-bottom: -1px;
    background: var(--bg);
    color: var(--fg);
    box-shadow: inset 0 2px 0 var(--primary-bg);
  }

  .tab-label {
    flex: 1;
    min-width: 0;
    height: 100%;
    padding: 0 6px 0 12px;
    overflow: hidden;
    border: 0;
    background: transparent;
    text-align: left;
    text-overflow: ellipsis;
    white-space: nowrap;
    cursor: default;
  }

  .tab.active .tab-label {
    font-weight: 600;
  }

  .tab-close {
    flex: none;
    width: 20px;
    height: 20px;
    font-size: 14px;
  }

  /* Only the tab in front and the one under the mouse show their close button. */
  .tab:not(.active):not(:hover) .tab-close:not(:focus-visible) {
    visibility: hidden;
  }

  .spacer {
    flex: 1;
  }

  .topbar {
    display: flex;
    flex: none;
    align-items: center;
    gap: 10px;
    height: 36px;
    padding: 0 10px;
    border-bottom: 1px solid var(--border);
  }

  .repo-root {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    color: var(--fg-muted);
    font-size: 12px;
    text-overflow: ellipsis;
    white-space: nowrap;
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

  /* Holds the stacked tab panes. */
  .main {
    position: relative;
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
