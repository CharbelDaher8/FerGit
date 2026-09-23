// Vim-style key sequences, as a pure interpreter: key presses and a clock in, commands out. The app
// decides which pane a press belongs to (its context) and carries the commands out there.

/**
 * Where a key press lands. `find` is the find bar's text field; `other` is any control outside the
 * panes (buttons, the toolbar).
 */
export type KeyContext = "graph" | "files" | "diff" | "find" | "help" | "other";

/** What the interpreter needs from a keyboard event. */
export interface KeyPress {
  key: string;
  ctrlKey: boolean;
  altKey: boolean;
  metaKey: boolean;
  /** Only read for Enter in the find bar. */
  shiftKey?: boolean;
  /** The press targets a text field or another editable element. */
  editable: boolean;
  context: KeyContext;
}

export type Command =
  /** Move the selection (graph), focused file (files) or view (diff) by `by` rows. */
  | { kind: "move"; by: number }
  /** Go to row `row` (0-based, clamped by whoever carries it out) or to the last row. */
  | { kind: "goto"; row: number | "last" }
  /** Move by `by` pages; half pages are 0.5. */
  | { kind: "page"; by: number }
  /** Scroll the diff sideways by `by` columns. */
  | { kind: "scrollX"; by: number }
  /** Scroll the diff to the start or the end of its longest line. */
  | { kind: "lineEdge"; edge: "start" | "end" }
  | { kind: "focus"; pane: "graph" | "files" }
  /** Open the focused file's diff. */
  | { kind: "open" }
  /** Close the diff. */
  | { kind: "close" }
  /** Open the find bar and focus its text field. */
  | { kind: "find" }
  /** Go `by` matches down (negative: up) from the selection, wrapping around. */
  | { kind: "findNext"; by: number }
  /** Show or hide the keyboard help. */
  | { kind: "help" }
  /** Esc with nothing pending: close what is open, innermost first. */
  | { kind: "escape" };

export interface KeyResult {
  command: Command | null;
  /** The press was consumed: its default action should be prevented. */
  handled: boolean;
}

/** How long a `g` waits for the second `g`. */
export const PENDING_G_TIMEOUT_MS = 1000;
/** Columns `h` and `l` scroll a diff sideways. */
export const SIDEWAYS_COLUMNS = 4;

const MAX_COUNT_DIGITS = 7;
const IGNORED: KeyResult = { command: null, handled: false };
const CONSUMED: KeyResult = { command: null, handled: true };
/** Pages moved by Ctrl+d/u/b. Ctrl+f finds, as in other desktop apps, rather than paging as in vim. */
const CTRL_PAGES: Record<string, number> = { d: 0.5, u: -0.5, b: -1 };
const MODIFIER_KEYS = new Set(["Shift", "Control", "Alt", "AltGraph", "Meta", "CapsLock", "OS"]);

const run = (command: Command): KeyResult => ({ command, handled: true });

/**
 * Turns key presses into commands, the way vim reads normal-mode keys:
 * - a count (`5j`, `20G`) prefixes a command; `0` extends a count but is its own command (line
 *   start, in a diff) when no count is pending;
 * - `g` waits `PENDING_G_TIMEOUT_MS` for a second `g`;
 * - Esc cancels a pending count or `g`, and otherwise becomes `escape`;
 * - presses with Alt or Meta, into editable elements, or of lone modifier keys are ignored, and Ctrl
 *   only means something with d, u, f and b.
 *
 * Keymap per context:
 * - graph: j/k and ↓/↑ move, gg/G/Home/End go to an end, {n}G/{n}gg to row n, Ctrl+d/u/f/b and
 *   PageDown/PageUp page, l/Enter focus the file list, h does nothing (leftmost pane);
 * - files: j/k and ↓/↑ move, gg/G/Home/End go to an end, l/Enter open, h focuses the graph;
 * - diff: j/k scroll lines, Ctrl+d/u/f/b page, gg/G go to an end, h/l scroll sideways, 0/$ go to
 *   a line edge, q closes; arrows and other keys keep their native scrolling;
 * - graph and files: also `/` finds, n/N go to the next/previous match (with a count);
 * - find (typing in the find bar): Enter/Shift+Enter go to the next/previous match, and Esc; every
 *   other key types;
 * - help and other: only `?` and Esc.
 * `?` toggles the help everywhere but the find bar. Ctrl+f finds from everywhere but the help,
 * the diff included.
 */
export class KeyInterpreter {
  #count = "";
  #gPressedAt: number | null = null;

  /** The half-typed sequence, for display: the count and a trailing `g`, or "". */
  get pending(): string {
    return this.#count + (this.#gPressedAt === null ? "" : "g");
  }

