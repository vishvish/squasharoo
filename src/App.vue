<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, ref } from "vue";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { invoke } from "@tauri-apps/api/core";

import {
  DEFAULT_COMPRESSION_LEVEL,
  formatPatternList,
  normalizeSettingsDraft,
  type CompressionSettings,
} from "./settings";

type DragDropPayload = {
  paths?: string[];
};

type CompressionOutcome = {
  sourcePath: string;
  outputPath: string | null;
  status: "compressed" | "skipped" | "failed";
  detail: string;
};

const compressionLevel = ref(DEFAULT_COMPRESSION_LEVEL);
const ignoredFilesText = ref(".DS_Store\nThumbs.db\n*.tmp\n*.temp");
const ignoredFoldersText = ref(".git\nnode_modules");
const isDragging = ref(false);
const isBusy = ref(false);
const isSaving = ref(false);
const statusMessage = ref("Drop files or folders anywhere on the window to squash them.");
const results = ref<CompressionOutcome[]>([]);
const lastDropCount = ref(0);

const dropHeadline = computed(() =>
  isBusy.value
    ? `Compressing ${lastDropCount.value || 1} item${lastDropCount.value === 1 ? "" : "s"}`
    : "Drop anything to compress it",
);

function draftSettings(): CompressionSettings {
  return normalizeSettingsDraft(
    compressionLevel.value,
    ignoredFilesText.value,
    ignoredFoldersText.value,
  );
}

function applySettings(settings: CompressionSettings) {
  compressionLevel.value = settings.compressionLevel;
  ignoredFilesText.value = formatPatternList(settings.ignoredFiles);
  ignoredFoldersText.value = formatPatternList(settings.ignoredFolders);
}

async function loadSettings() {
  try {
    const settings = await invoke<CompressionSettings>("load_settings");
    applySettings(settings);
  } catch (error) {
    statusMessage.value = `Could not load preferences: ${String(error)}`;
  }
}

async function saveSettings() {
  isSaving.value = true;
  statusMessage.value = "Saving preferences.";

  try {
    const saved = await invoke<CompressionSettings>("save_settings", {
      settings: draftSettings(),
    });
    applySettings(saved);
    statusMessage.value = "Preferences saved. The next drop will use them.";
  } catch (error) {
    statusMessage.value = `Could not save preferences: ${String(error)}`;
  } finally {
    isSaving.value = false;
  }
}

async function compressDroppedPaths(paths: string[]) {
  if (paths.length === 0 || isBusy.value) {
    return;
  }

  const settings = draftSettings();
  applySettings(settings);
  isBusy.value = true;
  isDragging.value = false;
  lastDropCount.value = paths.length;
  results.value = [];
  statusMessage.value = `Compressing ${paths.length} item${paths.length === 1 ? "" : "s"}.`;

  try {
    const compressed = await invoke<CompressionOutcome[]>("compress_paths", {
      paths,
      settings,
    });
    results.value = compressed;

    const compressedCount = compressed.filter((item) => item.status === "compressed").length;
    const skippedCount = compressed.filter((item) => item.status === "skipped").length;
    const failedCount = compressed.filter((item) => item.status === "failed").length;

    statusMessage.value =
      failedCount > 0
        ? `Finished with ${failedCount} failure${failedCount === 1 ? "" : "s"}.`
        : skippedCount > 0
          ? `Finished with ${compressedCount} compressed and ${skippedCount} skipped.`
          : `Finished compressing ${compressedCount} item${compressedCount === 1 ? "" : "s"}.`;
  } catch (error) {
    statusMessage.value = `Compression failed: ${String(error)}`;
  } finally {
    isBusy.value = false;
  }
}

const cleanup: UnlistenFn[] = [];

onMounted(async () => {
  cleanup.push(
    await listen<DragDropPayload>("tauri://drag-enter", (event) => {
      if ((event.payload.paths?.length ?? 0) > 0) {
        isDragging.value = true;
      }
    }),
  );

  cleanup.push(
    await listen("tauri://drag-leave", () => {
      isDragging.value = false;
    }),
  );

  cleanup.push(
    await listen<DragDropPayload>("tauri://drag-drop", async (event) => {
      await compressDroppedPaths(event.payload.paths ?? []);
    }),
  );

  await loadSettings();
});

