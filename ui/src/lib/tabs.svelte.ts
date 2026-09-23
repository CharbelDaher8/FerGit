import type { RepoInfo, SessionId } from "./bindings";
import type { RepoClient } from "./client";
import { Inspector } from "./inspector.svelte";
import { Operations } from "./operations.svelte";
import type { SavedTabs } from "./settings.svelte";
import { RepoView } from "./view.svelte";

/** What `Tabs` needs from the backend. */
export interface TabsBackend {
  /** Opens the repository containing `path`, or finds the session that already has it open. */
  open(path: string): Promise<{ session: SessionId; info: RepoInfo }>;
  /** The commands of session `session`. */
  client(session: SessionId): RepoClient;
}

/** Where the open tabs are remembered across launches. */
export interface TabsMemory {
  tabs: SavedTabs;
}

/**
 * One open repository: its rows, scroll position and selection (the view), what is shown about the
 * selection (the inspector), the operations running on it, and whether the details panel is open.
 * All of it stays as it is while other tabs are in front.
 */
export class Tab {
  readonly session: SessionId;
  readonly client: RepoClient;
  readonly view: RepoView;
  readonly inspector: Inspector;
  readonly operations: Operations;
  #info = $state.raw<RepoInfo>() as RepoInfo;
  /**
   * Counts refresh results and change events taken in, including those that leave the generation
   * unchanged. Views of the index and worktree, which snapshots don't cover, reload when it moves.
   */
  updates = $state(0);
  /** True while a refresh is on its way. */
  refreshing = $state(false);
  detailsOpen = $state(true);

  constructor(session: SessionId, info: RepoInfo, client: RepoClient) {
    this.session = session;
    this.client = client;
    this.#info = info;
    this.view = new RepoView(info, client);
    this.inspector = new Inspector(client);
    this.operations = new Operations(client, (info) => this.adopt(info));
  }

  /** As of the newest open, refresh or change event. */
  get info(): RepoInfo {
    return this.#info;
  }

  /** Re-reads the repository; the view follows if anything visible changed. */
  async refresh(): Promise<void> {
    if (this.refreshing) return;
    this.refreshing = true;
    try {
      this.adopt(await this.client.refresh());
    } finally {
      this.refreshing = false;
    }
  }

  /** Takes in a refresh result or change event; one older than what is shown is ignored. */
  adopt(info: RepoInfo): void {
    if (info.generation < this.#info.generation) return;
    this.#info = info;
    this.updates++;
    this.view.adopt(info);
  }

  /** Stops the inspector's timers and closes the backend session. */
  close(): Promise<void> {
    this.inspector.dispose();
    return this.client.close();
  }
}

/**
 * The open tabs, in order, and which one is in front.
 *
 * A repository is open in at most one tab: opening it again (by any path inside it) brings its tab
 * to the front. The backend guarantees one session per repository, so tabs are told apart by
 * session. Every change to the tabs or to the active tab is remembered, so the next launch can
 * `restore` them. Reads are reactive.
 */
export class Tabs {
  readonly #backend: TabsBackend;
  readonly #memory: TabsMemory;
  #tabs = $state.raw<Tab[]>([]);
  #active = $state.raw<Tab | null>(null);
  #restoring = $state(false);

  constructor(backend: TabsBackend, memory: TabsMemory) {
    this.#backend = backend;
    this.#memory = memory;
  }

  get all(): readonly Tab[] {
    return this.#tabs;
  }

  /** The tab in front; `null` only when there are no tabs. */
  get active(): Tab | null {
    return this.#active;
  }

  /** True while the tabs of the previous launch are reopening. */
  get restoring(): boolean {
    return this.#restoring;
  }

  /**
   * Opens the repository containing `path` in a new tab, or finds the tab that has it, and brings
   * the tab to the front. On failure the tabs stay as they were.
   */
  async open(path: string): Promise<Tab> {
    const tab = this.#add(await this.#backend.open(path));
    this.activate(tab);
    return tab;
  }

  /**
   * Reopens the tabs remembered from the previous launch, in their order, bringing the one that was
   * in front to the front. Repositories that fail to open are dropped from the list (and the first
   * failure is thrown once the others are open). Tabs opened meanwhile stay, ahead of the restored ones.
   */
  async restore(): Promise<void> {
    const saved = this.#memory.tabs;
    if (saved.paths.length === 0) return;
    this.#restoring = true;
    let results: PromiseSettledResult<{ session: SessionId; info: RepoInfo }>[];
    try {
      results = await Promise.allSettled(saved.paths.map((path) => this.#backend.open(path)));
    } finally {
      this.#restoring = false;
    }

    let front: Tab | null = null;
    for (const [index, result] of results.entries()) {
      if (result.status !== "fulfilled") continue;
      const tab = this.#add(result.value);
      if (index === saved.active || front === null) front = tab;
    }
    // A tab the user opened meanwhile stays in front.
    if (this.#active === null) this.#active = front;
    this.#remember();

    const failure = results.find((result) => result.status === "rejected");
    if (failure) throw failure.reason;
  }

  /** Brings `tab` to the front. */
  activate(tab: Tab): void {
    if (!this.#tabs.includes(tab)) return;
    this.#active = tab;
    this.#remember();
  }

  /** Brings the tab `by` places to the right (negative: left) to the front, wrapping around. */
  cycle(by: number): void {
    const count = this.#tabs.length;
    if (count === 0 || !this.#active) return;
    const index = this.#tabs.indexOf(this.#active);
    this.activate(this.#tabs[(((index + by) % count) + count) % count]);
  }

  /**
   * Closes `tab`. If it was in front, the tab to its right comes to the front, or the one to its
   * left if it was the last. Closing a closed tab does nothing.
   */
  close(tab: Tab): Promise<void> {
    const index = this.#tabs.indexOf(tab);
    if (index < 0) return Promise.resolve();
    const rest = this.#tabs.filter((other) => other !== tab);
    this.#tabs = rest;
    if (this.#active === tab) this.#active = rest[Math.min(index, rest.length - 1)] ?? null;
    this.#remember();
    return tab.close();
  }

  /** Routes a change event to the tab of `session`; events for closed sessions are dropped. */
  adopt(session: SessionId, info: RepoInfo): void {
    this.#tabs.find((tab) => tab.session === session)?.adopt(info);
  }

  /** Routes an operation's progress line to the tab of `session`. */
  progress(session: SessionId, id: string, text: string): void {
    this.#tabs.find((tab) => tab.session === session)?.operations.progress(id, text);
  }

  #add(opened: { session: SessionId; info: RepoInfo }): Tab {
    const existing = this.#tabs.find((tab) => tab.session === opened.session);
    if (existing) {
      existing.adopt(opened.info);
      return existing;
    }
    const tab = new Tab(opened.session, opened.info, this.#backend.client(opened.session));
    this.#tabs = [...this.#tabs, tab];
    return tab;
  }

  #remember(): void {
    // Mid-restore, the list isn't complete yet; restoring remembers the whole list once it's done.
    if (this.#restoring) return;
    const active = this.#active ? this.#tabs.indexOf(this.#active) : -1;
    this.#memory.tabs = {
      paths: this.#tabs.map((tab) => tab.info.root),
      active: active < 0 ? null : active,
    };
  }
}