  /** Drops a `g` (and its count) that has waited too long. Returns whether anything was dropped. */
  expire(now: number): boolean {
    if (this.#gPressedAt === null || now - this.#gPressedAt <= PENDING_G_TIMEOUT_MS) return false;
    this.#clear();
    return true;
  }

  press(input: KeyPress, now: number): KeyResult {
    const { key, context } = input;
    if (input.altKey || input.metaKey || MODIFIER_KEYS.has(key)) return IGNORED;
    if (context === "find") return this.#findBar(input);
    if (input.editable) return IGNORED;
    this.expire(now);

    if (input.ctrlKey && key.toLowerCase() === "f" && context !== "help") {
      this.#clear();
      return run({ kind: "find" });
    }

    if (key === "Escape") {
      if (this.pending === "") return run({ kind: "escape" });
      this.#clear();
      return CONSUMED;
    }

    if (context === "help" || context === "other") {
      if (key !== "?" || input.ctrlKey) return IGNORED;
      this.#clear();
      return run({ kind: "help" });
    }

    if (input.ctrlKey) {
      const pages = CTRL_PAGES[key.toLowerCase()];
      if (pages === undefined || context === "files") return IGNORED;
      return run({ kind: "page", by: pages * this.#takeCount(1) });
    }

    if (key.length === 1 && key >= "0" && key <= "9" && (key !== "0" || this.#count !== "")) {
      this.#gPressedAt = null;
      if (this.#count.length < MAX_COUNT_DIGITS) this.#count += key;
      return CONSUMED;
    }

    if (key === "g") {
      if (this.#gPressedAt === null) {
        this.#gPressedAt = now;
        return CONSUMED;
      }
      const count = this.#takeCount(null);
      return run({ kind: "goto", row: count === null ? 0 : count - 1 });
    }

    // Any other key ends a half-typed `g` sequence, and its count with it.
    if (this.#gPressedAt !== null) this.#clear();
    return this.#command(key, context);
  }

  #command(key: string, context: "graph" | "files" | "diff"): KeyResult {
    switch (key) {
      case "?":
        this.#clear();
        return run({ kind: "help" });
      case "G": {
        const count = this.#takeCount(null);
        return run({ kind: "goto", row: count === null ? "last" : count - 1 });
      }
      case "j":
        return run({ kind: "move", by: this.#takeCount(1) });
      case "k":
        return run({ kind: "move", by: -this.#takeCount(1) });
    }

    if (context !== "diff") {
      switch (key) {
        case "/":
          this.#clear();
          return run({ kind: "find" });
        case "n":
          return run({ kind: "findNext", by: this.#takeCount(1) });
        case "N":
          return run({ kind: "findNext", by: -this.#takeCount(1) });
      }
    }

    if (context === "diff") {
      switch (key) {
        case "h":
          return run({ kind: "scrollX", by: -SIDEWAYS_COLUMNS * this.#takeCount(1) });
        case "l":
          return run({ kind: "scrollX", by: SIDEWAYS_COLUMNS * this.#takeCount(1) });
        case "0":
          this.#clear();
          return run({ kind: "lineEdge", edge: "start" });
        case "$":
          this.#clear();
          return run({ kind: "lineEdge", edge: "end" });
        case "q":
          this.#clear();
          return run({ kind: "close" });
      }
    } else {
      switch (key) {
        case "ArrowDown":
          return run({ kind: "move", by: this.#takeCount(1) });
        case "ArrowUp":
          return run({ kind: "move", by: -this.#takeCount(1) });
        case "Home":
          this.#clear();
          return run({ kind: "goto", row: 0 });
        case "End":
          this.#clear();
          return run({ kind: "goto", row: "last" });
        case "l":
        case "Enter":
          this.#clear();
          return run(context === "graph" ? { kind: "focus", pane: "files" } : { kind: "open" });
        case "h":
          this.#clear();
          return context === "files" ? run({ kind: "focus", pane: "graph" }) : CONSUMED;
      }
      if (context === "graph" && (key === "PageDown" || key === "PageUp")) {
        return run({ kind: "page", by: (key === "PageDown" ? 1 : -1) * this.#takeCount(1) });
      }
    }

    // An unrelated key drops a half-typed count.
    this.#clear();
    return IGNORED;
  }

  /** Keys typed into the find bar: only Enter, Esc and Ctrl+f mean something; the rest is text. */
  #findBar(input: KeyPress): KeyResult {
    this.#clear();
    if (input.ctrlKey) return input.key.toLowerCase() === "f" ? run({ kind: "find" }) : IGNORED;
    if (input.key === "Enter") return run({ kind: "findNext", by: input.shiftKey ? -1 : 1 });
    if (input.key === "Escape") return run({ kind: "escape" });
    return IGNORED;
  }

  /** The pending count (or `fallback` without one), clearing all pending state. */
  #takeCount<T extends number | null>(fallback: T): number | T {
    const count = this.#count === "" ? fallback : Number(this.#count);
    this.#clear();
    return count;
  }

  #clear(): void {
    this.#count = "";
    this.#gPressedAt = null;
  }
}
