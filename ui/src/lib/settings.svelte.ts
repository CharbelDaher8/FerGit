import { DARK_SCHEME_QUERY, THEME_STORAGE_KEY, ThemeState, parseThemeMode, type ThemeMode } from "./theme.svelte";

/** Where settings persist: `localStorage` in the app, a stand-in in tests. */
export type SettingsStorage = Pick<Storage, "getItem" | "setItem">;

const SHOW_RELATIONS_KEY = "fergit.showRelations";
const TABS_KEY = "fergit.tabs";

/** The tabs open when the app last ran, to reopen on the next launch. */
export interface SavedTabs {
  /** Repository roots, in tab order. */
  paths: string[];
  /** Index into `paths` of the active tab; `null` when there are no tabs. */
  active: number | null;
}

const NO_TABS: SavedTabs = { paths: [], active: null };

/**
 * Viewer preferences that persist across launches. Storage can be missing or throw (disabled,
 * private mode, quota), so reads fall back to the default and failed writes still apply for the
 * current session. Reads are reactive.
 */
export class Settings {
  readonly #storage: SettingsStorage | null;
  #showRelations = $state(true);
  #tabs = $state.raw<SavedTabs>(NO_TABS);
  #themeMode = $state<ThemeMode>("system");

  constructor(storage: SettingsStorage | null) {
    this.#storage = storage;
    const stored = read(storage, SHOW_RELATIONS_KEY);
    if (stored !== null) this.#showRelations = stored !== "false";
    this.#tabs = parseTabs(read(storage, TABS_KEY));
    this.#themeMode = parseThemeMode(read(storage, THEME_STORAGE_KEY));
  }

  /** Whether branch relationship labels are drawn on graph lines. On by default. */
  get showRelations(): boolean {
    return this.#showRelations;
  }

  set showRelations(value: boolean) {
    this.#showRelations = value;
    write(this.#storage, SHOW_RELATIONS_KEY, String(value));
  }

  /** Light, dark, or whatever the OS prefers (the default). */
  get themeMode(): ThemeMode {
    return this.#themeMode;
  }

  set themeMode(value: ThemeMode) {
    this.#themeMode = value;
    write(this.#storage, THEME_STORAGE_KEY, value);
  }
  /** The open tabs, as last saved. None by default. */
  get tabs(): SavedTabs {
    return this.#tabs;
  }

  set tabs(value: SavedTabs) {
    this.#tabs = value;
    write(this.#storage, TABS_KEY, JSON.stringify(value));
  }
}

/**
 * Reads saved tabs leniently: anything unreadable (a hand-edited or future format) means no tabs,
 * entries that aren't paths are skipped, and an out-of-range active index falls back to the first.
 */
export function parseTabs(stored: string | null): SavedTabs {
  if (stored === null) return NO_TABS;
  let value: unknown;
  try {
    value = JSON.parse(stored);
  } catch {
    return NO_TABS;
  }
  if (typeof value !== "object" || value === null || !("paths" in value) || !Array.isArray(value.paths)) {
    return NO_TABS;
  }
  const paths = value.paths.filter((path): path is string => typeof path === "string" && path !== "");
  if (paths.length === 0) return NO_TABS;
  const active = "active" in value ? value.active : null;
  const valid = typeof active === "number" && Number.isInteger(active) && active >= 0 && active < paths.length;
  return { paths, active: valid ? active : 0 };
}

function read(storage: SettingsStorage | null, key: string): string | null {
  try {
    return storage?.getItem(key) ?? null;
  } catch {
    return null;
  }
}

function write(storage: SettingsStorage | null, key: string, value: string): void {
  try {
    storage?.setItem(key, value);
  } catch {
    // Not persisted; the value still applies until the app closes.
  }
}

function browserStorage(): SettingsStorage | null {
  try {
    return globalThis.localStorage ?? null;
  } catch {
    return null;
  }
}

function darkSchemeQuery(): MediaQueryList | null {
  return typeof globalThis.matchMedia === "function" ? globalThis.matchMedia(DARK_SCHEME_QUERY) : null;
}

export const settings = new Settings(browserStorage());
/** The theme shown, from `settings.themeMode` and the OS preference. */
export const theme = new ThemeState(settings, darkSchemeQuery());
