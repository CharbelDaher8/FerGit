import { describe, expect, it } from "vitest";
import {
  KeyInterpreter,
  PENDING_G_TIMEOUT_MS,
  SIDEWAYS_COLUMNS,
  type Command,
  type KeyContext,
  type KeyResult,
} from "./keys";

interface Options {
  ctrl?: boolean;
  shift?: boolean;
  alt?: boolean;
  meta?: boolean;
  editable?: boolean;
  /** Milliseconds since the previous press (default 10). */
  after?: number;
  context?: KeyContext;
}

/** An interpreter with a fake clock, pressing keys in `context` unless told otherwise. */
function keyboard(context: KeyContext = "graph") {
  const interpreter = new KeyInterpreter();
  let now = 1000;
  const press = (key: string, options: Options = {}): KeyResult => {
    now += options.after ?? 10;
    return interpreter.press(
      {
        key,
        ctrlKey: options.ctrl ?? false,
        shiftKey: options.shift ?? false,
        altKey: options.alt ?? false,
        metaKey: options.meta ?? false,
        editable: options.editable ?? false,
        context: options.context ?? context,
      },
      now,
    );
  };
  /** Presses each key and returns the commands that came out. */
  const type = (...keys: string[]): Command[] =>
    keys.map((key) => press(key).command).filter((command): command is Command => command !== null);
  return { interpreter, press, type, advance: (ms: number) => (now += ms), now: () => now };
}

describe("counts", () => {
  it("moves one row without a count", () => {
    const { type } = keyboard();
    expect(type("j")).toEqual([{ kind: "move", by: 1 }]);
    expect(type("k")).toEqual([{ kind: "move", by: -1 }]);
  });

  it("applies a count to the next command, and shows it while pending", () => {
    const { interpreter, press, type } = keyboard();
    expect(press("5")).toEqual({ command: null, handled: true });
    expect(interpreter.pending).toBe("5");
    expect(type("j")).toEqual([{ kind: "move", by: 5 }]);
    expect(interpreter.pending).toBe("");
    expect(type("1", "0", "k")).toEqual([{ kind: "move", by: -10 }]);
  });

  it("treats 0 as a digit inside a count, and as a command on its own only in a diff", () => {
    expect(keyboard().type("1", "0", "0", "j")).toEqual([{ kind: "move", by: 100 }]);
    const graph = keyboard("graph");
    expect(graph.press("0")).toEqual({ command: null, handled: false });
    expect(graph.interpreter.pending).toBe("");
    expect(keyboard("diff").type("0")).toEqual([{ kind: "lineEdge", edge: "start" }]);
    expect(keyboard("diff").type("2", "0", "j")).toEqual([{ kind: "move", by: 20 }]);
  });

  it("doesn't let lone modifier keys interrupt a sequence (Shift before G)", () => {
    const { press, type } = keyboard();
    press("2");
    press("0");
    expect(press("Shift")).toEqual({ command: null, handled: false });
    expect(type("G")).toEqual([{ kind: "goto", row: 19 }]);
  });

  it("drops a count on an unrelated key", () => {
    const { interpreter, press, type } = keyboard();
    press("5");
    expect(press("Tab")).toEqual({ command: null, handled: false });
    expect(interpreter.pending).toBe("");
    expect(type("j")).toEqual([{ kind: "move", by: 1 }]);
  });

  it("caps absurdly long counts", () => {
    const { type } = keyboard();
    const [command] = type(..."123456789012".split(""), "j");
    expect(command).toEqual({ kind: "move", by: 1234567 });
  });
});