onBeforeUnmount(() => {
  for (const unlisten of cleanup) {
    unlisten();
  }
});
</script>

<template>
  <main class="shell">
    <section class="hero">
      <p class="eyebrow">squasheroo</p>
      <h1>{{ dropHeadline }}</h1>
      <p class="intro">
        {{ statusMessage }}
      </p>

      <div class="drop-zone" :class="{ dragging: isDragging, busy: isBusy }">
        <div class="drop-mark">.zst</div>
        <div>
          <p class="drop-title">Drop files or folders onto this window.</p>
          <p class="drop-copy">
            Files become <code>.zst</code>. Folders become <code>.tar.zst</code> with your ignore
            rules applied.
          </p>
        </div>
      </div>
    </section>

    <section class="settings-panel">
      <div class="panel-head">
        <div>
          <p class="eyebrow">Compression</p>
          <h2>Global behaviour</h2>
        </div>
        <button class="save-button" :disabled="isSaving || isBusy" @click="saveSettings">
          {{ isSaving ? "Saving..." : "Save preferences" }}
        </button>
      </div>

      <label class="range-field">
        <span>Compression level</span>
        <strong>{{ compressionLevel }}</strong>
      </label>
      <input
        v-model.number="compressionLevel"
        class="range-input"
        type="range"
        min="1"
        max="22"
        step="1"
      />
      <p class="hint">Lower is faster. Higher squeezes harder.</p>

      <div class="ignore-grid">
        <label class="text-field">
          <span>Ignore files</span>
          <textarea
            v-model="ignoredFilesText"
            rows="8"
            placeholder=".DS_Store&#10;*.tmp&#10;build/*.map"
          />
        </label>

        <label class="text-field">
          <span>Ignore folders</span>
          <textarea
            v-model="ignoredFoldersText"
            rows="8"
            placeholder=".git&#10;node_modules&#10;build/cache"
          />
        </label>
      </div>
    </section>

    <section v-if="results.length > 0" class="results-panel">
      <div class="panel-head">
        <div>
          <p class="eyebrow">Results</p>
          <h2>Latest run</h2>
        </div>
      </div>

      <ul class="result-list">
        <li v-for="result in results" :key="`${result.sourcePath}-${result.outputPath}`" class="result-row">
          <span class="status-pill" :class="result.status">{{ result.status }}</span>
          <div class="result-copy">
            <p class="source">{{ result.sourcePath }}</p>
            <p class="detail">{{ result.detail }}</p>
            <p v-if="result.outputPath" class="output">{{ result.outputPath }}</p>
          </div>
        </li>
      </ul>
    </section>
  </main>
</template>

