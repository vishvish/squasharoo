export const DEFAULT_COMPRESSION_LEVEL = 3;
export const MIN_COMPRESSION_LEVEL = 1;
export const MAX_COMPRESSION_LEVEL = 22;

export type CompressionSettings = {
  compressionLevel: number;
  ignoredFiles: string[];
  ignoredFolders: string[];
};

export function clampCompressionLevel(value: number): number {
  if (!Number.isFinite(value)) {
    return DEFAULT_COMPRESSION_LEVEL;
  }

  return Math.min(MAX_COMPRESSION_LEVEL, Math.max(MIN_COMPRESSION_LEVEL, Math.round(value)));
}

export function normalizePatternList(text: string): string[] {
  const lines = text
    .split(/\r?\n/)
    .map((line) => line.trim())
    .filter(Boolean);

  return lines.filter((line, index) => lines.indexOf(line) === index);
}

export function formatPatternList(patterns: string[]): string {
  return patterns.join("\n");
}

export function normalizeSettingsDraft(
  compressionLevel: number,
  ignoredFilesText: string,
  ignoredFoldersText: string,
): CompressionSettings {
  return {
    compressionLevel: clampCompressionLevel(compressionLevel),
    ignoredFiles: normalizePatternList(ignoredFilesText),
    ignoredFolders: normalizePatternList(ignoredFoldersText),
  };
}
