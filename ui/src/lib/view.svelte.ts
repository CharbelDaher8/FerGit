import { commands, type Oid, type RepoInfo, type Row, type RowLocation } from "./bindings";
import { ROW_HEIGHT } from "./graph";
import { RowStore } from "./rows.svelte";

/** Rows kept rendered beyond each edge of the viewport, so fast scrolling doesn't show blanks. */
export const OVERSCAN = 6;
/** How long the old rows stay on screen waiting for the new snapshot's rows to load. */
const LOAD_WAIT_MS = 300;
/** Upper bound on showing the old rows, in case re-anchoring never finishes (e.g. a failed command). */
const FREEZE_LIMIT_MS = 1500;

/**
 * Items to render and fetch for a viewport of a virtualized list: the visible ones plus `overscan`
 * on each side, clamped to `0..total`. Defaults fit the commit graph.
 */
export function rowWindow(
  offset: number,
  height: number,
  total: number,
  itemHeight = ROW_HEIGHT,
  overscan = OVERSCAN,
): { first: number; end: number } {
  const first = Math.max(0, Math.floor(offset / itemHeight) - overscan);
  const end = Math.min(total, Math.ceil((offset + height) / itemHeight) + overscan);
  return { first, end: Math.max(first, end) };
}

/** What the user was looking at when the repository changed. */
export interface Anchor {
  /** Content offset (px) of the viewport's top edge. */
  offset: number;
  /** Index of the row at the viewport's top edge. */
  topIndex: number;
  /** Id of that row; `null` if it wasn't loaded. */
  topId: Oid | null;
  selected: number | null;
  /** Id of the selected row; `null` if nothing is selected or the row wasn't loaded. */
  selectedId: Oid | null;
  compared: number | null;
  /** Id of the row compared with the selection; `null` if none or not loaded. */
  comparedId: Oid | null;
}

export function captureAnchor(
  offset: number,
  selected: number | null,
  compared: number | null,
  rowAt: (index: number) => Row | undefined,
): Anchor {
  const topIndex = Math.floor(offset / ROW_HEIGHT);
  return {
    offset,
    topIndex,
    topId: rowAt(topIndex)?.id ?? null,
    selected,
    selectedId: selected === null ? null : (rowAt(selected)?.id ?? null),
    compared,
    comparedId: compared === null ? null : (rowAt(compared)?.id ?? null),
  };
}

/**
 * Where the viewport, selection and compared row go in a newer snapshot so the same content stays
 * in place. `top`, `selection` and `comparison` are the rows now showing the anchor's rows: `null`
 * if no row does any more, `undefined` if they weren't looked up (their ids weren't known).
 *
 * - A viewport at the very top stays there, so new commits come into view.
 * - Otherwise the top row keeps its pixel offset; if it is gone or unknown, the offset is kept.
 * - The selection and the compared row follow their rows and are cleared if their row is gone; an
 *   index whose id wasn't known is kept. Without a selection there is nothing to compare.
 */
export function restoreAnchor(
  anchor: Anchor,
  top: number | null | undefined,
  selection: number | null | undefined,
  comparison: number | null | undefined = undefined,
): { offset: number; selected: number | null; compared: number | null } {
  let offset = anchor.offset;
  if (anchor.offset <= 0) offset = 0;
  else if (top !== null && top !== undefined) {
    offset = top * ROW_HEIGHT + (anchor.offset - anchor.topIndex * ROW_HEIGHT);
  }
  const selected = anchor.selected === null || selection === undefined ? anchor.selected : selection;
  const compared =
    selected === null
      ? null
      : anchor.compared === null || comparison === undefined
        ? anchor.compared
        : comparison;
  return { offset, selected, compared };
}

/** Rows shown in place of the store's while the view re-anchors after a change. */
interface Frozen {
  generation: number;
  total: number;
  first: number;
  rows: (Row | undefined)[];
}

/**
 * The open repository as the user sees it: its rows, where the viewport is, which row is selected
 * and which one (if any) it is compared with. Owns keeping all of that on the same content when
 * the repository changes, and navigating to a commit by id.
 *
 * When the rows move to a newer snapshot, the view:
 * 1. remembers the ids of the top visible, selected and compared rows, and freezes the rows on
 *    screen (the component keeps rendering the old snapshot);
 * 2. `locate`s those ids in the new snapshot, ignoring locations from any other generation (the
 *    repository changed again meanwhile, and that change re-anchors in turn);
 * 3. loads the rows at the restored position, then in one step unfreezes, moves the selection and
 *    asks the component to scroll (`scrollRequest`), so nothing jumps on screen.
 *
 * Offsets are content pixels (row index × ROW_HEIGHT); mapping them to a scroll container is the
 * component's business. All reads are reactive.
 */
