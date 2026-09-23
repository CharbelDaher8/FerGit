import { commands, events, type OpError, type OpOutcome, type Operation, type RepoInfo } from "./bindings";

/** An operation on its way: what it is, in words, and git's latest progress line. */
export interface ActiveOp {
  id: string;
  title: string;
  progress: string;
}

/** An operation that failed, kept on screen until dismissed or the next operation starts. */
export interface FailedOp {
  title: string;
  error: OpError;
}

/**
 * Runs operations on the open repository and keeps what the UI shows about them: the ones on their
 * way (with progress) and the last failure.
 *
 * Each run gets an id generated here. The backend runs an id at most once, and asking for the same
 * operation again while it is still on its way (a double click) reuses the running request rather
 * than starting another, so pushing twice by accident pushes once.
 *
 * The outcome carries the repository as re-read afterwards, which is handed to `adopt` so the graph
 * follows at once instead of waiting for the file watcher.
 */
export class Operations {
  active = $state<ActiveOp[]>([]);
  failure = $state.raw<FailedOp | null>(null);

  #adopt: (info: RepoInfo) => void;
  #inFlight = new Map<string, Promise<OpOutcome>>();

  constructor(adopt: (info: RepoInfo) => void) {
    this.#adopt = adopt;
  }

  /**
   * Runs `op`, described to the user as `title` ("Pushing main"). Resolves with the outcome; rejects
   * only if the backend can't run operations at all (no repository open), which the app-wide error
   * handler reports.
   */
  run(op: Operation, title: string): Promise<OpOutcome> {
    const key = JSON.stringify(op);
    const running = this.#inFlight.get(key);
    if (running) return running;

    const id = crypto.randomUUID();
    this.failure = null;
    this.active.push({ id, title, progress: "" });
    const outcome = commands
      .runOperation(id, op)
      .then((outcome) => {
        this.#adopt(outcome.info);
        if (outcome.kind === "failed") this.failure = { title, error: outcome.error };
        return outcome;
      })
      .finally(() => {
        this.#inFlight.delete(key);
        this.active = this.active.filter((active) => active.id !== id);
      });
    this.#inFlight.set(key, outcome);
    return outcome;
  }

  /** Shows a progress line git reported for operation `id`. */
  progress(id: string, text: string): void {
    const active = this.active.find((active) => active.id === id);
    if (active) active.progress = text;
  }

  dismissFailure(): void {
    this.failure = null;
  }

  /** Follows the backend's progress events until the returned function is called. */
  followProgress(): () => void {
    let unlisten: (() => void) | undefined;
    let stopped = false;
    void events.opProgress
      .listen((event) => this.progress(event.payload.id, event.payload.text))
      .then((stop) => {
        if (stopped) stop();
        else unlisten = stop;
      });
    return () => {
      stopped = true;
      unlisten?.();
    };
  }
}
