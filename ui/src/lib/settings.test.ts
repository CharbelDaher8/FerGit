import { describe, expect, it, vi } from "vitest";
import { Settings, type SettingsStorage } from "./settings.svelte";

function memoryStorage(initial: Record<string, string> = {}) {
  const values = new Map(Object.entries(initial));
  return {
    values,
    getItem: vi.fn((key: string) => values.get(key) ?? null),
    setItem: vi.fn((key: string, value: string) => void values.set(key, value)),
  } satisfies SettingsStorage & { values: Map<string, string> };
}

describe("Settings", () => {
  it("shows relationships by default", () => {
    expect(new Settings(null).showRelations).toBe(true);
    expect(new Settings(memoryStorage()).showRelations).toBe(true);
  });

  it("reads a stored choice", () => {
    expect(new Settings(memoryStorage({ "fergit.showRelations": "false" })).showRelations).toBe(false);
    expect(new Settings(memoryStorage({ "fergit.showRelations": "true" })).showRelations).toBe(true);
    expect(new Settings(memoryStorage({ "fergit.showRelations": "garbage" })).showRelations).toBe(true);
  });

  it("persists changes, and a new instance reads them back", () => {
    const storage = memoryStorage();
    const settings = new Settings(storage);
    settings.showRelations = false;
    expect(settings.showRelations).toBe(false);
    expect(storage.setItem).toHaveBeenCalledWith("fergit.showRelations", "false");
    expect(new Settings(storage).showRelations).toBe(false);
  });

  it("follows the OS theme by default, and reads a stored theme", () => {
    expect(new Settings(null).themeMode).toBe("system");
    expect(new Settings(memoryStorage()).themeMode).toBe("system");
    expect(new Settings(memoryStorage({ "fergit.theme": "dark" })).themeMode).toBe("dark");
    expect(new Settings(memoryStorage({ "fergit.theme": "light" })).themeMode).toBe("light");
    expect(new Settings(memoryStorage({ "fergit.theme": "neon" })).themeMode).toBe("system");
  });

  it("persists the theme, and a new instance reads it back", () => {
    const storage = memoryStorage();
    const settings = new Settings(storage);
    settings.themeMode = "dark";
    expect(storage.setItem).toHaveBeenCalledWith("fergit.theme", "dark");
    expect(new Settings(storage).themeMode).toBe("dark");
    settings.themeMode = "system";
    expect(new Settings(storage).themeMode).toBe("system");
  });

  it("keeps working when storage throws", () => {
    const broken: SettingsStorage = {
      getItem: () => {
        throw new Error("denied");
      },
      setItem: () => {
        throw new Error("quota");
      },
    };
    const settings = new Settings(broken);
    expect(settings.showRelations).toBe(true);
    settings.showRelations = false;
    expect(settings.showRelations).toBe(false);
    expect(settings.themeMode).toBe("system");
    settings.themeMode = "light";
    expect(settings.themeMode).toBe("light");
  });
});
