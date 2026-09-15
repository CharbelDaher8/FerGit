import { commands, type RepoInfo, type Row, type RowsPage } from "./bindings";

/** Rows per `rows` request. */
export const PAGE_SIZE = 200;
/** Pages kept on each side of the viewport; pages further away are evicted. */
export const KEEP_PAGES = 4;
/**
 * Row requests allowed in flight at once. Requests beyond this wait, and are re-planned against
 * the viewport of the moment when a slot frees up, so dragging the scrollbar across a huge
 * history doesn't fetch every page it passes over.
 */
export const MAX_IN_FLIGHT = 3;

/**
 * The rows of one open repository, fetched from the backend a page at a time around whatever the
 * view is showing. Nothing outside this class knows about pages.
 *
 * The view reports what it shows with `setViewport` and reads rows with `get`, which returns
 * `undefined` for rows that haven't arrived yet. The store fetches the visible pages plus one page
 * ahead in the scroll direction, never requests a page that is already on its way, and evicts
 * pages far from the viewport, so memory stays bounded however long the history is.
 *
 * Every page carries the generation of the snapshot it was read from. A page from a newer
 * generation means the repository changed: the cache is dropped, the new generation and total are
 * adopted, and the visible rows are fetched again. A page from an older generation arrived after
 * such a change and is discarded and requested again. `adopt` applies the same rule to the result
 * of a refresh. Generations only grow within one open repository; opening another repository
 * means a new store (and `close` on the old one).
 *
 * `total`, `generation` and `get` are reactive: a Svelte template or effect that reads them runs
 * again when rows arrive or the repository changes.
 *
 * A failed request rejects unhandled, so it reaches the app's single error handler. The page that
 * failed isn't retried until the next `adopt`, so a broken backend can't cause a request storm.
 */
export class RowStore {
  #generation: number;
  #total: number;
  readonly #pages = new Map<number, Row[]>();
  readonly #inFlight = new Set<number>();
  readonly #failed = new Set<number>();
  #first = 0;
  #end = 0;
  #direction: 1 | -1 = 1;
  #closed = false;
  /** Bumped on every change a reader could see; the getters read it so Svelte tracks them. */
  #version = $state(0);

  constructor(info: RepoInfo) {
    this.#generation = info.generation;
    this.#total = info.rowCount;
  }

  /** Number of rows in the repository's current snapshot. */
  get total(): number {
    void this.#version;
    return this.#total;
  }

  /** Generation of the snapshot the cached rows belong to. */
  get generation(): number {
    void this.#version;
    return this.#generation;
  }

  /** Row `index`, or `undefined` if it isn't loaded (yet, or any more). */
  get(index: number): Row | undefined {
    void this.#version;
    if (index < 0) return undefined;
    return this.#pages.get(Math.floor(index / PAGE_SIZE))?.[index % PAGE_SIZE];
  }

  /**
   * Declares rows `first..end` as the ones on screen (overscan included) and fetches whatever of
   * them, plus one page ahead in the direction of travel, isn't cached. Cheap to call on every
   * scroll event.
   */
  setViewport(first: number, end: number): void {
    if (first !== this.#first) this.#direction = first > this.#first ? 1 : -1;
    this.#first = first;
    this.#end = end;
    this.#evict();
    this.#fetchWanted();
  }

  /** Brings the store up to date with a `refresh` result. */
  adopt(info: RepoInfo): void {
    if (this.#closed || info.generation < this.#generation) return;
    if (info.generation > this.#generation) this.#reset(info.generation, info.rowCount);
    else this.#failed.clear();
    this.#fetchWanted();
  }

  /** Stops all fetching; responses still on their way are ignored. */
  close(): void {
    this.#closed = true;
  }

  #fetchWanted(): void {
    if (this.#closed || this.#end <= this.#first) return;
    const low = Math.max(0, this.#direction < 0 ? this.#first - PAGE_SIZE : this.#first);
    const high = Math.min(this.#total, this.#direction > 0 ? this.#end + PAGE_SIZE : this.#end);
    if (low >= high) return;
    const firstPage = Math.floor(low / PAGE_SIZE);
    const lastPage = Math.floor((high - 1) / PAGE_SIZE);
    // Walk in the direction of travel, so visible pages come before the prefetched one.
    for (let k = 0; k <= lastPage - firstPage && this.#inFlight.size < MAX_IN_FLIGHT; k++) {
      const page = this.#direction > 0 ? firstPage + k : lastPage - k;
      if (!this.#pages.has(page) && !this.#inFlight.has(page) && !this.#failed.has(page)) {
        this.#request(page);
      }
    }
  }

  #request(page: number): void {
    this.#inFlight.add(page);
    void commands
      .rows(page * PAGE_SIZE, PAGE_SIZE)
      .then(
        (result) => this.#receive(page, result),
        (error: unknown) => {
          this.#failed.add(page);
          throw error;
        },
      )
      .finally(() => {
        this.#inFlight.delete(page);
        this.#fetchWanted();
      });
  }

  #receive(page: number, result: RowsPage): void {
    // An older generation is stale; the page is requested again once it leaves #inFlight.
    if (this.#closed || result.generation < this.#generation) return;
    if (result.generation > this.#generation) this.#reset(result.generation, result.total);
    if (result.start === page * PAGE_SIZE && result.rows.length > 0) {
      this.#pages.set(page, result.rows);
      this.#evict();
    }
    this.#version++;
  }

  #reset(generation: number, total: number): void {
    this.#generation = generation;
    this.#total = total;
    this.#pages.clear();
    this.#failed.clear();
    this.#version++;
  }

  #evict(): void {
    const low = Math.floor(this.#first / PAGE_SIZE) - KEEP_PAGES;
    const high = Math.floor(Math.max(this.#first, this.#end - 1) / PAGE_SIZE) + KEEP_PAGES;
    for (const page of this.#pages.keys()) {
      if (page < low || page > high) this.#pages.delete(page);
    }
  }
}
