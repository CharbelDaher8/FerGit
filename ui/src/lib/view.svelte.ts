import { commands, type Oid, type RepoInfo, type Row, type RowLocation } from "./bindings";
import { ROW_HEIGHT } from "./graph";
import { RowStore } from "./rows.svelte";

/** Rows kept rendered beyond each edge of the viewport, so fast scrolling doesn't show blanks. */
export const OVERSCAN = 6;
/** How long the old rows stay on screen waiting for the new snapshot's rows to load. */
const LOAD_WAIT_MS = 300;
/** Upper bound on showing the old rows, in case re-anchoring never finishes (e.g. a failed command). */
const FREEZE_LIMIT_MS = 1500;

/** Rows to render and fetch for a viewport: the visible ones plus overscan, clamped to `total`. */
export function rowWindow(offset: number, height: number, total: number): { first: number; end: number } {
  const first = Math.max(0, Math.floor(offset / ROW_HEIGHT) - OVERSCAN);
  const end = Math.min(total, Math.ceil((offset + height) / ROW_HEIGHT) + OVERSCAN);
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
}

export function captureAnchor(
  offset: number,
  selected: number | null,
  rowAt: (index: number) => Row | undefined,
): Anchor {
  const topIndex = Math.floor(offset / ROW_HEIGHT);
  return {
    offset,
    topIndex,
    topId: rowAt(topIndex)?.id ?? null,
    selected,
    selectedId: selected === null ? null : (rowAt(selected)?.id ?? null),
  };
}

/**
 * Where the viewport and selection go in a newer snapshot so the same content stays in place.
 * `top` and `selection` are the rows now showing the anchor's top and selected rows: `null` if no
 * row does any more, `undefined` if they weren't looked up (their ids weren't known).
 *
 * - A viewport at the very top stays there, so new commits come into view.
 * - Otherwise the top row keeps its pixel offset; if it is gone or unknown, the offset is kept.
 * - The selection follows its row, and is cleared if that row is gone. If its id wasn't known, the
 *   index is kept.
 */
export function restoreAnchor(
  anchor: Anchor,
  top: number | null | undefined,
  selection: number | null | undefined,
): { offset: number; selected: number | null } {
  let offset = anchor.offset;
  if (anchor.offset <= 0) offset = 0;
  else if (top !== null && top !== undefined) {
    offset = top * ROW_HEIGHT + (anchor.offset - anchor.topIndex * ROW_HEIGHT);
  }
  const selected = anchor.selected === null || selection === undefined ? anchor.selected : selection;
  return { offset, selected };
}

/** Rows shown in place of the store's while the view re-anchors after a change. */
interface Frozen {
  generation: number;
  total: number;
  first: number;
  rows: (Row | undefined)[];
}

/**
 * The open repository as the user sees it: its rows, where the viewport is, and which row is
 * selected. Owns keeping all of that on the same content when the repository changes, and
 * navigating to a commit by id.
 *
 * When the rows move to a newer snapshot, the view:
 * 1. remembers the ids of the top visible row and the selected row, and freezes the rows on screen
 *    (the component keeps rendering the old snapshot);
 * 2. `locate`s both ids in the new snapshot, ignoring locations from any other generation (the
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

  /** Selects row `index` (clamped), or nothing; with `reveal`, scrolls it into view. */
  select(index: number | null, reveal = false): void {
    const total = this.total;
    this.#selected = index === null || total === 0 ? null : Math.max(0, Math.min(total - 1, index));
    if (reveal && this.#selected !== null) this.#reveal(this.#selected);
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
    const anchor = captureAnchor(this.#offset, this.#selected, (index) => this.row(index));
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
    const [top, selection] = await Promise.all([locate(anchor.topId), locate(anchor.selectedId)]);
    if (token !== this.#transition) return;
    const generation = this.#rows.generation;
    if ((top && top.generation !== generation) || (selection && selection.generation !== generation)) {
      return;
    }

    const total = this.#rows.total;
    const restored = restoreAnchor(anchor, top?.row, selection?.row);
    const offset = Math.max(0, Math.min(restored.offset, total * ROW_HEIGHT - this.#height));
    const { first, end } = rowWindow(offset, this.#height, total);
    this.#rows.setViewport(first, end);
    await Promise.race([this.#rows.whenLoaded(first, end), delay(LOAD_WAIT_MS)]);
    if (token !== this.#transition) return;

    this.#frozen = null;
    this.#selected = restored.selected !== null && restored.selected < total ? restored.selected : null;
    this.#scrollTo(offset);
  }
}

function locate(id: Oid | null): Promise<RowLocation | undefined> {
  return id === null ? Promise.resolve(undefined) : commands.locate(id);
}

function delay(ms: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, ms));
}
