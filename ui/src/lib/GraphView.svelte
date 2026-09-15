<!--
  The commit list: a virtualized scroll container with DOM rows for the text columns and one
  canvas, fixed over the viewport, for the graph. Only visible rows (plus overscan) exist in the
  DOM and on the canvas. The RepoView decides which rows those are, what is selected, and where to
  scroll when the repository changes; this component maps it onto the scroll container.
-->
<script lang="ts">
  import { untrack } from "svelte";
  import type { Row } from "./bindings";
  import { formatLocalTime, shortId } from "./format";
  import {
    DARK_PALETTE,
    LIGHT_PALETTE,
    ROW_HEIGHT,
    drawGraph,
    graphWidth,
    lanesUsed,
    paletteColor,
    scrollMap,
    type GraphScene,
  } from "./graph";
  import { rowWindow, type RepoView } from "./view.svelte";

  interface Props {
    view: RepoView;
    /** The user clicked a row or pressed Enter on the selection. */
    onactivate: () => void;
  }

  let { view, onactivate }: Props = $props();

  /** Ref badges shown per row before collapsing the rest into "+N". */
  const MAX_BADGES = 4;
  /** Share of the view's width the graph column may take. */
  const MAX_GRAPH_SHARE = 0.4;
  const MIN_GRAPH_LANES = 3;

  let scroller: HTMLDivElement;
  let canvas: HTMLCanvasElement;

  let scrollTop = $state(0);
  let viewHeight = $state(0);
  let viewWidth = $state(0);
  let scrollbarWidth = $state(0);
  let dark = $state(false);

  const total = $derived(view.total);
  const map = $derived(scrollMap(total, viewHeight));
  // Rows are laid out at `index * ROW_HEIGHT + shift`. `shift` is 0 unless the history is too tall
  // for the browser and the scroll range is compressed; then it moves the rows to wherever the
  // compressed scrollbar says the viewport is. `viewOffset` is the content offset of the viewport.
  const shift = $derived(Math.round(scrollTop - map.toContent(scrollTop)));
  const viewOffset = $derived(scrollTop - shift);
  const span = $derived(rowWindow(viewOffset, viewHeight, total));
  const first = $derived(span.first);
  const end = $derived(span.end);
  const indices = $derived(Array.from({ length: end - first }, (_, k) => first + k));
  const palette = $derived(dark ? DARK_PALETTE : LIGHT_PALETTE);
  const comparison = $derived(view.comparison);

  // The graph column fits the widest visible row. Within one generation it only grows, so
  // scrolling never makes the text columns jump left; a new generation starts over once its rows
  // arrive.
  const laneMemo = { generation: -1, lanes: 1 };
  const lanes = $derived.by(() => {
    const generation = view.generation;
    let widest = 0;
    for (let i = first; i < end; i++) {
      const row = view.row(i);
      if (row) widest = Math.max(widest, lanesUsed(row));
    }
    if (widest === 0) return laneMemo.lanes;
    laneMemo.lanes = generation === laneMemo.generation ? Math.max(laneMemo.lanes, widest) : widest;
    laneMemo.generation = generation;
    return laneMemo.lanes;
  });
  const graphPx = $derived(
    Math.min(
      graphWidth(Math.max(lanes, MIN_GRAPH_LANES)),
      Math.max(graphWidth(MIN_GRAPH_LANES), Math.floor(viewWidth * MAX_GRAPH_SHARE)),
    ),
  );

  $effect(() => {
    view.setViewport(viewOffset, viewHeight);
  });

  // Scroll where the view asks (selection reveal, re-anchoring). The state is updated in the same
  // flush so rows are never painted at the old position with new content.
  $effect(() => {
    const request = view.scrollRequest;
    if (!request) return;
    untrack(() => {
      scroller.scrollTop = map.toScroll(request.offset);
      scrollTop = scroller.scrollTop;
    });
  });

  // Canvas drawing: this effect collects the scene (subscribing to everything it depends on) and
  // the next animation frame paints it. The scene object and its row array are reused.
  const sceneRows: (Row | undefined)[] = [];
  const scene: GraphScene = {
    first: 0,
    rows: sceneRows,
    offset: 0,
    width: 0,
    height: 0,
    palette: LIGHT_PALETTE,
  };
  let frame = 0;

  $effect(() => {
    sceneRows.length = 0;
    for (let i = first; i < end; i++) sceneRows.push(view.row(i));
    scene.first = first;
    scene.offset = viewOffset;
    scene.width = graphPx;
    scene.height = viewHeight;
    scene.palette = palette;
    schedulePaint();
  });

  function schedulePaint(): void {
    if (!frame) frame = requestAnimationFrame(paint);
  }

  function paint(): void {
    frame = 0;
    const ratio = window.devicePixelRatio || 1;
    const width = Math.round(scene.width * ratio);
    const height = Math.round(scene.height * ratio);
    if (canvas.width !== width) canvas.width = width;
    if (canvas.height !== height) canvas.height = height;
    const ctx = canvas.getContext("2d");
    if (!ctx) return;
    ctx.setTransform(ratio, 0, 0, ratio, 0, 0);
    drawGraph(ctx, scene);
  }

  $effect(() => {
    const measure = () => {
      viewHeight = scroller.clientHeight;
      viewWidth = scroller.clientWidth;
      scrollbarWidth = scroller.offsetWidth - scroller.clientWidth;
    };
    const observer = new ResizeObserver(measure);
    observer.observe(scroller);
    measure();

    const scheme = matchMedia("(prefers-color-scheme: dark)");
    const onScheme = () => (dark = scheme.matches);
    onScheme();
    scheme.addEventListener("change", onScheme);

    return () => {
      observer.disconnect();
      scheme.removeEventListener("change", onScheme);
      cancelAnimationFrame(frame);
    };
  });

  function onkeydown(event: KeyboardEvent): void {
    if (total === 0) return;
    const selected = view.selected;
    const pageRows = Math.max(1, Math.floor(viewHeight / ROW_HEIGHT) - 1);
    const topRow = Math.min(total - 1, Math.ceil(viewOffset / ROW_HEIGHT));
    let next: number;
    switch (event.key) {
      case "ArrowDown":
        next = selected === null ? topRow : selected + 1;
        break;
      case "ArrowUp":
        next = selected === null ? topRow : selected - 1;
        break;
      case "PageDown":
        next = (selected ?? topRow) + pageRows;
        break;
      case "PageUp":
        next = (selected ?? topRow) - pageRows;
        break;
      case "Home":
        next = 0;
        break;
      case "End":
        next = total - 1;
        break;
      case "Enter":
        if (selected !== null) onactivate();
        return;
      default:
        return;
    }
    event.preventDefault();
    view.select(next, true);
  }

  function onclick(event: MouseEvent): void {
    const y = event.clientY - scroller.getBoundingClientRect().top + viewOffset;
    const index = Math.floor(y / ROW_HEIGHT);
    if (index < 0 || index >= total) return;
    // Ctrl+click (Cmd+click on macOS) compares with the selection; a plain click selects.
    if (!((event.ctrlKey || event.metaKey) && view.compare(index))) view.select(index);
    onactivate();
  }