<style scoped>
:global(:root) {
  color: #1a1a16;
  background:
    radial-gradient(circle at top left, rgba(220, 137, 74, 0.24), transparent 28%),
    radial-gradient(circle at top right, rgba(62, 106, 92, 0.22), transparent 30%),
    linear-gradient(180deg, #f5efe1 0%, #efe5d2 100%);
  font-family: "Avenir Next", "Gill Sans", "Trebuchet MS", sans-serif;
  font-synthesis: none;
  text-rendering: optimizeLegibility;
  -webkit-font-smoothing: antialiased;
  -moz-osx-font-smoothing: grayscale;
}

:global(body) {
  margin: 0;
}

:global(*) {
  box-sizing: border-box;
}

.shell {
  min-height: 100vh;
  padding: 32px;
  display: grid;
  gap: 24px;
}

.hero,
.settings-panel,
.results-panel {
  border: 1px solid rgba(51, 44, 34, 0.12);
  border-radius: 28px;
  background: rgba(255, 250, 241, 0.8);
  backdrop-filter: blur(10px);
  box-shadow: 0 20px 60px rgba(80, 57, 27, 0.12);
}

.hero {
  padding: 40px;
}

.settings-panel,
.results-panel {
  padding: 28px;
}

.eyebrow {
  margin: 0 0 10px;
  font-size: 0.75rem;
  letter-spacing: 0.18em;
  text-transform: uppercase;
  color: #8f5d34;
}

h1,
h2,
p {
  margin: 0;
}

h1,
h2 {
  font-family: "Iowan Old Style", "Palatino Linotype", serif;
  font-weight: 700;
}

h1 {
  font-size: clamp(2.6rem, 5vw, 4.9rem);
  line-height: 0.95;
  max-width: 12ch;
}

h2 {
  font-size: 1.8rem;
}

.intro {
  margin-top: 16px;
  max-width: 48rem;
  color: #564536;
  font-size: 1.05rem;
}

.drop-zone {
  margin-top: 28px;
  padding: 28px;
  border-radius: 24px;
  border: 2px dashed rgba(143, 93, 52, 0.35);
  display: grid;
  grid-template-columns: 96px 1fr;
  gap: 20px;
  align-items: center;
  background: linear-gradient(135deg, rgba(255, 255, 255, 0.5), rgba(248, 229, 192, 0.45));
  transition:
    transform 140ms ease,
    border-color 140ms ease,
    background 140ms ease;
}

.drop-zone.dragging {
  transform: scale(1.01);
  border-color: #2e7667;
  background: linear-gradient(135deg, rgba(215, 252, 238, 0.78), rgba(253, 245, 215, 0.7));
}

.drop-zone.busy {
  opacity: 0.85;
}

.drop-mark {
  width: 96px;
  height: 96px;
  display: grid;
  place-items: center;
  border-radius: 20px;
  background: #1f5f54;
  color: #ffefcb;
  font-family: "Menlo", "SFMono-Regular", monospace;
  font-size: 1.4rem;
  font-weight: 700;
}

.drop-title {
  font-size: 1.15rem;
  font-weight: 700;
}

.drop-copy {
  margin-top: 8px;
  color: #5f4b3b;
}

.panel-head {
  display: flex;
  justify-content: space-between;
  gap: 16px;
  align-items: center;
  margin-bottom: 22px;
}

.save-button {
  border: 0;
  border-radius: 999px;
  padding: 12px 18px;
  background: #8f5d34;
  color: #fff9ee;
  font-weight: 700;
  cursor: pointer;
}

.save-button:disabled {
  cursor: default;
  opacity: 0.6;
}

.range-field,
.text-field {
  display: grid;
  gap: 10px;
}

.range-field {
  grid-template-columns: 1fr auto;
  align-items: center;
}

.range-input {
  width: 100%;
  accent-color: #1f5f54;
}

.hint {
  margin-top: 8px;
  color: #675849;
}

.ignore-grid {
  margin-top: 20px;
  display: grid;
  grid-template-columns: repeat(2, minmax(0, 1fr));
  gap: 18px;
}

textarea {
  width: 100%;
  border: 1px solid rgba(51, 44, 34, 0.14);
  border-radius: 18px;
  padding: 14px 16px;
  background: rgba(255, 255, 255, 0.68);
  color: #241e18;
  resize: vertical;
  font: inherit;
}

.result-list {
  list-style: none;
  padding: 0;
  margin: 0;
  display: grid;
  gap: 12px;
}

.result-row {
  display: grid;
  grid-template-columns: auto 1fr;
  gap: 14px;
  align-items: start;
  padding: 16px;
  border-radius: 18px;
  background: rgba(255, 255, 255, 0.58);
}

.status-pill {
  min-width: 98px;
  text-align: center;
  padding: 8px 12px;
  border-radius: 999px;
  font-size: 0.85rem;
  text-transform: capitalize;
  font-weight: 700;
}

.status-pill.compressed {
  background: #d7fce8;
  color: #145c45;
}

.status-pill.skipped {
  background: #fff1bf;
  color: #865c00;
}

.status-pill.failed {
  background: #ffd6d3;
  color: #8b2319;
}

.result-copy {
  min-width: 0;
}

.source,
.output {
  font-family: "Menlo", "SFMono-Regular", monospace;
  font-size: 0.92rem;
  overflow-wrap: anywhere;
}

.detail {
  margin: 6px 0;
  color: #5f4b3b;
}

code {
  font-family: "Menlo", "SFMono-Regular", monospace;
}

@media (max-width: 820px) {
  .shell {
    padding: 18px;
  }

  .hero,
  .settings-panel,
  .results-panel {
    padding: 22px;
  }

  .drop-zone,
  .ignore-grid,
  .result-row,
  .panel-head {
    grid-template-columns: 1fr;
  }

  .drop-mark {
    width: 72px;
    height: 72px;
  }
}
</style>