export class RepoView {
  readonly #rows: RowStore;
  #selected = $state<number | null>(null);
  #compared = $state<number | null>(null);
  #scrollRequest = $state.raw<{ offset: number } | null>(null);
  #frozen = $state.raw<Frozen | null>(null);
  #offset = 0;
  #height = 0;
  /** A reveal asked for before the viewport size was known. */
  #pendingReveal: number | null = null;
  /** Identifies the latest change being re-anchored; older runs give up when it moves on. */
  #transition = 0;

  constructor(info: RepoInfo) {
    this.#rows = new RowStore(info, () => this.#repositoryChanging());
  }

  /** Number of rows in the snapshot on screen. */
  get total(): number {
    return this.#frozen?.total ?? this.#rows.total;
  }

  /** Generation of the snapshot on screen. */
  get generation(): number {
    return this.#frozen?.generation ?? this.#rows.generation;
  }

  /** Row `index` of the snapshot on screen, or `undefined` if it isn't loaded. */
  row(index: number): Row | undefined {
    const frozen = this.#frozen;
    if (frozen) return frozen.rows[index - frozen.first];
    return this.#rows.get(index);
  }

  /** Index of the selected row. */
  get selected(): number | null {
    return this.#selected;
  }

  /** Index of the row compared with the selection, if any. */
  get compared(): number | null {
    return this.#compared;
  }

  /**
   * The two rows being compared, by age: `older` is the lower row in the graph (the larger index),
   * `newer` the upper one. `null` outside compare mode.
   */
  get comparison(): { older: number; newer: number } | null {
    const selected = this.#selected;
    const compared = this.#compared;
    if (selected === null || compared === null) return null;
    return { older: Math.max(selected, compared), newer: Math.min(selected, compared) };
  }

  /** The latest content offset the component should scroll to; a new object for every request. */
  get scrollRequest(): { offset: number } | null {
    return this.#scrollRequest;
  }

