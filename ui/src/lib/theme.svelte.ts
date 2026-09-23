// Light and dark themes. The user picks a mode; `system` follows the OS and keeps following it
// while the app runs. The colors themselves are CSS tokens in app.css, switched by `data-theme` on
// the root element.

/** What the user chose. */
export type ThemeMode = "system" | "light" | "dark";
/** What is shown: a mode with `system` resolved. */
export type Theme = "light" | "dark";

/** In the order the theme shortcut and button cycle through them. */
export const THEME_MODES: readonly ThemeMode[] = ["system", "light", "dark"];

/**
 * The storage key for the mode. /theme-boot.js reads it too, to set the theme before first paint;
 * a test keeps the two in step.
 */
export const THEME_STORAGE_KEY = "fergit.theme";

export const DARK_SCHEME_QUERY = "(prefers-color-scheme: dark)";

/** A stored mode; anything unrecognized (nothing stored, an older or newer value) is `system`. */
export function parseThemeMode(value: string | null): ThemeMode {
  return THEME_MODES.find((mode) => mode === value) ?? "system";
}

export function resolveTheme(mode: ThemeMode, systemDark: boolean): Theme {
  if (mode === "system") return systemDark ? "dark" : "light";
  return mode;
}

/** The mode after `mode` in `THEME_MODES`, wrapping around. */
export function nextThemeMode(mode: ThemeMode): ThemeMode {
  return THEME_MODES[(THEME_MODES.indexOf(mode) + 1) % THEME_MODES.length];
}

/** The part of a `MediaQueryList` the theme needs, so tests can stand in for the OS. */
export interface SchemeQuery {
  readonly matches: boolean;
  addEventListener(type: "change", listener: () => void): void;
  removeEventListener(type: "change", listener: () => void): void;
}

/** Where the mode persists: the app's settings. */
export interface ThemeSettings {
  themeMode: ThemeMode;
}

/**
 * The current theme, reactively: the persisted mode plus, for `system`, what the OS prefers. Call
 * `follow` to track OS changes; without a scheme query (no DOM) the OS counts as light.
 */
export class ThemeState {
  readonly #settings: ThemeSettings;
  readonly #query: SchemeQuery | null;
  #systemDark = $state(false);

  constructor(settings: ThemeSettings, query: SchemeQuery | null) {
    this.#settings = settings;
    this.#query = query;
    this.#systemDark = query?.matches ?? false;
  }

  get mode(): ThemeMode {
    return this.#settings.themeMode;
  }

  set mode(mode: ThemeMode) {
    this.#settings.themeMode = mode;
  }

  get theme(): Theme {
    return resolveTheme(this.mode, this.#systemDark);
  }

  /** Switches to the next mode: system, light, dark, system… */
  cycle(): void {
    this.mode = nextThemeMode(this.mode);
  }

  /** Tracks the OS preference until the returned function is called. */
  follow(): () => void {
    const query = this.#query;
    if (!query) return () => {};
    const update = () => (this.#systemDark = query.matches);
    update();
    query.addEventListener("change", update);
    return () => query.removeEventListener("change", update);
  }
}

/** Shows `theme` on the page: `data-theme` picks the token set in app.css. */
export function applyTheme(root: HTMLElement, theme: Theme): void {
  root.dataset.theme = theme;
}