describe("g, gg and G", () => {
  it("goes to the first row with gg, and the last with G", () => {
    const { interpreter, press, type } = keyboard();
    press("g");
    expect(interpreter.pending).toBe("g");
    expect(type("g")).toEqual([{ kind: "goto", row: 0 }]);
    expect(type("G")).toEqual([{ kind: "goto", row: "last" }]);
  });

  it("goes to a 1-based row with {count}G or {count}gg", () => {
    const { type } = keyboard();
    expect(type("2", "0", "G")).toEqual([{ kind: "goto", row: 19 }]);
    expect(type("3", "g", "g")).toEqual([{ kind: "goto", row: 2 }]);
    expect(type("1", "G")).toEqual([{ kind: "goto", row: 0 }]);
  });

  it("waits up to the timeout for the second g", () => {
    const { press } = keyboard();
    press("g");
    expect(press("g", { after: PENDING_G_TIMEOUT_MS })).toEqual({ command: { kind: "goto", row: 0 }, handled: true });
  });

  it("forgets a g (and its count) after the timeout", () => {
    const { interpreter, press } = keyboard();
    press("4");
    press("g");
    expect(interpreter.pending).toBe("4g");
    // The second g arrives too late: it starts a new sequence instead of completing one.
    expect(press("g", { after: PENDING_G_TIMEOUT_MS + 1 })).toEqual({ command: null, handled: true });
    expect(interpreter.pending).toBe("g");
  });

  it("expires a pending g when asked, so the indicator can clear itself", () => {
    const { interpreter, press, now } = keyboard();
    press("g");
    expect(interpreter.expire(now() + PENDING_G_TIMEOUT_MS)).toBe(false);
    expect(interpreter.pending).toBe("g");
    expect(interpreter.expire(now() + PENDING_G_TIMEOUT_MS + 1)).toBe(true);
    expect(interpreter.pending).toBe("");
  });

  it("drops a half-typed g when another key follows, and runs that key without the count", () => {
    const { interpreter, type } = keyboard();
    expect(type("5", "g", "j")).toEqual([{ kind: "move", by: 1 }]);
    expect(interpreter.pending).toBe("");
  });

  it("starts a count over when a digit follows g", () => {
    const { interpreter, press } = keyboard();
    press("g");
    press("7");
    expect(interpreter.pending).toBe("7");
  });
});

describe("Ctrl", () => {
  it("pages with Ctrl+d/u (half) and Ctrl+b (full), with counts", () => {
    const { press } = keyboard();
    expect(press("d", { ctrl: true }).command).toEqual({ kind: "page", by: 0.5 });
    expect(press("u", { ctrl: true }).command).toEqual({ kind: "page", by: -0.5 });
    press("2");
    expect(press("b", { ctrl: true }).command).toEqual({ kind: "page", by: -2 });
  });

  it("finds with Ctrl+f instead of paging, from everywhere but the help", () => {
    for (const context of ["graph", "files", "diff", "find", "other"] as const) {
      expect(keyboard(context).press("f", { ctrl: true }).command).toEqual({ kind: "find" });
      expect(keyboard(context).press("F", { ctrl: true }).command).toEqual({ kind: "find" });
    }
    expect(keyboard("help").press("f", { ctrl: true })).toEqual({ command: null, handled: false });
  });

  it("ignores other Ctrl combinations and keeps what is pending", () => {
    const { interpreter, press } = keyboard();
    press("5");
    expect(press("c", { ctrl: true })).toEqual({ command: null, handled: false });
    expect(press("j", { ctrl: true })).toEqual({ command: null, handled: false });
    expect(interpreter.pending).toBe("5");
  });

  it("doesn't page the file list", () => {
    expect(keyboard("files").press("d", { ctrl: true })).toEqual({ command: null, handled: false });
  });
});

describe("ignored presses", () => {
  it("ignores Alt and Meta, and leaves pending keys alone", () => {
    const { interpreter, press } = keyboard();
    press("3");
    expect(press("j", { alt: true })).toEqual({ command: null, handled: false });
    expect(press("j", { meta: true })).toEqual({ command: null, handled: false });
    expect(interpreter.pending).toBe("3");
  });

  it("ignores typing into editable elements", () => {
    const { interpreter, press } = keyboard();
    expect(press("j", { editable: true })).toEqual({ command: null, handled: false });
    expect(press("5", { editable: true })).toEqual({ command: null, handled: false });
    expect(press("Escape", { editable: true })).toEqual({ command: null, handled: false });
    expect(interpreter.pending).toBe("");
  });
});

