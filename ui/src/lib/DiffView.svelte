<!--
  One file's diff: a header with the path, the sides compared and a way back to the graph, then the
  changes. Text diffs are virtualized (only visible lines exist in the DOM), so diffs with hundreds of
  thousands of lines stay cheap. Everything is rendered as text.
-->
<script lang="ts">
  import { DiffModel, NO_NEWLINE_TEXT } from "./diff";
  import { shortId } from "./format";
  import { scrollMap } from "./graph";
  import { describeSide, type OpenDiff } from "./inspector.svelte";
  import { rowWindow } from "./view.svelte";

  interface Props {
    diff: OpenDiff;
    /** Back to the graph. */
    onclose: () => void;
  }

  let { diff, onclose }: Props = $props();

  const LINE_HEIGHT = 20;
  const OVERSCAN = 30;
  const SIGN = { context: " ", added: "+", removed: "-" } as const;

  const request = $derived(diff.request);
  const result = $derived(diff.result);
  const model = $derived(
    result?.kind === "text" && result.hunks.length > 0 ? new DiffModel(result.hunks) : null,
  );
  const digits = $derived(String(model?.maxLineNumber ?? 0).length);

  let scroller = $state<HTMLDivElement>();
  let scrollTop = $state(0);
  let viewHeight = $state(0);

  const count = $derived(model?.rowCount ?? 0);
  const map = $derived(scrollMap(count, viewHeight, LINE_HEIGHT));
  // As in the graph: `shift` is non-zero only when the content is too tall for the browser and the
  // scroll range is compressed.
  const shift = $derived(Math.round(scrollTop - map.toContent(scrollTop)));
  const span = $derived(rowWindow(scrollTop - shift, viewHeight, count, LINE_HEIGHT, OVERSCAN));
  const indices = $derived(Array.from({ length: span.end - span.first }, (_, k) => span.first + k));

  $effect(() => {
    const element = scroller;
    if (!element) return;
    const observer = new ResizeObserver(() => (viewHeight = element.clientHeight));
    observer.observe(element);
    viewHeight = element.clientHeight;
    // Keyboard scrolling works right away; Esc is handled by the app.
    element.focus({ preventScroll: true });
    return () => observer.disconnect();
  });

  // Scrolling for the app's key router. Offsets go through the scroll map, so they stay right when
  // a huge diff's scroll range is compressed.
  let section: HTMLElement;

  /** Approximate width of one monospace column, for sideways scrolling. */
  function columnWidth(element: HTMLElement): number {
    return parseFloat(getComputedStyle(element).fontSize) * 0.6;
  }

  /** Gives the diff keyboard focus. */
  export function focus(): void {
    (scroller ?? section).focus({ preventScroll: true });
  }

  /** Scrolls `lines` lines down (negative: up). */
  export function scrollLines(lines: number): void {
    if (!scroller) return;
    const offset = map.toContent(scroller.scrollTop) + lines * LINE_HEIGHT;
    scroller.scrollTop = map.toScroll(Math.max(0, offset));
  }

  /** Scrolls by `pages` viewports (0.5 for half a page), keeping a line of overlap per page. */
  export function scrollPages(pages: number): void {
    const perPage = Math.max(1, Math.floor(viewHeight / LINE_HEIGHT) - 1);
    scrollLines(Math.sign(pages) * Math.max(1, Math.round(Math.abs(pages) * perPage)));
  }

  /** Scrolls row `row` (0-based, clamped) to the top, or scrolls to the end. */
  export function scrollToRow(row: number | "last"): void {
    if (!scroller) return;
    scroller.scrollTop =
      row === "last" ? scroller.scrollHeight : map.toScroll(Math.max(0, Math.min(row, count - 1)) * LINE_HEIGHT);
  }

  /** Scrolls `columns` columns right (negative: left). */
  export function scrollColumns(columns: number): void {
    if (!scroller) return;
    scroller.scrollLeft += columns * columnWidth(scroller);
  }

  /** Scrolls to the start of the lines, or to the end of the longest one. */
  export function scrollToEdge(edge: "start" | "end"): void {
    if (!scroller) return;
    scroller.scrollLeft = edge === "start" ? 0 : scroller.scrollWidth;
  }
</script>

