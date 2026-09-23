import { describe, expect, it } from "vitest";
import {
  THEME_MODES,
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
