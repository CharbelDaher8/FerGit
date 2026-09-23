<!--
  One tab's repository: the graph, the diff opened over it, and the details panel. Every open tab
  keeps its pane mounted, hidden while another tab is in front, so switching back finds the graph
  scrolled, the files focused and the diff open exactly as they were.
-->
<script lang="ts">
  import { tick, untrack } from "svelte";
  import type { FileChange } from "./bindings";
  import DetailsPanel from "./DetailsPanel.svelte";
  import DiffView from "./DiffView.svelte";
  import GraphView from "./GraphView.svelte";
  import { subjectOf, type FileList } from "./inspector.svelte";
  import type { Command, KeyContext } from "./keys";
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

  // Tell the inspector what is selected, and when the repository may have changed (including
  // refreshes that leave the generation alone, which matter for the index and worktree).
  $effect(() => {
    const { view, inspector } = tab;
    const subject = subjectOf(view);
    const head = tab.info.head;
    const version = `${view.generation}:${tab.updates}`;
    untrack(() => inspector.show(subject, head, version));
  });

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

  /** Esc: closes the diff, or else leaves compare mode. */
  export function escape(): void {
    if (tab.inspector.diff) closeDiff();
    else if (tab.view.compared !== null) tab.view.compare(null);
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
      <GraphView bind:this={graphView} view={tab.view} onactivate={() => (tab.detailsOpen = true)} />
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
