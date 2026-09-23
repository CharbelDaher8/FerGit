import { describe, expect, it, vi } from "vitest";
import { Settings, parseTabs, type SettingsStorage } from "./settings.svelte";

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
  });
});

describe("saved tabs", () => {
  it("has none by default, and reads back what was saved", () => {
    const storage = memoryStorage();
    expect(new Settings(storage).tabs).toEqual({ paths: [], active: null });
    new Settings(storage).tabs = { paths: ["/a", "/b"], active: 1 };
    expect(new Settings(storage).tabs).toEqual({ paths: ["/a", "/b"], active: 1 });
  });

  it("reads anything unreadable as no tabs", () => {
    for (const stored of [null, "", "{", "null", "[]", '{"paths":"/a"}', '{"paths":[]}', '{"paths":[1,""]}']) {
      expect(parseTabs(stored)).toEqual({ paths: [], active: null });
    }
  });

  it("skips entries that aren't paths and falls back to the first tab for a bad active index", () => {
    expect(parseTabs('{"paths":["/a",3,"/b"],"active":1}')).toEqual({ paths: ["/a", "/b"], active: 1 });
    expect(parseTabs('{"paths":["/a","/b"],"active":2}')).toEqual({ paths: ["/a", "/b"], active: 0 });
    expect(parseTabs('{"paths":["/a","/b"],"active":0.5}')).toEqual({ paths: ["/a", "/b"], active: 0 });
    expect(parseTabs('{"paths":["/a"]}')).toEqual({ paths: ["/a"], active: 0 });
  });
});