</script>

<svelte:window onresize={schedulePaint} />

<div class="graph-view" style:--graph-width="{graphPx}px" style:--scrollbar-width="{scrollbarWidth}px">
  <div class="columns header" aria-hidden="true">
    <span>Graph</span>
    <span>Description</span>
    <span class="author">Author</span>
    <span>Date</span>
    <span>Commit</span>
  </div>

  <div class="body">
    <div
      bind:this={scroller}
      class="scroller"
      role="listbox"
      tabindex="0"
      aria-label="Commits"
      aria-activedescendant={view.selected !== null && view.selected >= first && view.selected < end
        ? `row-${view.selected}`
        : undefined}
      onscroll={() => (scrollTop = scroller.scrollTop)}
      {onkeydown}
      {onclick}
    >
      <div class="spacer" style:height="{map.height}px">
        {#each indices as i (i)}
          {@const row = view.row(i)}
          <div
            id="row-{i}"
            class="columns row"
            class:selected={i === view.selected}
            class:working-tree={row?.kind === "workingTree"}
            class:compared={i === comparison?.older || i === comparison?.newer}
            role="option"
            aria-selected={i === view.selected}
            style:transform="translateY({i * ROW_HEIGHT + shift}px)"
            style:--lane={row ? paletteColor(palette, row.graph.color) : undefined}
          >
            <span></span>
            {#if row}
              <span class="description">
                {#if i === comparison?.older || i === comparison?.newer}
                  <span
                    class="compare-tag"
                    title={i === comparison.older ? "Compared: older side (A)" : "Compared: newer side (B)"}
                    >{i === comparison.older ? "A" : "B"}</span
                  >
                {/if}
                {#each row.refs.slice(0, MAX_BADGES) as ref}
                  <span class="ref ref-{ref.kind}" class:current={ref.isHead} title={ref.fullName}
                    >{ref.name}</span
                  >
                {/each}
                {#if row.refs.length > MAX_BADGES}
                  <span class="ref ref-more" title="{row.refs.length - MAX_BADGES} more refs"
                    >+{row.refs.length - MAX_BADGES}</span
                  >
                {/if}
                <span class="summary" title={row.summary}>{row.summary}</span>
              </span>
              <span class="author" title={row.authorEmail}>{row.authorName}</span>
              {#if row.kind === "workingTree"}
                <span></span>
                <span></span>
              {:else}
                <span class="date">{formatLocalTime(row.time)}</span>
                <span class="id">{shortId(row.id)}</span>
              {/if}
            {:else}
              <span class="description"><span class="placeholder wide"></span></span>
              <span class="author"><span class="placeholder"></span></span>
              <span><span class="placeholder"></span></span>
              <span><span class="placeholder"></span></span>
            {/if}
          </div>
        {/each}
      </div>
    </div>
    <canvas
      bind:this={canvas}
      class="graph"
      style:width="{graphPx}px"
      style:height="{viewHeight}px"
      aria-hidden="true"
    ></canvas>
  </div>
</div>

<style>
  .graph-view {
    --col-author: 150px;
    --col-date: 118px;
    --col-id: 76px;
    display: flex;
    flex: 1;
    flex-direction: column;
    min-width: 0;
    container-type: inline-size;
  }

  @container (max-width: 640px) {
    .columns {
      --col-author: 0px;
    }
    .author {
      visibility: hidden;
    }
  }

  .columns {
    display: grid;
    grid-template-columns:
      var(--graph-width) minmax(0, 1fr) var(--col-author) var(--col-date)
      var(--col-id);
    align-items: center;
  }

  .columns > span {
    min-width: 0;
    overflow: hidden;
    padding-right: 12px;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .header {
    flex: none;
    height: 26px;
    padding-right: var(--scrollbar-width);
    border-bottom: 1px solid var(--border);
    color: var(--fg-muted);
    font-size: 11px;
    font-weight: 600;
    user-select: none;
  }

  .header > span:first-child {
    padding-left: 8px;
  }

  .body {
    position: relative;
    flex: 1;
    min-height: 0;
  }

  .scroller {
    position: absolute;
    inset: 0;
    overflow-x: hidden;
    overflow-y: auto;
    outline: none;
    user-select: none;
  }

  .scroller:focus-visible {
    box-shadow: inset 0 0 0 2px var(--focus);
  }

  .spacer {
    position: relative;
    min-height: 100%;
  }

  .graph {
    position: absolute;
    top: 0;
    left: 0;
    pointer-events: none;
  }

  .row {
    position: absolute;
    top: 0;
    right: 0;
    left: 0;
    height: 28px;
    contain: strict;
  }

  .row:hover {
    background: var(--bg-hover);
  }

  .row.selected {
    background: var(--bg-selected-blur);
  }

  .scroller:focus .row.selected {
    background: var(--bg-selected);
  }

  .scroller .row.compared {
    background: var(--compare-bg);
    box-shadow: inset 3px 0 0 var(--compare);
  }

  .compare-tag {
    flex: none;
    min-width: 16px;
    padding: 0 4px;
    border-radius: 3px;
    background: var(--compare);
    color: var(--on-lane);
    font-size: 10px;
    font-weight: 700;
    line-height: 16px;
    text-align: center;
  }

  .description {
    display: flex;
    align-items: center;
    gap: 4px;
  }

  .summary {
    flex: 1 1 auto;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .working-tree .summary {
    color: var(--fg-muted);
    font-style: italic;
  }

  .author,
  .date {
    color: var(--fg-muted);
  }

  .date {
    font-variant-numeric: tabular-nums;
  }

  .id {
    color: var(--fg-muted);
    font-family: var(--font-mono);
    font-size: 12px;
  }

  .ref {
    flex: 0 1 auto;
    min-width: 28px;
    max-width: 200px;
    overflow: hidden;
    padding: 0 5px;
    border: 1px solid var(--lane);
    border-radius: 4px;
    background: color-mix(in srgb, var(--lane) 14%, transparent);
    font-size: 11px;
    line-height: 16px;
    text-overflow: ellipsis;
  }

  .ref-localBranch.current {
    background: var(--lane);
    color: var(--on-lane);
    font-weight: 600;
  }

  .ref-remoteBranch {
    border-color: color-mix(in srgb, var(--lane) 50%, transparent);
    background: transparent;
    color: var(--fg-muted);
  }

  .ref-tag {
    border-color: var(--ref-tag);
    background: color-mix(in srgb, var(--ref-tag) 14%, transparent);
    border-radius: 8px;
  }

  .ref-stash {
    border-color: var(--ref-stash);
    background: color-mix(in srgb, var(--ref-stash) 14%, transparent);
    border-style: dashed;
  }

  .ref-head {
    border-color: var(--ref-head);
    background: var(--ref-head);
    color: var(--on-lane);
    font-weight: 600;
  }

  .ref-more {
    flex: none;
    border-color: var(--border-strong);
    background: transparent;
    color: var(--fg-muted);
  }

  .placeholder {
    display: block;
    width: 70%;
    height: 8px;
    border-radius: 4px;
    background: var(--placeholder);
  }

  .placeholder.wide {
    width: 45%;
  }
</style>
