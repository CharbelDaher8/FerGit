import { commands, events, type RepoInfo } from "./bindings";
import { RepoView } from "./view.svelte";

/** Quiet period after the window regains focus before refreshing, so alt-tabbing doesn't pile up. */
const FOCUS_REFRESH_DELAY_MS = 250;

/**
 * App-wide state: the open repository and the error banner.
 *
 * The backend reports changes on disk with `repoChanged`; `followChanges` routes them, like refresh
 * results, through `adopt`. Focus refreshes remain as a cheap fallback.
 *
 * Errors are handled in exactly one place. Commands called from here and from the views are left
 * to reject; `main.ts` routes every unhandled rejection (and uncaught error) to `reportError`,
 * which feeds the banner. Nothing catches errors along the way.
 */
class Session {
  /** The open repository as of the newest open, refresh or change event; `null` until one is opened. */
  info = $state.raw<RepoInfo | null>(null);
  /** The open repository's rows, scroll position and selection. */
  view = $state.raw<RepoView | null>(null);
  /**
   * Counts refresh results and change events taken in, including those that leave the generation
   * unchanged. Views of the index and worktree, which snapshots don't cover, reload when it moves.
   */
  updates = $state(0);
  /** True while a refresh is on its way. */
  refreshing = $state(false);
  /** Message of the last error, shown until dismissed. */
  error = $state<string | null>(null);

  #focusTimer: ReturnType<typeof setTimeout> | undefined;

  /** Opens the repository containing `path`. On failure the previous repository stays open. */
  async open(path: string): Promise<void> {
    const info = await commands.openRepo(path);
    this.info = info;
    if (this.view) this.view.replace(info);
    else this.view = new RepoView(info);
  }

  /** Re-reads the open repository; the view follows if anything visible changed. */
  async refresh(): Promise<void> {
    if (!this.view || this.refreshing) return;
    this.refreshing = true;
    try {
      this.adopt(await commands.refresh());
    } finally {
      this.refreshing = false;
    }
  }

  /**
   * Takes in a refresh result or change event. Generations grow across repositories, so anything
   * older than what is open (including news of a previously open repository) is ignored.
   */
  adopt(info: RepoInfo): void {
    if (!this.view || !this.info || info.generation < this.info.generation) return;
    this.info = info;
    this.updates++;
    this.view.adopt(info);
  }

  /** Follows the backend's change events until the returned function is called. */
  followChanges(): () => void {
    let unlisten: (() => void) | undefined;
    let stopped = false;
    void events.repoChanged
      .listen((event) => this.adopt(event.payload))
      .then((stop) => {
        if (stopped) stop();
        else unlisten = stop;
      });
    return () => {
      stopped = true;
      unlisten?.();
    };
  }

  /** Refreshes once things settle; call on every focus event. */
  refreshSoon(): void {
    clearTimeout(this.#focusTimer);
    this.#focusTimer = setTimeout(() => void this.refresh(), FOCUS_REFRESH_DELAY_MS);
  }

  reportError(reason: unknown): void {
    this.error = messageOf(reason);
  }

  dismissError(): void {
    this.error = null;
  }
}

/** Commands reject with `AppError { kind, message }`; plugins and bugs reject with strings or `Error`s. */
function messageOf(reason: unknown): string {
  if (typeof reason === "string") return reason;
  if (typeof reason === "object" && reason !== null && "message" in reason) {
    return String(reason.message);
  }
  return "Something went wrong.";
}

export const session = new Session();
