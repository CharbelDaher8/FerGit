import { commands, events } from "./bindings";
import { repoClient } from "./client";
import { settings } from "./settings.svelte";
import { Tabs } from "./tabs.svelte";

/** Quiet period after the window regains focus before refreshing, so alt-tabbing doesn't pile up. */
const FOCUS_REFRESH_DELAY_MS = 250;

/**
 * App-wide state: the open tabs and the error banner.
 *
 * The backend reports changes on disk with `repoChanged`, naming the session that changed;
 * `followChanges` routes each to its tab, which takes it in like a refresh result. Focus refreshes
 * the tab in front, as a cheap fallback.
 *
 * Errors are handled in exactly one place. Commands called from here and from the views are left
 * to reject; `main.ts` routes every unhandled rejection (and uncaught error) to `reportError`,
 * which feeds the banner. Nothing catches errors along the way.
 */
class Session {
  readonly tabs = new Tabs({ open: (path) => commands.openRepo(path), client: repoClient }, settings);
  /** Message of the last error, shown until dismissed. */
  error = $state<string | null>(null);

  #focusTimer: ReturnType<typeof setTimeout> | undefined;

  /** Follows the backend's change events until the returned function is called. */
  followChanges(): () => void {
    let unlisten: (() => void) | undefined;
    let stopped = false;
    void events.repoChanged
      .listen((event) => this.tabs.adopt(event.payload.session, event.payload.info))
      .then((stop) => {
        if (stopped) stop();
        else unlisten = stop;
      });
    return () => {
      stopped = true;
      unlisten?.();
    };
  }

  /** Refreshes the tab in front once things settle; call on every focus event. */
  refreshSoon(): void {
    clearTimeout(this.#focusTimer);
    this.#focusTimer = setTimeout(() => void this.tabs.active?.refresh(), FOCUS_REFRESH_DELAY_MS);
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
