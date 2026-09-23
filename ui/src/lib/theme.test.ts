import { describe, expect, it } from "vitest";
import bootScript from "../../public/theme-boot.js?raw";
import appCss from "../app.css?raw";
import { LANE_COLORS, laneToken } from "./graph";
import {
  DARK_SCHEME_QUERY,
  THEME_MODES,
  THEME_STORAGE_KEY,
  ThemeState,
  nextThemeMode,
  parseThemeMode,
  resolveTheme,
  type SchemeQuery,
  type ThemeMode,
} from "./theme.svelte";

/** A stand-in for `matchMedia("(prefers-color-scheme: dark)")` that the test can flip. */
function osScheme(dark: boolean) {
  const listeners = new Set<() => void>();
  const query = {
    matches: dark,
    addEventListener: (_type: "change", listener: () => void) => void listeners.add(listener),
    removeEventListener: (_type: "change", listener: () => void) => void listeners.delete(listener),
    listeners,
    set(value: boolean) {
      query.matches = value;
      for (const listener of listeners) listener();
    },
  };
  return query satisfies SchemeQuery;
}

describe("theme modes", () => {
  it("reads stored modes, and treats anything else as system", () => {
    expect(parseThemeMode("light")).toBe("light");
    expect(parseThemeMode("dark")).toBe("dark");
    expect(parseThemeMode("system")).toBe("system");
    expect(parseThemeMode(null)).toBe("system");
    expect(parseThemeMode("")).toBe("system");
    expect(parseThemeMode("Dark")).toBe("system");
    expect(parseThemeMode("solarized")).toBe("system");
  });

  it("resolves system from the OS preference, and fixed modes regardless of it", () => {
    expect(resolveTheme("system", true)).toBe("dark");
    expect(resolveTheme("system", false)).toBe("light");
    for (const systemDark of [true, false]) {
      expect(resolveTheme("light", systemDark)).toBe("light");
      expect(resolveTheme("dark", systemDark)).toBe("dark");
    }
  });

  it("cycles system, light, dark and back", () => {
    expect(nextThemeMode("system")).toBe("light");
    expect(nextThemeMode("light")).toBe("dark");
    expect(nextThemeMode("dark")).toBe("system");
    let mode: ThemeMode = "system";
    for (let i = 0; i < THEME_MODES.length; i++) mode = nextThemeMode(mode);
    expect(mode).toBe("system");
  });
});

describe("ThemeState", () => {
  it("follows the OS in system mode, live", () => {
    const os = osScheme(false);
    const theme = new ThemeState({ themeMode: "system" }, os);
    expect(theme.theme).toBe("light");
    const stop = theme.follow();
    os.set(true);
    expect(theme.theme).toBe("dark");
    os.set(false);
    expect(theme.theme).toBe("light");
    stop();
    expect(os.listeners.size).toBe(0);
    os.set(true);
    expect(theme.theme).toBe("light");
  });

  it("picks up an OS change made before following started", () => {
    const os = osScheme(false);
    const theme = new ThemeState({ themeMode: "system" }, os);
    os.matches = true;
    theme.follow();
    expect(theme.theme).toBe("dark");
  });

  it("ignores the OS in a fixed mode", () => {
    const os = osScheme(true);
    const theme = new ThemeState({ themeMode: "light" }, os);
    theme.follow();
    expect(theme.theme).toBe("light");
    os.set(false);
    theme.mode = "dark";
    expect(theme.theme).toBe("dark");
  });

  it("stores the mode in the settings it was given", () => {
    const settings: { themeMode: ThemeMode } = { themeMode: "system" };
    const theme = new ThemeState(settings, osScheme(false));
    theme.cycle();
    expect(settings.themeMode).toBe("light");
    expect(theme.theme).toBe("light");
    theme.cycle();
    expect(settings.themeMode).toBe("dark");
    expect(theme.theme).toBe("dark");
    theme.cycle();
    expect(theme.mode).toBe("system");
    expect(theme.theme).toBe("light");
  });

  it("counts the OS as light without a DOM", () => {
    const theme = new ThemeState({ themeMode: "system" }, null);
    expect(theme.follow()).toBeTypeOf("function");
    expect(theme.theme).toBe("light");
  });
});

describe("the pre-paint boot script", () => {
  /** Runs theme-boot.js against a fake page and returns the `data-theme` it set. */
  function boot(stored: string | null | Error, systemDark: boolean): string | undefined {
    const dataset: Record<string, string> = {};
    const localStorage = {
      getItem(key: string) {
        if (stored instanceof Error) throw stored;
        return key === THEME_STORAGE_KEY ? stored : null;
      },
    };
    const window = {
      matchMedia: (query: string) => ({ matches: query === DARK_SCHEME_QUERY && systemDark }),
    };
    const document = { documentElement: { dataset } };
    new Function("localStorage", "window", "document", bootScript)(localStorage, window, document);
    return dataset.theme;
  }

  it("resolves the stored mode the same way the app does", () => {
    for (const stored of [null, "system", "light", "dark", "neon"]) {
      for (const systemDark of [true, false]) {
        expect(boot(stored, systemDark), `${stored} / OS dark: ${systemDark}`).toBe(
          resolveTheme(parseThemeMode(stored), systemDark),
        );
      }
    }
  });

  it("follows the OS when storage throws", () => {
    expect(boot(new Error("denied"), true)).toBe("dark");
    expect(boot(new Error("denied"), false)).toBe("light");
  });
});

describe("the theme tokens in app.css", () => {
  /** The custom properties a rule defines, by its exact selector. */
  function tokens(selector: string): Map<string, string> {
    const start = appCss.indexOf(`${selector} {`);
    expect(start, selector).toBeGreaterThanOrEqual(0);
    const body = appCss.slice(start, appCss.indexOf("}", start));
    return new Map([...body.matchAll(/(--[\w-]+):\s*([^;]+);/g)].map((match) => [match[1], match[2].trim()]));
  }
  const light = tokens(":root");
  const dark = tokens(':root[data-theme="dark"]');

  it("gives the dark theme its own value for every color", () => {
    const colors = [...light.keys()].filter((name) => !name.startsWith("--font-"));
    expect([...dark.keys()].sort()).toEqual(colors.sort());
  });

  it("defines every lane color in both themes, all distinct", () => {
    for (const theme of [light, dark]) {
      const lanes = Array.from({ length: LANE_COLORS }, (_, color) => theme.get(laneToken(color)));
      expect(lanes.every((value) => value !== undefined)).toBe(true);
      expect(new Set(lanes).size).toBe(LANE_COLORS);
      expect(theme.has(`--lane-${LANE_COLORS}`)).toBe(false);
    }
  });
});
