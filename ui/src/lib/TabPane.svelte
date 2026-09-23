<!--
  One tab's repository: the graph, the diff opened over it, the details panel, the find bar, the
  filter picker and the graph's context menu. Every open tab keeps its pane mounted, hidden while another tab is in front, so
  switching back finds the graph scrolled, the files focused and the diff open exactly as they were.
-->
<script lang="ts">
  import { tick, untrack } from "svelte";
  import { perform } from "./actions";
  import type { FileChange, Filter } from "./bindings";
  import ContextMenu from "./ContextMenu.svelte";
  import DetailsPanel from "./DetailsPanel.svelte";
  import DiffView from "./DiffView.svelte";
  import FilterPanel from "./FilterPanel.svelte";
  import FindBar from "./FindBar.svelte";
  import GraphView from "./GraphView.svelte";
  import { subjectOf, type FileList } from "./inspector.svelte";
  import type { Command, KeyContext } from "./keys";
  import { menuFor, type MenuContext, type MenuEntry, type MenuRequest } from "./menu";
  import type { Tab } from "./tabs.svelte";

  interface Props {
    tab: Tab;
    /** Whether this tab is in front. */
    active: boolean;
  }

  let { tab, active }: Props = $props();

  /** Where focus was when a diff opened; it goes back there when the diff closes. */
  let focusBeforeDiff: HTMLElement | null = null;
  let graphView = $state<ReturnType<typeof GraphView>>();
  let detailsPanel = $state<ReturnType<typeof DetailsPanel>>();
  let diffView = $state<ReturnType<typeof DiffView>>();
  let findBar = $state<ReturnType<typeof FindBar>>();
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
    const { info, view } = tab;
    const comparison = view.comparison;
    const [older, newer] = comparison ? [view.row(comparison.older), view.row(comparison.newer)] : [];
    const commits = older?.kind === "commit" && newer?.kind === "commit";
    return {
      branch: info.branch,
      head: info.head,
      busy: info.state.kind !== "clean",
      compared: commits ? { older: older.id, newer: newer.id } : null,
    };
  }

  function closeMenu(): void {
    menu?.returnFocus?.focus({ preventScroll: true });
    menu = null;
  }

  // A tab sent to the back closes its menu, rather than showing it again when it comes back.
  $effect(() => {
    if (!active) untrack(() => (menu = null));
  });

  // Tell the inspector what is selected, and when the repository may have changed (including
  // refreshes that leave the generation alone, which matter for the index and worktree).
  $effect(() => {
    const { view, inspector } = tab;
    const subject = subjectOf(view);
    const head = tab.info.head;
    const version = `${view.generation}:${tab.updates}`;
    untrack(() => inspector.show(subject, head, version));
  });

  // The finder searches again whenever the rows on screen move to another snapshot (a refresh, a
  // filter), so its matches are always rows of what is shown.
  $effect(() => {
    void tab.view.generation;
    untrack(() => tab.finder.sync());
  });

  /** Opens the find bar (closing the diff over the graph) and focuses it. */
  export async function openFind(): Promise<void> {
    if (tab.inspector.diff) closeDiff();
    tab.finder.show();
    await tick();
    findBar?.focus();
  }

  function closeFind(): void {
    tab.finder.hide();
    graphView?.focus();
  }

  /** Applies `filter` to this tab, closing the filter picker. */
  export function applyFilter(filter: Filter): void {
    tab.filterOpen = false;
    void tab.setFilter(filter);
    graphView?.focus();
  }

  function openFile(list: FileList, file: FileChange): void {
    if (!tab.inspector.diff) {
      focusBeforeDiff = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    }
    tab.inspector.openFile(list, file);
  }

  function closeDiff(): void {
    tab.inspector.closeDiff();
    focusBeforeDiff?.focus();
    focusBeforeDiff = null;
  }

  /** Focuses what the keyboard works on: the open diff, or else the graph. */
  export function focus(): void {
    if (tab.inspector.diff) diffView?.focus();
    else graphView?.focus();
  }

  /**
   * Esc, pressed in `context`: closes the find bar being typed in, else the filter picker, else the
   * diff, else leaves compare mode, else closes the find bar.
   */
  export function escape(context: KeyContext): void {
    if (context === "find") closeFind();
    else if (tab.filterOpen) tab.filterOpen = false;
    else if (tab.inspector.diff) closeDiff();
    else if (tab.view.compared !== null) tab.view.compare(null);
    else if (tab.finder.open) closeFind();
  }

  /** Carries out a key command that belongs to a pane, in the pane the key was pressed in. */
  export function run(command: Command, context: KeyContext): void {
    const view = tab.view;
    switch (command.kind) {
      case "close":
        closeDiff();
        return;
      case "open":
        detailsPanel?.openFocused();
        return;
      case "menu":
        graphView?.openMenu();
        return;
      case "find":
        void openFind();
        return;
      case "findNext":
        void tab.finder.step(command.by);
        return;
      case "focus":
        if (command.pane === "files") void focusFileList();
        else focus();
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
        else view.moveSelection(command.by);
        return;
      case "page":
        if (context === "diff") diffView?.scrollPages(command.by);
        else view.pageSelection(command.by);
        return;
      case "goto":
        if (context === "files") detailsPanel?.gotoFile(command.row);
        else if (context === "diff") diffView?.scrollToRow(command.row);
        else view.selectRow(command.row);
        return;
    }
  }

  /** Opens the details panel if needed (selecting the top row if nothing is) and focuses its files. */
  async function focusFileList(): Promise<void> {
    if (tab.view.selected === null) tab.view.moveSelection(1);
    tab.detailsOpen = true;
    await tick();
    detailsPanel?.focusFiles();
  }