<section bind:this={section} class="diff" aria-label="Diff" tabindex="-1">
  <header class="diff-header">
    <button class="button" onclick={onclose} title="Back to the graph (Esc)">← Graph</button>
    <span
      class="path"
      title={request.oldPath !== null && request.oldPath !== request.path
        ? `${request.oldPath} → ${request.path}`
        : request.path}
    >
      {#if request.oldPath !== null && request.oldPath !== request.path}
        <span class="old-path">{request.oldPath}</span> <span class="arrow">→</span>
      {/if}
      {request.path}
    </span>
    <span class="sides">{describeSide(request.from)} → {describeSide(request.to)}</span>
  </header>

  {#if result === undefined}
    <p class="state">Loading diff…</p>
  {:else if result.kind === "binary"}
    <p class="state">Binary file: its contents aren't shown.</p>
  {:else if result.kind === "tooLarge"}
    <p class="state">This file is larger than 8 MiB on at least one side, so its diff isn't shown.</p>
  {:else if result.kind === "submodule"}
    <p class="state">
      Submodule:
      <span class="mono">{result.old === null ? "none" : shortId(result.old)}</span>
      →
      <span class="mono">{result.new === null ? "none" : shortId(result.new)}</span>
    </p>
  {:else if model === null}
    <p class="state">
      {#if request.oldPath !== null && request.oldPath !== request.path}
        The file was renamed or copied without changing its contents.
      {:else}
        The contents are identical; only metadata such as the file mode changed.
      {/if}
    </p>
  {:else}
    <!-- A scrollable region must be focusable for keyboard users. -->
    <!-- svelte-ignore a11y_no_noninteractive_tabindex -->
    <div
      bind:this={scroller}
      class="scroller"
      role="region"
      aria-label="Changes"
      tabindex="0"
      style:--digits={digits}
      onscroll={(event) => (scrollTop = event.currentTarget.scrollTop)}
    >
      <div
        class="spacer"
        style:height="{map.height}px"
        style:width="calc(var(--gutters) + {model.maxColumns}ch + 24px)"
      >
        {#each indices as i (i)}
          {@const row = model.row(i)}
          <div
            class="row {row.kind === 'line' ? row.line : row.kind}"
            style:transform="translateY({i * LINE_HEIGHT + shift}px)"
          >
            {#if row.kind === "line"}
              <span class="gutter">
                <span class="number">{row.oldNumber ?? ""}</span>
                <span class="number">{row.newNumber ?? ""}</span>
                <span class="sign">{SIGN[row.line]}</span>
              </span>
              <span class="text">{row.text}</span>
            {:else}
              <span class="gutter"></span>
              <span class="text">{row.kind === "hunk" ? row.header : NO_NEWLINE_TEXT}</span>
            {/if}
          </div>
        {/each}
      </div>
    </div>
  {/if}
</section>

<style>
  .diff:focus {
    outline: none;
  }

  .diff {
    display: flex;
    flex: 1;
    flex-direction: column;
    min-width: 0;
  }

  .diff-header {
    display: flex;
    flex: none;
    align-items: center;
    gap: 10px;
    height: 36px;
    padding: 0 10px;
    border-bottom: 1px solid var(--border);
    background: var(--bg-subtle);
  }

  .path,
  .sides,
  .mono {
    font-family: var(--font-mono);
    font-size: 12px;
  }

  .path {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .old-path,
  .arrow,
  .sides {
    color: var(--fg-muted);
  }

  .sides {
    flex: none;
  }

  .state {
    margin: 24px;
    color: var(--fg-muted);
  }

  .scroller {
    --number-width: calc(var(--digits) * 1ch + 16px);
    --gutters: calc(var(--number-width) * 2 + 2ch);
    position: relative;
    flex: 1;
    min-height: 0;
    overflow: auto;
    outline: none;
    font-family: var(--font-mono);
    font-size: 12px;
    tab-size: 4;
  }

  .scroller:focus-visible {
    box-shadow: inset 0 0 0 2px var(--focus);
  }

  .spacer {
    position: relative;
    min-width: 100%;
  }

  .row {
    position: absolute;
    top: 0;
    right: 0;
    left: 0;
    display: flex;
    height: 20px;
    line-height: 20px;
    white-space: pre;
  }

  .gutter {
    position: sticky;
    left: 0;
    z-index: 1;
    display: flex;
    flex: none;
    width: var(--gutters);
    background: var(--bg);
    color: var(--fg-subtle);
    user-select: none;
  }

  .number {
    width: var(--number-width);
    padding-right: 8px;
    text-align: right;
  }

  .sign {
    width: 2ch;
    text-align: center;
  }

  .text {
    flex: none;
    padding-right: 24px;
  }

  /* Gutters stay opaque (a tint over the page background) so text scrolling under them is hidden. */
  .added {
    background: var(--diff-added-bg);
  }
  .added .gutter {
    background: linear-gradient(var(--diff-added-gutter), var(--diff-added-gutter)), var(--bg);
  }

  .removed {
    background: var(--diff-removed-bg);
  }
  .removed .gutter {
    background: linear-gradient(var(--diff-removed-gutter), var(--diff-removed-gutter)), var(--bg);
  }

  .hunk {
    background: var(--diff-hunk-bg);
    color: var(--diff-hunk-fg);
  }
  .hunk .gutter {
    background: linear-gradient(var(--diff-hunk-bg), var(--diff-hunk-bg)), var(--bg);
  }

  .noNewline .text {
    color: var(--fg-muted);
    font-style: italic;
  }
</style>