describe("Esc and ?", () => {
  it("cancels a pending count or g without running anything", () => {
    const { interpreter, press } = keyboard();
    press("5");
    press("g");
    expect(press("Escape")).toEqual({ command: null, handled: true });
    expect(interpreter.pending).toBe("");
  });

  it("becomes an escape command when nothing is pending, in every context", () => {
    for (const context of ["graph", "files", "diff", "help", "other"] as const) {
      expect(keyboard(context).press("Escape").command).toEqual({ kind: "escape" });
    }
  });

  it("toggles help from anywhere, and ignores everything else while help or a control has focus", () => {
    for (const context of ["graph", "files", "diff", "help", "other"] as const) {
      expect(keyboard(context).press("?").command).toEqual({ kind: "help" });
    }
    expect(keyboard("help").press("j")).toEqual({ command: null, handled: false });
    expect(keyboard("other").press("j")).toEqual({ command: null, handled: false });
    expect(keyboard("other").press("Enter")).toEqual({ command: null, handled: false });
  });

  it("switches the theme from anywhere with T, but not with t, Ctrl+T or in a text field", () => {
    for (const context of ["graph", "files", "diff", "help", "other"] as const) {
      expect(keyboard(context).press("T")).toEqual({ command: { kind: "theme" }, handled: true });
      expect(keyboard(context).press("t").command).toBeNull();
      expect(keyboard(context).press("T", { ctrl: true }).command).not.toEqual({ kind: "theme" });
      expect(keyboard(context).press("T", { editable: true }).command).toBeNull();
    }
  });

  it("drops a half-typed count or g on T", () => {
    const { interpreter, press, type } = keyboard();
    press("5");
    press("g");
    expect(type("T")).toEqual([{ kind: "theme" }]);
    expect(interpreter.pending).toBe("");
    expect(type("j")).toEqual([{ kind: "move", by: 1 }]);
  });
});

describe("find", () => {
  it("opens with / and steps through matches with n and N, counts included", () => {
    for (const context of ["graph", "files"] as const) {
      expect(keyboard(context).type("/", "n", "N", "3", "n", "2", "N")).toEqual([
        { kind: "find" },
        { kind: "findNext", by: 1 },
        { kind: "findNext", by: -1 },
        { kind: "findNext", by: 3 },
        { kind: "findNext", by: -2 },
      ]);
    }
  });

  it("leaves / and n to the diff's own keys", () => {
    const { press } = keyboard("diff");
    expect(press("/")).toEqual({ command: null, handled: false });
    expect(press("n")).toEqual({ command: null, handled: false });
  });

  it("in the find bar, Enter and Shift+Enter step, Esc escapes, and every other key types", () => {
    const { interpreter, press } = keyboard("find");
    expect(press("Enter").command).toEqual({ kind: "findNext", by: 1 });
    expect(
      interpreter.press(
        { key: "Enter", shiftKey: true, ctrlKey: false, altKey: false, metaKey: false, editable: true, context: "find" },
        5000,
      ).command,
    ).toEqual({ kind: "findNext", by: -1 });
    expect(press("Escape", { editable: true }).command).toEqual({ kind: "escape" });
    for (const key of ["j", "n", "5", "g", "?", "/", "ArrowDown"]) {
      expect(press(key, { editable: true })).toEqual({ command: null, handled: false });
    }
    expect(press("d", { ctrl: true, editable: true })).toEqual({ command: null, handled: false });
    expect(interpreter.pending).toBe("");
  });
});

