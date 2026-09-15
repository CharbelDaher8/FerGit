import { commands, type RepoInfo } from "./bindings";
import { RowStore } from "./rows.svelte";

/** Quiet period after the window regains focus before refreshing, so alt-tabbing doesn't pile up. */
const FOCUS_REFRESH_DELAY_MS = 250;

/**
 * App-wide state: the open repository and the error banner.
 *
 * Errors are handled in exactly one place. Commands called from here and from the views are left
 * to reject; `main.ts` routes every unhandled rejection (and uncaught error) to `reportError`,
 * which feeds the banner. Nothing catches errors along the way.
 */
class Session {
  /** The open repository as of the last open or refresh; `null` until one is opened. */
  info = $state.raw<RepoInfo | null>(null);
  /** Rows of the open repository: a fresh store per opened repository. */
  rows = $state.raw<RowStore | null>(null);
  /** True while a refresh is on its way. */
  refreshing = $state(false);
  /** Message of the last error, shown until dismissed. */
  error = $state<string | null>(null);

  #focusTimer: ReturnType<typeof setTimeout> | undefined;

  /** Opens the repository containing `path`. On failure the previous repository stays open. */
  async open(path: string): Promise<void> {
    const info = await commands.openRepo(path);
    this.rows?.close();
    this.info = info;
    this.rows = new RowStore(info);
  }

  /** Re-reads the open repository; the rows follow if anything visible changed. */
  async refresh(): Promise<void> {
    const rows = this.rows;
    if (!rows || this.refreshing) return;
    this.refreshing = true;
    try {
      const info = await commands.refresh();
      if (rows === this.rows) {
        this.info = info;
        rows.adopt(info);
      }
    } finally {
      this.refreshing = false;
    }
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
