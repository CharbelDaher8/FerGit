import { DARK_SCHEME_QUERY, THEME_STORAGE_KEY, ThemeState, parseThemeMode, type ThemeMode } from "./theme.svelte";

/** Where settings persist: `localStorage` in the app, a stand-in in tests. */
export type SettingsStorage = Pick<Storage, "getItem" | "setItem">;

const SHOW_RELATIONS_KEY = "fergit.showRelations";

/**
 * Viewer preferences that persist across launches. Storage can be missing or throw (disabled,
 * private mode, quota), so reads fall back to the default and failed writes still apply for the
 * current session. Reads are reactive.
 */
export class Settings {
  readonly #storage: SettingsStorage | null;
  #showRelations = $state(true);
  #themeMode = $state<ThemeMode>("system");

  constructor(storage: SettingsStorage | null) {
    this.#storage = storage;
    const stored = read(storage, SHOW_RELATIONS_KEY);
    if (stored !== null) this.#showRelations = stored !== "false";
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