</script>

<div class="pane" class:hidden={!active} inert={!active}>
  <div class="workspace">
    <!-- The graph stays laid out under an open diff, so closing the diff finds it unchanged. -->
    <div class="graph-layer" inert={tab.inspector.diff !== null}>
      <GraphView
        bind:this={graphView}
        view={tab.view}
        highlight={tab.finder.open ? tab.finder : undefined}
        onactivate={() => (tab.detailsOpen = true)}
        onmenu={openMenu}
      />
      {#if tab.finder.open}
        <FindBar bind:this={findBar} finder={tab.finder} onclose={closeFind} />
      {/if}
    </div>
    {#if tab.inspector.diff}
      <div class="diff-layer">
        {#key tab.inspector.diff.request}
          <DiffView bind:this={diffView} diff={tab.inspector.diff} onclose={closeDiff} />
        {/key}
      </div>
    {/if}
  </div>
  {#if tab.detailsOpen && tab.view.selected !== null}
    <DetailsPanel
      bind:this={detailsPanel}
      view={tab.view}
      inspector={tab.inspector}
      onopen={openFile}
      onclose={() => (tab.detailsOpen = false)}
    />
  {/if}
  {#if tab.filterOpen}
    <FilterPanel
      current={tab.info.filter}
      loadRefs={() => tab.client.refs()}
      onapply={applyFilter}
      onclose={() => (tab.filterOpen = false)}
    />
  {/if}
  {#if menu}
    <ContextMenu x={menu.x} y={menu.y} entries={menu.entries} onpick={(action) => void perform(tab, action)} onclose={closeMenu} />
  {/if}
</div>

<style>
  /* Panes are stacked; the hidden ones keep their layout (and so their scroll positions). */
  .pane {
    position: absolute;
    inset: 0;
    display: flex;
  }

  .pane.hidden {
    visibility: hidden;
  }

  .workspace {
    position: relative;
    display: flex;
    flex: 1;
    min-width: 0;
  }

  .graph-layer {
    position: relative;
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
</style>