describe("keymaps", () => {
  it("graph: l and Enter go to the file list, h does nothing, arrows and paging keys work", () => {
    const { press, type } = keyboard("graph");
    expect(type("l")).toEqual([{ kind: "focus", pane: "files" }]);
    expect(type("Enter")).toEqual([{ kind: "focus", pane: "files" }]);
    expect(press("h")).toEqual({ command: null, handled: true });
    expect(type("ArrowDown", "ArrowUp")).toEqual([
      { kind: "move", by: 1 },
      { kind: "move", by: -1 },
    ]);
    expect(type("PageDown", "PageUp", "Home", "End")).toEqual([
      { kind: "page", by: 1 },
      { kind: "page", by: -1 },
      { kind: "goto", row: 0 },
      { kind: "goto", row: "last" },
    ]);
    expect(press("q")).toEqual({ command: null, handled: false });
  });

  it("the menu key and Shift+F10 open the context menu from the graph only", () => {
    const graph = keyboard("graph");
    graph.press("3");
    expect(graph.press("ContextMenu")).toEqual({ command: { kind: "menu" }, handled: true });
    expect(graph.interpreter.pending).toBe("");
    expect(graph.press("F10", { shift: true }).command).toEqual({ kind: "menu" });
    expect(graph.press("F10")).toEqual({ command: null, handled: false });
    expect(keyboard("files").press("ContextMenu")).toEqual({ command: null, handled: false });
    expect(keyboard("diff").press("F10", { shift: true })).toEqual({ command: null, handled: false });
  });

  it("u undoes from the graph and the file list, but not in a diff", () => {
    expect(keyboard("graph").type("3", "u")).toEqual([{ kind: "undo" }]);
    expect(keyboard("files").type("u")).toEqual([{ kind: "undo" }]);
    expect(keyboard("diff").press("u")).toEqual({ command: null, handled: false });
    expect(keyboard("graph").press("u", { ctrl: true }).command).toEqual({ kind: "page", by: -0.5 });
  });

  it("files: j/k move, gg/G go to the ends, l and Enter open, h goes back to the graph", () => {
    const { type } = keyboard("files");
    expect(type("j", "k", "g", "g", "G")).toEqual([
      { kind: "move", by: 1 },
      { kind: "move", by: -1 },
      { kind: "goto", row: 0 },
      { kind: "goto", row: "last" },
    ]);
    expect(type("l", "Enter")).toEqual([{ kind: "open" }, { kind: "open" }]);
    expect(type("h")).toEqual([{ kind: "focus", pane: "graph" }]);
  });

  it("diff: scrolls with j/k, h/l, 0/$, gg/G, and closes with q; arrows keep native scrolling", () => {
    const { press, type } = keyboard("diff");
    expect(type("j", "3", "k", "h", "2", "l", "$", "0", "g", "g", "G", "q")).toEqual([
      { kind: "move", by: 1 },
      { kind: "move", by: -3 },
      { kind: "scrollX", by: -SIDEWAYS_COLUMNS },
      { kind: "scrollX", by: 2 * SIDEWAYS_COLUMNS },
      { kind: "lineEdge", edge: "end" },
      { kind: "lineEdge", edge: "start" },
      { kind: "goto", row: 0 },
      { kind: "goto", row: "last" },
      { kind: "close" },
    ]);
    expect(press("ArrowDown")).toEqual({ command: null, handled: false });
    expect(press("Enter")).toEqual({ command: null, handled: false });
    expect(press("d", { ctrl: true }).command).toEqual({ kind: "page", by: 0.5 });
  });
});

describe("tabs", () => {
  it("opens, closes and switches tabs with Ctrl, in every context", () => {
    for (const context of ["graph", "files", "diff", "help", "other"] as const) {
      const { press } = keyboard(context);
      expect(press("t", { ctrl: true }).command).toEqual({ kind: "newTab" });
      expect(press("w", { ctrl: true }).command).toEqual({ kind: "closeTab" });
      expect(press("Tab", { ctrl: true }).command).toEqual({ kind: "switchTab", by: 1 });
      expect(press("Tab", { ctrl: true, shift: true }).command).toEqual({ kind: "switchTab", by: -1 });
    }
  });

  it("works from editable elements too, and with Caps Lock on", () => {
    const { press } = keyboard();
    expect(press("w", { ctrl: true, editable: true }).command).toEqual({ kind: "closeTab" });
    expect(press("T", { ctrl: true }).command).toEqual({ kind: "newTab" });
  });

  it("leaves other combinations alone", () => {
    const { press } = keyboard();
    expect(press("T", { ctrl: true, shift: true })).toEqual({ command: null, handled: false });
    expect(press("t", { ctrl: true, alt: true })).toEqual({ command: null, handled: false });
    expect(press("Tab")).toEqual({ command: null, handled: false });
  });

  it("drops a half-typed sequence", () => {
    const { interpreter, press, type } = keyboard();
    type("5", "g");
    press("Tab", { ctrl: true });
    expect(interpreter.pending).toBe("");
    expect(type("j")).toEqual([{ kind: "move", by: 1 }]);
  });
});