  /** Reports the viewport (content offset and height, in px); call on every scroll and resize. */
  setViewport(offset: number, height: number): void {
    this.#offset = offset;
    this.#height = height;
    if (this.#pendingReveal !== null && height > 0) {
      const index = this.#pendingReveal;
      this.#pendingReveal = null;
      this.#reveal(index);
    }
    if (this.#frozen) return;
    const { first, end } = rowWindow(offset, height, this.#rows.total);
    this.#rows.setViewport(first, end);
  }

  /**
   * Selects row `index` (clamped), or nothing, and leaves compare mode; with `reveal`, scrolls the
   * row into view.
   */
  select(index: number | null, reveal = false): void {
    const total = this.total;
    this.#selected = index === null || total === 0 ? null : Math.max(0, Math.min(total - 1, index));
    this.#compared = null;
    if (reveal && this.#selected !== null) this.#reveal(this.#selected);
  }

  /**
   * Compares the selected row with row `index`, or leaves compare mode with `null`. Both rows must
   * be loaded commits or stashes, and different; otherwise nothing changes and this returns `false`.
   */
  compare(index: number | null): boolean {
    if (index === null) {
      this.#compared = null;
      return true;
    }
    const selected = this.#selected;
    if (selected === null || index === selected) return false;
    if (!isCommitRow(this.row(index)) || !isCommitRow(this.row(selected))) return false;
    this.#compared = index;
    return true;
  }

  /**
   * Moves the selection `rows` down (negative: up) and scrolls it into view, as the arrow keys do.
   * Without a selection, selects the row at the top of the viewport.
   */
  moveSelection(rows: number): void {
    if (this.total === 0) return;
    this.select(this.#selected === null ? this.#topRow() : this.#selected + rows, true);
  }

  /** Moves the selection by `pages` viewports (0.5 for half a page), as Page Down and Up do. */
  pageSelection(pages: number): void {
    if (this.total === 0) return;
    const perPage = Math.max(1, Math.floor(this.#height / ROW_HEIGHT) - 1);
    const rows = Math.sign(pages) * Math.max(1, Math.round(Math.abs(pages) * perPage));
    this.select((this.#selected ?? this.#topRow()) + rows, true);
  }

  /** Selects row `index` (clamped) or the last row, and scrolls it into view. */
  selectRow(index: number | "last"): void {
    if (this.total === 0) return;
    this.select(index === "last" ? this.total - 1 : index, true);
  }

  /** The row at the viewport's top edge, where keyboard movement starts without a selection. */
  #topRow(): number {
    return Math.min(this.total - 1, Math.ceil(this.#offset / ROW_HEIGHT));
  }

  /** Follows a refresh result or change event; the view stays on the same content. */
  adopt(info: RepoInfo): void {
    this.#rows.adopt(info);
  }

  /** Shows a newly opened repository from the top, with nothing selected. */
  replace(info: RepoInfo): void {
    this.#transition++;
    this.#frozen = null;
    this.#pendingReveal = null;
    this.#rows.replace(info);
    this.#selected = null;
    this.#compared = null;
    this.#scrollTo(0);
  }

  /** The row showing `id` in the snapshot on screen; `null` if none does or the snapshot is changing. */
  async find(id: Oid): Promise<number | null> {
    const location = await commands.locate(id);
    return location.generation === this.generation ? location.row : null;
  }

  /** Selects the row showing `id` and scrolls it into view; `false` if no row shows it. */
  async goTo(id: Oid): Promise<boolean> {
    const row = await this.find(id);
    if (row === null) return false;
    this.select(row, true);
    return true;
  }

  #reveal(index: number): void {
    if (this.#height <= 0) {
      this.#pendingReveal = index;
      return;
    }
    const top = index * ROW_HEIGHT;
    if (top < this.#offset) this.#scrollTo(top);
    else if (top + ROW_HEIGHT > this.#offset + this.#height) this.#scrollTo(top + ROW_HEIGHT - this.#height);
  }

  #scrollTo(offset: number): void {
    this.#offset = offset;
    this.#scrollRequest = { offset };
  }

  /** Called by the store just before it drops the rows of the snapshot it is leaving. */
  #repositoryChanging(): void {
    const token = ++this.#transition;
    // Anchor on what is on screen: the frozen rows if a previous change is still settling.
    const anchor = captureAnchor(this.#offset, this.#selected, this.#compared, (index) => this.row(index));
    if (!this.#frozen) {
      const { first, end } = rowWindow(this.#offset, this.#height, this.#rows.total);
      const rows: (Row | undefined)[] = [];
      for (let i = first; i < end; i++) rows.push(this.#rows.get(i));
      const frozen = { generation: this.#rows.generation, total: this.#rows.total, first, rows };
      this.#frozen = frozen;
      setTimeout(() => {
        if (this.#frozen === frozen) this.#frozen = null;
      }, FREEZE_LIMIT_MS);
    }
    void this.#reanchor(anchor, token);
  }

  async #reanchor(anchor: Anchor, token: number): Promise<void> {
    const locations = await Promise.all([
      locate(anchor.topId),
      locate(anchor.selectedId),
      locate(anchor.comparedId),
    ]);
    if (token !== this.#transition) return;
    const generation = this.#rows.generation;
    if (locations.some((location) => location && location.generation !== generation)) return;

    const [top, selection, comparison] = locations;
    const total = this.#rows.total;
    const restored = restoreAnchor(anchor, top?.row, selection?.row, comparison?.row);
    const offset = Math.max(0, Math.min(restored.offset, total * ROW_HEIGHT - this.#height));
    const { first, end } = rowWindow(offset, this.#height, total);
    this.#rows.setViewport(first, end);
    await Promise.race([this.#rows.whenLoaded(first, end), delay(LOAD_WAIT_MS)]);
    if (token !== this.#transition) return;

    const selected = restored.selected !== null && restored.selected < total ? restored.selected : null;
    const compared =
      selected !== null && restored.compared !== null && restored.compared < total ? restored.compared : null;
    this.#frozen = null;
    this.#selected = selected;
    this.#compared = compared;
    this.#scrollTo(offset);
  }
}

function isCommitRow(row: Row | undefined): boolean {
  return row !== undefined && row.kind !== "workingTree";
}

function locate(id: Oid | null): Promise<RowLocation | undefined> {
  return id === null ? Promise.resolve(undefined) : commands.locate(id);
}

function delay(ms: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, ms));
}
