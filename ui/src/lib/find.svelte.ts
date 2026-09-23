import { commands, type SearchResult } from "./bindings";

/** Quiet period after typing before searching, so a search doesn't run per keystroke. */
export const FIND_DELAY_MS = 200;

/** What the finder moves around in: the view of the open repository. `RepoView` is one. */
export interface FindTarget {
  /** Generation of the snapshot on screen. */
  readonly generation: number;
  /** The selected row, where stepping through matches starts. */
  readonly selected: number | null;
  /** Selects row `index` and, with `reveal`, scrolls it into view. */
  select(index: number, reveal: boolean): void;
}

/**
 * The find bar's state: the query, the rows the backend found for it, and which of them is current.
 *
 * The backend searches the snapshot and answers with row numbers of one generation. Answers to an
 * older query are dropped; answers from a snapshot that is no longer on screen are kept only until
 * `sync` searches again, and meanwhile nothing is highlighted, since their row numbers point at
 * other commits now.
 *
 * Stepping (`step`) starts from the selection, like vim's `n` from the cursor: the next match below
 * it (or above, going back), wrapping around the ends. All reads are reactive.
 */
export class Finder {
  /** Whether the find bar is shown. */
  open = $state(false);
  /** The text being searched for, as typed. */
  query = $state("");
  #result = $state.raw<SearchResult | null>(null);
  /** The rows of `#result`, for highlighting; rebuilt with it. */
  #rows = new Set<number>();
  /** Index into `#result.rows` of the current match. */
  #current = $state<number | null>(null);
  #searching = $state(false);
  /** Identifies the latest search; answers to earlier ones are dropped. */
  #token = 0;
  #timer: ReturnType<typeof setTimeout> | undefined;
  readonly #target: () => FindTarget | null;

  constructor(target: () => FindTarget | null) {
    this.#target = target;
  }

  /** True while a search is on its way. */
  get searching(): boolean {
    return this.#searching;
  }

  /** How many rows the latest search found; only the first `listed` of them can be visited. */
  get total(): number {
    return this.#result?.total ?? 0;
  }

  /** How many of the rows found can be visited. */
  get listed(): number {
    return this.#result?.rows.length ?? 0;
  }

  /** Index (0-based) of the current match among those found; `null` if there is none. */
  get current(): number | null {
    return this.#current;
  }

  /** Whether row `index` of the snapshot on screen is a match. */
  isMatch(index: number): boolean {
    return this.#isCurrentSnapshot() && this.#rows.has(index);
  }

  /** Whether row `index` of the snapshot on screen is the current match. */
  isCurrent(index: number): boolean {
    const result = this.#result;
    const current = this.#current;
    return this.#isCurrentSnapshot() && current !== null && result?.rows[current] === index;
  }

  /** Shows the find bar. */
  show(): void {
    this.open = true;
  }

  /** Hides the find bar and forgets the matches; the query stays for next time. */
  hide(): void {
    this.open = false;
    this.#cancel();
    this.#setResult(null);
  }

  /** Takes a new query and searches once typing pauses, then goes to the first match from the selection. */
  setQuery(query: string): void {
    this.query = query;
    this.#cancel();
    if (query.trim() === "") {
      this.#setResult(null);
      return;
    }
    this.#timer = setTimeout(() => void this.search(true), FIND_DELAY_MS);
  }

  /**
   * Searches for the query now. With `jump`, then selects the first match at or below the
   * selection (wrapping to the top); otherwise only marks it current.
   */
  async search(jump: boolean): Promise<void> {
    this.#cancel();
    const target = this.#target();
    const query = this.query;
    if (!target || query.trim() === "") {
      this.#setResult(null);
      return;
    }
    const token = ++this.#token;
    this.#searching = true;
    let result: SearchResult;
    try {
      result = await commands.search(query);
    } finally {
      if (token === this.#token) this.#searching = false;
    }
    if (token !== this.#token) return;
    this.#setResult(result);
    const from = target.selected ?? 0;
    const at = result.rows.findIndex((row) => row >= from);
    this.#current = result.rows.length === 0 ? null : at < 0 ? 0 : at;
    if (jump) this.#visit();
  }

  /**
   * Goes `by` matches down from the selection (negative: up), wrapping around, and selects it.
   * Without results yet, searches first and goes to the first match.
   */
  async step(by: number): Promise<void> {
    const target = this.#target();
    const result = this.#result;
    if (!target || by === 0) return;
    if (!result || result.generation !== target.generation) {
      await this.search(true);
      return;
    }
    const rows = result.rows;
    if (rows.length === 0) return;
    const selected = target.selected;
    let index: number;
    if (by > 0) {
      // The first match below the selection is one step.
      const below = selected === null ? 0 : rows.findIndex((row) => row > selected);
      index = (below < 0 ? 0 : below) + by - 1;
    } else {
      const above = selected === null ? -1 : findLastIndex(rows, (row) => row < selected);
      index = (above < 0 ? rows.length - 1 : above) + by + 1;
    }
    this.#current = mod(index, rows.length);
    this.#visit();
  }

  /**
   * Searches again if the snapshot on screen changed since the last search (a refresh, a filter),
   * so the matches are rows of what is shown. Call whenever the target's generation changes.
   */
  sync(): void {
    const target = this.#target();
    const result = this.#result;
    if (!target || !this.open || this.#searching || this.query.trim() === "") return;
    if (result && result.generation < target.generation) void this.search(false);
  }

  #visit(): void {
    const target = this.#target();
    const row = this.#current === null ? undefined : this.#result?.rows[this.#current];
    if (target && row !== undefined) target.select(row, true);
  }

  #isCurrentSnapshot(): boolean {
    const result = this.#result;
    return result !== null && result.generation === this.#target()?.generation;
  }

  #setResult(result: SearchResult | null): void {
    this.#result = result;
    this.#rows = new Set(result?.rows);
    if (!result) this.#current = null;
  }

  /** Stops a pending or running search from landing. */
  #cancel(): void {
    clearTimeout(this.#timer);
    this.#token++;
    this.#searching = false;
  }
}

function mod(n: number, m: number): number {
  return ((n % m) + m) % m;
}

function findLastIndex<T>(items: readonly T[], predicate: (item: T) => boolean): number {
  for (let i = items.length - 1; i >= 0; i--) if (predicate(items[i])) return i;
  return -1;
}
