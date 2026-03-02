use std::{
    ffi::{c_char, c_int, c_uint, c_void, CStr},
    fs,
    path::{Path, PathBuf},
};

use anyhow::{anyhow, bail, Context, Result};
use glob::Pattern;
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager};
use tar::Builder;
use walkdir::{DirEntry, WalkDir};

const DEFAULT_COMPRESSION_LEVEL: i32 = 3;
const MIN_COMPRESSION_LEVEL: i32 = 1;
const MAX_COMPRESSION_LEVEL: i32 = 22;
const SETTINGS_FILE_NAME: &str = "settings.json";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
struct CompressionSettings {
    compression_level: i32,
    ignored_files: Vec<String>,
    ignored_folders: Vec<String>,
}

impl Default for CompressionSettings {
    fn default() -> Self {
        Self {
            compression_level: DEFAULT_COMPRESSION_LEVEL,
            ignored_files: vec![
                ".DS_Store".into(),
                "Thumbs.db".into(),
                "*.tmp".into(),
                "*.temp".into(),
            ],
            ignored_folders: vec![".git".into(), "node_modules".into()],
        }
    }
}

impl CompressionSettings {
    fn normalized(&self) -> Self {
        Self {
            compression_level: self
                .compression_level
                .clamp(MIN_COMPRESSION_LEVEL, MAX_COMPRESSION_LEVEL),
            ignored_files: normalize_patterns(&self.ignored_files),
            ignored_folders: normalize_patterns(&self.ignored_folders),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct CompressionOutcome {
    source_path: String,
    output_path: Option<String>,
    status: CompressionStatus,
    detail: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
enum CompressionStatus {
    Compressed,
    Skipped,
    Failed,
}

#[derive(Debug)]
struct IgnoreRules {
    file_patterns: Vec<Pattern>,
    folder_patterns: Vec<Pattern>,
}

impl IgnoreRules {
    fn from_settings(settings: &CompressionSettings) -> Result<Self> {
        let normalized = settings.normalized();

        Ok(Self {
            file_patterns: compile_patterns(&normalized.ignored_files)?,
            folder_patterns: compile_patterns(&normalized.ignored_folders)?,
        })
    }

    fn ignores_root(&self, path: &Path) -> bool {
        let name = path.file_name().and_then(|value| value.to_str()).unwrap_or("");
        if path.is_dir() {
            matches_any_pattern(&self.folder_patterns, name, name)
        } else {
            matches_any_pattern(&self.file_patterns, name, name)
        }
    }

    fn ignores_entry(&self, root: &Path, entry: &DirEntry) -> bool {
        if entry.depth() == 0 {
            return false;
        }

        let relative = entry.path().strip_prefix(root).unwrap_or(entry.path());
        let relative = slash_path(relative);
        let name = entry.file_name().to_string_lossy();

        if entry.file_type().is_dir() {
            matches_any_pattern(&self.folder_patterns, &relative, &name)
        } else if entry.file_type().is_file() {
            matches_any_pattern(&self.file_patterns, &relative, &name)
        } else {
            false
        }
    }
}

#[derive(Debug)]
#[repr(C)]
struct ZstdCCtx {
    _private: [u8; 0],
}

#[link(name = "zstd", kind = "static")]
unsafe extern "C" {
    fn ZSTD_compressBound(src_size: usize) -> usize;
    fn ZSTD_isError(code: usize) -> c_uint;
    fn ZSTD_getErrorName(code: usize) -> *const c_char;
    fn ZSTD_createCCtx() -> *mut ZstdCCtx;
    fn ZSTD_freeCCtx(context: *mut ZstdCCtx) -> usize;
    fn ZSTD_compressCCtx(
        context: *mut ZstdCCtx,
        dst: *mut c_void,
        dst_capacity: usize,
        src: *const c_void,
        src_size: usize,
        compression_level: c_int,
    ) -> usize;
}

#[tauri::command]
fn load_settings(app: AppHandle) -> Result<CompressionSettings, String> {
    load_settings_from_disk(&app).map_err(error_message)
}

#[tauri::command]
fn save_settings(app: AppHandle, settings: CompressionSettings) -> Result<CompressionSettings, String> {
    let normalized = settings.normalized();
    save_settings_to_disk(&app, &normalized).map_err(error_message)?;
    Ok(normalized)
}

#[tauri::command]
async fn compress_paths(
    app: AppHandle,
    paths: Vec<String>,
    settings: CompressionSettings,
) -> Result<Vec<CompressionOutcome>, String> {
    let normalized = settings.normalized();
    save_settings_to_disk(&app, &normalized).map_err(error_message)?;

    tauri::async_runtime::spawn_blocking(move || {
        let path_bufs = paths.into_iter().map(PathBuf::from).collect::<Vec<_>>();
        compress_many(path_bufs, &normalized)
    })
    .await
    .map_err(|error| error.to_string())?
    .map_err(error_message)
}

fn compress_many(paths: Vec<PathBuf>, settings: &CompressionSettings) -> Result<Vec<CompressionOutcome>> {
    let rules = IgnoreRules::from_settings(settings)?;
    let mut outcomes = Vec::with_capacity(paths.len());

    for path in paths {
        let source_path = path.display().to_string();

        let outcome = match compress_one(&path, settings, &rules) {
            Ok(outcome) => outcome,
            Err(error) => CompressionOutcome {
                source_path,
                output_path: None,
                status: CompressionStatus::Failed,
                detail: error.to_string(),
            },
        };

        outcomes.push(outcome);
    }

    Ok(outcomes)
}

fn compress_one(
    source: &Path,
    settings: &CompressionSettings,
    rules: &IgnoreRules,
) -> Result<CompressionOutcome> {
    if !source.exists() {
        bail!("Path does not exist");
    }

    if rules.ignores_root(source) {
        return Ok(CompressionOutcome {
            source_path: source.display().to_string(),
            output_path: None,
            status: CompressionStatus::Skipped,
            detail: "Skipped because it matches the global ignore rules.".into(),
        });
    }

    let output_path = next_output_path(source)?;
    let source_bytes = if source.is_dir() {
        archive_directory(source, rules)?
    } else if source.is_file() {
        fs::read(source).with_context(|| format!("Failed to read {}", source.display()))?
    } else {
        bail!("Only files and folders can be compressed");
    };

    let compressed_bytes = compress_bytes(&source_bytes, settings.compression_level)?;
    fs::write(&output_path, compressed_bytes)
        .with_context(|| format!("Failed to write {}", output_path.display()))?;

    Ok(CompressionOutcome {
        source_path: source.display().to_string(),
        output_path: Some(output_path.display().to_string()),
        status: CompressionStatus::Compressed,
        detail: if source.is_dir() {
            "Compressed folder into a .tar.zst archive.".into()
        } else {
            "Compressed file into a .zst archive.".into()
        },
    })
}

fn archive_directory(source: &Path, rules: &IgnoreRules) -> Result<Vec<u8>> {
    let root_name = source
        .file_name()
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("archive"));
    let mut builder = Builder::new(Vec::new());
    builder
        .append_dir(&root_name, source)
        .with_context(|| format!("Failed to archive {}", source.display()))?;

    let walker = WalkDir::new(source)
        .min_depth(1)
        .into_iter()
        .filter_entry(|entry| !rules.ignores_entry(source, entry));

    for entry in walker {
        let entry = entry.with_context(|| format!("Failed while walking {}", source.display()))?;
        let relative = entry
            .path()
            .strip_prefix(source)
            .with_context(|| format!("Failed to compute archive path for {}", entry.path().display()))?;
        let archive_path = root_name.join(relative);

        if entry.file_type().is_dir() {
            builder
                .append_dir(&archive_path, entry.path())
                .with_context(|| format!("Failed to append {} to archive", entry.path().display()))?;
        } else if entry.file_type().is_file() {
            builder
                .append_path_with_name(entry.path(), &archive_path)
                .with_context(|| format!("Failed to append {} to archive", entry.path().display()))?;
        }
    }

    builder
        .into_inner()
        .context("Failed to finish tar archive")
}

fn compress_bytes(source: &[u8], compression_level: i32) -> Result<Vec<u8>> {
    let context = unsafe { ZSTD_createCCtx() };
    if context.is_null() {
        bail!("Failed to create zstd context");
    }

    let bound = zstd_result(unsafe { ZSTD_compressBound(source.len()) })?;
    let mut output = vec![0u8; bound];
    let written_result = zstd_result(unsafe {
        ZSTD_compressCCtx(
            context,
            output.as_mut_ptr().cast(),
            output.len(),
            source.as_ptr().cast(),
            source.len(),
            compression_level.clamp(MIN_COMPRESSION_LEVEL, MAX_COMPRESSION_LEVEL),
        )
    });

    let _ = unsafe { ZSTD_freeCCtx(context) };
    let written = written_result?;
    output.truncate(written);
    Ok(output)
}

fn zstd_result(result: usize) -> Result<usize> {
    if unsafe { ZSTD_isError(result) } == 0 {
        return Ok(result);
    }

    let message = unsafe {
        let message_ptr = ZSTD_getErrorName(result);
        CStr::from_ptr(message_ptr).to_string_lossy().into_owned()
    };

    Err(anyhow!(message))
}

fn load_settings_from_disk(app: &AppHandle) -> Result<CompressionSettings> {
    let path = settings_path(app)?;
    if !path.exists() {
        return Ok(CompressionSettings::default());
    }

    let raw = fs::read_to_string(&path)
        .with_context(|| format!("Failed to read {}", path.display()))?;
    let settings: CompressionSettings =
        serde_json::from_str(&raw).with_context(|| format!("Invalid JSON in {}", path.display()))?;

    Ok(settings.normalized())
}

fn save_settings_to_disk(app: &AppHandle, settings: &CompressionSettings) -> Result<()> {
    let path = settings_path(app)?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("Failed to create {}", parent.display()))?;
    }

    let payload =
        serde_json::to_string_pretty(settings).context("Failed to serialize compression settings")?;
    fs::write(&path, payload).with_context(|| format!("Failed to write {}", path.display()))?;
    Ok(())
}

fn settings_path(app: &AppHandle) -> Result<PathBuf> {
    Ok(app.path().app_config_dir()?.join(SETTINGS_FILE_NAME))
}

fn next_output_path(source: &Path) -> Result<PathBuf> {
    let parent = source
        .parent()
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    let file_name = source
        .file_name()
        .and_then(|value| value.to_str())
        .ok_or_else(|| anyhow!("Path has no valid file name"))?;

    let extension = if source.is_dir() { ".tar.zst" } else { ".zst" };

    for index in 0.. {
        let candidate_name = if index == 0 {
            format!("{file_name}{extension}")
        } else {
            format!("{file_name} ({index}){extension}")
        };
        let candidate = parent.join(candidate_name);

        if !candidate.exists() {
            return Ok(candidate);
        }
    }

    unreachable!("Infinite iterator should always return");
}

fn compile_patterns(patterns: &[String]) -> Result<Vec<Pattern>> {
    patterns
        .iter()
        .map(|pattern| {
            Pattern::new(pattern)
                .with_context(|| format!("Invalid ignore pattern: {pattern}"))
        })
        .collect()
}

fn normalize_patterns(patterns: &[String]) -> Vec<String> {
    let mut normalized = Vec::new();

    for pattern in patterns {
        let trimmed = pattern.trim();
        if trimmed.is_empty() || normalized.iter().any(|existing| existing == trimmed) {
            continue;
        }
        normalized.push(trimmed.to_string());
    }

    normalized
}

fn matches_any_pattern(patterns: &[Pattern], relative: &str, name: &str) -> bool {
    patterns.iter().any(|pattern| {
        let pattern_text = pattern.as_str();
        if pattern_text.contains('/') || pattern_text.contains('\\') {
            pattern.matches(relative)
        } else {
            pattern.matches(name)
        }
    })
}

fn slash_path(path: &Path) -> String {
    path.components()
        .map(|component| component.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

fn error_message(error: anyhow::Error) -> String {
    error.to_string()
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .invoke_handler(tauri::generate_handler![
            load_settings,
            save_settings,
            compress_paths
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;
    use tar::Archive;
    use tempfile::tempdir;

    #[test]
    fn normalize_settings_clamps_level_and_deduplicates_patterns() {
        let settings = CompressionSettings {
            compression_level: 99,
            ignored_files: vec!["  *.tmp  ".into(), "".into(), "*.tmp".into()],
            ignored_folders: vec![" node_modules ".into(), "node_modules".into()],
        };

        let normalized = settings.normalized();

        assert_eq!(normalized.compression_level, MAX_COMPRESSION_LEVEL);
        assert_eq!(normalized.ignored_files, vec!["*.tmp"]);
        assert_eq!(normalized.ignored_folders, vec!["node_modules"]);
    }

    #[test]
    fn ignore_rules_match_file_names_and_relative_paths() {
        let settings = CompressionSettings {
            compression_level: DEFAULT_COMPRESSION_LEVEL,
            ignored_files: vec!["*.tmp".into(), "nested/*.cache".into()],
            ignored_folders: vec![],
        };
        let rules = IgnoreRules::from_settings(&settings).unwrap();

        let file_patterns = &rules.file_patterns;
        assert!(matches_any_pattern(file_patterns, "draft.tmp", "draft.tmp"));
        assert!(matches_any_pattern(
            file_patterns,
            "nested/data.cache",
            "data.cache"
        ));
        assert!(!matches_any_pattern(
            file_patterns,
            "nested/data.txt",
            "data.txt"
        ));
    }

    #[test]
    fn ignore_rules_match_folder_names_and_nested_paths() {
        let settings = CompressionSettings {
            compression_level: DEFAULT_COMPRESSION_LEVEL,
            ignored_files: vec![],
            ignored_folders: vec!["node_modules".into(), "build/cache".into()],
        };
        let rules = IgnoreRules::from_settings(&settings).unwrap();

        assert!(matches_any_pattern(
            &rules.folder_patterns,
            "node_modules",
            "node_modules"
        ));
        assert!(matches_any_pattern(
            &rules.folder_patterns,
            "build/cache",
            "cache"
        ));
        assert!(!matches_any_pattern(
            &rules.folder_patterns,
            "build/output",
            "output"
        ));
    }

    #[test]
    fn archive_directory_skips_ignored_entries() {
        let temp_dir = tempdir().unwrap();
        let root = temp_dir.path().join("demo");
        fs::create_dir_all(root.join("node_modules/package")).unwrap();
        fs::create_dir_all(root.join("keep/subdir")).unwrap();
        fs::write(root.join(".DS_Store"), "ignore me").unwrap();
        fs::write(root.join("keep/file.txt"), "keep me").unwrap();
        fs::write(root.join("node_modules/package/index.js"), "skip me").unwrap();

        let settings = CompressionSettings {
            compression_level: DEFAULT_COMPRESSION_LEVEL,
            ignored_files: vec![".DS_Store".into()],
            ignored_folders: vec!["node_modules".into()],
        };
        let rules = IgnoreRules::from_settings(&settings).unwrap();

        let archive = archive_directory(&root, &rules).unwrap();
        let mut names = Archive::new(Cursor::new(archive))
            .entries()
            .unwrap()
            .map(|entry| entry.unwrap().path().unwrap().display().to_string())
            .collect::<Vec<_>>();
        names.sort();

        assert!(names.contains(&"demo".to_string()));
        assert!(names.contains(&"demo/keep".to_string()));
        assert!(names.contains(&"demo/keep/file.txt".to_string()));
        assert!(!names.iter().any(|name| name.contains(".DS_Store")));
        assert!(!names.iter().any(|name| name.contains("node_modules")));
    }

    #[test]
    fn next_output_path_uses_zst_extensions_and_avoids_collisions() {
        let temp_dir = tempdir().unwrap();
        let file_path = temp_dir.path().join("report.txt");
        let dir_path = temp_dir.path().join("photos");

        fs::write(&file_path, "hello").unwrap();
        fs::create_dir_all(&dir_path).unwrap();
        fs::write(temp_dir.path().join("report.txt.zst"), "taken").unwrap();
        fs::write(temp_dir.path().join("photos.tar.zst"), "taken").unwrap();

        let file_output = next_output_path(&file_path).unwrap();
        let dir_output = next_output_path(&dir_path).unwrap();

        assert_eq!(file_output.file_name().unwrap(), "report.txt (1).zst");
        assert_eq!(dir_output.file_name().unwrap(), "photos (1).tar.zst");
    }
}
