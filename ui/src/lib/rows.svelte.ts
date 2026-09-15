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

interface Waiter {
  first: number;
  end: number;
  resolve: () => void;
}

/**
 * The rows of the open repository, fetched from the backend a page at a time around a viewport.
 * Nothing outside this class knows about pages.
 *
 * The owner reports the rows on screen with `setViewport` and reads rows with `get`, which returns
 * `undefined` for rows that haven't arrived yet. The store fetches the visible pages plus one page
 * ahead in the scroll direction, never requests a page that is already on its way, and evicts
 * pages far from the viewport, so memory stays bounded however long the history is.
 *
 * Every page carries the generation of the snapshot it was read from, and generations only grow,
 * even across repositories. A page from a newer generation means the repository changed: the
 * owner's `onNewGeneration` runs while the old rows can still be read, then the cache is dropped,
 * the new generation and total are adopted, and the visible rows are fetched again. A page from an
 * older generation (possibly of a previously open repository) is discarded and requested again.
 * `adopt` applies the same rule to a refresh result or change event.
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
  readonly #onNewGeneration: () => void;
  readonly #pages = new Map<number, Row[]>();
  readonly #inFlight = new Set<number>();
  readonly #failed = new Set<number>();
  #waiters: Waiter[] = [];
  #first = 0;
  #end = 0;
  #direction: 1 | -1 = 1;
  /** Bumped on every change a reader could see; the getters read it so Svelte tracks them. */
  #version = $state(0);

  /**
   * `onNewGeneration` runs just before the store moves to a newer snapshot of the same repository,
   * while `get` still returns the old rows. It must not call back into the store.
   */
  constructor(info: RepoInfo, onNewGeneration: () => void = () => {}) {
    this.#generation = info.generation;
    this.#total = info.rowCount;
    this.#onNewGeneration = onNewGeneration;
  }

  /** Number of rows in the current snapshot. */
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

  /** Brings the store up to date with a refresh result or change event; older ones are ignored. */
  adopt(info: RepoInfo): void {
    if (info.generation < this.#generation) return;
    if (info.generation > this.#generation) this.#reset(info.generation, info.rowCount, true);
    else this.#failed.clear();
    this.#fetchWanted();
  }

  /** Switches to a newly opened repository. Unlike `adopt`, this doesn't call `onNewGeneration`. */
  replace(info: RepoInfo): void {
    this.#reset(info.generation, info.rowCount, false);
    this.#fetchWanted();
  }

  /**
   * Resolves once rows `first..end` (clamped to the total) are all cached, or when the store moves
   * to another snapshot first, whichever happens earlier. Callers that care which should check the
   * generation afterwards.
   */
  whenLoaded(first: number, end: number): Promise<void> {
    if (this.#isLoaded(first, end)) return Promise.resolve();
    return new Promise((resolve) => this.#waiters.push({ first, end, resolve }));
  }

  #fetchWanted(): void {
    if (this.#end <= this.#first) return;
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
    if (result.generation < this.#generation) return;
    if (result.generation > this.#generation) this.#reset(result.generation, result.total, true);
    if (result.start === page * PAGE_SIZE && result.rows.length > 0) {
      this.#pages.set(page, result.rows);
      this.#evict();
    }
    this.#version++;
    this.#wakeWaiters(false);
  }

  #reset(generation: number, total: number, notify: boolean): void {
    if (notify) this.#onNewGeneration();
    this.#generation = generation;
    this.#total = total;
    this.#pages.clear();
    this.#failed.clear();
    this.#version++;
    this.#wakeWaiters(true);
  }

  #isLoaded(first: number, end: number): boolean {
    const high = Math.min(end, this.#total);
    for (let page = Math.floor(first / PAGE_SIZE); page * PAGE_SIZE < high; page++) {
      if (!this.#pages.has(page)) return false;
    }
    return true;
  }

  #wakeWaiters(all: boolean): void {
    if (this.#waiters.length === 0) return;
    this.#waiters = this.#waiters.filter((waiter) => {
      if (!all && !this.#isLoaded(waiter.first, waiter.end)) return true;
      waiter.resolve();
      return false;
    });
  }

  #evict(): void {
    const low = Math.floor(this.#first / PAGE_SIZE) - KEEP_PAGES;
    const high = Math.floor(Math.max(this.#first, this.#end - 1) / PAGE_SIZE) + KEEP_PAGES;
    for (const page of this.#pages.keys()) {
      if (page < low || page > high) this.#pages.delete(page);
    }
  }
}
