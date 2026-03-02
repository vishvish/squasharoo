import { describe, expect, it } from "vitest";

import {
  clampCompressionLevel,
  formatPatternList,
  normalizePatternList,
  normalizeSettingsDraft,
} from "./settings";

describe("settings helpers", () => {
  it("clamps compression levels into the supported zstd range", () => {
    expect(clampCompressionLevel(-4)).toBe(1);
    expect(clampCompressionLevel(99)).toBe(22);
    expect(clampCompressionLevel(7.6)).toBe(8);
  });

  it("normalizes ignore lists from textarea input", () => {
    expect(normalizePatternList("  *.tmp  \n\nnode_modules\n*.tmp\n")).toEqual([
      "*.tmp",
      "node_modules",
    ]);
  });

  it("formats stored patterns back into multiline text", () => {
    expect(formatPatternList([".DS_Store", "node_modules"])).toBe(".DS_Store\nnode_modules");
  });

  it("builds a normalized settings object from the draft fields", () => {
    expect(normalizeSettingsDraft(40, "*.tmp\n*.tmp", " node_modules \n")).toEqual({
      compressionLevel: 22,
      ignoredFiles: ["*.tmp"],
      ignoredFolders: ["node_modules"],
    });
  });
});
